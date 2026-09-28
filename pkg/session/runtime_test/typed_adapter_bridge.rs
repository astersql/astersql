// Copyright 2026 AsterSQL.

use super::*;
use astersql_executor::typed_kv_scan::KeyRange;
use std::sync::Arc;
use std::time::Duration;

struct PreparedBridgeSchemaLoader(astersql_infoschema::SchemaRef);

impl astersql_domain::InfoSchemaLoader for PreparedBridgeSchemaLoader {
    fn load_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<astersql_domain::LoadedInfoSchema, kv::errors::SharedError> {
        Ok(astersql_domain::LoadedInfoSchema::new(
            Arc::clone(&self.0),
            10,
        ))
    }

    fn load_snapshot_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
        timestamp: u64,
    ) -> Result<astersql_domain::LoadedInfoSchema, kv::errors::SharedError> {
        Ok(astersql_domain::LoadedInfoSchema::new(
            Arc::clone(&self.0),
            timestamp,
        ))
    }

    fn keyspace_exists(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        Ok(true)
    }
}

#[test]
fn detached_snapshot_source_has_an_independent_thread_safe_owner() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<crate::runtime::OwnedKVSnapshotSource>();
}

#[test]
fn session_bound_adapter_owner_uses_real_session_database_and_killer() {
    let session = concrete_session();
    let killer = session.SQLKiller();
    let owner = crate::runtime::SessionBoundAdapterOwner::new(session);
    assert_eq!(owner.CurrentDatabase(), "test");
    owner
        .KillSignal()
        .expect("unchanged SQLKiller allows execution");
    killer.SendKillSignal(1); // QueryInterrupted
    assert!(
        owner.KillSignal().is_err(),
        "real kill must stop the bound query"
    );
}

#[test]
fn session_bound_adapter_locks_encoded_record_keys_in_canonical_transaction() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::AdapterRuntime;

    let session = concrete_session();
    session
        .Execute("begin pessimistic")
        .expect("start canonical pessimistic transaction");
    let peer = ConcreteSession::new(Arc::clone(session.domain()));
    peer.Execute("set innodb_lock_wait_timeout = 0")
        .expect("set deterministic NOWAIT-like lock timeout");
    peer.Execute("begin pessimistic")
        .expect("start peer canonical transaction");
    let owner = crate::runtime::SessionBoundAdapterOwner::new(session);
    let peer = crate::runtime::SessionBoundAdapterOwner::new(peer);
    let (encoded, _) = super::planning::strict_t_multi_row(1, 10, "one", "first");
    owner
        .LockKeys(&[encoded.0.clone()], false)
        .expect("record key must be locked by canonical transaction");
    assert_eq!(owner.HeldRowLockCount(), 1);
    owner
        .LockKeys(&[encoded.0.clone()], false)
        .expect("same transaction can reuse its own row lock");
    assert_eq!(owner.HeldRowLockCount(), 1);
    assert!(
        peer.LockKeys(&[encoded.0.clone()], false).is_err(),
        "another transaction must observe the canonical row-lock conflict"
    );
    drop(owner);
    peer.LockKeys(&[encoded.0], false)
        .expect("session drop releases the real row lock");
}

#[test]
fn failed_pessimistic_select_releases_only_its_new_statement_locks() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::AdapterRuntime;

    let session = concrete_session();
    let peer = ConcreteSession::new(Arc::clone(session.domain()));
    session
        .Execute("begin pessimistic")
        .expect("begin owner txn");
    peer.Execute("begin pessimistic").expect("begin peer txn");
    let owner = crate::runtime::SessionBoundAdapterOwner::new(session);
    let peer = crate::runtime::SessionBoundAdapterOwner::new(peer);
    let (existing, _) = super::planning::strict_t_multi_row(1, 10, "one", "first");
    let (new, _) = super::planning::strict_t_multi_row(2, 20, "two", "second");
    owner
        .LockKeys(&[existing.0.clone()], false)
        .expect("existing transaction lock");
    owner
        .OnPessimisticStmtStart()
        .expect("begin locking statement");
    owner
        .LockKeys(&[new.0.clone()], false)
        .expect("new statement lock");
    owner
        .OnPessimisticStmtEnd(false)
        .expect("rollback failed statement locks");
    assert_eq!(owner.HeldRowLockCount(), 1);
    peer.LockKeys(&[new.0], false)
        .expect("rolled-back row can be locked by peer");
    assert_eq!(peer.HeldRowLockCount(), 1);
    assert!(
        owner.LockKeys(&[existing.0], false).is_ok(),
        "preexisting transaction lock remains owned"
    );
}

