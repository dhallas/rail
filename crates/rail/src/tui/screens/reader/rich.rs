//! An HTML body in rich mode (`rich_html`): the scanner's styled runs (`html_styled`, the same
//! text Rail stores) as reader lines, each stretch in its style. Quoted runs fold as a plain
//! body's do: a line is quoted when its first visible stretch is in a `blockquote` or its text
//! starts with `>`. Every outside string is escaped before it is measured or wrapped. With
//! images on, each picture (`pictures`) stands where the mail puts it, breaking the line it
//! falls in: its rows once it is decoded, `[image: alt]` until then (a remote one without alt
//! text, often a tracking pixel, takes no room). While a thread's remote pictures are held
//! back, a body that has some starts with a hint naming the keys that load them.

use std::collections::HashSet;
use std::fmt;

use rail_core::provider::gmail::TRUNCATED_MARKER;
use rail_core::provider::html::{Run, Style, html_styled};
use rail_core::store::view::MAX_SHOWN_BODY_CHARS;

use super::lines::{LineKind, ReaderLine, Tone, tag};
use super::pictures::{self, Inline, Picture, Pictures, Source};
use crate::tui::text::{QUOTE_FOLD_MIN, escape, fit, fold_label, wrap};

/// A message's HTML part as styled runs, cut to `MAX_SHOWN_BODY_CHARS` as the plain text is.
#[derive(Clone, PartialEq, Eq)]
pub struct RichBody {
    pub runs: Vec<Run>,
    /// Longer than `MAX_SHOWN_BODY_CHARS`.
    pub cut: bool,
    /// Its `cid:` pictures, in order, each before run `at`.
    pub pictures: Vec<Inline>,
}

/// Sizes only: the runs are mail text, which must never reach a log.
impl fmt::Debug for RichBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RichBody")
            .field("runs", &self.runs.len())
            .field("cut", &self.cut)
            .field("pictures", &self.pictures.len())
            .finish()
    }
}

/// `html` scanned for the reader.
pub fn from_html(html: &str) -> RichBody {
    let styled = html_styled(html);
    let mut runs = styled.runs;
    let (mut left, mut keep, mut cut) = (MAX_SHOWN_BODY_CHARS, runs.len(), false);
    for (index, run) in runs.iter_mut().enumerate() {
        let chars = run.text.chars().count();
        if chars <= left {
            left -= chars;
            continue;
        }
        if let Some((at, _)) = run.text.char_indices().nth(left) {
            run.text.truncate(at);
        }
        (keep, cut) = (index + 1, true);
        break;
    }
    runs.truncate(keep);
    runs.retain(|r| !r.text.is_empty());
    let mut pictures = pictures::inline(styled.images);
    pictures.retain(|p| p.at <= runs.len());
    RichBody {
        runs,
        cut,
        pictures,
    }
}

/// What placing pictures takes: the pictures read so far by (message id, source), the
/// terminal cell's size in pixels, this message's id, and the hint while its remote pictures
/// are held back.
pub(super) struct Placing<'a> {
    pub states: &'a Pictures,
    pub font: (u16, u16),
    pub message_id: i64,
    pub remote_hint: Option<&'a str>,
}

impl RichBody {
    /// The body with the note that Rail kept only the first 10 MiB, unstyled, when the stored
    /// text `stored` ends with it: the scanner never writes it, and the runs joined stay the
    /// stored text.
    pub(super) fn noting_cut(mut self, stored: &str) -> Self {
        if !self.cut && stored.ends_with(TRUNCATED_MARKER) {
            self.runs.push(Run {
                text: TRUNCATED_MARKER.into(),
                style: Style::PLAIN,
            });
        }
        self
    }
}

/// One line of the body: its stretches, unescaped.
type Segments = Vec<(String, Style)>;

/// A line of the body and the stored text's line it draws (a picture inside a line splits it
/// into two, both drawing the same stored line).
type Line = (Segments, usize);

/// A line of the body, or picture `n` of `RichBody::pictures`.
enum Item {
    Line(Line),
    Picture(usize),
}

