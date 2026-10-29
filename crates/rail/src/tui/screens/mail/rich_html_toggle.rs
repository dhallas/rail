//! `:rich-html`: draw HTML bodies with their styles or as plain text, for the session and in
//! `config.toml`; and reading the open messages' HTML for rich mode, once each.

use super::MailScreen;
use super::themes::Saved;
use crate::tui::app::Effect;
use crate::tui::data::{MailEvent, ReadError};
use crate::tui::screens::reader::RichBody;

impl MailScreen {
    /// Flip rich mode (the open reader's too), say so, and save: `rich_html = false` when off;
    /// on removes the key, since on is the default.
    pub(super) fn toggle_rich_html(&mut self) -> Vec<Effect> {
        let on = !self.ctx.rich_html;
        self.ctx.rich_html = on;
        if let Some(reader) = &mut self.reader {
            reader.set_rich(on);
        }
        let (notice, value) = if on {
            ("HTML mail: with its styles", None)
        } else {
            ("HTML mail: plain text", Some(false))
        };
        let mut effects = self.flip_and_save(
            Some(notice.to_string()),
            "rich_html",
            Saved::Bool(value),
            |result| MailEvent::RichHtmlSaved { result },
        );
        effects.extend(self.load_rich());
        effects
    }

    /// One read per open message whose HTML rich mode still needs.
    pub(crate) fn load_rich(&mut self) -> Vec<Effect> {
        let Some(reader) = self.reader.as_mut() else {
            return Vec::new();
        };
        let ids = reader.rich_wanted();
        reader.mark_rich_asked(&ids);
        let mut effects: Vec<Effect> = ids
            .into_iter()
            .map(|id| self.ctx.store.rich_html(id))
            .collect();
        // Bodies read before may have pictures a message just opened shows.
        effects.extend(self.load_pictures());
        effects
    }

    /// A message's HTML scanned: the open reader's own messages only (it may have closed or
    /// moved on). A failed read leaves the message plain and is reported.
    pub(super) fn rich_read(
        &mut self,
        message_id: i64,
        result: Result<Option<RichBody>, ReadError>,
    ) -> Vec<Effect> {
        let Some(reader) = self
            .reader
            .as_mut()
            .filter(|r| r.detail().messages.iter().any(|m| m.id == message_id))
        else {
            return Vec::new();
        };
        match result {
            Ok(body) => reader.rich_read(message_id, body),
            Err(error) => {
                self.side_read_failed(error, "message");
                return Vec::new();
            }
        }
        self.load_pictures()
    }
}
