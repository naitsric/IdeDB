//! Rules for a parsed statement, and what to do without one.

use std::ops::ControlFlow;

use idedb_core::Engine;
use sqlparser::ast::{
    AlterTableOperation, Insert, LockType, ObjectType, Query, Select, Set, SetExpr, Statement, Visit, Visitor,
};
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::Token;

use crate::scan::Scan;
use crate::{Forbidden as F, Kind, Verdict, Warning, WriteKind as W, verdict};

/// SQLite pragmas that only read when given no value; with one they change
/// a setting (`PRAGMA user_version = 5`).
const PRAGMA_SETTINGS: &[&str] = &[
    "application_id",
    "auto_vacuum",
    "automatic_index",
    "busy_timeout",
    "cache_size",
    "cache_spill",
    "cell_size_check",
    "compile_options",
    "data_version",
    "defer_foreign_keys",
    "encoding",
    "foreign_keys",
    "freelist_count",
    "journal_mode",
    "journal_size_limit",
    "legacy_alter_table",
    "locking_mode",
    "max_page_count",
    "mmap_size",
    "page_count",
    "page_size",
    "query_only",
    "read_uncommitted",
    "recursive_triggers",
    "schema_version",
    "secure_delete",
    "synchronous",
    "temp_store",
    "trusted_schema",
    "user_version",
    "wal_autocheckpoint",
];

/// SQLite pragmas that read whatever their argument: it names a table,
/// index or schema to describe or check.
const PRAGMA_LISTINGS: &[&str] = &[
    "collation_list",
    "database_list",
    "foreign_key_check",
    "foreign_key_list",
    "function_list",
    "index_info",
    "index_list",
    "index_xinfo",
    "integrity_check",
    "module_list",
    "pragma_list",
    "quick_check",
    "table_info",
    "table_list",
    "table_xinfo",
];

fn read(summary: impl Into<String>) -> Verdict {
    verdict(Kind::Read, summary, Vec::new())
}

fn write(kind: W, summary: impl Into<String>) -> Verdict {
    verdict(Kind::Write(kind), summary, Vec::new())
}

fn forbidden(kind: F, summary: impl Into<String>) -> Verdict {
    verdict(Kind::Forbidden(kind), summary, Vec::new())
}

fn destructive(kind: W, summary: impl Into<String>) -> Verdict {
    verdict(Kind::Write(kind), summary, vec![Warning::DropsOrTruncates])
}

fn without_where(selection_missing: bool) -> Vec<Warning> {
    if selection_missing { vec![Warning::NoWhereClause] } else { Vec::new() }
}

