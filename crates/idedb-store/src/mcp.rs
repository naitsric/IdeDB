//! State of the MCP server: registered clients (only a hash of each token is
//! stored), what each one may do per data source, per data source policy,
//! and the audit log of tool calls, newest first.
//!
//! Timestamps are UTC, RFC 3339 with milliseconds, like query history.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, Value, ValueRef};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::{Result, Store};

/// Audit rows kept; older ones are pruned on insert.
const AUDIT_RETENTION: usize = 20_000;

// Longest text an audit row keeps per field, in bytes. Longer values are cut
// on a char boundary; only cut SQL is flagged (`sql_truncated`).
const MAX_SQL: usize = 100 * 1024;
const MAX_ERROR: usize = 16 * 1024;
const MAX_REASON: usize = 4 * 1024;
/// Every other field: names, versions, ids, keys.
const MAX_SHORT: usize = 256;

/// What a client may do on a data source. Writes also need the user's
/// approval each time, and none happen on a never-write data source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub data_source_id: String,
    pub access: Access,
}

/// A registered MCP client. Its token's hash is never read back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpClient {
    pub id: String,
    pub name: String,
    /// The token's first characters, to tell tokens apart.
    pub token_prefix: String,
    pub created_at: String,
    /// Its last authenticated request.
    pub last_seen_at: Option<String>,
    /// The `clientInfo` it last declared; unverified.
    pub last_client_name: Option<String>,
    pub last_client_version: Option<String>,
    /// Set once revoked: its token no longer authenticates.
    pub revoked_at: Option<String>,
    /// By data source id.
    pub grants: Vec<Grant>,
}

/// How a tool call was let through or stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// Ran without asking.
    Allowed,
    /// The user approved it.
    Approved,
    /// The user rejected it.
    Rejected,
    /// Refused without asking.
    Denied,
    /// Nobody answered the approval in time.
    Timeout,
    /// The request went away while waiting for approval.
    Withdrawn,
}

/// How the client reached the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// Streamable HTTP, directly.
    Http,
    /// Through the stdio bridge.
    Bridge,
}

/// Stored as the same text serde uses.
macro_rules! text_enum {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(self.as_str().into())
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                match value.as_str()? {
                    $($text => Ok(Self::$variant),)+
                    other => Err(FromSqlError::Other(format!("unknown {} {other:?}", stringify!($name)).into())),
                }
            }
        }
    };
}

text_enum!(Access { Read => "read", Write => "write" });
text_enum!(Decision {
    Allowed => "allowed",
    Approved => "approved",
    Rejected => "rejected",
    Denied => "denied",
    Timeout => "timeout",
    Withdrawn => "withdrawn",
});
text_enum!(Transport { Http => "http", Bridge => "bridge" });

/// One tool call, as [`Store::mcp_add_audit`] records it. Client and data
/// source names are copied so the row outlives both.
///
/// Text is capped when stored: `sql` at 100 KiB, `error` at 16 KiB,
/// `reason` at 4 KiB and every other field at 256 bytes.
#[derive(Debug, Clone, Copy)]
pub struct NewAuditEntry<'a> {
    pub client_id: Option<&'a str>,
    pub client_name: &'a str,
    /// The `clientInfo` the client declared; unverified.
    pub client_info_name: Option<&'a str>,
    pub client_info_version: Option<&'a str>,
    pub protocol_version: Option<&'a str>,
    pub transport: Transport,
    /// The legacy `Mcp-Session-Id`, or the bridge's instance id.
    pub session_key: Option<&'a str>,
    pub tool: &'a str,
    pub data_source_id: Option<&'a str>,
    pub data_source_name: Option<&'a str>,
    pub sql: Option<&'a str>,
    pub statement_kind: Option<&'a str>,
    /// Why the client says it runs the statement.
    pub reason: Option<&'a str>,
    pub decision: Decision,
    pub approval_wait_ms: Option<u64>,
    pub elapsed_ms: Option<u64>,
    /// Rows returned, or affected for statements without a result set.
    pub row_count: Option<u64>,
    /// The result sent back was cut short.
    pub truncated: bool,
    pub error: Option<&'a str>,
}

/// A recorded [`NewAuditEntry`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: i64,
    pub at: String,
    pub client_id: Option<String>,
    pub client_name: String,
    pub client_info_name: Option<String>,
    pub client_info_version: Option<String>,
    pub protocol_version: Option<String>,
    pub transport: Transport,
    pub session_key: Option<String>,
    pub tool: String,
    pub data_source_id: Option<String>,
    pub data_source_name: Option<String>,
    pub sql: Option<String>,
    /// The SQL was longer and is cut to its first 100 KiB.
    pub sql_truncated: bool,
    pub statement_kind: Option<String>,
    pub reason: Option<String>,
    pub decision: Decision,
    pub approval_wait_ms: Option<u64>,
    pub elapsed_ms: Option<u64>,
    pub row_count: Option<u64>,
    pub truncated: bool,
    pub error: Option<String>,
}

/// Which audit rows [`Store::mcp_audit`] returns: those matching every
/// condition given, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuditFilter {
    pub client_id: Option<String>,
    pub data_source_id: Option<String>,
    pub decision: Option<Decision>,
    /// Contained in the SQL, case-insensitive for ASCII. Empty matches all.
    pub search: Option<String>,
    /// Only rows older than this one: the last id of the previous page.
    pub before_id: Option<i64>,
    pub limit: u32,
}

impl Default for AuditFilter {
    fn default() -> Self {
        Self { client_id: None, data_source_id: None, decision: None, search: None, before_id: None, limit: 100 }
    }
}

