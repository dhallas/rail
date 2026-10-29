//! Rich mode through the mail screen: an HTML message's links and styles drawn once its HTML
//! is read, and `:rich-html` flipping the open reader and new ones, saved to `config.toml`.

use crossterm::event::KeyCode;
use rail_core::provider::html::Style;

use super::{Fixture, fixture, keys, open_toms_card, press, store_bodies_on_fetch};
use crate::tui::modals::Target;
use crate::tui::screens::reader::Tone;

const LINK: Tone = Tone::Rich(Style {
    link: true,
    ..Style::PLAIN
});

/// The tone of the first segment of m1's body that reads `text`.
fn tone_of(f: &Fixture, text: &str) -> Tone {
    let lines = f.screen.reader().unwrap().lines(400);
    lines
        .iter()
        .flat_map(|l| l.segments().iter())
        .find(|(segment, _)| segment.contains(text))
        .map(|(_, tone)| *tone)
        .unwrap_or_else(|| panic!("no segment reads {text:?}"))
}

async fn run_rich_html(f: &mut Fixture) {
    press(f, KeyCode::Char(':')).await;
    keys(f, "rich-html").await;
    press(f, KeyCode::Enter).await;
}

fn has_entry(f: &Fixture, text: &str) -> bool {
    f.screen
        .palette_entries()
        .iter()
        .any(|e| e.target == Target::RichHtml && e.text == text)
}

fn config_text(f: &Fixture) -> String {
    std::fs::read_to_string(f.paths.config_file()).unwrap_or_default()
}

#[tokio::test]
async fn an_html_message_is_drawn_with_its_links_styled_once_opened() {
    let mut f = fixture(false).await;
    store_bodies_on_fetch(&f);
    press(&mut f, KeyCode::Enter).await;
    open_toms_card(&mut f).await;
    assert_eq!(tone_of(&f, "doc[1]"), LINK);
    assert_eq!(tone_of(&f, "Hi Dana,"), Tone::Plain);
    // m3 has no HTML part: its text stays as stored.
    assert_eq!(tone_of(&f, "body of m3"), Tone::Plain);
}

#[tokio::test]
async fn colon_rich_html_flips_the_open_reader_and_new_ones_and_saves_the_choice() {
    let mut f = fixture(false).await;
    store_bodies_on_fetch(&f);
    press(&mut f, KeyCode::Enter).await;
    open_toms_card(&mut f).await;
    assert_eq!(tone_of(&f, "doc[1]"), LINK);
    assert!(has_entry(&f, "Show HTML mail as plain text"));

    run_rich_html(&mut f).await;
    assert!(
        f.screen.status_line().contains("HTML mail: plain text"),
        "{}",
        f.screen.status_line()
    );
    assert_eq!(tone_of(&f, "doc[1]"), Tone::Plain);
    assert!(config_text(&f).contains("rich_html = false"));
    assert!(has_entry(&f, "Show HTML mail with its styles"));

    keys(&mut f, "q").await;
    press(&mut f, KeyCode::Enter).await;
    open_toms_card(&mut f).await;
    assert_eq!(tone_of(&f, "doc[1]"), Tone::Plain, "a new reader is plain");

    run_rich_html(&mut f).await;
    assert_eq!(tone_of(&f, "doc[1]"), LINK, "on again, read now");
    assert!(
        !config_text(&f).contains("rich_html"),
        "on is the default: {}",
        config_text(&f)
    );
    keys(&mut f, "q").await;
    press(&mut f, KeyCode::Enter).await;
    open_toms_card(&mut f).await;
    assert_eq!(tone_of(&f, "doc[1]"), LINK, "a new reader is rich again");
}
