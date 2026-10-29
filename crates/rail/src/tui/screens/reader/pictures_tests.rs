//! Inline pictures: `cid:` addresses, sizing, decoding under limits, and where a picture's
//! rows or placeholder stand among a rich body's lines.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use image::{DynamicImage, RgbaImage};

use rail_core::config::ThreadOrder;
use ratatui_image::picker::Picker;

use super::lines::{LineKind, ReaderLine};
use super::pictures::{
    DecodedPicture, MAX_KEPT_PIXELS, MAX_PICTURE_ROWS, MAX_PICTURES_IN_FLIGHT,
    MAX_PICTURES_PER_READER, Picture, content_id, decode, fit,
};
use super::rich::{Placing, from_html, lines};
use super::test_threads::{four, reader_of};

/// Cells of 8 by 16 pixels, as many terminals have.
const FONT: (u16, u16) = (8, 16);

fn picture(width: u32, height: u32) -> Picture {
    Picture::Ready(Rc::new(DecodedPicture {
        image: DynamicImage::ImageRgba8(RgbaImage::new(width, height)),
        natural: (width, height),
    }))
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
    let decoded = decode(&small).unwrap();
    assert_eq!((decoded.image.width(), decoded.image.height()), (3, 2));
    assert_eq!(decoded.natural, (3, 2));

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

#[test]
fn a_large_picture_is_kept_shrunk_with_its_own_size_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let large = dir.path().join("large.png");
    image::GrayImage::new(2400, 1200).save(&large).unwrap();
    let decoded = decode(&large).unwrap();
    assert_eq!(decoded.natural, (2400, 1200), "sized as the mail's own");
    let (w, h) = (decoded.image.width(), decoded.image.height());
    assert!(u64::from(w) * u64::from(h) <= MAX_KEPT_PIXELS, "{w}x{h}");
    assert_eq!(w, h * 2, "its shape kept");
}

/// A reader in rich mode with images on whose newest message carries `n` pictures.
fn reader_with_pictures(n: usize) -> super::Reader {
    let html: String = (0..n)
        .map(|i| format!(r#"<p>{i}</p><img src="cid:p{i}@x" width="160" height="64">"#))
        .collect();
    let mut reader = reader_of(four(0), ThreadOrder::Oldest)
        .with_rich(true)
        .with_images(Some(Picker::halfblocks()));
    reader.rich_read(4, Some(from_html(&html)));
    reader
}

#[test]
fn a_reader_asks_for_a_few_pictures_at_a_time_and_no_more_than_its_cap() {
    let mut reader = reader_with_pictures(MAX_PICTURES_PER_READER + 8);
    let mut asked = 0;
    loop {
        let wanted = reader.pictures_wanted();
        assert!(wanted.len() <= MAX_PICTURES_IN_FLIGHT, "{}", wanted.len());
        if wanted.is_empty() {
            break;
        }
        reader.mark_pictures_asked(&wanted);
        assert!(
            reader.pictures_wanted().is_empty(),
            "none more while they load"
        );
        asked += wanted.len();
        for (id, _, content_id) in wanted {
            reader.picture_read(id, content_id, None);
        }
    }
    assert_eq!(asked, MAX_PICTURES_PER_READER);
}

#[test]
fn a_picture_keeps_one_encoding_whatever_sizes_it_is_drawn_at() {
    let mut reader = reader_with_pictures(1);
    let decoded = DecodedPicture {
        image: DynamicImage::ImageRgba8(RgbaImage::new(160, 64)),
        natural: (160, 64),
    };
    reader.picture_read(4, "p0@x".into(), Some(decoded));
    for (cols, rows) in [(20, 4), (10, 2), (40, 8), (20, 4)] {
        assert!(reader.protocol(3, 0, cols, rows).is_some());
        assert_eq!(reader.protocols.borrow().len(), 1);
    }
}
