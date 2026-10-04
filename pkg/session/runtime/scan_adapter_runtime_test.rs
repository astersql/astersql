// Copyright 2026 AsterSQL.

use astersql_executor::adapter::AdapterRuntime;

use super::SessionBoundAdapterOwner;

use astersql_domain::{Domain, InfoSchemaLoader, LoadedInfoSchema};
use astersql_infoschema::{self as infoschema, SchemaRef};
use astersql_kv as kv;
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage};
use std::sync::Arc;

struct DMLBridgeSchemaLoader(SchemaRef);

impl InfoSchemaLoader for DMLBridgeSchemaLoader {
    fn load_info_schema(
        &self,
        _: &dyn kv::Storage,
        _: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        Ok(LoadedInfoSchema::new(Arc::clone(&self.0), 10))
    }
    fn load_snapshot_info_schema(
        &self,
        _: &dyn kv::Storage,
        _: &str,
        timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        Ok(LoadedInfoSchema::new(Arc::clone(&self.0), timestamp))
    }
    fn keyspace_exists(
        &self,
        _: &dyn kv::Storage,
        _: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        Ok(true)
    }
}

fn canonical_dml_session() -> crate::runtime::ConcreteSession {
    let column = |id, name: &str, offset| {
        let mut field_type =
            astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
        if id == 1 {
            field_type.AddFlag(astersql_parser_mysql::r#type::PriKeyFlag);
        }
        astersql_meta_model::ColumnInfo {
            ID: id,
            Name: astersql_parser_ast::NewCIStr(name),
            Offset: offset,
            State: astersql_meta_model::StatePublic,
            FieldType: field_type,
            ..Default::default()
        }
    };
    let columns = vec![column(1, "a", 0), column(2, "b", 1)];
    let model = Arc::new(astersql_meta_model::TableInfo {
        ID: 123,
        Name: astersql_parser_ast::NewCIStr("t"),
        Columns: columns.clone(),
        PKIsHandle: true,
        ..Default::default()
    });
    let schema = infoschema::infoschema::MockInfoSchema(vec![infoschema::infoschema::TableInfo {
        id: model.ID,
        name: infoschema::infoschema::CiString::new("t"),
        columns: columns
            .iter()
            .map(|column| infoschema::infoschema::ColumnInfo {
                id: column.ID,
                name: infoschema::infoschema::CiString::new(&column.Name.O),
                ..Default::default()
            })
            .collect(),
        model_meta: Some(model),
        ..Default::default()
    }]);
    let store = Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).unwrap())
        .unwrap_or_else(|_| panic!("unexpected storage owner"));
    let domain = Arc::new(Domain::new_mock(
        store,
        Arc::new(DMLBridgeSchemaLoader(schema)),
    ));
    domain.init().unwrap();
    crate::runtime::ConcreteSession::new(domain)
}

#[test]
fn pessimistic_dml_adapter_uses_canonical_transaction_write_and_unchanged_key_state() {
    use crate::testutil::TestSession;
    let session = canonical_dml_session();
    session
        .Execute("begin pessimistic")
        .expect("begin canonical pessimistic transaction");
    let owner = SessionBoundAdapterOwner::new(session);
    let mut transaction = owner
        .PessimisticTransaction()
        .expect("borrow active transaction without activating it");
    assert!(transaction.IsValid());
    assert!(transaction.KeysNeedToLock().unwrap().is_empty());
    owner
        .session
        .Execute("insert into t values (1,10)")
        .expect("write canonical transaction mutations");
    let keys = transaction
        .KeysNeedToLock()
        .expect("collect statement mutation lock keys");
    assert!(!keys.is_empty());
    for key in &keys {
        assert!(transaction.IsKeyWritten(key).unwrap());
    }
    owner
        .session
        .WithSessionVars(|vars| vars.TxnCtx.AddUnchangedKeyForLock(b"shared", true));
    owner
        .session
        .WithSessionVars(|vars| vars.TxnCtx.AddUnchangedKeyForLock(b"exclusive", false));
    assert_eq!(
        owner.CollectUnchangedKeysForXLock(Vec::new()),
        vec![b"exclusive".to_vec()]
    );
    assert_eq!(
        owner.CollectUnchangedKeysForSLock(Vec::new()),
        vec![b"shared".to_vec()]
    );
    owner.ResetUnchangedKeysForLock();
    assert!(owner.CollectUnchangedKeysForXLock(Vec::new()).is_empty());
    assert!(owner.CollectUnchangedKeysForSLock(Vec::new()).is_empty());
    let table_id = keys
        .iter()
        .map(|key| astersql_tablecodec::DecodeTableID(astersql_tablecodec::kv::Key(key.clone())))
        .find(|id| *id != 0)
        .expect("canonical mutation has a table key");
    owner
        .session
        .state
        .borrow_mut()
        .local_temporary_tables
        .insert(
            ("test".into(), "temp".into()),
            astersql_meta_model::TableInfo {
                ID: 999,
                ..Default::default()
            },
        );
    let real_key = astersql_tablecodec::EncodeRowKeyWithHandle(
        table_id,
        Box::new(astersql_kv::IntHandle(1000)),
    )
    .0;
    let temp_key =
        astersql_tablecodec::EncodeRowKeyWithHandle(999, Box::new(astersql_kv::IntHandle(1000))).0;
    let previous_locks = owner.HeldRowLockCount();
    owner
        .LockKeys(&[real_key.clone(), temp_key], false)
        .expect("filter temporary table key");
    assert_eq!(owner.HeldRowLockCount(), previous_locks + 1);
    owner
        .session
        .WithSessionVars(|vars| vars.StmtCtx.InsertLogicalPlanLockTableID(table_id));
    let other_key =
        astersql_tablecodec::EncodeRowKeyWithHandle(998, Box::new(astersql_kv::IntHandle(1000))).0;
    owner
        .LockKeys(&[real_key, other_key], false)
        .expect("filter table outside LockTableIDs");
    assert_eq!(owner.HeldRowLockCount(), previous_locks + 1);
    let shared_key =
        astersql_tablecodec::EncodeRowKeyWithHandle(table_id, Box::new(kv::IntHandle(2000))).0;
    owner
        .LockKeys(&[shared_key], true)
        .expect("collect shared lock details");
    let details = owner
        .session
        .WithSessionVars(|vars| vars.StmtCtx.GetExecDetails());
    assert_eq!(details.LockKeysDetail.as_ref().unwrap().LockKeys, 2);
    assert_eq!(details.SharedLockKeysDetail.as_ref().unwrap().LockKeys, 1);
}

fn dml_stmt(
    owner: Arc<SessionBoundAdapterOwner>,
    sql: &str,
    kind: astersql_executor::adapter::PlanKind,
) -> astersql_executor::adapter::ExecStmt {
    use astersql_executor::adapter::{
        ExecStmt, PlanInfo, StatementContext, StatementKind, StatementNode,
    };
    let statement_kind = match kind {
        astersql_executor::adapter::PlanKind::Insert => StatementKind::Insert,
        astersql_executor::adapter::PlanKind::Update => StatementKind::Update,
        astersql_executor::adapter::PlanKind::Delete => StatementKind::Delete,
        _ => unreachable!(),
    };
    ExecStmt {
        GoCtx: None,
        InfoSchema: 0,
        Plan: PlanInfo {
            id: 42,
            kind,
            schema: Vec::new(),
            calculate_no_delay: true,
            projection_child: None,
            encoded: String::new(),
            binary: String::new(),
            hints: String::new(),
        },
        TypedPlan: None,
        StmtNode: StatementNode {
            kind: statement_kind,
            original_text: sql.into(),
            text: sql.into(),
            secure_text: sql.into(),
            prepared_text: None,
        },
        Ctx: owner,
        LowerPriority: false,
        isPreparedStmt: false,
        isSelectForUpdate: false,
        retryCount: 0,
        retryStartTime: None,
        phaseBuildDurations: [std::time::Duration::ZERO; 2],
        phaseOpenDurations: [std::time::Duration::ZERO; 2],
        phaseNextDurations: [std::time::Duration::ZERO; 2],
        phaseLockDurations: [std::time::Duration::ZERO; 2],
        OutputNames: Vec::new(),
        PsStmt: None,
        Ti: None,
        StatementCtx: StatementContext {
            statement_type: format!("{statement_kind:?}"),
            sql_normalized: sql.into(),
            ..Default::default()
        },
    }
}

fn point_lock_stmt(
    owner: Arc<SessionBoundAdapterOwner>,
    sql: &str,
    columns: &[astersql_meta_model::ColumnInfo],
) -> astersql_executor::adapter::ExecStmt {
    use astersql_executor::adapter::{
        ExecStmt, FieldName, PlanInfo, PlanKind, SchemaColumn, StatementContext, StatementKind,
        StatementNode,
    };
    ExecStmt {
        GoCtx: None,
        InfoSchema: 10,
        Plan: PlanInfo {
            id: 42,
            kind: PlanKind::PointGet,
            schema: columns
                .iter()
                .map(|column| SchemaColumn {
                    field_type: column.FieldType.clone(),
                })
                .collect(),
            calculate_no_delay: false,
            projection_child: None,
            encoded: "point get lock".into(),
            binary: String::new(),
            hints: String::new(),
        },
        TypedPlan: None,
        StmtNode: StatementNode {
            kind: StatementKind::Select,
            original_text: sql.into(),
            text: sql.into(),
            secure_text: sql.into(),
            prepared_text: None,
        },
        Ctx: owner,
        LowerPriority: false,
        isPreparedStmt: false,
        isSelectForUpdate: true,
        retryCount: 0,
        retryStartTime: None,
        phaseBuildDurations: [std::time::Duration::ZERO; 2],
        phaseOpenDurations: [std::time::Duration::ZERO; 2],
        phaseNextDurations: [std::time::Duration::ZERO; 2],
        phaseLockDurations: [std::time::Duration::ZERO; 2],
        OutputNames: columns
            .iter()
            .map(|column| FieldName {
                column_name: column.Name.O.clone(),
                ..Default::default()
            })
            .collect(),
        PsStmt: None,
        Ti: None,
        StatementCtx: StatementContext {
            statement_type: "Select".into(),
            sql_normalized: sql.into(),
            ..Default::default()
        },
    }
}

#[test]
fn canonical_locked_point_get_locks_existing_and_rr_missing_keys_but_rc_only_existing_keys() {
    use crate::testutil::TestSession;
    use astersql_infoschema::infoschema::{CiString, InfoSchema};
    let session = canonical_dml_session();
    let row_key = astersql_tablecodec::EncodeRowKeyWithHandle(123, Box::new(kv::IntHandle(1)));
    let row_value = astersql_tablecodec::EncodeRow(
        Some(astersql_tablecodec::time::UTC),
        vec![
            astersql_types::datum::NewIntDatum(1),
            astersql_types::datum::NewIntDatum(10),
        ],
        vec![1, 2],
        Vec::new(),
        None,
        None,
        astersql_tablecodec::rowcodec::Encoder::new(true),
    )
    .unwrap();
    let mut seed = session
        .domain
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .unwrap();
    seed.Set(kv::Key(row_key.0), row_value).unwrap();
    seed.Commit(&kv::Context::default()).unwrap();
    session.Execute("begin pessimistic").unwrap();
    let domain = Arc::clone(&session.domain);
    let model = domain
        .info_schema()
        .ModelTableInfoByName(&CiString::new("test"), &CiString::new("t"))
        .unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let second = crate::runtime::ConcreteSession::new(domain);
    second.Execute("begin pessimistic").unwrap();
    let competitor = SessionBoundAdapterOwner::new(second);
    let bind = |handle| {
        let mut plan = astersql_planner_core_operator_physicalop::PointGetPlan::New(
            owner.session.AdapterPlanContext(),
        );
        plan.TblInfo = Some(model.as_ref().clone());
        plan.Handle = Some(handle);
        plan.Columns = model.Columns.clone();
        plan.Lock = true;
        plan.LockWaitTime = -1;
        let version = owner
            .session
            .domain
            .storage()
            .with_storage(|storage| storage.CurrentVersion("global"))
            .unwrap();
        owner
            .BindTypedPhysicalPlan(Box::new(plan), Vec::new(), version, 1, 1)
            .unwrap();
    };
    bind(1);
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from t where a=1 for update",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    rows.Next(&mut page).unwrap();
    assert_eq!(page.NumRows(), 1);
    assert_eq!(page.GetRow(0).GetInt64(0), 1);
    assert_eq!(page.GetRow(0).GetInt64(1), 10);
    rows.Close().unwrap();
    let first_key = astersql_tablecodec::EncodeRowKeyWithHandle(123, Box::new(kv::IntHandle(1))).0;
    assert!(competitor.TryLockKeys(&[first_key]).is_err());
    let previous = owner.HeldRowLockCount();
    bind(2);
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from t where a=2 for update",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    rows.Next(&mut page).unwrap();
    assert_eq!(page.NumRows(), 0);
    rows.Close().unwrap();
    assert_eq!(owner.HeldRowLockCount(), previous + 1);
    let missing_key =
        astersql_tablecodec::EncodeRowKeyWithHandle(123, Box::new(kv::IntHandle(2))).0;
    assert!(competitor.TryLockKeys(&[missing_key]).is_err());

    owner.session.state.borrow_mut().transaction_isolation = "READ-COMMITTED".into();
    let previous = owner.HeldRowLockCount();
    bind(3);
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from t where a=3 for update",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    rows.Next(&mut page).unwrap();
    assert_eq!(page.NumRows(), 0);
    rows.Close().unwrap();
    assert_eq!(owner.HeldRowLockCount(), previous);
}

