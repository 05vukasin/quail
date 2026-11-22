//! Results from decoding filtered data streams.

mod ascii_85;
pub(crate) mod ascii_hex;
#[cfg(feature = "images")]
mod ccitt;
#[cfg(feature = "images")]
mod dct;
#[cfg(feature = "images")]
mod jbig2;
#[cfg(feature = "images")]
mod jpx;
mod lzw_flate;
mod png;
mod run_length;

use crate::object::Dict;
use crate::object::Name;
use crate::object::dict::keys::*;
use crate::object::stream::{DecodeFailure, FilterResult, ImageDecodeParams};
use core::ops::Deref;

/// A data filter.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Filter {
    /// ASCII hexadecimal encoding.
    AsciiHexDecode,
    /// ASCII base-85 encoding.
    Ascii85Decode,
    /// Lempel-Ziv-Welch (LZW) compression.
    LzwDecode,
    /// DEFLATE compression (zlib/gzip).
    FlateDecode,
    /// Run-length encoding compression.
    RunLengthDecode,
    /// CCITT Group 3 or Group 4 fax compression.
    CcittFaxDecode,
    /// JBIG2 compression for bi-level images.
    Jbig2Decode,
    /// JPEG (DCT) compression.
    DctDecode,
    /// JPEG 2000 compression.
    JpxDecode,
    /// Encryption filter.
    Crypt,
}

impl Filter {
    pub(crate) fn from_name(name: Name<'_>) -> Option<Self> {
        match name.deref() {
            ASCII_HEX_DECODE | ASCII_HEX_DECODE_ABBREVIATION => Some(Self::AsciiHexDecode),
            ASCII85_DECODE | ASCII85_DECODE_ABBREVIATION => Some(Self::Ascii85Decode),
            LZW_DECODE | LZW_DECODE_ABBREVIATION => Some(Self::LzwDecode),
            FLATE_DECODE | FLATE_DECODE_ABBREVIATION => Some(Self::FlateDecode),
            RUN_LENGTH_DECODE | RUN_LENGTH_DECODE_ABBREVIATION => Some(Self::RunLengthDecode),
            CCITTFAX_DECODE | CCITTFAX_DECODE_ABBREVIATION => Some(Self::CcittFaxDecode),
            JBIG2_DECODE => Some(Self::Jbig2Decode),
            DCT_DECODE | DCT_DECODE_ABBREVIATION => Some(Self::DctDecode),
            JPX_DECODE => Some(Self::JpxDecode),
            CRYPT => Some(Self::Crypt),
            _ => None,
        }
    }

    pub(crate) fn apply(
        &self,
        data: &[u8],
        params: &Dict<'_>,
        #[cfg_attr(not(feature = "images"), allow(unused))] image_params: &ImageDecodeParams,
        // quail: output and pixel caps for this layer.
        limits: &crate::budget::DecodeLimits<'_>,
    ) -> Result<FilterResult<'static>, DecodeFailure> {
        // quail: generic data filters stop just past `max_out`; a longer result is refused
        // and recorded as a limit hit.
        let max_out = usize::try_from(limits.max_out).unwrap_or(usize::MAX);
        let capped = |d: Option<Vec<u8>>| match d {
            Some(d) if d.len() > max_out => {
                limits.refuse();
                Err(DecodeFailure::StreamDecode)
            }
            Some(d) => Ok(FilterResult::from_data(d)),
            None => Err(DecodeFailure::StreamDecode),
        };
        // quail: image filters must fit the caps, as declared by the image dictionary and by
        // the codestream headers, before any decoder allocates.
        #[cfg(feature = "images")]
        let refuse_image = || {
            limits.refuse();
            Err(DecodeFailure::ImageDecode)
        };
        #[cfg(feature = "images")]
        let declared_fits = || {
            let comps = u64::from(image_params.num_components.unwrap_or(4).max(1));
            limits.image_fits(image_params.width, image_params.height, comps)
        };
        match self {
            Self::AsciiHexDecode => capped(ascii_hex::decode(data)),
            Self::Ascii85Decode => capped(ascii_85::decode(data, max_out)),
            Self::RunLengthDecode => capped(run_length::decode(data, max_out)),
            Self::LzwDecode => capped(lzw_flate::lzw::decode(data, params, max_out)),
            Self::FlateDecode => capped(lzw_flate::flate::decode(data, params, max_out)),
            #[cfg(feature = "images")]
            Self::DctDecode => {
                match dct::header_fits(data, limits) {
                    _ if !declared_fits() => return refuse_image(),
                    Some(false) => return refuse_image(),
                    // quail: no frame header: corrupt, not a limit.
                    None => return Err(DecodeFailure::ImageDecode),
                    Some(true) => {}
                }
                dct::decode(data, params, image_params).ok_or(DecodeFailure::ImageDecode)
            }
            #[cfg(feature = "images")]
            Self::CcittFaxDecode => {
                if !declared_fits() {
                    return refuse_image();
                }
                ccitt::decode(data, params, image_params, limits).ok_or(DecodeFailure::ImageDecode)
            }
            #[cfg(feature = "images")]
            Self::Jbig2Decode => {
                if !declared_fits() {
                    return refuse_image();
                }
                jbig2::decode(data, params, image_params, limits).ok_or(DecodeFailure::ImageDecode)
            }
            #[cfg(feature = "images")]
            Self::JpxDecode => {
                match jpx::header_fits(data, limits) {
                    // quail: the dictionary's size too, as for the other image filters.
                    _ if !declared_fits() => return refuse_image(),
                    Some(false) => return refuse_image(),
                    // quail: neither a codestream nor a JP2 file with one: corrupt.
                    None => return Err(DecodeFailure::ImageDecode),
                    Some(true) => {}
                }
                jpx::decode(data, image_params).ok_or(DecodeFailure::ImageDecode)
            }
            #[cfg(not(feature = "images"))]
            Self::DctDecode | Self::CcittFaxDecode | Self::Jbig2Decode | Self::JpxDecode => {
                Err(DecodeFailure::ImageDecode)
            }
            _ => Err(DecodeFailure::StreamDecode),
        }
    }
}