/// A line, a run of at least `QUOTE_FOLD_MIN` quoted lines (as `text::blocks`), or a picture.
enum Block {
    Line(Line),
    Quote(Vec<Line>),
    Picture(usize),
}

/// `body`'s lines at `width`, for message `index`; `unfolded` holds the open folds, and
/// `placing` is `None` with images off. Pictures are not counted as blocks, so turning images
/// on or off leaves the folds where they were.
pub(super) fn lines(
    out: &mut Vec<ReaderLine>,
    index: usize,
    body: &RichBody,
    unfolded: &HashSet<(usize, usize)>,
    width: usize,
    placing: Option<&Placing<'_>>,
) {
    let shown: &[Inline] = if placing.is_some() {
        &body.pictures
    } else {
        &[]
    };
    let held = placing
        .and_then(|p| p.remote_hint)
        .filter(|_| shown.iter().any(|p| matches!(p.source, Source::Remote(_))));
    if let Some(hint) = held {
        let hint = escape(hint);
        let kind = LineKind::Body { message: index };
        out.push(ReaderLine::one(kind, fit(&hint, width), Tone::Muted));
    }
    let mut block_index = 0;
    for block in blocks(&body.runs, shown) {
        if let Block::Picture(n) = block {
            if let Some(placing) = placing {
                picture(out, index, n, &body.pictures[n], placing, width);
            }
            continue;
        }
        text_block(out, index, block_index, block, unfolded, width);
        block_index += 1;
    }
}

fn text_block(
    out: &mut Vec<ReaderLine>,
    index: usize,
    block_index: usize,
    block: Block,
    unfolded: &HashSet<(usize, usize)>,
    width: usize,
) {
    match block {
        Block::Line((segments, source)) => {
            push_rows(
                out,
                LineKind::Body { message: index },
                &segments,
                width,
                source,
            );
        }
        Block::Quote(rows) if unfolded.contains(&(index, block_index)) => {
            let kind = LineKind::Quote {
                message: index,
                block: block_index,
            };
            for (segments, source) in rows {
                let quoted: Segments = segments
                    .into_iter()
                    .map(|(text, style)| {
                        (
                            text,
                            Style {
                                quote: true,
                                ..style
                            },
                        )
                    })
                    .collect();
                push_rows(out, kind, &quoted, width, source);
            }
        }
        Block::Quote(rows) => {
            let kind = LineKind::Fold {
                message: index,
                block: block_index,
            };
            let label = fit(&fold_label(rows.len()), width);
            out.push(ReaderLine::one(kind, label, Tone::Muted));
            if let (Some((_, first)), Some((_, last))) = (rows.first(), rows.last()) {
                let from = out.len() - 1;
                tag(&mut out[from..], *first, last + 1);
            }
        }
        Block::Picture(_) => {}
    }
}

/// Picture `n`: its rows once decoded (none for one too small to show), else its placeholder.
fn picture(
    out: &mut Vec<ReaderLine>,
    index: usize,
    n: usize,
    inline: &Inline,
    placing: &Placing<'_>,
    width: usize,
) {
    let key = (placing.message_id, inline.source.clone());
    if let Some(Picture::Ready(decoded)) = placing.states.get(&key) {
        let natural = decoded.natural;
        let declared = (inline.width, inline.height);
        let columns = u16::try_from(width).unwrap_or(u16::MAX);
        let Some((cols, rows)) = pictures::fit(natural, declared, columns, placing.font) else {
            return;
        };
        for row in 0..rows {
            let kind = LineKind::Picture {
                message: index,
                picture: n,
                row,
                cols,
                rows,
            };
            out.push(ReaderLine::one(kind, String::new(), Tone::Plain));
        }
        return;
    }
    let label = if inline.alt.is_empty() && matches!(inline.source, Source::Remote(_)) {
        return;
    } else if inline.alt.is_empty() {
        "[image]".to_string()
    } else {
        format!("[image: {}]", inline.alt)
    };
    let kind = LineKind::Body { message: index };
    out.push(ReaderLine::one(kind, fit(&label, width), Tone::Muted));
}

