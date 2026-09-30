//! Writes waiting for the user: each one is announced to the UI, then waits
//! for an answer, a timeout, its caller to go away, or its client to be
//! revoked.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use idedb_sql::{Warning, WriteKind};
use serde::Serialize;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{ClientInfo, Decision, Host, McpEvent};

/// How often a waiting write reports progress, so clients that time out
/// idle requests keep waiting.
const PROGRESS_EVERY: Duration = Duration::from_secs(15);

/// A write the user must approve, as the approval dialog shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub id: u64,
    pub client_id: String,
    /// As registered in IdeDB: the verified identity.
    pub client_name: String,
    /// What the client declared itself to be; unverified.
    pub client_info: Option<ClientInfo>,
    pub data_source_id: String,
    pub data_source_name: String,
    pub data_source_color: Option<String>,
    pub sql: String,
    /// The statement type, e.g. `DELETE` or `CREATE TABLE`; for a read the
    /// engine refused as a write, e.g. `SELECT (engine refused as write)`.
    pub summary: String,
    /// Named `writeKind` rather than `kind`, which tags [`McpEvent`].
    pub write_kind: WriteKind,
    pub warnings: Vec<Warning>,
    /// Why the client says it runs the statement.
    pub reason: Option<String>,
    /// UTC, RFC 3339 with milliseconds, like the store's timestamps.
    pub requested_at: String,
    /// When it is refused unless answered.
    pub expires_at: String,
}

/// Reported to the client while a write waits for approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub waited_secs: u64,
    pub timeout_secs: u64,
    pub message: String,
}

/// How a wait for approval ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    Approved,
    Rejected,
    Timeout,
    /// The call was cancelled.
    Cancelled,
    /// The client was revoked or deleted.
    Revoked,
}

impl Answer {
    pub fn decision(self) -> Decision {
        match self {
            Answer::Approved => Decision::Approved,
            Answer::Rejected => Decision::Rejected,
            Answer::Timeout => Decision::Timeout,
            Answer::Cancelled | Answer::Revoked => Decision::Withdrawn,
        }
    }
}

#[derive(Default)]
pub(crate) struct Approvals {
    next_id: AtomicU64,
    pending: Mutex<BTreeMap<u64, Pending>>,
}

struct Pending {
    request: ApprovalRequest,
    answer: oneshot::Sender<bool>,
}

impl Approvals {
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn answer(&self, id: u64, approve: bool) -> bool {
        let Some(pending) = self.pending.lock().unwrap().remove(&id) else { return false };
        pending.answer.send(approve).is_ok()
    }

    pub fn pending(&self) -> Vec<ApprovalRequest> {
        self.pending.lock().unwrap().values().map(|p| p.request.clone()).collect()
    }

    /// Withdraws every pending approval of a client: each of their waits
    /// ends as [`Answer::Revoked`]. Returns how many there were.
    pub fn withdraw_client(&self, client_id: &str) -> usize {
        let mut pending = self.pending.lock().unwrap();
        let ids: Vec<u64> =
            pending.iter().filter(|(_, p)| p.request.client_id == client_id).map(|(id, _)| *id).collect();
        // Dropping an answer's sender wakes its wait.
        ids.iter().filter_map(|id| pending.remove(id)).count()
    }

    /// Registers `request`, announces it, and waits for the user's answer,
    /// at most `timeout`, reporting `progress` every 15 s. `cancel`
    /// withdraws it, and so does [`withdraw_client`](Self::withdraw_client).
    ///
    /// `client_active` is asked once the request is registered, before it
    /// is announced: a client revoked just before then would otherwise
    /// escape `withdraw_client`. Whatever ends the wait (dropping this
    /// future too), the request stops being pending and the UI hears how it
    /// ended.
    pub async fn wait(
        &self,
        host: &dyn Host,
        request: ApprovalRequest,
        timeout: Duration,
        cancel: &CancellationToken,
        progress: &(dyn Fn(Progress) + Send + Sync),
        client_active: &(dyn Fn() -> bool + Send + Sync),
    ) -> Answer {
        let id = request.id;
        let (answer, mut answered) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, Pending { request: request.clone(), answer });
        let mut waiting = Waiting { approvals: self, host, id, decision: Decision::Withdrawn };
        if !client_active() {
            return Answer::Revoked;
        }
        host.notify(McpEvent::ApprovalRequested(request));

