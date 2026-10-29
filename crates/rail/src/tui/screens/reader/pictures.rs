//! Inline pictures in rich mode: the `cid:` images of an HTML body, each fetched by the daemon
//! (`attachment.image`), decoded here off the UI thread under memory limits, and sized as a
//! browser would draw it: the size the mail declares (or the picture's own), at
//! `CSS_PX_PER_COL` CSS pixels to a column, rows from the terminal cell's shape, shrunk to the
//! pane and `MAX_PICTURE_ROWS`. A picture of `MAX_HIDDEN_PX` or less on a side is a tracking
//! pixel or a spacer and takes no room. Only `cid:` pictures: remote ones wait for stage 3.
//! A reader asks for at most `MAX_PICTURES_PER_READER`, `MAX_PICTURES_IN_FLIGHT` at a time,
//! and keeps each shrunk to `MAX_KEPT_PIXELS`, so one mail cannot fill memory.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use image::{DynamicImage, ImageReader, Limits};
use rail_core::provider::html::Image;
use rail_core::store::view::BodyState;
use ratatui::layout::Size;
use ratatui_image::picker::Picker;
use ratatui_image::sliced::SlicedProtocol;
use ratatui_image::{FilterType, Resize};

use super::Reader;

/// CSS pixels to one terminal column, as a mail client's reading pane draws them.
const CSS_PX_PER_COL: f64 = 8.0;
/// The most rows one picture takes.
pub const MAX_PICTURE_ROWS: u16 = 30;
/// A side this small or smaller hides the picture.
const MAX_HIDDEN_PX: u32 = 4;
/// The largest side Rail decodes, in pixels.
const MAX_DECODED_SIDE: u32 = 8192;
/// The most memory one decode may take.
const MAX_DECODE_ALLOC: u64 = 64 * 1024 * 1024;
/// The most pixels a decoded picture keeps (4 MiB as RGBA): `MAX_PICTURE_ROWS` of even
/// 32-pixel cells is under a thousand pixels, so more than the reader ever draws.
pub const MAX_KEPT_PIXELS: u64 = 1024 * 1024;
/// The most pictures one open reader asks for; the rest stay their placeholders.
pub const MAX_PICTURES_PER_READER: usize = 32;
/// The most pictures one reader has being fetched and decoded at once.
pub const MAX_PICTURES_IN_FLIGHT: usize = 4;
/// The longest Content-ID, as the store keeps it.
const MAX_CONTENT_ID_BYTES: usize = 255;

/// One `cid:` picture of a body: before run `at`, the part whose Content-ID is `content_id`.
#[derive(Clone, PartialEq, Eq)]
pub struct Inline {
    pub at: usize,
    pub content_id: String,
    /// Outside text, as the scanner cleaned it.
    pub alt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// A picture decoded and shrunk to `MAX_KEPT_PIXELS`; `natural` is its own size, which the
/// sizing goes by.
pub struct DecodedPicture {
    pub image: DynamicImage,
    pub natural: (u32, u32),
}

/// Where a picture is: asked for, drawn, or not to be had (the placeholder stays).
#[derive(Clone)]
pub enum Picture {
    Loading,
    Ready(Rc<DecodedPicture>),
    Failed,
}

/// The `cid:` pictures among `images`, tiny declared ones left out.
pub fn inline(images: Vec<Image>) -> Vec<Inline> {
    images
        .into_iter()
        .filter(|image| {
            let tiny = |side: Option<u32>| side.is_some_and(|px| px <= MAX_HIDDEN_PX);
            !tiny(image.width) && !tiny(image.height)
        })
        .filter_map(|image| {
            Some(Inline {
                content_id: content_id(&image.src)?,
                at: image.at,
                alt: image.alt,
                width: image.width,
                height: image.height,
            })
        })
        .collect()
}

/// A `cid:` address's Content-ID (RFC 2392: the rest of the address, percent-decoded); `None`
/// for any other address, or one the store would not have kept.
pub fn content_id(src: &str) -> Option<String> {
    let rest = src
        .get(..4)
        .filter(|scheme| scheme.eq_ignore_ascii_case("cid:"))
        .map(|_| &src[4..])?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    let kept = !out.is_empty()
        && out.len() <= MAX_CONTENT_ID_BYTES
        && out.iter().all(u8::is_ascii_graphic);
    kept.then(|| String::from_utf8_lossy(&out).into_owned())
}

/// The picture at `path` decoded, within `MAX_DECODED_SIDE` and `MAX_DECODE_ALLOC`, and
/// shrunk to `MAX_KEPT_PIXELS`. An animated picture gives its first frame. The error is the
/// decoder's, never the path.
pub fn decode(path: &Path) -> Result<DecodedPicture, String> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODED_SIDE);
    limits.max_image_height = Some(MAX_DECODED_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    let mut reader = ImageReader::open(path)
        .and_then(ImageReader::with_guessed_format)
        .map_err(|error| error.kind().to_string())?;
    reader.limits(limits);
    let image = reader.decode().map_err(|error| error.to_string())?;
    let natural = (image.width(), image.height());
    Ok(DecodedPicture {
        image: shrunk(image),
        natural,
    })
}