/// The runs split at line breaks and pictures, and grouped into blocks.
fn blocks(runs: &[Run], pictures: &[Inline]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut quoted: Vec<Line> = Vec::new();
    for item in split(runs, pictures) {
        let line = match item {
            Item::Line(line) => line,
            Item::Picture(n) => {
                end_quote(&mut out, &mut quoted);
                out.push(Block::Picture(n));
                continue;
            }
        };
        if is_quoted(&line.0) {
            quoted.push(line);
            continue;
        }
        end_quote(&mut out, &mut quoted);
        out.push(Block::Line(line));
    }
    end_quote(&mut out, &mut quoted);
    out
}

fn end_quote(out: &mut Vec<Block>, quoted: &mut Vec<Line>) {
    if quoted.len() >= QUOTE_FOLD_MIN {
        out.push(Block::Quote(std::mem::take(quoted)));
    } else {
        out.extend(quoted.drain(..).map(Block::Line));
    }
}

/// The runs as lines, with each picture where it stood: a picture inside a line ends it.
fn split(runs: &[Run], pictures: &[Inline]) -> Vec<Item> {
    let mut items = Vec::new();
    let mut current = Segments::new();
    let mut source = 0;
    let mut next = 0;
    let mut place = |items: &mut Vec<Item>, current: &mut Segments, source, before: usize| {
        while pictures.get(next).is_some_and(|p| p.at <= before) {
            if !current.is_empty() {
                items.push(Item::Line((std::mem::take(current), source)));
            }
            items.push(Item::Picture(next));
            next += 1;
        }
    };
    for (k, run) in runs.iter().enumerate() {
        place(&mut items, &mut current, source, k);
        for (n, piece) in run.text.split('\n').enumerate() {
            if n > 0 {
                items.push(Item::Line((std::mem::take(&mut current), source)));
                source += 1;
            }
            if !piece.is_empty() {
                current.push((piece.to_string(), run.style));
            }
        }
    }
    place(&mut items, &mut current, source, usize::MAX);
    // The last line, unless a picture already ended it.
    if !current.is_empty() || !matches!(items.last(), Some(Item::Picture(_))) {
        items.push(Item::Line((current, source)));
    }
    items
}

/// The first visible stretch is in a `blockquote`, or the text starts with `>`.
fn is_quoted(line: &Segments) -> bool {
    line.iter()
        .map(|(text, style)| (text.trim_start_matches([' ', '\t']), style))
        .find(|(text, _)| !text.is_empty())
        .is_some_and(|(text, style)| style.quote || text.starts_with('>'))
}

/// `segments` escaped and wrapped to `width`, each row keeping each character's style.
/// `wrap` only drops spaces, so every character of a row is found, in order, in the line.
/// The rows draw the stored text's line `source`.
fn push_rows(
    out: &mut Vec<ReaderLine>,
    kind: LineKind,
    segments: &Segments,
    width: usize,
    source: usize,
) {
    let from = out.len();
    let escaped: Vec<(String, Tone)> = segments
        .iter()
        .map(|(text, style)| (escape(text), tone(*style)))
        .collect();
    let chars: Vec<(char, Tone)> = escaped
        .iter()
        .flat_map(|(text, tone)| text.chars().map(move |c| (c, *tone)))
        .collect();
    let line: String = escaped.iter().map(|(text, _)| text.as_str()).collect();
    let mut at = 0;
    for row in wrap(&line, width) {
        let mut parts: Vec<(String, Tone)> = Vec::new();
        for c in row.chars() {
            while chars.get(at).is_some_and(|&(found, _)| found != c) {
                at += 1;
            }
            let tone = chars.get(at).map_or(Tone::Plain, |&(_, tone)| tone);
            at += 1;
            match parts.last_mut() {
                Some((text, last)) if *last == tone => text.push(c),
                _ => parts.push((c.to_string(), tone)),
            }
        }
        out.push(ReaderLine::of(kind, parts));
    }
    tag(&mut out[from..], source, source + 1);
}

/// Plain text in the body's own tone, anything else in its style.
fn tone(style: Style) -> Tone {
    if style == Style::PLAIN {
        Tone::Plain
    } else {
        Tone::Rich(style)
    }
}
