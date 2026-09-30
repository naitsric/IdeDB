//! Read-only sessions kept open between tool calls, one per client and data
//! source. A session opens on first use and closes after five minutes
//! without one; past 16 sessions the least recently used one closes. Each
//! counts against the server's connection limit, hence the cap.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use idedb_core::ConnectOptions;
use idedb_drivers::{AnyCanceller, AnySession, OpenError, resolve_data_source};
use idedb_store::DataSource;
use tokio::sync::OnceCell;
use tokio::time::Instant;

use crate::Host;

const IDLE_AFTER: Duration = Duration::from_secs(5 * 60);
const MAX_SESSIONS: usize = 16;

/// (client id, data source id)
pub(crate) type Key = (String, String);

pub(crate) struct Pool {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    cap: usize,
    idle: Duration,
    /// Sessions opened so far, to tell a reused session from a new one.
    opened: AtomicUsize,
}

#[derive(Default)]
struct State {
    entries: HashMap<Key, Arc<Entry>>,
    /// Whether the task closing idle sessions runs; it stops when the pool
    /// is empty.
    reaping: bool,
}

struct Entry {
    opened: OnceCell<Opened>,
    last_used: Mutex<Instant>,
}

/// A pooled session and what it was opened for.
pub(crate) struct Opened {
    /// The data source as it was when the session opened.
    pub data_source: DataSource,
    /// Where unqualified names resolved when the session opened.
    pub default_schema: Option<String>,
    canceller: Mutex<AnyCanceller>,
    pub session: tokio::sync::Mutex<Pooled>,
}

pub(crate) struct Pooled {
    pub session: AnySession,
    /// Where unqualified names resolve now.
    pub schema: Option<String>,
}

/// Why a session could not be opened.
#[derive(Debug)]
pub(crate) enum OpenFailure {
    Open(OpenError),
    /// The blocking task resolving the data source panicked.
    Worker(String),
}

impl Opened {
    /// Cancels whatever the session runs.
    pub fn canceller(&self) -> AnyCanceller {
        self.canceller.lock().unwrap().clone()
    }

    /// Replaces the session with a new one on the same data source, e.g. to
    /// go back to no current database, which MySQL cannot do otherwise.
    pub async fn reopen(&self, pooled: &mut Pooled, host: &Arc<dyn Host>) -> Result<(), OpenFailure> {
        let (_, session) = connect(host, &self.data_source.id, ConnectOptions { read_only: true }).await?;
        *self.canceller.lock().unwrap() = session.canceller();
        pooled.schema = session.server_info().default_schema.clone();
        pooled.session = session;
        Ok(())
    }
}

/// A pooled session in use. Using it counts as activity until it is
/// dropped.
pub(crate) struct Checkout {
    pool: Arc<Inner>,
    key: Key,
    entry: Arc<Entry>,
}

impl Checkout {
    pub fn opened(&self) -> &Opened {
        self.entry.opened.get().expect("a checkout is only handed out once opened")
    }

    /// Takes the session out of the pool, for when its state can no longer
    /// be trusted: it closes once this checkout is dropped.
    pub fn evict(&self) {
        let mut state = self.pool.state.lock().unwrap();
        if state.entries.get(&self.key).is_some_and(|entry| Arc::ptr_eq(entry, &self.entry)) {
            state.entries.remove(&self.key);
        }
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        self.entry.touch();
    }
}

impl Entry {
    fn touch(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    fn last_used(&self) -> Instant {
        *self.last_used.lock().unwrap()
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self::new(MAX_SESSIONS, IDLE_AFTER)
    }
}

impl Pool {
    pub fn new(cap: usize, idle: Duration) -> Self {
        Self { inner: Arc::new(Inner { state: Mutex::default(), cap, idle, opened: AtomicUsize::new(0) }) }
    }

    /// The client's session on `source`, opened read only if there is none.
    /// A session opened on an older version of the data source is replaced.
    pub async fn checkout(
        &self,
        host: &Arc<dyn Host>,
        client_id: &str,
        source: &DataSource,
    ) -> Result<Checkout, OpenFailure> {
        let key = (client_id.to_owned(), source.id.clone());
        let (entry, evicted) = {
            let mut state = self.inner.state.lock().unwrap();
            let stale = state.entries.get(&key).and_then(|e| e.opened.get()).is_some_and(|o| o.data_source != *source);
            let replaced = if stale { state.entries.remove(&key) } else { None };
            let entry = state
                .entries
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Entry { opened: OnceCell::new(), last_used: Mutex::new(Instant::now()) }))
                .clone();
            entry.touch();
            let mut evicted = self.inner.over_cap(&mut state, &key);
            evicted.extend(replaced);
            if !state.reaping {
                state.reaping = true;
                tokio::spawn(reap(Arc::downgrade(&self.inner)));
            }
            (entry, evicted)
        };
        // Closed outside the lock.
        drop(evicted);

