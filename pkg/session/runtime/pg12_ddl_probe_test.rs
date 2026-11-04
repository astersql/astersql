// Copyright 2026 AsterSQL.

use super::*;

fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut sets = session.execute(sql).expect(sql);
    let mut rows = Vec::new();
    let columns = sets.iter().map(|set| serde_json::json!({
        "labels":set.columns(),
        "native_fields":set.result_fields().iter().map(|f| f.as_ref().map(|f| &f.column)).collect::<Vec<_>>()
    })).collect::<Vec<_>>();
    for set in &mut sets {
        while let Some(row) = set.next_row().expect("read probe rows") {
            rows.push(row);
        }
    }
    eprintln!(
        "native SQL={sql} metadata={} rows={rows:?}",
        serde_json::to_string(&columns).unwrap()
    );
    rows
}

fn durable_state(domain: &Arc<Domain>, job_id: i64) -> (i64, bool, bool) {
    use astersql_domain::InfoSchemaLoader;
    domain.storage().with_storage(|store| {
        let loaded = astersql_domain::KvInfoSchemaLoader::new()
            .load_info_schema(store, "SYSTEM")
            .unwrap();
        let found = loaded
            .schema
            .ModelTableInfoByExactName(&astersql_infoschema::CiString::new("test"), "txddl_probe")
            .is_ok();
        let snapshot = store.GetSnapshot(store.CurrentVersion("global").unwrap());
        let reader = astersql_meta::SnapshotReader::new(snapshot);
        let mut job = reader.get_history_ddl_job(job_id).unwrap();
        assert_eq!(
            reader.get_schema_version_with_non_empty_diff().unwrap(),
            loaded.schema.SchemaMetaVersion()
        );
        let encoded_job = job
            .as_mut()
            .map(|job| String::from_utf8(job.encode(false).unwrap()).unwrap());
        eprintln!("latest KV job_id={job_id} history={encoded_job:?}");
        if let Some(ref job) = job {
            assert_eq!(job.state, astersql_meta_model::group_3::JobState::Synced);
            assert_eq!(job.table_name, "txddl_probe");
        }
        (loaded.schema.SchemaMetaVersion(), found, job.is_some())
    })
}

fn mdl_blocks(session: &ConcreteSession, id: i64, version: i64) -> bool {
    let mut jobs = HashMap::from([(
        1,
        Arc::new(astersql_session_sessmgr::mdldef::JobMDL {
            ver: version,
            table_ids: HashSet::from([id]),
        }),
    )]);
    session.transaction_mdl().check_jobs(&mut jobs);
    jobs.is_empty()
}

