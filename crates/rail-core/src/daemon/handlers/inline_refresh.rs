//! The one body refresh `attachment.image` makes for a message the store lists no such inline
//! part for (#125): mail stored before Content-IDs were kept (schema v11), or whose attachment
//! rows were kept after Microsoft 365 refused a list. Only a message whose stored HTML names a
//! `cid:` address is refreshed, at most once per daemon run: callers asking for the same
//! message at once wait for the one fetch, and a message refreshed already (its list refused,
//! its parts without a Content-ID, its body unreadable or gone) is not fetched again. A failure
//! a later ask may not meet (transient, a login needed) is shared with every ask for that
//! message for `FAILURE_COOLDOWN`, then tried again. Logs carry error kinds only.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use crate::daemon::DaemonContext;
use crate::ops::{BodyOutcome, OpError};
use crate::provider::RemoteId;
use crate::provider::http::{RetryPolicy, with_retry_policy};
use crate::store::Store;
use crate::sync::body_fetch;
use crate::sync::manager::SearchTarget;

/// How many messages the record remembers; the oldest not in use is forgotten first, and may
/// then be refreshed once more.
pub(crate) const REMEMBERED_REFRESHES: usize = 1024;

/// How long a failed refresh answers every ask for its message with its error before one is
/// let through to try again: a reader asking for 32 pictures offline makes one fetch, not 32.
pub(crate) const FAILURE_COOLDOWN: Duration = Duration::from_secs(30);

/// A message: its account's store id and its provider id.
type Key = (i64, RemoteId);

/// A message's refresh. Its lock is held across the fetch, so a second ask waits for the first.
type Entry = Arc<tokio::sync::Mutex<State>>;

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Done,
    Failed(Instant, OpError),
}

/// The messages refreshed, being refreshed, or whose refresh failed, in this daemon run.
pub(crate) struct InlineRefreshes {
    inner: Mutex<Remembered>,
    cooldown: Duration,
}

impl Default for InlineRefreshes {
    fn default() -> Self {
        Self::with_cooldown(FAILURE_COOLDOWN)
    }
}

#[derive(Default)]
struct Remembered {
    entries: HashMap<Key, Entry>,
    order: VecDeque<Key>,
}

