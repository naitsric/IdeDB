//! The token pass: what can be decided without (or before) parsing.
//!
//! Where a literal or a quoted identifier ends depends on server settings
//! the classifier cannot see, so the text is read once per setting that
//! moves those boundaries (see [`scan`]) and every check runs on each
//! reading: the strictest outcome wins.

use std::any::TypeId;

use idedb_core::Engine;
use sqlparser::ast::Statement;
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer, Whitespace};

/// Statements with more significant tokens than this are not parsed.
/// sqlparser builds a left-deep tree for chains like `1+1+…` or
/// `select 1 union select 1 …` without its recursion limit, and dropping
/// that tree recurses once per level: past ~20k levels a debug build
/// overflows a 2 MiB thread stack (the size of Tokio's worker threads).
const MAX_PARSED_TOKENS: usize = 10_000;

/// Functions that change state, wait, or reach outside the database even
/// inside a `SELECT`, compared by name in lowercase, whatever the engine:
/// the names are distinctive enough, and a false match only asks for
/// approval.
const SIDE_EFFECT_FUNCTIONS: &[&str] = &[
    // Postgres
    "set_config",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_reload_conf",
    "pg_rotate_logfile",
    "pg_notify",
    "cursor_to_xml",
    "nextval",
    "setval",
    "txid_current",
    "pg_current_xact_id",
    "pg_read_file",
    "pg_read_binary_file",
    "pg_stat_file",
    "pg_stat_statements_reset",
    "pg_switch_wal",
    "pg_promote",
    "pg_drop_replication_slot",
    "pg_replication_slot_advance",
    "pg_logical_slot_get_changes",
    "pg_logical_slot_get_binary_changes",
    "pg_logical_emit_message",
    "pg_log_backend_memory_contexts",
    "pg_backup_start",
    "pg_backup_stop",
    "pg_start_backup",
    "pg_stop_backup",
    "pg_wal_replay_pause",
    "pg_wal_replay_resume",
    "pg_import_system_collations",
    // MySQL
    "get_lock",
    "release_lock",
    "release_all_locks",
    "load_file",
    "benchmark",
    "sleep",
    "master_pos_wait",
    "source_pos_wait",
    "wait_for_executed_gtid_set",
    "wait_until_sql_thread_after_gtids",
    "sys_exec",
    "sys_eval",
    // SQLite
    "load_extension",
    "readfile",
    "writefile",
];

/// Prefixes of function families with side effects, as above.
const SIDE_EFFECT_PREFIXES: &[&str] = &[
    "pg_advisory",     // pg_advisory_lock, pg_advisory_xact_lock_shared…
    "pg_try_advisory", // pg_try_advisory_lock…
    "pg_sleep",        // pg_sleep, pg_sleep_for, pg_sleep_until
    "pg_ls_",          // server directories: pg_ls_dir, pg_ls_logdir, pg_ls_waldir…
    "lo_",             // large objects: lo_import, lo_export, lo_unlink…
    "dblink",          // dblink, dblink_exec, dblink_connect…
    "pg_file_",        // adminpack: pg_file_write, pg_file_unlink…
    "query_to_xml",    // runs the query text it is given
    "pg_stat_reset",
    "pg_create_", // replication slots, restore points
    "pg_copy_",   // replication slots
    "pg_replication_origin_",
];