pub(crate) fn classify(engine: Engine, statement: &Statement) -> Verdict {
    match statement {
        // Reads, when nothing inside them writes.
        Statement::Query(query) => query_verdict(engine, query),
        // Whatever EXPLAIN would do, it is judged by what it explains:
        // `EXPLAIN (ANALYZE) DELETE …` runs the DELETE.
        Statement::Explain { statement, analyze, options, .. } => {
            let analyze = *analyze || options.iter().flatten().any(|o| o.name.value.eq_ignore_ascii_case("analyze"));
            let inner = classify(engine, statement);
            let prefix = if analyze { "EXPLAIN ANALYZE" } else { "EXPLAIN" };
            Verdict { summary: format!("{prefix} {}", inner.summary), ..inner }
        }
        Statement::ExplainTable { .. } => read("DESCRIBE"),
        Statement::ShowFunctions { .. } => read("SHOW FUNCTIONS"),
        // Also what sqlparser makes of MySQL's SHOW INDEX, SHOW GRANTS,
        // SHOW WARNINGS, SHOW ENGINE … STATUS.
        Statement::ShowVariable { .. } => read("SHOW"),
        Statement::ShowStatus { .. } => read("SHOW STATUS"),
        Statement::ShowVariables { .. } => read("SHOW VARIABLES"),
        Statement::ShowCreate { .. } => read("SHOW CREATE"),
        Statement::ShowColumns { .. } => read("SHOW COLUMNS"),
        Statement::ShowCatalogs { .. } => read("SHOW CATALOGS"),
        Statement::ShowDatabases { .. } => read("SHOW DATABASES"),
        Statement::ShowProcessList { .. } => read("SHOW PROCESSLIST"),
        Statement::ShowSchemas { .. } => read("SHOW SCHEMAS"),
        Statement::ShowCharset { .. } => read("SHOW CHARSET"),
        Statement::ShowObjects { .. } => read("SHOW OBJECTS"),
        Statement::ShowTables { .. } => read("SHOW TABLES"),
        Statement::ShowViews { .. } => read("SHOW VIEWS"),
        Statement::ShowCollation { .. } => read("SHOW COLLATION"),
        Statement::Pragma { name, value, .. } => {
            pragma(name.0.last().and_then(|part| part.as_ident()).map(|ident| ident.value.as_str()), value.is_some())
        }

        // Data.
        Statement::Insert(insert) => write(W::Dml, insert_verb(insert)),
        Statement::Update(update) => verdict(Kind::Write(W::Dml), "UPDATE", without_where(update.selection.is_none())),
        Statement::Delete(delete) => verdict(Kind::Write(W::Dml), "DELETE", without_where(delete.selection.is_none())),
        Statement::Merge { .. } => write(W::Dml, "MERGE"),
        Statement::Truncate { .. } => destructive(W::Dml, "TRUNCATE"),

        // Schema.
        Statement::CreateTable(create) if create.temporary => write(W::Ddl, "CREATE TEMPORARY TABLE"),
        Statement::CreateTable { .. } => write(W::Ddl, "CREATE TABLE"),
        Statement::CreateView(create) if create.materialized => write(W::Ddl, "CREATE MATERIALIZED VIEW"),
        Statement::CreateView { .. } => write(W::Ddl, "CREATE VIEW"),
        Statement::CreateVirtualTable { .. } => write(W::Ddl, "CREATE VIRTUAL TABLE"),
        Statement::CreateIndex { .. } => write(W::Ddl, "CREATE INDEX"),
        Statement::CreateSchema { .. } => write(W::Ddl, "CREATE SCHEMA"),
        Statement::CreateDatabase { .. } => write(W::Ddl, "CREATE DATABASE"),
        Statement::CreateFunction { .. } => write(W::Ddl, "CREATE FUNCTION"),
        Statement::CreateProcedure { .. } => write(W::Ddl, "CREATE PROCEDURE"),
        Statement::CreateTrigger { .. } => write(W::Ddl, "CREATE TRIGGER"),
        Statement::CreateSequence { .. } => write(W::Ddl, "CREATE SEQUENCE"),
        Statement::CreateType { .. } => write(W::Ddl, "CREATE TYPE"),
        Statement::CreateDomain { .. } => write(W::Ddl, "CREATE DOMAIN"),
        Statement::CreateExtension { .. } => write(W::Ddl, "CREATE EXTENSION"),
        Statement::CreateCollation { .. } => write(W::Ddl, "CREATE COLLATION"),
        Statement::CreatePolicy { .. } => write(W::Ddl, "CREATE POLICY"),
        Statement::CreateServer { .. } => write(W::Ddl, "CREATE SERVER"),
        Statement::CreateSecret { .. } => write(W::Ddl, "CREATE SECRET"),
        Statement::CreateConnector { .. } => write(W::Ddl, "CREATE CONNECTOR"),
        Statement::CreateOperator { .. } => write(W::Ddl, "CREATE OPERATOR"),
        Statement::CreateOperatorFamily { .. } => write(W::Ddl, "CREATE OPERATOR FAMILY"),
        Statement::CreateOperatorClass { .. } => write(W::Ddl, "CREATE OPERATOR CLASS"),
        Statement::CreateTextSearch { .. } => write(W::Ddl, "CREATE TEXT SEARCH"),
        Statement::CreateMacro { .. } => write(W::Ddl, "CREATE MACRO"),
        Statement::CreateStage { .. } => write(W::Ddl, "CREATE STAGE"),
        Statement::CreateFileFormat { .. } => write(W::Ddl, "CREATE FILE FORMAT"),
        Statement::CreateWarehouse { .. } => write(W::Ddl, "CREATE WAREHOUSE"),
        Statement::AlterTable(alter) => {
            let drops_data = alter.operations.iter().any(|operation| {
                matches!(operation, AlterTableOperation::DropColumn { .. } | AlterTableOperation::DropPartitions { .. })
            });
            if drops_data { destructive(W::Ddl, "ALTER TABLE") } else { write(W::Ddl, "ALTER TABLE") }
        }
        Statement::AlterSchema { .. } => write(W::Ddl, "ALTER SCHEMA"),
        Statement::AlterIndex { .. } => write(W::Ddl, "ALTER INDEX"),
        Statement::AlterView { .. } => write(W::Ddl, "ALTER VIEW"),
        Statement::AlterFunction { .. } => write(W::Ddl, "ALTER FUNCTION"),
        Statement::AlterType { .. } => write(W::Ddl, "ALTER TYPE"),
        Statement::AlterCollation { .. } => write(W::Ddl, "ALTER COLLATION"),
        Statement::AlterOperator { .. } => write(W::Ddl, "ALTER OPERATOR"),
        Statement::AlterOperatorFamily { .. } => write(W::Ddl, "ALTER OPERATOR FAMILY"),
        Statement::AlterOperatorClass { .. } => write(W::Ddl, "ALTER OPERATOR CLASS"),
        Statement::AlterTextSearch { .. } => write(W::Ddl, "ALTER TEXT SEARCH"),
        Statement::AlterPolicy { .. } => write(W::Ddl, "ALTER POLICY"),
        Statement::AlterConnector { .. } => write(W::Ddl, "ALTER CONNECTOR"),
        Statement::Drop { object_type: object_type @ (ObjectType::Role | ObjectType::User), .. } => {
            destructive(W::Privileges, format!("DROP {object_type}"))
        }
        Statement::Drop { object_type, .. } => destructive(W::Ddl, format!("DROP {object_type}")),
        Statement::DropFunction { .. } => destructive(W::Ddl, "DROP FUNCTION"),
        Statement::DropProcedure { .. } => destructive(W::Ddl, "DROP PROCEDURE"),
        Statement::DropTrigger { .. } => destructive(W::Ddl, "DROP TRIGGER"),
        Statement::DropDomain { .. } => destructive(W::Ddl, "DROP DOMAIN"),
        Statement::DropExtension { .. } => destructive(W::Ddl, "DROP EXTENSION"),
        Statement::DropPolicy { .. } => destructive(W::Ddl, "DROP POLICY"),
        Statement::DropSecret { .. } => destructive(W::Ddl, "DROP SECRET"),
        Statement::DropConnector { .. } => destructive(W::Ddl, "DROP CONNECTOR"),
        Statement::DropOperator { .. } => destructive(W::Ddl, "DROP OPERATOR"),
        Statement::DropOperatorFamily { .. } => destructive(W::Ddl, "DROP OPERATOR FAMILY"),
        Statement::DropOperatorClass { .. } => destructive(W::Ddl, "DROP OPERATOR CLASS"),
        Statement::RenameTable { .. } => write(W::Ddl, "RENAME TABLE"),
        Statement::Comment { .. } => write(W::Ddl, "COMMENT ON"),

        // Privileges.
        Statement::Grant { .. } => write(W::Privileges, "GRANT"),
        Statement::Revoke { .. } => write(W::Privileges, "REVOKE"),
        Statement::Deny { .. } => write(W::Privileges, "DENY"),
        Statement::CreateRole { .. } => write(W::Privileges, "CREATE ROLE"),
        Statement::CreateUser { .. } => write(W::Privileges, "CREATE USER"),
        // sqlparser also reads Postgres' ALTER USER as ALTER ROLE.
        Statement::AlterRole { .. } => write(W::Privileges, "ALTER ROLE"),
        Statement::AlterUser { .. } => write(W::Privileges, "ALTER USER"),

        // Procedures and procedural code.
        Statement::Call { .. } => write(W::Procedural, "CALL"),
        Statement::Case { .. } => write(W::Procedural, "CASE"),
        Statement::If { .. } => write(W::Procedural, "IF"),
        Statement::While { .. } => write(W::Procedural, "WHILE"),
        Statement::Raise { .. } => write(W::Procedural, "RAISE"),
        Statement::RaisError { .. } => write(W::Procedural, "RAISERROR"),
        Statement::Throw { .. } => write(W::Procedural, "THROW"),
        Statement::Print { .. } => write(W::Procedural, "PRINT"),
        Statement::WaitFor { .. } => write(W::Procedural, "WAITFOR"),
        Statement::Return { .. } => write(W::Procedural, "RETURN"),
        Statement::Assert { .. } => write(W::Procedural, "ASSERT"),

        // Maintenance and anything else that changes the server.
        Statement::Analyze { .. } => write(W::Other, "ANALYZE"),
        Statement::OptimizeTable { .. } => write(W::Other, "OPTIMIZE TABLE"),
        Statement::Msck { .. } => write(W::Other, "MSCK"),
        Statement::Cache { .. } => write(W::Other, "CACHE"),
        Statement::UNCache { .. } => write(W::Other, "UNCACHE"),
        // FLUSH TABLES WITH READ LOCK (or FOR EXPORT) holds a lock after the
        // statement ends, like LOCK TABLES.
        Statement::Flush { read_lock, export, .. } if *read_lock || *export => {
            forbidden(F::TransactionControl, "FLUSH TABLES WITH READ LOCK")
        }
        Statement::Flush { .. } => write(W::Other, "FLUSH"),

        // Transactions: each MCP call runs in its own.
        Statement::StartTransaction { begin: true, .. } => forbidden(F::TransactionControl, "BEGIN"),
        Statement::StartTransaction { .. } => forbidden(F::TransactionControl, "START TRANSACTION"),
        Statement::Commit { end: true, .. } => forbidden(F::TransactionControl, "END"),
        Statement::Commit { .. } => forbidden(F::TransactionControl, "COMMIT"),
        Statement::Rollback { .. } => forbidden(F::TransactionControl, "ROLLBACK"),
        Statement::Savepoint { .. } => forbidden(F::TransactionControl, "SAVEPOINT"),
        Statement::ReleaseSavepoint { .. } => forbidden(F::TransactionControl, "RELEASE SAVEPOINT"),
        Statement::Lock { .. } => forbidden(F::TransactionControl, "LOCK TABLE"),
        Statement::LockTables { .. } => forbidden(F::TransactionControl, "LOCK TABLES"),
        Statement::UnlockTables => forbidden(F::TransactionControl, "UNLOCK TABLES"),

        // Session state: it would outlive the call on a pooled session.
        Statement::Set(set) => forbidden(F::SessionState, set_label(set)),
        Statement::Reset { .. } => forbidden(F::SessionState, "RESET"),
        Statement::Discard { .. } => forbidden(F::SessionState, "DISCARD"),
        Statement::Use { .. } => forbidden(F::SessionState, "USE"),
        Statement::AlterSession { .. } => forbidden(F::SessionState, "ALTER SESSION"),

        // Files and anything outside the database.
        Statement::Copy { .. } => forbidden(F::FileAccess, "COPY"),
        Statement::CopyIntoSnowflake { .. } => forbidden(F::FileAccess, "COPY INTO"),
        Statement::LoadData { .. } => forbidden(F::FileAccess, "LOAD DATA"),
        // DuckDB's LOAD, but also what sqlparser makes of Postgres'
        // `LOAD 'library'`.
        Statement::Load { .. } => forbidden(F::FileAccess, "LOAD"),
        Statement::Install { .. } => forbidden(F::FileAccess, "INSTALL"),
        Statement::Directory { .. } => forbidden(F::FileAccess, "INSERT OVERWRITE DIRECTORY"),
        Statement::AttachDatabase { .. } | Statement::AttachDuckDBDatabase { .. } => forbidden(F::FileAccess, "ATTACH"),
        Statement::DetachDuckDBDatabase { .. } => forbidden(F::FileAccess, "DETACH"),
        Statement::Vacuum { .. } => forbidden(F::FileAccess, "VACUUM"),
        Statement::Unload { .. } => forbidden(F::FileAccess, "UNLOAD"),
        Statement::ExportData { .. } => forbidden(F::FileAccess, "EXPORT DATA"),
        Statement::Put { .. } => forbidden(F::FileAccess, "PUT"),
        Statement::List { .. } => forbidden(F::FileAccess, "LIST"),
        Statement::Remove { .. } => forbidden(F::FileAccess, "REMOVE"),

        // Cursors and prepared statements outlive the call too.
        Statement::Declare { .. } => forbidden(F::Cursor, "DECLARE"),
        Statement::Fetch { .. } => forbidden(F::Cursor, "FETCH"),
        Statement::Open { .. } => forbidden(F::Cursor, "OPEN"),
        Statement::Close { .. } => forbidden(F::Cursor, "CLOSE"),
        Statement::Prepare { .. } => forbidden(F::PreparedStatement, "PREPARE"),
        Statement::Execute { .. } => forbidden(F::PreparedStatement, "EXECUTE"),
        Statement::Deallocate { .. } => forbidden(F::PreparedStatement, "DEALLOCATE"),

        Statement::LISTEN { .. } => forbidden(F::Other, "LISTEN"),
        Statement::UNLISTEN { .. } => forbidden(F::Other, "UNLISTEN"),
        Statement::NOTIFY { .. } => forbidden(F::Other, "NOTIFY"),
        Statement::Kill { .. } => forbidden(F::Other, "KILL"),
    }
}

