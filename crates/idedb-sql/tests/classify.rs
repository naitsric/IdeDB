//! Table-driven checks of `classify`: shared cases on every engine, then
//! each engine's own syntax. A case lists the expected kind and warnings;
//! every case also checks that `looks_like_read` agrees with the kind.
//!
//! Where sqlparser cannot express a statement, a comment says so and the
//! expected outcome is the safe one.

use idedb_core::Engine;
use idedb_sql::{Classification, Forbidden as F, Kind, Warning, WriteKind as W, classify};

const R: Kind = Kind::Read;
const DML: Kind = Kind::Write(W::Dml);
const DDL: Kind = Kind::Write(W::Ddl);
const PRIV: Kind = Kind::Write(W::Privileges);
const PROC: Kind = Kind::Write(W::Procedural);
const FUNC: Kind = Kind::Write(W::SideEffectFunction);
const LOCK: Kind = Kind::Write(W::LockingRead);
const OTHER: Kind = Kind::Write(W::Other);
const UNPARSED: Kind = Kind::Write(W::Unparsed);
const EMPTY: Kind = Kind::Forbidden(F::Empty);
const MULTI: Kind = Kind::Forbidden(F::MultipleStatements);
const TX: Kind = Kind::Forbidden(F::TransactionControl);
const SESSION: Kind = Kind::Forbidden(F::SessionState);
const FILE: Kind = Kind::Forbidden(F::FileAccess);
const CURSOR: Kind = Kind::Forbidden(F::Cursor);
const PREPARED: Kind = Kind::Forbidden(F::PreparedStatement);
const REFUSED: Kind = Kind::Forbidden(F::Other);

const NONE: &[Warning] = &[];
const NO_WHERE: &[Warning] = &[Warning::NoWhereClause];
const DROPS: &[Warning] = &[Warning::DropsOrTruncates];

type Case = (&'static str, Kind, &'static [Warning]);