const CLIENT_COLUMNS: &str =
    "id, name, token_prefix, created_at, last_seen_at, last_client_name, last_client_version, revoked_at";

const AUDIT_COLUMNS: &str = "id, at, client_id, client_name, client_info_name, client_info_version, \
    protocol_version, transport, session_key, tool, data_source_id, data_source_name, sql, sql_truncated, \
    statement_kind, reason, decision, approval_wait_ms, elapsed_ms, row_count, truncated, error";

impl Store {
    /// Revoked ones included, oldest first.
    pub fn mcp_clients(&self) -> Result<Vec<McpClient>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(&format!("select {CLIENT_COLUMNS} from mcp_client order by created_at, rowid"))?;
        let clients = stmt.query_map([], client_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        clients.into_iter().map(|client| with_grants(&db, client)).collect()
    }

    /// Registers a client, with no grants, for a token of which only the
    /// hash is kept.
    pub fn mcp_client_create(&self, name: &str, token_hash: &[u8; 32], token_prefix: &str) -> Result<McpClient> {
        let id = uuid::Uuid::new_v4().to_string();
        let db = self.db.lock().unwrap();
        db.execute(
            "insert into mcp_client (id, name, token_hash, token_prefix, created_at)
             values (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![id, name, token_hash, token_prefix],
        )?;
        Ok(client_by_id(&db, &id)?.expect("client just inserted"))
    }

    /// The client this token authenticates; never a revoked one.
    pub fn mcp_client_by_token_hash(&self, token_hash: &[u8; 32]) -> Result<Option<McpClient>> {
        let db = self.db.lock().unwrap();
        let client = db
            .query_row(
                &format!("select {CLIENT_COLUMNS} from mcp_client where token_hash = ?1 and revoked_at is null"),
                [token_hash],
                client_row,
            )
            .optional()?;
        client.map(|client| with_grants(&db, client)).transpose()
    }

    /// The renamed client, or None if there is no such client.
    pub fn mcp_client_rename(&self, id: &str, name: &str) -> Result<Option<McpClient>> {
        let db = self.db.lock().unwrap();
        db.execute("update mcp_client set name = ?2 where id = ?1", params![id, name])?;
        client_by_id(&db, id)
    }

    /// Its token stops authenticating at once. Revoking again keeps the
    /// first time. None if there is no such client.
    pub fn mcp_client_revoke(&self, id: &str) -> Result<Option<McpClient>> {
        let db = self.db.lock().unwrap();
        db.execute(
            "update mcp_client set revoked_at = coalesce(revoked_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             where id = ?1",
            [id],
        )?;
        client_by_id(&db, id)
    }

    /// Replaces the token; the old one stops authenticating at once. None,
    /// and nothing changed, if there is no such client or it is revoked: a
    /// revoked client gets no new token.
    pub fn mcp_client_rotate(&self, id: &str, token_hash: &[u8; 32], token_prefix: &str) -> Result<Option<McpClient>> {
        let db = self.db.lock().unwrap();
        let rotated = db.execute(
            "update mcp_client set token_hash = ?2, token_prefix = ?3 where id = ?1 and revoked_at is null",
            params![id, token_hash, token_prefix],
        )?;
        if rotated == 0 {
            return Ok(None);
        }
        client_by_id(&db, id)
    }

    /// With its grants. Its audit rows stay, under the name they recorded.
    pub fn mcp_client_delete(&self, id: &str) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute("delete from mcp_grant where client_id = ?1", [id])?;
        tx.execute("delete from mcp_client where id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// Marks the client seen now, with the `clientInfo` it declared. Without
    /// a `client_name` the last declared name and version are kept.
    pub fn mcp_touch_client(&self, id: &str, client_name: Option<&str>, client_version: Option<&str>) -> Result<()> {
        self.db.lock().unwrap().execute(
            "update mcp_client set
               last_seen_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
               last_client_name = coalesce(?2, last_client_name),
               last_client_version = case when ?2 is null then last_client_version else ?3 end
             where id = ?1",
            params![id, client_name, client_version],
        )?;
        Ok(())
    }

    /// Replaces all of the client's grants at once and returns the client
    /// with the grants kept, or None, and nothing changed, if there is no
    /// such client. Grants on data sources that do not exist (any more) are
    /// dropped; a data source listed twice keeps the last access.
    pub fn mcp_set_grants(&self, client_id: &str, grants: &[Grant]) -> Result<Option<McpClient>> {
        let mut db = self.db.lock().unwrap();
        // Takes the write lock first, so what is checked here still holds
        // when the grants are written, whatever another process does.
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if client_by_id(&tx, client_id)?.is_none() {
            return Ok(None);
        }
        tx.execute("delete from mcp_grant where client_id = ?1", [client_id])?;
        {
            let mut insert = tx.prepare(
                "insert into mcp_grant (client_id, data_source_id, access)
                 select ?1, ?2, ?3 where exists (select 1 from data_source where id = ?2)
                 on conflict (client_id, data_source_id) do update set access = excluded.access",
            )?;
            for grant in grants {
                insert.execute(params![client_id, grant.data_source_id, grant.access])?;
            }
        }
        let client = client_by_id(&tx, client_id)?;
        tx.commit()?;
        Ok(client)
    }

    /// Whether writes on the data source are refused without asking,
    /// whatever the grants. False unless set.
    pub fn mcp_never_write(&self, data_source_id: &str) -> Result<bool> {
        let db = self.db.lock().unwrap();
        let never_write = db
            .query_row("select never_write from mcp_data_source where data_source_id = ?1", [data_source_id], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(never_write.unwrap_or(false))
    }

    /// False, and nothing stored, if there is no such data source: a policy
    /// never outlives its data source.
    pub fn mcp_set_never_write(&self, data_source_id: &str, never_write: bool) -> Result<bool> {
        let stored = self.db.lock().unwrap().execute(
            "insert into mcp_data_source (data_source_id, never_write)
             select ?1, ?2 where exists (select 1 from data_source where id = ?1)
             on conflict (data_source_id) do update set never_write = excluded.never_write",
            params![data_source_id, never_write],
        )?;
        Ok(stored > 0)
    }

    /// Records a tool call, stamped now, and returns it as stored.
    pub fn mcp_add_audit(&self, entry: NewAuditEntry) -> Result<AuditEntry> {
        self.mcp_add_audit_retaining(entry, AUDIT_RETENTION)
    }

    fn mcp_add_audit_retaining(&self, entry: NewAuditEntry, keep: usize) -> Result<AuditEntry> {
        fn short(text: &str) -> &str {
            cut(text, MAX_SHORT)
        }
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "insert into mcp_audit (at, client_id, client_name, client_info_name, client_info_version,
               protocol_version, transport, session_key, tool, data_source_id, data_source_name, sql,
               sql_truncated, statement_kind, reason, decision, approval_wait_ms, elapsed_ms, row_count,
               truncated, error)
             values (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
               ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            params![
                entry.client_id.map(short),
                short(entry.client_name),
                entry.client_info_name.map(short),
                entry.client_info_version.map(short),
                entry.protocol_version.map(short),
                entry.transport,
                entry.session_key.map(short),
                short(entry.tool),
                entry.data_source_id.map(short),
                entry.data_source_name.map(short),
                entry.sql.map(|sql| cut(sql, MAX_SQL)),
                entry.sql.is_some_and(|sql| sql.len() > MAX_SQL),
                entry.statement_kind.map(short),
                entry.reason.map(|reason| cut(reason, MAX_REASON)),
                entry.decision,
                entry.approval_wait_ms.map(|n| n as i64),
                entry.elapsed_ms.map(|n| n as i64),
                entry.row_count.map(|n| n as i64),
                entry.truncated,
                entry.error.map(|error| cut(error, MAX_ERROR)),
            ],
        )?;
        let id = tx.last_insert_rowid();
        // Rows only ever go from the oldest end, so ids are consecutive and
        // the newest `keep` rows are exactly those above `id - keep`.
        tx.execute("delete from mcp_audit where id <= ?1", [id - keep as i64])?;
        let stored = tx.query_row(&format!("select {AUDIT_COLUMNS} from mcp_audit where id = ?1"), [id], audit_row)?;
        tx.commit()?;
        Ok(stored)
    }

    /// Newest first; see [`AuditFilter`].
    pub fn mcp_audit(&self, filter: &AuditFilter) -> Result<Vec<AuditEntry>> {
        // Only the conditions given, so SQLite can use the (client_id, id)
        // and (data_source_id, id) indexes.
        let mut conditions = Vec::new();
        let mut args: Vec<Value> = Vec::new();
        if let Some(id) = &filter.client_id {
            conditions.push("client_id = ?");
            args.push(id.clone().into());
        }
        if let Some(id) = &filter.data_source_id {
            conditions.push("data_source_id = ?");
            args.push(id.clone().into());
        }
        if let Some(decision) = filter.decision {
            conditions.push("decision = ?");
            args.push(decision.as_str().to_owned().into());
        }
        if let Some(search) = filter.search.as_deref().filter(|s| !s.is_empty()) {
            conditions.push("instr(lower(sql), lower(?)) > 0");
            args.push(search.to_owned().into());
        }
        if let Some(id) = filter.before_id {
            conditions.push("id < ?");
            args.push(id.into());
        }
        args.push(i64::from(filter.limit).into());

        let mut sql = format!("select {AUDIT_COLUMNS} from mcp_audit");
        if !conditions.is_empty() {
            sql.push_str(" where ");
            sql.push_str(&conditions.join(" and "));
        }
        sql.push_str(" order by id desc limit ?");

        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), audit_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// Drops the MCP grants and policy of a deleted data source, inside
/// [`Store::delete`]'s transaction.
pub(crate) fn forget_data_source(db: &Connection, data_source_id: &str) -> Result<()> {
    db.execute("delete from mcp_grant where data_source_id = ?1", [data_source_id])?;
    db.execute("delete from mcp_data_source where data_source_id = ?1", [data_source_id])?;
    Ok(())
}

/// `text` cut to at most `max` bytes, on a char boundary.
fn cut(text: &str, max: usize) -> &str {
    &text[..text.floor_char_boundary(max)]
}

fn client_by_id(db: &Connection, id: &str) -> Result<Option<McpClient>> {
    let client =
        db.query_row(&format!("select {CLIENT_COLUMNS} from mcp_client where id = ?1"), [id], client_row).optional()?;
    client.map(|client| with_grants(db, client)).transpose()
}

fn client_row(row: &Row) -> rusqlite::Result<McpClient> {
    Ok(McpClient {
        id: row.get(0)?,
        name: row.get(1)?,
        token_prefix: row.get(2)?,
        created_at: row.get(3)?,
        last_seen_at: row.get(4)?,
        last_client_name: row.get(5)?,
        last_client_version: row.get(6)?,
        revoked_at: row.get(7)?,
        grants: Vec::new(),
    })
}

fn with_grants(db: &Connection, mut client: McpClient) -> Result<McpClient> {
    let mut stmt =
        db.prepare("select data_source_id, access from mcp_grant where client_id = ?1 order by data_source_id")?;
    client.grants = stmt
        .query_map([&client.id], |row| Ok(Grant { data_source_id: row.get(0)?, access: row.get(1)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(client)
}

fn audit_row(row: &Row) -> rusqlite::Result<AuditEntry> {
    let count = |i: usize| -> rusqlite::Result<Option<u64>> { Ok(row.get::<_, Option<i64>>(i)?.map(|n| n as u64)) };
    Ok(AuditEntry {
        id: row.get(0)?,
        at: row.get(1)?,
        client_id: row.get(2)?,
        client_name: row.get(3)?,
        client_info_name: row.get(4)?,
        client_info_version: row.get(5)?,
        protocol_version: row.get(6)?,
        transport: row.get(7)?,
        session_key: row.get(8)?,
        tool: row.get(9)?,
        data_source_id: row.get(10)?,
        data_source_name: row.get(11)?,
        sql: row.get(12)?,
        sql_truncated: row.get(13)?,
        statement_kind: row.get(14)?,
        reason: row.get(15)?,
        decision: row.get(16)?,
        approval_wait_ms: count(17)?,
        elapsed_ms: count(18)?,
        row_count: count(19)?,
        truncated: row.get(20)?,
        error: row.get(21)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DataSource, NewHistoryEntry};
    use idedb_core::{ConnectionParams, Engine, SslMode};

    fn hash(n: u8) -> [u8; 32] {
        [n; 32]
    }

    fn grant(data_source_id: &str, access: Access) -> Grant {
        Grant { data_source_id: data_source_id.into(), access }
    }

    fn entry<'a>(client_id: &'a str, data_source_id: &'a str, sql: &'a str) -> NewAuditEntry<'a> {
        NewAuditEntry {
            client_id: Some(client_id),
            client_name: "claude-code",
            client_info_name: Some("claude-code"),
            client_info_version: Some("2.1.0"),
            protocol_version: Some("2026-07-28"),
            transport: Transport::Http,
            session_key: None,
            tool: "query",
            data_source_id: Some(data_source_id),
            data_source_name: Some("shop"),
            sql: Some(sql),
            statement_kind: Some("select"),
            reason: None,
            decision: Decision::Allowed,
            approval_wait_ms: None,
            elapsed_ms: Some(4),
            row_count: Some(2),
            truncated: false,
            error: None,
        }
    }

    fn sqls(entries: &[AuditEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.sql.as_deref().unwrap_or_default()).collect()
    }

    fn count(store: &Store, sql: &str, id: &str) -> i64 {
        store.db.lock().unwrap().query_row(sql, [id], |r| r.get(0)).unwrap()
    }

    fn sqlite_source(name: &str) -> DataSource {
        DataSource {
            id: String::new(),
            name: name.into(),
            params: ConnectionParams {
                engine: Engine::Sqlite,
                host: String::new(),
                port: None,
                user: String::new(),
                database: String::new(),
                ssl_mode: SslMode::Prefer,
                path: format!("{name}.db"),
            },
            color: None,
            save_password: false,
        }
    }

    /// Saves a data source; grants and policy only apply to existing ones.
    fn saved(store: &Store, name: &str) -> String {
        store.save(sqlite_source(name)).unwrap().id
    }

    #[test]
    fn migrates_a_store_from_before_mcp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idedb.db");
        {
            // A store as v0.1.2 left it: data sources and query history.
            let db = Connection::open(&path).unwrap();
            for migration in &crate::MIGRATIONS[..2] {
                db.execute_batch(migration).unwrap();
            }
            db.pragma_update(None, "user_version", 2).unwrap();
            db.execute(
                "insert into data_source (id, name, params, color, save_password, position)
                 values ('ds', 'shop', '{\"engine\":\"sqlite\",\"path\":\"shop.db\"}', '#e5484d', 0, 1)",
                [],
            )
            .unwrap();
            db.execute(
                "insert into query_history (data_source_id, sql, executed_at, elapsed_ms, row_count)
                 values ('ds', 'select 1', '2026-09-25T15:04:05.123Z', 3, 1)",
                [],
            )
            .unwrap();
        }

        let store = Store::open(&path).unwrap();
        let sources = store.list().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!((sources[0].id.as_str(), sources[0].name.as_str()), ("ds", "shop"));
        assert_eq!(sources[0].params.path, "shop.db");
        assert_eq!(sources[0].color.as_deref(), Some("#e5484d"));
        let history = store.history(Some("ds"), None, 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].executed_at, "2026-09-25T15:04:05.123Z");

        // The new tables are there and keep their rows across a reopen.
        store.set_setting("mcp", "{}").unwrap();
        let client = store.mcp_client_create("claude-code", &hash(1), "idedb_ab").unwrap();
        store.mcp_set_grants(&client.id, &[grant("ds", Access::Read)]).unwrap();
        store.mcp_set_never_write("ds", true).unwrap();
        store.mcp_add_audit(entry(&client.id, "ds", "select 1")).unwrap();
        drop(store);

        let store = Store::open(&path).unwrap();
        assert_eq!(store.setting("mcp").unwrap().as_deref(), Some("{}"));
        assert_eq!(store.mcp_clients().unwrap()[0].grants, [grant("ds", Access::Read)]);
        assert!(store.mcp_never_write("ds").unwrap());
        assert_eq!(store.mcp_audit(&AuditFilter::default()).unwrap().len(), 1);
        let version: i64 = store.db.lock().unwrap().pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, crate::MIGRATIONS.len() as i64);
    }

    #[test]
    fn creates_renames_and_deletes_clients() {
        let store = Store::in_memory().unwrap();
        let a = store.mcp_client_create("claude-code", &hash(1), "idedb_aa").unwrap();
        let b = store.mcp_client_create("cursor", &hash(2), "idedb_bb").unwrap();
        assert!(!a.id.is_empty() && a.id != b.id);
        assert_eq!((a.name.as_str(), a.token_prefix.as_str()), ("claude-code", "idedb_aa"));
        assert!(a.created_at.ends_with('Z') && a.created_at.contains('T'), "{}", a.created_at);
        assert_eq!((&a.last_seen_at, &a.revoked_at), (&None, &None));
        assert!(a.grants.is_empty());
        assert_eq!(store.mcp_clients().unwrap(), [a.clone(), b.clone()]);

        let json = serde_json::to_value(&a).unwrap();
        assert!(json.get("tokenPrefix").is_some() && json.get("createdAt").is_some(), "{json}");
        assert!(json.as_object().unwrap().keys().all(|k| !k.to_lowercase().contains("hash")), "{json}");

        let renamed = store.mcp_client_rename(&a.id, "claude").unwrap().unwrap();
        assert_eq!(renamed, McpClient { name: "claude".into(), ..a.clone() });
        assert_eq!(store.mcp_client_rename("missing", "x").unwrap(), None);

        let ds = saved(&store, "shop");
        let with_grant = store.mcp_set_grants(&a.id, &[grant(&ds, Access::Write)]).unwrap().unwrap();
        assert_eq!(with_grant.grants, [grant(&ds, Access::Write)]);
        store.mcp_add_audit(entry(&a.id, &ds, "select 1")).unwrap();
        store.mcp_client_delete(&a.id).unwrap();
        assert_eq!(store.mcp_clients().unwrap(), [b]);
        assert_eq!(store.mcp_client_by_token_hash(&hash(1)).unwrap(), None);
        assert_eq!(count(&store, "select count(*) from mcp_grant where client_id = ?1", &a.id), 0);
        // Its audit rows stay.
        let audit = store.mcp_audit(&AuditFilter { client_id: Some(a.id.clone()), ..Default::default() }).unwrap();
        assert_eq!(audit.len(), 1);
    }

    #[test]
    fn a_revoked_token_is_not_found() {
        let store = Store::in_memory().unwrap();
        let client = store.mcp_client_create("claude-code", &hash(1), "idedb_aa").unwrap();
        let ds = saved(&store, "shop");
        store.mcp_set_grants(&client.id, &[grant(&ds, Access::Read)]).unwrap();
        let found = store.mcp_client_by_token_hash(&hash(1)).unwrap().unwrap();
        assert_eq!(found.id, client.id);
        assert_eq!(found.grants, [grant(&ds, Access::Read)]);
        assert_eq!(store.mcp_client_by_token_hash(&hash(9)).unwrap(), None);

        let revoked = store.mcp_client_revoke(&client.id).unwrap().unwrap();
        let revoked_at = revoked.revoked_at.clone().expect("revoked_at set");
        assert_eq!(store.mcp_client_by_token_hash(&hash(1)).unwrap(), None);
        // Still listed, and revoking again keeps the first time.
        assert_eq!(store.mcp_clients().unwrap(), [revoked]);
        let again = store.mcp_client_revoke(&client.id).unwrap().unwrap();
        assert_eq!(again.revoked_at, Some(revoked_at));
        assert_eq!(store.mcp_client_revoke("missing").unwrap(), None);
    }

    #[test]
    fn rotating_replaces_the_token() {
        let store = Store::in_memory().unwrap();
        let client = store.mcp_client_create("claude-code", &hash(1), "idedb_aa").unwrap();
        let rotated = store.mcp_client_rotate(&client.id, &hash(2), "idedb_bb").unwrap().unwrap();
        assert_eq!(rotated, McpClient { token_prefix: "idedb_bb".into(), ..client.clone() });

        assert_eq!(store.mcp_client_by_token_hash(&hash(1)).unwrap(), None);
        assert_eq!(store.mcp_client_by_token_hash(&hash(2)).unwrap(), Some(rotated));
        assert_eq!(store.mcp_client_rotate("missing", &hash(3), "idedb_cc").unwrap(), None);

        // A token hash already in use is refused.
        let other = store.mcp_client_create("cursor", &hash(4), "idedb_dd").unwrap();
        assert!(store.mcp_client_rotate(&other.id, &hash(2), "idedb_bb").is_err());
        assert!(store.mcp_client_create("dup", &hash(2), "idedb_bb").is_err());

        // A revoked client gets no new token, and keeps its old prefix.
        let revoked = store.mcp_client_revoke(&other.id).unwrap().unwrap();
        assert_eq!(store.mcp_client_rotate(&other.id, &hash(5), "idedb_ee").unwrap(), None);
        assert_eq!(store.mcp_clients().unwrap()[1], revoked);
        assert_eq!(revoked.token_prefix, "idedb_dd");
        assert_eq!(store.mcp_client_by_token_hash(&hash(5)).unwrap(), None);
        // Nor did the new hash take a place: another client can still use it.
        store.mcp_client_create("fresh", &hash(5), "idedb_ee").unwrap();
    }

    #[test]
    fn touching_records_last_seen_and_client_info() {
        let store = Store::in_memory().unwrap();
        let client = store.mcp_client_create("claude-code", &hash(1), "idedb_aa").unwrap();
        store.mcp_touch_client(&client.id, Some("claude-code"), Some("2.1.0")).unwrap();
        let seen = store.mcp_client_by_token_hash(&hash(1)).unwrap().unwrap();
        assert!(seen.last_seen_at.as_deref().is_some_and(|at| at.ends_with('Z')), "{:?}", seen.last_seen_at);
        assert_eq!(seen.last_client_name.as_deref(), Some("claude-code"));
        assert_eq!(seen.last_client_version.as_deref(), Some("2.1.0"));

        // Without clientInfo the last one is kept; a new name replaces both.
        store.mcp_touch_client(&client.id, None, None).unwrap();
        let kept = store.mcp_client_by_token_hash(&hash(1)).unwrap().unwrap();
        assert_eq!(kept.last_client_name.as_deref(), Some("claude-code"));
        assert_eq!(kept.last_client_version.as_deref(), Some("2.1.0"));
        store.mcp_touch_client(&client.id, Some("cursor"), None).unwrap();
        let replaced = store.mcp_client_by_token_hash(&hash(1)).unwrap().unwrap();
        assert_eq!(replaced.last_client_name.as_deref(), Some("cursor"));
        assert_eq!(replaced.last_client_version, None);
    }

    #[test]
    fn grants_are_replaced_not_appended() {
        let store = Store::in_memory().unwrap();
        let (pg, my, lite) = (saved(&store, "pg"), saved(&store, "my"), saved(&store, "lite"));
        let a = store.mcp_client_create("a", &hash(1), "idedb_aa").unwrap();
        let b = store.mcp_client_create("b", &hash(2), "idedb_bb").unwrap();
        store.mcp_set_grants(&a.id, &[grant(&pg, Access::Read), grant(&lite, Access::Write)]).unwrap();
        store.mcp_set_grants(&b.id, &[grant(&pg, Access::Write)]).unwrap();

        let replacement = [grant(&pg, Access::Write), grant(&my, Access::Read), grant(&my, Access::Write)];
        let replaced = store.mcp_set_grants(&a.id, &replacement).unwrap().unwrap();
        let mut expected = vec![grant(&my, Access::Write), grant(&pg, Access::Write)];
        expected.sort_by(|x, y| x.data_source_id.cmp(&y.data_source_id));
        assert_eq!(replaced.grants, expected);
        let clients = store.mcp_clients().unwrap();
        assert_eq!(clients[0].grants, expected);
        assert_eq!(clients[1].grants, [grant(&pg, Access::Write)]);

        store.mcp_set_grants(&a.id, &[]).unwrap();
        let clients = store.mcp_clients().unwrap();
        assert!(clients[0].grants.is_empty());
        assert_eq!(clients[1].grants, [grant(&pg, Access::Write)]);
    }

    #[test]
    fn grants_need_an_existing_client_and_data_source() {
        let store = Store::in_memory().unwrap();
        let pg = saved(&store, "pg");
        let client = store.mcp_client_create("a", &hash(1), "idedb_aa").unwrap();

        // Grants on unknown data sources are dropped, the rest kept.
        let set = store.mcp_set_grants(&client.id, &[grant("missing", Access::Write), grant(&pg, Access::Read)]);
        assert_eq!(set.unwrap().unwrap().grants, [grant(&pg, Access::Read)]);

        // An unknown client gets nothing, and nothing else changes.
        assert_eq!(store.mcp_set_grants("missing", &[grant(&pg, Access::Write)]).unwrap(), None);
        assert_eq!(count(&store, "select count(*) from mcp_grant where client_id = ?1", "missing"), 0);
        assert_eq!(store.mcp_clients().unwrap()[0].grants, [grant(&pg, Access::Read)]);

        // After its client is deleted, no grant comes back for it.
        store.mcp_client_delete(&client.id).unwrap();
        assert_eq!(store.mcp_set_grants(&client.id, &[grant(&pg, Access::Read)]).unwrap(), None);
        assert_eq!(count(&store, "select count(*) from mcp_grant where client_id = ?1", &client.id), 0);
    }

    #[test]
    fn never_write_defaults_to_false() {
        let store = Store::in_memory().unwrap();
        let ds = saved(&store, "shop");
        let other = saved(&store, "other");
        assert!(!store.mcp_never_write(&ds).unwrap());
        assert!(store.mcp_set_never_write(&ds, true).unwrap());
        assert!(store.mcp_never_write(&ds).unwrap());
        assert!(!store.mcp_never_write(&other).unwrap());
        assert!(store.mcp_set_never_write(&ds, false).unwrap());
        assert!(!store.mcp_never_write(&ds).unwrap());
    }

    #[test]
    fn never_write_on_an_unknown_data_source_stores_nothing() {
        let store = Store::in_memory().unwrap();
        assert!(!store.mcp_set_never_write("missing", true).unwrap());
        assert!(!store.mcp_never_write("missing").unwrap());
        assert_eq!(count(&store, "select count(*) from mcp_data_source where data_source_id = ?1", "missing"), 0);
    }

    #[test]
    fn deleting_a_data_source_drops_its_policy_but_keeps_audit() {
        let store = Store::in_memory().unwrap();
        let gone = store.save(sqlite_source("gone")).unwrap();
        let kept = store.save(sqlite_source("kept")).unwrap();
        let client = store.mcp_client_create("claude-code", &hash(1), "idedb_aa").unwrap();
        store.mcp_set_grants(&client.id, &[grant(&gone.id, Access::Write), grant(&kept.id, Access::Read)]).unwrap();
        store.mcp_set_never_write(&gone.id, true).unwrap();
        store.mcp_set_never_write(&kept.id, true).unwrap();
        let audited = NewAuditEntry { data_source_name: Some(&gone.name), ..entry(&client.id, &gone.id, "select 1") };
        store.mcp_add_audit(audited).unwrap();

        store.delete(&gone.id).unwrap();

        assert_eq!(store.list().unwrap(), std::slice::from_ref(&kept));
        assert_eq!(store.mcp_clients().unwrap()[0].grants, [grant(&kept.id, Access::Read)]);
        assert_eq!(count(&store, "select count(*) from mcp_data_source where data_source_id = ?1", &gone.id), 0);
        assert!(store.mcp_never_write(&kept.id).unwrap());

        // Its audit rows stay, under the name it had.
        let filter = AuditFilter { data_source_id: Some(gone.id.clone()), ..Default::default() };
        let audit = store.mcp_audit(&filter).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].data_source_name.as_deref(), Some("gone"));

        // A late write for the deleted data source brings nothing back.
        let all = [grant(&gone.id, Access::Write), grant(&kept.id, Access::Read)];
        assert_eq!(store.mcp_set_grants(&client.id, &all).unwrap().unwrap().grants, [grant(&kept.id, Access::Read)]);
        assert!(!store.mcp_set_never_write(&gone.id, true).unwrap());
        assert_eq!(count(&store, "select count(*) from mcp_grant where data_source_id = ?1", &gone.id), 0);
        assert_eq!(count(&store, "select count(*) from mcp_data_source where data_source_id = ?1", &gone.id), 0);
    }

