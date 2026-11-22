//! Header and footer detection: geometry (top/bottom band of the page) and repetition of the
//! same text, digits ignored, in the same band on several pages.

use std::collections::HashMap;

use quail_core::{BBox, Zone};

/// Blocks entirely inside this top/bottom share of the page are headers/footers by geometry.
const TIGHT_BAND: f32 = 0.06;
/// Blocks inside this wider band are headers/footers when they repeat across pages.
const WIDE_BAND: f32 = 0.15;
/// A text repeats when it occurs on at least this many pages...
const MIN_REPEATS: usize = 2;
/// ...and on at least this share of the pages (for documents of 4 pages or more).
const MIN_REPEAT_SHARE: f32 = 0.3;

/// One block as seen by the zoner.
pub(crate) struct ZoneInput<'a> {
    pub text: &'a str,
    pub bbox: BBox,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Band {
    Top,
    Bottom,
}

/// Digits become `#`, case and whitespace are folded: "Page 3 of 10" == "Page 4 of 10".
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(256));
    let mut space = false;
    for c in text.chars().take(200) {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        if c.is_ascii_digit() {
            if !out.ends_with('#') {
                out.push('#');
            }
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

fn band(b: &BBox, height: f32, share: f32) -> Option<Band> {
    if !(height.is_finite() && height > 0.0) {
        return None;
    }
    if b.y1 <= height * share {
        Some(Band::Top)
    } else if b.y0 >= height * (1.0 - share) {
        Some(Band::Bottom)
    } else {
        None
    }
}

/// Zones for every block of every page. `pages[i]` is (page height, blocks).
pub(crate) fn assign(pages: &[(f32, Vec<ZoneInput<'_>>)]) -> Vec<Vec<Zone>> {
    // Pages on which each (band, normalized text) occurs.
    let mut seen: HashMap<(Band, String), usize> = HashMap::new();
    for (h, blocks) in pages {
        let mut on_page: Vec<(Band, String)> = blocks
            .iter()
            .filter_map(|b| band(&b.bbox, *h, WIDE_BAND).map(|band| (band, normalize(b.text))))
            .filter(|(_, t)| !t.is_empty())
            .collect();
        // Sorted by (band, text), so `dedup` sees every repeat: a page counts once per key.
        on_page.sort();
        on_page.dedup();
        for k in on_page {
            *seen.entry(k).or_default() += 1;
        }
    }
    let n = pages.len();
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let needed = if n >= 4 {
        MIN_REPEATS.max((MIN_REPEAT_SHARE * n as f32).ceil() as usize)
    } else {
        MIN_REPEATS
    };
    pages
        .iter()
        .map(|(h, blocks)| {
            blocks
                .iter()
                .map(|b| {
                    let repeated = band(&b.bbox, *h, WIDE_BAND).filter(|band| {
                        seen.get(&(*band, normalize(b.text)))
                            .is_some_and(|&c| n > 1 && c >= needed)
                    });
                    match repeated.or_else(|| band(&b.bbox, *h, TIGHT_BAND)) {
                        Some(Band::Top) => Zone::Header,
                        Some(Band::Bottom) => Zone::Footer,
                        None => Zone::Body,
                    }
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(y0: f32, y1: f32) -> BBox {
        BBox {
            x0: 72.0,
            y0,
            x1: 300.0,
            y1,
        }
    }

    #[test]
    fn repetition_and_geometry() {
        let page = |n: usize| {
            vec![
                ZoneInput {
                    text: "ACME Corp confidential",
                    bbox: b(80.0, 95.0), // 10–12 % of 792: only the wide band
                },
                ZoneInput {
                    text: "body text",
                    bbox: b(300.0, 320.0),
                },
                ZoneInput {
                    text: if n == 0 { "Page 1 of 3" } else { "Page 2 of 3" },
                    bbox: b(700.0, 712.0),
                },
            ]
        };
        let pages: Vec<_> = (0..3).map(|i| (792.0, page(i))).collect();
        let z = assign(&pages);
        assert_eq!(z[1], [Zone::Header, Zone::Body, Zone::Footer]);
    }

    #[test]
    fn single_page_uses_geometry_only() {
        let pages = vec![(
            792.0,
            vec![
                ZoneInput {
                    text: "letterhead",
                    bbox: b(20.0, 40.0),
                },
                ZoneInput {
                    text: "not repeated",
                    bbox: b(80.0, 95.0),
                },
            ],
        )];
        assert_eq!(assign(&pages)[0], [Zone::Header, Zone::Body]);
    }

    #[test]
    fn a_page_counts_once_per_band_and_text() {
        // The same text twice in the top band and once in the bottom band of one page, and a
        // second page without it: no repetition across pages, so no header or footer.
        let block = |y0, y1| ZoneInput {
            text: "same words",
            bbox: b(y0, y1),
        };
        let pages = vec![
            (
                792.0,
                vec![block(80.0, 95.0), block(700.0, 712.0), block(82.0, 97.0)],
            ),
            (
                792.0,
                vec![ZoneInput {
                    text: "body text",
                    bbox: b(300.0, 320.0),
                }],
            ),
        ];
        assert_eq!(assign(&pages)[0], [Zone::Body, Zone::Body, Zone::Body]);
    }

    #[test]
    fn normalize_folds_digits_and_space() {
        assert_eq!(normalize("Page  12\nof 30"), "page # of #");
    }
}
