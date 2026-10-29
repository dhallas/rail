//! Inline pictures: `cid:` addresses, sizing, decoding under limits, and where a picture's
//! rows or placeholder stand among a rich body's lines.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use image::{DynamicImage, RgbaImage};

use super::lines::{LineKind, ReaderLine};
use super::pictures::{MAX_PICTURE_ROWS, Picture, content_id, decode, fit};
use super::rich::{Placing, from_html, lines};

/// Cells of 8 by 16 pixels, as many terminals have.
const FONT: (u16, u16) = (8, 16);

fn picture(width: u32, height: u32) -> Picture {
    Picture::Ready(Rc::new(DynamicImage::ImageRgba8(RgbaImage::new(
        width, height,
    ))))
}

fn built(html: &str, states: Option<&HashMap<(i64, String), Picture>>) -> Vec<ReaderLine> {
    let body = from_html(html);
    let placing = states.map(|states| Placing {
        states,
        font: FONT,
        message_id: 7,
    });
    let mut out = Vec::new();
    lines(&mut out, 0, &body, &HashSet::new(), 80, placing.as_ref());
    out
}

fn texts(lines: &[ReaderLine]) -> Vec<&str> {
    lines.iter().map(ReaderLine::text).collect()
}

#[test]
fn a_cid_address_names_its_content_id_percent_decoded() {
    assert_eq!(content_id("cid:logo@x").as_deref(), Some("logo@x"));
    assert_eq!(content_id("CID:a%40b%2Ec").as_deref(), Some("a@b.c"));
    assert_eq!(content_id("https://x/logo.png"), None);
    assert_eq!(content_id("cid:"), None);
    assert_eq!(content_id("cid:a%20b"), None, "a space is no Content-ID");
}

#[test]
fn a_picture_takes_its_declared_size_shrunk_to_the_pane_and_the_row_cap() {
    // 160 by 64 CSS pixels: 20 columns, 4 rows of 16-pixel cells 8 wide.
    assert_eq!(
        fit((320, 128), (Some(160), Some(64)), 80, FONT),
        Some((20, 4))
    );
    // The picture's own size when the mail declares none, keeping its shape from one side.
    assert_eq!(fit((160, 64), (None, None), 80, FONT), Some((20, 4)));
    assert_eq!(fit((320, 128), (Some(160), None), 80, FONT), Some((20, 4)));
    // Wider than the pane: shrunk to it.
    assert_eq!(fit((1600, 640), (None, None), 50, FONT), Some((50, 10)));
    // Taller than the cap: shrunk to it, keeping its shape.
    let (_, rows) = fit((100, 4000), (None, None), 80, FONT).unwrap();
    assert_eq!(rows, MAX_PICTURE_ROWS);
    // A tracking pixel takes no room.
    assert_eq!(fit((1, 1), (None, None), 80, FONT), None);
}

#[test]
fn decoding_reads_a_real_picture_and_refuses_one_past_the_limits() {
    let dir = tempfile::tempdir().unwrap();
    let small = dir.path().join("small.png");
    RgbaImage::new(3, 2).save(&small).unwrap();
    let image = decode(&small).unwrap();
    assert_eq!((image.width(), image.height()), (3, 2));

    let huge = dir.path().join("huge.png");
    image::GrayImage::new(9000, 1).save(&huge).unwrap();
    assert!(decode(&huge).is_err(), "wider than Rail decodes");
    let junk = dir.path().join("junk.png");
    std::fs::write(&junk, b"<svg onload=alert(1)>").unwrap();
    assert!(decode(&junk).is_err());
}

#[test]
fn a_picture_shows_its_alt_text_until_read_then_its_rows_where_it_stood() {
    let html =
        r#"<p>Before</p><img src="cid:logo@x" alt="Logo" width="160" height="64"><p>After</p>"#;
    assert_eq!(
        texts(&built(html, Some(&HashMap::new()))),
        ["Before", "[image: Logo]", "", "After"]
    );

    let states = HashMap::from([((7, "logo@x".to_string()), picture(320, 128))]);
    let out = built(html, Some(&states));
    let kinds: Vec<LineKind> = out.iter().map(|l| l.kind).collect();
    let rows: Vec<LineKind> = (0..4)
        .map(|row| LineKind::Picture {
            message: 0,
            picture: 0,
            row,
            cols: 20,
            rows: 4,
        })
        .collect();
    assert_eq!(kinds[1..5], rows[..]);
    assert_eq!(texts(&out)[0], "Before");
    assert_eq!(texts(&out)[6], "After");

    let failed = HashMap::from([((7, "logo@x".to_string()), Picture::Failed)]);
    assert_eq!(texts(&built(html, Some(&failed)))[1], "[image: Logo]");
}

#[test]
fn a_picture_inside_a_line_breaks_it_and_images_off_leave_the_text_alone() {
    let html = r#"<p>one <img src="cid:icon@x" alt="icon"> two</p>"#;
    assert_eq!(
        texts(&built(html, Some(&HashMap::new()))),
        ["one", "[image: icon]", " two"]
    );
    assert_eq!(texts(&built(html, None)), ["one two"]);
}

#[test]
fn tracking_pixels_remote_pictures_and_tiny_declared_ones_take_no_room() {
    let html = r#"<p>Hi</p><img src="cid:pixel@x" width="1" height="1">
                  <img src="https://tracker.example/p.gif"><p>Bye</p>"#;
    assert_eq!(
        texts(&built(html, Some(&HashMap::new()))),
        ["Hi", "", "Bye"]
    );
    let tiny = HashMap::from([((7, "dot@x".to_string()), picture(2, 2))]);
    let html = r#"<p>Hi</p><img src="cid:dot@x"><p>Bye</p>"#;
    assert_eq!(texts(&built(html, Some(&tiny))), ["Hi", "", "Bye"]);
}

#[test]
fn pictures_do_not_move_the_folds() {
    let html = r#"<img src="cid:logo@x"><blockquote>a<br>b<br>c<br>d</blockquote>"#;
    for states in [None, Some(&HashMap::new())] {
        let fold = built(html, states)
            .into_iter()
            .find(|l| matches!(l.kind, LineKind::Fold { .. }))
            .expect("the quote folds");
        assert_eq!(
            fold.kind,
            LineKind::Fold {
                message: 0,
                block: 0
            }
        );
    }
}