#[test]
fn session_bound_adapter_maximum_time_counts_time_before_executor_build() {
    use astersql_executor::adapter::AdapterRuntime;
    use std::time::SystemTime;

    let session = concrete_session();
    let killer = session.SQLKiller();
    let owner = crate::runtime::SessionBoundAdapterOwner::new(session);
    owner.SetProcessInfo(
        "select slow table scan",
        SystemTime::now() - Duration::from_millis(50),
        3,
        10,
    );
    let signal = AdapterRuntime::KillSignal(&owner);
    assert!(
        signal.is_err(),
        "time spent before build counts toward max_execution_time; signal={} max_var={}",
        killer.GetKillSignal(),
        owner.MaximumExecutionTime(),
    );
}

#[test]
fn session_bound_adapter_cancels_completed_query_deadline() {
    use astersql_executor::adapter::AdapterRuntime;
    use std::time::SystemTime;

    let owner = crate::runtime::SessionBoundAdapterOwner::new(concrete_session());
    owner.SetProcessInfo("select fast table scan", SystemTime::now(), 3, 20);
    owner.CancelMaximumExecutionTime();
    std::thread::sleep(Duration::from_millis(30));
    owner
        .KillSignal()
        .expect("finished query must not kill reused session");
}

#[test]
fn session_bound_adapter_deadline_kills_idle_result_set_without_next_polling() {
    use astersql_executor::adapter::AdapterRuntime;
    use std::time::SystemTime;

    let session = concrete_session();
    let killer = session.SQLKiller();
    let owner = crate::runtime::SessionBoundAdapterOwner::new(session);
    owner.SetProcessInfo("select blocked table scan", SystemTime::now(), 3, 20);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        killer.GetKillSignal(),
        astersql_util_sqlkiller::sqlkiller::MaxExecTimeExceeded
    );
    owner.CancelMaximumExecutionTime();
}

#[test]
fn session_bound_adapter_audit_calls_ready_plugin_with_canonical_session_view() {
    use astersql_executor::adapter::AdapterRuntime;
    use astersql_plugin as plugin;
    use std::sync::Mutex;

    struct PluginCleanup(plugin::Context);
    impl Drop for PluginCleanup {
        fn drop(&mut self) {
            plugin::shutdown(&self.0);
            plugin::clear_static_plugins();
        }
    }
    let context = plugin::Context::default();
    plugin::shutdown(&context);
    plugin::clear_static_plugins();
    let _cleanup = PluginCleanup(context.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let collected = Arc::clone(&events);
    let manifest = plugin::AuditManifest {
        manifest: plugin::Manifest::new(plugin::Kind::Audit, "adapterbridge", 1),
        on_general_event: Some(Arc::new(move |context, vars, event, command| {
            let vars = vars.expect("canonical session view");
            if vars.original_sql != "select audit_adapter_bridge" {
                return;
            }
            assert!(context.value(plugin::EXEC_START_TIME_CONTEXT_KEY).is_some());
            collected.lock().unwrap().push((
                event,
                vars.original_sql.clone(),
                command.to_owned(),
                vars.tables.clone(),
            ));
        })),
        ..Default::default()
    };
    let exported = plugin::export_manifest(&manifest);
    plugin::register_static_plugin("adapterbridge", Arc::new(move || exported.clone()))
        .expect("register actual audit plugin");
    let config = plugin::Config {
        plugins: vec!["adapterbridge-1".into()],
        ..Default::default()
    };
    plugin::load(&context, &config).expect("load audit plugin");
    plugin::init(&context, &config).expect("ready audit plugin");
    let session = concrete_session();
    let info_schema = session.domain().info_schema();
    let system_tables = info_schema
        .SchemaTableInfos(&astersql_infoschema::infoschema::CiString::new("mysql"))
        .expect("canonical mysql table metadata");
    let table_id = system_tables.first().expect("mysql has a table").id;
    let table = info_schema.TableItemByID(table_id).expect("table location");
    let expected_table = plugin::TableEntry {
        db: table.DBName.original,
        table: table.TableName.original,
    };
    let owner = crate::runtime::SessionBoundAdapterOwner::new(session);
    owner.BindTypedScan(crate::runtime::TypedScanSpec {
        table_id,
        pk_is_handle: false,
        descending: false,
        columns: Vec::new(),
        ranges: Vec::new(),
        version: kv::MinVersion,
        initial_capacity: 1,
        maximum_chunk_size: 1,
    });
    owner.Audit("select audit_adapter_bridge");
    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].0, plugin::GeneralEvent::Completed);
    assert_eq!(captured[0].1, "select audit_adapter_bridge");
    assert_eq!(captured[0].2, "Sleep");
    assert_eq!(captured[0].3, vec![expected_table]);
}

