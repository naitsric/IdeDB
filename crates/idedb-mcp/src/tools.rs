//! The tools, as async methods of [`McpServer`]: their arguments and
//! results (with JSON schemas for the transport to publish), and what each
//! call goes through: the client's grants, the statement's classification,
//! the user's approval for writes, and the audit row every call leaves.
//!
//! Each call runs on a task of its own. Dropping a call's future cancels
//! it, but the task still finishes: a read's transaction is always rolled
//! back, a pending approval is withdrawn, and the call is audited.

use std::future::Future;
use std::sync::Arc;

use chrono::{SecondsFormat, TimeDelta, Utc};
use idedb_core::{Engine, ObjectKind, SchemaInfo, TableInfo};
use idedb_drivers::OpenError;
use idedb_sql::{Forbidden, Kind, WriteKind};
use idedb_store::{Access, DataSource, Grant, NewAuditEntry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::approvals::{ApprovalRequest, Progress};
use crate::pool::{Checkout, OpenFailure};
use crate::run::{self, Failure, Limits, Ran};
use crate::settings::MAX_ROWS;
use crate::{CallError, Caller, Core, Decision, Error, McpEvent, McpServer, ToolError};

// Arguments and results. Doc comments become the schemas' descriptions,
// which the model reads.

/// Arguments of `list_connections`: none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListConnectionsArgs {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListConnectionsOutput {
    pub connections: Vec<ConnectionInfo>,
}

