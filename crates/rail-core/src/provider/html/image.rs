//! An `<img>` tag as the styled scan lists it: its address, its alt text and the size it asks
//! for in CSS pixels, from `width` and `height` or the inline `style`. The tag is read again
//! within its byte cap, once per attribute, as an `<a>` is for `href`, so the scan stays
//! linear. Nothing is fetched: what the reader does with the address is its own business.

use std::fmt;

use super::{attribute, decode_value};
use crate::provider::gmail::body::clean;

/// Images listed per message; later ones are left out.
pub const MAX_IMAGES: usize = 200;
/// The longest alt text kept, in characters.
pub const MAX_ALT_CHARS: usize = 200;
/// The largest side Rail believes, in CSS pixels.
const MAX_SIDE: u32 = 10_000;

/// One `<img>`: it stands before run `at` of the styled text (`runs.len()` after the last).
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub at: usize,
    /// The address as written, character references decoded: `cid:`, `https:` or any other.
    pub src: String,
    /// Outside text, cleaned of control and format characters, white space collapsed.
    pub alt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Place and sizes only: the address and alt text are mail, which must never reach a log.
impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("at", &self.at)
            .field("src_len", &self.src.len())
            .field("alt_len", &self.alt.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

/// The image of a tag whose attributes are `attributes`; `None` without an address (or with
/// one too long to be any Rail keeps, such as a large `data:` image).
pub(super) fn read(attributes: &[u8]) -> Option<Image> {
    let get = |name: &[u8]| attribute(attributes, name).and_then(decode_value);
    let src = get(b"src").filter(|s| !s.is_empty())?;
    let alt = get(b"alt").map_or_else(String::new, |a| alt_text(&a));
    let style = get(b"style").unwrap_or_default();
    let side = |name: &str| {
        get(name.as_bytes())
            .and_then(|v| pixels(&v))
            .or_else(|| style_pixels(&style, name))
    };
    Some(Image {
        at: 0,
        width: side("width"),
        height: side("height"),
        src,
        alt,
    })
}

fn alt_text(raw: &str) -> String {
    clean(raw)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_ALT_CHARS)
        .collect()
}

/// `32`, `32px` or `32.5` as whole pixels; not `50%`, `auto` or `0`.
fn pixels(value: &str) -> Option<u32> {
    let value = value.trim();
    let number = value
        .strip_suffix("px")
        .or_else(|| value.strip_suffix("PX"))
        .unwrap_or(value)
        .trim();
    let n = number.parse::<f64>().ok().filter(|n| n.is_finite())?;
    (n >= 1.0).then(|| n.round().min(f64::from(MAX_SIDE)) as u32)
}

/// `width: 32px` out of an inline style; `max-width` and the like are not it.
fn style_pixels(style: &str, key: &str) -> Option<u32> {
    style.split(';').find_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case(key)
            .then(|| pixels(value))
            .flatten()
    })
}