fn insert_verb(insert: &Insert) -> &'static str {
    let replace = matches!(&insert.insert_token.0.token, Token::Word(word) if word.keyword == Keyword::REPLACE);
    if replace || insert.replace_into { "REPLACE" } else { "INSERT" }
}

fn set_label(set: &Set) -> &'static str {
    match set {
        Set::SetRole { .. } => "SET ROLE",
        Set::SetTransaction { .. } => "SET TRANSACTION",
        Set::SetNames { .. } | Set::SetNamesDefault { .. } => "SET NAMES",
        _ => "SET",
    }
}

/// A query is a read unless something inside it writes, creates a table
/// or locks rows, at any depth: CTEs, subqueries, set operations.
fn query_verdict(engine: Engine, query: &Query) -> Verdict {
    let mut parts = QueryParts::default();
    let _ = query.visit(&mut parts);
    let label = body_label(&query.body);
    if let Some(dml) = parts.dml {
        let summary = if label == dml { dml.to_string() } else { format!("{label} with {dml}") };
        return verdict(Kind::Write(W::Dml), summary, parts.warnings);
    }
    if parts.other_statement {
        return write(W::Other, label);
    }
    if parts.select_into {
        // MySQL stores into variables (`INTO @x`); Postgres creates a table.
        return match engine {
            Engine::Mysql => forbidden(F::SessionState, "SELECT INTO"),
            Engine::Postgres | Engine::Sqlite => write(W::Ddl, "SELECT INTO"),
        };
    }
    if let Some(lock) = parts.lock {
        return write(W::LockingRead, format!("{label} {lock}"));
    }
    read(label)
}