#[test]
fn canonical_locked_unique_point_get_locks_index_and_record_and_rr_missing_index_key() {
    use crate::testutil::TestSession;
    use astersql_infoschema::infoschema::{CiString, InfoSchema};
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    session
        .Execute("create table idx_t (id int primary key, u int unique, v int)")
        .unwrap();
    session
        .Execute("insert into idx_t values (1,7,70)")
        .unwrap();
    session.Execute("begin pessimistic").unwrap();
    let domain = Arc::clone(&session.domain);
    let model = domain
        .info_schema()
        .ModelTableInfoByName(&CiString::new("test"), &CiString::new("idx_t"))
        .unwrap();
    let index = model
        .Indices
        .iter()
        .find(|index| index.Unique)
        .unwrap()
        .clone();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let other = crate::runtime::ConcreteSession::new(domain);
    other.Execute("begin pessimistic").unwrap();
    let competitor = SessionBoundAdapterOwner::new(other);
    let bind = |value| {
        let mut plan = astersql_planner_core_operator_physicalop::PointGetPlan::New(
            owner.session.AdapterPlanContext(),
        );
        plan.TblInfo = Some(model.as_ref().clone());
        plan.IndexInfo = Some(index.clone());
        plan.IndexValues = vec![astersql_types::datum::NewIntDatum(value)];
        plan.Columns = model.Columns.clone();
        plan.Lock = true;
        plan.LockWaitTime = -1;
        let version = owner
            .session
            .domain
            .storage()
            .with_storage(|storage| storage.CurrentVersion("global"))
            .unwrap();
        owner
            .BindTypedPhysicalPlan(Box::new(plan), Vec::new(), version, 1, 1)
            .unwrap();
    };
    let index_key = |value| {
        astersql_tablecodec::GenIndexKey(
            astersql_tablecodec::codec::NewEncoder(
                astersql_tablecodec::collate::NewCollationEnabled(),
            ),
            Some(astersql_tablecodec::time::UTC),
            Box::new(model.as_ref().clone()),
            Box::new(index.clone()),
            model.ID,
            vec![astersql_types::datum::NewIntDatum(value)],
            None,
            None,
        )
        .unwrap()
        .0
    };
    bind(7);
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from idx_t where u=7 for update",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    rows.Next(&mut page).unwrap();
    assert_eq!(page.NumRows(), 1);
    rows.Close().unwrap();
    let record_key =
        astersql_tablecodec::EncodeRowKeyWithHandle(model.ID, Box::new(kv::IntHandle(1))).0;
    assert!(competitor.TryLockKeys(&[index_key(7)]).is_err());
    assert!(competitor.TryLockKeys(&[record_key]).is_err());
    let before = owner.session.state.borrow().held_row_locks.len();
    bind(99);
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from idx_t where u=99 for update",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    rows.Next(&mut page).unwrap();
    assert_eq!(page.NumRows(), 0);
    rows.Close().unwrap();
    assert_eq!(
        owner.session.state.borrow().held_row_locks.len(),
        before + 1
    );
    assert!(competitor.TryLockKeys(&[index_key(99)]).is_err());
    owner.session.state.borrow_mut().transaction_isolation = "READ-COMMITTED".into();
    let before = owner.session.state.borrow().held_row_locks.len();
    bind(100);
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from idx_t where u=100 for update",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    rows.Next(&mut page).unwrap();
    rows.Close().unwrap();
    assert_eq!(page.NumRows(), 0);
    assert_eq!(owner.session.state.borrow().held_row_locks.len(), before);
}

#[test]
fn canonical_locked_point_get_respects_millisecond_wait_and_leaves_no_partial_row() {
    use crate::testutil::TestSession;
    use astersql_infoschema::infoschema::{CiString, InfoSchema};
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    session
        .Execute("create table wait_t (id int primary key, v int)")
        .unwrap();
    session.Execute("insert into wait_t values (1,10)").unwrap();
    session.Execute("begin pessimistic").unwrap();
    let domain = Arc::clone(&session.domain);
    let model = domain
        .info_schema()
        .ModelTableInfoByName(&CiString::new("test"), &CiString::new("wait_t"))
        .unwrap();
    let competitor = SessionBoundAdapterOwner::new(crate::runtime::ConcreteSession::new(domain));
    competitor.session.Execute("begin pessimistic").unwrap();
    let key = astersql_tablecodec::EncodeRowKeyWithHandle(model.ID, Box::new(kv::IntHandle(1))).0;
    competitor.TryLockKeys(&[key]).unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut plan = astersql_planner_core_operator_physicalop::PointGetPlan::New(
        owner.session.AdapterPlanContext(),
    );
    plan.TblInfo = Some(model.as_ref().clone());
    plan.Handle = Some(1);
    plan.Columns = model.Columns.clone();
    plan.Lock = true;
    plan.LockWaitTime = 50;
    let version = owner
        .session
        .domain
        .storage()
        .with_storage(|storage| storage.CurrentVersion("global"))
        .unwrap();
    owner
        .BindTypedPhysicalPlan(Box::new(plan), Vec::new(), version, 1, 1)
        .unwrap();
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from wait_t where id=1 for update wait 0.05",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    let started = std::time::Instant::now();
    let error = rows.Next(&mut page).unwrap_err();
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(30),
        "{error}"
    );
    assert_eq!(page.NumRows(), 0);
    assert_eq!(owner.HeldRowLockCount(), 0);
}

#[test]
fn canonical_locked_point_get_rejects_a_record_deleted_after_rr_read_ts_without_retaining_lock() {
    use crate::testutil::TestSession;
    use astersql_infoschema::infoschema::{CiString, InfoSchema};
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    session
        .Execute("create table stale_t (id int primary key, v int)")
        .unwrap();
    session
        .Execute("insert into stale_t values (1,10)")
        .unwrap();
    session.Execute("begin pessimistic").unwrap();
    let domain = Arc::clone(&session.domain);
    let model = domain
        .info_schema()
        .ModelTableInfoByName(&CiString::new("test"), &CiString::new("stale_t"))
        .unwrap();
    let read_version = domain
        .storage()
        .with_storage(|storage| storage.CurrentVersion("global"))
        .unwrap();
    let writer = crate::runtime::ConcreteSession::new(Arc::clone(&domain));
    writer.Execute("delete from stale_t where id=1").unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut plan = astersql_planner_core_operator_physicalop::PointGetPlan::New(
        owner.session.AdapterPlanContext(),
    );
    plan.TblInfo = Some(model.as_ref().clone());
    plan.Handle = Some(1);
    plan.Columns = model.Columns.clone();
    plan.Lock = true;
    plan.LockWaitTime = -1;
    owner
        .BindTypedPhysicalPlan(Box::new(plan), Vec::new(), read_version, 1, 1)
        .unwrap();
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from stale_t where id=1 for update",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    let error = rows.Next(&mut page).unwrap_err();
    assert!(kv::ErrWriteConflict.Equal(Some(&error)), "{error}");
    assert_eq!(page.NumRows(), 0);
    assert_eq!(owner.HeldRowLockCount(), 0);
}

#[test]
fn canonical_locked_point_get_reads_its_own_uncommitted_transaction_row() {
    use crate::testutil::TestSession;
    use astersql_infoschema::infoschema::{CiString, InfoSchema};
    let session = canonical_dml_session();
    session.Execute("begin pessimistic").unwrap();
    session.Execute("insert into t values (4,40)").unwrap();
    let domain = Arc::clone(&session.domain);
    let model = domain
        .info_schema()
        .ModelTableInfoByName(&CiString::new("test"), &CiString::new("t"))
        .unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut plan = astersql_planner_core_operator_physicalop::PointGetPlan::New(
        owner.session.AdapterPlanContext(),
    );
    plan.TblInfo = Some(model.as_ref().clone());
    plan.Handle = Some(4);
    plan.Columns = model.Columns.clone();
    plan.Lock = true;
    plan.LockWaitTime = -1;
    let version = owner
        .session
        .domain
        .storage()
        .with_storage(|storage| storage.CurrentVersion("global"))
        .unwrap();
    owner
        .BindTypedPhysicalPlan(Box::new(plan), Vec::new(), version, 1, 1)
        .unwrap();
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from t where a=4 for update",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    rows.Next(&mut page).unwrap();
    assert_eq!(page.NumRows(), 1);
    assert_eq!(page.GetRow(0).GetInt64(1), 40);
    rows.Close().unwrap();

    owner
        .session
        .Execute("update t set b=50 where a=4")
        .unwrap();
    owner.session.state.borrow_mut().transaction_isolation = "READ-COMMITTED".into();
    let mut plan = astersql_planner_core_operator_physicalop::PointGetPlan::New(
        owner.session.AdapterPlanContext(),
    );
    plan.TblInfo = Some(model.as_ref().clone());
    plan.Handle = Some(4);
    plan.Columns = model.Columns.clone();
    plan.Lock = true;
    plan.LockWaitTime = -1;
    owner
        .BindTypedPhysicalPlan(Box::new(plan), Vec::new(), version, 1, 1)
        .unwrap();
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from t where a=4 for update",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    rows.Next(&mut page).unwrap();
    assert_eq!(page.NumRows(), 1);
    assert_eq!(page.GetRow(0).GetInt64(1), 50);
    rows.Close().unwrap();

    owner.session.Execute("delete from t where a=4").unwrap();
    let mut plan = astersql_planner_core_operator_physicalop::PointGetPlan::New(
        owner.session.AdapterPlanContext(),
    );
    plan.TblInfo = Some(model.as_ref().clone());
    plan.Handle = Some(4);
    plan.Columns = model.Columns.clone();
    plan.Lock = true;
    plan.LockWaitTime = -1;
    owner
        .BindTypedPhysicalPlan(Box::new(plan), Vec::new(), version, 1, 1)
        .unwrap();
    let mut rows = point_lock_stmt(
        Arc::clone(&owner),
        "select * from t where a=4 for update",
        &model.Columns,
    )
    .Exec()
    .unwrap()
    .unwrap();
    let mut page = rows.NewChunk();
    rows.Next(&mut page).unwrap();
    assert_eq!(page.NumRows(), 0);
}

#[test]
fn canonical_adapter_dml_executes_lazily_and_locks_insert_update_delete_mutations() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::PlanKind;
    let session = canonical_dml_session();
    session.Execute("begin pessimistic").unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    for (sql, kind, expected) in [
        (
            "insert into t values (1,10)",
            PlanKind::Insert,
            Some(vec!["10".to_owned()]),
        ),
        (
            "update t set b=20 where a=1",
            PlanKind::Update,
            Some(vec!["20".to_owned()]),
        ),
        ("delete from t where a=1", PlanKind::Delete, None),
    ] {
        owner.BindDMLStatement(sql).unwrap();
        let previous = owner.HeldRowLockCount();
        let mut stmt = dml_stmt(Arc::clone(&owner), sql, kind);
        assert!(stmt.Exec().unwrap().is_none());
        assert!(owner.HeldRowLockCount() >= previous);
        assert!(owner.session.state.borrow().transaction_write_keys.len() > 0);
        let mut rows = owner.session.Execute("select b from t where a=1").unwrap();
        assert_eq!(rows[0].Next().unwrap(), expected);
    }
}