impl InlineRefreshes {
    pub(crate) fn with_cooldown(cooldown: Duration) -> Self {
        Self {
            inner: Mutex::default(),
            cooldown,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Remembered> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The message's entry when the record has one: its HTML was checked already.
    fn known(&self, key: &Key) -> Option<Entry> {
        self.lock().entries.get(key).cloned()
    }

    /// The message's entry, made when there is none. Past the cap the oldest entry no caller
    /// holds is forgotten; one whose refresh is in flight or awaited is never forgotten, so two
    /// fetches of one message cannot run at once. When every entry is in use (more concurrent
    /// refreshes than the cap, which the daemon's request limits rule out) the record grows
    /// past the cap until they end.
    fn entry(&self, key: Key) -> Entry {
        let mut remembered = self.lock();
        if let Some(entry) = remembered.entries.get(&key) {
            return entry.clone();
        }
        if remembered.order.len() >= REMEMBERED_REFRESHES {
            forget_one_unused(&mut remembered);
        }
        let entry = Entry::default();
        remembered.entries.insert(key.clone(), entry.clone());
        remembered.order.push_back(key);
        entry
    }

    /// Run `refresh` under `entry`'s lock unless it is done, or failed within the cooldown (its
    /// error answered again).
    async fn once<F>(&self, entry: &Entry, refresh: impl FnOnce() -> F) -> Result<(), OpError>
    where
        F: Future<Output = Result<(), OpError>>,
    {
        let mut state = entry.lock().await;
        match &*state {
            State::Done => return Ok(()),
            State::Failed(at, error) if at.elapsed() < self.cooldown => return Err(error.clone()),
            State::Failed(..) | State::Idle => {}
        }
        let result = refresh().await;
        *state = match &result {
            Ok(()) => State::Done,
            Err(error) => State::Failed(Instant::now(), error.clone()),
        };
        result
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().entries.len()
    }
}

/// Forget the oldest entry only the record holds.
fn forget_one_unused(remembered: &mut Remembered) {
    let unused = remembered.order.iter().position(|key| {
        remembered
            .entries
            .get(key)
            .is_none_or(|entry| Arc::strong_count(entry) == 1)
    });
    if let Some(key) = unused.and_then(|index| remembered.order.remove(index)) {
        remembered.entries.remove(&key);
    }
}

/// Fetch message `id`'s body again and write it, once per daemon run, when its stored HTML
/// names a `cid:` address; nothing when it does not, or was refreshed already. A message the
/// record knows is not checked again.
///
/// # Errors
/// The store could not be read, the writer is gone, or the fetch failed in a way a later ask
/// may not (a transient error, a login needed): the same error for `FAILURE_COOLDOWN`.
pub(super) async fn refresh_once(
    ctx: &DaemonContext,
    target: &SearchTarget,
    id: &RemoteId,
) -> Result<(), OpError> {
    let record = &ctx.inline_refreshes;
    let key = (target.account_id, id.clone());
    let entry = match record.known(&key) {
        Some(entry) => entry,
        None if names_cid(ctx, target.account_id, id).await? => record.entry(key),
        None => return Ok(()),
    };
    record.once(&entry, || refresh(ctx, target, id)).await
}

async fn names_cid(ctx: &DaemonContext, account_id: i64, id: &RemoteId) -> Result<bool, OpError> {
    let (db, id) = (ctx.paths.db_file(), id.clone());
    ctx.blocking(move || Ok(Store::open_reader(&db)?.names_cid(account_id, &id)?))
        .await
}

/// One refresh: `Ok` when it is done for this run (the body written, gone, or unreadable).
async fn refresh(ctx: &DaemonContext, target: &SearchTarget, id: &RemoteId) -> Result<(), OpError> {
    // As an open's fetch: one short retry, because a person is waiting.
    let outcome = with_retry_policy(
        RetryPolicy::interactive(),
        body_fetch::refetch(ctx, target, id),
    )
    .await?;
    match outcome {
        BodyOutcome::Fetched => {
            tell_watchers(ctx, &target.name);
            Ok(())
        }
        BodyOutcome::Missing | BodyOutcome::Unreadable => Ok(()),
        BodyOutcome::Failed(error) => {
            tracing::warn!(code = ?error.code, "an inline picture's body refresh failed");
            Err(error)
        }
    }
}

/// Send the account's status to every `sync.progress` watcher unchanged: the reader reads its
/// open thread again on one, so the attachments line shows the refreshed list.
fn tell_watchers(ctx: &DaemonContext, account: &str) {
    let board = ctx.sync.board();
    if let Some(status) = board.all().into_iter().find(|s| s.name == account) {
        board.update(&status.address, |_| {});
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::error::ErrorClass;
    use crate::ops::ErrorCode;

    fn key(account: i64, id: &str) -> Key {
        (account, id.to_string())
    }

    #[test]
    fn the_record_keeps_one_entry_per_message_and_forgets_the_oldest_past_its_cap() {
        let record = InlineRefreshes::default();
        let first = record.entry(key(1, "m0"));
        assert!(Arc::ptr_eq(&first, &record.entry(key(1, "m0"))));
        assert!(!Arc::ptr_eq(&first, &record.entry(key(2, "m0"))));
        drop(first);
        for i in 1..=REMEMBERED_REFRESHES {
            record.entry(key(1, &format!("m{i}")));
        }
        assert_eq!(record.len(), REMEMBERED_REFRESHES);
        assert!(
            record.known(&key(1, "m0")).is_none(),
            "the oldest was forgotten"
        );
        assert!(record.known(&key(1, "m1")).is_some());
    }

    #[test]
    fn an_entry_in_use_is_never_forgotten() {
        let record = InlineRefreshes::default();
        let held = record.entry(key(1, "m0"));
        for i in 1..=REMEMBERED_REFRESHES {
            record.entry(key(1, &format!("m{i}")));
        }
        assert_eq!(record.len(), REMEMBERED_REFRESHES);
        let kept = record.known(&key(1, "m0")).expect("in use, kept");
        assert!(Arc::ptr_eq(&held, &kept));
        assert!(
            record.known(&key(1, "m1")).is_none(),
            "the oldest unused went"
        );
    }

    fn busy() -> OpError {
        OpError::new(ErrorCode::Network, ErrorClass::Transient, "busy")
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_asks_share_one_failure_until_the_cooldown_ends() {
        let record = InlineRefreshes::with_cooldown(Duration::from_secs(30));
        let entry = record.entry(key(1, "m1"));
        let runs = AtomicUsize::new(0);
        let failing = || async {
            runs.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(100)).await;
            Err(busy())
        };
        let asks = (0..8).map(|_| record.once(&entry, failing));
        for answer in futures_util::future::join_all(asks).await {
            assert_eq!(answer.unwrap_err().class, ErrorClass::Transient);
        }
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "one run, its failure shared"
        );

        tokio::time::advance(Duration::from_secs(29)).await;
        assert!(record.once(&entry, failing).await.is_err());
        assert_eq!(runs.load(Ordering::SeqCst), 1, "still cooling down");

        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(record.once(&entry, || async { Ok(()) }).await.is_ok());
        assert!(
            record.once(&entry, failing).await.is_ok(),
            "done: not run again"
        );
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }
}
