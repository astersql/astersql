// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::mview_refresh::RefreshMaterializedViewExec;
use super::normal_ddl_fixture::Fixture;
use super::*;
use astersql_executor::mview_refresh_steps::{ObserveStep, StepObserver};
use std::time::{Duration, SystemTime};

fn query(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut sets = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
    let Some(mut set) = sets.pop() else {
        return vec![];
    };
    let mut rows = Vec::new();
    while let Some(row) = set.next_row().unwrap() {
        rows.push(row);
    }
    set.close().unwrap();
    rows
}
fn save_meta(f: &Fixture, mut table: astersql_meta_model::TableInfo) {
    let mut transaction = f
        .domain
        .storage()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    let mut metadata = astersql_meta::TransactionMutator::new(transaction.as_mut());
    metadata.update_table(f.db, &mut table).unwrap();
    let version = metadata.gen_schema_version().unwrap();
    metadata
        .set_table_schema_diff(
            &astersql_meta_model::group_3::Job {
                schema_id: f.db,
                table_id: table.ID,
                ..Default::default()
            },
            version,
        )
        .unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();
    f.domain.force_full_reload_for_test().unwrap();
}
fn fixture() -> (Fixture, ConcreteSession) {
    fixture_definition(
        "CREATE TABLE src_runtime (id int primary key,g int not null,v int not null)",
        "(1,1,10),(2,1,20),(3,2,30)",
        "CREATE TABLE mv_runtime (g int not null,s decimal(65,0) not null,cnt bigint not null)",
        "SELECT g,SUM(v),COUNT(*) FROM test.src_runtime GROUP BY g",
    )
}
fn fixture_definition(
    base_ddl: &str,
    initial_rows: &str,
    mv_ddl: &str,
    definition: &str,
) -> (Fixture, ConcreteSession) {
    let f = Fixture::new();
    BootstrapCanonicalDomain(f.domain.clone()).unwrap();
    f.domain.set_stats_updating(true);
    let session = ConcreteSession::new(f.domain.clone());
    query(&session, "USE test");
    query(&session, "SET tidb_mview_enable=ON");
    query(&session, base_ddl);
    query(
        &session,
        &format!("INSERT INTO src_runtime VALUES {initial_rows}"),
    );
    query(
        &session,
        "CREATE MATERIALIZED VIEW LOG ON src_runtime (id,g,v)",
    );
    query(&session, mv_ddl);
    query(&session, &format!("INSERT INTO mv_runtime {definition}"));
    let mut view = (*f.domain.table_by_name("test", "mv_runtime").unwrap()).clone();
    // CREATE MATERIALIZED VIEW derives writable types from its defining SELECT.
    // Preserve that contract in this metadata fixture instead of inventing SUM precision.
    let select = astersql_parser::New()
        .ParseOneStmt(definition, "", "")
        .unwrap()
        .into_any()
        .downcast::<ast::SelectStmt>()
        .unwrap();
    let version = f
        .domain
        .storage()
        .with_storage(|store| store.CurrentVersion("global"))
        .unwrap()
        .Ver;
    let source = session.compile_mview_source(*select, version).unwrap();
    assert_eq!(view.Columns.len(), source.Schema().len());
    for (column, output) in view.Columns.iter_mut().zip(source.Schema()) {
        column.FieldType = output.field_type.clone();
    }

    let mut base = (*f.domain.table_by_name("test", "src_runtime").unwrap()).clone();
    view.MaterializedView = Some(astersql_meta_model::MaterializedViewInfo {
        BaseTableIDs: vec![base.ID],
        SQLContent: definition.into(),
        DefinitionDivPrecisionIncrement: 9,
        DefinitionTimeZone: astersql_meta_model::TimeZoneLocation {
            name: "UTC".into(),
            ..Default::default()
        },
        ..Default::default()
    });
    base.MaterializedViewBase
        .as_mut()
        .unwrap()
        .MViewIDs
        .push(view.ID);
    let view_id = view.ID;
    let mut log = (*f
        .domain
        .info_schema()
        .TableByID(base.MaterializedViewBase.as_ref().unwrap().MLogID)
        .unwrap()
        .ModelMeta()
        .unwrap())
    .clone();
    log.MaterializedViewLog
        .as_mut()
        .unwrap()
        .DependentMViewIDs
        .push(view_id);
    save_meta(&f, log);
    save_meta(&f, view);
    save_meta(&f, base);
    let read = f
        .domain
        .storage()
        .with_storage(|store| store.CurrentVersion("global"))
        .unwrap()
        .Ver;
    query(
        &session,
        &format!(
            "INSERT INTO mysql.tidb_mview_refresh_info (MVIEW_ID,LAST_SUCCESS_READ_TSO) VALUES ({view_id},{read})"
        ),
    );
    (f, session)
}
#[derive(Default)]
struct Observer {
    started: Vec<String>,
    ended: Vec<String>,
    planned: Vec<String>,
    panic_at: Option<&'static str>,
}
impl StepObserver for Observer {
    fn on_step_start(&mut self, step: &ObserveStep, _: SystemTime) {
        self.started.push(step.name.into());
    }
    fn on_step_end(&mut self, step: &ObserveStep, _: SystemTime, _: Option<&str>) {
        self.ended.push(step.name.into());
        if self.panic_at == Some(step.name) {
            panic!("original refresh panic");
        }
    }
    fn on_step_plan_rows(&mut self, step: &ObserveStep, rows: Vec<Vec<String>>) {
        assert_eq!(self.started.last().map(String::as_str), Some(step.name));
        assert!(!self.ended.iter().any(|name| name == step.name));
        assert!(!rows.is_empty());
        self.planned.push(step.name.into());
    }
}
fn run(
    session: &ConcreteSession,
    mode: &str,
    observer: &mut dyn StepObserver,
) -> (RefreshMaterializedViewExec, SessionResult<()>) {
    let statement = astersql_parser::New()
        .ParseOneStmt(
            &format!("REFRESH MATERIALIZED VIEW test.mv_runtime {mode}"),
            "",
            "",
        )
        .unwrap();
    let statement = statement
        .as_any()
        .downcast_ref::<ast::RefreshMaterializedViewStmt>()
        .unwrap()
        .clone();
    let mut executor = RefreshMaterializedViewExec::new(session.clone(), statement, false);
    let result = executor.next(Some(&mut *observer));
    // Utility Next is idempotent, including after a failed attempt.
    executor.next(Some(&mut *observer)).unwrap();
    (executor, result)
}
#[test]
fn port_a87bf8db64_runtime_real_rows_history_rollback_and_retry() {
    for mode in [
        "fast",
        "complete in place",
        "complete delta apply",
        "complete out of place",
    ] {
        let (f, session) = fixture();
        if mode == "complete out of place" {
            EnableMockDDLForTest(&f.domain).unwrap();
        }
        let before = f.domain.table_by_name("test", "mv_runtime").unwrap().ID;
        let old = query(
            &session,
            "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info",
        )[0][0]
            .parse::<u64>()
            .unwrap();
        query(&session, "DELETE FROM src_runtime WHERE id=2");
        query(&session, "UPDATE src_runtime SET g=3,v=5 WHERE id=3");
        query(&session, "INSERT INTO src_runtime VALUES (4,4,7)");
        let mut observer = Observer::default();
        let (_, result) = run(&session, mode, &mut observer);
        result.unwrap_or_else(|error| panic!("{mode}: {error}"));
        if matches!(mode, "fast" | "complete delta apply") {
            assert_eq!(observer.planned.len(), 1);
        }
        let sink_result = (
            session.session_vars.StmtCtx.AffectedRows(),
            session.session_vars.StmtCtx.GetMessage(),
        );
        assert_eq!(
            query(&session, "SELECT * FROM mv_runtime ORDER BY g"),
            vec![
                vec!["1", "10", "1"],
                vec!["3", "5", "1"],
                vec!["4", "7", "1"]
            ],
            "{mode}; sink={sink_result:?}; initial watermark={old}; history={:?}",
            query(
                &session,
                "SELECT REFRESH_ROWS,REFRESH_READ_TSO FROM mysql.tidb_mview_refresh_hist"
            )
        );
        assert_eq!(
            query(
                &session,
                "SELECT REFRESH_STATUS,REFRESH_METHOD FROM mysql.tidb_mview_refresh_hist"
            ),
            vec![vec!["success".to_owned(), format!("{mode} manual")]]
        );
        let new = query(
            &session,
            "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info",
        )[0][0]
            .parse::<u64>()
            .unwrap();
        assert!(new > old);
        assert_eq!(observer.started, observer.ended);
        assert!(observer.ended.contains(&"FINALIZE_HIST".into()));
        let after = f.domain.table_by_name("test", "mv_runtime").unwrap().ID;
        assert_eq!(before == after, mode != "complete out of place");
        query(&session, "INSERT INTO src_runtime VALUES (5,8,9)");
        let point = if mode == "complete out of place" {
            "mockRefreshMaterializedViewOutOfPlaceBuildErrorAfterLoadShadow"
        } else {
            "mockRefreshMaterializedViewErrorAfterDataChanges"
        };
        let guard = astersql_testkit_testfailpoint::enable(
            &format!("github.com/pingcap/tidb/pkg/executor/{point}"),
            "return(\"runtime failure\")",
        );
        let (_, result) = run(&session, mode, &mut Observer::default());
        assert!(result.unwrap_err().to_string().contains("runtime failure"));
        assert_eq!(
            query(&session, "SELECT * FROM mv_runtime ORDER BY g"),
            vec![
                vec!["1", "10", "1"],
                vec!["3", "5", "1"],
                vec!["4", "7", "1"]
            ]
        );
        assert_eq!(
            query(
                &session,
                "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info"
            ),
            vec![vec![new.to_string()]]
        );
        assert_eq!(
            query(
                &session,
                "SELECT REFRESH_STATUS FROM mysql.tidb_mview_refresh_hist ORDER BY REFRESH_JOB_ID"
            ),
            vec![vec!["success"], vec!["failed"]]
        );
        assert_eq!(
            query(
                &session,
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='test' AND table_name LIKE '__mv_shadow_%'"
            ),
            vec![vec!["0"]]
        );
        let lock_session = ConcreteSession::new(f.domain.clone());
        let lock_name = format!("mv_refresh_{}_{}", f.db, after);
        assert_eq!(
            query(&lock_session, &format!("SELECT GET_LOCK('{lock_name}',0)")),
            vec![vec!["1"]]
        );
        assert_eq!(
            query(
                &lock_session,
                &format!("SELECT RELEASE_LOCK('{lock_name}')")
            ),
            vec![vec!["1"]]
        );
        drop(guard);
        let (_, result) = run(&session, mode, &mut Observer::default());
        result.unwrap();
        assert_eq!(
            query(&session, "SELECT * FROM mv_runtime ORDER BY g").len(),
            4
        );
        assert_eq!(
            query(
                &session,
                "SELECT REFRESH_STATUS FROM mysql.tidb_mview_refresh_hist ORDER BY REFRESH_JOB_ID"
            ),
            vec![vec!["success"], vec!["failed"], vec!["success"]]
        );
    }
}