/// Runs every case and reports all failures at once.
fn check(engine: Engine, cases: &[Case]) {
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|&(sql, kind, warnings)| {
            let got = classify(engine, sql);
            let consistent = got.kind == UNPARSED || got.looks_like_read == (got.kind == R);
            (got.kind != kind || got.warnings != warnings || !consistent).then(|| {
                format!(
                    "{engine:?} {sql:?}\n    expected {kind:?} {warnings:?}\n    got      {:?} {:?} looks_like_read={} ({})",
                    got.kind, got.warnings, got.looks_like_read, got.summary
                )
            })
        })
        .collect();
    assert!(failures.is_empty(), "{} of {} cases failed:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

/// The same outcome on Postgres, MySQL and SQLite.
const COMMON: &[Case] = &[
    // Empty input.
    ("", EMPTY, NONE),
    ("   \n\t ", EMPTY, NONE),
    ("-- only a comment", EMPTY, NONE),
    ("/* only a comment */", EMPTY, NONE),
    (";", EMPTY, NONE),
    (" ; ; -- nothing", EMPTY, NONE),
    // One statement, however it is written.
    ("select 1", R, NONE),
    ("select 1;", R, NONE),
    ("select 1 ;  -- done", R, NONE),
    ("select 1;;", R, NONE),
    ("; select 1", R, NONE),
    ("  SeLeCt  *\n\tFrOm   t  \n", R, NONE),
    ("SELECT 1 /* ; drop table t */", R, NONE),
    ("select 1 -- ; drop table t", R, NONE),
    // More than one.
    ("select 1; select 2", MULTI, NONE),
    ("select 1; drop table t", MULTI, NONE),
    ("select 1;\n-- then\nselect 2;", MULTI, NONE),
    ("delete from t; select 1", MULTI, NONE),
    ("begin; delete from t; commit", MULTI, NONE),
    // Literals and quoted names that look like SQL are just data.
    ("select * from t where a = 'x;y'", R, NONE),
    ("select 'drop table t; select 1'", R, NONE),
    ("select * from t where a = 'FOR UPDATE'", R, NONE),
    ("select 'nextval(1)'", R, NONE),
    ("select \"delete\" from t", R, NONE),
    // Reads.
    ("with x as (select 1 as a) select * from x", R, NONE),
    ("with recursive r(n) as (select 1 union all select n + 1 from r where n < 5) select * from r", R, NONE),
    ("select a, count(*) from t group by a having count(*) > 1 order by 2 desc limit 10", R, NONE),
    ("select * from t join u on t.id = u.t_id left join v on v.id = u.v_id", R, NONE),
    ("select * from t where id in (select t_id from u where u.a > 1)", R, NONE),
    ("select case when a > 1 then 'x' else 'y' end from t", R, NONE),
    ("(select 1) union (select 2)", R, NONE),
    ("select 1 union all select 2", R, NONE),
    ("explain select * from t", R, NONE),
    // Side effects and locks turn a read into a write.
    ("select nextval('s')", FUNC, NONE),
    ("select * from t for update", LOCK, NONE),
    ("select * from t where id in (select id from u for update)", LOCK, NONE),
    // Writes.
    ("insert into t (a) values (1)", DML, NONE),
    ("insert into t (a) select a from u", DML, NONE),
    ("update t set a = 1 where id = 2", DML, NONE),
    ("update t set a = 1", DML, NO_WHERE),
    ("delete from t where id = 1", DML, NONE),
    ("delete from t", DML, NO_WHERE),
    ("create table t (id int primary key, name varchar(10))", DDL, NONE),
    ("create index i on t (a)", DDL, NONE),
    ("create view v as select 1", DDL, NONE),
    ("alter table t add column c int", DDL, NONE),
    ("drop table t", DDL, DROPS),
    ("drop view if exists v", DDL, DROPS),
    ("explain delete from t where id = 1", DML, NONE),
    ("explain update t set a = 1", DML, NO_WHERE),
    // Transactions.
    ("begin", TX, NONE),
    ("commit", TX, NONE),
    ("rollback", TX, NONE),
    ("savepoint s", TX, NONE),
    ("release savepoint s", TX, NONE),
    // Not SQL at all.
    ("hello there", UNPARSED, NONE),
    ("'just a string'", UNPARSED, NONE),
];

#[test]
fn common_cases_on_every_engine() {
    for engine in [Engine::Postgres, Engine::Mysql, Engine::Sqlite] {
        check(engine, COMMON);
    }
}

#[test]
fn postgres() {
    check(
        Engine::Postgres,
        &[
            // Side-effect functions, however they are called.
            ("select pg_catalog.set_config('search_path', 'x', false)", FUNC, NONE),
            ("SELECT set_config('a', 'b', true)", FUNC, NONE),
            ("select \"nextval\"('s')", FUNC, NONE),
            ("select nextval /* c */ ('s')", FUNC, NONE),
            ("select nextval\n  ('s')", FUNC, NONE),
            ("select setval('s', 1)", FUNC, NONE),
            ("select pg_advisory_lock(1)", FUNC, NONE),
            ("select pg_try_advisory_xact_lock(1)", FUNC, NONE),
            ("select pg_terminate_backend(123)", FUNC, NONE),
            ("select pg_cancel_backend(123)", FUNC, NONE),
            ("select pg_reload_conf()", FUNC, NONE),
            ("select * from dblink('host=x', 'select 1') as t(a int)", FUNC, NONE),
            ("select dblink_exec('delete from t')", FUNC, NONE),
            ("select lo_import('/etc/passwd')", FUNC, NONE),
            ("select pg_file_write('x', 'y', false)", FUNC, NONE),
            ("select pg_notify('ch', 'x')", FUNC, NONE),
            ("select query_to_xml('delete from t returning *', true, false, '')", FUNC, NONE),
            ("select cursor_to_xml('c', 1, true, false, '')", FUNC, NONE),
            ("select txid_current()", FUNC, NONE),
            ("select pg_current_xact_id()", FUNC, NONE),
            ("select pg_read_file('/etc/passwd')", FUNC, NONE),
            ("select pg_stat_file('postgresql.conf')", FUNC, NONE),
            ("select pg_ls_dir('.')", FUNC, NONE),
            ("select * from pg_ls_logdir()", FUNC, NONE),
            ("select * from pg_ls_waldir()", FUNC, NONE),
            ("select pg_stat_statements_reset()", FUNC, NONE),
            ("select pg_stat_reset()", FUNC, NONE),
            ("select pg_sleep(10)", FUNC, NONE),
            ("select pg_sleep_for('1 minute')", FUNC, NONE),
            ("explain analyze select nextval('s')", FUNC, NONE),
            // Their harmless relatives.
            ("select currval('s')", R, NONE),
            ("select txid_current_if_assigned()", R, NONE),
            ("select now(), current_user, version()", R, NONE),
            ("select * from generate_series(1, 10)", R, NONE),
            ("select nextval from t", R, NONE),
            // `:=` names an argument in Postgres; it only writes in MySQL.
            ("select f(a := 1)", R, NONE),
            // Row locks. sqlparser 0.63 does not parse FOR NO KEY UPDATE or
            // FOR KEY SHARE; the token pass catches them.
            ("select * from t for share", LOCK, NONE),
            ("select * from t for update of t skip locked", LOCK, NONE),
            ("select * from t for no key update", LOCK, NONE),
            ("select * from t for key share", LOCK, NONE),
            ("select * from (select * from t for update) s", LOCK, NONE),
            ("with x as (select * from t for update) select * from x", LOCK, NONE),
            // SELECT INTO creates a table.
            ("select * into new_t from t", DDL, NONE),
            ("select * into temp new_t from t", DDL, NONE),
            // Data-modifying CTEs.
            ("with d as (delete from t returning *) select * from d", DML, NO_WHERE),
            ("with u as (update t set a = 1 where id = 1 returning *) select * from u", DML, NONE),
            ("with x as (select 1) insert into t select * from x", DML, NONE),
            // EXPLAIN is judged by what it explains.
            ("explain analyze update t set a = 1", DML, NO_WHERE),
            ("explain (analyze) delete from t where id = 1", DML, NONE),
            ("explain (analyze, format json) select 1", R, NONE),
            ("explain (costs off) insert into t values (1)", DML, NONE),
            // sqlparser only knows the American spelling; ANALYSE is read as
            // ANALYZE.
            ("explain analyse delete from t", DML, NO_WHERE),
            ("explain (analyse) delete from t where id = 1", DML, NONE),
            ("explain analyse select 1", R, NONE),
            // Invalid order, so it does not parse; the DELETE inside keeps it
            // from looking like a read (see the looks_like_read test).
            ("explain verbose analyse delete from t", UNPARSED, NONE),
            // Other reads.
            ("show search_path", R, NONE),
            ("show all", R, NONE),
            ("values (1, 'a'), (2, 'b')", R, NONE),
            ("select * from t where id = $1", R, NONE),
            // sqlparser 0.63 does not parse a bare TABLE statement: unparsed,
            // but it looks like a read.
            ("table t", UNPARSED, NONE),
            // parse_statements would stop quietly at END; the whole input
            // must be one statement.
            ("select 1 end", UNPARSED, NONE),
            // Data.
            ("insert into t values (1) on conflict do nothing", DML, NONE),
            ("insert into t values (1) on conflict (id) do update set a = excluded.a", DML, NONE),
            ("merge into t using s on t.id = s.id when matched then delete", DML, NONE),
            ("update t set a = 1 from u", DML, NO_WHERE),
            ("delete from t using u where t.id = u.id", DML, NONE),
            ("truncate t", DML, DROPS),
            ("truncate table t, u restart identity cascade", DML, DROPS),
            // Schema.
            ("create temp table t (id int)", DDL, NONE),
            ("create table t as select * from u", DDL, NONE),
            ("create materialized view mv as select 1", DDL, NONE),
            ("create extension dblink", DDL, NONE),
            ("alter table t drop column c", DDL, DROPS),
            ("drop schema s cascade", DDL, DROPS),
            ("drop function f(int)", DDL, DROPS),
            ("drop index i", DDL, DROPS),
            ("comment on table t is 'x'", DDL, NONE),
            // sqlparser 0.63 does not parse REFRESH MATERIALIZED VIEW.
            ("refresh materialized view mv", UNPARSED, NONE),
            // Privileges. sqlparser 0.63 rejects CREATE USER … WITH PASSWORD.
            ("grant select on t to reader", PRIV, NONE),
            ("revoke all on t from public", PRIV, NONE),
            ("create role r login", PRIV, NONE),
            ("alter role r superuser", PRIV, NONE),
            ("alter user u with password 'x'", PRIV, NONE),
            ("drop role r", PRIV, DROPS),
            ("drop user u", PRIV, DROPS),
            ("create user u with password 'x'", UNPARSED, NONE),
            // Procedures. sqlparser 0.63 does not parse DO at all.
            ("call p(1)", PROC, NONE),
            ("do $$ begin perform 1; end $$", PROC, NONE),
            ("do language plpgsql $body$ begin delete from t; end $body$", PROC, NONE),
            // Transactions.
            ("begin read only", TX, NONE),
            ("start transaction read write", TX, NONE),
            ("end", TX, NONE),
            ("abort", TX, NONE),
            ("rollback to savepoint s", TX, NONE),
            ("lock table t in access exclusive mode", TX, NONE),
            // Session state.
            ("set search_path to x", SESSION, NONE),
            ("set role admin", SESSION, NONE),
            ("set local statement_timeout = 0", SESSION, NONE),
            ("set session characteristics as transaction read write", SESSION, NONE),
            ("set transaction read write", SESSION, NONE),
            ("reset all", SESSION, NONE),
            ("discard all", SESSION, NONE),
            // Files.
            ("copy t to stdout", FILE, NONE),
            ("copy t from '/tmp/x'", FILE, NONE),
            ("copy (select 1) to program 'rm -rf /'", FILE, NONE),
            ("load 'plpgsql'", FILE, NONE),
            ("vacuum full t", FILE, NONE),
            // Cursors, prepared statements, notifications.
            ("declare c cursor for select 1", CURSOR, NONE),
            ("fetch next from c", CURSOR, NONE),
            ("move next from c", CURSOR, NONE),
            ("close c", CURSOR, NONE),
            ("prepare p as select 1", PREPARED, NONE),
            ("execute p", PREPARED, NONE),
            ("deallocate p", PREPARED, NONE),
            ("listen ch", REFUSED, NONE),
            ("notify ch, 'x'", REFUSED, NONE),
            ("unlisten *", REFUSED, NONE),
            // Maintenance.
            ("analyze t", OTHER, NONE),
            ("checkpoint", UNPARSED, NONE),
            // Where literals and comments end.
            ("select $$;drop table t$$", R, NONE),
            ("select $tag$ ; drop table t $tag$", R, NONE),
            ("select E'\\';drop table t'", R, NONE),
            ("select 1 /* nested /* ; */ still a comment ; */", R, NONE),
            ("select 1 --x; drop table t", R, NONE),
            // With standard_conforming_strings on (the default) the literal
            // ends at the second quote.
            ("select 'a\\'; drop table t; --'", MULTI, NONE),
            // With it off, `\'` escapes the quote and the literal ends
            // early: both readings are checked.
            ("select '\\''; drop table t; --'", MULTI, NONE),
            // The cost of checking both: a literal ending in a backslash
            // followed by one holding a `;` is refused.
            ("select 'C:\\', ';'", MULTI, NONE),
            ("select 'C:\\' as path", R, NONE),
            // Each reading is also parsed and the strictest one wins: with
            // standard_conforming_strings on this is one literal, with it off
            // the literal ends early and uncovers a DELETE in a CTE.
            (
                "with x as (select '\\''), d as (delete from t returning 1) select 1 as \"' as a) select * from x --\"",
                DML,
                NO_WHERE,
            ),
            // sqlparser rejects the escape; a `;` after that point starts a
            // statement, and alone it is unreadable.
            ("select E'\\uZZZZ'; delete from t", MULTI, NONE),
            ("select E'\\uZZZZ'", UNPARSED, NONE),
        ],
    );
}

#[test]
fn mysql() {
    check(
        Engine::Mysql,
        &[
            // Executable comments are code.
            ("select 1 /*!; drop table t */", MULTI, NONE),
            ("select 1 /*!50000 ; drop table t */", MULTI, NONE),
            ("select 1 /*M!; drop table t */", MULTI, NONE),
            ("select 1 /*M!100100 ; drop table t */", MULTI, NONE),
            ("select 1 /*!50000 , 2 */", R, NONE),
            ("select 1 /*! , get_lock('x', 1) */", FUNC, NONE),
            ("select 1 /*!50000 for update */", LOCK, NONE),
            ("select 1 /* plain; comment */", R, NONE),
            // `--` is a comment only before a space or control character.
            ("select 1 --x; drop table t", MULTI, NONE),
            ("select 1 -- x; drop table t", R, NONE),
            ("select 1 # x; drop table t", R, NONE),
            // sqlparser takes `--` followed by a no-break space for a comment;
            // MySQL does not.
            ("select 1 --\u{a0}; drop table t", MULTI, NONE),
            // Every combination of backslash escapes and ANSI_QUOTES is read;
            // each of these hides the `;` from all readings but one or two.
            ("select 'a\\';drop table t; -- '", MULTI, NONE),
            ("select \"a\\\"; drop table t; -- \"", MULTI, NONE),
            ("select \"\\\"\" ; drop table t; -- \"", MULTI, NONE),
            // ANSI_QUOTES with backslash escapes: `"\"` is an identifier,
            // `'\''` a quote.
            ("select \"\\\" , '\\'' ; drop table t; -- '\"", MULTI, NONE),
            ("select 'it\\'s'", R, NONE),
            // Under ANSI_QUOTES `"get_lock"` is a name, and this a call.
            ("select \"get_lock\"('x', 1)", FUNC, NONE),
            // Unterminated in the default reading: unreadable, and a `;`
            // after that point starts a statement.
            ("select 'abc\\'", UNPARSED, NONE),
            ("select 'unterminated; drop table t", MULTI, NONE),
            ("select 'a;b', \"c;d\"", R, NONE),
            ("select `delete`, `for` from t", R, NONE),
            // Files.
            ("select * from t into outfile '/tmp/x'", FILE, NONE),
            ("select * into outfile '/tmp/x' from t", FILE, NONE),
            ("select 1 into dumpfile '/tmp/x'", FILE, NONE),
            ("select 1 into /*c*/ outfile '/tmp/x'", FILE, NONE),
            ("load data infile '/tmp/x' into table t", FILE, NONE),
            ("load data local infile '/tmp/x' into table t", FILE, NONE),
            // Writing user variables is session state, however it is done.
            // sqlparser 0.63 does not parse INTO after FROM, and reads `:=`
            // as a plain expression; the token pass catches both.
            ("select 1 into @x", SESSION, NONE),
            ("select a, b into @a, @b from t", SESSION, NONE),
            ("select * from t into @x", SESSION, NONE),
            ("select @x := 1", SESSION, NONE),
            ("select * from t where (@a := 1)", SESSION, NONE),
            ("update t set a = (@x := @x + 1) where id = 1", SESSION, NONE),
            ("set @x := 1", SESSION, NONE),
            ("select @x, @@version", R, NONE),
            // Side-effect functions.
            ("select load_file('/etc/passwd')", FUNC, NONE),
            ("select get_lock('x', 10)", FUNC, NONE),
            ("select GET_LOCK('x', 10)", FUNC, NONE),
            ("select `get_lock`('x', 10)", FUNC, NONE),
            ("select release_lock('x')", FUNC, NONE),
            ("select release_all_locks()", FUNC, NONE),
            ("select benchmark(1000000, md5('x'))", FUNC, NONE),
            ("select sys_exec('id')", FUNC, NONE),
            ("select sys_eval('id')", FUNC, NONE),
            // Waits hold the session (and a pool slot) as long as they like.
            ("select sleep(1)", FUNC, NONE),
            ("select master_pos_wait('binlog.000001', 4)", FUNC, NONE),
            ("select source_pos_wait('binlog.000001', 4)", FUNC, NONE),
            ("select wait_for_executed_gtid_set('3e11fa47-71ca-11e1-9e33-c80aa9429562:1-5')", FUNC, NONE),
            // Row locks. sqlparser 0.63 does not parse LOCK IN SHARE MODE.
            ("select * from t lock in share mode", LOCK, NONE),
            ("select * from t for share nowait", LOCK, NONE),
            // Reads. sqlparser reads SHOW INDEX, SHOW GRANTS… as a SHOW of a
            // variable: still a read.
            ("show tables", R, NONE),
            ("show full tables from db like 'a%'", R, NONE),
            ("show databases", R, NONE),
            ("show columns from t", R, NONE),
            ("show create table t", R, NONE),
            ("show index from t", R, NONE),
            ("show grants", R, NONE),
            ("show variables like 'max%'", R, NONE),
            ("show processlist", R, NONE),
            ("describe t", R, NONE),
            ("desc t", R, NONE),
            ("explain t", R, NONE),
            ("explain analyze select * from t", R, NONE),
            ("explain format=json delete from t", DML, NO_WHERE),
            ("describe select 1", R, NONE),
            ("values row(1, 2), row(3, 4)", R, NONE),
            ("select 1 from dual", R, NONE),
            ("table t", UNPARSED, NONE),
            // Data.
            ("replace into t values (1)", DML, NONE),
            ("insert into t values (1) on duplicate key update a = 1", DML, NONE),
            ("insert ignore into t values (1)", DML, NONE),
            ("update t set a = 1 limit 1", DML, NO_WHERE),
            ("delete from t where a = 1 order by b limit 5", DML, NONE),
            ("truncate table t", DML, DROPS),
            // Schema.
            ("rename table a to b", DDL, NONE),
            ("create temporary table t (a int)", DDL, NONE),
            ("create table t (a int) /*!50100 engine=InnoDB */", DDL, NONE),
            ("drop temporary table t", DDL, DROPS),
            ("drop index i on t", DDL, DROPS),
            ("alter table t drop column c", DDL, DROPS),
            // Privileges. sqlparser 0.63 rejects 'user'@'host' outside GRANT.
            ("grant all on *.* to 'u'@'%'", PRIV, NONE),
            ("revoke all on *.* from 'u'@'%'", PRIV, NONE),
            ("create user 'u'@'%' identified by 'x'", UNPARSED, NONE),
            ("drop user 'u'@'%'", UNPARSED, DROPS),
            // Procedures. sqlparser 0.63 does not parse DO.
            ("call p()", PROC, NONE),
            ("do sleep(1)", PROC, NONE),
            // Transactions.
            ("start transaction", TX, NONE),
            ("begin work", TX, NONE),
            ("lock tables t read", TX, NONE),
            ("unlock tables", TX, NONE),
            ("xa start 'x'", TX, NONE),
            ("flush tables with read lock", TX, NONE),
            ("flush privileges", OTHER, NONE),
            // Session state.
            ("use db", SESSION, NONE),
            ("set names utf8mb4", SESSION, NONE),
            ("set @x = 1", SESSION, NONE),
            ("set session transaction read write", SESSION, NONE),
            ("set global read_only = 0", SESSION, NONE),
            ("set autocommit = 0", SESSION, NONE),
            // Prepared statements, handlers, KILL. sqlparser 0.63 only parses
            // Postgres' PREPARE … AS.
            ("prepare s from 'select 1'", PREPARED, NONE),
            ("execute s", PREPARED, NONE),
            ("deallocate prepare s", PREPARED, NONE),
            ("handler t open", CURSOR, NONE),
            ("kill 1", REFUSED, NONE),
            ("kill query 1", REFUSED, NONE),
            ("shutdown", REFUSED, NONE),
            // Maintenance.
            ("analyze table t", OTHER, NONE),
            ("optimize table t", UNPARSED, NONE),
        ],
    );
}

#[test]
fn sqlite() {
    check(
        Engine::Sqlite,
        &[
            // PRAGMA reads. sqlparser 0.63 only takes literal values, so
            // PRAGMA is read from its tokens.
            ("pragma table_info(t)", R, NONE),
            ("pragma table_info('t')", R, NONE),
            ("pragma table_info = t", R, NONE),
            ("pragma main.table_info(t)", R, NONE),
            ("PRAGMA table_xinfo(\"t\")", R, NONE),
            ("pragma index_list(t)", R, NONE),
            ("pragma index_info(i)", R, NONE),
            ("pragma foreign_key_list(t)", R, NONE),
            ("pragma foreign_key_check", R, NONE),
            ("pragma database_list", R, NONE),
            ("pragma table_list", R, NONE),
            ("pragma integrity_check", R, NONE),
            ("pragma user_version", R, NONE),
            ("PRAGMA page_count;", R, NONE),
            ("pragma journal_mode", R, NONE),
            ("pragma encoding", R, NONE),
            ("pragma compile_options", R, NONE),
            ("pragma \"schema_version\"", R, NONE),
            ("pragma foreign_keys", R, NONE),
            // PRAGMA writes, and anything not on the lists.
            ("pragma user_version = 5", OTHER, NONE),
            ("pragma main.user_version = 5", OTHER, NONE),
            ("pragma user_version(5)", OTHER, NONE),
            ("pragma journal_mode = wal", OTHER, NONE),
            ("pragma foreign_keys = off", OTHER, NONE),
            ("pragma optimize", OTHER, NONE),
            ("pragma wal_checkpoint(truncate)", OTHER, NONE),
            ("pragma writable_schema = 1", OTHER, NONE),
            ("pragma", OTHER, NONE),
            ("pragma table_info(t) extra", OTHER, NONE),
            ("pragma user_version; pragma user_version = 1", MULTI, NONE),
            // Table-valued pragmas are plain reads.
            ("select * from pragma_table_info('t')", R, NONE),
            // Extensions and files.
            ("select load_extension('/tmp/evil.so')", FUNC, NONE),
            ("select writefile('/tmp/x', 'y')", FUNC, NONE),
            ("attach database '/tmp/x.db' as x", FILE, NONE),
            ("attach '/tmp/x.db' as x", FILE, NONE),
            // sqlparser 0.63 does not parse DETACH or VACUUM INTO.
            ("detach database x", FILE, NONE),
            ("vacuum", FILE, NONE),
            ("vacuum into '/tmp/copy.db'", FILE, NONE),
            // Transactions.
            ("begin immediate", TX, NONE),
            ("begin exclusive transaction", TX, NONE),
            ("end transaction", TX, NONE),
            ("rollback to s", TX, NONE),
            ("release s", TX, NONE),
            // Data.
            ("replace into t values (1)", DML, NONE),
            ("insert or replace into t values (1)", DML, NONE),
            ("insert or ignore into t values (1)", DML, NONE),
            ("insert into t values (1) on conflict (a) do update set b = excluded.b", DML, NONE),
            ("insert into t default values", DML, NONE),
            ("delete from t where id = 1 returning *", DML, NONE),
            ("update t set a = 1 returning *", DML, NO_WHERE),
            ("with d as (delete from t returning *) select * from d", DML, NO_WHERE),
            // Schema.
            ("create temp table t (a)", DDL, NONE),
            ("create temporary view v as select 1", DDL, NONE),
            ("create virtual table f using fts5(body)", DDL, NONE),
            ("create table t as select 1", DDL, NONE),
            ("alter table t rename to u", DDL, NONE),
            ("alter table t drop column c", DDL, DROPS),
            ("drop table if exists t", DDL, DROPS),
            ("drop index i", DDL, DROPS),
            ("drop trigger tr", DDL, DROPS),
            // A trigger body holds `;`: it counts as more than one statement.
            ("create trigger tr after insert on t begin select 1; end", MULTI, NONE),
            // Maintenance. sqlparser 0.63 does not parse REINDEX.
            ("analyze", OTHER, NONE),
            ("reindex", UNPARSED, NONE),
            // Reads.
            ("explain query plan select * from t", R, NONE),
            ("explain delete from t", DML, NO_WHERE),
            ("select [order], `group` from t", R, NONE),
            ("select * from t limit 10 offset 5", R, NONE),
            ("select json_extract(a, '$.b') from t", R, NONE),
            ("select sqlite_version()", R, NONE),
            ("select * from t where a = ?1 or b = :name", R, NONE),
            ("values (1), (2)", R, NONE),
            // Comments: `--` needs no space in SQLite.
            ("select 1 --x; drop table t", R, NONE),
            ("select * from t where a = 'x' -- '; drop table t", R, NONE),
        ],
    );
}

#[test]
fn looks_like_read_lets_read_like_unparsed_statements_through() {
    let cases = [
        (Engine::Postgres, "table t", true),
        (Engine::Postgres, "select 1 end", true),
        (Engine::Mysql, "(table t)", true),
        (Engine::Mysql, "table t order by replace(a, 'x', 'y')", true),
        (Engine::Postgres, "refresh materialized view mv", false),
        (Engine::Postgres, "checkpoint", false),
        (Engine::Postgres, "create user u with password 'x'", false),
        (Engine::Mysql, "optimize table t", false),
        // EXPLAIN and WITH can wrap a write: a write keyword anywhere keeps
        // an unparsed statement from looking like a read.
        (Engine::Postgres, "explain verbose analyse delete from t", false),
        (Engine::Postgres, "explain verbose analyse update t set a = 1", false),
        (Engine::Mysql, "table t union select 1 from u into @x", false),
        // A literal that never ends, in some reading: nothing can be said
        // about it.
        (Engine::Postgres, "select 'unterminated", false),
        (Engine::Postgres, "select E'\\uZZZZ'", false),
        (Engine::Mysql, "select 'unterminated\\'", false),
        (Engine::Mysql, "table t where a = 'it\\'s'", false),
        // Token findings win over the first keyword.
        (Engine::Postgres, "select nextval('s') end", false),
        (Engine::Postgres, "select * from t for key share", false),
    ];
    for (engine, sql, expected) in cases {
        let got = classify(engine, sql);
        assert_eq!(got.looks_like_read, expected, "{engine:?} {sql:?}: {got:?}");
        assert!(got.kind != R, "{engine:?} {sql:?} parsed as a read: {got:?}");
    }
    assert_eq!(classify(Engine::Postgres, "select nextval('s') end").kind, FUNC);
    assert_eq!(classify(Engine::Postgres, "select 'unterminated").kind, UNPARSED);
}

#[test]
fn summaries_label_the_statement() {
    let cases = [
        (Engine::Postgres, "", "Empty statement"),
        (Engine::Postgres, "select 1; select 2", "Multiple statements"),
        (Engine::Postgres, "select 1", "SELECT"),
        (Engine::Postgres, "values (1)", "VALUES"),
        (Engine::Postgres, "delete from t", "DELETE"),
        (Engine::Postgres, "create table t (a int)", "CREATE TABLE"),
        (Engine::Postgres, "create temp table t (a int)", "CREATE TEMPORARY TABLE"),
        (Engine::Postgres, "drop role r", "DROP ROLE"),
        (Engine::Postgres, "explain analyze update t set a = 1", "EXPLAIN ANALYZE UPDATE"),
        (Engine::Postgres, "explain (analyze) delete from t", "EXPLAIN ANALYZE DELETE"),
        (Engine::Postgres, "explain analyse delete from t", "EXPLAIN ANALYZE DELETE"),
        (Engine::Postgres, "explain (analyse) select 1", "EXPLAIN ANALYZE SELECT"),
        (Engine::Postgres, "select 'unterminated", "Unreadable SQL"),
        (Engine::Mysql, "select 'abc\\'", "Unreadable SQL"),
        (Engine::Mysql, "select @x := 1", "SELECT with := assignment"),
        (Engine::Mysql, "select * from t into @x", "SELECT INTO @variable"),
        (Engine::Mysql, "select 1 into @x", "SELECT INTO"),
        (Engine::Postgres, "with d as (delete from t returning *) select * from d", "SELECT with DELETE"),
        (Engine::Postgres, "with x as (select 1) insert into t select * from x", "INSERT"),
        (Engine::Postgres, "select pg_catalog.set_config('a', 'b', false)", "SELECT calling set_config()"),
        (Engine::Postgres, "select * from t for share", "SELECT FOR SHARE"),
        (Engine::Postgres, "select * from t for no key update", "SELECT FOR NO KEY UPDATE"),
        (Engine::Postgres, "set role admin", "SET ROLE"),
        (Engine::Postgres, "do $$ begin end $$", "DO"),
        (Engine::Postgres, "show search_path", "SHOW"),
        (Engine::Mysql, "show tables", "SHOW TABLES"),
        (Engine::Mysql, "describe t", "DESCRIBE"),
        (Engine::Mysql, "replace into t values (1)", "REPLACE"),
        (Engine::Mysql, "select 1 into dumpfile '/tmp/x'", "SELECT INTO DUMPFILE"),
        (Engine::Mysql, "drop user 'u'@'%'", "DROP"),
        (Engine::Sqlite, "pragma main.table_info(t)", "PRAGMA table_info"),
        (Engine::Sqlite, "PRAGMA User_Version = 1", "PRAGMA user_version"),
        (Engine::Sqlite, "insert or replace into t values (1)", "INSERT"),
    ];
    for (engine, sql, expected) in cases {
        assert_eq!(classify(engine, sql).summary, expected, "{engine:?} {sql:?}");
    }
}

#[test]
fn serializes_flat_in_camel_case() {
    let write = classify(Engine::Postgres, "delete from t");
    let json = serde_json::to_value(&write).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "kind": "write",
            "detail": "dml",
            "summary": "DELETE",
            "warnings": ["noWhereClause"],
            "looksLikeRead": false,
        })
    );
    assert_eq!(serde_json::from_value::<Classification>(json).unwrap(), write);

    let read = classify(Engine::Postgres, "select 1");
    assert_eq!(
        serde_json::to_value(&read).unwrap(),
        serde_json::json!({ "kind": "read", "summary": "SELECT", "warnings": [], "looksLikeRead": true })
    );

    let multi = classify(Engine::Postgres, "select 1; select 2");
    assert_eq!(serde_json::to_value(&multi).unwrap()["detail"], "multipleStatements");
}

