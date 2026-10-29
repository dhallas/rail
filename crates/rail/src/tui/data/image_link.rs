//! `attachment.image` for the reader's pictures: the daemon writes the picture into its cache
//! and answers the path; the job then decodes it on the blocking pool, so a large picture never
//! holds up the screen. A picture that cannot be fetched or decoded stays its placeholder: the
//! log says which step failed, never which picture.

use std::path::PathBuf;

use image::DynamicImage;
use rail_core::ops::op::AttachmentImage;
use rail_core::ops::{AttachmentImageRequest, AttachmentImageResponse, OpError};

use super::daemon_link::Link;
use super::{DaemonLink, MailEvent, job, send};
use crate::tui::app::Effect;
use crate::tui::screens::reader::decode_picture;

impl DaemonLink {
    /// The picture `content_id` of message `message_id` (its provider id `remote_id`), fetched
    /// and decoded.
    pub fn attachment_image(
        &self,
        account: String,
        message_id: i64,
        remote_id: String,
        content_id: String,
    ) -> Effect {
        match self.link.clone() {
            Link::Daemon(connector) => job(move |sink| async move {
                let request = AttachmentImageRequest {
                    account,
                    message_id: remote_id,
                    content_id: content_id.clone(),
                };
                let answer = connector.call::<AttachmentImage>(&request).await;
                let image = decoded(answer).await;
                send(
                    &sink,
                    MailEvent::Picture {
                        message_id,
                        content_id,
                        image,
                    },
                );
            }),
            #[cfg(test)]
            Link::Fake(fake) => job(move |sink| async move {
                let answer = fake.attachment_image(&account, &remote_id, &content_id);
                let image = decoded(answer).await;
                send(
                    &sink,
                    MailEvent::Picture {
                        message_id,
                        content_id,
                        image,
                    },
                );
            }),
        }
    }
}

async fn decoded(answer: Result<AttachmentImageResponse, OpError>) -> Option<DynamicImage> {
    let response = match answer {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!(code = ?error.code, "an inline picture was not fetched");
            return None;
        }
    };
    let path = PathBuf::from(response.path);
    match tokio::task::spawn_blocking(move || decode_picture(&path)).await {
        Ok(Ok(image)) => Some(image),
        Ok(Err(_)) | Err(_) => {
            tracing::warn!("an inline picture could not be decoded");
            None
        }
    }
}

#[cfg(test)]
impl super::FakeDaemon {
    /// Record the call and answer it from `image_answers` by Content-ID: not found when the
    /// test set none.
    fn attachment_image(
        &self,
        account: &str,
        message_id: &str,
        content_id: &str,
    ) -> Result<AttachmentImageResponse, OpError> {
        self.record(super::FakeCall::AttachmentImage {
            account: account.to_string(),
            message_id: message_id.to_string(),
            content_id: content_id.to_string(),
        });
        self.image_answers
            .lock()
            .unwrap()
            .get(content_id)
            .cloned()
            .unwrap_or_else(|| Err(OpError::not_found("no such inline picture")))
    }
}