fn body_label(body: &SetExpr) -> &'static str {
    match body {
        SetExpr::Insert(statement)
        | SetExpr::Update(statement)
        | SetExpr::Delete(statement)
        | SetExpr::Merge(statement) => dml_verb(statement).unwrap_or("SELECT"),
        SetExpr::Values(_) => "VALUES",
        SetExpr::Table(_) => "TABLE",
        SetExpr::Select(_) | SetExpr::Query(_) | SetExpr::SetOperation { .. } => "SELECT",
    }
}

fn dml_verb(statement: &Statement) -> Option<&'static str> {
    match statement {
        Statement::Insert(insert) => Some(insert_verb(insert)),
        Statement::Update { .. } => Some("UPDATE"),
        Statement::Delete { .. } => Some("DELETE"),
        Statement::Merge { .. } => Some("MERGE"),
        _ => None,
    }
}

/// What a query holds, found by walking all of it.
#[derive(Default)]
struct QueryParts {
    /// The first data-modifying statement (`WITH d AS (DELETE …)`, or the
    /// body of `WITH … INSERT`).
    dml: Option<&'static str>,
    /// Any other statement nested in the query (none is known to parse).
    other_statement: bool,
    select_into: bool,
    lock: Option<&'static str>,
    warnings: Vec<Warning>,
}

