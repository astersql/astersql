// Copyright 2026 AsterSQL.

use crate::runtime::CreateAnalyzeSession;

#[test]
fn session_read_staleness_honors_mock_stale_read_tso() {
    let (_, session) = CreateAnalyzeSession().expect("create canonical session");
    let snapshot_ts = (1_700_000_000_123_u64 << 18) + 17;
    let now_ts = snapshot_ts + (1_000_u64 << 18);
    let guard = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/sessiontxn/staleread/mockStaleReadTSO",
        &format!("1*return({now_ts})"),
    );

    session
        .execute("set @@tidb_read_staleness='-1'")
        .expect("enable one-second session staleness");
    session
        .execute("select 1")
        .expect("refresh session staleness from the injected TSO");

    assert_eq!(
        session.StaleReadStateForTest().session_read_ts,
        Some(snapshot_ts)
    );
    assert!(
        astersql_testkit_testfailpoint::eval_string(
            "github.com/pingcap/tidb/pkg/sessiontxn/staleread/mockStaleReadTSO"
        )
        .is_none(),
        "the statement must consume the injected stale-read TSO"
    );
    drop(guard);
}

#[test]
fn fixed_time_zone_range_matches_go_parse_time_zone() {
    let (_, session) = CreateAnalyzeSession().expect("create canonical session");

    for value in ["-12:59", "+14:00"] {
        session
            .execute(&format!("SET time_zone = '{value}'"))
            .unwrap_or_else(|error| panic!("Go accepts boundary time zone {value}: {error}"));
    }

    for value in ["-13:00", "-14:00", "+14:01", "--01:00", "+-01:00"] {
        let error = match session.execute(&format!("SET time_zone = '{value}'")) {
            Ok(_) => panic!("Go rejects fixed time zone {value}"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("Unknown or incorrect time zone"),
            "unexpected error for {value}: {error}"
        );
    }
}

/// The Go test injects only the RPC boundary; SQL still creates and reads real tables.
#[test]
fn terminal_auto_id_rpc_error_aborts_all_insert_ignore_paths() {
    use astersql_meta_autoid as autoid;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Discovery;
    impl autoid::LeaderDiscovery for Discovery {
        fn leader(&self, _: &autoid::Context, _: &str) -> autoid::Result<Option<String>> {
            panic!("seeded client must be used")
        }
    }
    impl autoid::AutoIdClientConnector for Discovery {
        fn connect(
            &self,
            _: &str,
        ) -> autoid::Result<(
            Arc<dyn autoid::AutoIdClient>,
            Arc<dyn autoid::ClientConnection>,
        )> {
            panic!("terminal error must not reconnect")
        }
    }
    #[derive(Default)]
    struct Client {
        alloc: AtomicUsize,
        rebase: AtomicUsize,
    }
    const MESSAGE: &str = "autoid alloc failed after reaching the RPC retry limit";
    impl autoid::AutoIdClient for Client {
        fn alloc_auto_id(
            &self,
            _: &autoid::Context,
            request: autoid::AutoIdRequest,
        ) -> autoid::Result<autoid::AutoIdResponse> {
            assert!(request.n > 0);
            self.alloc.fetch_add(1, Ordering::SeqCst);
            Err(autoid::AutoIdError::RpcRetryLimit(MESSAGE.into()))
        }
        fn rebase(
            &self,
            _: &autoid::Context,
            request: autoid::RebaseRequest,
        ) -> autoid::Result<autoid::RebaseResponse> {
            assert_eq!(request.base, 100);
            assert!(!request.force);
            self.rebase.fetch_add(1, Ordering::SeqCst);
            Err(autoid::AutoIdError::RpcRetryLimit(MESSAGE.into()))
        }
    }
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let cases = [
        "insert into %s (v) values (1)",
        "insert ignore into %s (v) values (1)",
        "insert ignore into %s (v) values (1), (2)",
        "insert ignore into %s (v) select 1",
        "insert ignore into %s (id, v) values (100, 1)",
    ];
    for (index, sql) in cases.iter().enumerate() {
        let name = format!("terminal_autoid_{index}");
        session
            .execute(&format!(
                "create table {name} (id int key auto_increment, v int) auto_id_cache 1"
            ))
            .unwrap();
        let table = domain.table_by_name("test", &name).unwrap();
        let client = Arc::new(Client::default());
        let discover = Arc::new(autoid::ClientDiscover::new(
            Arc::new(Discovery),
            Arc::new(Discovery),
        ));
        discover.seed_client_for_test(client.clone());
        domain.install_single_point_auto_id_allocator(
            table.ID,
            Arc::new(autoid::SinglePointAllocator::new(
                table.DBID,
                table.ID,
                false,
                autoid::NULLSPACE_ID,
                discover,
            )),
        );
        let error = session
            .execute(&sql.replace("%s", &name))
            .err()
            .expect("terminal error must abort INSERT IGNORE");
        assert!(error.to_string().contains(MESSAGE), "{error}");
        let mut cause: &(dyn std::error::Error + 'static) = &error;
        let mut marked = false;
        loop {
            if let Some(error) = cause.downcast_ref::<autoid::AutoIdError>() {
                marked |= autoid::is_rpc_retry_limit_error(error);
            }
            match cause.source() {
                Some(next) => cause = next,
                None => break,
            }
        }
        assert!(
            marked,
            "terminal AutoID identity lost in SQL error chain: {error}"
        );
        assert_eq!(client.alloc.load(Ordering::SeqCst), usize::from(index != 4));
        assert_eq!(
            client.rebase.load(Ordering::SeqCst),
            usize::from(index == 4)
        );
        let mut sets = session.execute(&format!("select id from {name}")).unwrap();
        assert_eq!(
            sets[0].next_row().unwrap(),
            None,
            "no row may be inserted without an ID"
        );
        sets[0].close().unwrap();
    }
}

#[test]
fn single_point_auto_id_factory_preserves_successful_insert_and_rebase() {
    let (_, session) = CreateAnalyzeSession().unwrap();
    session
        .execute(
            "create table rpc_autoid_success (id int key auto_increment, v int) auto_id_cache 1",
        )
        .unwrap();
    session
        .execute("insert into rpc_autoid_success (v) values (1)")
        .unwrap();
    session
        .execute("insert into rpc_autoid_success (id, v) values (100, 2)")
        .unwrap();
    session
        .execute("insert into rpc_autoid_success (v) values (3)")
        .unwrap();
    let mut sets = session
        .execute("select id from rpc_autoid_success order by id")
        .unwrap();
    for id in ["1", "100", "101"] {
        assert_eq!(sets[0].next_row().unwrap(), Some(vec![id.into()]));
    }
    assert_eq!(sets[0].next_row().unwrap(), None);
    sets[0].close().unwrap();
}

#[test]
fn pg_core_dialect_session_lifecycle() {
    use crate::runtime::{ConcreteSession, PARSED_DIALECTS};
    use astersql_parser_ast::SqlDialect;
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), SqlDialect::PostgreSQL);
    for (session, dialect) in [(&pg, SqlDialect::PostgreSQL), (&mysql, SqlDialect::MySQL)] {
        for sql in [
            "SELECT 73",
            "BEGIN",
            "SELECT 79",
            "ROLLBACK",
            "SET sql_mode = ''",
            "SELECT 83",
        ] {
            PARSED_DIALECTS.with(|seen| seen.borrow_mut().clear());
            let mut sets = session.execute(sql).unwrap();
            if sql.starts_with("SELECT") {
                assert_eq!(
                    sets[0].next_row().unwrap().unwrap(),
                    vec![sql[7..].to_owned()]
                );
            }
            assert_eq!(session.sql_dialect(), dialect);
            PARSED_DIALECTS.with(|seen| assert_eq!(*seen.borrow(), vec![dialect], "{sql}"));
        }
        PARSED_DIALECTS.with(|seen| seen.borrow_mut().clear());
        assert!(session.execute("SELECT FROM").is_err());
        PARSED_DIALECTS.with(|seen| assert_eq!(*seen.borrow(), vec![dialect]));
        assert_eq!(
            session.execute("SELECT 89").unwrap()[0]
                .next_row()
                .unwrap()
                .unwrap(),
            vec!["89"]
        );
    }
    drop(pg);
    let reconnected = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    assert_eq!(reconnected.sql_dialect(), SqlDialect::PostgreSQL);
    assert_eq!(
        reconnected.execute("SELECT 97").unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        vec!["97"]
    );
}

#[test]
fn pg_core_integer_cast_syntax_split_boundary() {
    use crate::runtime::{SplitSQLStatements, SplitSQLStatementsWithDialect};
    use astersql_parser_ast::SqlDialect;
    let sql = "SELECT ($12)::int4; SELECT CAST(1 AS bigint)";
    assert_eq!(
        SplitSQLStatementsWithDialect(sql, SqlDialect::PostgreSQL).unwrap(),
        vec!["SELECT ($12)::int4", "SELECT CAST(1 AS bigint)"]
    );
    assert!(SplitSQLStatements(sql).is_err());
    assert!(SplitSQLStatementsWithDialect("SELECT 1::", SqlDialect::PostgreSQL).is_err());
}