#[test]
fn typed_dml_writer_defers_mutation_until_next_and_adapter_rejects_snapshot_writes() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::{ExecExecutor, PlanInfo, PlanKind};
    let session = canonical_dml_session();
    session.Execute("begin pessimistic").unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let sql = "insert into t values (2,30)";
    owner.BindDMLStatement(sql).unwrap();
    let plan = PlanInfo {
        id: 42,
        kind: PlanKind::Insert,
        schema: Vec::new(),
        calculate_no_delay: true,
        projection_child: None,
        encoded: String::new(),
        binary: String::new(),
        hints: String::new(),
    };
    let mut executor: Box<dyn ExecExecutor> = owner.BuildExecutor(&plan, None).unwrap();
    executor.Open().unwrap();
    let mut rows = owner.session.Execute("select b from t where a=2").unwrap();
    assert_eq!(rows[0].Next().unwrap(), None);
    let mut page = executor.NewChunk();
    executor.Next(&mut page).unwrap();
    executor.Close().unwrap();
    let mut rows = owner.session.Execute("select b from t where a=2").unwrap();
    assert_eq!(rows[0].Next().unwrap(), Some(vec!["30".into()]));

    owner.session.state.borrow_mut().snapshot_read_ts = Some(10);
    let blocked_sql = "insert into t values (3,40)";
    owner.BindDMLStatement(blocked_sql).unwrap();
    let mut stmt = dml_stmt(Arc::clone(&owner), blocked_sql, PlanKind::Insert);
    let error = match stmt.Exec() {
        Ok(_) => panic!("snapshot write should fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("tidb_snapshot"));
    assert!(
        !owner
            .session
            .state
            .borrow()
            .transaction_write_keys
            .iter()
            .any(|key| key.key
                == astersql_tablecodec::EncodeRowKeyWithHandle(123, Box::new(kv::IntHandle(3))).0)
    );
    owner.session.state.borrow_mut().snapshot_read_ts = None;
    owner.session.SetConnectionID(1);
    owner
        .session
        .Execute("set tidb_low_resolution_tso=1")
        .unwrap();
    let low_resolution_sql = "insert into t values (4,50)";
    owner.BindDMLStatement(low_resolution_sql).unwrap();
    let error = match dml_stmt(Arc::clone(&owner), low_resolution_sql, PlanKind::Insert).Exec() {
        Ok(_) => panic!("low resolution TSO write should fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("tidb_low_resolution_tso"));
}

#[test]
fn pessimistic_dml_lazy_uniqueness_failure_aborts_entire_canonical_transaction() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::PlanKind;
    let session = canonical_dml_session();
    session.Execute("begin pessimistic").unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    owner
        .session
        .state
        .borrow_mut()
        .constraint_check_in_place_pessimistic = false;
    let first = "insert into t values (1,10)";
    owner.BindDMLStatement(first).unwrap();
    assert!(
        dml_stmt(Arc::clone(&owner), first, PlanKind::Insert)
            .Exec()
            .unwrap()
            .is_none()
    );
    let duplicate = "insert into t values (1,11)";
    owner.BindDMLStatement(duplicate).unwrap();
    let error = match dml_stmt(Arc::clone(&owner), duplicate, PlanKind::Insert).Exec() {
        Ok(_) => panic!("duplicate writer should fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("8147"), "{error}");
    assert!(!owner.session.TransactionIsPessimistic());
    assert!(!owner.session.WithSessionVars(|vars| vars.InTxn()));
    let mut rows = owner.session.Execute("select b from t where a=1").unwrap();
    assert_eq!(rows[0].Next().unwrap(), None);
}

#[test]
fn adapter_fk_trigger_savepoint_rolls_back_cascade_mutation_and_suppresses_affected_rows() {
    use crate::testutil::TestSession;
    let session = canonical_dml_session();
    session.Execute("begin pessimistic").unwrap();
    let owner = SessionBoundAdapterOwner::new(session);
    owner.PrepareFKCascadeContext();
    let savepoint = owner.ForeignKeySavepointName();
    assert!(!savepoint.is_empty());
    owner
        .session
        .Execute("insert into t values (5,60)")
        .unwrap();
    owner.HandleFKTriggerError().unwrap();
    let mut rows = owner.session.Execute("select b from t where a=5").unwrap();
    assert_eq!(rows[0].Next().unwrap(), None);
    assert!(owner.ForeignKeySavepointName().is_empty());
    let before = owner
        .session
        .WithSessionVars(|vars| vars.StmtCtx.AffectedRows());
    owner.SetInHandleForeignKeyTrigger(true);
    owner
        .session
        .WithSessionVars(|vars| vars.StmtCtx.AddAffectedRows(9));
    owner.SetInHandleForeignKeyTrigger(false);
    owner
        .session
        .WithSessionVars(|vars| vars.StmtCtx.AddAffectedRows(2));
    assert_eq!(
        owner
            .session
            .WithSessionVars(|vars| vars.StmtCtx.AffectedRows()),
        before + 2
    );
}

#[test]
fn adapter_dml_preserves_canonical_fk_parent_check_and_delete_cascade() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::PlanKind;
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    session
        .Execute("create table parent_t (id int primary key)")
        .unwrap();
    session.Execute("create table child_t (id int primary key, pid int, constraint fk_parent foreign key (pid) references parent_t(id) on delete cascade)").unwrap();
    session.Execute("begin pessimistic").unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let invalid = "insert into child_t values (1,7)";
    owner.BindDMLStatement(invalid).unwrap();
    let error = match dml_stmt(Arc::clone(&owner), invalid, PlanKind::Insert).Exec() {
        Ok(_) => panic!("missing FK parent should fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("1452"), "{error}");
    for (sql, kind) in [
        ("insert into parent_t values (7)", PlanKind::Insert),
        ("insert into child_t values (1,7)", PlanKind::Insert),
    ] {
        owner.BindDMLStatement(sql).unwrap();
        assert!(
            dml_stmt(Arc::clone(&owner), sql, kind)
                .Exec()
                .unwrap()
                .is_none()
        );
    }
    let mut child = owner
        .session
        .Execute("select pid from child_t where id=1")
        .unwrap();
    assert_eq!(child[0].Next().unwrap(), Some(vec!["7".into()]));
    let delete = "delete from parent_t where id=7";
    owner.BindDMLStatement(delete).unwrap();
    assert!(
        dml_stmt(Arc::clone(&owner), delete, PlanKind::Delete)
            .Exec()
            .unwrap()
            .is_none()
    );
    let mut child = owner
        .session
        .Execute("select pid from child_t where id=1")
        .unwrap();
    assert_eq!(child[0].Next().unwrap(), None);
    assert_eq!(
        owner
            .Effects()
            .events
            .iter()
            .filter(|event| event.as_str() == "fk_stmt_commit")
            .count(),
        2,
        "parent and child FK batches must both enter the adapter trigger path"
    );
}

#[test]
fn adapter_real_fk_trigger_error_restores_parent_delete_and_keeps_transaction_active() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::PlanKind;
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    session
        .Execute("create table fk_restrict_parent (id int primary key)")
        .unwrap();
    session.Execute("create table fk_restrict_child (id int primary key, pid int, constraint fk_restrict foreign key (pid) references fk_restrict_parent(id) on delete restrict)").unwrap();
    session.Execute("begin pessimistic").unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    for (sql, kind) in [
        (
            "insert into fk_restrict_parent values (7)",
            PlanKind::Insert,
        ),
        (
            "insert into fk_restrict_child values (1,7)",
            PlanKind::Insert,
        ),
    ] {
        owner.BindDMLStatement(sql).unwrap();
        dml_stmt(Arc::clone(&owner), sql, kind).Exec().unwrap();
    }
    let delete = "delete from fk_restrict_parent where id=7";
    owner.BindDMLStatement(delete).unwrap();
    let error = match dml_stmt(Arc::clone(&owner), delete, PlanKind::Delete).Exec() {
        Err(error) => error,
        Ok(_) => panic!("restrict child must reject parent delete"),
    };
    assert!(error.to_string().contains("1451"), "{error}");
    let mut parent = owner
        .session
        .Execute("select id from fk_restrict_parent where id=7")
        .unwrap();
    assert_eq!(parent[0].Next().unwrap(), Some(vec!["7".into()]));
    assert!(owner.session.state.borrow().transaction.is_some());
}

#[test]
fn adapter_dml_acquires_fk_parent_shared_lock_in_second_phase() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::PlanKind;
    use astersql_infoschema::infoschema::{CiString, InfoSchema};
    let (domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    session
        .Execute("create table fk_parent_phase (id int primary key)")
        .unwrap();
    session.Execute("create table fk_child_phase (id int primary key, pid int, constraint fk_phase foreign key (pid) references fk_parent_phase(id))").unwrap();
    session
        .Execute("insert into fk_parent_phase values (7)")
        .unwrap();
    session.Execute("begin pessimistic").unwrap();
    session.state.borrow_mut().foreign_key_check_in_shared_lock = true;
    let model = domain
        .info_schema()
        .ModelTableInfoByName(&CiString::new("test"), &CiString::new("fk_parent_phase"))
        .unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let sql = "insert into fk_child_phase values (1,7)";
    owner.BindDMLStatement(sql).unwrap();
    dml_stmt(Arc::clone(&owner), sql, PlanKind::Insert)
        .Exec()
        .unwrap();
    let details = owner
        .session
        .WithSessionVars(|vars| vars.StmtCtx.GetExecDetails());
    assert_eq!(
        details
            .SharedLockKeysDetail
            .as_ref()
            .map(|detail| detail.LockKeys),
        Some(1)
    );
    let parent_key =
        astersql_tablecodec::EncodeRowKeyWithHandle(model.ID, Box::new(kv::IntHandle(7))).0;
    assert!(
        owner
            .session
            .state
            .borrow()
            .held_row_locks
            .iter()
            .any(|key| key.key == parent_key)
    );
    let competitor = SessionBoundAdapterOwner::new(crate::runtime::ConcreteSession::new(domain));
    competitor.session.Execute("begin pessimistic").unwrap();
    assert!(competitor.TryLockKeys(&[parent_key]).is_err());
}

#[test]
fn pessimistic_adapter_retry_rolls_back_only_failed_statement_mutations() {
    use crate::testutil::TestSession;
    let session = canonical_dml_session();
    session.Execute("begin pessimistic").unwrap();
    session.Execute("insert into t values (1,10)").unwrap();
    let owner = SessionBoundAdapterOwner::new(session);
    owner.OnPessimisticStmtStart().unwrap();
    owner
        .session
        .Execute("insert into t values (2,20)")
        .unwrap();
    owner.RollbackStatementForRetry().unwrap();
    let mut retained = owner.session.Execute("select b from t where a=1").unwrap();
    let mut reverted = owner.session.Execute("select b from t where a=2").unwrap();
    assert_eq!(retained[0].Next().unwrap(), Some(vec!["10".into()]));
    assert_eq!(reverted[0].Next().unwrap(), None);
    owner.OnPessimisticStmtEnd(false).unwrap();
}

#[test]
fn pessimistic_adapter_retries_lock_conflict_after_reverting_first_attempt_writes() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::PlanKind;
    let session = canonical_dml_session();
    session.Execute("begin pessimistic").unwrap();
    session.Execute("insert into t values (1,10)").unwrap();
    session
        .state
        .borrow_mut()
        .constraint_check_in_place_pessimistic = false;
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let sql = "insert into t values (2,20)";
    owner.BindDMLStatement(sql).unwrap();
    owner.retry_lock_conflict_once.set(true);
    assert!(
        dml_stmt(Arc::clone(&owner), sql, PlanKind::Insert)
            .Exec()
            .unwrap()
            .is_none()
    );
    assert!(
        owner
            .Effects()
            .events
            .iter()
            .any(|event| event.starts_with("pessimistic_retry_read_ts:"))
    );
    let mut first = owner.session.Execute("select b from t where a=1").unwrap();
    let mut second = owner.session.Execute("select b from t where a=2").unwrap();
    assert_eq!(first[0].Next().unwrap(), Some(vec!["10".into()]));
    assert_eq!(second[0].Next().unwrap(), Some(vec!["20".into()]));
    assert_eq!(second[0].Next().unwrap(), None);
    assert!(owner.session.TransactionIsPessimistic());
}

#[test]
fn canonical_dml_error_bridge_preserves_write_conflict_for_adapter_retry() {
    use astersql_executor::adapter::PessimisticErrorAction;
    let conflict = kv::ErrWriteConflict.FastGenByArgs(&[]);
    let session_error = crate::SessionError::with_source(
        format!("apply relational transaction DML: {conflict}"),
        conflict,
    );
    let bridged = session_error.into_shared();
    assert!(kv::ErrWriteConflict.Equal(Some(&bridged)));
    let owner = SessionBoundAdapterOwner::new(canonical_dml_session());
    assert_eq!(
        owner.OnPessimisticLockError(&bridged).unwrap(),
        PessimisticErrorAction::RetryReady
    );
}

#[test]
fn canonical_adapter_summary_reaches_formal_statement_digest_map() {
    use astersql_executor::adapter::StatementSummary;
    let owner = SessionBoundAdapterOwner::new(canonical_dml_session());
    let digest = "adapter-4104-formal-summary";
    owner.Summary(&StatementSummary {
        original_sql: "select b from t where a=1".into(),
        normalized_sql: "select b from t where a=?".into(),
        sql_digest: digest.into(),
        plan_digest: "adapter-plan-4104".into(),
        binary_plan: "binary-plan".into(),
        encoded_plan: "encoded-plan".into(),
        success: true,
        ..Default::default()
    });
    let summaries = astersql_util_stmtsummary::StmtSummaryByDigestMap
        .lock()
        .unwrap()
        .Summaries();
    let entry = summaries
        .iter()
        .find(|summary| summary.digest == digest)
        .expect("adapter summary missing from production digest map");
    assert_eq!(entry.cumulative.execCount, 1);
    assert_eq!(entry.cumulative.sampleSQL, "select b from t where a=1");
    assert_eq!(entry.cumulative.samplePlan, "encoded-plan");
}

#[test]
fn go_merge_38_session_ru_version_reaches_summary_accounting() {
    use astersql_executor::adapter::StatementSummary;
    use astersql_util_execdetails::execdetails::util::RUDetails;
    let owner = SessionBoundAdapterOwner::new(canonical_dml_session());
    astersql_util_stmtsummary_v2::Close();
    for (version, is_write) in [(1, false), (1, true), (2, false), (2, true)] {
        owner.session.domain.set_ru_version(version);
        assert_eq!(u64::from(owner.RUVersion()), version);
        let digest = format!("go_merge_38_ru_{version}_{is_write}");
        owner.Summary(&StatementSummary {
            original_sql: "select 1".into(),
            normalized_sql: "select ?".into(),
            sql_digest: digest.clone(),
            ru_version: owner.RUVersion(),
            total_ru_v2: Some(27.0),
            is_write,
            ru_details: Some(RUDetails {
                read_ru: 11.0,
                write_ru: 7.0,
                ru_wait_duration: std::time::Duration::from_millis(20),
                ..Default::default()
            }),
            success: true,
            ..Default::default()
        });
        let summaries = astersql_util_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .unwrap()
            .Summaries();
        let entry = summaries
            .iter()
            .find(|entry| entry.digest == digest)
            .unwrap();
        let ru = &entry.cumulative.StmtRUSummary;
        let (read, write) = if version == 2 {
            if is_write { (0.0, 27.0) } else { (27.0, 0.0) }
        } else {
            (11.0, 7.0)
        };
        assert_eq!((ru.SumRRU, ru.SumWRU), (read, write));
        assert_eq!(ru.SumRUWaitDuration, std::time::Duration::from_millis(20));
    }
}

#[test]
fn canonical_adapter_top_sql_begin_finish_updates_formal_statement_stats() {
    use astersql_executor::adapter::{Digest, StatementContext};
    let owner = SessionBoundAdapterOwner::new(canonical_dml_session());
    let mut context = StatementContext::default();
    context.sql_normalized = "select b from t where a=?".into();
    context.plan_digest = Some((
        "PointGet".into(),
        Digest {
            text: "plan".into(),
            bytes: b"plan-4104".to_vec(),
        },
    ));
    context.network_received_bytes = 11;
    context.network_sent_bytes = 17;
    owner.SetStatementContext(&context);
    owner.TopSQLStart(b"sql-4104", b"plan-4104");
    owner.TopSQLFinish(0.0);
    let stats = owner.top_sql_stats.borrow().as_ref().cloned().unwrap();
    let item = stats.GetOrCreateStatementStatsItem(b"sql-4104", b"plan-4104");
    assert_eq!(item.ExecCount, 1);
    assert_eq!(item.DurationCount, 1);
    assert_eq!(item.NetworkInBytes, 11);
    assert_eq!(item.NetworkOutBytes, 17);
}

#[test]
fn canonical_adapter_ruv2_reports_through_domain_consumption_service() {
    use std::sync::Mutex;
    struct Reporter(Mutex<Vec<(String, f64, f64, f64)>>);
    impl astersql_domain::ruv2_reporter::RUV2ConsumptionReporter for Reporter {
        fn report_ruv2_consumption(&self, group: &str, tikv: f64, tidb: f64, tiflash: f64) {
            self.0
                .lock()
                .unwrap()
                .push((group.into(), tikv, tidb, tiflash));
        }
    }
    let session = canonical_dml_session();
    let reporter = Arc::new(Reporter(Mutex::new(Vec::new())));
    session
        .domain
        .bind_ruv2_consumption_reporter(Some(reporter.clone()));
    let owner = SessionBoundAdapterOwner::new(session);
    assert!(owner.RUV2ReporterAvailable());
    owner.ReportRUV2Consumption("rg1", 1.0, 2.0, 3.0);
    assert_eq!(
        reporter.0.lock().unwrap().as_slice(),
        &[("rg1".into(), 1.0, 2.0, 3.0)]
    );
}

#[test]
fn canonical_adapter_runaway_manager_switches_group_before_executor() {
    use astersql_executor::adapter::{StatementKind, StatementNode};
    use astersql_resourcegroup_runaway as runaway;
    struct Catalog(Vec<runaway::ResourceGroup>);
    impl runaway::ResourceGroupCatalog for Catalog {
        fn GetResourceGroup(&self, name: &str) -> runaway::Result<Option<runaway::ResourceGroup>> {
            Ok(self.0.iter().find(|group| group.name == name).cloned())
        }
    }
    let settings = runaway::RunawaySettings {
        rule: runaway::RunawayRule {
            exec_elapsed_time_ms: 1_000,
            request_unit: 100,
            processed_keys: 10,
        },
        action: runaway::RunawayAction::Kill,
        switch_group_name: String::new(),
        watch: Some(runaway::RunawayWatch {
            kind: runaway::RunawayWatchType::Similar,
            lasting_duration_ms: 60_000,
        }),
    };
    let manager = runaway::manager::Manager::NewRunawayManager(
        Arc::new(Catalog(vec![
            runaway::ResourceGroup {
                name: "source".into(),
                runaway_settings: Some(settings),
            },
            runaway::ResourceGroup {
                name: "target".into(),
                runaway_settings: None,
            },
        ])),
        "server-1",
        Arc::new(runaway::NoopExecutor),
        Arc::new(runaway::syncer::AllSystemTables),
    );
    manager.AddWatch(runaway::record::QuarantineRecord {
        ResourceGroupName: "source".into(),
        WatchText: "select switch".into(),
        Watch: runaway::RunawayWatchType::Exact,
        Action: runaway::RunawayAction::SwitchGroup,
        SwitchGroupName: "target".into(),
        ..Default::default()
    });
    let session = canonical_dml_session();
    session.domain.bind_runaway_manager(Some(Arc::new(manager)));
    let owner = SessionBoundAdapterOwner::new(session);
    *owner.runaway_resource_group_override.borrow_mut() = Some("source".into());
    let old = astersql_sessionctx_vardef::EnableResourceControl.Load();
    astersql_sessionctx_vardef::EnableResourceControl.Store(true);
    let statement = StatementNode {
        kind: StatementKind::Select,
        original_text: "select switch".into(),
        text: "select switch".into(),
        secure_text: "select switch".into(),
        prepared_text: None,
    };
    let result = owner.RunawayBeforeExecutor(&statement, "sql", "plan");
    astersql_sessionctx_vardef::EnableResourceControl.Store(old);
    result.unwrap();
    assert_eq!(owner.ResourceGroupName(), "target");
    assert!(owner.runaway_checker.borrow().is_some());
    assert!(owner.session.state.borrow().runaway_checker.is_some());
}

#[test]
fn canonical_runaway_checker_crosses_kv_interface_with_deadline_and_processed_keys() {
    use astersql_resourcegroup_runaway as runaway;
    struct Catalog(runaway::ResourceGroup);
    impl runaway::ResourceGroupCatalog for Catalog {
        fn GetResourceGroup(&self, name: &str) -> runaway::Result<Option<runaway::ResourceGroup>> {
            Ok((self.0.name == name).then(|| self.0.clone()))
        }
    }
    let settings = runaway::RunawaySettings {
        rule: runaway::RunawayRule {
            exec_elapsed_time_ms: 60_000,
            request_unit: 4,
            processed_keys: 9,
        },
        action: runaway::RunawayAction::Kill,
        switch_group_name: String::new(),
        watch: None,
    };
    let manager = runaway::manager::Manager::NewRunawayManager(
        Arc::new(Catalog(runaway::ResourceGroup {
            name: "kill".into(),
            runaway_settings: Some(settings.clone()),
        })),
        "server-1",
        Arc::new(runaway::NoopExecutor),
        Arc::new(runaway::syncer::AllSystemTables),
    );
    let checker = Arc::new(runaway::checker::Checker::NewChecker(
        manager,
        "kill".into(),
        Some(settings),
        "select 1".into(),
        "sql".into(),
        "plan".into(),
        runaway::nowMicros(),
    ));
    let bridge = super::typed_runaway_checker::SessionRunawayChecker(checker);
    let mut request = kv::resourcegroup::CopRequest {
        resource_group_name: "kill".into(),
        ..Default::default()
    };
    kv::resourcegroup::RunawayChecker::BeforeCopRequest(&bridge, &mut request).unwrap();
    assert!(request.max_execution_duration_ms > 0);
    assert!(request.max_execution_duration_ms <= 60_000);
    kv::resourcegroup::RunawayChecker::CheckThresholds(&bridge, None, 8, None).unwrap();
    assert!(
        kv::resourcegroup::RunawayChecker::CheckThresholds(&bridge, None, 1, None)
            .unwrap_err()
            .contains("ProcessedKeys")
    );
    assert_eq!(
        kv::resourcegroup::RunawayChecker::CheckAction(&bridge),
        kv::resourcegroup::RunawayAction::Kill
    );
}

#[test]
fn restricted_adapter_analyze_publishes_stats_and_restores_temporary_session_settings() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::{PlanKind, StatementKind};
    use astersql_infoschema::infoschema::{CiString, InfoSchema};
    let (domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    session
        .Execute("create table adapter_analyze_t (id int primary key)")
        .unwrap();
    session
        .Execute("insert into adapter_analyze_t values (1),(2)")
        .unwrap();
    let table = domain
        .info_schema()
        .ModelTableInfoByName(&CiString::new("test"), &CiString::new("adapter_analyze_t"))
        .unwrap();
    {
        let mut state = session.state.borrow_mut();
        state.in_restricted_sql = true;
        state.analyze_concurrency = 3;
        state.transaction_isolation = "REPEATABLE-READ".into();
    }
    session
        .Execute("set session tidb_sysproc_scan_concurrency = 7")
        .unwrap();
    let request = super::relational_analyze_select_request(
        &table,
        1,
        7,
        kv::ReplicaReadType::ReplicaReadLeader,
        kv::GlobalTxnScope,
        false,
        0,
    )
    .unwrap();
    assert_eq!(request.Concurrency, 7);
    // ANALYZE scans the complete record prefix in one unordered request.
    assert!(!request.KeepOrder);
    let ranges = request.KeyRanges.as_ref().unwrap();
    assert_eq!(ranges.TotalRangeNum(), 1);
    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table.ID).0);
    assert_eq!(ranges.FirstPartitionRange()[0].StartKey, prefix);
    assert_eq!(ranges.FirstPartitionRange()[0].EndKey, prefix.PrefixNext());
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let sql = "analyze table adapter_analyze_t";
    owner.BindAnalyzeStatement(sql).unwrap();
    let mut statement = dml_stmt(Arc::clone(&owner), sql, PlanKind::Insert);
    statement.Plan.kind = PlanKind::Analyze;
    statement.StmtNode.kind = StatementKind::Other;
    assert!(statement.Exec().unwrap().is_none());
    assert_eq!(
        domain
            .stats_context()
            .physical_stats(table.ID)
            .unwrap()
            .realtime_count,
        2
    );
    assert_eq!(owner.session.state.borrow().analyze_concurrency, 3);
    assert_eq!(
        owner
            .session
            .state
            .borrow()
            .restricted_analyze_scan_concurrency,
        None
    );
    assert_eq!(
        owner.session.state.borrow().transaction_isolation,
        "REPEATABLE-READ"
    );
    let fail_sql = "analyze table adapter_missing_t";
    owner.BindAnalyzeStatement(fail_sql).unwrap();
    let mut failure = dml_stmt(Arc::clone(&owner), fail_sql, PlanKind::Insert);
    failure.Plan.kind = PlanKind::Analyze;
    failure.StmtNode.kind = StatementKind::Other;
    assert!(failure.Exec().is_err());
    assert_eq!(owner.session.state.borrow().analyze_concurrency, 3);
    assert_eq!(
        owner
            .session
            .state
            .borrow()
            .restricted_analyze_scan_concurrency,
        None
    );
    assert_eq!(
        owner.session.state.borrow().transaction_isolation,
        "REPEATABLE-READ"
    );
}

#[test]
fn finish_execute_stmt_exposes_retries_to_slow_log_and_clears_mpp_query_ids() {
    use astersql_executor::adapter::PlanKind;
    use std::sync::atomic::Ordering;
    let session = canonical_dml_session();
    session.WithSessionVars(|vars| {
        let mpp = &vars.StmtCtx.MPPQueryInfo;
        mpp.QueryID.store(11, Ordering::Release);
        mpp.QueryTS.store(12, Ordering::Release);
        mpp.AllocatedMPPTaskID.store(13, Ordering::Release);
        mpp.AllocatedMPPGatherID.store(14, Ordering::Release);
    });
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut statement = dml_stmt(
        Arc::clone(&owner),
        "insert into t values (1,1)",
        PlanKind::Insert,
    );
    statement.retryCount = 2;
    statement.FinishExecuteStmt(0, None, false);
    owner.session.WithSessionVars(|vars| {
        assert_eq!(vars.StmtCtx.ExecRetryCountValue(), 2);
        let mpp = &vars.StmtCtx.MPPQueryInfo;
        assert_eq!(mpp.QueryID.load(Ordering::Acquire), 0);
        assert_eq!(mpp.QueryTS.load(Ordering::Acquire), 0);
        assert_eq!(mpp.AllocatedMPPTaskID.load(Ordering::Acquire), 0);
        assert_eq!(mpp.AllocatedMPPGatherID.load(Ordering::Acquire), 0);
    });
}

#[test]
fn finish_execute_stmt_accounts_real_scan_and_commit_details_in_session_sli() {
    use astersql_executor::adapter::PlanKind;
    use astersql_util_execdetails::execdetails::{CopExecDetails, util};
    use std::sync::atomic::Ordering;
    let session = canonical_dml_session();
    session.WithSessionVars(|vars| {
        vars.StmtCtx.SyncExecDetails.MergeCopExecDetails(
            Some(&CopExecDetails {
                ScanDetail: Some(util::ScanDetail {
                    ProcessedKeys: 7,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            std::time::Duration::ZERO,
        );
        vars.StmtCtx
            .SyncExecDetails
            .MergeExecDetails(Some(util::CommitDetails {
                WriteSize: 58,
                WriteKeys: 2,
                ..Default::default()
            }));
    });
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut statement = dml_stmt(
        Arc::clone(&owner),
        "insert into t values (1,1)",
        PlanKind::Insert,
    );
    statement.StatementCtx.affected_rows = 2;
    statement.FinishExecuteStmt(0, None, false);
    owner.session.WithSessionVars(|vars| {
        assert_eq!(vars.KeysExamined.load(Ordering::Acquire), 7);
    });
    assert!(
        owner
            .session
            .state
            .borrow()
            .txn_write_throughput_sli
            .String()
            .contains("writeSize: 58, readKeys: 7, writeKeys: 2")
    );
}

#[test]
fn finish_execute_stmt_observes_each_real_commit_phase_from_statement_details() {
    use astersql_executor::adapter::PlanKind;
    use astersql_util_execdetails::execdetails::util;
    let session = canonical_dml_session();
    session.WithSessionVars(|vars| {
        vars.StmtCtx
            .SyncExecDetails
            .MergeExecDetails(Some(util::CommitDetails {
                PrewriteTime: std::time::Duration::from_millis(1),
                CommitTime: std::time::Duration::from_millis(2),
                GetCommitTsTime: std::time::Duration::from_millis(3),
                GetLatestTsTime: std::time::Duration::from_millis(4),
                LocalLatchTime: std::time::Duration::from_millis(5),
                WaitPrewriteBinlogTime: std::time::Duration::from_millis(6),
                ..Default::default()
            }));
    });
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut statement = dml_stmt(
        Arc::clone(&owner),
        "insert into t values (1,1)",
        PlanKind::Insert,
    );
    statement.FinishExecuteStmt(0, None, false);
    let effects = owner.effects.borrow();
    for (phase, ms) in [
        ("commit_prewrite", 1),
        ("commit_commit", 2),
        ("commit_wait_commit_ts", 3),
        ("commit_wait_latest_ts", 4),
        ("commit_wait_latch", 5),
        ("commit_wait_binlog", 6),
    ] {
        assert!(
            effects
                .events
                .contains(&format!("phase:{phase}:false:{ms}ms")),
            "missing {phase}"
        );
    }
}

#[test]
fn finish_execute_stmt_attaches_commit_and_lock_stats_to_shared_plan_collector() {
    use astersql_executor::adapter::PlanKind;
    use astersql_util_execdetails::execdetails::{RuntimeStatsColl, util};
    let mut session = canonical_dml_session();
    let collector = Arc::new(RuntimeStatsColl::default());
    Arc::get_mut(&mut session.session_vars)
        .unwrap()
        .StmtCtx
        .RuntimeStatsColl = Some(collector.clone());
    session.WithSessionVars(|vars| {
        vars.StmtCtx
            .SyncExecDetails
            .MergeExecDetails(Some(util::CommitDetails {
                WriteSize: 58,
                WriteKeys: 2,
                ..Default::default()
            }));
        vars.StmtCtx
            .SyncExecDetails
            .MergeLockKeysExecDetails(Some(util::LockKeysDetails {
                LockKeys: 3,
                ..Default::default()
            }));
    });
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut statement = dml_stmt(
        Arc::clone(&owner),
        "insert into t values (1,1)",
        PlanKind::Insert,
    );
    statement.FinishExecuteStmt(0, None, false);
    let rendered = collector.GetRootStatsStringShared(42);
    assert!(
        rendered.contains("write_keys:2, write_byte:58"),
        "{rendered}"
    );
    assert!(rendered.contains("lock_keys:"), "{rendered}");
    assert!(collector.GetRootStatsStringShared(43).is_empty());
}

#[test]
fn fair_locking_exec_success_sets_transaction_flags_and_finish_reports_stmt_and_txn() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::PlanKind;
    use astersql_util_execdetails::execdetails::util;
    use std::sync::atomic::Ordering;
    let session = canonical_dml_session();
    session.Execute("begin pessimistic").unwrap();
    session.WithSessionVars(|vars| {
        vars.StmtCtx
            .SyncExecDetails
            .MergeLockKeysExecDetails(Some(util::LockKeysDetails {
                AggressiveLockNewCount: 1,
                LockedWithConflictCount: 1,
                ..Default::default()
            }));
        vars.StmtCtx
            .SyncExecDetails
            .MergeSharedLockKeysExecDetails(Some(util::LockKeysDetails {
                LockKeys: 2,
                ..Default::default()
            }));
        vars.StmtCtx
            .SyncExecDetails
            .MergeExecDetails(Some(util::CommitDetails {
                WriteSize: 1,
                WriteKeys: 1,
                ..Default::default()
            }));
    });
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    owner
        .BindDMLStatement("insert into t values (1,1)")
        .unwrap();
    let mut statement = dml_stmt(
        Arc::clone(&owner),
        "insert into t values (1,1)",
        PlanKind::Insert,
    );
    statement.Exec().unwrap();
    assert!(
        owner
            .effects
            .borrow()
            .events
            .iter()
            .any(|event| event.starts_with("exec_locks:") && event.contains(":2:"))
    );
    owner.session.WithSessionVars(|vars| {
        assert!(vars.TxnCtx.FairLockingUsed.load(Ordering::Acquire));
        assert!(vars.TxnCtx.FairLockingEffective.load(Ordering::Acquire));
    });
    statement.FinishExecuteStmt(0, None, false);
    assert!(
        owner
            .effects
            .borrow()
            .events
            .contains(&"fair_locking:true:true:true:true".to_owned())
    );
}

#[test]
fn beginning_next_transaction_clears_prior_fair_locking_usage_flags() {
    use crate::testutil::TestSession;
    use std::sync::atomic::Ordering;
    let session = canonical_dml_session();
    session.WithSessionVars(|vars| {
        vars.TxnCtx.FairLockingUsed.store(true, Ordering::Release);
        vars.TxnCtx
            .FairLockingEffective
            .store(true, Ordering::Release);
    });
    session.Execute("begin pessimistic").unwrap();
    session.WithSessionVars(|vars| {
        assert!(!vars.TxnCtx.FairLockingUsed.load(Ordering::Acquire));
        assert!(!vars.TxnCtx.FairLockingEffective.load(Ordering::Acquire));
    });
}

#[test]
fn finish_execute_stmt_reports_tiflash_and_table_cache_flags_then_clears_shared_state() {
    use astersql_executor::adapter::PlanKind;
    use std::sync::atomic::Ordering;
    let session = canonical_dml_session();
    session.WithSessionVars(|vars| {
        vars.StmtCtx.IsTiFlash.store(true, Ordering::Release);
        vars.StmtCtx.SetReadFromTableCache();
    });
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut statement = dml_stmt(
        Arc::clone(&owner),
        "insert into t values (1,1)",
        PlanKind::Insert,
    );
    statement.FinishExecuteStmt(0, None, false);
    assert!(
        owner
            .effects
            .borrow()
            .events
            .contains(&"supplementary:true:true".to_owned())
    );
    owner.session.WithSessionVars(|vars| {
        assert!(!vars.StmtCtx.IsTiFlash.load(Ordering::Acquire));
        assert!(!vars.StmtCtx.IsReadFromTableCache());
    });
}

#[test]
fn finish_execute_stmt_reports_execute_duration_from_formal_statement_start() {
    use astersql_executor::adapter::PlanKind;
    let session = canonical_dml_session();
    session.WithSessionVars(|vars| {
        vars.SetStatementStartTime(std::time::Instant::now() - std::time::Duration::from_secs(1));
        assert!(vars.GetExecuteDuration() >= std::time::Duration::from_secs(1));
    });
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut statement = dml_stmt(
        Arc::clone(&owner),
        "insert into t values (1,1)",
        PlanKind::Insert,
    );
    statement.FinishExecuteStmt(0, None, false);
    assert!(
        owner
            .effects
            .borrow()
            .events
            .iter()
            .any(|event| event.starts_with("execute_run_duration:false:"))
    );
}

#[test]
fn finish_execute_stmt_restores_formal_plan_when_prior_path_missed_it() {
    use astersql_executor::adapter::PlanKind;
    let session = canonical_dml_session();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut statement = dml_stmt(
        Arc::clone(&owner),
        "insert into t values (1,1)",
        PlanKind::Insert,
    );
    statement.StatementCtx.plan = None;
    statement.FinishExecuteStmt(0, None, false);
    assert_eq!(
        owner
            .statement_context
            .borrow()
            .plan
            .as_ref()
            .map(|plan| plan.id),
        Some(42)
    );
}

#[test]
fn finish_execute_stmt_resets_shared_parse_duration_and_statement_staleness() {
    use astersql_executor::adapter::PlanKind;
    let session = canonical_dml_session();
    session.state.borrow_mut().snapshot_read_ts = Some(100);
    session.begin_txn_statement_observation("select * from t");
    session.WithSessionVars(|vars| {
        assert!(vars.StmtCtx.IsStalenessValue());
        vars.SetDurationParse(std::time::Duration::from_millis(25));
        assert_eq!(vars.DurationParseValue().as_millis(), 25);
    });
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut statement = dml_stmt(
        Arc::clone(&owner),
        "insert into t values (1,1)",
        PlanKind::Insert,
    );
    statement.FinishExecuteStmt(0, None, false);
    owner.session.WithSessionVars(|vars| {
        assert!(!vars.StmtCtx.IsStalenessValue());
        assert_eq!(vars.DurationParseValue(), std::time::Duration::ZERO);
    });
}

#[test]
fn go_merge_187_runtime_evidence_bridge_terminal_snapshot() {
    use astersql_executor::adapter::PlanKind;
    let session = canonical_dml_session();
    session.WithSessionVars(|vars| {
        vars.StmtCtx.SyncExecDetails.MergeExecDetails(Some(
            astersql_util_execdetails::execdetails::util::CommitDetails {
                WriteKeys: 2,
                WriteSize: 58,
                ..Default::default()
            },
        ));
    });
    session.domain.set_ru_version(3);
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut statement = dml_stmt(
        Arc::clone(&owner),
        "insert into t values (1,1)",
        PlanKind::Insert,
    );
    statement.FinishExecuteStmt(0, None, false);
    let frozen = owner.StatementContext().statement_ru_evidence.unwrap();
    assert_eq!(frozen.writes.unwrap().bytes, 58);
    assert!(frozen.point.is_none());
    owner.session.WithSessionVars(|vars| {
        vars.StmtCtx.SyncExecDetails.MergeExecDetails(Some(
            astersql_util_execdetails::execdetails::util::CommitDetails {
                WriteSize: 99,
                ..Default::default()
            },
        ));
    });
    assert_eq!(frozen.writes.unwrap().bytes, 58);
    statement.SnapshotStatementRUEvidence();
    assert_eq!(
        statement
            .StatementCtx
            .statement_ru_evidence
            .unwrap()
            .writes
            .unwrap()
            .bytes,
        58
    );
}

#[test]
fn go_merge_197_setup_eligibility_live_session_boundary() {
    use astersql_executor::adapter::{AdapterRuntime, PlanKind};
    use astersql_executor::statement_ru_result::{
        install_statement_ru_owner, is_statement_ru_ttl_job,
    };
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut statement = dml_stmt(owner.clone(), "delete from t", PlanKind::Delete);
    let mut vars = astersql_sessionctx_variable::session::SessionVars::new();
    vars.StmtCtx.IsReadOnly = false;
    vars.StmtCtx.InSelectStmt = true;
    vars.InRestrictedSQL = true;
    vars.RequestSourceType = astersql_kv::InternalTxnTTL.into();
    vars.TTLJobID = "ttl-job-1".into();
    vars.Status = astersql_parser_mysql::r#const::ServerStatusCursorExists;
    vars.StmtCtx.SetFlatPlan(Some(Arc::new(1_i32)));
    let state = super::scan_adapter_runtime::statement_ru_install_state_from_session_vars(&vars);
    assert!(!state.is_read_only);
    assert!(state.in_select_stmt);
    assert!(state.restricted_sql);
    assert!(is_statement_ru_ttl_job(&state));
    assert!(state.cursor_exists);
    assert!(state.flat_plan_cached);
    install_statement_ru_owner(&mut statement);
    assert!(
        statement.StatementCtx.statement_ru_owner.is_none(),
        "a missing typed plan must stay excluded"
    );
    vars.TTLJobID.clear();
    assert!(!is_statement_ru_ttl_job(
        &super::scan_adapter_runtime::statement_ru_install_state_from_session_vars(&vars)
    ));
    assert!(
        !owner
            .StatementRUInstallState(&statement.StmtNode)
            .unwrap()
            .in_select_stmt
    );
}

#[test]
fn go_merge_20_187_195_197_production_ru_point_collects_evidence() {
    use astersql_infoschema::infoschema::{CiString, InfoSchema};
    for version in [1, 2] {
        let (domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
        domain.set_ru_version(version);
        session
            .execute("create table ru_point (id int primary key)")
            .unwrap();
        session.execute("insert into ru_point values (1)").unwrap();
        let table = domain
            .info_schema()
            .ModelTableInfoByName(&CiString::new("test"), &CiString::new("ru_point"))
            .unwrap();
        let mut point = astersql_planner_core_operator_physicalop::PointGetPlan::New(
            session.AdapterPlanContext(),
        );
        point.TblInfo = Some(table.as_ref().clone());
        point.Handle = Some(1);
        point.Columns = table.Columns.clone();
        let point_id = astersql_planner_core_base::Plan::id(&point);
        let owner = Arc::new(crate::runtime::SessionBoundAdapterOwner::new(session));
        let version = domain
            .storage()
            .with_storage(|store| store.CurrentVersion("global"))
            .unwrap();
        owner
            .BindTypedPhysicalPlan(Box::new(point), Vec::new(), version, 32, 1024)
            .unwrap();
        // The physical point executor uses the real catalog, encoded KV rows and RPC
        // statistics. No fabricated payload or scan-byte evidence enters this test.
        let mut stmt = owner
            .BuildExecStmt(
                astersql_executor::adapter::PlanInfo {
                    kind: astersql_executor::adapter::PlanKind::PointGet,
                    schema: table
                        .Columns
                        .iter()
                        .map(|col| astersql_executor::adapter::SchemaColumn {
                            field_type: col.FieldType.clone(),
                        })
                        .collect(),
                    id: point_id,
                    calculate_no_delay: false,
                    projection_child: None,
                    encoded: String::new(),
                    binary: String::new(),
                    hints: String::new(),
                },
                astersql_executor::adapter::StatementNode {
                    kind: astersql_executor::adapter::StatementKind::Select,
                    text: "select id from ru_point where id = 1".into(),
                    original_text: "select id from ru_point where id = 1".into(),
                    secure_text: "select id from ru_point where id = 1".into(),
                    prepared_text: None,
                },
                Vec::new(),
                false,
            )
            .unwrap();
        let mut result = stmt.Exec().unwrap().unwrap();
        let mut chunk = result.NewChunk();
        result.Next(&mut chunk).unwrap();
        assert_eq!(chunk.NumRows(), 1);
        result.Next(&mut chunk).unwrap();
        assert_eq!(chunk.NumRows(), 0);
        let evidence = stmt.Ctx.StatementRURuntimeEvidence(&[]);
        assert!(
            evidence.point.is_some(),
            "PointGet must retain its real RPC evidence, regardless of domain RUVersion"
        );
        stmt.RecordStatementRUFinalOutcome(true);
        result.Close().unwrap();
        domain.close();
    }
}

fn canonical_rc_wait_session() -> (
    crate::runtime::ConcreteSession,
    astersql_store_mockstore_mockstorage::OracleHandle,
) {
    let column = |id, name: &str, offset| {
        let mut field_type =
            astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
        if id == 1 {
            field_type.AddFlag(astersql_parser_mysql::r#type::PriKeyFlag);
        }
        astersql_meta_model::ColumnInfo {
            ID: id,
            Name: astersql_parser_ast::NewCIStr(name),
            Offset: offset,
            State: astersql_meta_model::StatePublic,
            FieldType: field_type,
            ..Default::default()
        }
    };
    let columns = vec![
        column(1, "id1", 0),
        column(2, "id2", 1),
        column(3, "id3", 2),
    ];
    let model = Arc::new(astersql_meta_model::TableInfo {
        ID: 123,
        Name: astersql_parser_ast::NewCIStr("t1"),
        Columns: columns.clone(),
        PKIsHandle: true,
        Indices: vec![astersql_meta_model::IndexInfo {
            ID: 2,
            Name: astersql_parser_ast::NewCIStr("udx_id2"),
            Table: astersql_parser_ast::NewCIStr("t1"),
            Unique: true,
            State: astersql_meta_model::StatePublic,
            Columns: vec![astersql_meta_model::IndexColumn {
                Name: astersql_parser_ast::NewCIStr("id2"),
                Offset: 1,
                Length: -1,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    let schema = infoschema::infoschema::MockInfoSchema(vec![infoschema::infoschema::TableInfo {
        id: model.ID,
        name: infoschema::infoschema::CiString::new("t1"),
        columns: columns
            .iter()
            .map(|column| infoschema::infoschema::ColumnInfo {
                id: column.ID,
                name: infoschema::infoschema::CiString::new(&column.Name.O),
                ..Default::default()
            })
            .collect(),
        model_meta: Some(model),
        ..Default::default()
    }]);
    let store = Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).unwrap())
        .unwrap_or_else(|_| panic!("unexpected storage owner"));
    let oracle = store.canonical_oracle.clone();
    let domain = Arc::new(Domain::new_mock(
        store,
        Arc::new(DMLBridgeSchemaLoader(schema)),
    ));
    domain.init().unwrap();
    (crate::runtime::ConcreteSession::new(domain), oracle)
}

struct DelayedTimestampOracle {
    original: Arc<dyn kv::oracle::Oracle>,
    delay: Arc<std::sync::Mutex<std::time::Duration>>,
}
struct DelayedTimestampFuture {
    original: Box<dyn kv::oracle::Future>,
    delay: std::time::Duration,
}
impl kv::oracle::Future for DelayedTimestampFuture {
    fn Wait(&mut self) -> Result<u64, kv::errors::SharedError> {
        std::thread::sleep(self.delay);
        self.original.Wait()
    }
}
impl kv::oracle::Oracle for DelayedTimestampOracle {
    fn GetTimestampAsync(&self, scope: &str) -> Option<Box<dyn kv::oracle::Future>> {
        Some(Box::new(DelayedTimestampFuture {
            original: self.original.GetTimestampAsync(scope)?,
            delay: *self.delay.lock().unwrap(),
        }))
    }
    fn GetLowResolutionTimestampAsync(&self, scope: &str) -> Option<Box<dyn kv::oracle::Future>> {
        Some(Box::new(DelayedTimestampFuture {
            original: self.original.GetLowResolutionTimestampAsync(scope)?,
            delay: *self.delay.lock().unwrap(),
        }))
    }
}
struct RestoreTimestampOracle {
    handle: astersql_store_mockstore_mockstorage::OracleHandle,
    original: Arc<dyn kv::oracle::Oracle>,
}
impl Drop for RestoreTimestampOracle {
    fn drop(&mut self) {
        self.handle.SetOracle(self.original.clone());
    }
}

#[test]
fn rc_point_and_empty_range_updates_account_delayed_timestamp_waits() {
    assert_rc_update_timestamp_waits(false);
}

#[test]
fn rc_low_resolution_updates_account_delayed_timestamp_waits() {
    assert_rc_update_timestamp_waits(true);
}

fn assert_rc_update_timestamp_waits(low_resolution: bool) {
    use crate::testutil::TestSession;
    use std::time::Duration;
    let (session, handle) = canonical_rc_wait_session();
    use astersql_statistics_handle::AnalyzeStatsStorage;
    session
        .domain
        .stats_handle()
        .lock()
        .unwrap()
        .register_table_stats(123)
        .unwrap();
    session.state.borrow_mut().low_resolution_tso = low_resolution;
    let original = handle.GetOracle();
    let restore = RestoreTimestampOracle {
        handle: handle.clone(),
        original: original.clone(),
    };
    let restored_original = original.clone();
    let delay = Arc::new(std::sync::Mutex::new(Duration::from_millis(1)));
    handle.SetOracle(Arc::new(DelayedTimestampOracle {
        original,
        delay: delay.clone(),
    }));
    session
        .Execute("set session transaction_isolation = 'READ-COMMITTED'")
        .unwrap();
    session
        .Execute("insert into t1 values (1,1,1), (2,2,2), (3,3,3)")
        .unwrap();
    assert_eq!(
        session.state.borrow().transaction_isolation,
        "READ-COMMITTED"
    );
    session.Execute("begin pessimistic").unwrap();
    *session.session_vars.DurationWaitTS.lock().unwrap() = Duration::ZERO;
    session
        .Execute("update t1 set id3 = id3 + 10 where id1 = 1")
        .unwrap();
    let point_wait = *session.session_vars.DurationWaitTS.lock().unwrap();
    let second_delay = point_wait + Duration::from_millis(1);
    *delay.lock().unwrap() = second_delay;
    session
        .Execute("update t1 set id3 = id3 + 10 where id1 > 3 and id1 < 6")
        .unwrap();
    let range_wait = *session.session_vars.DurationWaitTS.lock().unwrap();
    session.Execute("commit").unwrap();
    assert!(
        point_wait > Duration::from_millis(1),
        "point wait: {point_wait:?}"
    );
    assert!(
        range_wait >= second_delay,
        "range wait: {range_wait:?}, delay: {second_delay:?}"
    );
    let mut results = session.Execute("select * from t1 order by id1").unwrap();
    let mut rows = Vec::new();
    while let Some(row) = results[0].Next().unwrap() {
        rows.push(row);
    }
    results[0].Close().unwrap();
    assert_eq!(
        rows,
        vec![
            vec!["1", "1", "11"],
            vec!["2", "2", "2"],
            vec!["3", "3", "3"]
        ]
    );
    drop(restore);
    assert!(Arc::ptr_eq(&handle.GetOracle(), &restored_original));
}

#[test]
fn delayed_timestamp_future_preserves_values_errors_and_both_oracle_methods() {
    use kv::oracle::Oracle;
    use std::time::{Duration, Instant};
    struct FixedOracle;
    impl Oracle for FixedOracle {
        fn GetTimestampAsync(&self, _: &str) -> Option<Box<dyn kv::oracle::Future>> {
            Some(Box::new(kv::oracle::ReadyFuture(Ok(42))))
        }
        fn GetLowResolutionTimestampAsync(&self, _: &str) -> Option<Box<dyn kv::oracle::Future>> {
            Some(Box::new(kv::oracle::ReadyFuture(Err(kv::errors::New(
                "oracle failure",
            )))))
        }
    }
    let delay = Duration::from_millis(1);
    let oracle = DelayedTimestampOracle {
        original: Arc::new(FixedOracle),
        delay: Arc::new(std::sync::Mutex::new(delay)),
    };
    let mut normal = oracle.GetTimestampAsync("global").unwrap();
    let start = Instant::now();
    assert_eq!(normal.Wait().unwrap(), 42);
    assert!(start.elapsed() >= delay);
    let mut low_resolution = oracle.GetLowResolutionTimestampAsync("global").unwrap();
    let start = Instant::now();
    assert_eq!(
        low_resolution.Wait().unwrap_err().to_string(),
        "oracle failure"
    );
    assert!(start.elapsed() >= delay);
}

#[test]
fn rc_failed_timestamp_wait_does_not_account_time_or_mutate_rows() {
    use crate::testutil::TestSession;
    use astersql_statistics_handle::AnalyzeStatsStorage;
    struct FailedOracle;
    impl kv::oracle::Oracle for FailedOracle {
        fn GetTimestampAsync(&self, _: &str) -> Option<Box<dyn kv::oracle::Future>> {
            Some(Box::new(kv::oracle::ReadyFuture(Err(kv::errors::New(
                "oracle unavailable",
            )))))
        }
    }
    let (session, handle) = canonical_rc_wait_session();
    session
        .domain
        .stats_handle()
        .lock()
        .unwrap()
        .register_table_stats(123)
        .unwrap();
    session
        .Execute("insert into t1 values (1,1,1), (2,2,2), (3,3,3)")
        .unwrap();
    session
        .Execute("set session transaction_isolation = 'READ-COMMITTED'")
        .unwrap();
    session.Execute("begin pessimistic").unwrap();
    let restore = RestoreTimestampOracle {
        handle: handle.clone(),
        original: handle.GetOracle(),
    };
    handle.SetOracle(Arc::new(FailedOracle));
    *session.session_vars.DurationWaitTS.lock().unwrap() = std::time::Duration::ZERO;
    let error = match session.Execute("update t1 set id3 = id3 + 10 where id1 = 1") {
        Err(error) => error,
        Ok(_) => panic!("failed timestamp must abort the update"),
    };
    assert!(error.to_string().contains("oracle unavailable"));
    assert_eq!(
        *session.session_vars.DurationWaitTS.lock().unwrap(),
        std::time::Duration::ZERO
    );
    drop(restore);
    session.Execute("rollback").unwrap();
    let mut result = session.Execute("select id3 from t1 where id1 = 1").unwrap();
    assert_eq!(result[0].Next().unwrap(), Some(vec!["1".to_owned()]));
    result[0].Close().unwrap();
}

#[test]
fn client_load_data_deadlock_stops_before_retry_count_and_executor_rebuild() {
    use crate::testutil::TestSession;
    use astersql_executor::adapter::{PessimisticErrorAction, PlanKind};
    let session = canonical_dml_session();
    session.Execute("begin pessimistic").unwrap();
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let sql = "insert into t values (1,10)";
    owner.BindDMLStatement(sql).unwrap();
    let _fault = astersql_testkit_testfailpoint::enable(
        "pessimisticLockReturnDeadlock",
        "1*return(true)->return(false)",
    );
    let start_ts = owner
        .session
        .state
        .borrow()
        .transaction
        .as_ref()
        .unwrap()
        .StartTS();
    let original = owner.LockKeys(&[b"record".to_vec()], false).unwrap_err();
    assert_eq!(
        original.downcast_ref::<astersql_store_mockstore_unistore_tikv::mvcc::MvccError>(),
        Some(
            &astersql_store_mockstore_unistore_tikv::mvcc::MvccError::Deadlock {
                lock_key: b"record".to_vec(),
                lock_ts: start_ts + 1,
                deadlock_key_hash:
                    astersql_store_mockstore_unistore_tikv::util::keys_to_hash_values(&[
                        b"record".to_vec()
                    ])[0],
            }
        )
    );
    assert!(matches!(
        original.downcast_ref::<astersql_store_mockstore_unistore_tikv::mvcc::MvccError>(),
        Some(astersql_store_mockstore_unistore_tikv::mvcc::MvccError::Deadlock { .. })
    ));
    assert_eq!(
        owner.OnPessimisticLockError(&original).unwrap(),
        PessimisticErrorAction::RetryReady
    );
    let mut statement = dml_stmt(owner.clone(), sql, PlanKind::Insert);
    statement.Plan.kind = PlanKind::LoadData(astersql_parser_ast::FileLocRef::Client);
    let returned = match statement.handlePessimisticLockError(original.clone()) {
        Ok(_) => panic!("client-local input must not be retried"),
        Err(error) => error,
    };
    assert!(returned.ptr_eq(&original));
    assert_eq!(statement.retryCount, 0);
    assert!(statement.retryStartTime.is_none());
    assert!(
        !owner
            .Effects()
            .events
            .iter()
            .any(|event| event.starts_with("pessimistic_retry_read_ts"))
    );
    statement.retryCount = owner.MaximumPessimisticRetries();
    let returned = match statement.handlePessimisticLockError(original.clone()) {
        Ok(_) => panic!("client input must return the deadlock even at the retry limit"),
        Err(error) => error,
    };
    assert!(returned.ptr_eq(&original));
    statement.Plan.kind = PlanKind::LoadData(astersql_parser_ast::FileLocRef::ServerOrRemote);
    let returned = match statement.handlePessimisticLockError(original.clone()) {
        Ok(_) => panic!("server input must retain the retry limit check"),
        Err(error) => error,
    };
    assert_eq!(returned.to_string(), "pessimistic lock retry limit reached");
    statement.retryCount = 0;
    // Ordinary DML keeps its existing retry and executor reopening behavior.
    statement.Plan.kind = PlanKind::Insert;
    assert!(
        statement
            .handlePessimisticLockError(original)
            .unwrap()
            .is_some()
    );
    assert_eq!(statement.retryCount, 1);
    assert!(statement.retryStartTime.is_some());
    owner.session.Execute("rollback").unwrap();
}

#[test]
fn paging_budget_reaches_real_sql_select_request() {
    use crate::testutil::TestSession;
    let session = canonical_dml_session();
    session
        .Execute("create resource group `default` ru_per_sec=1000 burstable=off")
        .unwrap();
    session
        .Execute("set global tidb_paging_size_bytes = 4194304")
        .unwrap();
    session
        .Execute("set global tidb_enable_resource_control = on")
        .unwrap();
    session.Execute("begin").unwrap();
    session
        .Execute("insert into t values (1,10),(2,20)")
        .unwrap();
    session.Execute("select * from t").unwrap();
    let request = session
        .LastSelectRequestForTest()
        .expect("SQL must construct a cop request");
    assert_eq!(request.request.Paging.PagingSizeBytes, 4_194_304);
}

fn paging_grant(
    name: &str,
    burst: i64,
) -> astersql_store_driver::resource_manager_proto::TokenBucketResponse {
    use astersql_store_driver::resource_manager_proto::*;
    TokenBucketResponse {
        resource_group_name: name.into(),
        granted_r_u_tokens: vec![GrantedRuTokenBucket {
            granted_tokens: Some(TokenBucket {
                settings: Some(TokenLimitSettings {
                    fill_rate: 1000,
                    burst_limit: burst,
                    ..Default::default()
                }),
                tokens: 1000.0,
            }),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn paging_controller(
    names: &[&str],
) -> Arc<astersql_store_driver::resource_group_runtime::ResourceGroupRuntimeStates> {
    use astersql_store_driver::resource_manager_proto::*;
    let controller = Arc::new(
        astersql_store_driver::resource_group_runtime::ResourceGroupRuntimeStates::default(),
    );
    for name in names {
        controller.register_group(&ResourceGroup {
            name: (*name).into(),
            r_u_settings: Some(GroupRequestUnitSettings {
                r_u: Some(TokenBucket {
                    settings: Some(TokenLimitSettings {
                        fill_rate: 1000,
                        burst_limit: -1,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        });
    }
    controller
}

#[test]
fn paging_runtime_grants_override_real_resource_group_metadata() {
    use super::paging::{effective_paging_size_bytes, resource_group_allows_paging_size_bytes};
    use crate::testutil::TestSession;
    let session = canonical_dml_session();
    session
        .Execute("create resource group capped ru_per_sec=1000 burstable=off")
        .unwrap();
    session
        .Execute("create resource group unlimited ru_per_sec=1000 burstable=unlimited")
        .unwrap();
    session
        .Execute("create resource group moderated ru_per_sec=1000 burstable=moderated")
        .unwrap();
    let domain = Some(&session.domain);
    let budget = 4_194_304;
    let check = |name, enabled, expected| {
        assert_eq!(
            effective_paging_size_bytes(domain, name, budget, enabled),
            expected,
            "resource group {name}, resource control {enabled}"
        );
    };
    check("default", true, 0);
    session
        .Execute("alter resource group `default` ru_per_sec=1000 burstable=off")
        .unwrap();
    check("default", true, budget);
    check("capped", true, budget);
    check("CAPPED", true, budget);
    check("unlimited", true, 0);
    check("moderated", true, 0);
    session
        .Execute("create resource group infinite_rate ru_per_sec=unlimited burstable=off")
        .unwrap();
    check("infinite_rate", true, 0);
    check("capped", false, 0);
    check("missing", true, 0);
    check("", true, 0);
    assert!(!resource_group_allows_paging_size_bytes(None, "capped"));
    for nonpositive in [0, -1] {
        assert_eq!(
            effective_paging_size_bytes(None, "", nonpositive, false),
            nonpositive
        );
    }

    let controller = paging_controller(&["capped", "unlimited", "moderated"]);
    super::SetResourceGroupRuntimeStates(&session.domain, Some(controller.clone()));
    // Registered but no response yet: schema remains authoritative.
    check("capped", true, budget);
    check("unlimited", true, 0);
    controller.handle_token_bucket_responses(&[
        paging_grant("capped", -1),
        paging_grant("unlimited", 100),
        paging_grant("moderated", 0),
    ]);
    check("capped", true, 0);
    check("unlimited", true, budget);
    check("moderated", true, budget);
    check("unlimited", false, 0);
    check("missing", true, 0);
    check("", true, 0);
    controller.tombstone_group("capped");
    check("capped", true, budget);
    super::SetResourceGroupRuntimeStates(&session.domain, None);
    check("unlimited", true, 0);
    session.Execute("drop resource group capped").unwrap();
    check("capped", true, 0);
}

#[test]
fn paging_context_caches_grants_and_budget_until_statement_retry_reset() {
    use crate::testutil::TestSession;
    let session = canonical_dml_session();
    session
        .Execute("create resource group capped ru_per_sec=1000 burstable=off")
        .unwrap();
    session
        .Execute("set global tidb_paging_size_bytes=4194304")
        .unwrap();
    session
        .Execute("set global tidb_enable_resource_control=on")
        .unwrap();
    let controller = paging_controller(&["capped"]);
    super::SetResourceGroupRuntimeStates(&session.domain, Some(controller.clone()));
    controller.handle_token_bucket_responses(&[paging_grant("capped", -1)]);
    assert_eq!(session.cop_paging_size_bytes("capped"), 0);
    controller.handle_token_bucket_responses(&[paging_grant("capped", 100)]);
    assert_eq!(session.cop_paging_size_bytes("capped"), 0);
    session.WithSessionVars(|vars| vars.StmtCtx.ResetForRetry());
    let captured = session.cop_paging_size_bytes("capped");
    assert_eq!(captured, 4_194_304);
    session
        .domain
        .set_global_system_variable("tidb_paging_size_bytes", "2097152");
    assert_eq!(session.cop_paging_size_bytes("capped"), 4_194_304);
    session.WithSessionVars(|vars| vars.StmtCtx.ResetForRetry());
    assert_eq!(session.cop_paging_size_bytes("capped"), 2_097_152);
    assert_eq!(captured, 4_194_304);
}

#[test]
fn paging_runtime_grants_are_consumed_by_sql_select_requests() {
    use crate::testutil::TestSession;
    let session = canonical_dml_session();
    session
        .Execute("create resource group `default` ru_per_sec=1000 burstable=off")
        .unwrap();
    session
        .Execute("set global tidb_paging_size_bytes=4194304")
        .unwrap();
    session
        .Execute("set global tidb_enable_resource_control=on")
        .unwrap();
    session.Execute("begin").unwrap();
    session
        .Execute("insert into t values (1,10),(2,20)")
        .unwrap();
    let controller = paging_controller(&["default"]);
    super::SetResourceGroupRuntimeStates(&session.domain, Some(controller.clone()));
    let mut captured_positive = None;
    for (burst, expected) in [(-1, 0), (100, 4_194_304), (0, 4_194_304), (-1, 0)] {
        controller.handle_token_bucket_responses(&[paging_grant("default", burst)]);
        session.Execute("select * from t").unwrap();
        let request = session
            .LastSelectRequestForTest()
            .expect("real SQL request");
        assert_eq!(request.request.Paging.PagingSizeBytes, expected);
        if expected > 0 && captured_positive.is_none() {
            captured_positive = Some(request.request.clone());
        }
    }
    assert_eq!(captured_positive.unwrap().Paging.PagingSizeBytes, 4_194_304);
    session
        .Execute("set global tidb_enable_resource_control=off")
        .unwrap();
    controller.handle_token_bucket_responses(&[paging_grant("default", 100)]);
    session.Execute("select * from t").unwrap();
    assert_eq!(
        session
            .LastSelectRequestForTest()
            .unwrap()
            .request
            .Paging
            .PagingSizeBytes,
        0
    );
    session.Execute("rollback").unwrap();
}

#[test]
fn paging_byte_budget_default_reset_through_sql() {
    use crate::testutil::TestSession;
    let session = canonical_dml_session();
    for (set_sql, expected) in [
        (None, "0"),
        (Some("set global tidb_paging_size_bytes=4194304"), "4194304"),
        (Some("set global tidb_paging_size_bytes=default"), "0"),
        (Some("set global tidb_paging_size_bytes=0"), "0"),
        (Some("set global tidb_paging_size_bytes=4194304"), "4194304"),
        (Some("set global tidb_paging_size_bytes=default"), "0"),
    ] {
        if let Some(sql) = set_sql {
            session.Execute(sql).unwrap();
        }
        let mut rows = session
            .Execute("select @@global.tidb_paging_size_bytes")
            .unwrap();
        assert_eq!(rows[0].Next().unwrap(), Some(vec![expected.to_owned()]));
        assert!(rows[0].Next().unwrap().is_none());
    }
}

#[test]
fn read_pool_slow_query_consumer_exposes_the_statement_aggregate() {
    use crate::testutil::TestSession;
    use astersql_util_execdetails::execdetails::util::PoolTaskDetails;
    let owner = SessionBoundAdapterOwner::new(canonical_dml_session());
    owner.session.WithSessionVars(|vars| {
        vars.StmtCtx
            .SyncExecDetails
            .MergeReadPoolTaskDetails(Some(&PoolTaskDetails {
                TaskCount: 2,
                PollCount: 8,
                MaxPollCount: 4,
                MinPollCount: 4,
                ..Default::default()
            }));
    });
    let expected = owner.session.WithSessionVars(|vars| {
        vars.StmtCtx
            .GetExecDetails()
            .ReadPoolTaskDetails
            .unwrap()
            .String()
    });
    owner.SetProcessInfo(
        "select b from t where a=1",
        std::time::SystemTime::now() - std::time::Duration::from_secs(1),
        3,
        0,
    );
    owner.SlowQuery(0, "select b from t where a=1", true, false);
    let mut rows = owner
        .session
        .Execute("select read_pool_task_details from information_schema.slow_query")
        .unwrap();
    let row = rows[0].Next().unwrap().expect("slow query row");
    assert_eq!(row[0], expected);
}

#[test]
fn read_pool_point_finish_runtime_is_attached_once_without_disabling_ru_evidence() {
    use astersql_util_execdetails::execdetails::{NewRuntimeStatsColl, util::PoolTaskDetails};
    let mut session = canonical_dml_session();
    Arc::get_mut(&mut session.session_vars)
        .unwrap()
        .StmtCtx
        .RuntimeStatsColl = Some(Arc::new(NewRuntimeStatsColl(None)));
    let owner = SessionBoundAdapterOwner::new(session);
    let pool = PoolTaskDetails {
        TaskCount: 2,
        PollCount: 8,
        MaxPollCount: 4,
        MinPollCount: 4,
        ..Default::default()
    };
    owner.point_read_stats_active.set(true);
    owner.session.WithSessionVars(|vars| {
        vars.StmtCtx
            .SyncExecDetails
            .MergeReadPoolTaskDetails(Some(&pool));
    });
    owner.OnFinishStatement(0, false, 0);
    owner.OnFinishStatement(0, false, 0);
    owner.AttachFinishRuntimeStats(57);
    owner.AttachFinishRuntimeStats(57);
    assert!(owner.point_read_stats_active.get());
    owner.session.WithSessionVars(|vars| {
        assert_eq!(
            vars.StmtCtx
                .GetExecDetails()
                .ReadPoolTaskDetails
                .unwrap()
                .TaskCount,
            2
        );
        assert!(
            vars.StmtCtx
                .RuntimeStatsColl
                .as_ref()
                .unwrap()
                .GetRootStatsStringShared(57)
                .contains(&format!("read_pool:{}", pool.String()))
        );
    });
}

#[test]
fn read_pool_canonical_count_consumer_keeps_completed_stats_on_decode_error() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Subset(Vec<u8>);
    impl kv::ResultSubset for Subset {
        fn ReadPoolTaskDetails(&self) -> Option<kv::PoolTaskDetails> {
            Some(kv::PoolTaskDetails {
                TaskCount: 1,
                PollCount: 4,
                MaxPollCount: 4,
                MinPollCount: 4,
                ..Default::default()
            })
        }
        fn GetData(&self) -> &[u8] {
            &self.0
        }
        fn GetStartKey(&self) -> kv::Key {
            kv::Key(Vec::new())
        }
        fn MemSize(&self) -> i64 {
            self.0.len() as i64
        }
        fn RespTime(&self) -> std::time::Duration {
            std::time::Duration::ZERO
        }
    }
    struct Response {
        data: Option<Vec<u8>>,
        closed: Arc<AtomicUsize>,
    }
    impl kv::Response for Response {
        fn Next(
            &mut self,
            _: &kv::Context,
        ) -> Result<Option<Box<dyn kv::ResultSubset>>, kv::errors::SharedError> {
            Ok(self
                .data
                .take()
                .map(|data| Box::new(Subset(data)) as Box<dyn kv::ResultSubset>))
        }
        fn Close(&mut self) -> Result<(), kv::errors::SharedError> {
            self.closed.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }
    struct Client {
        data: Vec<u8>,
        closed: Arc<AtomicUsize>,
    }
    impl kv::Client for Client {
        fn Send(
            &self,
            _: &kv::Context,
            _: &kv::Request,
            _: &dyn std::any::Any,
            _: &kv::ClientSendOption,
        ) -> Option<Box<dyn kv::Response>> {
            Some(Box::new(Response {
                data: Some(self.data.clone()),
                closed: self.closed.clone(),
            }))
        }
        fn IsRequestTypeSupported(&self, _: i64, _: i64) -> bool {
            true
        }
    }
    let session = canonical_dml_session();
    let request = super::relational_scan::relational_coprocessor_request(
        &astersql_meta_model::TableInfo {
            ID: 123,
            Columns: vec![astersql_meta_model::ColumnInfo {
                ID: 1,
                FieldType: astersql_parser_types::NewFieldType(
                    astersql_parser_mysql::r#type::TypeLonglong,
                ),
                ..Default::default()
            }],
            ..Default::default()
        },
        10,
    )
    .unwrap();
    let option = kv::ClientSendOption {
        SessionMemTracker: None,
        EnabledRateLimitAction: false,
        EventCb: None,
        EnableCollectExecutionInfo: true,
        TiFlashReplicaRead: Default::default(),
        AppendWarning: None,
        TryCopLiteWorker: None,
    };
    let mut response = tipb::ChecksumResponse::new();
    response.set_total_kvs(3);
    let data = protobuf::Message::write_to_bytes(&response).unwrap();
    let closed = Arc::new(AtomicUsize::new(0));
    let client = Client {
        data,
        closed: closed.clone(),
    };
    session.WithSessionVars(|vars| {
        let count = super::relational_scan::execute_relational_count_request(
            &client,
            &kv::Context::todo(),
            &request,
            &option,
            true,
            &vars.StmtCtx.SyncExecDetails,
        )
        .unwrap();
        assert_eq!(count, 3);
        assert_eq!(
            vars.StmtCtx
                .GetExecDetails()
                .ReadPoolTaskDetails
                .unwrap()
                .TaskCount,
            1
        );
        let invalid = Client {
            data: vec![0xff],
            closed: closed.clone(),
        };
        assert!(
            super::relational_scan::execute_relational_count_request(
                &invalid,
                &kv::Context::todo(),
                &request,
                &option,
                true,
                &vars.StmtCtx.SyncExecDetails
            )
            .is_err()
        );
        let pool = vars.StmtCtx.GetExecDetails().ReadPoolTaskDetails.unwrap();
        assert_eq!(
            (
                pool.TaskCount,
                pool.PollCount,
                pool.MaxPollCount,
                pool.MinPollCount
            ),
            (2, 8, 4, 4)
        );
    });
    assert_eq!(closed.load(Ordering::Acquire), 2);
}

#[test]
fn read_pool_point_builder_reinitializes_diagnostics_for_cached_fast_path() {
    use astersql_infoschema::infoschema::{CiString, InfoSchema};
    let session = canonical_dml_session();
    let model = session
        .domain
        .info_schema()
        .ModelTableInfoByName(&CiString::new("test"), &CiString::new("t"))
        .unwrap();
    let mut version = session
        .domain
        .storage()
        .with_storage(|store| store.CurrentVersion("global"))
        .unwrap();
    version.Ver = u64::MAX;
    let owner = Arc::new(SessionBoundAdapterOwner::new(session));
    let mut physical = astersql_planner_core_operator_physicalop::PointGetPlan::New(
        owner.session.AdapterPlanContext(),
    );
    physical.TblInfo = Some(model.as_ref().clone());
    physical.Handle = Some(1);
    physical.Columns = model.Columns.clone();
    owner
        .BindTypedPhysicalPlan(Box::new(physical), Vec::new(), version, 1, 1)
        .unwrap();
    let statement = point_lock_stmt(owner.clone(), "select b from t where a=1", &model.Columns);
    for _ in 0..2 {
        owner.point_read_pool_merged.set(true);
        owner.point_read_pool_runtime_registered.set(true);
        let previous = owner.point_read_stats.lock().unwrap().clone();
        let mut executor = owner
            .BuildPointGetExecutor(&statement.Plan, version.Ver, Some("read-pool-cache"))
            .unwrap();
        assert!(!owner.point_read_pool_merged.get());
        assert!(!owner.point_read_pool_runtime_registered.get());
        assert!(!Arc::ptr_eq(
            &previous,
            &owner.point_read_stats.lock().unwrap()
        ));
        executor.Open().unwrap();
        executor.Close().unwrap();
    }
    assert!(
        owner
            .Effects()
            .events
            .iter()
            .any(|event| event == "point_get_cache_hit")
    );
}

#[test]
fn imported_integer_primary_key_is_restored_from_handle() {
    use astersql_meta_model::{ColumnInfo, TableInfo};
    use astersql_parser_mysql::r#type as mysql;
    for (unsigned, expected) in [(false, "-1"), (true, "18446744073709551615")] {
        let mut primary = ColumnInfo::default();
        primary.ID = 7;
        primary.Name = astersql_meta_model::ast::NewCIStr("a");
        primary.SetType(mysql::TypeLonglong);
        primary.SetFlag(mysql::PriKeyFlag | if unsigned { mysql::UnsignedFlag } else { 0 });
        let mut text = ColumnInfo::default();
        text.ID = 12;
        text.Offset = 1;
        text.Name = astersql_meta_model::ast::NewCIStr("b");
        text.SetType(mysql::TypeVarchar);
        let table = TableInfo {
            Columns: vec![primary, text],
            PKIsHandle: true,
            ..Default::default()
        };
        let fields = table
            .Columns
            .iter()
            .map(|column| (column.ID, Box::new(column.FieldType.clone())))
            .collect();
        let value = astersql_tablecodec::EncodeRow(
            Some(astersql_tablecodec::time::UTC),
            vec![{
                let mut datum = astersql_tablecodec::types::Datum::default();
                datum.SetString("test-1".into(), "utf8mb4_bin".into());
                datum
            }],
            vec![12],
            Vec::new(),
            None,
            None,
            astersql_tablecodec::rowcodec::Encoder::new(true),
        )
        .unwrap();
        let handle = astersql_tablecodec::kv::IntHandle(-1);
        let (_, row) =
            super::relational_scan::decode_relational_row_value(&table, &fields, &handle, &value)
                .unwrap();
        assert_eq!(row.get("a").and_then(|v| v.as_deref()), Some(expected));
        assert_eq!(row.get("b").and_then(|v| v.as_deref()), Some("test-1"));
    }
}

#[test]
fn foreign_key_shared_lock_sql_gate_and_persisted_initialization() {
    struct Restore(Option<Box<dyn FnOnce()>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            self.0.take().unwrap()();
        }
    }
    let _restore = Restore(Some(Box::new(astersql_config::restore_func())));
    astersql_config::update_global(|conf| {
        conf.experimental
            .allow_enable_foreign_key_check_in_shared_lock = false
    });
    let (domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    for scope in ["session", "global"] {
        for input in ["ON", "1"] {
            let result = session.execute(&format!(
                "SET @@{scope}.tidb_foreign_key_check_in_shared_lock = {input}"
            ));
            if astersql_config_kerneltype::IsNextGen() {
                let error = match result {
                    Ok(_) => panic!("NextGen must reject {scope} {input}"),
                    Err(error) => error,
                };
                assert!(
                    error.to_string().contains("can't be set to the value"),
                    "{error}"
                );
                assert!(!session.state.borrow().foreign_key_check_in_shared_lock);
            } else {
                result.unwrap();
            }
            session
                .execute(&format!(
                    "SET @@{scope}.tidb_foreign_key_check_in_shared_lock = OFF"
                ))
                .unwrap();
        }
    }
    astersql_config::update_global(|conf| {
        conf.experimental
            .allow_enable_foreign_key_check_in_shared_lock = true
    });
    for input in ["ON", "1"] {
        session
            .execute(&format!(
                "SET @@global.tidb_foreign_key_check_in_shared_lock = {input}"
            ))
            .unwrap();
        assert!(
            !session.state.borrow().foreign_key_check_in_shared_lock,
            "GLOBAL SET must not mutate current session"
        );
        session
            .execute(&format!(
                "SET @@session.tidb_foreign_key_check_in_shared_lock = {input}"
            ))
            .unwrap();
        assert!(session.state.borrow().foreign_key_check_in_shared_lock);
        session
            .execute("SET @@session.tidb_foreign_key_check_in_shared_lock = OFF")
            .unwrap();
    }
    astersql_config::update_global(|conf| {
        conf.experimental
            .allow_enable_foreign_key_check_in_shared_lock = false
    });
    let historical = crate::runtime::ConcreteSession::new(domain);
    assert!(historical.state.borrow().foreign_key_check_in_shared_lock);
    let mut sets = historical.execute("SELECT @@session.tidb_foreign_key_check_in_shared_lock, @@global.tidb_foreign_key_check_in_shared_lock").unwrap();
    assert_eq!(
        sets[0].next_row().unwrap(),
        Some(vec!["ON".into(), "ON".into()])
    );
    historical
        .execute("SET @@session.tidb_foreign_key_check_in_shared_lock = OFF")
        .unwrap();
    assert!(!historical.state.borrow().foreign_key_check_in_shared_lock);
}

thread_local! {
    static STATEMENT_RU_POST_COMPILE: std::cell::RefCell<Option<Box<dyn Fn(&astersql_executor::adapter::ExecStmt) -> crate::SessionResult<()>>>> = const { std::cell::RefCell::new(None) };
}

thread_local! {
    static STATEMENT_RU_POST_RUN: std::cell::RefCell<Option<Box<dyn Fn() -> crate::SessionResult<()>>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn statement_ru_post_run() -> crate::SessionResult<()> {
    STATEMENT_RU_POST_RUN.with(|hook| match hook.borrow().as_ref() {
        Some(hook) => hook(),
        None => Ok(()),
    })
}

pub(super) fn statement_ru_post_compile(
    stmt: &astersql_executor::adapter::ExecStmt,
) -> crate::SessionResult<()> {
    STATEMENT_RU_POST_COMPILE.with(|hook| match hook.borrow().as_ref() {
        Some(hook) => hook(stmt),
        None => Ok(()),
    })
}

fn statement_ru_post_compile_cases(mode: u8) {
    use astersql_executor::statement_ru_plan_walk::StatementRUFinalOutcome;
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            STATEMENT_RU_POST_COMPILE.with(|hook| *hook.borrow_mut() = None);
            STATEMENT_RU_POST_RUN.with(|hook| *hook.borrow_mut() = None);
        }
    }
    for fault in 0..if mode != 0 { 5 } else { 3 } {
        let (domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
        session
            .execute("create table ru_terminal (id int primary key)")
            .unwrap();
        session
            .execute("insert into ru_terminal values (11), (22)")
            .unwrap();
        let prepared = session
            .PreparePlannedKVSelect(
                "select id from ru_terminal order by id",
                domain.info_schema(),
            )
            .unwrap();
        if mode == 2 || mode == 4 {
            session
                .execute("prepare ru_stmt from 'select id from ru_terminal limit 2'")
                .unwrap();
        }
        let captured = std::rc::Rc::new(std::cell::RefCell::new(None));
        let saved = captured.clone();
        let _reset = Reset;
        STATEMENT_RU_POST_COMPILE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |stmt| {
                *saved.borrow_mut() = Some(
                    stmt.StatementCtx
                        .statement_ru_owner
                        .clone()
                        .expect("real compiled statement installs owner"),
                );
                match fault {
                    1 => Err(crate::SessionError::new("post compile failure")),
                    2 => panic!("post compile panic"),
                    _ => Ok(()),
                }
            }))
        });
        STATEMENT_RU_POST_RUN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || match fault {
                3 => Err(crate::SessionError::new("session finish failure")),
                4 => panic!("session post run panic"),
                _ => Ok(()),
            }))
        });
        let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if mode == 3 {
                session
                    .execute_with_load_data_reader(
                        "do 1",
                        std::io::Cursor::new(b"transfer payload".to_vec()),
                    )
                    .map(|results| {
                        assert!(results.iter().all(|result| result.columns().is_empty()));
                    })
            } else if mode != 0 {
                let sql = if mode == 1 {
                    "explain analyze select id from ru_terminal limit 2"
                } else {
                    "execute ru_stmt"
                };
                let execution = if mode == 4 {
                    session
                        .execute_with_load_data_reader(sql, std::io::Cursor::new(Vec::<u8>::new()))
                } else {
                    session.execute(sql)
                };
                execution.map(|mut results| {
                    let result = results.first_mut().expect("EXPLAIN rows");
                    let mut rows = Vec::new();
                    while let Some(row) = result.next_row().unwrap() {
                        rows.push(row);
                    }
                    if mode == 1 {
                        assert!(
                            rows.iter().any(|row| row[2] == "2"),
                            "actual rows include the nonempty KV scan"
                        );
                    } else {
                        assert_eq!(rows, vec![vec!["11".to_string()], vec!["22".to_string()]]);
                    }
                    result.close().unwrap();
                })
            } else {
                session
                    .ExecutePreparedPlannedKVSelectThroughAdapter(prepared, &[])
                    .map(|result| {
                        assert_eq!(
                            result
                                .Rows
                                .iter()
                                .map(|row| row.0.clone())
                                .collect::<Vec<_>>(),
                            vec![
                                vec![astersql_executor_sortexec::SortValue::Int(11)],
                                vec![astersql_executor_sortexec::SortValue::Int(22)],
                            ]
                        );
                    })
            }
        }));
        let owner = captured.borrow().clone().expect("compiled owner captured");
        match fault {
            0 => {
                execution.unwrap().unwrap();
                assert_eq!(owner.final_outcome(), StatementRUFinalOutcome::Success);
            }
            1 | 3 => {
                assert_eq!(
                    execution.unwrap().unwrap_err().to_string(),
                    if fault == 1 {
                        "post compile failure"
                    } else {
                        "session finish failure"
                    }
                );
                assert_eq!(owner.final_outcome(), StatementRUFinalOutcome::Failure);
            }
            _ => {
                assert!(execution.is_err());
                assert_eq!(owner.final_outcome(), StatementRUFinalOutcome::Failure);
            }
        }
        assert!(
            owner.take_terminal_setup().is_none(),
            "every outcome consumes terminal responsibility"
        );
        assert_eq!(session.inner.statement_ru_scope_depth.get(), 0);
        assert!(session.inner.statement_ru_pending.borrow().is_none());
        assert!(session.inner.statement_ru_delayed.borrow().is_none());
        assert!(!session.has_file_transfer_reader());
        domain.close();
    }
}

#[test]
fn prepared_statement_ru_post_compile_outcome_and_terminal() {
    statement_ru_post_compile_cases(0);
}

#[test]
fn explain_statement_ru_post_compile_outcome_and_terminal() {
    statement_ru_post_compile_cases(1);
}

#[test]
fn execute_statement_ru_post_compile_outcome_and_terminal() {
    statement_ru_post_compile_cases(2);
}

#[test]
fn file_transfer_statement_ru_post_compile_outcome_and_terminal() {
    statement_ru_post_compile_cases(3);
}

#[test]
fn file_transfer_result_set_statement_ru_post_compile_outcome_and_terminal() {
    statement_ru_post_compile_cases(4);
}
