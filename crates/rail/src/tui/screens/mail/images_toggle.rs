//! `:images`: draw an HTML body's pictures or leave them out, and `:remote-images`: fetch
//! remote pictures always or only once a thread's are loaded, each for the session and in
//! `config.toml`; and asking the daemon for the open messages' pictures, once each. Pictures
//! need rich mode and a terminal Rail could ask about its cells (`MailContext::picker`).

use super::MailScreen;
use super::themes::Saved;
use crate::tui::app::Effect;
use crate::tui::data::MailEvent;
use crate::tui::screens::reader::{DecodedPicture, PictureSource};

impl MailScreen {
    /// Flip images (the open reader's too), say so, and save: `images = false` when off; on
    /// removes the key, since on is the default.
    pub(super) fn toggle_images(&mut self) -> Vec<Effect> {
        self.ctx.images = !self.ctx.images;
        let picker = self.picker();
        if let Some(reader) = &mut self.reader {
            reader.set_images(picker);
        }
        let (notice, value) = if self.ctx.images {
            ("images: on", None)
        } else {
            ("images: off", Some(false))
        };
        let mut effects = self.flip_and_save(
            Some(notice.to_string()),
            "images",
            Saved::Bool(value),
            |result| MailEvent::ImagesSaved { result },
        );
        effects.extend(self.load_pictures());
        effects
    }

    /// Flip remote pictures (the open reader's too, fetching its pictures when on), say so,
    /// and save: `remote_images = true` when on; off removes the key, since off is the default.
    pub(super) fn toggle_remote_images(&mut self) -> Vec<Effect> {
        self.ctx.remote_images = !self.ctx.remote_images;
        if let Some(reader) = &mut self.reader {
            reader.set_remote(self.ctx.remote_images);
        }
        let (notice, value) = if self.ctx.remote_images {
            ("remote images: on", Some(true))
        } else {
            ("remote images: off", None)
        };
        let mut effects = self.flip_and_save(
            Some(notice.to_string()),
            "remote_images",
            Saved::Bool(value),
            |result| MailEvent::RemoteImagesSaved { result },
        );
        effects.extend(self.load_pictures());
        effects
    }

    /// What a reader draws pictures with: none with images off or no terminal to ask.
    pub(super) fn picker(&self) -> Option<ratatui_image::picker::Picker> {
        self.ctx.picker.clone().filter(|_| self.ctx.images)
    }

    /// One `attachment.image` or `image.remote` per open message picture not asked for yet,
    /// as many as the reader has room for (`Reader::pictures_wanted`).
    pub(crate) fn load_pictures(&mut self) -> Vec<Effect> {
        let Some(reader) = self.reader.as_ref() else {
            return Vec::new();
        };
        let account = self
            .account_ids
            .iter()
            .position(|id| *id == Some(reader.account_id()))
            .and_then(|i| self.accounts.get(i))
            .map(|a| a.name.clone());
        let Some(account) = account else {
            return Vec::new();
        };
        let Some(reader) = self.reader.as_mut() else {
            return Vec::new();
        };
        let wanted = reader.pictures_wanted();
        reader.mark_pictures_asked(&wanted);
        wanted
            .into_iter()
            .map(|(id, remote, source)| self.ctx.link.picture(account.clone(), id, remote, source))
            .collect()
    }

    /// A picture read: the open reader's own messages only; then the next ones waiting.
    pub(super) fn picture_read(
        &mut self,
        message_id: i64,
        source: PictureSource,
        image: Option<DecodedPicture>,
    ) -> Vec<Effect> {
        if let Some(reader) = self
            .reader
            .as_mut()
            .filter(|r| r.detail().messages.iter().any(|m| m.id == message_id))
        {
            reader.picture_read(message_id, source, image);
        }
        self.load_pictures()
    }
}
