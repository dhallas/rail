//! The images the styled scan finds: where each stands among the runs, what its tag says
//! (address, alt text, declared size), the caps, and that a flood of tags stays linear. The
//! text itself never changes: images stand beside it.

use std::time::Instant;

use super::gmail::SLOWDOWN;
use super::html::{Image, MAX_ALT_CHARS, MAX_IMAGES, html_styled, html_text};

fn joined(html: &str) -> String {
    html_styled(html)
        .runs
        .iter()
        .map(|r| r.text.as_str())
        .collect()
}

#[test]
fn an_image_stands_before_the_run_that_follows_it_and_the_text_is_unchanged() {
    let html = r#"<p>Hi</p><img src="cid:logo@x" alt="Logo" width="32" height=16><p>after</p>"#;
    let styled = html_styled(html);
    assert_eq!(
        styled.images,
        vec![Image {
            at: 1,
            src: "cid:logo@x".into(),
            alt: "Logo".into(),
            width: Some(32),
            height: Some(16),
        }]
    );
    let before: String = styled.runs[..1].iter().map(|r| r.text.as_str()).collect();
    assert_eq!(before, "Hi\n");
    assert_eq!(joined(html), html_text(html));
}

#[test]
fn sizes_come_from_attributes_or_the_inline_style_and_values_are_decoded() {
    let html = "<IMG style=\"max-width: 600px; width:120px\" SRC='cid:a&amp;b' alt=\"x&lt;y\">\
                <img src=cid:c width=\"50%\" height=\"auto\">";
    let images = html_styled(html).images;
    assert_eq!(images.len(), 2);
    assert_eq!(
        (images[0].src.as_str(), images[0].alt.as_str()),
        ("cid:a&b", "x<y")
    );
    assert_eq!((images[0].width, images[0].height), (Some(120), None));
    assert_eq!((images[1].width, images[1].height), (None, None));
}

#[test]
fn an_image_after_the_last_text_stands_at_the_end() {
    let styled = html_styled("text<img src=cid:end>");
    assert_eq!(styled.images[0].at, styled.runs.len());
}

#[test]
fn images_without_an_address_are_skipped_and_the_caps_hold() {
    let mut html = String::from("<img alt=nothing><img src=\"\">");
    for n in 0..MAX_IMAGES + 50 {
        html.push_str(&format!("<img src=cid:{n}>"));
    }
    let long_alt = "a\u{1b}b ".repeat(MAX_ALT_CHARS);
    html.push_str(&format!("<p>x</p><img src=cid:alt alt=\"{long_alt}\">"));
    let images = html_styled(&html).images;
    assert_eq!(images.len(), MAX_IMAGES);
    assert_eq!(images[0].src, "cid:0");

    let one = html_styled(&format!("<img src=cid:alt alt=\"{long_alt}\">")).images;
    assert_eq!(one[0].alt.chars().count(), MAX_ALT_CHARS);
    assert!(!one[0].alt.contains('\u{1b}'));
}

#[test]
fn a_flood_of_image_tags_stays_linear() {
    let mut html = String::new();
    while html.len() < 10 * 1024 * 1024 {
        html.push_str("<img src=cid:a alt=b width=1 height=1 style=\"width:1px\">x");
    }
    html.push_str("<p>zzend</p>");
    let started = Instant::now();
    let styled = html_styled(&html);
    let took = started.elapsed();
    assert!(took.as_secs() < 5 * u64::from(SLOWDOWN), "took {took:?}");
    assert_eq!(styled.images.len(), MAX_IMAGES);
    assert!(
        styled
            .runs
            .last()
            .is_some_and(|r| r.text.ends_with("zzend"))
    );
}