#[test]
fn canonical_session_typed_scan_is_lazy_and_detach_survives_session_drop() {
    let session = concrete_session();
    let mut transaction = session
        .domain()
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("begin seed transaction");
    for (handle, a, b, c) in [(1, 10, "one", "first"), (2, 20, "two", "second")] {
        let (key, value) = super::planning::strict_t_multi_row(handle, a, b, c);
        transaction.Set(key, value).expect("seed encoded row");
    }
    transaction
        .Commit(&kv::Context::default())
        .expect("commit encoded rows");
    let version = session
        .domain()
        .storage()
        .with_storage(|storage| storage.CurrentVersion("global"))
        .expect("read pinned version");
    let columns = vec![
        astersql_meta_model::ColumnInfo {
            ID: 1,
            Name: astersql_parser_ast::NewCIStr("a"),
            FieldType: astersql_parser_types::NewFieldType(
                astersql_parser_mysql::r#type::TypeLonglong,
            ),
            ..Default::default()
        },
        astersql_meta_model::ColumnInfo {
            ID: 2,
            Name: astersql_parser_ast::NewCIStr("b"),
            FieldType: astersql_parser_types::NewFieldType(
                astersql_parser_mysql::r#type::TypeVarchar,
            ),
            ..Default::default()
        },
        astersql_meta_model::ColumnInfo {
            ID: 3,
            Name: astersql_parser_ast::NewCIStr("c"),
            FieldType: astersql_parser_types::NewFieldType(
                astersql_parser_mysql::r#type::TypeVarchar,
            ),
            ..Default::default()
        },
    ];
    let start = kv::Key(astersql_tablecodec::GenTableRecordPrefix(88).0);
    let end = start.PrefixNext();
    let mut executor = session.OpenTypedKVSnapshotScan(
        88,
        false,
        false,
        columns,
        vec![KeyRange { start, end }],
        version,
        1,
        1,
    );
    executor.Open().expect("open without reading KV");
    let mut output = executor.NewChunk();
    executor.Next(&mut output).expect("read first typed page");
    assert_eq!(output.GetRow(0).GetInt64(0), 10);
    let mut detached = executor.Detach().expect("scan supports detach");
    executor.Close().expect("close original");
    drop(executor);
    drop(session);
    let mut independent = detached.NewChunk();
    detached
        .Next(&mut independent)
        .expect("read pinned page after session drop");
    assert_eq!(independent.GetRow(0).GetInt64(0), 20);
    detached.Close().expect("close detached");
}

#[test]
fn session_bound_adapter_accepts_a_canonical_physical_limit_root() {
    use astersql_executor::adapter::{AdapterRuntime, PlanInfo, PlanKind, SchemaColumn};

    let session = concrete_session();
    let (key, value) = super::planning::strict_t_multi_row(1, 10, "one", "first");
    let mut seed = session
        .domain()
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("seed real KV");
    seed.Set(key, value).expect("write codec row");
    seed.Commit(&kv::Context::default()).expect("commit row");
    let version = session
        .domain()
        .storage()
        .with_storage(|storage| storage.CurrentVersion("global"))
        .expect("pin read version");
    let context = session.AdapterPlanContext();
    let mut scan =
        astersql_planner_core_operator_physicalop::PhysicalTableScan::New(context.clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 88,
        Name: astersql_parser_ast::NewCIStr("t"),
        ..Default::default()
    });
    let field_type =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
    scan.Columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("a"),
        FieldType: field_type.clone(),
        ..Default::default()
    }];
    let mut limit = astersql_planner_core_operator_physicalop::PhysicalLimit::New(context, 0, 1);
    use astersql_planner_core_base::PhysicalPlan as _;
    limit.set_children(vec![Box::new(scan)]);
    let start = kv::Key(astersql_tablecodec::GenTableRecordPrefix(88).0);
    let end = start.PrefixNext();
    let owner = crate::runtime::SessionBoundAdapterOwner::new(session);
    owner
        .BindTypedPhysicalPlan(
            Box::new(limit),
            vec![KeyRange { start, end }],
            version,
            1,
            1,
        )
        .expect("bind optimizer root with one table scan");
    let plan = PlanInfo {
        id: 42,
        kind: PlanKind::Query,
        schema: vec![SchemaColumn { field_type }],
        calculate_no_delay: false,
        projection_child: None,
        encoded: "limit -> table scan".into(),
        binary: String::new(),
        hints: String::new(),
    };
    let mut executor = owner
        .BuildExecutor(&plan, None)
        .expect("build typed physical root through adapter runtime");
    executor.Open().expect("open lazily");
    let mut output = executor.NewChunk();
    executor.Next(&mut output).expect("read bounded typed row");
    assert_eq!(output.GetRow(0).GetInt64(0), 10);
    executor.Next(&mut output).expect("LIMIT EOF");
    assert_eq!(output.NumRows(), 0);
    executor.Close().expect("close owned KV executor");
}