/// `image` within `MAX_KEPT_PIXELS`, its shape kept.
fn shrunk(image: DynamicImage) -> DynamicImage {
    let pixels = u64::from(image.width()) * u64::from(image.height());
    if pixels <= MAX_KEPT_PIXELS {
        return image;
    }
    let scale = (MAX_KEPT_PIXELS as f64 / pixels as f64).sqrt();
    let side = |px: u32| ((f64::from(px) * scale).floor() as u32).max(1);
    image.resize(
        side(image.width()),
        side(image.height()),
        FilterType::Triangle,
    )
}

/// The cells a picture of `natural` pixels takes at `width` columns, with cells of `font`
/// pixels (width, height): `None` when it is too small to show.
pub fn fit(
    natural: (u32, u32),
    declared: (Option<u32>, Option<u32>),
    width: u16,
    font: (u16, u16),
) -> Option<(u16, u16)> {
    if natural.0 <= MAX_HIDDEN_PX || natural.1 <= MAX_HIDDEN_PX {
        return None;
    }
    let (nw, nh) = (f64::from(natural.0), f64::from(natural.1));
    let (w, h) = match declared {
        (Some(w), Some(h)) => (f64::from(w), f64::from(h)),
        (Some(w), None) => (f64::from(w), f64::from(w) * nh / nw),
        (None, Some(h)) => (f64::from(h) * nw / nh, f64::from(h)),
        (None, None) => (nw, nh),
    };
    let shape = f64::from(font.0.max(1)) / f64::from(font.1.max(1));
    let cols = (w / CSS_PX_PER_COL).max(1.0);
    let rows = (cols * (h / w) * shape).max(1.0);
    let scale = (f64::from(width.max(1)) / cols)
        .min(f64::from(MAX_PICTURE_ROWS) / rows)
        .min(1.0);
    let cells = |n: f64| (n * scale).round().clamp(1.0, f64::from(u16::MAX)) as u16;
    Some((cells(cols), cells(rows)))
}

/// The pictures asked for or read, by (message id, Content-ID).
pub type Pictures = HashMap<(i64, String), Picture>;

/// Each picture encoded for the size it was last drawn at, by (message id, Content-ID): one
/// entry a picture, replaced when the size changes.
pub(super) type Protocols = HashMap<(i64, String), ((u16, u16), Rc<SlicedProtocol>)>;

impl Reader {
    /// The reader with pictures drawn by `picker`, or none (`None`: images off).
    pub fn with_images(mut self, picker: Option<Picker>) -> Self {
        self.picker = picker;
        self
    }

