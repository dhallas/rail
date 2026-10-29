//! The reader's pictures from the daemon: `attachment.image` for an inline one, `image.remote`
//! for a remote one. The daemon writes the picture into its cache and answers the path; the
//! job then decodes and shrinks it on the blocking pool, so a large picture never holds up the
//! screen. A picture that cannot be fetched or decoded stays its placeholder: the log says
//! which step failed, never which picture.

use std::path::PathBuf;

use rail_core::ops::op::{AttachmentImage, ImageRemote};
use rail_core::ops::{
    AttachmentImageRequest, AttachmentImageResponse, OpError, RemoteImageRequest,
};

use rail_core::daemon::Connector;

use super::daemon_link::Link;
use super::{DaemonLink, MailEvent, job, send};
use crate::tui::app::Effect;
use crate::tui::screens::reader::{DecodedPicture, PictureSource, decode_picture};

impl DaemonLink {
    /// The picture `source` of message `message_id` (its provider id `remote_id`), fetched
    /// and decoded.
    pub fn picture(
        &self,
        account: String,
        message_id: i64,
        remote_id: String,
        source: PictureSource,
    ) -> Effect {
        match self.link.clone() {
            Link::Daemon(connector) => job(move |sink| async move {
                let answer = fetched(&connector, account, remote_id, &source).await;
                let image = decoded(answer).await;
                send(
                    &sink,
                    MailEvent::Picture {
                        message_id,
                        source,
                        image,
                    },
                );
            }),
            #[cfg(test)]
            Link::Fake(fake) => job(move |sink| async move {
                let answer = match &source {
                    PictureSource::Inline(content_id) => {
                        fake.attachment_image(&account, &remote_id, content_id)
                    }
                    PictureSource::Remote(url) => fake.remote_image(url),
                };
                let image = decoded(answer).await;
                send(
                    &sink,
                    MailEvent::Picture {
                        message_id,
                        source,
                        image,
                    },
                );
            }),
        }
    }
}

async fn fetched(
    connector: &Connector,
    account: String,
    remote_id: String,
    source: &PictureSource,
) -> Result<AttachmentImageResponse, OpError> {
    match source {
        PictureSource::Inline(content_id) => {
            let request = AttachmentImageRequest {
                account,
                message_id: remote_id,
                content_id: content_id.clone(),
            };
            connector.call::<AttachmentImage>(&request).await
        }
        PictureSource::Remote(url) => {
            let request = RemoteImageRequest { url: url.clone() };
            connector.call::<ImageRemote>(&request).await
        }
    }
}

async fn decoded(answer: Result<AttachmentImageResponse, OpError>) -> Option<DecodedPicture> {
    let response = match answer {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!(code = ?error.code, "a picture was not fetched");
            return None;
        }
    };
    let path = PathBuf::from(response.path);
    match tokio::task::spawn_blocking(move || decode_picture(&path)).await {
        Ok(Ok(image)) => Some(image),
        Ok(Err(_)) | Err(_) => {
            tracing::warn!("a picture could not be decoded");
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
        self.image_answer(content_id)
    }

    /// Record the call and answer it from `image_answers` by address.
    fn remote_image(&self, url: &str) -> Result<AttachmentImageResponse, OpError> {
        self.record(super::FakeCall::RemoteImage(url.to_string()));
        self.image_answer(url)
    }

    fn image_answer(&self, key: &str) -> Result<AttachmentImageResponse, OpError> {
        self.image_answers
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or_else(|| Err(OpError::not_found("no such picture")))
    }
}
