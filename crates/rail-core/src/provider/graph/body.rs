//! One message's body: `GET me/messages/{id}?$select=id,body,hasAttachments` with
//! `Prefer: outlook.body-content-type="html"`, rendered as Gmail's are (`gmail::body::render`:
//! HTML through `provider::html`, text as it is, both cut at `MAX_BODY_BYTES`). With
//! `hasAttachments`, the attachment list comes with it, inline ones included, as Gmail lists
//! every named part; each says whether it is a plain file (`reconcile::plain_file`), the only
//! kind a forward carries and an import fetches, and its Content-ID. Graph says
//! `hasAttachments` is false when every attachment is inline: the list is then read only when
//! the HTML names a `cid:` image (in any case). Only that list, refused, leaves the body
//! without attachments rather than unreadable: a refused list Graph says has attachments fails
//! the fetch, since a body stored without them is never fetched again.

use super::reconcile::plain_file;
use super::wire::{AttachmentWire, BodyWire, Page};
use super::{COST_REQUEST, GraphProvider, graph_id};
use crate::provider::gmail::body::{Kind, render};
use crate::provider::gmail::metadata::one_line_capped;
use crate::provider::limits::{MAX_ATTACHMENTS, MAX_FILENAME_CHARS};
use crate::provider::types::content_id;
use crate::provider::{AttachmentMeta, Body, ProviderError};

/// The preference that asks for the HTML body.
pub const HTML_BODY: &str = "outlook.body-content-type=\"html\"";
const MAX_MIME_CHARS: usize = 255;

fn unreadable() -> ProviderError {
    ProviderError::permanent("Rail could not read this message's body")
}

fn meta(wire: AttachmentWire) -> AttachmentMeta {
    let mime = wire
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream")
        .to_ascii_lowercase();
    AttachmentMeta {
        filename: one_line_capped(
            wire.name.as_deref().unwrap_or("attachment"),
            MAX_FILENAME_CHARS,
        ),
        mime_type: one_line_capped(&mime, MAX_MIME_CHARS),
        size: wire.size.unwrap_or(0).max(0),
        plain_file: plain_file(wire.odata_type.as_deref(), wire.is_inline),
        content_id: wire.content_id.as_deref().and_then(content_id),
        part_id: wire.id,
    }
}

/// Whether HTML names a `cid:` image, in any case (`CID:` is the same scheme).
fn names_cid(html: &str) -> bool {
    html.as_bytes()
        .windows(4)
        .any(|w| w.eq_ignore_ascii_case(b"cid:"))
}

impl GraphProvider {
    pub(crate) async fn read_body(&self, id: &str) -> Result<Body, ProviderError> {
        if !graph_id(id) {
            return Err(unreadable());
        }
        let query = [("$select", "id,body,hasAttachments".to_string())];
        let path = format!("me/messages/{id}");
        let wire: BodyWire = self
            .api
            .get_json_with(&path, &query, COST_REQUEST, &[HTML_BODY])
            .await?;
        if wire.id.as_deref() != Some(id) {
            return Err(unreadable());
        }
        let body = wire.body.unwrap_or_default();
        let kind = match body.content_type.as_deref() {
            Some(text) if text.eq_ignore_ascii_case("text") => Kind::Plain,
            _ => Kind::Html,
        };
        let content = body.content.unwrap_or_default();
        // Graph says `hasAttachments` is false when every attachment is inline, so HTML that
        // names a `cid:` image is listed anyway.
        let attachments = if wire.has_attachments == Some(true) {
            self.attachment_list(id).await?
        } else if kind == Kind::Html && names_cid(&content) {
            self.inline_list(id).await?
        } else {
            Vec::new()
        };
        tokio::task::spawn_blocking(move || render(kind, content.as_bytes(), None, attachments))
            .await
            .map_err(|_| unreadable())?
    }

    /// The list asked for on a `cid:` alone. The body is readable without it: a refused
    /// listing must not cost it. Anything a retry may fix (or a login) still fails the fetch.
    async fn inline_list(&self, id: &str) -> Result<Vec<AttachmentMeta>, ProviderError> {
        match self.attachment_list(id).await {
            Err(error @ ProviderError::Permanent { .. }) => {
                tracing::warn!(class = ?error.class(), detail = ?error.detail(), "the attachment list was refused; the body is kept without it");
                Ok(Vec::new())
            }
            listed => listed,
        }
    }

    async fn attachment_list(&self, id: &str) -> Result<Vec<AttachmentMeta>, ProviderError> {
        let query = [
            // `contentId` is a file attachment's own property: Graph refuses it on the base
            // attachment type (HTTP 400), so it is asked for through a type cast.
            (
                "$select",
                "id,name,contentType,size,isInline,microsoft.graph.fileAttachment/contentId"
                    .to_string(),
            ),
            ("$top", MAX_ATTACHMENTS.to_string()),
        ];
        let path = format!("me/messages/{id}/attachments");
        let page: Page<AttachmentWire> = self.api.get_json(&path, &query, COST_REQUEST).await?;
        Ok(page
            .value
            .into_iter()
            .filter(|a| graph_id(&a.id))
            .take(MAX_ATTACHMENTS)
            .map(meta)
            .collect())
    }
}
