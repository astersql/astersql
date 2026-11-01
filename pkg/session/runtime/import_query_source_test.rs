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

use super::*;

#[test]
fn port_964af3fa46_capture_restore_session_states() {
    let (domain, submit) = crate::runtime::CreateUnbootstrappedAnalyzeSessionForTest().unwrap();
    for sql in [
        "create database test",
        "use test",
        "create table query_src(i bigint)",
        "create table unused_target(i bigint)",
        "set cte_max_recursion_depth=5, tidb_mem_quota_query=1048576, @v=1",
        "begin",
    ] {
        submit.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    let q = submit
        .CaptureImportQuerySource("import into unused_target from select i from query_src")
        .unwrap();
    assert_eq!(q.Tables.len(), 1);
    assert_eq!(q.SessionStates.SystemVars["cte_max_recursion_depth"], "5");
    assert_eq!(q.SessionStates.UserVars["v"].GetInt64(), 1);
    assert!(!q.SessionStates.SystemVars["timestamp"].is_empty());
    let worker = crate::runtime::ConcreteSession::new(domain);
    q.RestoreStates(&worker).unwrap();
    assert_eq!(
        worker.select_variable("timestamp", true).unwrap(),
        q.SessionStates.SystemVars["timestamp"]
    );
    let mut sets = worker
        .execute("select @v, @@cte_max_recursion_depth, database()")
        .unwrap();
    assert_eq!(sets[0].next_row().unwrap().unwrap(), vec!["1", "5", "test"]);
    submit.execute("rollback").unwrap();
}

fn port_964af3fa46_setup() -> (std::sync::Arc<Domain>, crate::runtime::ConcreteSession) {
    let (domain, session) = crate::runtime::CreateUnbootstrappedAnalyzeSessionForTest().unwrap();
    for sql in [
        "create database test",
        "use test",
        "create table query_src(i bigint)",
        "create table unused_target(i bigint)",
    ] {
        session
            .execute(sql)
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    EnableMockDDLForTest(&domain).unwrap();
    (domain, session)
}

#[test]
fn port_964af3fa46_source_tables_and_rejections() {
    let (domain, s) = port_964af3fa46_setup();
    for sql in [
        "create table query_other(i int)",
        "create temporary table query_tmp(i int)",
        "create global temporary table query_gtmp(i int) on commit delete rows",
        "create sequence query_seq",
    ] {
        s.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    s.state.borrow_mut().runaway_resource_group_name = Some("rg1".into());
    let q = s.CaptureImportQuerySource("import into unused_target from (select i from query_src union all select i from query_other)").unwrap();
    assert_eq!(q.Tables.len(), 2);
    assert_eq!(q.SessionStates.ResourceGroupName, "rg1");
    let names: Vec<_> = q
        .Tables
        .iter()
        .map(|src| {
            domain
                .info_schema()
                .TableByID(src.ID)
                .unwrap()
                .Meta()
                .name
                .lower
                .clone()
        })
        .collect();
    assert!(names.contains(&"query_src".to_owned()) && names.contains(&"query_other".to_owned()));
    for (select, expected) in [
        ("select * from query_tmp", "temporary source tables"),
        ("select * from query_gtmp", "temporary source tables"),
        ("select nextval(query_seq)", "sequences"),
        ("select lastval(query_seq)", "sequences"),
        ("select setval(query_seq,1)", "sequences"),
        ("select 1 from query_seq", "sequences"),
        (
            "select table_name from information_schema.tables",
            "virtual source tables",
        ),
        (
            "select variable_name from performance_schema.session_variables",
            "virtual source tables",
        ),
        (
            "select i from query_src where i in (select 1 from information_schema.processlist)",
            "virtual source tables",
        ),
        ("select ? from query_src", "unexpected '?'"),
        (
            "select i from query_src as of timestamp now(6)",
            "stale reads",
        ),
    ] {
        let result =
            s.CaptureImportQuerySource(&format!("import into unused_target from {select}"));
        let error = result.err().unwrap_or_else(|| panic!("accepted {select}"));
        assert!(error.to_string().contains(expected), "{select}: {error}");
        if expected != "unexpected '?'" {
            assert!(
                error.to_string().contains("[planner:1235]"),
                "preserve Go unsupported error: {error}"
            );
        }
        assert!(!s.mdl_autocommit_write.get(), "error cleanup");
    }
}

#[test]
fn port_964af3fa46_views_snapshot_and_temporary_shadow() {
    let (domain, s) = port_964af3fa46_setup();
    for sql in [
        "create database query_view_db",
        "create table query_view_db.base(i int)",
        "create table query_cached(i int)",
        "alter table query_cached cache",
        "use query_view_db",
        "create view inner_v as select i from base",
        "use test",
        "create view outer_v(i) as select i from query_view_db.base",
    ] {
        s.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    // The existing CREATE VIEW bootstrap supports only SelectStmt. Persist
    // the original Go UNION view definition using real metadata, retaining the
    // actual parser, view resolver, planner and snapshot reader under test.
    port_964af3fa46_update_table(&domain, "outer_v", |table| {
        table.View.as_mut().unwrap().SelectStmt = "create view outer_v as select i from query_view_db.inner_v union all select i from query_cached".into();
    });
    let q = s
        .CaptureImportQuerySource("import into unused_target from select * from outer_v")
        .unwrap();
    assert_eq!(q.Tables.len(), 4);
    let names: BTreeSet<_> = q
        .Tables
        .iter()
        .map(|table| {
            let schema = domain.info_schema();
            format!(
                "{}.{}",
                schema.SchemaByID(table.DBID).unwrap().name.lower,
                schema.TableByID(table.ID).unwrap().Meta().name.lower
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            "test.outer_v",
            "query_view_db.inner_v",
            "query_view_db.base",
            "test.query_cached"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    let schema = domain
        .storage()
        .with_storage(|store| q.BuildMinimalInfoSchema(store, domain.info_schema().as_ref()))
        .unwrap();
    assert_eq!(schema.SchemaMetaVersion(), q.SchemaVersion);
    for src in &q.Tables {
        let model = schema.TableByID(src.ID).unwrap().ModelMeta().unwrap();
        assert_eq!(model.Revision, src.Revision);
        assert_eq!(
            model.TableCacheStatusType,
            astersql_meta_model::TableCacheStatusDisable
        );
    }
    assert!(
        schema
            .TableByName(&"test".into(), &"unused_target".into())
            .is_err()
    );
    let mut changed = q.clone();
    changed.Tables[0].Revision += 1;
    assert!(
        domain
            .storage()
            .with_storage(
                |store| changed.BuildMinimalInfoSchema(store, domain.info_schema().as_ref())
            )
            .err()
            .unwrap()
            .contains("changed before the import query snapshot")
    );
    for sql in [
        "create table query_shadowed(i int)",
        "create view query_shadowed_v as select i from query_shadowed",
    ] {
        s.execute(sql).unwrap();
    }
    let physical = s
        .resolve_runtime_table("test", "query_shadowed")
        .unwrap()
        .ID;
    s.execute("create temporary table query_shadowed(i int)")
        .unwrap();
    let q = s
        .CaptureImportQuerySource("import into unused_target from select * from query_shadowed_v")
        .unwrap();
    assert_eq!(q.Tables.len(), 2);
    assert!(q.Tables.iter().any(|t| t.ID == physical));
}

#[test]
fn port_964af3fa46_transaction_schema_and_mdl_revision() {
    struct RestoreMDL(bool);
    impl Drop for RestoreMDL {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::SetEnableMDL(self.0);
        }
    }
    let _restore = RestoreMDL(astersql_sessionctx_vardef::IsMDLEnabled());
    astersql_sessionctx_vardef::SetEnableMDL(true);
    let (domain, s) = port_964af3fa46_setup();
    let schema = domain.info_schema();
    let original = schema
        .ModelTableInfoByName(&"test".into(), &"query_src".into())
        .unwrap();
    // Go PrepareTxnCtx retains an immutable catalog without activating a KV
    // transaction. Use that same real catalog as the local prepared context.
    s.state.borrow_mut().transaction_info_schema = Some(schema.clone());
    let ddl = crate::runtime::ConcreteSession::new(domain.clone());
    ddl.execute("alter table test.query_src add column j int default 42")
        .unwrap();
    // Existing ALTER COLUMN metadata does not increment Revision. Persist the
    // post-DDL revision through the canonical metadata transaction, as a fixture
    // for this source's revision-sensitive capture (not a DDL implementation).
    let mut changed = (*domain
        .info_schema()
        .ModelTableInfoByName(&"test".into(), &"query_src".into())
        .unwrap())
    .clone();
    changed.Revision = original.Revision + 1;
    let mut tx = domain
        .storage()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    let mut meta = astersql_meta::TransactionMutator::new(tx.as_mut());
    meta.update_table(changed.DBID, &mut changed).unwrap();
    let version = meta.gen_schema_version().unwrap();
    meta.set_table_schema_diff(
        &astersql_meta_model::group_3::Job {
            schema_id: changed.DBID,
            table_id: changed.ID,
            ..Default::default()
        },
        version,
    )
    .unwrap();
    tx.Commit(&kv::Context::default()).unwrap();
    domain.reload().unwrap();
    let q = s
        .CaptureImportQuerySource("import into unused_target from select i from query_src")
        .unwrap();
    let latest = domain
        .info_schema()
        .ModelTableInfoByName(&"test".into(), &"query_src".into())
        .unwrap();
    assert_eq!(original.Columns.len(), 1);
    assert_eq!(latest.Columns.len(), 2);
    assert!(latest.Revision > original.Revision);
    assert_eq!(q.SchemaVersion, schema.SchemaMetaVersion());
    assert_eq!(q.Tables[0].Revision, latest.Revision);
    assert!(
        s.mdl_tables
            .borrow()
            .values()
            .any(|(_, t)| t.ID == original.ID && t.Revision == latest.Revision)
    );
    domain
        .storage()
        .with_storage(|store| q.BuildMinimalInfoSchema(store, schema.as_ref()))
        .unwrap();
}

#[test]
fn port_964af3fa46_migration_check_and_worker_owned_vars() {
    let (domain, s) = port_964af3fa46_setup();
    s.execute("set @str='001', @v=1, @max=18446744073709551615")
        .unwrap();
    s.execute("begin").unwrap();
    s.state
        .borrow_mut()
        .last_warnings
        .push(session::SessionWarning::warning(
            "warning of the statement before".into(),
        ));
    assert!(
        s.execute("show session_states")
            .err()
            .unwrap()
            .to_string()
            .contains("active transaction")
    );
    s.state
        .borrow_mut()
        .last_warnings
        .push(session::SessionWarning::warning(
            "warning of the statement before".into(),
        ));
    let mut q = s
        .CaptureImportQuerySource("import into unused_target from select i from query_src")
        .unwrap();
    assert!(!q.SessionStates.Warnings.is_empty());
    assert!(q.SessionStates.Status & 1 != 0);
    for name in astersql_executor_importer::IMPORT_QUERY_WORKER_VARS {
        q.SessionStates
            .SystemVars
            .insert((*name).into(), "invalid-worker-owned-value".into());
    }
    q.SessionStates
        .SystemVars
        .insert("tidb_redact_log".into(), "ON".into());
    q.SessionStates.ResourceGroupName = "rg1".into();
    assert_eq!(q.SessionStates.UserVars["max"].GetUint64(), u64::MAX);
    let encoded = serde_json::to_string(&q).unwrap();
    let q: astersql_executor_importer::QuerySource = serde_json::from_str(&encoded).unwrap();
    let worker = ConcreteSession::new(domain.clone());
    worker.execute("set tidb_mem_quota_query=2097152").unwrap();
    let quota = worker
        .session_vars
        .GetHintSystemVar("tidb_mem_quota_query")
        .unwrap();
    q.RestoreStates(&worker).unwrap();
    assert_eq!(
        worker
            .session_vars
            .GetHintSystemVar("tidb_mem_quota_query")
            .unwrap(),
        quota
    );
    assert!(worker.state.borrow().transaction.is_none());
    assert!(worker.state.borrow().last_warnings.is_empty());
    assert!(worker.state.borrow().string_user_variables.contains("str"));
    assert_eq!(worker.state.borrow().user_variables["str"], "001");
    assert_eq!(
        worker.state.borrow().user_variables["max"],
        u64::MAX.to_string()
    );
    assert_eq!(
        worker.state.borrow().runaway_resource_group_name.as_deref(),
        Some("rg1")
    );
    assert_eq!(q.SessionStates.Status & 1, 1);
    assert!(!q.SessionStates.Warnings.is_empty());
    s.execute("rollback").unwrap();
    s.execute("select get_lock('query_capture_lock',0)")
        .unwrap();
    assert!(
        s.execute("show session_states")
            .err()
            .unwrap()
            .to_string()
            .contains("advisory locks")
    );
    s.CaptureImportQuerySource("import into unused_target from select i from query_src")
        .unwrap();
    s.execute("select release_all_locks()").unwrap();
    s.execute("create temporary table migration_tmp(i int)")
        .unwrap();
    assert!(
        s.execute("show session_states")
            .err()
            .unwrap()
            .to_string()
            .contains("local temporary tables")
    );
}

fn port_964af3fa46_update_table(
    domain: &Arc<Domain>,
    name: &str,
    update: impl FnOnce(&mut astersql_meta_model::TableInfo),
) {
    let mut table = (*domain
        .info_schema()
        .ModelTableInfoByName(&"test".into(), &name.into())
        .unwrap())
    .clone();
    update(&mut table);
    let mut tx = domain
        .storage()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    let mut meta = astersql_meta::TransactionMutator::new(tx.as_mut());
    meta.update_table(table.DBID, &mut table).unwrap();
    let version = meta.gen_schema_version().unwrap();
    meta.set_table_schema_diff(
        &astersql_meta_model::group_3::Job {
            schema_id: table.DBID,
            table_id: table.ID,
            ..Default::default()
        },
        version,
    )
    .unwrap();
    tx.Commit(&kv::Context::default()).unwrap();
    domain.reload().unwrap();
}

#[test]
fn port_964af3fa46_sql_import_consumes_captured_state() {
    let (_, s) = port_964af3fa46_setup();
    for sql in [
        "insert into query_src values (40)",
        "set @v=2",
        "import into unused_target from select i+@v from query_src with checksum_table='off'",
    ] {
        s.execute(sql)
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
    let mut sets = s.execute("select i from unused_target").unwrap();
    assert_eq!(sets[0].next_row().unwrap().unwrap(), vec!["42"]);
}

#[test]
fn port_964af3fa46_snapshot_missing_metadata_and_legacy_charsets() {
    let (domain, s) = port_964af3fa46_setup();
    port_964af3fa46_update_table(&domain, "query_src", |t| {
        t.Version = 1;
        t.Charset = "utf8".into();
        t.Collate = "utf8_bin".into();
        t.Columns[0].Version = 1;
        t.Columns[0].SetCharset("utf8".into());
        t.Columns[0].SetCollate("utf8_bin".into());
    });
    let q = s
        .CaptureImportQuerySource("import into unused_target from select i from query_src")
        .unwrap();
    for (upgrade, expected) in [(true, "utf8mb4"), (false, "utf8")] {
        let schema = domain
            .storage()
            .with_storage(|store| {
                q.BuildMinimalInfoSchemaWithCharsetPolicy(
                    store,
                    domain.info_schema().as_ref(),
                    upgrade,
                )
            })
            .unwrap();
        let t = schema
            .TableByID(q.Tables[0].ID)
            .unwrap()
            .ModelMeta()
            .unwrap();
        assert_eq!(t.Charset, expected);
        assert_eq!(t.Columns[0].GetCharset(), expected);
    }
    let mut missing = q.clone();
    missing.Tables[0].DBID = i64::MAX;
    assert!(
        domain
            .storage()
            .with_storage(
                |store| missing.BuildMinimalInfoSchema(store, domain.info_schema().as_ref())
            )
            .err()
            .unwrap()
            .contains("source database")
    );
    let mut missing = q;
    missing.Tables[0].ID = i64::MAX;
    assert!(
        domain
            .storage()
            .with_storage(
                |store| missing.BuildMinimalInfoSchema(store, domain.info_schema().as_ref())
            )
            .err()
            .unwrap()
            .contains("changed before the import query snapshot")
    );
}

#[test]
fn port_964af3fa46_read_committed_uses_statement_snapshot() {
    let (domain, s) = port_964af3fa46_setup();
    s.execute("set transaction_isolation='READ-COMMITTED'")
        .unwrap();
    s.execute("begin").unwrap();
    let start = s.state.borrow().transaction.as_ref().unwrap().StartTS();
    let writer = ConcreteSession::new(domain);
    writer
        .execute("insert into test.query_src values (77)")
        .unwrap();
    let q = s
        .CaptureImportQuerySource("import into unused_target from select i from query_src")
        .unwrap();
    assert!(
        q.ReadTS > start,
        "READ COMMITTED must use a new statement read version"
    );
    s.execute("rollback").unwrap();
}
