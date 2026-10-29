//! `attachment.image` through a real daemon over the fake Gmail: the cached copy (private,
//! named by a hash, answered again without a second fetch), and the refusals an unknown
//! Content-ID, a type the reader does not draw, a picture over the cap and bytes that are not
//! what they claim get, each before anything is written; and the one body refresh a message
//! stored without Content-IDs gets (#125).

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::attachments_tests::{
    age, attachment_calls, bytes_at, entries, fetch_policies, fixture, nothing_written,
};
use super::draft_tests::{BOUND, DaemonHarness};
use crate::error::ErrorClass;
use crate::ops::op::{AttachmentImage, SyncNow};
use crate::ops::{
    AccountSyncStatus, AttachmentImageRequest, AttachmentImageResponse, CONTENT_MISMATCH,
    ErrorCode, MAX_IMAGE_BYTES, OpError, SyncNowRequest, names,
};
use crate::provider::http::RetryPolicy;
use crate::provider::mock::Call;
use crate::provider::{AttachmentMeta, Body, ProviderError};
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

#[tokio::test]
async fn a_cached_picture_answered_again_is_not_swept_by_the_next_fetch() {
    let (h, _dest) = fixture().await;
    inline_part(&h, "0.2", "logo@x", "image/png", 16);
    inline_part(&h, "0.3", "other@x", "image/png", 16);
    bytes_at(&h, "0.2", PNG.to_vec());
    bytes_at(&h, "0.3", PNG.to_vec());
    let before = attachment_calls(&h);
    let path = PathBuf::from(image(&h, "logo@x").await.unwrap().path);
    age(&path, 8 * 86_400);

    // Answered from disk, so its age starts again: the sweep before the next write keeps it.
    assert_eq!(
        image(&h, "logo@x").await.unwrap().path,
        path.display().to_string()
    );
    image(&h, "other@x").await.unwrap();
    assert!(path.is_file(), "{:?}", entries(&h.paths.images_dir()));
    assert_eq!(attachment_calls(&h), before + 2, "the hit was not fetched");
}

// Mail stored before schema v11 (or kept after Microsoft 365 refused a list, #125) has its
// attachment rows without a Content-ID: the first ask fetches the body once more.

const STALE_HTML: &str = "<p>Hi</p><img src=\"CID:logo@x\">";

/// m1 as it was stored before Content-IDs were kept: HTML naming `cid:logo@x`, its picture
/// listed with no Content-ID and as a plain file.
fn stored_before_v11(h: &DaemonHarness, html: &str) {
    let message = h.message_id("m1");
    let store = Store::open(&h.paths.db_file()).unwrap();
    store
        .conn()
        .execute(
            "UPDATE messages SET body_text = 'stored text', body_html = ?2,
                                 body_fetched_at = 1 WHERE id = ?1",
            rusqlite::params![message, html],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO attachments (message_id, remote_part_id, filename, mime_type, size,
                                      plain_file, content_id)
             VALUES (?1, '0.2', 'image001.png', 'image/png', 16, 1, NULL)",
            [message],
        )
        .unwrap();
}

/// What the provider lists for m1 now: the picture with its Content-ID, and another.
fn listed_now(h: &DaemonHarness, content_ids: &[&str]) {
    let attachments = content_ids
        .iter()
        .enumerate()
        .map(|(i, id)| AttachmentMeta {
            part_id: format!("0.{}", i + 2),
            filename: format!("image00{i}.png"),
            mime_type: "image/png".into(),
            size: 16,
            plain_file: false,
            content_id: Some((*id).to_string()),
        })
        .collect();
    h.mock.with(|m| {
        m.bodies.insert(
            "m1".into(),
            Body {
                text: "fresh text".into(),
                html: Some(STALE_HTML.into()),
                attachments,
                ..Body::default()
            },
        );
    });
    for i in 0..content_ids.len() {
        bytes_at(h, &format!("0.{}", i + 2), PNG.to_vec());
    }
}

fn body_calls(h: &DaemonHarness) -> usize {
    h.mock.calls().iter().filter(|c| **c == Call::Body).count()
}

