use crate::bit_reader::BitWriter;
use crate::filter::FilterResult;
use crate::object::stream::{ImageColorSpace, ImageData, ImageDecodeParams};
use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;
use hayro_jpeg2000::{ColorSpace, DecodeSettings};

impl ImageColorSpace {
    fn num_components(&self) -> u8 {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
            Self::Unknown(num) => *num,
        }
    }
}

/// quail: JP2 file signature box, as hayro-jpeg2000 recognizes it.
const JP2_MAGIC: &[u8] = b"\x00\x00\x00\x0C\x6A\x50\x20\x20";
/// quail: SOC followed by SIZ: the start of every codestream.
const SOC_SIZ: &[u8] = &[0xFF, 0x4F, 0xFF, 0x51];
/// quail: the contiguous codestream box type, `jp2c`.
const JP2C: u32 = 0x6A70_3263;

/// quail: the image sizes the decoder will read fit the caps, checked before it allocates.
/// The data is read the way hayro-jpeg2000 reads it: a raw codestream starts with SOC + SIZ;
/// a JP2 file is a sequence of top-level boxes (lenient lengths: 0 = to the end, 1 = 64-bit
/// length, a length past the end takes the rest), and the decoder parses every `jp2c` box,
/// so the SIZ at the start of every one must fit. A SIZ found anywhere else (a `uuid` or
/// `xml` box) is not what gets decoded. `Some(false)`: a size over the caps; `None`: neither
/// format, or a `jp2c` box without SOC + SIZ (corrupt, not a limit).
pub(crate) fn header_fits(data: &[u8], limits: &crate::budget::DecodeLimits<'_>) -> Option<bool> {
    if data.starts_with(SOC_SIZ) {
        return siz_fits(data, limits);
    }
    if !data.starts_with(JP2_MAGIC) {
        return None;
    }
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let mut rest = data;
    let mut codestreams = 0usize;
    while rest.len() >= 8 {
        let (l_box, t_box) = (be32(&rest[..4]), be32(&rest[4..8]));
        let (header, len) = match l_box {
            0 => (8, rest.len() - 8),
            1 => {
                let Some(xl) = rest.get(8..16) else { break };
                let xl =
                    u64::from_be_bytes([xl[0], xl[1], xl[2], xl[3], xl[4], xl[5], xl[6], xl[7]]);
                let Some(len) = xl.checked_sub(16) else { break };
                (16, usize::try_from(len).unwrap_or(usize::MAX))
            }
            // Lengths 2..=7 make the decoder stop reading boxes.
            l if l < 8 => break,
            l => (8, (l - 8) as usize),
        };
        let body = &rest[header..];
        let len = len.min(body.len());
        if t_box == JP2C {
            codestreams += 1;
            match siz_fits(&body[..len], limits) {
                Some(true) => {}
                other => return other,
            }
        }
        rest = &body[len..];
    }
    (codestreams > 0).then_some(true)
}

/// quail: the SIZ marker at the start of `codestream` (after SOC) fits the caps.
fn siz_fits(codestream: &[u8], limits: &crate::budget::DecodeLimits<'_>) -> Option<bool> {
    if !codestream.starts_with(SOC_SIZ) {
        return None;
    }
    let siz = codestream.get(4..4 + 38)?;
    let be32 = |o: usize| u32::from_be_bytes([siz[o], siz[o + 1], siz[o + 2], siz[o + 3]]);
    // Lsiz(2) Rsiz(2) Xsiz Ysiz XOsiz YOsiz XTsiz YTsiz XTOsiz YTOsiz Csiz(2)
    let w = be32(4).saturating_sub(be32(12));
    let h = be32(8).saturating_sub(be32(16));
    let comps = u64::from(u16::from_be_bytes([siz[36], siz[37]]));
    // Decoders keep samples wider than a byte: charge 4 bytes per sample.
    Some(comps > 0 && limits.image_fits(w, h, comps.saturating_mul(4)))
}

pub(crate) fn decode(data: &[u8], params: &ImageDecodeParams) -> Option<FilterResult<'static>> {
    use crate::object::stream::ImageColorSpace;

    let settings = DecodeSettings {
        resolve_palette_indices: false,
        strict: false,
        target_resolution: params.target_dimension,
    };

    let image = hayro_jpeg2000::Image::new(data, &settings).ok()?;

    let width = image.width();
    let height = image.height();
    let bpc = if params.is_indexed {
        params.bpc.unwrap_or(image.original_bit_depth())
    } else {
        8
    };
    let cs = match image.color_space() {
        ColorSpace::Gray => ImageColorSpace::Gray,
        ColorSpace::RGB => ImageColorSpace::Rgb,
        ColorSpace::CMYK => ImageColorSpace::Cmyk,
        ColorSpace::Unknown { num_channels } => ImageColorSpace::Unknown(*num_channels),
        ColorSpace::Icc {
            num_channels: num_components,
            ..
        } => match num_components {
            1 => ImageColorSpace::Gray,
            3 => ImageColorSpace::Rgb,
            4 => ImageColorSpace::Cmyk,
            _ => return None,
        },
    };
    let has_alpha = image.has_alpha();
    let mut decoder_context = hayro_jpeg2000::DecoderContext::default();
    let bitmap = image.decode(&mut decoder_context).ok()?.data_u8();

    let (mut data, mut alpha) = if !has_alpha {
        (bitmap, None)
    } else {
        // Extract the alpha channel.
        let total_channels = cs.num_components() + 1;
        let mut color_channels = Vec::with_capacity(
            (bitmap.len() / total_channels as usize) * cs.num_components() as usize,
        );
        let mut alpha_channel = Vec::with_capacity(bitmap.len() / total_channels as usize);

        for sample in bitmap.chunks_exact(total_channels as usize) {
            let (alpha, color) = sample.split_last()?;
            alpha_channel.push(*alpha);
            color_channels.extend_from_slice(color);
        }

        (color_channels, Some(alpha_channel))
    };

    // The decoded image is always 8-bit, so if necessary we have to rescale
    // ourselves.
    if bpc != 8 {
        data = scale(&data, bpc, cs.num_components(), width, height)?;
        alpha = alpha.and_then(|alpha| scale(&alpha, bpc, cs.num_components(), width, height));
    }

    Some(FilterResult {
        data: Cow::Owned(data),
        image_data: Some(ImageData {
            alpha,
            color_space: Some(cs),
            bits_per_component: bpc,
            width,
            height,
        }),
    })
}

fn scale(
    data: &[u8],
    bit_per_component: u8,
    num_components: u8,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    if bit_per_component == 0 || bit_per_component > 32 {
        return None;
    }

    let div_factor = ((1 << 8) - 1) as f32;
    let mul_factor = ((1_u32 << bit_per_component) - 1) as f32;

    let bits_per_row = (width as usize)
        .checked_mul(num_components as usize)?
        .checked_mul(bit_per_component as usize)?;
    let input_len = bits_per_row.div_ceil(8).checked_mul(height as usize)?;
    let mut input = vec![0; input_len];
    let mut writer = BitWriter::new(&mut input, bit_per_component)?;
    let components_per_row = (num_components as usize).checked_mul(width as usize)?;

    for bytes in data.chunks_exact(components_per_row) {
        for byte in bytes {
            let scaled = ((*byte as f32 / div_factor) * mul_factor + 0.5) as u32;
            writer.write(scaled)?;
        }

        writer.align();
    }

    let final_pos = writer.cur_pos();
    input.truncate(final_pos);

    Some(input)
}
