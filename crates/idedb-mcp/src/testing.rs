//! A [`Host`] for tests: an in-memory store and secrets, and every event
//! kept. Built with the `testing` feature.

use std::path::Path;
use std::sync::{Arc, Mutex};

use idedb_core::{ConnectionParams, Engine, Fetch, QueryEvent, SslMode};
use idedb_drivers::AnySession;
use idedb_store::{Access, AuditEntry, DataSource, Grant, MemorySecrets, SecretStore, Store};
use tokio::sync::broadcast;

use crate::{Caller, Host, McpEvent, Transport, new_token};

pub struct TestHost {
    pub store: Store,
    pub secrets: MemorySecrets,
    events: Mutex<Vec<McpEvent>>,
    sender: broadcast::Sender<McpEvent>,
}

impl Host for TestHost {
    fn store(&self) -> &Store {
        &self.store
    }

    fn secrets(&self) -> &dyn SecretStore {
        &self.secrets
    }

    fn notify(&self, event: McpEvent) {
        self.events.lock().unwrap().push(event.clone());
        let _ = self.sender.send(event);
    }
}

impl TestHost {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            store: Store::in_memory().expect("in-memory store"),
            secrets: MemorySecrets::default(),
            events: Mutex::default(),
            sender: broadcast::channel(1024).0,
        })
    }

    /// Every event so far, oldest first.
    pub fn events(&self) -> Vec<McpEvent> {
        self.events.lock().unwrap().clone()
    }

    pub fn clear_events(&self) {
        self.events.lock().unwrap().clear();
    }

    /// Events from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<McpEvent> {
        self.sender.subscribe()
    }

    /// The audit rows notified so far, oldest first.
    pub fn audit(&self) -> Vec<AuditEntry> {
        let events = self.events.lock().unwrap();
        events
            .iter()
            .filter_map(|event| match event {
                McpEvent::Audit(entry) => Some(entry.clone()),
                _ => None,
            })
            .collect()
    }

    /// The last audit row notified.
    pub fn last_audit(&self) -> AuditEntry {
        self.audit().pop().expect("an audit row")
    }

    /// Saves a SQLite data source on `path`, creating an empty database
    /// there when there is no file (SQLite reads an empty file as one).
    pub fn save_sqlite(&self, name: &str, path: &Path) -> DataSource {
        if !path.exists() {
            std::fs::File::create(path).expect("create the database file");
        }
        let params = ConnectionParams {
            engine: Engine::Sqlite,
            host: String::new(),
            port: None,
            user: String::new(),
            database: String::new(),
            ssl_mode: SslMode::Disable,
            path: path.to_str().expect("a UTF-8 path").to_owned(),
        };
        self.save(DataSource { id: String::new(), name: name.into(), params, color: None, save_password: false })
    }

    pub fn save(&self, source: DataSource) -> DataSource {
        self.store.save(source).expect("save the data source")
    }

    /// Registers a client with these grants; returns it as a caller over
    /// HTTP, and its token.
    pub fn client(&self, name: &str, grants: &[(&DataSource, Access)]) -> (Caller, String) {
        let (token, hash, prefix) = new_token();
        let client = self.store.mcp_client_create(name, &hash, &prefix).expect("create the client");
        self.grant(&client.id, grants);
        let caller = Caller {
            client_id: client.id,
            client_name: client.name,
            client_info: None,
            protocol_version: None,
            transport: Transport::Http,
            session_key: None,
        };
        (caller, token)
    }

    /// Replaces the client's grants.
    pub fn grant(&self, client_id: &str, grants: &[(&DataSource, Access)]) {
        let grants: Vec<Grant> =
            grants.iter().map(|(source, access)| Grant { data_source_id: source.id.clone(), access: *access }).collect();
        self.store.mcp_set_grants(client_id, &grants).expect("set grants").expect("the client exists");
    }
}

/// Runs `sql` on a new read-write session of `source`, returning its rows;
/// panics if it fails.
pub async fn exec(source: &DataSource, password: Option<&str>, sql: &str) -> Vec<idedb_core::Row> {
    let mut session = AnySession::connect(&source.params, password).await.expect("connect");
    let mut rows = Vec::new();
    let mut last = None;
    session
        .execute(sql, Fetch::all(1000), &mut |event| match event {
            QueryEvent::Rows { rows: page } => rows.extend(page),
            QueryEvent::Columns { .. } => {}
            other => last = Some(other),
        })
        .await;
    match last {
        Some(QueryEvent::Done { .. }) => rows,
        other => panic!("{sql}: {other:?}"),
    }
}
