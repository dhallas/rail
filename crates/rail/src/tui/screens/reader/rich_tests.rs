//! An HTML body's lines in rich mode: each stretch in its style, wrapping that keeps the
//! styles, quoted runs folded as plain bodies fold them, outside text escaped, and the cap;
//! a rich body arriving in an open reader keeps the cursor's line.

use std::collections::HashSet;

use rail_core::config::ThreadOrder;
use rail_core::provider::html::Style;
use rail_core::store::view::{BodyState, MAX_SHOWN_BODY_CHARS};

use super::Reader;
use super::lines::{LineKind, ReaderLine, Tone};
use super::rich::{RichBody, from_html, lines};
use super::test_threads::{four, reader_of};

const BODY: LineKind = LineKind::Body { message: 0 };
const STRONG: Style = Style {
    strong: true,
    ..Style::PLAIN
};
const LINK: Style = Style {
    link: true,
    ..Style::PLAIN
};
const QUOTED: Style = Style {
    quote: true,
    ..Style::PLAIN
};

fn built(body: &RichBody, unfolded: &HashSet<(usize, usize)>, width: usize) -> Vec<ReaderLine> {
    let mut out = Vec::new();
    lines(&mut out, 0, body, unfolded, width);
    out
}

fn segments(line: &ReaderLine) -> Vec<(&str, Tone)> {
    line.segments()
        .iter()
        .map(|(text, tone)| (text.as_str(), *tone))
        .collect()
}

#[test]
fn each_stretch_keeps_its_style_and_plain_text_stays_plain() {
    let body = from_html(r#"<p>Hi <b>there</b>, see <a href="https://e.com">this</a></p>"#);
    let out = built(&body, &HashSet::new(), 80);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].kind, BODY);
    assert_eq!(
        segments(&out[0]),
        vec![
            ("Hi ", Tone::Plain),
            ("there", Tone::Rich(STRONG)),
            (", see ", Tone::Plain),
            ("this[1]", Tone::Rich(LINK)),
        ]
    );
}

#[test]
fn wrapping_keeps_each_words_style() {
    let body = from_html("<p>one <b>two three</b> four</p>");
    let out = built(&body, &HashSet::new(), 10);
    let rows: Vec<Vec<(&str, Tone)>> = out.iter().map(segments).collect();
    assert_eq!(
        rows,
        vec![
            vec![("one ", Tone::Plain), ("two", Tone::Rich(STRONG))],
            vec![("three", Tone::Rich(STRONG)), (" four", Tone::Plain)],
        ]
    );
}

#[test]
fn a_long_quote_folds_and_unfolds_like_a_plain_one() {
    // Two lines quoted in HTML and two in text, one run.
    let body = from_html("<p>Reply</p><blockquote>a<br>b</blockquote>&gt; c<br>&gt; d<p>Bye");
    let folded = built(&body, &HashSet::new(), 80);
    let texts: Vec<&str> = folded.iter().map(ReaderLine::text).collect();
    assert_eq!(texts, vec!["Reply", "", "[4 quoted lines]", "Bye"]);
    let fold = folded[2].kind;
    assert_eq!(
        fold,
        LineKind::Fold {
            message: 0,
            block: 2
        }
    );

    let open = built(&body, &HashSet::from([(0, 2)]), 80);
    let quoted: Vec<&ReaderLine> = open
        .iter()
        .filter(|l| matches!(l.kind, LineKind::Quote { block: 2, .. }))
        .collect();
    assert_eq!(quoted.len(), 4);
    assert_eq!(segments(quoted[0]), vec![("a", Tone::Rich(QUOTED))]);
    assert_eq!(segments(quoted[3]), vec![("> d", Tone::Rich(QUOTED))]);
}

#[test]
fn outside_text_is_escaped() {
    let body = RichBody {
        runs: vec![rail_core::provider::html::Run {
            text: "a\u{1b}[31mb".into(),
            style: STRONG,
        }],
        cut: false,
    };
    let out = built(&body, &HashSet::new(), 80);
    assert_eq!(
        segments(&out[0]),
        vec![("a\\u{1b}[31mb", Tone::Rich(STRONG))]
    );
}

#[test]
fn a_body_past_the_shown_limit_is_cut_and_says_so() {
    let html = format!("<b>{}</b>tail", "x".repeat(MAX_SHOWN_BODY_CHARS + 10));
    let body = from_html(&html);
    let kept: usize = body.runs.iter().map(|r| r.text.chars().count()).sum();
    assert_eq!(kept, MAX_SHOWN_BODY_CHARS);
    assert!(body.cut);
    assert!(!from_html("<p>short</p>").cut);
}

/// A blockquote of four lines: rich mode folds it, its stored text (no `>`) does not.
const QUOTING: &str = "<p>Reply</p><blockquote>a<br>b<br>c<br>d</blockquote><p>Tail line</p>";

fn joined(body: &RichBody) -> String {
    body.runs.iter().map(|r| r.text.as_str()).collect()
}

/// `four`, its newest message (open under the title) an HTML one stored as `stored`, in rich
/// mode with the HTML not read yet.
fn rich_reader(stored: String) -> Reader {
    let mut detail = four(0);
    let newest = detail.messages.last_mut().unwrap();
    newest.has_html = true;
    newest.body = BodyState::Text {
        text: stored,
        cut: false,
    };
    reader_of(detail, ThreadOrder::Oldest).with_rich(true)
}

#[test]
fn a_rich_body_arriving_keeps_the_cursor_on_its_line() {
    let body = from_html(QUOTING);
    let mut reader = rich_reader(joined(&body));
    let lines = reader.lines(80);
    reader.cursor = lines.iter().position(|l| l.text() == "Tail line").unwrap();

    reader.rich_read(4, Some(body));

    assert!(
        reader
            .lines(80)
            .iter()
            .any(|l| l.text() == "[4 quoted lines]")
    );
    assert_eq!(reader.cursor_line().unwrap().text(), "Tail line");
}