        let opened = entry
            .opened
            .get_or_try_init(|| async {
                let (data_source, session) = connect(host, &source.id, ConnectOptions { read_only: true }).await?;
                self.inner.opened.fetch_add(1, Ordering::Relaxed);
                let default_schema = session.server_info().default_schema.clone();
                Ok(Opened {
                    data_source,
                    default_schema: default_schema.clone(),
                    canceller: Mutex::new(session.canceller()),
                    session: tokio::sync::Mutex::new(Pooled { session, schema: default_schema }),
                })
            })
            .await
            .map(drop);
        let checkout = Checkout { pool: self.inner.clone(), key, entry };
        if let Err(failure) = opened {
            checkout.evict();
            return Err(failure);
        }
        Ok(checkout)
    }

    /// Takes the sessions whose key matches out of the pool and cancels what
    /// they run; each closes once its last user is done.
    pub async fn close(&self, matches: impl Fn(&Key) -> bool) {
        let closed: Vec<Arc<Entry>> = {
            let mut state = self.inner.state.lock().unwrap();
            let keys: Vec<Key> = state.entries.keys().filter(|key| matches(key)).cloned().collect();
            keys.iter().filter_map(|key| state.entries.remove(key)).collect()
        };
        for entry in closed {
            if let Some(opened) = entry.opened.get() {
                let _ = opened.canceller().cancel().await;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.inner.state.lock().unwrap().entries.len()
    }

    /// Sessions opened since the pool was created.
    #[cfg(test)]
    pub fn opened(&self) -> usize {
        self.inner.opened.load(Ordering::Relaxed)
    }
}

impl Inner {
    /// Takes the least recently used sessions out until the pool is within
    /// its cap, never the one for `keep`.
    fn over_cap(&self, state: &mut State, keep: &Key) -> Vec<Arc<Entry>> {
        let mut evicted = Vec::new();
        while state.entries.len() > self.cap {
            let oldest = state
                .entries
                .iter()
                .filter(|(key, _)| *key != keep)
                .min_by_key(|(_, entry)| entry.last_used())
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            evicted.extend(state.entries.remove(&oldest));
        }
        evicted
    }
}

/// Closes sessions idle for longer than the pool's `idle`, checking every
/// tenth of it, until the pool is empty or gone.
async fn reap(pool: Weak<Inner>) {
    let every = pool.upgrade().map_or(IDLE_AFTER, |inner| inner.idle) / 10;
    loop {
        tokio::time::sleep(every).await;
        let Some(inner) = pool.upgrade() else { return };
        let idle: Vec<Arc<Entry>> = {
            let mut state = inner.state.lock().unwrap();
            let now = Instant::now();
            // Only the pool holds an entry nobody is using or opening.
            let keys: Vec<Key> = state
                .entries
                .iter()
                .filter(|(_, entry)| Arc::strong_count(entry) == 1 && now - entry.last_used() >= inner.idle)
                .map(|(key, _)| key.clone())
                .collect();
            let idle = keys.iter().filter_map(|key| state.entries.remove(key)).collect();
            if state.entries.is_empty() {
                state.reaping = false;
                return;
            }
            idle
        };
        drop(idle);
    }
}

/// Opens a session on the saved data source `id`, without a password of
/// its own: v1 only uses saved ones. Looking the data source and its
/// password up blocks (the Keychain may even ask the user), so it runs on
/// a blocking thread.
pub(crate) async fn connect(
    host: &Arc<dyn Host>,
    id: &str,
    options: ConnectOptions,
) -> Result<(DataSource, AnySession), OpenFailure> {
    let (host, id) = (host.clone(), id.to_owned());
    let resolved = tokio::task::spawn_blocking(move || resolve_data_source(host.store(), host.secrets(), &id, None))
        .await
        .map_err(|e| OpenFailure::Worker(e.to_string()))?;
    let (source, password) = resolved.map_err(OpenFailure::Open)?;
    let session = AnySession::connect_with(&source.params, password.as_deref(), options)
        .await
        .map_err(|e| OpenFailure::Open(OpenError::Driver(e)))?;
    Ok((source, session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestHost;

    struct Fixture {
        test: Arc<TestHost>,
        host: Arc<dyn Host>,
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let test = TestHost::new();
            Self { host: test.clone(), test, dir: tempfile::tempdir().unwrap() }
        }

        fn source(&self, name: &str) -> DataSource {
            self.test.save_sqlite(name, &self.dir.path().join(format!("{name}.db")))
        }

        async fn checkout(&self, pool: &Pool, client: &str, source: &DataSource) -> Checkout {
            pool.checkout(&self.host, client, source).await.map_err(|e| format!("{e:?}")).unwrap()
        }
    }

    #[tokio::test]
    async fn reuses_a_session_per_client_and_data_source() {
        let f = Fixture::new();
        let (a, b) = (f.source("a"), f.source("b"));
        let pool = Pool::default();

        let first = f.checkout(&pool, "c1", &a).await;
        let first_session = &raw const first.opened().session;
        drop(first);
        let again = f.checkout(&pool, "c1", &a).await;
        assert!(std::ptr::eq(&raw const again.opened().session, first_session));
        assert_eq!(pool.opened(), 1);

        // Another client or data source gets its own.
        f.checkout(&pool, "c2", &a).await;
        f.checkout(&pool, "c1", &b).await;
        assert_eq!((pool.opened(), pool.len()), (3, 3));
    }

    #[tokio::test]
    async fn an_edited_data_source_gets_a_new_session() {
        let f = Fixture::new();
        let source = f.source("a");
        let pool = Pool::default();
        f.checkout(&pool, "c1", &source).await;

        let renamed = f.test.save(DataSource { name: "renamed".into(), ..source });
        let fresh = f.checkout(&pool, "c1", &renamed).await;
        assert_eq!(fresh.opened().data_source.name, "renamed");
        assert_eq!((pool.opened(), pool.len()), (2, 1));
    }

    #[tokio::test(start_paused = true)]
    async fn closes_idle_sessions() {
        let f = Fixture::new();
        let source = f.source("a");
        let pool = Pool::default();

        let busy = f.checkout(&pool, "c1", &source).await;
        tokio::time::sleep(IDLE_AFTER * 2).await;
        // In use: never idle.
        assert_eq!(pool.len(), 1);
        drop(busy);

        tokio::time::sleep(IDLE_AFTER / 2).await;
        assert_eq!(pool.len(), 1);
        tokio::time::sleep(IDLE_AFTER).await;
        assert_eq!(pool.len(), 0);

        // The reaper stopped with the pool empty; a new session starts it again.
        f.checkout(&pool, "c1", &source).await;
        tokio::time::sleep(IDLE_AFTER * 2).await;
        assert_eq!((pool.len(), pool.opened()), (0, 2));
    }

    #[tokio::test]
    async fn closes_the_least_recently_used_past_the_cap() {
        let f = Fixture::new();
        let sources: Vec<DataSource> = (0..3).map(|i| f.source(&format!("s{i}"))).collect();
        let pool = Pool::new(2, IDLE_AFTER);

        f.checkout(&pool, "c", &sources[0]).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        f.checkout(&pool, "c", &sources[1]).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        // s0 is used again, so s1 is now the least recently used.
        f.checkout(&pool, "c", &sources[0]).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        f.checkout(&pool, "c", &sources[2]).await;

        assert_eq!(pool.len(), 2);
        let state = pool.inner.state.lock().unwrap();
        let open: Vec<bool> = sources.iter().map(|s| state.entries.contains_key(&("c".into(), s.id.clone()))).collect();
        assert_eq!(open, [true, false, true]);
    }

    #[tokio::test]
    async fn closing_takes_sessions_out() {
        let f = Fixture::new();
        let (a, b) = (f.source("a"), f.source("b"));
        let pool = Pool::default();
        for (client, source) in [("c1", &a), ("c2", &a), ("c1", &b)] {
            f.checkout(&pool, client, source).await;
        }

        pool.close(|(_, source)| *source == a.id).await;
        assert_eq!(pool.len(), 1);
        pool.close(|(client, _)| client == "c1").await;
        assert_eq!(pool.len(), 0);
        // The next use opens a new one.
        f.checkout(&pool, "c1", &a).await;
        assert_eq!(pool.opened(), 4);
    }

    #[tokio::test]
    async fn a_failed_open_is_not_pooled() {
        let f = Fixture::new();
        let mut source = f.source("a");
        let missing = f.dir.path().join("missing.db");
        source.params.path = missing.to_str().unwrap().to_owned();
        let source = f.test.save(source);
        let pool = Pool::default();

        let failed = pool.checkout(&f.host, "c1", &source).await;
        assert!(matches!(failed, Err(OpenFailure::Open(OpenError::Driver(_)))), "{:?}", failed.err());
        assert_eq!(pool.len(), 0);
        // Read only never creates the file.
        assert!(!missing.exists());
    }
}
