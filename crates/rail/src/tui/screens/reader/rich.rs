//! An HTML body in rich mode (`rich_html`): the scanner's styled runs (`html_styled`, the same
//! text Rail stores) as reader lines, each stretch in its style. Quoted runs fold as a plain
//! body's do: a line is quoted when its first visible stretch is in a `blockquote` or its text
//! starts with `>`. Every outside string is escaped before it is measured or wrapped.

use std::collections::HashSet;
use std::fmt;

use rail_core::provider::gmail::TRUNCATED_MARKER;
use rail_core::provider::html::{Run, Style, html_styled};
use rail_core::store::view::MAX_SHOWN_BODY_CHARS;

use super::lines::{LineKind, ReaderLine, Tone, tag};
use crate::tui::text::{QUOTE_FOLD_MIN, escape, fit, fold_label, wrap};

/// A message's HTML part as styled runs, cut to `MAX_SHOWN_BODY_CHARS` as the plain text is.
#[derive(Clone, PartialEq, Eq)]
pub struct RichBody {
    pub runs: Vec<Run>,
    /// Longer than `MAX_SHOWN_BODY_CHARS`.
    pub cut: bool,
}

/// Sizes only: the runs are mail text, which must never reach a log.
impl fmt::Debug for RichBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RichBody")
            .field("runs", &self.runs.len())
            .field("cut", &self.cut)
            .finish()
    }
}

/// `html` scanned for the reader.
pub fn from_html(html: &str) -> RichBody {
    let mut runs = html_styled(html).runs;
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
    RichBody { runs, cut }
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

/// A line, or a run of at least `QUOTE_FOLD_MIN` quoted lines (as `text::blocks`).
enum Block {
    Line(Segments),
    Quote(Vec<Segments>),
}

/// `body`'s lines at `width`, for message `index`; `unfolded` holds the open folds.
pub(super) fn lines(
    out: &mut Vec<ReaderLine>,
    index: usize,
    body: &RichBody,
    unfolded: &HashSet<(usize, usize)>,
    width: usize,
) {
    let mut at = 0;
    for (block_index, block) in blocks(&body.runs).into_iter().enumerate() {
        match block {
            Block::Line(segments) => {
                push_rows(out, LineKind::Body { message: index }, &segments, width, at);
                at += 1;
            }
            Block::Quote(rows) if unfolded.contains(&(index, block_index)) => {
                let kind = LineKind::Quote {
                    message: index,
                    block: block_index,
                };
                for segments in rows {
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
                    push_rows(out, kind, &quoted, width, at);
                    at += 1;
                }
            }
            Block::Quote(rows) => {
                let kind = LineKind::Fold {
                    message: index,
                    block: block_index,
                };
                let label = fit(&fold_label(rows.len()), width);
                out.push(ReaderLine::one(kind, label, Tone::Muted));
                let from = out.len() - 1;
                tag(&mut out[from..], at, at + rows.len());
                at += rows.len();
            }
        }
    }
}

/// The runs split at line breaks and grouped into blocks.
fn blocks(runs: &[Run]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut quoted: Vec<Segments> = Vec::new();
    for line in split_lines(runs) {
        if is_quoted(&line) {
            quoted.push(line);
            continue;
        }
        end_quote(&mut out, &mut quoted);
        out.push(Block::Line(line));
    }
    end_quote(&mut out, &mut quoted);
    out
}

fn end_quote(out: &mut Vec<Block>, quoted: &mut Vec<Segments>) {
    if quoted.len() >= QUOTE_FOLD_MIN {
        out.push(Block::Quote(std::mem::take(quoted)));
    } else {
        out.extend(quoted.drain(..).map(Block::Line));
    }
}

fn split_lines(runs: &[Run]) -> Vec<Segments> {
    let mut lines = vec![Segments::new()];
    for run in runs {
        for (n, piece) in run.text.split('\n').enumerate() {
            if n > 0 {
                lines.push(Segments::new());
            }
            if !piece.is_empty()
                && let Some(line) = lines.last_mut()
            {
                line.push((piece.to_string(), run.style));
            }
        }
    }
    lines
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
