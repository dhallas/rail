//! Inline pictures through the mail screen: an open HTML message's `cid:` picture asked of
//! the daemon once, its rows kept and drawn, and `:images` hiding it, saved to `config.toml`.

use crossterm::event::KeyCode;
use image::RgbaImage;
use rail_core::ops::AttachmentImageResponse;
use rail_core::store::Store;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use super::{Fixture, fixture, keys, open_toms_card, press, store_bodies_on_fetch};
use crate::tui::data::FakeCall;
use crate::tui::screen::Screen;
use crate::tui::screens::reader::LineKind;
use crate::tui::theme::Theme;

/// m1 shows a 160 by 64 picture under its first paragraph, whose bytes the fake daemon has.
async fn with_a_picture() -> (Fixture, tempfile::TempDir) {
    let f = fixture(false).await;
    store_bodies_on_fetch(&f);
    let html = r#"<p>Hi Dana,</p><img src="cid:logo@x" alt="Logo" width="160" height="64">
                  <p>Thanks,<br>Tom</p>"#;
    Store::open(&f.paths.db_file())
        .unwrap()
        .conn()
        .execute(
            "UPDATE messages SET body_html = ?1 WHERE remote_id = 'm1'",
            [html],
        )
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("logo.png");
    RgbaImage::from_pixel(32, 16, image::Rgba([200, 30, 30, 255]))
        .save(&png)
        .unwrap();
    f.fake.image_answers.lock().unwrap().insert(
        "logo@x".into(),
        Ok(AttachmentImageResponse {
            path: png.display().to_string(),
            mime_type: "image/png".into(),
        }),
    );
    (f, dir)
}

fn image_calls(f: &Fixture) -> Vec<FakeCall> {
    f.fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| matches!(c, FakeCall::AttachmentImage { .. }))
        .cloned()
        .collect()
}

fn picture_rows(f: &Fixture) -> usize {
    f.screen
        .reader()
        .unwrap()
        .lines(80)
        .iter()
        .filter(|l| matches!(l.kind, LineKind::Picture { .. }))
        .count()
}

async fn run_images(f: &mut Fixture) {
    press(f, KeyCode::Char(':')).await;
    keys(f, "images").await;
    press(f, KeyCode::Enter).await;
}

#[tokio::test]
async fn an_open_messages_picture_is_asked_for_once_and_drawn_in_its_rows() {
    let (mut f, _dir) = with_a_picture().await;
    press(&mut f, KeyCode::Enter).await;
    open_toms_card(&mut f).await;
    assert_eq!(
        image_calls(&f),
        [FakeCall::AttachmentImage {
            account: "work".into(),
            message_id: "m1".into(),
            content_id: "logo@x".into(),
        }]
    );
    // 160 by 64 CSS pixels in the half-block picker's 10 by 20 cells: 20 columns, 4 rows.
    assert_eq!(picture_rows(&f), 4);
    // Half blocks draw a solid picture as cells in its colour.
    let mut terminal = Terminal::new(TestBackend::new(200, 60)).unwrap();
    terminal
        .draw(|frame| f.screen.view(frame, &Theme::default()))
        .unwrap();
    let red = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .filter(|cell| cell.bg == Color::Rgb(200, 30, 30))
        .count();
    assert!(red >= 20 * 4 / 2, "{red} cells in the picture's colour");

    // Closing and opening the card again asks for nothing new.
    press(&mut f, KeyCode::Tab).await;
    press(&mut f, KeyCode::Tab).await;
    assert_eq!(image_calls(&f).len(), 1);
}

#[tokio::test]
async fn colon_images_hides_and_shows_the_pictures_and_saves_the_choice() {
    let (mut f, _dir) = with_a_picture().await;
    press(&mut f, KeyCode::Enter).await;
    open_toms_card(&mut f).await;
    assert_eq!(picture_rows(&f), 4);

    run_images(&mut f).await;
    assert!(f.screen.status_line().contains("images: off"));
    assert_eq!(picture_rows(&f), 0);
    let text = std::fs::read_to_string(f.paths.config_file()).unwrap();
    assert!(text.contains("images = false"), "{text}");

    run_images(&mut f).await;
    assert_eq!(picture_rows(&f), 4, "drawn again from what was read");
    assert_eq!(image_calls(&f).len(), 1);
    let text = std::fs::read_to_string(f.paths.config_file()).unwrap();
    assert!(!text.contains("images"), "{text}");
}