#[test]
fn pg12_ddl_atomic_boundary() {
    struct RestoreMdl(bool);
    impl Drop for RestoreMdl {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::SetEnableMDL(self.0);
        }
    }
    let _mdl_restore = RestoreMdl(astersql_sessionctx_vardef::IsMDLEnabled());
    let (domain, bootstrap) = CreateAnalyzeSession().expect("real canonical factory");
    astersql_sessionctx_vardef::SetEnableMDL(true);
    let a = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let b = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    eprintln!(
        "sha={} storage=CreateAnalyzeSession (not RealTiKV) isolation={}",
        String::from_utf8_lossy(&sha.stdout).trim(),
        a.TransactionIsolation()
    );
    assert!(
        astersql_sessionctx_vardef::IsMDLEnabled(),
        "probe requires real MDL"
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let database = format!("pg12_ddl_atomic_{}", std::process::id());
    let reference = std::process::Command::new("python3")
        .arg(root.join("pkg/server/testdata/pg12_transaction_libpq.py"))
        .args([
            "--case",
            "ddl_atomic_reference",
            "--host",
            "127.0.0.1",
            "--port",
            "0",
            "--database",
            &database,
        ])
        .status()
        .expect("run independent real PG reference");
    assert!(reference.success(), "PG18.6 reference required; no skip");
    let evidence = root
        .join("target/pg12-ddl_atomic_reference-evidence")
        .join(database);
    let reference: serde_json::Value =
        serde_json::from_slice(&std::fs::read(evidence.join("reference.json")).unwrap()).unwrap();
    let identity = ["SELECT version()", "SHOW server_version_num"]
        .iter()
        .map(|sql| {
            let observed = match a.execute(sql) {
                Ok(mut sets) => {
                    let mut values = Vec::new();
                    for set in &mut sets {
                        while let Some(row) = set.next_row().unwrap() {
                            values.push(row);
                        }
                    }
                    serde_json::json!({"sql":sql, "rows":values})
                }
                Err(error) => serde_json::json!({"sql":sql,"unverified":error.to_string()}),
            };
            observed
        })
        .collect::<Vec<_>>();
    let mut observations = Vec::new();
    let mut failures = Vec::new();
    for exit in ["ROLLBACK", "COMMIT"] {
        a.execute("CREATE TABLE txddl_sentinel (id integer PRIMARY KEY,v integer NOT NULL)")
            .unwrap();
        a.execute("INSERT INTO txddl_sentinel VALUES (1,10)")
            .unwrap();
        let version_before = domain.info_schema().SchemaMetaVersion();
        let sentinel_id = domain.table_by_name("test", "txddl_sentinel").unwrap().ID;
        a.execute("BEGIN").unwrap();
        a.execute("UPDATE txddl_sentinel SET v=11 WHERE id=1")
            .unwrap();
        let observer_before = rows(&b, "SELECT id,v FROM txddl_sentinel ORDER BY id");
        assert_eq!(observer_before, vec![vec!["1".to_owned(), "10".to_owned()]]);
        assert!(mdl_blocks(&a, sentinel_id, version_before + 1));
        a.execute("CREATE TABLE txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)")
            .unwrap();
        let version_after_create = domain.info_schema().SchemaMetaVersion();
        let before_exit_object = domain.table_by_name("test", "txddl_probe").is_ok();
        let transaction_after_create = a.state.borrow().transaction.is_some();
        assert!(transaction_after_create);
        assert!(!before_exit_object);
        assert_eq!(version_after_create, version_before);
        let job_id = *a.state.borrow().pending_create_jobs.last().unwrap();
        assert_eq!(
            durable_state(&domain, job_id),
            (version_before, false, false)
        );
        let private = a.resolve_runtime_table("test", "txddl_probe").unwrap();
        assert_eq!(
            private
                .Columns
                .iter()
                .map(|c| c.Name.O.as_str())
                .collect::<Vec<_>>(),
            ["id", "v"]
        );
        assert!(
            private
                .Columns
                .iter()
                .all(|c| astersql_parser_mysql::r#type::HasNotNullFlag(c.GetFlag()))
        );
        assert!(private.PKIsHandle || private.Indices.iter().any(|i| i.Primary));
        assert!(
            b.execute("SELECT id,v FROM txddl_probe ORDER BY id")
                .is_err()
        );
        a.execute("INSERT INTO txddl_probe VALUES (1,30)").unwrap();
        let writer_data = rows(&a, "SELECT id,v FROM txddl_probe ORDER BY id");
        a.execute(exit).unwrap();
        let sentinel = rows(&b, "SELECT id,v FROM txddl_sentinel ORDER BY id");
        let persisted = durable_state(&domain, job_id);
        assert_eq!(
            persisted,
            (
                version_before + i64::from(exit == "COMMIT"),
                exit == "COMMIT",
                exit == "COMMIT"
            )
        );
        assert!(!mdl_blocks(&a, sentinel_id, version_before + 1));
        assert!(a.state.borrow().pending_create_schema.is_none());
        assert!(a.state.borrow().pending_create_jobs.is_empty());
        let table = domain.table_by_name("test", "txddl_probe").ok();
        let data = table
            .as_ref()
            .map(|_| rows(&b, "SELECT id,v FROM txddl_probe ORDER BY id"));
        let observation = serde_json::json!({"exit":exit, "sentinel":sentinel,
            "object_exists":table.is_some(), "data":data,
            "private_table_metadata":private, "committed_table_metadata":table,
            "durable_schema_version":persisted.0, "durable_history_exists":persisted.2,
            "user_transaction_after_create":transaction_after_create,
            "global_schema_after_create":version_after_create,
            "MDL_released_after_exit":!mdl_blocks(&a, sentinel_id, version_before + 1)});
        // Compare independently observed native object existence to the real
        // reference catalog, without manufacturing a local pg_class DataRow.
        let ref_case = &reference["cases"][observations.len()];
        for key in ["exit", "sentinel", "data"] {
            assert_eq!(observation[key], ref_case[key]);
        }
        assert_eq!(
            table.is_some(),
            !ref_case["directory"].as_array().unwrap().is_empty()
        );
        eprintln!(
            "exit={exit} durable={persisted:?} private_table={} MDL=held_until_exit cleanup_pending=true",
            private.ID
        );
        observations.push(observation);
        if exit == "ROLLBACK"
            && (table.is_some() || sentinel != vec![vec!["1".to_owned(), "10".to_owned()]])
        {
            failures.push("ROLLBACK must remove created object and restore sentinel");
        }
        if exit == "COMMIT"
            && (table.is_none() || data != Some(vec![vec!["1".to_owned(), "30".to_owned()]]))
        {
            failures.push("COMMIT must retain object and data");
        }
        a.execute("DROP TABLE IF EXISTS txddl_probe").unwrap();
        a.execute("DROP TABLE txddl_sentinel").unwrap();
        assert!(domain.table_by_name("test", "txddl_probe").is_err());
        assert!(domain.table_by_name("test", "txddl_sentinel").is_err());
        eprintln!("exit={exit} cleanup=objects_absent");
    }
    std::fs::write(evidence.join("aster.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "repository_sha":String::from_utf8_lossy(&sha.stdout).trim(),
        "storage":"CreateAnalyzeSession shared MVCC test store; not RealTiKV",
        "isolation":a.TransactionIsolation(), "identity":identity,
        "client":"native ConcreteSession API", "cases":observations,
        "wire_metadata":"not applicable: internal session API; public PG gate tested separately",
        "durable_history_and_schema":"checked against independent latest KV snapshots",
        "mdl":"sentinel blocks future-version job until actual transaction exit",
        "cleanup":"both objects absent after each round"
    })).unwrap()).unwrap();
    drop(bootstrap);
    assert!(failures.is_empty(), "atomicity target failed: {failures:?}");
}

