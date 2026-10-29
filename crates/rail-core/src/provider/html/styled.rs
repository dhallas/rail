//! The styled scan's output: the text as runs of one style each, and the images beside them
//! (see the parent module's documentation).

use std::fmt;

use super::{Image, Scanned, scan};
use crate::provider::gmail::body::clean;

/// How a stretch of text is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub strong: bool,
    pub emphasis: bool,
    pub link: bool,
    pub quote: bool,
}

impl Style {
    pub const PLAIN: Self = Self {
        strong: false,
        emphasis: false,
        link: false,
        quote: false,
    };
}

/// A stretch of text in one style.
#[derive(Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub style: Style,
}

/// A body's text as styled runs, with the addresses its `[n]` markers stand for (link `n` is
/// `links[n - 1]`) and its images. The runs joined are the body text Rail stores for the same
/// HTML.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Styled {
    pub runs: Vec<Run>,
    pub links: Vec<String>,
    pub images: Vec<Image>,
}

/// Its length and style only: the text is mail, which must never reach a log.
impl fmt::Debug for Run {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Run")
            .field("len", &self.text.len())
            .field("style", &self.style)
            .finish()
    }
}

/// Counts, styles and places only, as `Run`'s and `Image`'s.
impl fmt::Debug for Styled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Styled")
            .field("runs", &self.runs)
            .field("links", &self.links.len())
            .field("images", &self.images)
            .finish()
    }
}

/// `html` as styled runs (see the module documentation), each cleaned of control and format
/// characters as stored body text is.
pub fn html_styled(html: &str) -> Styled {
    let Scanned {
        out,
        links,
        starts,
        images,
    } = scan(html, true);
    let mut runs: Vec<Run> = Vec::with_capacity(starts.len());
    let mut images = images.into_iter().peekable();
    let mut placed = Vec::new();
    for (index, &(start, style)) in starts.iter().enumerate() {
        // An image splits the runs where it stood, so it lands before this run's text.
        while let Some((_, image)) = images.next_if(|&(at, _)| at <= start) {
            placed.push(Image {
                at: runs.len(),
                ..image
            });
        }
        let end = starts.get(index + 1).map_or(out.len(), |&(next, _)| next);
        let text = clean(&String::from_utf8_lossy(&out[start.min(end)..end]));
        if !text.is_empty() {
            runs.push(Run { text, style });
        }
    }
    let end = runs.len();
    placed.extend(images.map(|(_, image)| Image { at: end, ..image }));
    Styled {
        runs,
        links,
        images: placed,
    }
}