#[test]
fn canonical_prepared_exec_stmt_binds_limit_and_streams_rows_then_restores_plan_cache() {
    use astersql_executor::adapter::{
        ExecStmt, FieldName, PlanInfo, PlanKind, Priority, SchemaColumn, StatementContext,
        StatementKind, StatementNode,
    };

    fn prepared_stmt(owner: Arc<crate::runtime::SessionBoundAdapterOwner>) -> ExecStmt {
        let execute = "execute prepared_limit";
        ExecStmt {
            GoCtx: None,
            InfoSchema: 0,
            Plan: PlanInfo {
                id: 42,
                kind: PlanKind::Query,
                schema: vec![SchemaColumn {
                    field_type: astersql_parser_types::NewFieldType(
                        astersql_parser_mysql::r#type::TypeLonglong,
                    ),
                }],
                calculate_no_delay: false,
                projection_child: None,
                encoded: "prepared physical SELECT".into(),
                binary: String::new(),
                hints: String::new(),
            },
            StmtNode: StatementNode {
                kind: StatementKind::Execute,
                original_text: execute.into(),
                text: execute.into(),
                secure_text: execute.into(),
                prepared_text: None,
            },
            Ctx: owner,
            LowerPriority: false,
            isPreparedStmt: true,
            isSelectForUpdate: false,
            retryCount: 0,
            retryStartTime: None,
            phaseBuildDurations: [Duration::ZERO; 2],
            phaseOpenDurations: [Duration::ZERO; 2],
            phaseNextDurations: [Duration::ZERO; 2],
            phaseLockDurations: [Duration::ZERO; 2],
            OutputNames: vec![FieldName {
                column_name: "a".into(),
                ..Default::default()
            }],
            PsStmt: None,
            Ti: None,
            StatementCtx: StatementContext {
                priority: Priority::Unspecified,
                statement_type: "Execute".into(),
                sql_normalized: execute.into(),
                ..Default::default()
            },
        }
    }

    fn physical_limits(plan: &dyn astersql_planner_core_base::PhysicalPlan) -> Vec<(u64, u64)> {
        use astersql_planner_core_operator_physicalop::{PhysicalLimit, PhysicalTableReader};
        if let Some(limit) = plan.as_any().downcast_ref::<PhysicalLimit>() {
            let mut values = vec![(limit.Offset, limit.Count)];
            for child in plan.children() {
                values.extend(physical_limits(child));
            }
            return values;
        }
        if let Some(reader) = plan.as_any().downcast_ref::<PhysicalTableReader>() {
            return reader.GetTablePlan().map_or_else(Vec::new, physical_limits);
        }
        plan.children()
            .into_iter()
            .flat_map(physical_limits)
            .collect()
    }

    let column = astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("a"),
        Offset: 0,
        State: astersql_meta_model::StatePublic,
        FieldType: astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong),
        ..Default::default()
    };
    let model = Arc::new(astersql_meta_model::TableInfo {
        ID: 88,
        Name: astersql_parser_ast::NewCIStr("t_prepared_limit"),
        Columns: vec![column.clone()],
        ..Default::default()
    });
    let schema: astersql_infoschema::SchemaRef =
        astersql_infoschema::infoschema::MockInfoSchema(vec![
            astersql_infoschema::infoschema::TableInfo {
                id: 88,
                name: astersql_infoschema::infoschema::CiString::new("t_prepared_limit"),
                columns: vec![astersql_infoschema::infoschema::ColumnInfo {
                    id: 1,
                    name: astersql_infoschema::infoschema::CiString::new("a"),
                    ..Default::default()
                }],
                model_meta: Some(model),
                ..Default::default()
            },
        ]);
    let domain = Arc::new(astersql_domain::Domain::new_mock(
        Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).expect("create KV storage"))
            .unwrap_or_else(|_| panic!("unexpected storage owner")),
        Arc::new(PreparedBridgeSchemaLoader(Arc::clone(&schema))),
    ));
    domain.init().expect("initialize canonical domain");
    let session = ConcreteSession::new(Arc::clone(&domain));
    let mut seed = domain
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("begin real KV seed transaction");
    for (handle, a) in [(1, 10), (2, 20), (3, 30)] {
        let (key, value) = super::planning::strict_t_multi_row(handle, a, "b", "c");
        seed.Set(key, value).expect("seed encoded record");
    }
    seed.Commit(&kv::Context::default())
        .expect("commit canonical records");
    let info_schema = session.domain().info_schema();
    let statement_id = session
        .PreparePlannedKVSelect("select a from t_prepared_limit limit ?", info_schema)
        .expect("prepare canonical SELECT");
    let offset_statement_id = session
        .PreparePlannedKVSelect(
            "select a from t_prepared_limit limit ?, ?",
            session.domain().info_schema(),
        )
        .expect("prepare canonical OFFSET/COUNT SELECT");
    let owner = Arc::new(crate::runtime::SessionBoundAdapterOwner::new(session));
    for invalid in [
        astersql_types::datum::NewIntDatum(-1),
        astersql_types::datum::NewStringDatum("1.1".to_owned()),
    ] {
        let error = owner
            .BindPreparedPlannedKVSelect(statement_id, &[invalid], 1, 1)
            .expect_err("prepared LIMIT requires a non-negative integer");
        assert!(error.to_string().contains("Incorrect arguments to LIMIT"));
    }
    owner
        .BindPreparedPlannedKVSelect(statement_id, &[astersql_types::datum::NewIntDatum(2)], 1, 1)
        .expect("bind first parameterized physical plan");
    assert!(!owner.LastPlanFromCache());
    let mut first = prepared_stmt(owner.clone());
    let mut result = first
        .Exec()
        .expect("execute canonical prepared physical plan")
        .expect("lazy typed result set");
    assert!(
        Arc::ptr_eq(
            first
                .GoCtx
                .as_ref()
                .and_then(|context| context.sql_killer.as_ref())
                .expect("canonical execution context carries SQL killer"),
            &astersql_executor::adapter::AdapterRuntime::SQLKillerHandle(owner.as_ref())
                .expect("owner SQL killer"),
        ),
        "typed result must observe the same canonical session kill signal"
    );
    let (detached, detachable) = result.TryDetach().expect("inspect prepared Detach");
    assert!(
        detachable,
        "bound LIMIT and owned scan can detach independently"
    );
    let mut detached = detached.expect("detached prepared result set");
    assert_eq!(first.StmtNode.kind, StatementKind::Select);
    assert_eq!(
        first.StmtNode.prepared_text.as_deref(),
        Some("select a from t_prepared_limit limit ?")
    );
    let mut output = result.NewChunk();
    for value in [10, 20] {
        result.Next(&mut output).expect("fetch prepared typed page");
        assert_eq!(output.NumRows(), 1);
        assert_eq!(output.GetRow(0).GetInt64(0), value);
    }
    result.Next(&mut output).expect("prepared LIMIT EOF");
    assert_eq!(output.NumRows(), 0);
    result
        .Close()
        .expect("admit cache after prepared execution");
    owner
        .BindPreparedPlannedKVSelect(statement_id, &[astersql_types::datum::NewIntDatum(1)], 1, 1)
        .expect("restore cached plan with changed LIMIT parameter");
    assert!(owner.LastPlanFromCache());
    let mut second = prepared_stmt(owner.clone());
    let mut cached = second
        .Exec()
        .expect("execute restored canonical plan")
        .expect("cached typed result set");
    cached.Next(&mut output).expect("fetch one rebound row");
    assert_eq!(output.GetRow(0).GetInt64(0), 10);
    cached.Next(&mut output).expect("rebound LIMIT EOF");
    assert_eq!(output.NumRows(), 0);
    cached.Close().expect("close cached prepared result");
    for (offset, expected, cached_plan) in [(1, 20, false), (2, 30, true), (0, 10, true)] {
        let parameters = [
            astersql_types::datum::NewIntDatum(offset),
            astersql_types::datum::NewIntDatum(1),
        ];
        let inspected = owner
            .PlanPreparedPlannedKVSelect(offset_statement_id, &parameters)
            .expect("inspect canonical bound OFFSET/COUNT plan");
        assert!(
            physical_limits(inspected.Plan.as_ref()).contains(&(offset as u64, 1)),
            "bound physical LIMIT nodes: {:?}",
            physical_limits(inspected.Plan.as_ref())
        );
        assert!(
            physical_limits(inspected.Plan.as_ref()).contains(&(0, (offset + 1) as u64)),
            "cop LIMIT must keep enough rows for bound OFFSET: {:?}",
            physical_limits(inspected.Plan.as_ref())
        );
        owner
            .BindPreparedPlannedKVSelect(offset_statement_id, &parameters, 1, 1)
            .expect("bind canonical LIMIT offset and count");
        assert_eq!(owner.LastPlanFromCache(), cached_plan);
        let mut offset_stmt = prepared_stmt(owner.clone());
        let mut offset_result = offset_stmt
            .Exec()
            .expect("execute bound OFFSET plan")
            .expect("typed OFFSET result");
        offset_result.Next(&mut output).expect("fetch offset row");
        assert_eq!(output.GetRow(0).GetInt64(0), expected);
        offset_result.Next(&mut output).expect("OFFSET/COUNT EOF");
        assert_eq!(output.NumRows(), 0);
        offset_result
            .Close()
            .expect("admit or finish cached OFFSET plan");
    }
    drop(cached);
    drop(second);
    drop(result);
    drop(first);
    drop(owner);
    drop(domain);
    detached
        .Next(&mut output)
        .expect("detached prepared page after session drop");
    assert_eq!(output.GetRow(0).GetInt64(0), 10);
    detached
        .Next(&mut output)
        .expect("detached second bound page");
    assert_eq!(output.GetRow(0).GetInt64(0), 20);
    detached.Close().expect("close independent prepared scan");
}