/// A connection this client may use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    /// Pass it as `connection` to the other tools.
    pub id: String,
    pub name: String,
    pub engine: EngineName,
    /// The database it connects to; the file name for SQLite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// What the user allows this client here.
    pub access: AccessLevel,
    /// Whether `execute` may change data here, with the user's approval each
    /// time.
    pub writes_allowed: bool,
    /// False when the connection cannot be used over MCP; `note` says why.
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EngineName {
    Postgres,
    Mysql,
    Sqlite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccessLevel {
    /// `query` and the listing tools only.
    Read,
    /// Also `execute`, with the user's approval for each statement.
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListSchemasArgs {
    /// Connection id, or its name when no other connection has it.
    pub connection: String,
    /// Also list engine catalogs such as pg_catalog or information_schema.
    #[serde(default)]
    pub include_system: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListSchemasOutput {
    /// Where unqualified names resolve when a call passes no `schema`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_schema: Option<String>,
    pub schemas: Vec<SchemaEntry>,
}

/// A schema (Postgres), database (MySQL) or attached database (SQLite).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SchemaEntry {
    pub name: String,
    pub is_system: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListTablesArgs {
    /// Connection id, or its name when no other connection has it.
    pub connection: String,
    /// Defaults to the connection's default schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListTablesOutput {
    pub schema: String,
    pub tables: Vec<TableEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TableEntry {
    pub name: String,
    pub kind: TableKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
    ForeignTable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DescribeTableArgs {
    /// Connection id, or its name when no other connection has it.
    pub connection: String,
    /// Table or view name, without the schema.
    pub table: String,
    /// Defaults to the connection's default schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DescribeTableOutput {
    pub schema: String,
    pub name: String,
    pub kind: TableKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// In ordinal order.
    pub columns: Vec<ColumnDescription>,
    /// Foreign keys of this table.
    pub foreign_keys: Vec<ForeignKeyDescription>,
    /// Foreign keys of other tables in the same schema that point at this one.
    pub referenced_by: Vec<Reference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ColumnDescription {
    pub name: String,
    /// Declared type, e.g. `varchar(255)`.
    #[serde(rename = "type")]
    pub type_name: String,
    pub nullable: bool,
    /// Default expression, as SQL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// 1-based position in the primary key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_key: Option<u16>,
    /// The database fills it itself (identity, auto-increment, computed).
    pub generated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKeyDescription {
    pub name: String,
    /// Paired by position with `referencedColumns`.
    pub columns: Vec<String>,
    pub referenced_schema: String,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
}

/// A foreign key of another table pointing at the described one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    /// The foreign key's name.
    pub name: String,
    /// The table that has the foreign key, in the same schema.
    pub table: String,
    /// Its columns, paired by position with `referencedColumns`.
    pub columns: Vec<String>,
    /// Columns of the described table.
    pub referenced_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryArgs {
    /// Connection id, or its name when no other connection has it.
    pub connection: String,
    /// One read-only statement: SELECT, WITH, SHOW, EXPLAIN… No `;`-separated scripts.
    pub sql: String,
    /// Where unqualified names resolve. Defaults to the connection's default schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Most rows to return; 200 unless the user changed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 1000))]
    pub max_rows: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryOutput {
    pub columns: Vec<ResultColumn>,
    /// Each row's values in column order.
    pub rows: Vec<Vec<Json>>,
    /// Rows returned; rows affected for a statement without a result set.
    pub row_count: u64,
    /// More rows exist than were returned: narrow the query or aggregate.
    pub truncated: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResultColumn {
    pub name: String,
    /// The engine's type name, e.g. `int8`.
    #[serde(rename = "type")]
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteArgs {
    /// Connection id, or its name when no other connection has it.
    pub connection: String,
    /// One statement (INSERT, UPDATE, DELETE, CREATE…). The user must approve it in IdeDB.
    pub sql: String,
    /// Where unqualified names resolve. Defaults to the connection's default schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Why the statement should run, shown to the user who approves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteOutput {
    /// Rows affected, or returned when the statement returns rows (RETURNING).
    pub row_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<ResultColumn>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<Json>>>,
    pub elapsed_ms: u64,
    /// More rows were returned than fit in the result.
    pub truncated: bool,
}

// Messages for the model.

const NO_PASSWORD: &str = "This connection's password isn't saved in IdeDB, so it can't be used over MCP. The user can \
                           save it in the connection's settings in IdeDB.";
const FILE_URI: &str = "This SQLite connection's path is a file: URI, which IdeDB can't open read only for MCP. The \
                        user can change it to a plain file path in IdeDB.";

fn not_available(connection: &str) -> String {
    format!(
        "No connection '{connection}' is available to this client. Call list_connections to see the ones it may use; \
         the user grants access in IdeDB."
    )
}

fn forbidden(kind: Forbidden, summary: &str) -> String {
    let why = match kind {
        Forbidden::Empty => return "The SQL is empty: pass one statement.".into(),
        Forbidden::MultipleStatements => {
            return "Only one statement per call: send each statement in its own call, without `;` between them."
                .into();
        }
        Forbidden::TransactionControl => {
            "each call runs on its own, so transaction control (BEGIN, COMMIT, ROLLBACK, SAVEPOINT, LOCK…) isn't allowed"
        }
        Forbidden::SessionState => {
            "session settings (SET, RESET, USE…) aren't allowed; pass `schema` to choose where unqualified names resolve"
        }
        Forbidden::FileAccess => "statements that read or write files (COPY, LOAD DATA, ATTACH, VACUUM, INTO OUTFILE…) aren't allowed",
        Forbidden::Cursor => "cursors aren't allowed; page with LIMIT and OFFSET instead",
        Forbidden::PreparedStatement => "prepared statements aren't allowed; send the statement itself",
        Forbidden::Other => "this kind of statement isn't allowed over MCP",
    };
    format!("IdeDB refuses this statement ({summary}): {why}.")
}

impl McpServer {
    /// The connections this client may use, in the user's order.
    pub async fn list_connections(&self, caller: &Caller) -> Result<ListConnectionsOutput, CallError> {
        self.detached(caller, &CancellationToken::new(), |core, caller, _| async move { core.list_connections(&caller) })
            .await
    }

    pub async fn list_schemas(&self, caller: &Caller, args: ListSchemasArgs) -> Result<ListSchemasOutput, CallError> {
        self.detached(caller, &CancellationToken::new(), |core, caller, _| async move {
            core.list_schemas(&caller, args).await
        })
        .await
    }

    pub async fn list_tables(&self, caller: &Caller, args: ListTablesArgs) -> Result<ListTablesOutput, CallError> {
        self.detached(caller, &CancellationToken::new(), |core, caller, _| async move {
            core.list_tables(&caller, args).await
        })
        .await
    }

    pub async fn describe_table(
        &self,
        caller: &Caller,
        args: DescribeTableArgs,
    ) -> Result<DescribeTableOutput, CallError> {
        self.detached(caller, &CancellationToken::new(), |core, caller, _| async move {
            core.describe_table(&caller, args).await
        })
        .await
    }

    /// Runs a read. `cancel` stops it (as does dropping the future).
    pub async fn query(
        &self,
        caller: &Caller,
        args: QueryArgs,
        cancel: &CancellationToken,
    ) -> Result<QueryOutput, CallError> {
        self.detached(caller, cancel, |core, caller, cancel| async move { core.query(&caller, args, &cancel).await })
            .await
    }

    /// Runs a statement: a read right away, a write once the user approves
    /// it. `progress` is called every 15 s while it waits for the user;
    /// `cancel` (or dropping the future) withdraws it, or stops it once
    /// running.
    pub async fn execute(
        &self,
        caller: &Caller,
        args: ExecuteArgs,
        progress: impl Fn(Progress) + Send + Sync + 'static,
        cancel: &CancellationToken,
    ) -> Result<ExecuteOutput, CallError> {
        self.detached(caller, cancel, |core, caller, cancel| async move {
            core.execute(&caller, args, &progress, &cancel).await
        })
        .await
    }

    /// Runs a call on a task of its own, which a dropped caller cancels
    /// rather than aborts, so it always cleans up and audits.
    async fn detached<T, F>(
        &self,
        caller: &Caller,
        cancel: &CancellationToken,
        call: impl FnOnce(Arc<Core>, Caller, CancellationToken) -> F,
    ) -> Result<T, CallError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, CallError>> + Send + 'static,
    {
        let cancel = cancel.child_token();
        let on_drop = cancel.clone().drop_guard();
        let task = tokio::spawn(call(self.0.clone(), caller.clone(), cancel));
        let joined = task.await;
        on_drop.disarm();
        joined.map_err(|e| Error::Internal(format!("the tool call failed: {e}")))?
    }
}

/// The data source a call is about, and what the client may do on it.
struct Target {
    source: DataSource,
    access: Access,
}

/// Why a call cannot use the connection it named, and the data source it
/// named if it exists, for the audit row.
struct Refusal {
    message: String,
    source: Option<DataSource>,
}

impl Core {
    fn list_connections(&self, caller: &Caller) -> Result<ListConnectionsOutput, CallError> {
        self.seen(&caller.client_id, caller.client_info.as_ref());
        let call = Call::new(self, caller, "list_connections");
        let grants = self.grants(caller)?;
        let connections: Vec<ConnectionInfo> = self
            .host
            .store()
            .list()?
            .into_iter()
            .filter_map(|source| {
                let access = grants.iter().find(|g| g.data_source_id == source.id)?.access;
                Some((source, access))
            })
            .map(|(source, access)| {
                let never_write = self.host.store().mcp_never_write(&source.id)?;
                let note = unavailable(&source);
                Ok(ConnectionInfo {
                    database: database_name(&source),
                    engine: match source.params.engine {
                        Engine::Postgres => EngineName::Postgres,
                        Engine::Mysql => EngineName::Mysql,
                        Engine::Sqlite => EngineName::Sqlite,
                    },
                    access: match access {
                        Access::Read => AccessLevel::Read,
                        Access::Write => AccessLevel::Write,
                    },
                    writes_allowed: access == Access::Write && !never_write,
                    available: note.is_none(),
                    note: note.map(str::to_owned),
                    id: source.id,
                    name: source.name,
                })
            })
            .collect::<Result<_, idedb_store::Error>>()?;
        let outcome = Outcome {
            elapsed_ms: Some(call.started.elapsed().as_millis() as u64),
            row_count: Some(connections.len() as u64),
            ..Outcome::default()
        };
        call.record(Decision::Allowed, outcome)?;
        Ok(ListConnectionsOutput { connections })
    }

    async fn list_schemas(self: Arc<Self>, caller: &Caller, args: ListSchemasArgs) -> Result<ListSchemasOutput, CallError> {
        self.seen(&caller.client_id, caller.client_info.as_ref());
        let timeout = self.settings()?.statement_timeout();
        let mut call = Call::new(&self, caller, "list_schemas");
        let target = self.target_or_deny(&mut call, &args.connection)?;
        let checkout = self.checkout(&call, &target.source).await?;
        let listed = {
            let mut pooled = checkout.opened().session.lock().await;
            tokio::time::timeout(timeout, pooled.session.schemas()).await
        };
        let schemas: Vec<SchemaInfo> = self.introspected(&call, &checkout, listed, "list the schemas")?;
        let schemas: Vec<SchemaEntry> = schemas
            .into_iter()
            .filter(|s| args.include_system || !s.is_system)
            .map(|s| SchemaEntry { name: s.name, is_system: s.is_system })
            .collect();
        call.record(Decision::Allowed, call.listed(schemas.len()))?;
        Ok(ListSchemasOutput { default_schema: checkout.opened().default_schema.clone(), schemas })
    }

    async fn list_tables(self: Arc<Self>, caller: &Caller, args: ListTablesArgs) -> Result<ListTablesOutput, CallError> {
        self.seen(&caller.client_id, caller.client_info.as_ref());
        let timeout = self.settings()?.statement_timeout();
        let mut call = Call::new(&self, caller, "list_tables");
        let target = self.target_or_deny(&mut call, &args.connection)?;
        let checkout = self.checkout(&call, &target.source).await?;
        let schema = self.schema_or_deny(&call, &checkout, args.schema)?;
        let model = {
            let mut pooled = checkout.opened().session.lock().await;
            tokio::time::timeout(timeout, pooled.session.introspect(&schema)).await
        };
        let model = self.introspected(&call, &checkout, model, "list the tables")?;
        let tables: Vec<TableEntry> = model
            .tables
            .into_iter()
            .map(|t| TableEntry { name: t.name, kind: table_kind(t.kind), comment: t.comment })
            .collect();
        call.record(Decision::Allowed, call.listed(tables.len()))?;
        Ok(ListTablesOutput { schema, tables })
    }

    async fn describe_table(
        self: Arc<Self>,
        caller: &Caller,
        args: DescribeTableArgs,
    ) -> Result<DescribeTableOutput, CallError> {
        self.seen(&caller.client_id, caller.client_info.as_ref());
        let timeout = self.settings()?.statement_timeout();
        let mut call = Call::new(&self, caller, "describe_table");
        let target = self.target_or_deny(&mut call, &args.connection)?;
        let checkout = self.checkout(&call, &target.source).await?;
        let schema = self.schema_or_deny(&call, &checkout, args.schema)?;
        let model = {
            let mut pooled = checkout.opened().session.lock().await;
            tokio::time::timeout(timeout, pooled.session.introspect(&schema)).await
        };
        let model = self.introspected(&call, &checkout, model, "describe the table")?;

        let found = match named(model.tables.iter(), &args.table, |t| &t.name).as_slice() {
            [table] => (*table).clone(),
            [] => {
                return call.deny(format!(
                    "There is no table or view '{}' in schema '{schema}'. Call list_tables to see them.",
                    args.table
                ));
            }
            several => {
                let names: Vec<&str> = several.iter().map(|t| t.name.as_str()).collect();
                return call.deny(format!("Several tables match '{}': {}. Pass the exact name.", args.table, names.join(", ")));
            }
        };
        let referenced_by = model
            .tables
            .iter()
            .filter(|other| other.name != found.name)
            .flat_map(|other| {
                other
                    .foreign_keys
                    .iter()
                    .filter(|fk| fk.referenced_schema == schema && fk.referenced_table == found.name)
                    .map(|fk| Reference {
                        name: fk.name.clone(),
                        table: other.name.clone(),
                        columns: fk.columns.clone(),
                        referenced_columns: fk.referenced_columns.clone(),
                    })
            })
            .collect();
        call.record(Decision::Allowed, call.listed(found.columns.len()))?;
        Ok(describe(schema, found, referenced_by))
    }

    async fn query(
        self: Arc<Self>,
        caller: &Caller,
        args: QueryArgs,
        cancel: &CancellationToken,
    ) -> Result<QueryOutput, CallError> {
        self.seen(&caller.client_id, caller.client_info.as_ref());
        let settings = self.settings()?;
        let mut call = Call::new(&self, caller, "query");
        call.sql = Some(&args.sql);
        let target = self.target_or_deny(&mut call, &args.connection)?;
        let classification = idedb_sql::classify(target.source.params.engine, &args.sql);
        call.statement_kind = Some(classification.summary.clone());
        match classification.kind {
            Kind::Read => {}
            // The read-only session is what stops it if it writes after all.
            Kind::Write(WriteKind::Unparsed) if classification.looks_like_read => {
                call.statement_kind = Some("unparsed read".into());
            }
            Kind::Forbidden(kind) => return call.deny(forbidden(kind, &classification.summary)),
            Kind::Write(_) => {
                return call.deny(format!(
                    "This statement writes ({}); use the execute tool, the user must approve it.",
                    classification.summary
                ));
            }
        }
        let max_rows = args.max_rows.unwrap_or(settings.max_rows).clamp(1, MAX_ROWS) as usize;
        let limits = Limits { max_rows, timeout: settings.statement_timeout() };
        let ran = self.read(&call, &target.source, &args.sql, args.schema.as_deref(), limits, cancel).await?;
        call.record(Decision::Allowed, Outcome::ran(&ran))?;
        Ok(QueryOutput {
            columns: result_columns(ran.columns.unwrap_or_default()),
            rows: ran.rows,
            row_count: ran.row_count,
            truncated: ran.truncated,
            elapsed_ms: ran.elapsed_ms,
        })
    }

    async fn execute(
        self: Arc<Self>,
        caller: &Caller,
        args: ExecuteArgs,
        progress: &(dyn Fn(Progress) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<ExecuteOutput, CallError> {
        self.seen(&caller.client_id, caller.client_info.as_ref());
        let settings = self.settings()?;
        let mut call = Call::new(&self, caller, "execute");
        call.sql = Some(&args.sql);
        call.reason = args.reason.as_deref();
        let target = self.target_or_deny(&mut call, &args.connection)?;
        let source = target.source;
        let classification = idedb_sql::classify(source.params.engine, &args.sql);
        call.statement_kind = Some(classification.summary.clone());
        let limits = Limits { max_rows: settings.max_rows as usize, timeout: settings.statement_timeout() };
        let write_kind = match classification.kind {
            // A read needs no approval, whichever tool it comes through.
            Kind::Read => {
                let ran = self.read(&call, &source, &args.sql, args.schema.as_deref(), limits, cancel).await?;
                call.record(Decision::Allowed, Outcome::ran(&ran))?;
                return Ok(execute_output(ran));
            }
            Kind::Forbidden(kind) => return call.deny(forbidden(kind, &classification.summary)),
            Kind::Write(kind) => kind,
        };
        if target.access != Access::Write {
            return call.deny(format!(
                "This client may only read '{}': writing needs write access, which the user grants in IdeDB. Use the \
                 query tool for reads.",
                source.name
            ));
        }
        if self.host.store().mcp_never_write(&source.id)? {
            return call.deny(format!(
                "'{}' is marked never-write in IdeDB: no statement that writes runs on it over MCP.",
                source.name
            ));
        }

        let now = Utc::now();
        let request = ApprovalRequest {
            id: self.approvals.next_id(),
            client_id: caller.client_id.clone(),
            client_name: caller.client_name.clone(),
            client_info: caller.client_info.clone(),
            data_source_id: source.id.clone(),
            data_source_name: source.name.clone(),
            data_source_color: source.color.clone(),
            sql: args.sql.clone(),
            summary: classification.summary.clone(),
            write_kind,
            warnings: classification.warnings.clone(),
            reason: args.reason.clone(),
            requested_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
            expires_at: (now + TimeDelta::seconds(settings.approval_timeout_secs as i64))
                .to_rfc3339_opts(SecondsFormat::Millis, true),
        };
        let waiting = Instant::now();
        let decision = self.approvals.wait(&*self.host, request, settings.approval_timeout(), cancel, progress).await;
        let waited = Outcome { approval_wait_ms: Some(waiting.elapsed().as_millis() as u64), ..Outcome::default() };
        let refused = match decision {
            Decision::Approved => None,
            Decision::Rejected => Some(
                "The user rejected this statement in IdeDB, so it did not run. Don't retry it unchanged; ask the user \
                 what they want instead."
                    .to_owned(),
            ),
            Decision::Timeout => Some(format!(
                "Nobody approved the statement in IdeDB within {}s, so it did not run. Ask the user to watch for \
                 IdeDB's approval dialog, then try again.",
                settings.approval_timeout_secs
            )),
            _ => Some("The call was cancelled before the user answered in IdeDB; the statement did not run.".to_owned()),
        };
        if let Some(message) = refused {
            return call.refuse(decision, message, waited);
        }

        // The user may have changed what this client may do while deciding.
        let still_allowed = match self.target(caller, &source.id)? {
            Ok(now) => now.access == Access::Write && !self.host.store().mcp_never_write(&source.id)?,
            Err(_) => false,
        };
        if !still_allowed {
            let message = format!(
                "While the statement waited for approval, the user changed this client's access to '{}' in IdeDB, so \
                 it did not run.",
                source.name
            );
            return call.refuse(Decision::Denied, message, waited);
        }

        match run::write(&self.host, &source.id, &args.sql, args.schema.as_deref(), limits, cancel).await {
            Ok(ran) => {
                let outcome = Outcome { approval_wait_ms: waited.approval_wait_ms, ..Outcome::ran(&ran) };
                if let Err(e) = call.record(Decision::Approved, outcome) {
                    return Err(Error::Internal(format!(
                        "The statement ran, but IdeDB could not record it in its audit log: {e}"
                    ))
                    .into());
                }
                Ok(execute_output(ran))
            }
            Err(failure) => Err(self.failed(&call, failure, Some(Decision::Approved), waited)),
        }
    }

    /// Runs a read on the client's pooled session.
    async fn read(
        &self,
        call: &Call<'_>,
        source: &DataSource,
        sql: &str,
        schema: Option<&str>,
        limits: Limits,
        cancel: &CancellationToken,
    ) -> Result<Ran, CallError> {
        let checkout = self.checkout(call, source).await?;
        run::read(&self.host, &checkout, sql, schema, limits, cancel)
            .await
            .map_err(|failure| self.failed(call, failure, None, Outcome::default()))
    }

    /// The client's pooled session on `source`; a failure to open it is
    /// audited.
    async fn checkout(&self, call: &Call<'_>, source: &DataSource) -> Result<Checkout, CallError> {
        self.pool
            .checkout(&self.host, &call.caller.client_id, source)
            .await
            .map_err(|failure| self.failed(call, Failure::Open(failure), None, Outcome::default()))
    }

    /// Audits a call that failed and says why. `decision` is what the user
    /// decided, when they did; otherwise an open failure that is IdeDB's
    /// policy counts as denied, anything else as allowed.
    fn failed(&self, call: &Call<'_>, failure: Failure, decision: Option<Decision>, outcome: Outcome) -> CallError {
        let (policy, message, elapsed_ms) = match failure {
            Failure::Statement { message, elapsed_ms } => (Decision::Allowed, message, elapsed_ms),
            Failure::Open(OpenFailure::Open(OpenError::PasswordRequired(_))) => {
                (Decision::Denied, NO_PASSWORD.to_owned(), None)
            }
            Failure::Open(OpenFailure::Open(OpenError::NotFound)) => (
                Decision::Denied,
                "This connection no longer exists in IdeDB. Call list_connections.".to_owned(),
                None,
            ),
            Failure::Open(OpenFailure::Open(OpenError::Driver(e))) => {
                let name = call.data_source.as_ref().map_or("the connection", |s| s.name.as_str());
                (Decision::Allowed, format!("IdeDB could not connect to {name}: {e}"), None)
            }
            Failure::Open(OpenFailure::Open(OpenError::Store(e))) => {
                let error = Error::from(e);
                let _ = call.record(decision.unwrap_or(Decision::Allowed), Outcome { error: Some(error.to_string()), ..outcome });
                return error.into();
            }
            Failure::Open(OpenFailure::Worker(e)) => {
                let error = Error::Internal(format!("opening the session failed: {e}"));
                let _ = call.record(decision.unwrap_or(Decision::Allowed), Outcome { error: Some(error.to_string()), ..outcome });
                return error.into();
            }
        };
        let outcome = Outcome { error: Some(message.clone()), elapsed_ms, ..outcome };
        match call.record(decision.unwrap_or(policy), outcome) {
            Ok(()) => ToolError::new(message).into(),
            Err(e) => e.into(),
        }
    }

    /// Audits an introspection's failure or timeout; a timed out session is
    /// dropped, its state unknown.
    fn introspected<T>(
        &self,
        call: &Call<'_>,
        checkout: &Checkout,
        result: Result<idedb_core::Result<T>, tokio::time::error::Elapsed>,
        what: &str,
    ) -> Result<T, CallError> {
        let message = match result {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(e)) => format!("IdeDB could not {what}: {e}"),
            Err(_) => {
                checkout.evict();
                format!("IdeDB could not {what} in time; the database may be busy. Try again later.")
            }
        };
        let elapsed_ms = Some(call.started.elapsed().as_millis() as u64);
        call.refuse(Decision::Allowed, message, Outcome { elapsed_ms, ..Outcome::default() })
    }

    /// The schema given, else the session's default.
    fn schema_or_deny(&self, call: &Call<'_>, checkout: &Checkout, schema: Option<String>) -> Result<String, CallError> {
        match schema.or_else(|| checkout.opened().default_schema.clone()) {
            Some(schema) => Ok(schema),
            None => call.deny(
                "This connection has no default schema: pass `schema` (list_schemas shows them).".to_owned(),
            ),
        }
    }

    /// The client's grants; none once it is revoked.
    fn grants(&self, caller: &Caller) -> Result<Vec<Grant>, Error> {
        let client = self.host.store().mcp_client(&caller.client_id)?;
        Ok(client.filter(|c| c.revoked_at.is_none()).map(|c| c.grants).unwrap_or_default())
    }

    /// The data source `connection` names (an id, or a name unique among
    /// the client's grants) if the client may use it.
    fn target(&self, caller: &Caller, connection: &str) -> Result<Result<Target, Refusal>, Error> {
        let grants = self.grants(caller)?;
        let sources = self.host.store().list()?;
        let access = |source: &DataSource| grants.iter().find(|g| g.data_source_id == source.id).map(|g| g.access);
        let refused = |source: Option<&DataSource>| Refusal { message: not_available(connection), source: source.cloned() };

        let source = match sources.iter().find(|s| s.id == connection) {
            Some(source) => source,
            None => match named(sources.iter().filter(|s| access(s).is_some()), connection, |s| &s.name).as_slice() {
                [source] => *source,
                [] => {
                    // Named after a data source the client may not use: the
                    // audit row still says which, the client learns nothing.
                    let others = named(sources.iter().filter(|s| access(s).is_none()), connection, |s| &s.name);
                    return Ok(Err(refused(if others.len() == 1 { Some(others[0]) } else { None })));
                }
                several => {
                    let ids: Vec<&str> = several.iter().map(|s| s.id.as_str()).collect();
                    let message = format!(
                        "{} connections this client may use are named '{connection}'. Pass one of their ids instead: \
                         {}.",
                        several.len(),
                        ids.join(", ")
                    );
                    return Ok(Err(Refusal { message, source: None }));
                }
            },
        };
        let Some(access) = access(source) else { return Ok(Err(refused(Some(source)))) };
        if let Some(note) = unavailable(source) {
            return Ok(Err(Refusal { message: note.to_owned(), source: Some(source.clone()) }));
        }
        Ok(Ok(Target { source: source.clone(), access }))
    }

    /// [`target`](Self::target), auditing a refusal; notes the data source
    /// in `call` either way.
    fn target_or_deny(&self, call: &mut Call<'_>, connection: &str) -> Result<Target, CallError> {
        match self.target(call.caller, connection)? {
            Ok(target) => {
                call.data_source = Some(target.source.clone());
                Ok(target)
            }
            Err(refusal) => {
                call.data_source = refusal.source;
                call.deny(refusal.message)
            }
        }
    }
}

/// Items named `name`: exactly, or else ignoring case.
fn named<'a, T>(items: impl Iterator<Item = &'a T> + Clone, name: &str, name_of: impl Fn(&T) -> &String) -> Vec<&'a T> {
    let exact: Vec<&T> = items.clone().filter(|item| name_of(item) == name).collect();
    if !exact.is_empty() {
        return exact;
    }
    let lower = name.to_lowercase();
    items.filter(|item| name_of(item).to_lowercase() == lower).collect()
}

/// Why a data source cannot be used over MCP, if it cannot.
fn unavailable(source: &DataSource) -> Option<&'static str> {
    match source.params.engine {
        // Read-only sessions refuse URIs: see `SqliteSession::connect_with`.
        Engine::Sqlite if source.params.path.trim().starts_with("file:") => Some(FILE_URI),
        Engine::Sqlite => None,
        // v1 never asks for a password.
        Engine::Postgres | Engine::Mysql if !source.save_password => Some(NO_PASSWORD),
        Engine::Postgres | Engine::Mysql => None,
    }
}

fn database_name(source: &DataSource) -> Option<String> {
    let params = &source.params;
    let name = match params.engine {
        Engine::Sqlite => {
            std::path::Path::new(params.path.trim()).file_name().map(|n| n.to_string_lossy().into_owned())
        }
        Engine::Postgres | Engine::Mysql => Some(params.database.clone()),
    };
    name.filter(|name| !name.is_empty())
}

fn table_kind(kind: ObjectKind) -> TableKind {
    match kind {
        ObjectKind::Table => TableKind::Table,
        ObjectKind::View => TableKind::View,
        ObjectKind::MaterializedView => TableKind::MaterializedView,
        ObjectKind::ForeignTable => TableKind::ForeignTable,
    }
}

fn describe(schema: String, table: TableInfo, referenced_by: Vec<Reference>) -> DescribeTableOutput {
    DescribeTableOutput {
        schema,
        name: table.name,
        kind: table_kind(table.kind),
        comment: table.comment,
        columns: table
            .columns
            .into_iter()
            .map(|c| ColumnDescription {
                name: c.name,
                type_name: c.type_name,
                nullable: c.nullable,
                default: c.default,
                primary_key: c.primary_key,
                generated: c.generated,
                comment: c.comment,
            })
            .collect(),
        foreign_keys: table
            .foreign_keys
            .into_iter()
            .map(|fk| ForeignKeyDescription {
                name: fk.name,
                columns: fk.columns,
                referenced_schema: fk.referenced_schema,
                referenced_table: fk.referenced_table,
                referenced_columns: fk.referenced_columns,
            })
            .collect(),
        referenced_by,
    }
}

fn result_columns(columns: Vec<idedb_core::Column>) -> Vec<ResultColumn> {
    columns.into_iter().map(|c| ResultColumn { name: c.name, type_name: c.type_name }).collect()
}

fn execute_output(ran: Ran) -> ExecuteOutput {
    let returned = ran.columns.is_some();
    ExecuteOutput {
        row_count: ran.row_count,
        columns: ran.columns.map(result_columns),
        rows: returned.then_some(ran.rows),
        elapsed_ms: ran.elapsed_ms,
        truncated: ran.truncated,
    }
}

/// A tool call's audit row in the making.
struct Call<'a> {
    core: &'a Core,
    caller: &'a Caller,
    tool: &'static str,
    data_source: Option<DataSource>,
    sql: Option<&'a str>,
    statement_kind: Option<String>,
    reason: Option<&'a str>,
    started: Instant,
}

/// How a call ended, for its audit row.
#[derive(Debug, Default)]
struct Outcome {
    approval_wait_ms: Option<u64>,
    elapsed_ms: Option<u64>,
    row_count: Option<u64>,
    truncated: bool,
    error: Option<String>,
}

impl Outcome {
    fn ran(ran: &Ran) -> Self {
        Self {
            elapsed_ms: Some(ran.elapsed_ms),
            row_count: Some(ran.row_count),
            truncated: ran.truncated,
            ..Self::default()
        }
    }
}

impl<'a> Call<'a> {
    fn new(core: &'a Core, caller: &'a Caller, tool: &'static str) -> Self {
        Self {
            core,
            caller,
            tool,
            data_source: None,
            sql: None,
            statement_kind: None,
            reason: None,
            started: Instant::now(),
        }
    }

    /// A listing of `count` items, done now.
    fn listed(&self, count: usize) -> Outcome {
        Outcome {
            elapsed_ms: Some(self.started.elapsed().as_millis() as u64),
            row_count: Some(count as u64),
            ..Outcome::default()
        }
    }

    /// Writes the audit row and tells the UI.
    fn record(&self, decision: Decision, outcome: Outcome) -> Result<(), Error> {
        let caller = self.caller;
        let info = caller.client_info.as_ref();
        let entry = self.core.host.store().mcp_add_audit(NewAuditEntry {
            client_id: Some(&caller.client_id),
            client_name: &caller.client_name,
            client_info_name: info.map(|i| i.name.as_str()),
            client_info_version: info.and_then(|i| i.version.as_deref()),
            protocol_version: caller.protocol_version.as_deref(),
            transport: caller.transport,
            session_key: caller.session_key.as_deref(),
            tool: self.tool,
            data_source_id: self.data_source.as_ref().map(|s| s.id.as_str()),
            data_source_name: self.data_source.as_ref().map(|s| s.name.as_str()),
            sql: self.sql,
            statement_kind: self.statement_kind.as_deref(),
            reason: self.reason,
            decision,
            approval_wait_ms: outcome.approval_wait_ms,
            elapsed_ms: outcome.elapsed_ms,
            row_count: outcome.row_count,
            truncated: outcome.truncated,
            error: outcome.error.as_deref(),
        })?;
        self.core.host.notify(McpEvent::Audit(entry));
        Ok(())
    }

    /// Audits a call refused without running anything.
    fn deny<T>(&self, message: String) -> Result<T, CallError> {
        self.refuse(Decision::Denied, message, Outcome::default())
    }

    /// Audits a call that did not run, as `decision`.
    fn refuse<T>(&self, decision: Decision, message: String, outcome: Outcome) -> Result<T, CallError> {
        self.record(decision, Outcome { error: Some(message.clone()), ..outcome })?;
        Err(ToolError::new(message).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_describe_arguments_for_the_model() {
        let schema = serde_json::to_value(schemars::schema_for!(QueryArgs)).unwrap();
        let properties = &schema["properties"];
        assert!(properties["sql"]["description"].as_str().unwrap().contains("read-only"), "{schema}");
        assert_eq!(properties["maxRows"]["minimum"], 1, "{schema}");
        assert_eq!(properties["maxRows"]["maximum"], 1000, "{schema}");
        assert_eq!(schema["required"], serde_json::json!(["connection", "sql"]), "{schema}");

        let execute = serde_json::to_value(schemars::schema_for!(ExecuteArgs)).unwrap();
        assert!(execute["properties"]["reason"].is_object(), "{execute}");
        // Outputs have schemas too, for structured content.
        let output = serde_json::to_value(schemars::schema_for!(ListConnectionsOutput)).unwrap();
        assert!(output.to_string().contains("writesAllowed"), "{output}");
    }

    #[test]
    fn arguments_accept_what_models_send() {
        let args: ListConnectionsArgs = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(args, ListConnectionsArgs {});
        let args: ListSchemasArgs = serde_json::from_value(serde_json::json!({ "connection": "shop" })).unwrap();
        assert!(!args.include_system);
        let args: QueryArgs =
            serde_json::from_value(serde_json::json!({ "connection": "shop", "sql": "select 1", "maxRows": 5 })).unwrap();
        assert_eq!(args.max_rows, Some(5));
    }
}
