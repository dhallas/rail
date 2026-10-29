//! `attachment.image` through a real daemon over the fake Gmail: the cached copy (private,
//! named by a hash, answered again without a second fetch), and the refusals an unknown
//! Content-ID, a type the reader does not draw, a picture over the cap and bytes that are not
//! what they claim get, each before anything is written.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::attachments_tests::{
    age, attachment_calls, bytes_at, entries, fetch_policies, fixture, nothing_written,
};
use super::draft_tests::DaemonHarness;
use crate::error::ErrorClass;
use crate::ops::op::AttachmentImage;
use crate::ops::{
    AttachmentImageRequest, AttachmentImageResponse, CONTENT_MISMATCH, ErrorCode, MAX_IMAGE_BYTES,
    OpError,
};
use crate::provider::http::RetryPolicy;
use crate::store::Store;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// Tell the store m1 has this inline part, as a body fetch would.
fn inline_part(h: &DaemonHarness, part: &str, content_id: &str, mime: &str, size: i64) {
    let message = h.message_id("m1");
    Store::open(&h.paths.db_file())
        .unwrap()
        .conn()
        .execute(
            "INSERT INTO attachments (message_id, remote_part_id, filename, mime_type, size,
                                      content_id)
             VALUES (?1, ?2, 'image001.png', ?3, ?4, ?5)",
            rusqlite::params![message, part, mime, size, content_id],
        )
        .unwrap();
}

async fn image(h: &DaemonHarness, content_id: &str) -> Result<AttachmentImageResponse, OpError> {
    let request = AttachmentImageRequest {
        account: "work".into(),
        message_id: "m1".into(),
        content_id: content_id.into(),
    };
    h.call::<AttachmentImage>(&request).await
}

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn an_inline_picture_is_cached_privately_and_answered_again_without_a_fetch() {
    let (h, _dest) = fixture().await;
    inline_part(&h, "0.2", "logo@x", "image/png", 16);
    bytes_at(&h, "0.2", PNG.to_vec());
    let before = attachment_calls(&h);

    let first = image(&h, "logo@x").await.unwrap();
    let path = PathBuf::from(&first.path);
    assert_eq!(first.mime_type, "image/png");
    assert_eq!(std::fs::read(&path).unwrap(), PNG);
    assert_eq!(path.parent(), Some(h.paths.images_dir().as_path()));
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&h.paths.images_dir()), 0o700);
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    assert!(name.ends_with(".png") && !name.contains("logo"), "{name}");
    assert_eq!(fetch_policies(&h), [Some(RetryPolicy::interactive())]);

    let again = image(&h, "logo@x").await.unwrap();
    assert_eq!(again.path, first.path);
    assert_eq!(attachment_calls(&h), before + 1, "fetched once");
}

#[tokio::test]
async fn an_unknown_content_id_is_not_found_without_a_fetch_or_a_quote() {
    let (h, _dest) = fixture().await;
    let before = attachment_calls(&h);
    let error = image(&h, "nobody@x").await.expect_err("no such part");
    assert_eq!(
        (error.code, error.class),
        (ErrorCode::NotFound, ErrorClass::Permanent)
    );
    assert!(!error.to_string().contains("nobody"), "{error}");
    assert_eq!(attachment_calls(&h), before);
    nothing_written(&h.paths.images_dir());
}

#[tokio::test]
async fn a_type_the_reader_does_not_draw_and_a_picture_over_the_cap_are_refused_unfetched() {
    let (h, _dest) = fixture().await;
    inline_part(&h, "0.3", "drawing@x", "image/svg+xml", 10);
    inline_part(&h, "0.4", "huge@x", "image/png", MAX_IMAGE_BYTES + 1);
    let before = attachment_calls(&h);
    for id in ["drawing@x", "huge@x"] {
        let error = image(&h, id).await.expect_err(id);
        assert_eq!(
            (error.code, error.class),
            (ErrorCode::Unsupported, ErrorClass::Permanent)
        );
    }
    assert_eq!(attachment_calls(&h), before);
    nothing_written(&h.paths.images_dir());
}

#[tokio::test]
async fn bytes_that_are_not_what_they_claim_or_too_many_are_refused_and_not_kept() {
    let (h, _dest) = fixture().await;
    inline_part(&h, "0.5", "fake@x", "image/png", 10);
    bytes_at(&h, "0.5", b"<svg onload=alert(1)>".to_vec());
    let error = image(&h, "fake@x").await.expect_err("not a PNG");
    assert_eq!(error.code, ErrorCode::Unsupported);
    assert_eq!(error.message, CONTENT_MISMATCH);

    // The listed size is the provider's word; the bytes are counted too.
    inline_part(&h, "0.6", "liar@x", "image/png", 10);
    let mut big = PNG.to_vec();
    big.resize(usize::try_from(MAX_IMAGE_BYTES).unwrap() + 1, 0);
    bytes_at(&h, "0.6", big);
    let error = image(&h, "liar@x").await.expect_err("over the cap");
    assert_eq!(error.code, ErrorCode::Unsupported);
    nothing_written(&h.paths.images_dir());
}

#[tokio::test]
async fn a_malformed_content_id_is_refused() {
    let (h, _dest) = fixture().await;
    for id in ["", "<logo@x>", "a b", &"x".repeat(300)] {
        let error = image(&h, id).await.expect_err(id);
        assert_eq!(error.code, ErrorCode::InvalidInput, "{id}");
    }
}

#[tokio::test]
async fn a_fetch_sweeps_pictures_cached_more_than_a_week_ago() {
    let (h, _dest) = fixture().await;
    let dir = h.paths.images_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let (old, recent) = (dir.join("old.png"), dir.join("recent.png"));
    for path in [&old, &recent] {
        std::fs::write(path, PNG).unwrap();
    }
    age(&old, 8 * 86_400);
    age(&recent, 86_400);
    inline_part(&h, "0.2", "logo@x", "image/png", 16);
    bytes_at(&h, "0.2", PNG.to_vec());
    image(&h, "logo@x").await.unwrap();
    let left = entries(&dir);
    assert!(!left.contains(&"old.png".to_string()), "{left:?}");
    assert!(left.contains(&"recent.png".to_string()), "{left:?}");
    assert_eq!(left.len(), 2, "{left:?}");
}