/// Runs `f` on a thread with Tokio's default worker stack (2 MiB).
fn on_small_stack(f: impl FnOnce() -> Classification + Send + 'static) -> Classification {
    std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(f).unwrap().join().unwrap()
}

#[test]
fn deep_statements_do_not_overflow_the_stack() {
    // Left-deep trees that sqlparser builds without its recursion limit:
    // long ones are classified from their tokens only.
    let long = on_small_stack(|| classify(Engine::Postgres, &format!("select 1{}", " + 1".repeat(200_000))));
    assert_eq!((long.kind, long.looks_like_read), (UNPARSED, true));
    let unions = on_small_stack(|| classify(Engine::Mysql, &format!("select 1{}", " union select 1".repeat(50_000))));
    assert_eq!((unions.kind, unions.looks_like_read), (UNPARSED, true));

    // Just under the limit, parsed and dropped on the same small stack.
    let parsed = on_small_stack(|| classify(Engine::Postgres, &format!("select 1{}", " + 1".repeat(4_999))));
    assert_eq!(parsed.kind, R);

    // Nesting hits sqlparser's recursion limit instead.
    let nested =
        on_small_stack(|| classify(Engine::Sqlite, &format!("select {}1{}", "(".repeat(5_000), ")".repeat(5_000))));
    assert_eq!((nested.kind, nested.looks_like_read), (UNPARSED, true));
}