        let started = Instant::now();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let mut ticks = tokio::time::interval_at(started + PROGRESS_EVERY, PROGRESS_EVERY);
        let answer = loop {
            tokio::select! {
                // An answer that arrives with the deadline still counts.
                biased;
                answer = &mut answered => break match answer {
                    Ok(true) => Answer::Approved,
                    Ok(false) => Answer::Rejected,
                    // Only `withdraw_client` drops the sender unanswered.
                    Err(_) => Answer::Revoked,
                },
                () = &mut deadline => break Answer::Timeout,
                () = cancel.cancelled() => break Answer::Cancelled,
                _ = ticks.tick() => {
                    let waited_secs = started.elapsed().as_secs();
                    progress(Progress {
                        waited_secs,
                        timeout_secs: timeout.as_secs(),
                        message: format!(
                            "Waiting for the user to approve the statement in IdeDB ({waited_secs} of {} s)",
                            timeout.as_secs()
                        ),
                    });
                }
            }
        };
        waiting.decision = answer.decision();
        answer
    }
}

/// Ends a wait however it ends.
struct Waiting<'a> {
    approvals: &'a Approvals,
    host: &'a dyn Host,
    id: u64,
    decision: Decision,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.approvals.pending.lock().unwrap().remove(&self.id);
        self.host.notify(McpEvent::ApprovalResolved { id: self.id, decision: self.decision });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestHost;

    fn request(id: u64, client_id: &str) -> ApprovalRequest {
        ApprovalRequest {
            id,
            client_id: client_id.into(),
            client_name: client_id.into(),
            client_info: None,
            data_source_id: "ds".into(),
            data_source_name: "shop".into(),
            data_source_color: None,
            sql: "delete from t".into(),
            summary: "DELETE".into(),
            write_kind: WriteKind::Dml,
            warnings: Vec::new(),
            reason: None,
            requested_at: String::new(),
            expires_at: String::new(),
        }
    }

    #[tokio::test]
    async fn a_client_revoked_before_the_announcement_is_never_asked_about() {
        let host = TestHost::new();
        let approvals = Approvals::default();
        let cancel = CancellationToken::new();
        let wait = approvals.wait(&*host, request(1, "c1"), Duration::from_secs(60), &cancel, &|_| {}, &|| false);
        assert_eq!(wait.await, Answer::Revoked);
        assert!(approvals.pending().is_empty());
        let events = host.events();
        assert!(
            matches!(events.as_slice(), [McpEvent::ApprovalResolved { id: 1, decision: Decision::Withdrawn }]),
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn withdrawing_a_client_ends_only_its_waits() {
        let host = TestHost::new();
        let approvals = Approvals::default();
        let cancel = CancellationToken::new();
        let wait = |id, client| {
            approvals.wait(&*host, request(id, client), Duration::from_secs(60), &cancel, &|_| {}, &|| true)
        };
        let (mut first, mut second, mut other) = (Box::pin(wait(1, "c1")), Box::pin(wait(2, "c1")), Box::pin(wait(3, "c2")));
        // Registered and announced.
        for waiting in [&mut first, &mut second, &mut other] {
            assert!(futures_poll(waiting).is_none());
        }
        assert_eq!(approvals.pending().len(), 3);

        assert_eq!(approvals.withdraw_client("c1"), 2);
        assert_eq!((first.await, second.await), (Answer::Revoked, Answer::Revoked));
        assert_eq!(approvals.pending().iter().map(|r| r.id).collect::<Vec<_>>(), [3]);
        assert!(approvals.answer(3, false));
        assert_eq!(other.await, Answer::Rejected);
    }

    /// Polls a future once.
    fn futures_poll<F: std::future::Future + Unpin>(future: &mut F) -> Option<F::Output> {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        match std::pin::Pin::new(future).poll(&mut context) {
            std::task::Poll::Ready(output) => Some(output),
            std::task::Poll::Pending => None,
        }
    }
}
