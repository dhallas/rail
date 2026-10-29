//! The part id a sync wrote, read back: through `thread_detail`, which the reader draws, and
//! through `listed_attachments`, the one source `attachment.save` and `attachment.open` have
//! for a part's filename, type and size.

use super::Store;
use crate::provider::mock::envelope;
use crate::provider::{AttachmentMeta, Body, FolderKind, RemoteFolder};
use crate::sync::batch::Batch;

const BASE_MS: i64 = 1_760_000_000_000;
const NOW: i64 = 1_760_100_000;

/// Two accounts, each with an inbox.
fn seeded() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .conn()
        .execute_batch(
            "INSERT INTO accounts (name, kind, address) VALUES ('work', 'gmail', 'w@example.com');
             INSERT INTO accounts (name, kind, address) VALUES ('home', 'gmail', 'h@example.com');",
        )
        .unwrap();
    for account in [1, 2] {
        let folders = vec![RemoteFolder {
            parent: None,
            remote_id: "INBOX".into(),
            name: "Inbox".into(),
            kind: FolderKind::Inbox,
        }];
        store
            .apply(
                &Batch {
                    folders: Some(folders),
                    ..Batch::new(account)
                },
                NOW,
            )
            .unwrap();
    }
    store
}

fn part(part_id: &str, filename: &str, mime_type: &str, size: i64) -> AttachmentMeta {
    AttachmentMeta {
        part_id: part_id.into(),
        filename: filename.into(),
        mime_type: mime_type.into(),
        size,
        plain_file: true,
        content_id: None,
    }
}

fn parts(listed: &[super::view::AttachmentView]) -> Vec<&str> {
    listed.iter().map(|a| a.part_id.as_str()).collect()
}

/// AC-33: `remote_part_id` reaches `AttachmentView.part_id` both ways, in `attachments.id`
/// order, and a message the account does not have lists nothing.
#[test]
fn part_id_round_trips_and_listed_attachments_reads_it() {
    let mut store = seeded();
    store
        .apply(
            &Batch {
                upserts: vec![envelope("m1", "t1", BASE_MS)],
                ..Batch::new(1)
            },
            NOW,
        )
        .unwrap();
    let body = Body {
        text: "the plan is ready".into(),
        attachments: vec![
            part("0.1", "plan.pdf", "application/pdf", 2048),
            part("0.2", "logo.png", "image/png", 512),
        ],
        ..Body::default()
    };
    store
        .apply(
            &Batch {
                bodies: vec![("m1".into(), body)],
                ..Batch::new(1)
            },
            NOW,
        )
        .unwrap();

    let thread_id: i64 = store
        .conn()
        .query_row("SELECT id FROM threads WHERE remote_id = 't1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let detail = store.thread_detail(thread_id).unwrap().expect("the thread");
    let drawn = &detail.messages[0].attachments;
    assert_eq!(parts(drawn), ["0.1", "0.2"]);
    assert_eq!(drawn[0].filename, "plan.pdf");

    let listed = store.listed_attachments(1, "m1").unwrap();
    assert_eq!(parts(&listed), ["0.1", "0.2"]);
    assert_eq!(listed[0].filename, "plan.pdf");
    assert_eq!(listed[0].mime_type, "application/pdf");
    assert_eq!(listed[0].size, 2048);
    assert_eq!(listed[1].filename, "logo.png");
    assert!(
        store.listed_attachments(1, "nope").unwrap().is_empty(),
        "a message the store does not have"
    );
    assert!(
        store.listed_attachments(2, "m1").unwrap().is_empty(),
        "another account's message"
    );
}

/// An inline part's Content-ID is kept with it, and a part without one stores NULL.
#[test]
fn a_content_id_is_stored_with_its_part() {
    let mut store = seeded();
    store
        .apply(
            &Batch {
                upserts: vec![envelope("m1", "t1", BASE_MS)],
                ..Batch::new(1)
            },
            NOW,
        )
        .unwrap();
    let logo = AttachmentMeta {
        content_id: Some("logo@x".into()),
        ..part("0.2", "logo.png", "image/png", 512)
    };
    let body = Body {
        text: "hi".into(),
        attachments: vec![part("0.1", "plan.pdf", "application/pdf", 2048), logo],
        ..Body::default()
    };
    store
        .apply(
            &Batch {
                bodies: vec![("m1".into(), body)],
                ..Batch::new(1)
            },
            NOW,
        )
        .unwrap();
    let mut stmt = store
        .conn()
        .prepare("SELECT remote_part_id, content_id FROM attachments ORDER BY id")
        .unwrap();
    let rows: Vec<(String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        [
            ("0.1".to_string(), None),
            ("0.2".to_string(), Some("logo@x".to_string()))
        ]
    );
}

/// Migration 11 adds the column to a version-10 store; the parts already there have none.
#[test]
fn migration_eleven_adds_content_id_to_a_version_ten_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rail.db");
    {
        let store = Store::open_at_version(&path, 10).unwrap();
        store
            .conn()
            .execute_batch(
                "INSERT INTO accounts (name, kind, address) VALUES ('work', 'gmail', 'w@example.com');
                 INSERT INTO threads (account_id, remote_id) VALUES (1, 't1');
                 INSERT INTO messages (account_id, thread_id, remote_id) VALUES (1, 1, 'm1');
                 INSERT INTO attachments (message_id, remote_part_id) VALUES (1, 'a1');",
            )
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        super::migrations::LATEST_VERSION
    );
    let id: Option<String> = store
        .conn()
        .query_row("SELECT content_id FROM attachments", [], |r| r.get(0))
        .unwrap();
    assert_eq!(id, None);
}

/// `names_cid`: only a message of that account whose stored HTML has a `cid:` address, in any
/// case; one without HTML, without one, of another account or not stored names none.
#[test]
fn names_cid_finds_a_cid_address_in_the_stored_html() {
    let mut store = seeded();
    let upserts = ["m1", "m2", "m3"]
        .iter()
        .enumerate()
        .map(|(i, id)| envelope(id, &format!("t{i}"), BASE_MS))
        .collect();
    store
        .apply(
            &Batch {
                upserts,
                ..Batch::new(1)
            },
            NOW,
        )
        .unwrap();
    let html = |id: &str, html: Option<&str>| {
        let body = Body {
            text: "hi".into(),
            html: html.map(Into::into),
            ..Body::default()
        };
        (id.to_string(), body)
    };
    let bodies = vec![
        html("m1", Some("<img src=\"CID:logo@x\">")),
        html("m2", Some("<p>a cid without its colon</p>")),
        html("m3", None),
    ];
    store
        .apply(
            &Batch {
                bodies,
                ..Batch::new(1)
            },
            NOW,
        )
        .unwrap();
    assert!(store.names_cid(1, "m1").unwrap());
    assert!(!store.names_cid(1, "m2").unwrap());
    assert!(!store.names_cid(1, "m3").unwrap());
    assert!(!store.names_cid(2, "m1").unwrap(), "another account");
    assert!(!store.names_cid(1, "nobody").unwrap());
}
