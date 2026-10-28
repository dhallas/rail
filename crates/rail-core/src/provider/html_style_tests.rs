//! The scanner's styled text: which stretches are strong, emphasised, linked or quoted, that
//! the text is the body text Rail stores, and that hostile nesting stays linear.

use std::time::Instant;

use super::gmail::SLOWDOWN;
use super::html::{Run, Style, html_styled, html_text};

fn runs(html: &str) -> Vec<(String, Style)> {
    html_styled(html)
        .runs
        .into_iter()
        .map(|Run { text, style }| (text, style))
        .collect()
}

fn plain(text: &str) -> (String, Style) {
    (text.to_string(), Style::default())
}

fn styled(text: &str, style: Style) -> (String, Style) {
    (text.to_string(), style)
}

const STRONG: Style = Style {
    strong: true,
    ..Style::PLAIN
};
const EMPHASIS: Style = Style {
    emphasis: true,
    ..Style::PLAIN
};
const LINK: Style = Style {
    link: true,
    ..Style::PLAIN
};
const QUOTE: Style = Style {
    quote: true,
    ..Style::PLAIN
};

#[test]
fn bold_italic_headings_and_quotes_are_styled() {
    let html = "<p>Hi <b>there</b>, <em>you</em></p><h2>Title</h2>\
                <blockquote>said <strong><i>this</i></strong></blockquote>after";
    assert_eq!(
        runs(html),
        vec![
            plain("Hi "),
            styled("there", STRONG),
            plain(", "),
            styled("you", EMPHASIS),
            plain("\n\n"),
            styled("Title", STRONG),
            plain("\n\n"),
            styled("said ", QUOTE),
            styled(
                "this",
                Style {
                    strong: true,
                    emphasis: true,
                    quote: true,
                    ..Style::PLAIN
                }
            ),
            plain("\nafter"),
        ]
    );
}

#[test]
fn a_kept_links_text_and_marker_are_link_styled() {
    let html = r#"<p>See <a href="https://example.com/a">the <b>docs</b></a> and
                  <a href="javascript:x">this</a>.</p>"#;
    let styled_text = html_styled(html);
    assert_eq!(styled_text.links, vec!["https://example.com/a".to_string()]);
    assert_eq!(
        runs(html),
        vec![
            plain("See "),
            styled("the ", LINK),
            styled(
                "docs",
                Style {
                    strong: true,
                    link: true,
                    ..Style::PLAIN
                }
            ),
            styled("[1]", LINK),
            plain(" and this."),
        ]
    );
}

#[test]
fn stray_end_tags_do_not_unbalance_the_styles() {
    assert_eq!(
        runs("</b></em>x <b>y</b></b> z"),
        vec![plain("x "), styled("y", STRONG), plain(" z")]
    );
}

#[test]
fn the_text_is_the_body_text_rail_stores_with_controls_removed() {
    let html = "<p>a<b>b&#x202e;c</b></p><ul><li>one<li><i>two</i></ul>\
                <table><tr><td>x</td><td><a href='https://e.com'>y</a></td></tr></table>";
    let joined: String = html_styled(html)
        .runs
        .iter()
        .map(|r| r.text.as_str())
        .collect();
    assert_eq!(joined, html_text(html).replace('\u{202e}', ""));
    assert!(!joined.contains('\u{202e}'));
}

#[test]
fn deep_style_nesting_stays_linear_and_short() {
    let mut html = String::new();
    while html.len() < 10 * 1024 * 1024 {
        html.push_str("<b><i><blockquote>x");
    }
    html.push_str("<p>zzend</p>");
    let started = Instant::now();
    let styled_text = html_styled(&html);
    let took = started.elapsed();
    let length: usize = styled_text.runs.iter().map(|r| r.text.len()).sum();
    assert!(took.as_secs() < 5 * u64::from(SLOWDOWN), "took {took:?}");
    assert!(styled_text.runs.len() <= length);
    assert!(
        styled_text
            .runs
            .last()
            .is_some_and(|r| r.text.ends_with("zzend"))
    );
}