#[test]
fn pg12_ddl_atomic_boundary_concurrent_catalog() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let a = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    let b = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    for session in [&a, &b] {
        session.execute("BEGIN").unwrap();
        session
            .execute("CREATE TABLE txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)")
            .unwrap();
    }
    a.execute("INSERT INTO txddl_probe VALUES (1,30)").unwrap();
    b.execute("INSERT INTO txddl_probe VALUES (2,40)").unwrap();
    a.execute("COMMIT").unwrap();
    assert!(
        b.execute("COMMIT").is_err(),
        "stale metadata allocation must conflict"
    );
    assert!(b.state.borrow().pending_create_schema.is_none());
    assert_eq!(
        rows(&a, "SELECT id,v FROM txddl_probe ORDER BY id"),
        vec![vec!["1".to_owned(), "30".to_owned()]]
    );
    a.execute("DROP TABLE txddl_probe").unwrap();
}

#[test]
fn pg12_ddl_atomic_boundary_commit_failure() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let a = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    a.execute("BEGIN").unwrap();
    a.execute("CREATE TABLE txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)")
        .unwrap();
    a.execute("INSERT INTO txddl_probe VALUES (1,30)").unwrap();
    let job = *a.state.borrow().pending_create_jobs.last().unwrap();
    a.state.borrow_mut().next_dml_commit_error = Some("deterministic probe commit failure".into());
    assert!(a.execute("COMMIT").is_err());
    assert!(!durable_state(&domain, job).1 && !durable_state(&domain, job).2);
    assert!(a.state.borrow().pending_create_schema.is_none());
    a.execute("BEGIN").unwrap();
    a.execute("CREATE TABLE txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)")
        .unwrap();
    a.execute("COMMIT").unwrap();
    a.execute("DROP TABLE txddl_probe").unwrap();
}