    #[test]
    fn deleting_a_data_source_keeps_its_history() {
        // Unchanged by the MCP cleanup: history outlives its data source.
        let store = Store::in_memory().unwrap();
        let gone = store.save(sqlite_source("gone")).unwrap();
        let entry =
            NewHistoryEntry { data_source_id: &gone.id, sql: "select 1", elapsed_ms: None, row_count: None, error: None };
        store.add_history(entry).unwrap();
        store.delete(&gone.id).unwrap();
        assert_eq!(store.history(Some(&gone.id), None, 10).unwrap().len(), 1);
    }

    #[test]
    fn records_audit_entries_as_given() {
        let store = Store::in_memory().unwrap();
        let written = store
            .mcp_add_audit(NewAuditEntry {
                transport: Transport::Bridge,
                session_key: Some("bridge-1"),
                tool: "execute",
                statement_kind: Some("update"),
                reason: Some("fix a typo"),
                decision: Decision::Approved,
                approval_wait_ms: Some(1500),
                truncated: true,
                error: Some("boom"),
                ..entry("c", "ds", "update t set x = 1")
            })
            .unwrap();
        assert!(written.at.ends_with('Z') && written.at.contains('T'), "{}", written.at);
        assert_eq!(
            written,
            AuditEntry {
                id: written.id,
                at: written.at.clone(),
                client_id: Some("c".into()),
                client_name: "claude-code".into(),
                client_info_name: Some("claude-code".into()),
                client_info_version: Some("2.1.0".into()),
                protocol_version: Some("2026-07-28".into()),
                transport: Transport::Bridge,
                session_key: Some("bridge-1".into()),
                tool: "execute".into(),
                data_source_id: Some("ds".into()),
                data_source_name: Some("shop".into()),
                sql: Some("update t set x = 1".into()),
                sql_truncated: false,
                statement_kind: Some("update".into()),
                reason: Some("fix a typo".into()),
                decision: Decision::Approved,
                approval_wait_ms: Some(1500),
                elapsed_ms: Some(4),
                row_count: Some(2),
                truncated: true,
                error: Some("boom".into()),
            }
        );
        assert_eq!(store.mcp_audit(&AuditFilter::default()).unwrap(), std::slice::from_ref(&written));

        let json = serde_json::to_value(&written).unwrap();
        assert_eq!(json["decision"], "approved");
        assert_eq!(json["transport"], "bridge");
        assert_eq!(json["approvalWaitMs"], 1500);
        assert_eq!(json["sqlTruncated"], false);
    }