fn is_side_effect_function(name: &str) -> bool {
    SIDE_EFFECT_FUNCTIONS.contains(&name) || SIDE_EFFECT_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// Keywords that start or introduce a statement that is not a read. A
/// statement that does not parse but starts like a read (`EXPLAIN …`,
/// `WITH …`) only looks like one without them. A keyword followed by `(` is
/// a function (`replace(…)`, MySQL `insert(…)`, `truncate(…)`) and does not
/// count.
const WRITE_KEYWORDS: &[&str] = &[
    "INSERT",
    "UPDATE",
    "DELETE",
    "MERGE",
    "REPLACE",
    "UPSERT",
    "TRUNCATE",
    "CREATE",
    "ALTER",
    "DROP",
    "RENAME",
    "GRANT",
    "REVOKE",
    "CALL",
    "DO",
    "SET",
    "RESET",
    "COPY",
    "LOAD",
    "LOCK",
    "UNLOCK",
    "EXECUTE",
    "PREPARE",
    "DECLARE",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "VACUUM",
    "ATTACH",
    "DETACH",
    "REFRESH",
    "REINDEX",
    "LISTEN",
    "NOTIFY",
    "KILL",
    "OUTFILE",
    "DUMPFILE",
];

/// One reading of the text.
struct Reading {
    /// Whitespace and comments included, with MySQL executable comments
    /// expanded.
    tokens: Vec<TokenWithSpan>,
    /// Byte offset where tokenizing failed (unterminated literal or
    /// comment…); `tokens` then holds what came before.
    error: Option<usize>,
}

/// What the token pass found, over every reading.
pub(crate) struct Scan {
    /// The server's default reading first, then the other settings'.
    readings: Vec<Reading>,
    /// The first keyword of the default reading, uppercased, skipping
    /// leading `(` and `;`.
    pub first_keyword: Option<String>,
    /// Whether the default reading has a `WHERE` keyword anywhere.
    pub has_where: bool,
    /// Nothing but whitespace, comments and semicolons, in every reading.
    pub empty: bool,
    /// More than one statement in some reading, or a `;` followed by more
    /// text after the point where some reading failed.
    pub multiple_statements: bool,
    /// MySQL `INTO OUTFILE` or `INTO DUMPFILE`.
    pub into_file: Option<&'static str>,
    /// MySQL user variables written by a statement: `INTO @x`, `@x := …`.
    pub user_variable: Option<&'static str>,
    /// The first known side-effect function called, lowercased.
    pub side_effect_function: Option<String>,
    /// The first row-locking clause, e.g. `FOR UPDATE`.
    pub row_lock: Option<&'static str>,
    /// The first keyword of [`WRITE_KEYWORDS`], uppercased.
    pub write_keyword: Option<String>,
}

/// Reads `sql` the ways `engine` may read it:
///
/// - Postgres: `standard_conforming_strings` on (the default: `\` is plain
///   in `'…'`) and off (`\'` escapes the quote).
/// - MySQL: backslash escapes on (the default) and off
///   (`NO_BACKSLASH_ESCAPES`), each with `"…"` as a string (the default)
///   and as an identifier without escapes (`ANSI_QUOTES`). Mixing the two
///   quote styles can hide a `;` from every reading but one.
/// - SQLite: one way; it has no backslash escapes.
pub(crate) fn scan(engine: Engine, sql: &str) -> Scan {
    let readings = readings(engine, sql);
    let default = significant(&readings[0].tokens);
    let mut scan = Scan {
        first_keyword: first_keyword(&default),
        has_where: default.iter().any(|token| is_keyword(token, "WHERE")),
        empty: true,
        multiple_statements: false,
        into_file: None,
        user_variable: None,
        side_effect_function: None,
        row_lock: None,
        write_keyword: None,
        readings: Vec::new(),
    };
    for reading in &readings {
        let words = significant(&reading.tokens);
        scan.empty &= statement_count(&words, reading.error.is_some()) == 0;
        scan.multiple_statements |= reading.more_than_one_statement(sql);
        if engine == Engine::Mysql {
            scan.into_file = scan.into_file.or_else(|| into_file(&words));
            scan.user_variable = scan.user_variable.or_else(|| user_variable(&words));
        }
        scan.side_effect_function = scan.side_effect_function.take().or_else(|| side_effect_function(&words));
        scan.row_lock = scan.row_lock.or_else(|| row_lock(&words));
        scan.write_keyword = scan.write_keyword.take().or_else(|| write_keyword(&words));
    }
    scan.readings = readings;
    scan
}

impl Scan {
    /// Every reading tokenized without errors.
    pub fn tokenized(&self) -> bool {
        self.readings.iter().all(|reading| reading.error.is_none())
    }

    /// The default reading tokenized without errors.
    pub fn default_tokenized(&self) -> bool {
        self.readings[0].error.is_none()
    }

    /// The significant tokens of the default reading, semicolons excluded.
    pub fn words(&self) -> Vec<&Token> {
        significant(&self.readings[0].tokens).into_iter().filter(|token| **token != Token::SemiColon).collect()
    }

    /// The other readings that tokenized, by index; the default one is 0.
    pub fn other_readings(&self) -> impl Iterator<Item = usize> + '_ {
        (1..self.readings.len()).filter(|&index| self.readings[index].error.is_none())
    }

    /// Parses the single statement of a reading, or `None` when it does not
    /// parse, the reading did not tokenize, or it is too long to parse
    /// safely. Unlike `Parser::parse_statements`, which stops quietly at a
    /// stray `END`, everything after the statement must be semicolons.
    pub fn parse(&self, engine: Engine, reading: usize) -> Option<Statement> {
        let reading = &self.readings[reading];
        if reading.error.is_some() || significant(&reading.tokens).len() > MAX_PARSED_TOKENS {
            return None;
        }
        let dialect: &dyn Dialect = match engine {
            Engine::Postgres => &PostgreSqlDialect {},
            Engine::Mysql => &MySqlDialect {},
            Engine::Sqlite => &SQLiteDialect {},
        };
        let mut parser = Parser::new(dialect).with_tokens_with_locations(british_analyse(reading.tokens.clone()));
        while parser.consume_token(&Token::SemiColon) {}
        let statement = parser.parse_statement().ok()?;
        while parser.consume_token(&Token::SemiColon) {}
        (parser.peek_token().token == Token::EOF).then_some(statement)
    }
}