#[test]
fn port_a87bf8db64_internal_collectors_preserve_nested_ownership() {
    use super::mview_refresh::InternalCollectorGuard;
    let (_, session) = fixture();
    assert!(session.internal_collectors.borrow().stats.is_none());
    let outer = InternalCollectorGuard::attach(&session);
    let stats = session.internal_collectors.borrow().stats.clone().unwrap();
    let indexes = session
        .internal_collectors
        .borrow()
        .indexes
        .clone()
        .unwrap();
    {
        let _inner = InternalCollectorGuard::attach(&session);
        assert!(Arc::ptr_eq(
            &stats,
            session.internal_collectors.borrow().stats.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            &indexes,
            session
                .internal_collectors
                .borrow()
                .indexes
                .as_ref()
                .unwrap()
        ));
    }
    assert!(Arc::ptr_eq(
        &stats,
        session.internal_collectors.borrow().stats.as_ref().unwrap()
    ));
    let table = session
        .domain
        .table_by_name("test", "mv_runtime")
        .unwrap()
        .ID;
    indexes.update(
        table,
        1,
        astersql_statistics_handle_usage::IndexUsageSample {
            query_total: 1,
            row_access_total: 2,
            ..Default::default()
        },
    );
    drop(outer);
    assert!(session.internal_collectors.borrow().stats.is_none());
    assert!(session.internal_collectors.borrow().indexes.is_none());
    assert_eq!(
        session
            .domain
            .internal_index_usage()
            .sample(table, 1)
            .row_access_total,
        2
    );
}

