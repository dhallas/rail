//! The reader's rich mode reads a message's HTML part and scans it into styled runs. It lives
//! beside `store_reader.rs` rather than in it because that file is at CONTRIBUTING's 800-line
//! cap.

use rail_core::store::view::HtmlBody;

use super::{MailEvent, StoreReader, job, send};
use crate::tui::app::Effect;
use crate::tui::screens::reader::from_html;

impl StoreReader {
    /// Message `message_id`'s HTML part scanned for the reader's rich mode, off the UI thread;
    /// `None` when it has none.
    pub fn rich_html(&self, message_id: i64) -> Effect {
        let reader = self.clone();
        job(move |sink| async move {
            let result = reader
                .read(move |store| store.message_html(message_id))
                .await
                .map(|html| match html {
                    Some(HtmlBody::Html(html)) => Some(from_html(&html)),
                    Some(HtmlBody::NotFetched | HtmlBody::Absent) | None => None,
                });
            send(&sink, MailEvent::RichHtml { message_id, result });
        })
    }
}