#[test]
fn canonical_pessimistic_select_for_update_locks_scanned_record_before_returning_rows() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::{
        AdapterRuntime, ExecStmt, FieldName, PlanInfo, PlanKind, Priority, SchemaColumn,
        StatementContext, StatementKind, StatementNode,
    };

    let session = concrete_session();
    let (key, value) = super::planning::strict_t_multi_row(1, 10, "one", "first");
    let mut seed = session
        .domain()
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("seed canonical KV transaction");
    seed.Set(key.clone(), value).expect("write encoded row");
    seed.Commit(&kv::Context::default()).expect("commit seed");
    let peer = ConcreteSession::new(Arc::clone(session.domain()));
    peer.Execute("set innodb_lock_wait_timeout = 0")
        .expect("set immediate conflict timeout");
    peer.Execute("begin pessimistic")
        .expect("start competing transaction");
    session
        .Execute("begin pessimistic")
        .expect("start SELECT FOR UPDATE transaction");
    let version = session
        .domain()
        .storage()
        .with_storage(|storage| storage.CurrentVersion("global"))
        .expect("pin canonical read version");
    let mut physical = astersql_planner_core_operator_physicalop::PhysicalTableScan::New(
        session.AdapterPlanContext(),
    );
    physical.Table = Some(astersql_meta_model::TableInfo {
        ID: 88,
        Name: astersql_parser_ast::NewCIStr("t"),
        ..Default::default()
    });
    let field_type =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
    physical.Columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("a"),
        FieldType: field_type.clone(),
        ..Default::default()
    }];
    let start = kv::Key(astersql_tablecodec::GenTableRecordPrefix(88).0);
    let end = start.PrefixNext();
    let owner = Arc::new(crate::runtime::SessionBoundAdapterOwner::new(session));
    owner
        .BindPhysicalTableScan(physical, vec![KeyRange { start, end }], version, 1, 1)
        .expect("bind canonical plain table scan");
    let peer = crate::runtime::SessionBoundAdapterOwner::new(peer);
    let sql = "select a from t for update";
    let mut stmt = ExecStmt {
        GoCtx: None,
        InfoSchema: 0,
        Plan: PlanInfo {
            id: 42,
            kind: PlanKind::Query,
            schema: vec![SchemaColumn { field_type }],
            calculate_no_delay: false,
            projection_child: None,
            encoded: "table scan t".into(),
            binary: String::new(),
            hints: String::new(),
        },
        StmtNode: StatementNode {
            kind: StatementKind::Select,
            original_text: sql.into(),
            text: sql.into(),
            secure_text: sql.into(),
            prepared_text: None,
        },
        Ctx: owner.clone(),
        LowerPriority: false,
        isPreparedStmt: false,
        isSelectForUpdate: true,
        retryCount: 0,
        retryStartTime: None,
        phaseBuildDurations: [Duration::ZERO; 2],
        phaseOpenDurations: [Duration::ZERO; 2],
        phaseNextDurations: [Duration::ZERO; 2],
        phaseLockDurations: [Duration::ZERO; 2],
        OutputNames: vec![FieldName {
            column_name: "a".into(),
            ..Default::default()
        }],
        PsStmt: None,
        Ti: None,
        StatementCtx: StatementContext {
            priority: Priority::Unspecified,
            statement_type: "Select".into(),
            sql_normalized: sql.into(),
            ..Default::default()
        },
    };
    let mut result = stmt
        .Exec()
        .expect("lock and execute canonical table scan")
        .expect("return Go-style buffered locking result set");
    assert_eq!(owner.HeldRowLockCount(), 1);
    assert!(peer.LockKeys(&[key.0.clone()], false).is_err());
    let mut batch = result.NewChunk();
    result.Next(&mut batch).expect("read already locked row");
    assert_eq!(batch.GetRow(0).GetInt64(0), 10);
    result.Close().expect("finish locking result set");
    drop(result);
    drop(stmt);
    drop(owner);
    peer.LockKeys(&[key.0], false)
        .expect("canonical transaction releases lock on session drop");
}

