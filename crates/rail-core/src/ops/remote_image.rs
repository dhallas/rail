//! `image.remote`: one picture a mail's HTML names by its web address, fetched by the daemon
//! into Rail's image cache for the reader to draw, as `attachment.image` does for an inline
//! one: it answers a **path**, never bytes, and the same address is answered from the cache
//! again. Fetching tells the sender the mail was read, so the reader asks only when the person
//! loads a message's remote pictures. Only `https` from public addresses (see the daemon's
//! guarded fetch), at most `MAX_IMAGE_BYTES`. Terminal-only. Nothing here prints the address.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::attachment_image::AttachmentImageResponse;
use super::error::OpError;

/// The longest address Rail fetches, in bytes.
pub const MAX_REMOTE_URL_BYTES: usize = 4096;

/// `image.remote`: the picture at `url`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteImageRequest {
    /// The address an `<img>` names, as written in the mail.
    pub url: String,
}

/// Where the cached copy is, and its type, read from its bytes.
pub type RemoteImageResponse = AttachmentImageResponse;

impl RemoteImageRequest {
    /// Check the address's length; which addresses are fetched is the daemon's to decide.
    ///
    /// # Errors
    /// `InvalidInput`/`Permanent`, never quoting the address.
    pub fn checked(&self) -> Result<(), OpError> {
        if self.url.is_empty() || self.url.len() > MAX_REMOTE_URL_BYTES {
            return Err(OpError::invalid_input(format!(
                "url: 1 to {MAX_REMOTE_URL_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

/// Nothing: the address is the sender's, and can carry who the mail was sent to.
impl fmt::Debug for RemoteImageRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteImageRequest").finish_non_exhaustive()
    }
}