fn stored_text(h: &DaemonHarness) -> String {
    h.store(|s| {
        s.conn()
            .query_row(
                "SELECT body_text FROM messages WHERE remote_id = 'm1'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    })
}

fn assert_not_found(error: &OpError) {
    assert_eq!(
        (error.code, error.class),
        (ErrorCode::NotFound, ErrorClass::Permanent),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_picture_of_mail_stored_before_content_ids_refreshes_its_body_once() {
    let (h, _dest) = fixture().await;
    stored_before_v11(&h, STALE_HTML);
    listed_now(&h, &["logo@x"]);
    let before = body_calls(&h);

    let answer = image(&h, "logo@x").await.unwrap();
    assert_eq!(std::fs::read(&answer.path).unwrap(), PNG);
    assert_eq!(body_calls(&h), before + 1, "one body fetch");
    let row: (Option<String>, bool) = h.store(|s| {
        s.conn()
            .query_row(
                "SELECT content_id, plain_file FROM attachments WHERE remote_part_id = '0.2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    });
    assert_eq!(row, (Some("logo@x".to_string()), false));
    let body_policy = h.mock.with(|m| {
        m.policies
            .iter()
            .rev()
            .find(|(call, _)| *call == Call::Body)
            .map(|(_, policy)| *policy)
    });
    assert_eq!(body_policy, Some(Some(RetryPolicy::interactive())));

    image(&h, "logo@x").await.unwrap();
    assert_eq!(body_calls(&h), before + 1, "not fetched again");
}

#[tokio::test]
async fn concurrent_asks_for_one_messages_pictures_fetch_its_body_once() {
    let (h, _dest) = fixture().await;
    stored_before_v11(&h, STALE_HTML);
    listed_now(&h, &["logo@x", "a@x", "b@x", "c@x"]);
    h.mock
        .with(|m| m.body_delay = std::time::Duration::from_millis(300));
    let before = body_calls(&h);

    let asks = ["logo@x", "a@x", "b@x", "c@x"].map(|id| image(&h, id));
    for answer in futures_util::future::join_all(asks).await {
        answer.unwrap();
    }
    assert_eq!(body_calls(&h), before + 1);
}

#[tokio::test]
async fn a_refreshed_list_without_the_content_id_is_not_found_and_not_fetched_again() {
    let (h, _dest) = fixture().await;
    stored_before_v11(&h, STALE_HTML);
    // Gmail lists an inline part without a filename with no Content-ID Rail keeps.
    listed_now(&h, &[]);
    let before = body_calls(&h);

    assert_not_found(&image(&h, "logo@x").await.expect_err("still none"));
    assert_eq!(body_calls(&h), before + 1);
    assert_eq!(
        stored_text(&h),
        "fresh text",
        "the refreshed body was written"
    );
    assert_not_found(&image(&h, "logo@x").await.expect_err("still none"));
    assert_eq!(body_calls(&h), before + 1, "once per message per run");
}

#[tokio::test]
async fn a_transient_body_error_is_answered_and_an_ask_after_the_cooldown_tries_again() {
    let h =
        DaemonHarness::new_with(|ctx| ctx.with_inline_refresh_cooldown(std::time::Duration::ZERO))
            .await;
    stored_before_v11(&h, STALE_HTML);
    listed_now(&h, &["logo@x"]);
    h.mock
        .fail_next(Call::Body, ProviderError::transient("the server is busy"));
    let before = body_calls(&h);

    let error = image(&h, "logo@x").await.expect_err("transient");
    assert_eq!(error.class, ErrorClass::Transient, "{error:?}");
    assert_eq!(body_calls(&h), before + 1);

    image(&h, "logo@x").await.unwrap();
    assert_eq!(body_calls(&h), before + 2, "tried again");
}

#[tokio::test]
async fn an_unreadable_body_leaves_the_stored_one_and_is_not_fetched_again() {
    let (h, _dest) = fixture().await;
    stored_before_v11(&h, STALE_HTML);
    h.mock.with(|m| m.broken.insert("m1".into()));
    let before = body_calls(&h);

    assert_not_found(&image(&h, "logo@x").await.expect_err("unreadable"));
    assert_eq!(body_calls(&h), before + 1);
    assert_eq!(
        stored_text(&h),
        "stored text",
        "no unreadable marker over it"
    );
    assert_not_found(&image(&h, "logo@x").await.expect_err("unreadable"));
    assert_eq!(body_calls(&h), before + 1);
}

#[tokio::test]
async fn a_message_whose_html_names_no_cid_is_never_refreshed() {
    let (h, _dest) = fixture().await;
    stored_before_v11(&h, "<p>no pictures, cid without a colon</p>");
    listed_now(&h, &["logo@x"]);
    let before = body_calls(&h);

    assert_not_found(&image(&h, "logo@x").await.expect_err("nothing to refresh"));
    let request = AttachmentImageRequest {
        account: "work".into(),
        message_id: "nobody".into(),
        content_id: "logo@x".into(),
    };
    let error = h
        .call::<AttachmentImage>(&request)
        .await
        .expect_err("no such message");
    assert_not_found(&error);
    assert_eq!(body_calls(&h), before);
}

#[tokio::test]
async fn a_refreshed_body_tells_sync_progress_watchers_so_the_reader_reads_it_again() {
    let (h, _dest) = fixture().await;
    stored_before_v11(&h, STALE_HTML);
    listed_now(&h, &["logo@x"]);
    let mut notes = h.client.notifications();
    let watch = SyncNowRequest {
        account: None,
        watch: true,
    };
    h.call::<SyncNow>(&watch).await.unwrap();
    // Let the pass sync.now started end first: its own notes say nothing here.
    tokio::time::timeout(BOUND, async {
        loop {
            let note = notes.recv().await.unwrap();
            let status: AccountSyncStatus = serde_json::from_value(note.params).unwrap();
            if !status.pass_running && status.passes_completed > 1 {
                return;
            }
        }
    })
    .await
    .expect("the pass ended");

    image(&h, "logo@x").await.unwrap();
    let note = tokio::time::timeout(std::time::Duration::from_secs(2), notes.recv())
        .await
        .expect("a progress note after the refresh")
        .unwrap();
    assert_eq!(note.method, names::SYNC_PROGRESS);
}

#[tokio::test]
async fn concurrent_asks_share_one_transient_failure() {
    let (h, _dest) = fixture().await;
    stored_before_v11(&h, STALE_HTML);
    listed_now(&h, &["logo@x", "a@x", "b@x", "c@x"]);
    h.mock
        .with(|m| m.body_delay = std::time::Duration::from_millis(300));
    for _ in 0..4 {
        h.mock
            .fail_next(Call::Body, ProviderError::transient("the server is busy"));
    }
    let before = body_calls(&h);

    let asks = ["logo@x", "a@x", "b@x", "c@x"].map(|id| image(&h, id));
    for answer in futures_util::future::join_all(asks).await {
        let error = answer.expect_err("transient");
        assert_eq!(error.class, ErrorClass::Transient, "{error:?}");
    }
    assert_eq!(body_calls(&h), before + 1, "one fetch, its failure shared");
    let error = image(&h, "logo@x").await.expect_err("within the cooldown");
    assert_eq!(error.class, ErrorClass::Transient, "{error:?}");
    assert_eq!(
        body_calls(&h),
        before + 1,
        "not fetched within the cooldown"
    );
}

#[tokio::test]
async fn a_message_gone_at_the_provider_is_not_found_unwritten_and_not_fetched_again() {
    let (h, _dest) = fixture().await;
    stored_before_v11(&h, STALE_HTML);
    h.mock.fail_next(Call::Body, ProviderError::NotFound);
    let before = body_calls(&h);

    assert_not_found(&image(&h, "logo@x").await.expect_err("gone"));
    assert_eq!(stored_text(&h), "stored text");
    assert_not_found(&image(&h, "logo@x").await.expect_err("gone"));
    assert_eq!(body_calls(&h), before + 1);
}

#[tokio::test]
async fn a_refreshed_body_whose_list_is_refused_keeps_the_old_rows() {
    let (h, _dest) = fixture().await;
    stored_before_v11(&h, STALE_HTML);
    h.mock.with(|m| {
        m.bodies.insert(
            "m1".into(),
            Body {
                text: "fresh text".into(),
                html: Some(STALE_HTML.into()),
                attachments_unknown: true,
                ..Body::default()
            },
        );
    });
    let before = body_calls(&h);

    assert_not_found(&image(&h, "logo@x").await.expect_err("still unlisted"));
    assert_eq!(stored_text(&h), "fresh text", "the body was written");
    let rows: Vec<(String, Option<String>, bool)> = h.store(|s| {
        let mut stmt = s
            .conn()
            .prepare(
                "SELECT a.remote_part_id, a.content_id, a.plain_file FROM attachments a
                 JOIN messages m ON m.id = a.message_id WHERE m.remote_id = 'm1'
                 AND a.remote_part_id = '0.2'",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    });
    assert_eq!(rows, [("0.2".to_string(), None, true)], "the old row stays");
    assert_not_found(&image(&h, "logo@x").await.expect_err("still unlisted"));
    assert_eq!(body_calls(&h), before + 1);
}