/// The readings of `sql` described at [`scan`], the default one first.
fn readings(engine: Engine, sql: &str) -> Vec<Reading> {
    match engine {
        Engine::Postgres => vec![
            tokenize(&PostgreSqlDialect {}, sql),
            tokenize(&Settings { dialect: PostgreSqlDialect {}, backslash_escapes: true, ansi_quotes: false }, sql),
        ],
        Engine::Mysql => {
            let with = |backslash_escapes, ansi_quotes| {
                tokenize(&Settings { dialect: MySqlDialect {}, backslash_escapes, ansi_quotes }, sql)
            };
            vec![tokenize(&MySqlDialect {}, sql), with(false, false), with(true, true), with(false, true)]
        }
        Engine::Sqlite => vec![tokenize(&SQLiteDialect {}, sql)],
    }
}

impl Reading {
    /// More than one statement, or a `;` followed by more text past the
    /// point where tokenizing failed: nothing is known there, so it may
    /// start another statement.
    fn more_than_one_statement(&self, sql: &str) -> bool {
        statement_count(&significant(&self.tokens), self.error.is_some()) > 1
            || self.error.is_some_and(|at| semicolon_then_more(&sql[at..]))
    }
}

/// Postgres accepts `EXPLAIN ANALYSE`; sqlparser only the American
/// spelling.
fn british_analyse(mut tokens: Vec<TokenWithSpan>) -> Vec<TokenWithSpan> {
    let mut words = tokens.iter_mut().filter(|t| !matches!(t.token, Token::Whitespace(_)));
    if let (Some(first), Some(second)) = (words.next(), words.next())
        && is_keyword(&first.token, "EXPLAIN")
        && is_keyword(&second.token, "ANALYSE")
    {
        second.token = Token::make_keyword("ANALYZE");
    }
    tokens
}

/// Tokenizes `sql`, keeping the tokens before an error. For MySQL, also
/// expands the comments the server runs as code.
fn tokenize(dialect: &dyn Dialect, sql: &str) -> Reading {
    let mut tokens = Vec::new();
    let error = Tokenizer::new(dialect, sql)
        .tokenize_with_location_into_buf(&mut tokens)
        .err()
        .map(|error| offset_of(sql, error.location));
    let mut reading = Reading { tokens, error };
    if dialect.is::<MySqlDialect>() {
        expand_mysql_comments(dialect, sql, &mut reading);
    }
    reading
}

/// Replaces the MySQL comments that the server reads as code with their
/// tokens. sqlparser already expands top-level `/*! … */`; this covers
/// MariaDB's `/*M! … */`, a `/*!` nested in another, and `--` followed by
/// a non-ASCII space: sqlparser takes it for a comment (it accepts any
/// Unicode whitespace after `--`), MySQL only an ASCII space or control
/// character. An error inside such a comment counts from its start.
fn expand_mysql_comments(dialect: &dyn Dialect, sql: &str, reading: &mut Reading) {
    let tokens = std::mem::take(&mut reading.tokens);
    for token in tokens {
        let code = match &token.token {
            Token::Whitespace(Whitespace::MultiLineComment(comment)) => executable_comment(comment),
            Token::Whitespace(Whitespace::SingleLineComment { prefix, comment })
                if prefix == "--" && comment.starts_with(|c: char| !c.is_ascii()) =>
            {
                Some(comment.as_str())
            }
            _ => None,
        };
        match code {
            Some(code) => {
                let inner = tokenize(dialect, code);
                if inner.error.is_some() {
                    let at = offset_of(sql, token.span.start);
                    reading.error = Some(reading.error.map_or(at, |error| error.min(at)));
                }
                reading.tokens.extend(inner.tokens);
            }
            None => reading.tokens.push(token),
        }
    }
}