    /// Pictures drawn by `picker` from now on, or none (`:images`).
    pub fn set_images(&mut self, picker: Option<Picker>) {
        self.picker = picker;
        self.protocols.borrow_mut().clear();
        self.changed();
    }

    /// What `lines::State::pictures` takes: in rich mode with images on, the pictures read so
    /// far and the cell's size.
    pub(super) fn placing(&self) -> Option<(&Pictures, (u16, u16))> {
        let picker = self.picker.as_ref().filter(|_| self.rich)?;
        let font = picker.font_size();
        Some((&self.pictures, (font.width, font.height)))
    }

    /// The open messages' pictures not asked for yet, as (message id, its provider id,
    /// Content-ID): as many as `MAX_PICTURES_IN_FLIGHT` and `MAX_PICTURES_PER_READER` leave
    /// room for; none in plain mode or with images off.
    pub fn pictures_wanted(&self) -> Vec<(i64, String, String)> {
        if self.placing().is_none() {
            return Vec::new();
        }
        let loading = self
            .pictures
            .values()
            .filter(|p| matches!(p, Picture::Loading))
            .count();
        let room = MAX_PICTURES_IN_FLIGHT
            .saturating_sub(loading)
            .min(MAX_PICTURES_PER_READER.saturating_sub(self.pictures.len()));
        let mut wanted = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (m, open) in self.detail.messages.iter().zip(&self.expanded) {
            let Some(body) = self.rich_bodies.get(&m.id) else {
                continue;
            };
            if !*open || !matches!(m.body, BodyState::Text { .. }) {
                continue;
            }
            for inline in &body.pictures {
                if wanted.len() == room {
                    return wanted;
                }
                let key = (m.id, inline.content_id.clone());
                if !self.pictures.contains_key(&key) && seen.insert(key) {
                    wanted.push((m.id, m.remote_id.clone(), inline.content_id.clone()));
                }
            }
        }
        wanted
    }

    /// `wanted` asked for: never asked again while the reader is open.
    pub fn mark_pictures_asked(&mut self, wanted: &[(i64, String, String)]) {
        for (id, _, content_id) in wanted {
            self.pictures
                .insert((*id, content_id.clone()), Picture::Loading);
        }
    }

    /// A picture read: drawn from now on, or (`None`) left as its placeholder.
    pub fn picture_read(&mut self, id: i64, content_id: String, image: Option<DecodedPicture>) {
        let state = image.map_or(Picture::Failed, |image| Picture::Ready(Rc::new(image)));
        self.pictures.insert((id, content_id), state);
        self.changed();
    }

    /// Picture `picture` of message `message` encoded for `cols` by `rows` cells, again only
    /// when its size changes; `None` when it is not read, or images are off.
    pub fn protocol(
        &self,
        message: usize,
        picture: usize,
        cols: u16,
        rows: u16,
    ) -> Option<Rc<SlicedProtocol>> {
        let picker = self.picker.as_ref()?;
        let m = self.detail.messages.get(message)?;
        let inline = self.rich_bodies.get(&m.id)?.pictures.get(picture)?;
        let key = (m.id, inline.content_id.clone());
        if let Some((_, found)) = self
            .protocols
            .borrow()
            .get(&key)
            .filter(|(size, _)| *size == (cols, rows))
        {
            return Some(found.clone());
        }
        let Some(Picture::Ready(decoded)) = self.pictures.get(&key) else {
            return None;
        };
        // Scaled, not fit: a mail declares the size it wants drawn, often larger than the
        // picture itself, and a browser stretches it to that size.
        let encoded = SlicedProtocol::new_with_resize(
            picker,
            decoded.image.clone(),
            Size::new(cols, rows),
            Resize::Scale(Some(FilterType::Triangle)),
        );
        let encoded = Rc::new(encoded.ok()?);
        self.protocols
            .borrow_mut()
            .insert(key, ((cols, rows), encoded.clone()));
        Some(encoded)
    }
}