#[test]
fn port_a87bf8db64_initialization_restores_definition_and_execution_vars() {
    let (_, caller) = fixture();
    query(&caller, "SET tidb_mview_maintain_mem_quota=4096");
    query(
        &caller,
        "SET tidb_mview_maintain_isolation_read_engines='tikv'",
    );
    query(&caller, "SET tidb_mview_maintain_import_threads=3");
    query(&caller, "SET tidb_mview_maintain_import_disk_quota='2GiB'");
    let internal = ConcreteSession::new(caller.domain.clone());
    query(&internal, "SET sql_mode='STRICT_TRANS_TABLES'");
    query(&internal, "SET time_zone='+00:00'");
    query(&internal, "SET div_precision_increment=4");
    let info = caller
        .domain
        .table_by_name("test", "mv_runtime")
        .unwrap()
        .MaterializedView
        .clone()
        .unwrap();
    let names = [
        "sql_mode",
        "time_zone",
        "div_precision_increment",
        "tidb_mview_enable",
        "tidb_mem_quota_query",
        "tidb_isolation_read_engines",
        "tidb_max_tiflash_threads",
        "tiflash_query_spill_ratio",
        "tiflash_fine_grained_shuffle_batch_size",
    ];
    let inspect = format!(
        "SELECT {}",
        names
            .iter()
            .map(|name| format!("@@session.{name}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let before = query(&internal, &inspect);
    {
        let _guard =
            super::mview_refresh::initialize_session(&internal, &info, &caller, false).unwrap();
        assert_eq!(
            internal
                .session_vars
                .GetSystemVar("div_precision_increment")
                .as_deref(),
            Some("9")
        );
        assert_eq!(internal.state.borrow().sql_mode, "");
        for (name, expected) in [
            ("tidb_mem_quota_query", "4096"),
            ("tidb_isolation_read_engines", "tikv"),
            ("tidb_mview_maintain_import_threads", "3"),
            ("tidb_mview_maintain_import_disk_quota", "2GiB"),
        ] {
            assert_eq!(
                internal.session_vars.GetSystemVar(name).as_deref(),
                Some(expected),
                "{name}"
            );
        }
    }
    assert_eq!(query(&internal, &inspect), before);
    assert_eq!(internal.state.borrow().sql_mode, "STRICT_TRANS_TABLES");
}

#[test]
fn port_a87bf8db64_slow_overhead_logs_preserve_mode_fields() {
    use super::mview_refresh::RefreshTimings;
    let table = astersql_meta_model::TableInfo {
        ID: 17,
        Name: ast::NewCIStr("overhead_test"),
        ..Default::default()
    };
    for out_of_place in [false, true] {
        for success in [false, true] {
            for (total, execution, logged) in [
                (3, 1, false),
                (6, 2, false),
                (6, 1, true),
                (9, 2, true),
                (1, 9, false),
                (5, 0, true),
            ] {
                let timings = RefreshTimings {
                    total: Duration::from_secs(total),
                    execution: Duration::from_secs(execution),
                    out_of_place,
                    success,
                    ..Default::default()
                };
                let before = BgLogger().entries().len();
                timings.emit(&table, "test");
                let logs = BgLogger().entries();
                assert_eq!(logs.len() - before, usize::from(logged));
                if logged {
                    let log = logs.last().unwrap();
                    assert!(
                        log.fields
                            .contains(&LogField::Bool("success".into(), success))
                    );
                    assert!(log.fields.contains(&LogField::U64(
                        "overhead".into(),
                        total.saturating_sub(execution) * 1_000_000_000
                    )));
                    let keys = log
                        .fields
                        .iter()
                        .map(|field| match field {
                            LogField::U64(name, _) => name.as_str(),
                            _ => "",
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(keys.contains(&"loadShadow"), out_of_place);
                    assert_eq!(keys.contains(&"cutover"), out_of_place);
                    assert_eq!(keys.contains(&"lockRefreshInfoRow"), !out_of_place);
                    assert_eq!(keys.contains(&"transactionTotal"), !out_of_place);
                }
            }
        }
    }
}

#[test]
fn port_a87bf8db64_out_of_place_panic_cleanup_preserves_original_failure() {
    for (panic_step, cleanup_fails, fail_after_load, committed) in [
        (Some("OUT_OF_PLACE_CREATE_SHADOW"), false, false, false),
        (Some("OUT_OF_PLACE_LOAD_SHADOW"), false, false, false),
        (Some("OUT_OF_PLACE_LOAD_SHADOW"), true, false, false),
        (Some("OUT_OF_PLACE_CUTOVER"), false, false, true),
        (None, true, true, false),
        (None, false, false, true),
    ] {
        let (f, session) = fixture();
        EnableMockDDLForTest(&f.domain).unwrap();
        let before_id = f.domain.table_by_name("test", "mv_runtime").unwrap().ID;
        let before_rows = query(&session, "SELECT * FROM mv_runtime ORDER BY g");
        query(&session, "INSERT INTO src_runtime VALUES (4,1,5)");
        let cleanup = cleanup_fails.then(|| astersql_testkit_testfailpoint::enable("github.com/pingcap/tidb/pkg/executor/mockRefreshMaterializedViewShadowCleanupError","return(\"shadow cleanup failure\")"));
        let failure = fail_after_load.then(|| astersql_testkit_testfailpoint::enable("github.com/pingcap/tidb/pkg/executor/mockRefreshMaterializedViewOutOfPlaceBuildErrorAfterLoadShadow","return(\"runtime failure\")"));
        let log_start = BgLogger().entries().len();
        let (executor, result) = run(
            &session,
            "complete out of place",
            &mut Observer {
                panic_at: panic_step,
                ..Default::default()
            },
        );
        let result_text = result.as_ref().err().map(ToString::to_string);
        if panic_step.is_some() {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("original refresh panic"),
                "panic step={panic_step:?}; result={result_text:?}"
            );
        } else if fail_after_load {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("runtime failure"));
            assert!(error.contains("shadow cleanup failure"));
        } else {
            result.unwrap();
        }
        assert_eq!(
            before_id != f.domain.table_by_name("test", "mv_runtime").unwrap().ID,
            committed
        );
        let after_rows = query(&session, "SELECT * FROM mv_runtime ORDER BY g");
        if committed {
            assert_eq!(after_rows[0], vec!["1", "35", "3"]);
        } else {
            assert_eq!(before_rows, after_rows);
        }
        assert_eq!(executor.timings.cleanup_shadow > Duration::ZERO, !committed);
        assert!(executor.timings.create_shadow > Duration::ZERO);
        if panic_step != Some("OUT_OF_PLACE_CREATE_SHADOW") {
            assert!(executor.timings.execution > Duration::ZERO);
        }
        if committed {
            assert!(executor.timings.cutover > Duration::ZERO);
        }
        assert_eq!(executor.timings.transaction_total, Duration::ZERO);
        let logs = BgLogger().entries();
        let cleanup_logs = logs[log_start..]
            .iter()
            .filter(|log| {
                log.message
                    == "failed to cleanup shadow table after out-of-place complete refresh error"
            })
            .collect::<Vec<_>>();
        assert_eq!(
            cleanup_logs.len(),
            usize::from(cleanup_fails),
            "{cleanup_logs:?}; result={result_text:?}"
        );
        assert_eq!(
            query(
                &session,
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='test' AND table_name LIKE '__mv_shadow_%'"
            ),
            vec![vec![if cleanup_fails { "1" } else { "0" }]]
        );
        drop(failure);
        drop(cleanup);
        if cleanup_fails {
            let shadow = cleanup_logs[0]
                .fields
                .iter()
                .find_map(|field| match field {
                    LogField::String(name, value) if name == "shadowTable" => Some(value.clone()),
                    _ => None,
                })
                .unwrap();
            super::mview_refresh::drop_shadow_table(&session, "test", &shadow).unwrap();
        }
        assert_eq!(
            query(
                &session,
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='test' AND table_name LIKE '__mv_shadow_%'"
            ),
            vec![vec!["0"]]
        );
    }
}

#[test]
fn port_a87bf8db64_tikv_import_sql_omits_unconfigured_options() {
    use super::mview_refresh::build_shadow_sql;
    let table = astersql_meta_model::TableInfo {
        MaterializedView: Some(astersql_meta_model::MaterializedViewInfo {
            SQLContent: "SELECT g,SUM(v),COUNT(*) FROM test.src GROUP BY g".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        build_shadow_sql("test", "shadow", &table, "TiKV", 0, "").unwrap(),
        "IMPORT INTO `test`.`shadow` FROM (SELECT g,SUM(v),COUNT(*) FROM test.src GROUP BY g) WITH disable_precheck"
    );
    assert_eq!(
        build_shadow_sql("test", "shadow", &table, "TiKV", 4, "1GiB").unwrap(),
        "IMPORT INTO `test`.`shadow` FROM (SELECT g,SUM(v),COUNT(*) FROM test.src GROUP BY g) WITH disable_precheck, thread=4, disk_quota='1GiB'"
    );
    assert_eq!(
        build_shadow_sql("test", "shadow", &table, "tikv", 4, "1GiB").unwrap(),
        "INSERT INTO `test`.`shadow` SELECT g,SUM(v),COUNT(*) FROM test.src GROUP BY g"
    );
    assert!(build_shadow_sql("test", "shadow", &Default::default(), "TiKV", 0, "").is_err());
}

#[test]
fn port_a87bf8db64_source_accepts_fractional_definition_timezone() {
    let (_, session) = fixture();
    for (zone, seconds) in [("+05:30", 19800), ("-03:30", -12600), ("+14:00", 50400)] {
        query(&session, &format!("SET time_zone='{zone}'"));
        let node = astersql_parser::New()
            .ParseOneStmt("SELECT NOW(6)", "", "")
            .unwrap();
        let select = node.into_any().downcast::<ast::SelectStmt>().unwrap();
        let version = session
            .domain
            .storage()
            .with_storage(|store| store.CurrentVersion("global"))
            .unwrap()
            .Ver;
        let before = chrono::Utc::now().naive_utc() + chrono::Duration::seconds(seconds);
        let mut source = session.compile_mview_source(*select, version).unwrap();
        use astersql_executor::adapter::ExecExecutor;
        source.Open().unwrap();
        let mut chunk = source.NewChunk();
        source.Next(&mut chunk).unwrap();
        assert_eq!(chunk.NumRows(), 1);
        let datum = chunk.GetRow(0).GetDatum(0, &source.Schema()[0].field_type);
        let actual = parse_runtime_datetime(&datum.ToString().unwrap()).unwrap();
        let after = chrono::Utc::now().naive_utc() + chrono::Duration::seconds(seconds);
        assert!(
            actual >= before - chrono::Duration::milliseconds(1)
                && actual <= after + chrono::Duration::milliseconds(1),
            "zone={zone} actual={actual} before={before} after={after}"
        );
        source.Close().unwrap();
    }
}

#[test]
fn port_a87bf8db64_where_nullable_keys_and_minmax_match_full_query() {
    let definition = "SELECT g,SUM(v),COUNT(v),COUNT(*),MIN(v),MAX(v) FROM test.src_runtime WHERE v IS NULL OR v>2 GROUP BY g";
    for mode in [
        "fast",
        "complete delta apply",
        "complete in place",
        "complete out of place",
    ] {
        let (f, session) = fixture_definition(
            "CREATE TABLE src_runtime(id int primary key,g int,v int,KEY gv(g,v))",
            "(1,NULL,10),(2,NULL,20),(3,2,30),(4,2,NULL),(5,9,1),(6,2,40),(9,2,35)",
            "CREATE TABLE mv_runtime(g int,s decimal(65,0),cv bigint not null,cnt bigint not null,mn int,mx int)",
            definition,
        );
        if mode == "complete out of place" {
            EnableMockDDLForTest(&f.domain).unwrap();
        }
        query(&session, "DELETE FROM src_runtime WHERE id=3");
        query(&session, "UPDATE src_runtime SET v=5 WHERE id=6");
        query(&session, "UPDATE src_runtime SET v=1 WHERE id=2");
        query(
            &session,
            "INSERT INTO src_runtime VALUES (7,NULL,30),(8,9,3)",
        );
        let (_, result) = run(&session, mode, &mut Observer::default());
        result.unwrap_or_else(|error| panic!("{mode}: {error}"));
        if mode == "fast" {
            let base = f.domain.table_by_name("test", "src_runtime").unwrap();
            let index = base
                .Indices
                .iter()
                .find(|index| index.Name.L == "gv")
                .unwrap();
            assert!(
                f.domain
                    .internal_index_usage()
                    .sample(base.ID, index.ID)
                    .row_access_total
                    > 0,
                "bound MIN/MAX lookup must consume the real covering index"
            );
        }
        assert_eq!(
            query(&session, "SELECT * FROM mv_runtime ORDER BY g"),
            query(&session, &format!("{definition} ORDER BY g")),
            "{mode}"
        );
    }
}

#[test]
fn port_a87bf8db64_bounded_fast_persists_target_and_rejects_older_watermark() {
    let (_, session) = fixture();
    query(&session, "INSERT INTO src_runtime VALUES(4,3,7)");
    let target = session
        .domain
        .storage()
        .with_storage(|store| store.CurrentVersion("global"))
        .unwrap()
        .Ver;
    query(&session, "INSERT INTO src_runtime VALUES(5,4,8)");
    let command = format!("fast AS OF TIMESTAMP {target}");
    let (_, result) = run(&session, &command, &mut Observer::default());
    result.unwrap();
    assert_eq!(
        query(&session, "SELECT * FROM mv_runtime ORDER BY g"),
        vec![
            vec!["1", "30", "2"],
            vec!["2", "30", "1"],
            vec!["3", "7", "1"]
        ]
    );
    assert_eq!(
        query(
            &session,
            "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info"
        ),
        vec![vec![target.to_string()]]
    );
    assert_eq!(
        query(
            &session,
            "SELECT REFRESH_READ_TSO FROM mysql.tidb_mview_refresh_hist"
        ),
        vec![vec![target.to_string()]]
    );
    let (_, result) = run(&session, &command, &mut Observer::default());
    result.unwrap();
    assert_eq!(
        query(
            &session,
            "SELECT COUNT(*) FROM mysql.tidb_mview_refresh_hist"
        ),
        vec![vec!["1"]]
    );
    let (_, result) = run(
        &session,
        &format!("fast AS OF TIMESTAMP {}", target - 1),
        &mut Observer::default(),
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("older than LAST_SUCCESS_READ_TSO")
    );
    assert_eq!(
        query(
            &session,
            "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info"
        ),
        vec![vec![target.to_string()]]
    );
    let (_, result) = run(&session, "fast", &mut Observer::default());
    result.unwrap();
    assert_eq!(
        query(&session, "SELECT * FROM mv_runtime ORDER BY g").len(),
        4
    );
}

#[test]
fn port_a87bf8db64_collectors_respect_stats_and_execution_info_switches() {
    use super::mview_refresh::InternalCollectorGuard;
    let (_, session) = fixture();
    session.domain.set_stats_updating(false);
    {
        let _guard = InternalCollectorGuard::attach(&session);
        assert!(session.internal_collectors.borrow().stats.is_none());
        assert!(session.internal_collectors.borrow().indexes.is_none());
    }
    session.domain.set_stats_updating(true);
    struct Restore(Arc<astersql_config::Config>, bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            self.0.instance.enable_collect_execution_info.store(self.1);
        }
    }
    let config = astersql_config::get_global_config();
    let old = config.instance.enable_collect_execution_info.load();
    config.instance.enable_collect_execution_info.store(false);
    let _restore = Restore(config, old);
    {
        let _guard = InternalCollectorGuard::attach(&session);
        assert!(session.internal_collectors.borrow().stats.is_some());
        assert!(session.internal_collectors.borrow().indexes.is_none());
    }
    query(&session, "INSERT INTO src_runtime VALUES (4,3,7)");
    run(&session, "fast", &mut Observer::default()).1.unwrap();
    assert_eq!(
        query(
            &session,
            "SELECT REFRESH_ROWS FROM mysql.tidb_mview_refresh_hist"
        ),
        vec![vec!["<nil>"]]
    );
}

#[test]
fn port_a87bf8db64_tikv_shadow_import_uses_autocommit_and_real_read_tso() {
    let (f, session) = fixture();
    query(
        &session,
        "CREATE TABLE shadow_import(g int not null,s decimal(65,0) not null,cnt bigint not null)",
    );
    let view = (*f.domain.table_by_name("test", "mv_runtime").unwrap()).clone();
    let mut shadow = (*f.domain.table_by_name("test", "shadow_import").unwrap()).clone();
    shadow.MaterializedViewShadow = Some(astersql_meta_model::MaterializedViewShadowInfo {
        SourceMViewID: view.ID,
    });
    save_meta(&f, shadow);
    session.state.borrow_mut().in_restricted_sql = true;
    let sql = super::mview_refresh::build_shadow_sql("test", "shadow_import", &view, "TiKV", 0, "")
        .unwrap();
    assert!(session.state.borrow().transaction.is_none());
    query(&session, &sql);
    assert!(session.state.borrow().transaction.is_none());
    let info: serde_json::Value =
        serde_json::from_str(&session.state.borrow().last_query_info).unwrap();
    assert!(info["start_ts"].as_u64().is_some_and(|ts| ts > 0), "{info}");
    assert_eq!(
        query(&session, "SELECT * FROM shadow_import ORDER BY g"),
        query(&session, "SELECT * FROM mv_runtime ORDER BY g")
    );
}

#[test]
fn port_a87bf8db64_manual_cancel_rolls_back_and_records_requester() {
    use astersql_testkit_testfailpoint as fp;
    struct CancelObserver {
        session: ConcreteSession,
        watched: Arc<AtomicBool>,
        requester: Option<&'static str>,
        requested: bool,
    }
    impl StepObserver for CancelObserver {
        fn on_step_start(&mut self, _: &ObserveStep, _: SystemTime) {}
        fn on_step_plan_rows(&mut self, _: &ObserveStep, _: Vec<Vec<String>>) {}
        fn on_step_end(&mut self, step: &ObserveStep, _: SystemTime, error: Option<&str>) {
            if self.requested
                || error.is_some()
                || !matches!(
                    step.name,
                    "DATA_CHANGE_FAST_MERGE"
                        | "DATA_CHANGE_COMPLETE_INSERT"
                        | "DATA_CHANGE_COMPLETE_DELTA_APPLY"
                        | "OUT_OF_PLACE_LOAD_SHADOW"
                )
            {
                return;
            }
            self.requested = true;
            let job=query(&self.session,"SELECT REFRESH_JOB_ID FROM mysql.tidb_mview_refresh_hist WHERE REFRESH_STATUS='running'")[0][0].clone();
            if let Some(requester) = self.requester {
                assert_eq!(requester, "'operator'@'localhost'");
                let mut caller = ConcreteSession::new(self.session.domain.clone());
                caller.login_user = Some("operator".into());
                caller.login_host = Some("localhost".into());
                caller.authenticated_host = Some("localhost".into());
                query(
                    &caller,
                    &format!("CANCEL MATERIALIZED VIEW REFRESH JOB {job}"),
                );
            } else {
                query(
                    &self.session,
                    &format!("CANCEL MATERIALIZED VIEW REFRESH JOB {job}"),
                );
            }
            let until = Instant::now() + Duration::from_secs(5);
            while !self.watched.load(Ordering::Acquire) && Instant::now() < until {
                std::thread::park_timeout(Duration::from_millis(1));
            }
            assert!(
                self.watched.load(Ordering::Acquire),
                "real cancellation watcher did not observe committed request"
            );
        }
    }
    for (mode, requester) in [
        ("fast", None),
        ("complete in place", None),
        ("complete delta apply", None),
        ("complete out of place", Some("'operator'@'localhost'")),
    ] {
        let (f, session) = fixture();
        if mode == "complete out of place" {
            EnableMockDDLForTest(&f.domain).unwrap();
        }
        let before = query(&session, "SELECT * FROM mv_runtime ORDER BY g");
        let watermark = query(
            &session,
            "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info",
        );
        query(&session, "INSERT INTO src_runtime VALUES(4,3,7)");
        let watched = Arc::new(AtomicBool::new(false));
        let callback = watched.clone();
        let _callback = fp::enable_concurrent_call(
            "github.com/pingcap/tidb/pkg/executor/mvTaskCancelWatcherRequested",
            move || {
                callback.store(true, Ordering::Release);
            },
        );
        let _poll = fp::enable(
            "github.com/pingcap/tidb/pkg/executor/mockMVTaskMonitorPollInterval",
            "return(1)",
        );
        let _heartbeat = fp::enable(
            "github.com/pingcap/tidb/pkg/executor/mockMVTaskHistHeartbeatInterval",
            "return(1)",
        );
        if requester.is_some() {
            query(&session, "CREATE USER 'operator'@'localhost'");
            query(
                &session,
                "GRANT OPERATE VIEW ON test.mv_runtime TO 'operator'@'localhost'",
            );
        }
        let mut observer = CancelObserver {
            session: session.clone(),
            watched,
            requester,
            requested: false,
        };
        let (_, result) = run(&session, mode, &mut observer);
        assert_eq!(
            result.unwrap_err().to_string(),
            "materialized view task canceled manually"
        );
        assert!(observer.requested);
        assert_eq!(
            query(&session, "SELECT * FROM mv_runtime ORDER BY g"),
            before
        );
        assert_eq!(
            query(
                &session,
                "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info"
            ),
            watermark
        );
        let reason = requester.map_or_else(
            || "cancelled manually".to_owned(),
            |user| format!("cancelled manually by {user}"),
        );
        assert_eq!(
            query(
                &session,
                "SELECT REFRESH_STATUS,REFRESH_FAILED_REASON FROM mysql.tidb_mview_refresh_hist"
            ),
            vec![vec!["failed".to_owned(), reason]]
        );
        let job = query(
            &session,
            "SELECT REFRESH_JOB_ID FROM mysql.tidb_mview_refresh_hist",
        )[0][0]
            .clone();
        assert!(
            session
                .execute(&format!("CANCEL MATERIALIZED VIEW REFRESH JOB {job}"))
                .is_err()
        );
        run(&session, mode, &mut Observer::default()).1.unwrap();
        assert_eq!(
            query(&session, "SELECT * FROM mv_runtime ORDER BY g").len(),
            3
        );
    }
}

#[test]
fn port_a87bf8db64_source_preserves_current_mv_column_types() {
    let (f, session) = fixture();
    query(&session, "INSERT INTO src_runtime VALUES(4,1,7)");
    let table = f.domain.table_by_name("test", "mv_runtime").unwrap();
    let version = f
        .domain
        .storage()
        .with_storage(|store| store.CurrentVersion("global"))
        .unwrap()
        .Ver;
    for sql in ["SELECT mv.g,mv.s,mv.cnt FROM test.mv_runtime AS mv"] {
        let node = astersql_parser::New()
            .ParseOneStmt(sql, "", "")
            .unwrap()
            .into_any()
            .downcast::<ast::SelectStmt>()
            .unwrap();
        let executor = session.compile_mview_source(*node, version).unwrap();
        for (i, col) in executor.Schema().iter().enumerate() {
            assert!(
                col.field_type.Equal(&table.Columns[i].FieldType),
                "simple current column {i}: source={:?} target={:?}; plan={:?}",
                col.field_type,
                table.Columns[i].FieldType,
                session.mview_plan_rows.borrow()
            );
        }
    }
    let (definition, select, _) =
        super::mview_source::definition(&session, "test", &table).unwrap();
    let log = f
        .domain
        .info_schema()
        .TableByID(definition.mlog_table_id)
        .unwrap()
        .ModelMeta()
        .unwrap();
    assert!(
        !query(&session, &format!("SELECT * FROM test.`{}`", log.Name.O)).is_empty(),
        "real MLog is empty after base INSERT"
    );
    for sql in [format!("SELECT _tidb_commit_ts FROM test.`{}`", log.Name.O)] {
        let select = astersql_parser::New()
            .ParseOneStmt(&sql, "", "")
            .unwrap()
            .into_any()
            .downcast::<ast::SelectStmt>()
            .unwrap();
        let mut scan = session.compile_mview_source(*select, version).unwrap();
        scan.Open().unwrap();
        let mut chunk = scan.NewChunk();
        scan.Next(&mut chunk).unwrap();
        assert!(chunk.NumRows() > 0, "real MLog typed scan empty");
        let value = chunk
            .GetRow(0)
            .GetDatum(0, &scan.Schema()[0].field_type)
            .GetUint64();
        assert!(
            value > 1_000_000_000,
            "commit timestamp projection read {value} instead of actual MVCC version"
        );
        let watermark = query(
            &session,
            "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info",
        )[0][0]
            .parse::<u64>()
            .unwrap();
        assert!(
            value > watermark,
            "ML actual commit tso {value} is not newer than initial watermark {watermark}"
        );
        scan.Close().unwrap();
        let sql = format!(
            "SELECT _tidb_commit_ts FROM test.`{}` WHERE _tidb_commit_ts > {watermark}",
            log.Name.O
        );
        let select = astersql_parser::New()
            .ParseOneStmt(&sql, "", "")
            .unwrap()
            .into_any()
            .downcast::<ast::SelectStmt>()
            .unwrap();
        let mut bounded = session.compile_mview_source(*select, version).unwrap();
        bounded.Open().unwrap();
        let mut data = bounded.NewChunk();
        bounded.Next(&mut data).unwrap();
        assert!(
            data.NumRows() > 0,
            "real ts predicate removed new log rows, ts={value}, watermark={watermark}, pk={}, plan={:?}",
            log.PKIsHandle,
            session.mview_plan_rows.borrow()
        );
        bounded.Close().unwrap();
    }
    let (source, plan) = astersql_planner_core::mview_refresh_source::BuildFastRefreshSource(
        &definition,
        "mv_runtime",
        &log.Name.O,
        &select,
        astersql_planner_core::MViewBuildOptions {
            from_ts: query(
                &session,
                "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info",
            )[0][0]
                .parse()
                .unwrap(),
            to_ts: 0,
        },
    )
    .unwrap();
    let mut executor = session.compile_mview_source(source, version).unwrap();
    for i in 0..table.Columns.len() {
        let source = &executor.Schema()[plan.delta_column_count + i].field_type;
        assert!(
            source.Equal(&table.Columns[i].FieldType),
            "merge current column {i}: source={source:?} target={:?}; plan={:?}",
            table.Columns[i].FieldType,
            session.mview_plan_rows.borrow()
        );
    }
    use astersql_executor::adapter::ExecExecutor;
    executor.Open().unwrap();
    let mut chunk = executor.NewChunk();
    executor
        .NextWithContext(
            &astersql_executor::adapter::ExecutionContext {
                sql_killer: Some(session.sql_killer.clone()),
                ..Default::default()
            },
            &mut chunk,
        )
        .unwrap();
    assert!(
        chunk.NumRows() > 0,
        "real signed delta aggregation produced no rows; plans={:?}",
        session.mview_plan_rows.borrow()
    );
    let values = chunk
        .GetRow(0)
        .GetDatumRow(
            &executor
                .Schema()
                .iter()
                .map(|column| column.field_type.clone())
                .collect::<Vec<_>>(),
        )
        .iter()
        .map(|datum| datum.ToString().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        &values[..5],
        &["1", "7", "1", "30", "2"],
        "actual merge source row {values:?}"
    );
    executor.Close().unwrap();
}

#[test]
fn port_a87bf8db64_history_fallback_and_purge_hazard_preserve_watermark() {
    for before_history in [true, false] {
        let (_, session) = fixture();
        let watermark = query(
            &session,
            "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info",
        );
        let before = query(&session, "SELECT * FROM mv_runtime ORDER BY g");
        query(&session, "INSERT INTO src_runtime VALUES (4,3,7)");
        let fault = before_history.then(|| astersql_testkit_testfailpoint::enable(
            "github.com/pingcap/tidb/pkg/executor/mockRefreshMaterializedViewErrorBeforeInsertHist",
            "return(\"before running history\")",
        ));
        if !before_history {
            let table = session.domain.table_by_name("test", "src_runtime").unwrap();
            let log = table.MaterializedViewBase.as_ref().unwrap().MLogID;
            let last = watermark[0][0].parse::<u64>().unwrap();
            query(
                &session,
                &format!(
                    "UPDATE mysql.tidb_mlog_purge_info SET LAST_PURGED_TSO={} WHERE MLOG_ID={log}",
                    last + 1
                ),
            );
        }
        let mut observer = Observer::default();
        let (_, result) = run(&session, "fast", &mut observer);
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(if before_history {
                "before running history"
            } else {
                "may have been purged"
            }),
            "{error}"
        );
        assert_eq!(
            query(&session, "SELECT * FROM mv_runtime ORDER BY g"),
            before
        );
        assert_eq!(
            query(
                &session,
                "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info"
            ),
            watermark
        );
        let history = query(
            &session,
            "SELECT REFRESH_STATUS,REFRESH_READ_TSO,REFRESH_FAILED_REASON FROM mysql.tidb_mview_refresh_hist",
        );
        assert_eq!(history.len(), 1);
        assert_eq!(history[0][0], "failed");
        assert_eq!(history[0][1], "<nil>");
        assert!(history[0][2].contains(if before_history {
            "before running history"
        } else {
            "may have been purged"
        }));
        if !before_history {
            assert_eq!(
                observer.ended.last().map(String::as_str),
                Some("FINALIZE_HIST")
            );
        }
        drop(fault);
    }
}

#[test]
fn port_a87bf8db64_refresh_info_nowait_preserves_locked_row_and_failure_timing() {
    let (_, session) = fixture();
    let locker = ConcreteSession::new(session.domain.clone());
    query(&locker, "BEGIN PESSIMISTIC");
    let watermark = query(
        &locker,
        "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info FOR UPDATE",
    );
    let (executor, result) = run(&session, "fast", &mut Observer::default());
    let error = result
        .expect_err("refresh must not bypass the locked watermark row")
        .to_string();
    assert!(
        error.contains("NOWAIT") && error.contains("3572"),
        "{error}"
    );
    assert!(executor.timings.lock_refresh_info > Duration::ZERO);
    assert!(executor.timings.transaction_total > Duration::ZERO);
    query(&locker, "ROLLBACK");
    assert_eq!(
        query(
            &session,
            "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info"
        ),
        watermark
    );
    assert_eq!(
        query(
            &session,
            "SELECT REFRESH_STATUS FROM mysql.tidb_mview_refresh_hist"
        ),
        vec![vec!["failed"]]
    );
    let (_, result) = run(&session, "fast", &mut Observer::default());
    result.unwrap();
}

#[test]
fn port_05fd982b2d_sql_entry_executes_real_refresh() {
    let (_f, session) = fixture();
    query(&session, "INSERT INTO src_runtime VALUES (4,1,40)");
    query(
        &session,
        "REFRESH MATERIALIZED VIEW test.mv_runtime complete in place",
    );
    assert_eq!(
        query(&session, "SELECT * FROM mv_runtime ORDER BY g"),
        vec![vec!["1", "70", "3"], vec!["2", "30", "1"]]
    );
    assert_eq!(
        query(
            &session,
            "SELECT REFRESH_STATUS FROM mysql.tidb_mview_refresh_hist"
        ),
        vec![vec!["success"]]
    );
}

#[test]
fn port_05fd982b2d_dry_run_profile_and_plain_preserve_entry_contract() {
    for (mode, step, operator) in [
        ("fast", "DATA_CHANGE_FAST_MERGE", "MViewDeltaMerge"),
        ("complete in place", "DATA_CHANGE_COMPLETE_INSERT", "Insert"),
        (
            "complete delta apply",
            "DATA_CHANGE_COMPLETE_DELTA_APPLY",
            "MViewCompleteDeltaApply",
        ),
        (
            "complete out of place",
            "OUT_OF_PLACE_LOAD_SHADOW",
            "Insert",
        ),
    ] {
        let (f, session) = fixture();
        if mode == "complete out of place" {
            EnableMockDDLForTest(&f.domain).unwrap();
        }
        let old_id = f.domain.table_by_name("test", "mv_runtime").unwrap().ID;
        query(&session, "INSERT INTO src_runtime VALUES(4,1,40)");
        let before = query(&session, "SELECT * FROM mv_runtime ORDER BY g");
        let watermark = query(
            &session,
            "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info",
        );
        let refresh = format!("REFRESH MATERIALIZED VIEW test.mv_runtime {mode}");
        for _ in 0..2 {
            let plan = format!("{:?}", query(&session, &format!("{refresh} DRY RUN")));
            assert!(plan.contains(step), "{mode}: {plan}");
            assert!(plan.contains(operator), "{mode}: {plan}");
            assert!(plan.contains("FINALIZE_HIST"));
            assert!(!plan.contains("status:success"));
            assert_eq!(
                query(&session, "SELECT * FROM mv_runtime ORDER BY g"),
                before
            );
            assert_eq!(
                query(
                    &session,
                    "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info"
                ),
                watermark
            );
            assert_eq!(
                query(
                    &session,
                    "SELECT COUNT(*) FROM mysql.tidb_mview_refresh_hist"
                ),
                vec![vec!["0"]]
            );
            assert_eq!(
                query(
                    &session,
                    "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='test' AND table_name LIKE '__mv_shadow_%'"
                ),
                vec![vec!["0"]]
            );
        }
        let profile = format!("{:?}", query(&session, &format!("{refresh} WITH PROFILE")));
        assert!(profile.contains(step));
        assert!(profile.contains(operator), "{mode}: {profile}");
        assert!(profile.contains("status:success, time:"));
        assert!(profile.contains("actRows:"), "{mode}: {profile}");
        assert!(profile.contains("loops:"), "{mode}: {profile}");
        assert!(profile.contains("FINALIZE_HIST"));
        assert_eq!(
            query(&session, "SELECT * FROM mv_runtime ORDER BY g"),
            vec![vec!["1", "70", "3"], vec!["2", "30", "1"]]
        );
        assert_eq!(
            query(
                &session,
                "SELECT REFRESH_STATUS,REFRESH_METHOD FROM mysql.tidb_mview_refresh_hist"
            ),
            vec![vec!["success".to_string(), format!("{mode} manual")]]
        );
        let current_id = f.domain.table_by_name("test", "mv_runtime").unwrap().ID;
        assert_eq!(old_id == current_id, mode != "complete out of place");
        query(&session, "INSERT INTO src_runtime VALUES(5,3,50)");
        query(&session, &refresh);
        assert!(session.protocol_state().affected_rows > 0);
        assert_eq!(
            query(&session, "SELECT * FROM mv_runtime ORDER BY g"),
            vec![
                vec!["1", "70", "3"],
                vec!["2", "30", "1"],
                vec!["3", "50", "1"]
            ]
        );
        assert!(format!("{:?}", query(&session, &format!("{refresh} DRY RUN"))).contains(step));
        assert!(!session.state.borrow().in_restricted_sql);
    }
}

#[test]
fn port_05fd982b2d_transaction_direct_prepared_and_autocommit_validation() {
    let (_f, session) = fixture();
    let refresh = "REFRESH MATERIALIZED VIEW test.mv_runtime complete in place";
    assert!(
        session
            .execute("REFRESH MATERIALIZED VIEW test.mv_runtime WITH ASYNC MODE complete in place")
            .err()
            .unwrap()
            .to_string()
            .contains("WITH ASYNC MODE is not supported yet")
    );
    for suffix in ["", " DRY RUN", " WITH PROFILE"] {
        query(&session, "BEGIN");
        assert!(
            session
                .execute(&format!("{refresh}{suffix}"))
                .err()
                .unwrap()
                .to_string()
                .contains("cannot run REFRESH MATERIALIZED VIEW in explicit transaction")
        );
        query(&session, "ROLLBACK");
    }
    query(&session, &format!("PREPARE refresh_stmt FROM '{refresh}'"));
    query(&session, "BEGIN");
    assert!(
        session
            .execute("EXECUTE refresh_stmt")
            .err()
            .unwrap()
            .to_string()
            .contains("cannot run REFRESH MATERIALIZED VIEW in explicit transaction")
    );
    query(&session, "ROLLBACK");
    query(&session, "DEALLOCATE PREPARE refresh_stmt");
    query(&session, "SET autocommit=0");
    query(&session, refresh);
    query(&session, "INSERT INTO src_runtime VALUES(4,1,40)");
    assert!(
        session
            .execute(refresh)
            .err()
            .unwrap()
            .to_string()
            .contains("cannot run REFRESH MATERIALIZED VIEW in explicit transaction")
    );
    query(&session, "ROLLBACK");
    query(&session, "SET autocommit=1");
}

#[test]
fn port_05fd982b2d_entry_privileges_and_cancel_request() {
    let (f, session) = fixture();
    query(&session, "CREATE USER 'refresh_entry_u'@'%'");
    let mut user = ConcreteSession::new(f.domain.clone());
    user.login_user = Some("refresh_entry_u".into());
    user.login_host = Some("%".into());
    user.authenticated_host = Some("%".into());
    let refresh = "REFRESH MATERIALIZED VIEW test.mv_runtime complete delta apply";
    assert!(
        user.execute(refresh)
            .err()
            .unwrap()
            .to_string()
            .contains("OPERATE VIEW command denied")
    );
    query(
        &session,
        "GRANT OPERATE VIEW ON test.mv_runtime TO 'refresh_entry_u'@'%'",
    );
    assert!(
        user.execute(refresh)
            .err()
            .unwrap()
            .to_string()
            .contains("SELECT command denied")
    );
    for suffix in [" DRY RUN", " WITH PROFILE"] {
        assert!(
            user.execute(&format!("{refresh}{suffix}"))
                .err()
                .unwrap()
                .to_string()
                .contains("SHOW VIEW command denied")
        );
    }
    query(
        &session,
        "GRANT SHOW VIEW ON test.mv_runtime TO 'refresh_entry_u'@'%'",
    );
    for suffix in [" DRY RUN", " WITH PROFILE"] {
        assert!(
            user.execute(&format!("{refresh}{suffix}"))
                .err()
                .unwrap()
                .to_string()
                .contains("SELECT command denied")
        );
    }
    query(
        &session,
        "GRANT SELECT ON test.src_runtime TO 'refresh_entry_u'@'%'",
    );
    query(&user, refresh);
    for suffix in [" DRY RUN", " WITH PROFILE"] {
        assert!(
            format!("{:?}", query(&user, &format!("{refresh}{suffix}"))).contains("FINALIZE_HIST")
        );
    }
    let id = f.domain.table_by_name("test", "mv_runtime").unwrap().ID;
    query(
        &session,
        &format!(
            "INSERT INTO mysql.tidb_mview_refresh_hist (REFRESH_JOB_ID,MVIEW_ID,REFRESH_METHOD,REFRESH_STATUS) VALUES(9000000000000000001,{id},'fast manual','running'),(9000000000000000002,{id},'fast manual','success')"
        ),
    );
    query(
        &session,
        "REVOKE OPERATE VIEW ON test.mv_runtime FROM 'refresh_entry_u'@'%'",
    );
    assert!(
        user.execute("CANCEL MATERIALIZED VIEW REFRESH JOB 9000000000000000001")
            .err()
            .unwrap()
            .to_string()
            .contains("OPERATE VIEW command denied")
    );
    query(
        &session,
        "GRANT OPERATE VIEW ON test.mv_runtime TO 'refresh_entry_u'@'%'",
    );
    query(
        &user,
        "CANCEL MATERIALIZED VIEW REFRESH JOB 9000000000000000001",
    );
    assert_eq!(
        query(
            &session,
            "SELECT CANCEL_REQUEST_TIME IS NOT NULL,CANCEL_REQUESTED_BY FROM mysql.tidb_mview_refresh_hist WHERE REFRESH_JOB_ID=9000000000000000001"
        ),
        vec![vec!["1", "'refresh_entry_u'@'%'"]]
    );
    for id in [
        9000000000000000001_i64,
        9000000000000000002,
        9000000000000000003,
    ] {
        assert!(
            user.execute(&format!("CANCEL MATERIALIZED VIEW REFRESH JOB {id}"))
                .err()
                .unwrap()
                .to_string()
                .contains(&format!("cannot cancel materialized view refresh job {id}"))
        );
    }
    query(
        &session,
        "INSERT INTO mysql.tidb_mview_refresh_hist (REFRESH_JOB_ID,MVIEW_ID,REFRESH_METHOD,REFRESH_STATUS) VALUES(9000000000000000004,-1234,'fast manual','running')",
    );
    assert_eq!(
        user.execute("CANCEL MATERIALIZED VIEW REFRESH JOB 9000000000000000004")
            .err()
            .unwrap()
            .to_string(),
        "refresh materialized view: cannot resolve target materialized view -1234 for cancel job 9000000000000000004"
    );
    query(&session, "DROP TABLE mysql.tidb_mview_refresh_hist");
    assert_eq!(
        user.execute("CANCEL MATERIALIZED VIEW REFRESH JOB 9000000000000000001")
            .err()
            .unwrap()
            .to_string(),
        "refresh materialized view: required system table mysql.tidb_mview_refresh_hist does not exist"
    );
}

#[test]
fn port_05fd982b2d_purge_next_null_logs_only_automatic_and_redacts_literals() {
    let message = "purge materialized view log: automatic purge schedule disabled because NEXT expression evaluated to NULL, updating NEXT_PURGE_UNIX_SECONDS to NULL";
    for (expression, want_null, want_log) in [
        ("", true, false),
        ("DATE_ADD(NOW(), INTERVAL 1 HOUR)", false, false),
        (
            "CAST(NULLIF('purge-next-secret','purge-next-secret') AS DATETIME)",
            true,
            true,
        ),
    ] {
        let (f, session) = fixture();
        let base = f.domain.table_by_name("test", "src_runtime").unwrap();
        let mut log = (*f
            .domain
            .info_schema()
            .TableByID(base.MaterializedViewBase.as_ref().unwrap().MLogID)
            .unwrap()
            .ModelMeta()
            .unwrap())
        .clone();
        log.MaterializedViewLog.as_mut().unwrap().PurgeNext = expression.into();
        save_meta(&f, log);
        let before = BgLogger().entries().len();
        query(&session, "PURGE MATERIALIZED VIEW LOG ON test.src_runtime");
        assert!(
            !BgLogger().entries()[before..]
                .iter()
                .any(|e| e.message == message)
        );
        let before = BgLogger().entries().len();
        let old_restricted = session.state.borrow().in_restricted_sql;
        session.state.borrow_mut().in_restricted_sql = true;
        query(&session, "PURGE MATERIALIZED VIEW LOG ON test.src_runtime");
        session.state.borrow_mut().in_restricted_sql = old_restricted;
        assert_eq!(
            query(
                &session,
                "SELECT NEXT_PURGE_UNIX_SECONDS IS NULL FROM mysql.tidb_mlog_purge_info"
            ),
            vec![vec![if want_null { "1" } else { "0" }]]
        );
        let logs = BgLogger().entries();
        let logs = logs[before..]
            .iter()
            .filter(|e| e.message == message)
            .collect::<Vec<_>>();
        assert_eq!(!logs.is_empty(), want_log);
        for log in logs {
            assert!(
                log.fields
                    .contains(&LogField::String("schemaName".into(), "test".into()))
            );
            assert!(log.fields.contains(&LogField::String(
                "tableName".into(),
                "$mlog$src_runtime".into()
            )));
            assert!(!format!("{:?}", log.fields).contains("purge-next-secret"));
        }
    }
}

#[test]
fn port_05fd982b2d_profile_requires_actual_operator_rows() {
    for mode in ["fast", "complete delta apply"] {
        let (_f, session) = fixture();
        for values in ["(4,1,40)", "(5,3,50),(6,3,60)"] {
            let before = query(&session, "SELECT COUNT(*) FROM test.src_runtime")[0][0]
                .parse::<u64>()
                .unwrap();
            query(
                &session,
                &format!("INSERT INTO src_runtime VALUES {values}"),
            );
            let after = query(&session, "SELECT COUNT(*) FROM test.src_runtime")[0][0]
                .parse::<u64>()
                .unwrap();
            let source_table = if mode == "fast" {
                "$mlog$src_runtime"
            } else {
                "src_runtime"
            };
            // FAST reads the newly committed insert batch after its persisted
            // watermark; COMPLETE reads the full base relation. Query actual
            // base cardinalities rather than estimates or hidden commit-TS SQL.
            let expected = if mode == "fast" {
                after - before
            } else {
                after
            }
            .to_string();
            let output = query(
                &session,
                &format!("REFRESH MATERIALIZED VIEW test.mv_runtime {mode} WITH PROFILE"),
            );
            let scan = output
                .iter()
                .map(|row| &row[0])
                .find(|line| {
                    line.contains("TableScan") && line.contains(&format!("table:{source_table}"))
                })
                .unwrap_or_else(|| panic!("actual MLog scan absent: {output:?}"));
            let measured = scan
                .split("actRows:")
                .nth(1)
                .and_then(|field| field.split(" | ").next())
                .expect("Go Analyze=true profile must include actual operator row statistics");
            assert_eq!(measured, expected, "{scan}");
            assert!(scan.contains("loops:"), "{scan}");
            assert!(scan.contains("open:"), "{scan}");
            assert!(scan.contains("close:"), "{scan}");
            assert!(
                output
                    .iter()
                    .any(|row| row[0].contains("MView") && row[0].contains("actRows:0")),
                "write sink returns no result rows: {output:?}"
            );
        }
    }
}
