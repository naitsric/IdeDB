//! Writes waiting for the user: each one is announced to the UI, then waits
//! for an answer, a timeout, or its caller to go away.

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
    /// The statement type, e.g. `DELETE` or `CREATE TABLE`.
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

    /// Announces `request` and waits for the user's answer, at most
    /// `timeout`, reporting `progress` every 15 s. `cancel` withdraws it.
    /// Whatever ends the wait (dropping this future too), the request stops
    /// being pending and the UI hears how it ended.
    pub async fn wait(
        &self,
        host: &dyn Host,
        request: ApprovalRequest,
        timeout: Duration,
        cancel: &CancellationToken,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Decision {
        let id = request.id;
        let (answer, mut answered) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, Pending { request: request.clone(), answer });
        let mut waiting = Waiting { approvals: self, host, id, decision: Decision::Withdrawn };
        host.notify(McpEvent::ApprovalRequested(request));

        let started = Instant::now();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let mut ticks = tokio::time::interval_at(started + PROGRESS_EVERY, PROGRESS_EVERY);
        waiting.decision = loop {
            tokio::select! {
                // An answer that arrives with the deadline still counts.
                biased;
                answer = &mut answered => break match answer {
                    Ok(true) => Decision::Approved,
                    Ok(false) => Decision::Rejected,
                    Err(_) => Decision::Withdrawn,
                },
                () = &mut deadline => break Decision::Timeout,
                () = cancel.cancelled() => break Decision::Withdrawn,
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
        waiting.decision
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
