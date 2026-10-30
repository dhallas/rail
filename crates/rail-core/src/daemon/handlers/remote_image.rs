//! `image.remote`: one remote picture fetched (`RemoteFetch`, the guarded client) into the same
//! 0700 cache as the inline ones, a 0600 file named by a hash of its address and carrying the
//! extension of the type its bytes are, so the same address is answered from disk the next
//! time without a request. Written and swept as `attachment.image` writes. Logs carry kinds
//! only.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::attachment_image::{cached, write_cached};
use crate::daemon::registry::Call;
use crate::ops::{
    AttachmentImageResponse, IMAGE_TYPES, MAX_IMAGE_BYTES, OpError, RemoteImageRequest,
    image_extension,
};

pub(super) async fn image(
    call: Call,
    request: RemoteImageRequest,
) -> Result<AttachmentImageResponse, OpError> {
    request.checked()?;
    let dir = call.ctx.paths.images_dir();
    let key = cache_key(&request.url);
    let (hit_dir, hit_key) = (dir.clone(), key.clone());
    if let Some((path, mime)) = call
        .ctx
        .blocking(move || Ok(on_disk(&hit_dir, &hit_key)))
        .await?
    {
        return Ok(response(&path, mime));
    }
    let cap = usize::try_from(MAX_IMAGE_BYTES).unwrap_or(usize::MAX);
    let (bytes, mime) = call.ctx.remote.fetch(&request.url, cap).await?;
    let extension = image_extension(mime).unwrap_or("img");
    let path = dir.join(format!("{key}.{extension}"));
    let (paths, target) = (call.ctx.paths.clone(), path.clone());
    call.ctx
        .blocking(move || write_cached(&paths, &target, &bytes))
        .await?;
    Ok(response(&path, mime))
}

fn response(path: &Path, mime: &str) -> AttachmentImageResponse {
    AttachmentImageResponse {
        path: path.display().to_string(),
        mime_type: mime.to_string(),
    }
}

/// The cached copy of `key`, under whichever type's extension it was written with.
fn on_disk(dir: &Path, key: &str) -> Option<(PathBuf, &'static str)> {
    IMAGE_TYPES.iter().find_map(|(mime, extension)| {
        let path = dir.join(format!("{key}.{extension}"));
        cached(&path).then_some((path, *mime))
    })
}

/// The hex SHA-256 of the address, after a tag that keeps it apart from inline pictures' keys:
/// a file name that says nothing of the mail and is the same for the same address.
fn cache_key(url: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"remote\0");
    hash.update(url.as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