/// The code inside `/*!12345 … */` or `/*M!12345 … */` (version optional).
fn executable_comment(comment: &str) -> Option<&str> {
    let rest =
        comment.strip_prefix('!').or_else(|| comment.strip_prefix("M!")).or_else(|| comment.strip_prefix("m!"))?;
    Some(rest.trim_start_matches(|c: char| c.is_ascii_digit()))
}

/// The byte offset of a tokenizer location (1-based line and column in
/// characters), clamped to the line it names.
fn offset_of(sql: &str, location: Location) -> usize {
    let line_start: usize =
        sql.split_inclusive('\n').take((location.line as usize).saturating_sub(1)).map(str::len).sum();
    let line = sql[line_start..].split('\n').next().unwrap_or("");
    let column = (location.column as usize).saturating_sub(1);
    line_start + line.char_indices().nth(column).map_or(line.len(), |(index, _)| index)
}

/// A `;` with anything but whitespace after it.
fn semicolon_then_more(text: &str) -> bool {
    text.split_once(';').is_some_and(|(_, rest)| rest.chars().any(|c| !c.is_whitespace()))
}

fn significant(tokens: &[TokenWithSpan]) -> Vec<&Token> {
    tokens.iter().map(|t| &t.token).filter(|t| !matches!(t, Token::Whitespace(_) | Token::EOF)).collect()
}

/// Counts non-empty statements. When tokenizing failed, the text after the
/// last token belongs to the last statement, or starts a new one after a
/// `;`.
fn statement_count(significant: &[&Token], failed: bool) -> usize {
    let mut count = 0;
    let mut open = false;
    for token in significant {
        if **token == Token::SemiColon {
            if open {
                count += 1;
                open = false;
            }
        } else {
            open = true;
        }
    }
    if open || failed {
        count += 1;
    }
    count
}

/// An unquoted word equal to `keyword`, ignoring case.
fn is_keyword(token: &Token, keyword: &str) -> bool {
    matches!(token, Token::Word(word) if word.quote_style.is_none() && word.value.eq_ignore_ascii_case(keyword))
}

fn first_keyword(significant: &[&Token]) -> Option<String> {
    match significant.iter().find(|token| !matches!(***token, Token::LParen | Token::SemiColon))? {
        Token::Word(word) if word.quote_style.is_none() => Some(word.value.to_ascii_uppercase()),
        _ => None,
    }
}

fn into_file(significant: &[&Token]) -> Option<&'static str> {
    significant.windows(2).find_map(|pair| {
        if !is_keyword(pair[0], "INTO") {
            None
        } else if is_keyword(pair[1], "OUTFILE") {
            Some("INTO OUTFILE")
        } else if is_keyword(pair[1], "DUMPFILE") {
            Some("INTO DUMPFILE")
        } else {
            None
        }
    })
}

/// MySQL `@x := …` anywhere, or `INTO @x` (after the select list or at the
/// end: sqlparser only parses the former).
fn user_variable(significant: &[&Token]) -> Option<&'static str> {
    if significant.iter().any(|token| **token == Token::Assignment) {
        return Some("with := assignment");
    }
    let variable = |token: &Token| match token {
        Token::AtSign => true,
        Token::Word(word) => word.quote_style.is_none() && word.value.starts_with('@'),
        _ => false,
    };
    significant.windows(2).any(|pair| is_keyword(pair[0], "INTO") && variable(pair[1])).then_some("INTO @variable")
}

/// A word, quoted or not and optionally schema-qualified, followed by `(`:
/// `"nextval"('s')` and `pg_catalog.set_config(…)` are calls too.
fn side_effect_function(significant: &[&Token]) -> Option<String> {
    significant.windows(2).find_map(|pair| match (pair[0], pair[1]) {
        (Token::Word(word), Token::LParen) => {
            let name = word.value.to_lowercase();
            is_side_effect_function(&name).then_some(name)
        }
        _ => None,
    })
}