impl Visitor for QueryParts {
    type Break = ();

    // Only nested statements: the visit starts at the query.
    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<()> {
        match statement {
            Statement::Update(update) => self.warnings.extend(without_where(update.selection.is_none())),
            Statement::Delete(delete) => self.warnings.extend(without_where(delete.selection.is_none())),
            _ => {}
        }
        match dml_verb(statement) {
            Some(verb) => {
                self.dml.get_or_insert(verb);
            }
            None => self.other_statement = true,
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        if let Some(lock) = query.locks.first() {
            self.lock.get_or_insert(match lock.lock_type {
                LockType::Share => "FOR SHARE",
                LockType::Update => "FOR UPDATE",
            });
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_select(&mut self, select: &Select) -> ControlFlow<()> {
        self.select_into |= select.into.is_some();
        ControlFlow::Continue(())
    }
}

/// A SQLite `PRAGMA`: a read only for the allowlists above.
fn pragma(name: Option<&str>, has_value: bool) -> Verdict {
    let Some(name) = name else {
        return write(W::Other, "PRAGMA");
    };
    let name = name.to_ascii_lowercase();
    let reads = PRAGMA_LISTINGS.contains(&name.as_str()) || (!has_value && PRAGMA_SETTINGS.contains(&name.as_str()));
    let summary = format!("PRAGMA {name}");
    if reads { read(summary) } else { write(W::Other, summary) }
}

/// `PRAGMA [schema.]name [= value | (value)]`, read from its tokens:
/// sqlparser only accepts literal values, so it rejects the common
/// `PRAGMA table_info(t)`.
pub(crate) fn pragma_from_tokens(scan: &Scan) -> Verdict {
    if !scan.ok() {
        return write(W::Unparsed, "PRAGMA");
    }
    let words = scan.words();
    let (name, rest) = match words.as_slice() {
        [_, Token::Word(_), Token::Period, Token::Word(name), rest @ ..] => (name, rest),
        [_, Token::Word(name), rest @ ..] => (name, rest),
        _ => return write(W::Other, "PRAGMA"),
    };
    let has_value = match rest {
        [] => false,
        [Token::Eq, _, ..] | [Token::LParen, .., Token::RParen] => true,
        _ => return write(W::Other, format!("PRAGMA {}", name.value.to_ascii_lowercase())),
    };
    pragma(Some(&name.value), has_value)
}

/// A statement that did not parse (or was too long to), judged by its
/// first keyword: what would be forbidden stays forbidden.
pub(crate) fn fallback(scan: &Scan) -> Verdict {
    let Some(first) = scan.first_keyword.as_deref() else {
        return write(W::Unparsed, "Unrecognized statement");
    };
    let kind = match first {
        "BEGIN" | "START" | "COMMIT" | "END" | "ROLLBACK" | "ABORT" | "SAVEPOINT" | "RELEASE" | "LOCK" | "UNLOCK"
        | "XA" => Kind::Forbidden(F::TransactionControl),
        "SET" | "RESET" | "DISCARD" | "USE" => Kind::Forbidden(F::SessionState),
        "COPY" | "LOAD" | "ATTACH" | "DETACH" | "VACUUM" | "UNLOAD" => Kind::Forbidden(F::FileAccess),
        "DECLARE" | "FETCH" | "MOVE" | "OPEN" | "CLOSE" | "HANDLER" => Kind::Forbidden(F::Cursor),
        "PREPARE" | "EXECUTE" | "DEALLOCATE" => Kind::Forbidden(F::PreparedStatement),
        "LISTEN" | "NOTIFY" | "UNLISTEN" | "KILL" | "SHUTDOWN" | "RESTART" => Kind::Forbidden(F::Other),
        // sqlparser does not parse DO at all.
        "DO" | "CALL" => Kind::Write(W::Procedural),
        _ => Kind::Write(W::Unparsed),
    };
    let warnings = match first {
        "DROP" | "TRUNCATE" => vec![Warning::DropsOrTruncates],
        "UPDATE" | "DELETE" => without_where(!scan.has_where),
        _ => Vec::new(),
    };
    verdict(kind, first, warnings)
}
