//! The token pass: what can be decided without (or before) parsing.

use std::any::TypeId;

use idedb_core::Engine;
use sqlparser::ast::Statement;
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer, Whitespace};

/// Statements with more significant tokens than this are not parsed.
/// sqlparser builds a left-deep tree for chains like `1+1+…` or
/// `select 1 union select 1 …` without its recursion limit, and dropping
/// that tree recurses once per level: past ~20k levels a debug build
/// overflows a 2 MiB thread stack (the size of Tokio's worker threads).
const MAX_PARSED_TOKENS: usize = 10_000;

/// Functions that change state (or reach outside the database) even inside
/// a `SELECT`, compared by name in lowercase, whatever the engine: the
/// names are distinctive enough, and a false match only asks for approval.
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
    "pg_ls_dir",
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

/// What the token pass found. Every finding is the union over the default
/// and the flipped-backslash readings of the text.
pub(crate) struct Scan {
    /// The default reading, whitespace and comments included, with MySQL
    /// executable comments expanded; what gets parsed.
    tokens: Vec<TokenWithSpan>,
    /// The default reading failed to tokenize (unterminated literal or
    /// comment…); `tokens` holds what came before the error.
    failed: bool,
    /// The number of statements, empty ones not counted.
    statements: usize,
    /// The first keyword, uppercased, skipping leading `(` and `;`.
    pub first_keyword: Option<String>,
    pub multiple_statements: bool,
    /// MySQL `INTO OUTFILE` or `INTO DUMPFILE`.
    pub into_file: Option<&'static str>,
    /// The first known side-effect function called, lowercased.
    pub side_effect_function: Option<String>,
    /// The first row-locking clause, e.g. `FOR UPDATE`.
    pub row_lock: Option<&'static str>,
    /// Whether a `WHERE` keyword appears anywhere.
    pub has_where: bool,
}

pub(crate) fn scan(engine: Engine, sql: &str) -> Scan {
    let (tokens, failed) = match engine {
        Engine::Postgres => tokenize(&PostgreSqlDialect {}, sql),
        Engine::Mysql => tokenize(&MySqlDialect {}, sql),
        Engine::Sqlite => tokenize(&SQLiteDialect {}, sql),
    };
    // SQLite has no backslash escapes, so it reads its literals one way only.
    let flipped = match engine {
        Engine::Postgres => Some(tokenize(&FlippedBackslash(PostgreSqlDialect {}), sql)),
        Engine::Mysql => Some(tokenize(&FlippedBackslash(MySqlDialect {}), sql)),
        Engine::Sqlite => None,
    };

    let words = significant(&tokens);
    let statements = statement_count(&words, failed);
    let mut scan = Scan {
        first_keyword: first_keyword(&words),
        multiple_statements: statements > 1,
        into_file: if engine == Engine::Mysql { into_file(&words) } else { None },
        side_effect_function: side_effect_function(&words),
        row_lock: row_lock(&words),
        has_where: words.iter().any(|token| is_keyword(token, "WHERE")),
        statements,
        tokens,
        failed,
    };
    if let Some((tokens, failed)) = &flipped {
        let words = significant(tokens);
        scan.multiple_statements |= statement_count(&words, *failed) > 1;
        if engine == Engine::Mysql && scan.into_file.is_none() {
            scan.into_file = into_file(&words);
        }
        if scan.side_effect_function.is_none() {
            scan.side_effect_function = side_effect_function(&words);
        }
        if scan.row_lock.is_none() {
            scan.row_lock = row_lock(&words);
        }
    }
    scan
}

impl Scan {
    /// Nothing but whitespace, comments and semicolons.
    pub fn is_empty(&self) -> bool {
        self.statements == 0
    }

    /// The default reading tokenized without errors.
    pub fn ok(&self) -> bool {
        !self.failed
    }

    /// The significant tokens of the default reading, semicolons excluded.
    pub fn words(&self) -> Vec<&Token> {
        significant(&self.tokens).into_iter().filter(|token| **token != Token::SemiColon).collect()
    }

    /// Parses the single statement, or `None` when it does not parse, the
    /// text did not tokenize, or it is too long to parse safely. Unlike
    /// `Parser::parse_statements`, which stops quietly at a stray `END`,
    /// everything after the statement must be semicolons.
    pub fn parse(&self, engine: Engine) -> Option<Statement> {
        if self.failed || significant(&self.tokens).len() > MAX_PARSED_TOKENS {
            return None;
        }
        let dialect: &dyn Dialect = match engine {
            Engine::Postgres => &PostgreSqlDialect {},
            Engine::Mysql => &MySqlDialect {},
            Engine::Sqlite => &SQLiteDialect {},
        };
        let mut parser = Parser::new(dialect).with_tokens_with_locations(self.tokens.clone());
        while parser.consume_token(&Token::SemiColon) {}
        let statement = parser.parse_statement().ok()?;
        while parser.consume_token(&Token::SemiColon) {}
        (parser.peek_token().token == Token::EOF).then_some(statement)
    }
}

