//! Classifies a SQL statement that an MCP client (an LLM) sends to IdeDB:
//! a read the read-only `query` tool may run, a write that needs a person's
//! approval through the `execute` tool, or something IdeDB refuses outright.
//!
//! The classifier is an **allowlist**: only what it can show is a read is a
//! [`Kind::Read`]; anything else is a write or forbidden. It is defense in
//! depth and the policy layer (it also writes the label shown in the
//! approval dialog and the audit log), not the security boundary: MCP reads
//! run on read-only engine sessions, and a read-only database user is what
//! finally limits what a statement can do. Functions with side effects that
//! the classifier does not know (user-defined ones, for instance) look like
//! reads to it.
//!
//! [`classify`] works in three steps:
//!
//! 1. **Tokens**, which work even when parsing fails. More than one
//!    statement is forbidden (a trailing `;` is fine), and so is empty input.
//!    Calls to known functions with side effects (`set_config`, `nextval`,
//!    `pg_advisory_lock`, `get_lock`, `load_file`…) turn a read into a
//!    write, and so do row locks (`FOR UPDATE`…); MySQL `INTO OUTFILE` and
//!    `INTO DUMPFILE` are forbidden. MySQL executable comments (`/*! … */`,
//!    MariaDB's `/*M! … */`) count as code. Postgres and MySQL are also
//!    tokenized with the opposite backslash rule in string literals, because
//!    the server's setting (`standard_conforming_strings`,
//!    `NO_BACKSLASH_ESCAPES`) decides where a literal ends: every check runs
//!    on both readings.
//! 2. **Parse** with the engine's dialect (sqlparser). A statement that does
//!    not parse is classified by its first keyword: transaction control,
//!    `SET`, `COPY`, cursors… stay forbidden, `DO` and `CALL` are
//!    procedural, and anything else is [`WriteKind::Unparsed`]. So is a
//!    statement of more than 10,000 tokens, which is not parsed: dropping
//!    the deep trees sqlparser builds for long `a + b + …` chains could
//!    overflow the stack. SQLite `PRAGMA` is classified from its tokens
//!    (sqlparser rejects `PRAGMA table_info(t)`).
//! 3. **Rules** on the parsed statement; see [`Kind`] and its parts.
//!
//! A `;` anywhere outside literals and comments separates statements, so
//! a body holding `;` (SQLite `CREATE TRIGGER … BEGIN …; END`, MySQL
//! `CREATE PROCEDURE … BEGIN …; END`) counts as several and is forbidden.

mod scan;
mod statement;

use idedb_core::Engine;
use serde::{Deserialize, Serialize};

/// What IdeDB may do with a statement, with a label and warnings for the
/// person approving it.
///
/// Serialized flat, for audit rows and the UI:
/// `{"kind":"write","detail":"dml","summary":"DELETE","warnings":["noWhereClause"],"looksLikeRead":false}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    #[serde(flatten)]
    pub kind: Kind,
    /// Short label of the statement type: `SELECT`, `DELETE`,
    /// `CREATE TABLE`, `EXPLAIN ANALYZE UPDATE`, `PRAGMA table_info`…
    pub summary: String,
    pub warnings: Vec<Warning>,
    /// Whether the read-only `query` tool may run the statement: true for
    /// every [`Kind::Read`], and for a [`WriteKind::Unparsed`] statement whose
    /// first keyword is `SELECT`, `WITH`, `SHOW`, `EXPLAIN`, `DESCRIBE`,
    /// `DESC`, `VALUES` or `TABLE` (the engine's read-only session is the
    /// barrier for those); false for everything else. The token checks
    /// never let a statement through: with a side-effect function or a row
    /// lock the kind is no longer `Unparsed`.
    pub looks_like_read: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "camelCase")]