#[test]
fn canonical_session_exec_stmt_streams_typed_rows_and_finishes_statement() {
    use astersql_executor::adapter::{
        ExecStmt, FieldName, PlanInfo, PlanKind, Priority, SchemaColumn, StatementContext,
        StatementKind, StatementNode,
    };

    let session = concrete_session();
    let mut transaction = session
        .domain()
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("begin real KV transaction");
    for (handle, a) in [(1, 10), (2, 20)] {
        let (key, value) = super::planning::strict_t_multi_row(handle, a, "b", "c");
        transaction.Set(key, value).expect("seed typed row");
    }
    transaction
        .Commit(&kv::Context::default())
        .expect("commit typed rows");
    let version = session
        .domain()
        .storage()
        .with_storage(|storage| storage.CurrentVersion("global"))
        .expect("pin MVCC version");
    let domain = Arc::clone(session.domain());
    let columns = vec![
        astersql_meta_model::ColumnInfo {
            ID: 1,
            Name: astersql_parser_ast::NewCIStr("a"),
            FieldType: astersql_parser_types::NewFieldType(
                astersql_parser_mysql::r#type::TypeLonglong,
            ),
            ..Default::default()
        },
        astersql_meta_model::ColumnInfo {
            ID: 2,
            Name: astersql_parser_ast::NewCIStr("b"),
            FieldType: astersql_parser_types::NewFieldType(
                astersql_parser_mysql::r#type::TypeVarchar,
            ),
            ..Default::default()
        },
        astersql_meta_model::ColumnInfo {
            ID: 3,
            Name: astersql_parser_ast::NewCIStr("c"),
            FieldType: astersql_parser_types::NewFieldType(
                astersql_parser_mysql::r#type::TypeVarchar,
            ),
            ..Default::default()
        },
    ];
    let start = kv::Key(astersql_tablecodec::GenTableRecordPrefix(88).0);
    let end = start.PrefixNext();
    let mut physical = astersql_planner_core_operator_physicalop::PhysicalTableScan::New(
        session.AdapterPlanContext(),
    );
    physical.Table = Some(astersql_meta_model::TableInfo {
        ID: 88,
        Name: astersql_parser_ast::NewCIStr("t"),
        ..Default::default()
    });
    physical.Columns = columns;
    let owner = Arc::new(crate::runtime::SessionBoundAdapterOwner::new(session));
    owner
        .BindPhysicalTableScan(physical, vec![KeyRange { start, end }], version, 1, 1)
        .expect("bind canonical physical table scan");
    let sql = "select a,b,c from t";
    let plan = PlanInfo {
        id: 42,
        kind: PlanKind::Query,
        schema: [
            astersql_parser_mysql::r#type::TypeLonglong,
            astersql_parser_mysql::r#type::TypeVarchar,
            astersql_parser_mysql::r#type::TypeVarchar,
        ]
        .map(|type_code| SchemaColumn {
            field_type: astersql_parser_types::NewFieldType(type_code),
        })
        .to_vec(),
        calculate_no_delay: false,
        projection_child: None,
        encoded: "table scan t".into(),
        binary: String::new(),
        hints: String::new(),
    };
    let mut stmt = ExecStmt {
        GoCtx: None,
        InfoSchema: 0,
        Plan: plan,
        StmtNode: StatementNode {
            kind: StatementKind::Select,
            original_text: sql.into(),
            text: sql.into(),
            secure_text: sql.into(),
            prepared_text: None,
        },
        Ctx: owner.clone(),
        LowerPriority: false,
        isPreparedStmt: false,
        isSelectForUpdate: false,
        retryCount: 0,
        retryStartTime: None,
        phaseBuildDurations: [Duration::ZERO; 2],
        phaseOpenDurations: [Duration::ZERO; 2],
        phaseNextDurations: [Duration::ZERO; 2],
        phaseLockDurations: [Duration::ZERO; 2],
        OutputNames: ["a", "b", "c"]
            .map(|name| FieldName {
                column_name: name.into(),
                ..Default::default()
            })
            .to_vec(),
        PsStmt: None,
        Ti: None,
        StatementCtx: StatementContext {
            priority: Priority::Unspecified,
            statement_type: "Select".into(),
            sql_normalized: sql.into(),
            ..Default::default()
        },
    };
    let mut result = stmt
        .Exec()
        .expect("execute through production runtime")
        .expect("streaming result set");
    assert_eq!(
        result.Fields()[0].field_type.GetType(),
        astersql_parser_mysql::r#type::TypeLonglong
    );
    let mut batch = result.NewChunk();
    result.Next(&mut batch).expect("first typed page");
    assert_eq!(batch.GetRow(0).GetInt64(0), 10);
    result.OnFetchReturned();
    let (detached, detachable) = result.TryDetach().expect("detach original result set");
    assert!(detachable);
    let mut detached = detached.expect("independent typed result set");
    result.Next(&mut batch).expect("second typed page");
    assert_eq!(batch.GetRow(0).GetInt64(0), 20);
    result.Next(&mut batch).expect("end of stream");
    assert_eq!(batch.NumRows(), 0);
    std::thread::sleep(Duration::from_millis(310));
    result.Close().expect("finish statement");
    let effects = owner.Effects();
    assert_eq!(effects.last_found_rows, 2);
    assert_eq!(owner.LastFoundRows(), 2);
    assert_eq!(effects.audited_sql, vec![sql]);
    assert_eq!(effects.top_sql_started, 1);
    assert_eq!(effects.top_sql_finished, 1);
    assert_eq!(effects.slow_queries.len(), 2);
    assert_eq!(effects.summaries, vec![sql]);
    assert!(
        domain
            .show_slow_queries(astersql_domain::domain::SlowQueryKind::Recent)
            .iter()
            .any(|query| query.sql == sql)
    );
    stmt.isPreparedStmt = true;
    assert!(
        stmt.Exec().is_err(),
        "read-only typed scan runtime must reject unbound prepared execution"
    );
    stmt.isPreparedStmt = false;
    stmt.isSelectForUpdate = true;
    assert!(
        stmt.Exec().is_err(),
        "read-only typed scan runtime must reject locking SELECT FOR UPDATE"
    );
    drop(result);
    drop(stmt);
    drop(owner);
    drop(domain);
    let mut detached_batch = detached.NewChunk();
    detached
        .Next(&mut detached_batch)
        .expect("detached second page after canonical session drops");
    assert_eq!(detached_batch.GetRow(0).GetInt64(0), 20);
    detached.Close().expect("close independent result set");
}