fn row_lock(significant: &[&Token]) -> Option<&'static str> {
    significant.windows(2).enumerate().find_map(|(i, pair)| {
        if is_keyword(pair[0], "FOR") {
            if is_keyword(pair[1], "UPDATE") {
                return Some("FOR UPDATE");
            }
            if is_keyword(pair[1], "SHARE") {
                return Some("FOR SHARE");
            }
            if is_keyword(pair[1], "NO") && significant.get(i + 2).is_some_and(|t| is_keyword(t, "KEY")) {
                return Some("FOR NO KEY UPDATE");
            }
            if is_keyword(pair[1], "KEY") && significant.get(i + 2).is_some_and(|t| is_keyword(t, "SHARE")) {
                return Some("FOR KEY SHARE");
            }
        }
        let lock_in_share_mode = ["LOCK", "IN", "SHARE", "MODE"];
        let rest = significant.get(i..i + 4)?;
        rest.iter().zip(lock_in_share_mode).all(|(t, k)| is_keyword(t, k)).then_some("LOCK IN SHARE MODE")
    })
}

fn write_keyword(significant: &[&Token]) -> Option<String> {
    significant.iter().enumerate().find_map(|(i, token)| {
        let Token::Word(word) = token else { return None };
        let keyword = word.value.to_ascii_uppercase();
        let is_call = significant.get(i + 1).is_some_and(|next| **next == Token::LParen);
        // MySQL `CAST(x AS CHAR CHARACTER SET utf8mb4)`.
        let is_charset = keyword == "SET"
            && i > 0
            && (is_keyword(significant[i - 1], "CHARACTER") || is_keyword(significant[i - 1], "CHARSET"));
        (word.quote_style.is_none() && WRITE_KEYWORDS.contains(&keyword.as_str()) && !is_call && !is_charset)
            .then_some(keyword)
    })
}

/// A dialect that tokenizes like `D` under server settings that move where
/// literals and quoted identifiers end: backslash escapes in string
/// literals on or off, and (MySQL `ANSI_QUOTES`) `"…"` as an identifier.
/// Only the tokenizer's hooks are forwarded; it is never used to parse.
#[derive(Debug)]
struct Settings<D> {
    dialect: D,
    backslash_escapes: bool,
    ansi_quotes: bool,
}

impl<D: Dialect> Dialect for Settings<D> {
    // Makes the tokenizer's `dialect_of!` checks (MySQL `#` comments…) see `D`.
    fn dialect(&self) -> TypeId {
        self.dialect.dialect()
    }
    fn supports_string_literal_backslash_escape(&self) -> bool {
        self.backslash_escapes
    }
    fn is_delimited_identifier_start(&self, ch: char) -> bool {
        (self.ansi_quotes && ch == '"') || self.dialect.is_delimited_identifier_start(ch)
    }