pub enum Kind {
    /// Only reads: `SELECT`/`WITH`/`VALUES`/`TABLE` without data-modifying
    /// parts, `INTO` or row locks; `SHOW`; `DESCRIBE`; `EXPLAIN` of a read;
    /// a SQLite `PRAGMA` from an allowlist.
    Read,
    /// Needs a person's approval.
    Write(WriteKind),
    /// Refused without asking: each MCP call runs one statement in its own
    /// transaction on a pooled session, so these would break that model or
    /// reach outside the database.
    Forbidden(Forbidden),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WriteKind {
    /// `INSERT`, `REPLACE`, `UPDATE`, `DELETE`, `MERGE`, `TRUNCATE`, and a
    /// query with a data-modifying `WITH` (`WITH d AS (DELETE …) SELECT …`).
    Dml,
    /// `CREATE`, `ALTER`, `DROP`, `RENAME`, `COMMENT ON`, and `SELECT … INTO`
    /// a new table.
    Ddl,
    /// `GRANT`, `REVOKE`, `DENY`, and creating, altering or dropping users
    /// and roles.
    Privileges,
    /// `CALL`, `DO`, and procedural statements (`IF`, `RAISE`…).
    Procedural,
    /// A read that calls a function with side effects (`set_config`,
    /// `nextval`, `pg_advisory_lock`, `dblink`, `get_lock`, `load_file`,
    /// `load_extension`…). The summary names the function.
    SideEffectFunction,
    /// A read that locks rows: `FOR UPDATE`, `FOR SHARE`, `FOR NO KEY
    /// UPDATE`, `FOR KEY SHARE`, `LOCK IN SHARE MODE`.
    LockingRead,
    /// A statement the parser understood that fits no other kind:
    /// `ANALYZE`, `FLUSH`, `OPTIMIZE TABLE`, a `PRAGMA` outside the
    /// allowlist…
    Other,
    /// A statement the parser could not read (or too long to parse safely).
    /// See [`Classification::looks_like_read`].
    Unparsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Forbidden {
    /// Nothing to run: whitespace, comments or semicolons only.
    Empty,
    /// More than one statement.
    MultipleStatements,
    /// `BEGIN`/`START TRANSACTION`, `COMMIT`/`END`, `ROLLBACK`/`ABORT`,
    /// `SAVEPOINT`, `RELEASE`, `LOCK TABLE(S)`, `UNLOCK TABLES`, `XA`,
    /// `FLUSH TABLES WITH READ LOCK`.
    TransactionControl,
    /// `SET` (including `SET ROLE` and `SET TRANSACTION`), `RESET`,
    /// `DISCARD`, `USE`, and MySQL `SELECT … INTO @variable`.
    SessionState,
    /// `COPY`, `LOAD DATA`, `LOAD`, `ATTACH`/`DETACH`, `VACUUM` (`VACUUM
    /// INTO` writes a file), `UNLOAD`, MySQL `INTO OUTFILE`/`INTO DUMPFILE`.
    FileAccess,
    /// `DECLARE`, `FETCH`, `MOVE`, `OPEN`, `CLOSE`, MySQL `HANDLER`.
    Cursor,
    /// `PREPARE`, `EXECUTE`, `DEALLOCATE`.
    PreparedStatement,
    /// `LISTEN`, `NOTIFY`, `UNLISTEN`, `KILL`, and MySQL `SHUTDOWN` and
    /// `RESTART`.
    Other,
}

/// Something the person approving a write should look at twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Warning {
    /// `UPDATE` or `DELETE` without `WHERE`: every row of the table.
    NoWhereClause,
    /// `DROP`, `TRUNCATE` or `ALTER TABLE … DROP COLUMN`: data is destroyed.
    DropsOrTruncates,
}

/// Keywords that make an unparsed statement look like a read.
const READ_KEYWORDS: &[&str] = &["SELECT", "WITH", "SHOW", "EXPLAIN", "DESCRIBE", "DESC", "VALUES", "TABLE"];

/// Classifies one SQL statement for `engine`. Never fails: input it cannot
/// understand is a write ([`WriteKind::Unparsed`]) or forbidden.
pub fn classify(engine: Engine, sql: &str) -> Classification {
    let scan = scan::scan(engine, sql);
    if scan.is_empty() {
        return verdict(Kind::Forbidden(Forbidden::Empty), "Empty statement", Vec::new()).finish(false);
    }
    if scan.multiple_statements {
        return verdict(Kind::Forbidden(Forbidden::MultipleStatements), "Multiple statements", Vec::new())
            .finish(false);
    }
    if let Some(into_file) = scan.into_file {
        return verdict(Kind::Forbidden(Forbidden::FileAccess), format!("SELECT {into_file}"), Vec::new())
            .finish(false);
    }

    let first = scan.first_keyword.as_deref();
    let mut verdict = if engine == Engine::Sqlite && first == Some("PRAGMA") {
        statement::pragma_from_tokens(&scan)
    } else {
        match scan.parse(engine) {
            Some(parsed) => statement::classify(engine, &parsed),
            None => statement::fallback(&scan),
        }
    };

    let read_keyword = scan.ok() && first.is_some_and(|word| READ_KEYWORDS.contains(&word));
    let looks_like_read = |kind: Kind| match kind {
        Kind::Read => true,
        Kind::Write(WriteKind::Unparsed) => read_keyword,
        _ => false,
    };
    // The token checks catch what the parser may not see (or not parse):
    // they turn a read, or an unparsed statement that looks like one, into
    // a write.
    if looks_like_read(verdict.kind) {
        if let Some(function) = &scan.side_effect_function {
            verdict.kind = Kind::Write(WriteKind::SideEffectFunction);
            verdict.summary = format!("{} calling {function}()", verdict.summary);
        } else if let Some(lock) = scan.row_lock {
            verdict.kind = Kind::Write(WriteKind::LockingRead);
            verdict.summary = format!("{} {lock}", verdict.summary);
        }
    }
    let looks_like_read = looks_like_read(verdict.kind);
    verdict.finish(looks_like_read)
}

/// A classification in the making.
pub(crate) struct Verdict {
    pub kind: Kind,
    pub summary: String,
    pub warnings: Vec<Warning>,
}

pub(crate) fn verdict(kind: Kind, summary: impl Into<String>, warnings: Vec<Warning>) -> Verdict {
    Verdict { kind, summary: summary.into(), warnings }
}

impl Verdict {
    fn finish(mut self, looks_like_read: bool) -> Classification {
        // A query can hold several UPDATEs or DELETEs without WHERE.
        self.warnings.sort();
        self.warnings.dedup();
        Classification { kind: self.kind, summary: self.summary, warnings: self.warnings, looks_like_read }
    }
}