    #[test]
    fn audit_keeps_the_newest_rows() {
        let store = Store::in_memory().unwrap();
        for i in 0..5 {
            store.mcp_add_audit_retaining(entry("a", "ds", &format!("select {i}")), 3).unwrap();
        }
        store.mcp_add_audit_retaining(entry("b", "other", "select b"), 3).unwrap();

        // Retention is global, not per client or data source.
        assert_eq!(sqls(&store.mcp_audit(&AuditFilter::default()).unwrap()), ["select b", "select 4", "select 3"]);

        // Exactly `keep` rows stay however many go in, down to one.
        for i in 0..40 {
            store.mcp_add_audit_retaining(entry("a", "ds", &format!("select {i}")), 7).unwrap();
        }
        let kept = store.mcp_audit(&AuditFilter::default()).unwrap();
        assert_eq!(sqls(&kept), (33..40).rev().map(|i| format!("select {i}")).collect::<Vec<_>>());
        store.mcp_add_audit_retaining(entry("a", "ds", "select last"), 1).unwrap();
        assert_eq!(sqls(&store.mcp_audit(&AuditFilter::default()).unwrap()), ["select last"]);
    }

    #[test]
    fn filters_and_pages_audit_newest_first() {
        let store = Store::in_memory().unwrap();
        store.mcp_add_audit(entry("a", "pg", "SELECT * FROM Orders")).unwrap();
        store.mcp_add_audit(entry("b", "pg", "select * from customers")).unwrap();
        store.mcp_add_audit(NewAuditEntry { decision: Decision::Denied, ..entry("a", "my", "delete from orders") }).unwrap();
        store.mcp_add_audit(NewAuditEntry { sql: None, tool: "list_tables", ..entry("a", "pg", "") }).unwrap();

        let all = store.mcp_audit(&AuditFilter::default()).unwrap();
        assert_eq!(sqls(&all), ["", "delete from orders", "select * from customers", "SELECT * FROM Orders"]);
        assert!(all.windows(2).all(|w| w[0].id > w[1].id));

        let find = |filter: AuditFilter| -> Vec<String> {
            store.mcp_audit(&filter).unwrap().into_iter().map(|e| e.sql.unwrap_or_default()).collect()
        };
        assert_eq!(
            find(AuditFilter { client_id: Some("a".into()), ..Default::default() }),
            ["", "delete from orders", "SELECT * FROM Orders"]
        );
        assert_eq!(
            find(AuditFilter { data_source_id: Some("pg".into()), ..Default::default() }),
            ["", "select * from customers", "SELECT * FROM Orders"]
        );
        assert_eq!(find(AuditFilter { decision: Some(Decision::Denied), ..Default::default() }), ["delete from orders"]);
        assert_eq!(
            find(AuditFilter { search: Some("ORDERS".into()), ..Default::default() }),
            ["delete from orders", "SELECT * FROM Orders"]
        );
        assert_eq!(find(AuditFilter { search: Some(String::new()), ..Default::default() }).len(), 4);
        assert_eq!(
            find(AuditFilter {
                client_id: Some("a".into()),
                data_source_id: Some("pg".into()),
                search: Some("orders".into()),
                ..Default::default()
            }),
            ["SELECT * FROM Orders"]
        );
        assert!(find(AuditFilter { client_id: Some("missing".into()), ..Default::default() }).is_empty());

        // Pages of two, each one below the last id of the previous page.
        let first = store.mcp_audit(&AuditFilter { limit: 2, ..Default::default() }).unwrap();
        assert_eq!(sqls(&first), ["", "delete from orders"]);
        let second =
            store.mcp_audit(&AuditFilter { limit: 2, before_id: Some(first[1].id), ..Default::default() }).unwrap();
        assert_eq!(sqls(&second), ["select * from customers", "SELECT * FROM Orders"]);
        let third = store.mcp_audit(&AuditFilter { before_id: Some(second[1].id), ..Default::default() }).unwrap();
        assert!(third.is_empty());
    }