    fn is_identifier_start(&self, ch: char) -> bool {
        self.dialect.is_identifier_start(ch)
    }
    fn is_identifier_part(&self, ch: char) -> bool {
        self.dialect.is_identifier_part(ch)
    }
    fn is_custom_operator_part(&self, ch: char) -> bool {
        self.dialect.is_custom_operator_part(ch)
    }
    fn ignores_wildcard_escapes(&self) -> bool {
        self.dialect.ignores_wildcard_escapes()
    }
    fn requires_single_line_comment_whitespace(&self) -> bool {
        self.dialect.requires_single_line_comment_whitespace()
    }
    fn supports_dollar_as_money_prefix(&self) -> bool {
        self.dialect.supports_dollar_as_money_prefix()
    }
    fn supports_dollar_placeholder(&self) -> bool {
        self.dialect.supports_dollar_placeholder()
    }
    fn supports_geometric_types(&self) -> bool {
        self.dialect.supports_geometric_types()
    }
    fn supports_multiline_comment_hints(&self) -> bool {
        self.dialect.supports_multiline_comment_hints()
    }
    fn supports_nested_comments(&self) -> bool {
        self.dialect.supports_nested_comments()
    }
    fn supports_numeric_literal_underscores(&self) -> bool {
        self.dialect.supports_numeric_literal_underscores()
    }
    fn supports_numeric_prefix(&self) -> bool {
        self.dialect.supports_numeric_prefix()
    }
    fn supports_pipe_operator(&self) -> bool {
        self.dialect.supports_pipe_operator()
    }
    fn supports_quote_delimited_string(&self) -> bool {
        self.dialect.supports_quote_delimited_string()
    }
    fn supports_string_escape_constant(&self) -> bool {
        self.dialect.supports_string_escape_constant()
    }
    fn supports_triple_quoted_string(&self) -> bool {
        self.dialect.supports_triple_quoted_string()
    }
    fn supports_unicode_string_literal(&self) -> bool {
        self.dialect.supports_unicode_string_literal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which readings see more than one statement, in [`readings`] order.
    fn multiple_by_reading(engine: Engine, sql: &str) -> Vec<bool> {
        readings(engine, sql).iter().map(|reading| reading.more_than_one_statement(sql)).collect()
    }

    fn write_keyword_of(engine: Engine, sql: &str) -> Option<String> {
        write_keyword(&significant(&readings(engine, sql)[0].tokens))
    }

    #[test]
    fn each_mysql_setting_is_read() {
        // Default: `\"` escapes the quote, so the string ends at the second `"`.
        let default = r#"select "\"" ; drop table t; -- ""#;
        assert_eq!(multiple_by_reading(Engine::Mysql, default), [true, false, false, false]);
        // NO_BACKSLASH_ESCAPES, with or without ANSI_QUOTES (literals end at
        // the same places): `'a\'` is a whole literal.
        let no_backslash = r"select 'a\' ; drop table t; -- '";
        assert_eq!(multiple_by_reading(Engine::Mysql, no_backslash), [false, true, false, true]);
        // ANSI_QUOTES with backslash escapes: `"\"` is an identifier and
        // `'\''` a quote, so the `;` is outside both; every other reading
        // keeps it inside a literal.
        let ansi_quotes = r#"select "\" , '\'' ; drop table t; -- '""#;
        assert_eq!(multiple_by_reading(Engine::Mysql, ansi_quotes), [false, false, true, false]);
    }

    #[test]
    fn each_postgres_setting_is_read() {
        // standard_conforming_strings on: `'a\'` is a whole literal.
        assert_eq!(multiple_by_reading(Engine::Postgres, r"select 'a\'; drop table t; --'"), [true, false]);
        // Off: `\'` escapes the quote, so `'\''` is a whole literal.
        assert_eq!(multiple_by_reading(Engine::Postgres, r"select '\''; drop table t; --'"), [false, true]);
    }

    #[test]
    fn a_semicolon_past_a_tokenizing_error_starts_a_statement() {
        // sqlparser rejects the escape; nothing after it is known.
        let rejected = r"select E'\uZZZZ'; delete from t";
        assert_eq!(multiple_by_reading(Engine::Postgres, rejected), [true, true]);
        assert_eq!(multiple_by_reading(Engine::Postgres, r"select E'\uZZZZ';  "), [false, false]);
        assert_eq!(multiple_by_reading(Engine::Mysql, "select 'unterminated; drop table t"), [true; 4]);
    }

    #[test]
    fn tokenizer_locations_become_byte_offsets() {
        let sql = "select 1,\n  'ñ', 'x";
        assert_eq!(offset_of(sql, Location { line: 2, column: 8 }), sql.rfind('\'').unwrap());
        assert_eq!(offset_of(sql, Location { line: 1, column: 1 }), 0);
        assert_eq!(offset_of(sql, Location { line: 9, column: 9 }), sql.len());
    }

    #[test]
    fn write_keywords_skip_functions_and_character_sets() {
        assert_eq!(write_keyword_of(Engine::Postgres, "explain analyse delete from t").as_deref(), Some("DELETE"));
        assert_eq!(
            write_keyword_of(Engine::Mysql, "select replace(a, 'x', 'y'), truncate(1.5, 0), insert('ab', 1, 1, 'c')"),
            None
        );
        assert_eq!(write_keyword_of(Engine::Mysql, "select cast(a as char character set utf8mb4)"), None);
        assert_eq!(write_keyword_of(Engine::Postgres, "select \"delete\", 'update' from t"), None);
    }
}