/// Tokenizes `sql`, keeping the tokens before an error. For MySQL, also
/// expands the comments the server runs as code.
fn tokenize(dialect: &dyn Dialect, sql: &str) -> (Vec<TokenWithSpan>, bool) {
    let mut tokens = Vec::new();
    let mut failed = Tokenizer::new(dialect, sql).tokenize_with_location_into_buf(&mut tokens).is_err();
    if dialect.is::<MySqlDialect>() {
        tokens = expand_mysql_comments(dialect, tokens, &mut failed);
    }
    (tokens, failed)
}

/// Replaces the MySQL comments that the server reads as code with their
/// tokens. sqlparser already expands top-level `/*! … */`; this covers
/// MariaDB's `/*M! … */`, a `/*!` nested in another, and `--` followed by
/// a non-ASCII space: sqlparser takes it for a comment (it accepts any
/// Unicode whitespace after `--`), MySQL only an ASCII space or control
/// character.
fn expand_mysql_comments(dialect: &dyn Dialect, tokens: Vec<TokenWithSpan>, failed: &mut bool) -> Vec<TokenWithSpan> {
    let mut expanded = Vec::with_capacity(tokens.len());
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
                let (inner, inner_failed) = tokenize(dialect, code);
                *failed |= inner_failed;
                expanded.extend(inner);
            }
            None => expanded.push(token),
        }
    }
    expanded
}

/// The code inside `/*!12345 … */` or `/*M!12345 … */` (version optional).
fn executable_comment(comment: &str) -> Option<&str> {
    let rest =
        comment.strip_prefix('!').or_else(|| comment.strip_prefix("M!")).or_else(|| comment.strip_prefix("m!"))?;
    Some(rest.trim_start_matches(|c: char| c.is_ascii_digit()))
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

/// A dialect that tokenizes like `D` but with the opposite rule for
/// backslashes in string literals: Postgres with
/// `standard_conforming_strings = off`, MySQL with `NO_BACKSLASH_ESCAPES`
/// (or `ANSI_QUOTES`, where `"…"` is an identifier without escapes). Only
/// the tokenizer's hooks are forwarded; it is never used to parse.
#[derive(Debug)]
struct FlippedBackslash<D>(D);

impl<D: Dialect> Dialect for FlippedBackslash<D> {
    // Makes the tokenizer's `dialect_of!` checks (MySQL `#` comments…) see `D`.
    fn dialect(&self) -> TypeId {
        self.0.dialect()
    }
    fn supports_string_literal_backslash_escape(&self) -> bool {
        !self.0.supports_string_literal_backslash_escape()
    }

    fn is_identifier_start(&self, ch: char) -> bool {
        self.0.is_identifier_start(ch)
    }
    fn is_identifier_part(&self, ch: char) -> bool {
        self.0.is_identifier_part(ch)
    }
    fn is_delimited_identifier_start(&self, ch: char) -> bool {
        self.0.is_delimited_identifier_start(ch)
    }
    fn is_custom_operator_part(&self, ch: char) -> bool {
        self.0.is_custom_operator_part(ch)
    }
    fn ignores_wildcard_escapes(&self) -> bool {
        self.0.ignores_wildcard_escapes()
    }
    fn requires_single_line_comment_whitespace(&self) -> bool {
        self.0.requires_single_line_comment_whitespace()
    }
    fn supports_dollar_as_money_prefix(&self) -> bool {
        self.0.supports_dollar_as_money_prefix()
    }
    fn supports_dollar_placeholder(&self) -> bool {
        self.0.supports_dollar_placeholder()
    }
    fn supports_geometric_types(&self) -> bool {
        self.0.supports_geometric_types()
    }
    fn supports_multiline_comment_hints(&self) -> bool {
        self.0.supports_multiline_comment_hints()
    }
    fn supports_nested_comments(&self) -> bool {
        self.0.supports_nested_comments()
    }
    fn supports_numeric_literal_underscores(&self) -> bool {
        self.0.supports_numeric_literal_underscores()
    }
    fn supports_numeric_prefix(&self) -> bool {
        self.0.supports_numeric_prefix()
    }
    fn supports_pipe_operator(&self) -> bool {
        self.0.supports_pipe_operator()
    }
    fn supports_quote_delimited_string(&self) -> bool {
        self.0.supports_quote_delimited_string()
    }
    fn supports_string_escape_constant(&self) -> bool {
        self.0.supports_string_escape_constant()
    }
    fn supports_triple_quoted_string(&self) -> bool {
        self.0.supports_triple_quoted_string()
    }
    fn supports_unicode_string_literal(&self) -> bool {
        self.0.supports_unicode_string_literal()
    }
}