    #[test]
    fn deserializes_a_partial_filter() {
        let filter: AuditFilter = serde_json::from_str(r#"{"clientId":"a","decision":"timeout","beforeId":7}"#).unwrap();
        let expected = AuditFilter {
            client_id: Some("a".into()),
            decision: Some(Decision::Timeout),
            before_id: Some(7),
            ..Default::default()
        };
        assert_eq!(filter, expected);
    }

    #[test]
    fn cuts_long_sql_on_a_char_boundary_and_flags_it() {
        let store = Store::in_memory().unwrap();
        // 9 bytes, then 2-byte chars: the limit (even) falls inside one.
        let long = format!("select 'x{}'", "é".repeat(MAX_SQL));
        assert!(!long.is_char_boundary(MAX_SQL));
        let stored = store.mcp_add_audit(entry("a", "ds", &long)).unwrap();
        let sql = stored.sql.unwrap();
        assert_eq!(sql.len(), MAX_SQL - 1);
        assert!(long.starts_with(&sql));
        assert!(stored.sql_truncated);

        let short = "select 'é'";
        let stored = store.mcp_add_audit(entry("a", "ds", short)).unwrap();
        assert_eq!((stored.sql.as_deref(), stored.sql_truncated), (Some(short), false));
        let exact = "x".repeat(MAX_SQL);
        let stored = store.mcp_add_audit(entry("a", "ds", &exact)).unwrap();
        assert_eq!((stored.sql, stored.sql_truncated), (Some(exact), false));
        let stored = store.mcp_add_audit(NewAuditEntry { sql: None, ..entry("a", "ds", "") }).unwrap();
        assert_eq!((stored.sql, stored.sql_truncated), (None, false));
    }

    #[test]
    fn caps_every_other_text_field() {
        let store = Store::in_memory().unwrap();
        // One byte, then 2-byte chars, so each (even) limit falls inside one.
        let long = |max: usize| format!("x{}", "é".repeat(max));
        let (short, reason, error) = (long(MAX_SHORT), long(MAX_REASON), long(MAX_ERROR));
        let stored = store
            .mcp_add_audit(NewAuditEntry {
                client_id: Some(&short),
                client_name: &short,
                client_info_name: Some(&short),
                client_info_version: Some(&short),
                protocol_version: Some(&short),
                session_key: Some(&short),
                tool: &short,
                data_source_id: Some(&short),
                data_source_name: Some(&short),
                statement_kind: Some(&short),
                reason: Some(&reason),
                error: Some(&error),
                ..entry("a", "ds", "select 1")
            })
            .unwrap();

        let cut_short = &short[..MAX_SHORT - 1];
        for field in [
            stored.client_id.as_deref(),
            Some(stored.client_name.as_str()),
            stored.client_info_name.as_deref(),
            stored.client_info_version.as_deref(),
            stored.protocol_version.as_deref(),
            stored.session_key.as_deref(),
            Some(stored.tool.as_str()),
            stored.data_source_id.as_deref(),
            stored.data_source_name.as_deref(),
            stored.statement_kind.as_deref(),
        ] {
            assert_eq!(field, Some(cut_short));
        }
        assert_eq!(stored.reason.as_deref(), Some(&reason[..MAX_REASON - 1]));
        assert_eq!(stored.error.as_deref(), Some(&error[..MAX_ERROR - 1]));
        assert!(!stored.sql_truncated);
    }
}
