// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 预处理计划缓存运行时测试：索引范围重建与 IN 列表过长时的缓存跳过。
//
// 在 Mock Domain / InfoSchema 上验证 `PreparePlannedKVSelect` 与
// `ExecutePreparedPlannedKVSelect`：短 IN 列表可命中缓存并重建 IndexRange；
// 复合 IN 超出 `tidb_opt_range_max_size` 时告警并拒绝写入缓存。

use std::sync::Arc;

use astersql_domain::{Domain, InfoSchemaLoader, LoadedInfoSchema};
use astersql_infoschema::{self as infoschema, SchemaRef};
use astersql_kv as kv;
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage};

use crate::runtime::ConcreteSession;
use crate::testutil::TestSession;

#[test]
fn prepared_covering_index_select_uses_index_reader_physical_tree() {
    fn has_reader(plan: &dyn astersql_planner_core_base::PhysicalPlan) -> bool {
        plan.as_any()
            .is::<astersql_planner_core_operator_physicalop::PhysicalIndexReader>()
            || plan.children().into_iter().any(has_reader)
    }
    let schema = cache_info_schema();
    let domain = Arc::new(Domain::new_mock(
        Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).expect("create KV store"))
            .unwrap_or_else(|_| panic!("unexpected store owner")),
        Arc::new(CacheSchemaLoader {
            schema: Arc::clone(&schema),
        }),
    ));
    domain.init().expect("initialize canonical domain");
    let mut seed = domain
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("begin index-only KV seed");
    for (handle, a, b) in [(1, "aa", "b1"), (2, "bb", "b2"), (3, "aa", "b3")] {
        let mut suffix = astersql_util_codec::EncodeKey(
            astersql_tablecodec::time::UTC,
            Vec::new(),
            vec![
                astersql_types::datum::NewStringDatum(a.into()),
                astersql_types::datum::NewStringDatum(b.into()),
            ],
        )
        .expect("encode covered index values");
        suffix.push(astersql_util_codec::IntHandleFlag);
        suffix = astersql_util_codec::EncodeInt(suffix, handle);
        let key = astersql_tablecodec::EncodeIndexSeekKey(101, 201, Some(suffix));
        seed.Set(kv::Key(key.0), vec![0])
            .expect("seed covered index only");
    }
    seed.Commit(&kv::Context::default())
        .expect("commit index-only KV seed");
    let session = ConcreteSession::new(Arc::clone(&domain));
    let id = session
        .PreparePlannedKVSelect("select a, b from t use index(idx_a_b) where a = ?", schema)
        .expect("prepare covered indexed SELECT");
    let planned = session
        .PlanPreparedPlannedKVSelect(id, &[astersql_types::datum::NewStringDatum("aa".into())])
        .expect("plan bound covered index SELECT");
    assert!(
        has_reader(planned.Plan.as_ref()),
        "optimizer selected {}",
        planned.Plan.tp(&[])
    );
    let owner = crate::runtime::SessionBoundAdapterOwner::new(session);
    owner
        .BindPreparedPlannedKVSelect(
            id,
            &[astersql_types::datum::NewStringDatum("aa".into())],
            1,
            1,
        )
        .expect("bind covered index range");
    use astersql_executor::adapter::{AdapterRuntime, PlanInfo, PlanKind};
    let plan_info = PlanInfo {
        id: 42,
        kind: PlanKind::Query,
        schema: Vec::new(),
        calculate_no_delay: false,
        projection_child: None,
        encoded: String::new(),
        binary: String::new(),
        hints: String::new(),
    };
    let mut executor = owner
        .BuildExecutor(&plan_info, None)
        .expect("build index-only typed tree");
    executor.Open().expect("open index-only reader");
    let mut output = executor.NewChunk();
    for (handle, b) in [(1, b"b1".as_slice()), (3, b"b3".as_slice())] {
        executor
            .Next(&mut output)
            .expect("fetch covered index row without table KV");
        assert_eq!(output.GetRow(0).GetBytes(0), b"aa");
        assert_eq!(output.GetRow(0).GetBytes(1), b);
        let record_key =
            astersql_tablecodec::EncodeRowKeyWithHandle(101, Box::new(kv::IntHandle(handle)));
        assert_eq!(executor.TakeLockKeys(), vec![record_key.0]);
    }
    executor.Next(&mut output).expect("covered index EOF");
    assert_eq!(output.NumRows(), 0);
    assert_eq!(executor.ScannedRows(), 2);
    executor.Close().expect("close covered index reader");
    owner.FinalizePreparedExecution(2, true);
    owner
        .BindPreparedPlannedKVSelect(
            id,
            &[astersql_types::datum::NewStringDatum("bb".into())],
            1,
            1,
        )
        .expect("restore covered index reader with rebound parameter");
    assert!(owner.LastPlanFromCache());
    let mut cached = owner
        .BuildExecutor(&plan_info, None)
        .expect("build restored index reader");
    cached.Open().expect("open restored reader");
    cached.Next(&mut output).expect("read rebound bb index row");
    assert_eq!(output.GetRow(0).GetBytes(0), b"bb");
    assert_eq!(output.GetRow(0).GetBytes(1), b"b2");
    let record_key = astersql_tablecodec::EncodeRowKeyWithHandle(101, Box::new(kv::IntHandle(2)));
    assert_eq!(cached.TakeLockKeys(), vec![record_key.0]);
    cached.Close().expect("close cached reader");
}

#[test]
fn prepared_uncovered_index_select_uses_a_double_read_physical_plan() {
    fn has_index_lookup(plan: &dyn astersql_planner_core_base::PhysicalPlan) -> bool {
        plan.as_any()
            .is::<astersql_planner_core_operator_physicalop::PhysicalIndexLookUpReader>()
            || plan.children().into_iter().any(has_index_lookup)
    }
    let schema = cache_info_schema();
    let domain = Arc::new(Domain::new_mock(
        Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).expect("create KV store"))
            .unwrap_or_else(|_| panic!("unexpected store owner")),
        Arc::new(CacheSchemaLoader {
            schema: Arc::clone(&schema),
        }),
    ));
    domain.init().expect("initialize canonical domain");
    let mut seed = domain
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("begin canonical KV seed");
    for (handle, a, b, c) in [
        (1, "aa", "b1", "c1"),
        (2, "bb", "b2", "c2"),
        (3, "aa", "b3", "c3"),
    ] {
        let record_key =
            astersql_tablecodec::EncodeRowKeyWithHandle(101, Box::new(kv::IntHandle(handle)));
        let record_value = astersql_tablecodec::EncodeRow(
            Some(astersql_tablecodec::time::UTC),
            vec![
                astersql_tablecodec::types::NewStringDatum(a.into()),
                astersql_tablecodec::types::NewStringDatum(b.into()),
                astersql_tablecodec::types::NewStringDatum(c.into()),
            ],
            vec![1, 2, 3],
            Vec::new(),
            None,
            None,
            astersql_tablecodec::rowcodec::Encoder::new(true),
        )
        .expect("encode canonical record");
        seed.Set(kv::Key(record_key.0), record_value)
            .expect("seed record");
        let mut index_suffix = astersql_util_codec::EncodeKey(
            astersql_tablecodec::time::UTC,
            Vec::new(),
            vec![
                astersql_types::datum::NewStringDatum(a.into()),
                astersql_types::datum::NewStringDatum(b.into()),
            ],
        )
        .expect("encode indexed columns");
        index_suffix.push(astersql_util_codec::IntHandleFlag);
        index_suffix = astersql_util_codec::EncodeInt(index_suffix, handle);
        let index_key = astersql_tablecodec::EncodeIndexSeekKey(101, 201, Some(index_suffix));
        seed.Set(kv::Key(index_key.0), vec![0])
            .expect("seed canonical nonunique index entry");
    }
    seed.Commit(&kv::Context::default())
        .expect("commit canonical indexed rows");
    let session = ConcreteSession::new(Arc::clone(&domain));
    let id = session
        .PreparePlannedKVSelect(
            "select * from t use index(idx_a_b) where a = ? for update",
            schema,
        )
        .expect("prepare indexed SELECT");
    let planned = session
        .PlanPreparedPlannedKVSelect(id, &[astersql_types::datum::NewStringDatum("aa".into())])
        .expect("plan bound index SELECT");
    assert!(
        has_index_lookup(planned.Plan.as_ref()),
        "canonical optimizer selected {}",
        planned.Plan.tp(&[])
    );
    session
        .Execute("begin pessimistic")
        .expect("begin indexed locking transaction");
    let peer_session = ConcreteSession::new(Arc::clone(&domain));
    peer_session
        .Execute("begin pessimistic")
        .expect("begin competing transaction");
    let owner = Arc::new(crate::runtime::SessionBoundAdapterOwner::new(session));
    let peer = crate::runtime::SessionBoundAdapterOwner::new(peer_session);
    owner
        .BindPreparedPlannedKVSelect(
            id,
            &[astersql_types::datum::NewStringDatum("aa".into())],
            1,
            1,
        )
        .expect("bind canonical parameterized index range to typed double read");
    use astersql_executor::adapter::{AdapterRuntime, PlanInfo, PlanKind};
    let plan_info = PlanInfo {
        id: 42,
        kind: PlanKind::Query,
        schema: Vec::new(),
        calculate_no_delay: false,
        projection_child: None,
        encoded: String::new(),
        binary: String::new(),
        hints: String::new(),
    };
    assert!(
        owner.BuildExecutor(&plan_info, None).is_err(),
        "SelectLock cannot run outside locking adapter path"
    );
    let mut executor = owner
        .BuildExecutorForSelectLock(&plan_info, None)
        .expect("build typed index lookup from canonical physical plan");
    executor.Open().expect("open lazy index lookup");
    let mut output = executor.NewChunk();
    executor.Next(&mut output).expect("fetch indexed aa row");
    assert_eq!(output.NumRows(), 1);
    assert_eq!(output.GetRow(0).GetBytes(0), b"aa");
    assert_eq!(output.GetRow(0).GetBytes(2), b"c1");
    let first_record = astersql_tablecodec::EncodeRowKeyWithHandle(101, Box::new(kv::IntHandle(1)));
    assert_eq!(executor.TakeLockKeys(), vec![first_record.0.clone()]);
    executor.Next(&mut output).expect("second indexed aa row");
    assert_eq!(output.GetRow(0).GetBytes(2), b"c3");
    let third_record = astersql_tablecodec::EncodeRowKeyWithHandle(101, Box::new(kv::IntHandle(3)));
    assert_eq!(executor.TakeLockKeys(), vec![third_record.0.clone()]);
    executor.Next(&mut output).expect("indexed point range EOF");
    assert_eq!(output.NumRows(), 0);
    assert_eq!(executor.ScannedRows(), 2);
    executor.Close().expect("close typed double read");
    // Commit a newer record version after the typed statement pinned its
    // first read TS. The index key stays unchanged, so the lock must detect
    // the record's CommitTS and retry the full double read.
    let mut writer = domain
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("begin concurrent canonical KV writer");
    let updated = astersql_tablecodec::EncodeRow(
        Some(astersql_tablecodec::time::UTC),
        vec![
            astersql_tablecodec::types::NewStringDatum("aa".into()),
            astersql_tablecodec::types::NewStringDatum("b3".into()),
            astersql_tablecodec::types::NewStringDatum("new-c3".into()),
        ],
        vec![1, 2, 3],
        Vec::new(),
        None,
        None,
        astersql_tablecodec::rowcodec::Encoder::new(true),
    )
    .expect("encode concurrent record update");
    writer
        .Set(kv::Key(third_record.0.clone()), updated)
        .expect("write updated record");
    writer
        .Commit(&kv::Context::default())
        .expect("commit newer record version");
    use astersql_executor::adapter::{
        ExecStmt, FieldName, Priority, SchemaColumn, StatementContext, StatementKind, StatementNode,
    };
    let sql = "execute indexed_select";
    let field_type =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeVarchar);
    let mut stmt = ExecStmt {
        GoCtx: None,
        InfoSchema: 0,
        Plan: PlanInfo {
            id: 42,
            kind: PlanKind::Query,
            schema: vec![
                SchemaColumn {
                    field_type: field_type.clone()
                };
                3
            ],
            calculate_no_delay: false,
            projection_child: None,
            encoded: "index lookup".into(),
            binary: String::new(),
            hints: String::new(),
        },
        TypedPlan: None,
        StmtNode: StatementNode {
            kind: StatementKind::Execute,
            original_text: sql.into(),
            text: sql.into(),
            secure_text: sql.into(),
            prepared_text: None,
        },
        Ctx: owner.clone(),
        LowerPriority: false,
        isPreparedStmt: true,
        isSelectForUpdate: true,
        retryCount: 0,
        retryStartTime: None,
        phaseBuildDurations: [std::time::Duration::ZERO; 2],
        phaseOpenDurations: [std::time::Duration::ZERO; 2],
        phaseNextDurations: [std::time::Duration::ZERO; 2],
        phaseLockDurations: [std::time::Duration::ZERO; 2],
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
            statement_type: "Execute".into(),
            sql_normalized: sql.into(),
            ..Default::default()
        },
    };
    owner.AddFoundRows(9);
    let mut locked = stmt
        .Exec()
        .expect("execute canonical prepared indexed FOR UPDATE")
        .expect("buffer locking rows after record locks");
    assert_eq!(
        stmt.retryCount, 1,
        "stale record version triggers one Go-style pessimistic retry"
    );
    assert_eq!(
        owner.StatementFoundRows(),
        0,
        "canonical statement row counters reset at the retry boundary"
    );
    assert!(
        owner
            .Effects()
            .events
            .iter()
            .any(|event| event == "pessimistic_retry_rollback:1"),
        "retry must release the first attempt's one newly held record lock"
    );
    assert_eq!(owner.HeldRowLockCount(), 2);
    assert!(
        peer.TryLockKeys(&[first_record.0.clone()]).is_err(),
        "peer cannot lock returned record"
    );
    assert!(
        peer.TryLockKeys(&[third_record.0.clone()]).is_err(),
        "peer cannot lock the row updated during retry"
    );
    let mut page = locked.NewChunk();
    locked.Next(&mut page).expect("return locked indexed row");
    assert_eq!(page.GetRow(0).GetBytes(0), b"aa");
    assert_eq!(page.GetRow(0).GetBytes(2), b"c1");
    locked
        .Next(&mut page)
        .expect("return second locked indexed row");
    assert_eq!(page.GetRow(0).GetBytes(2), b"new-c3");
    locked.Close().expect("finish indexed FOR UPDATE result");
    owner
        .BindPreparedPlannedKVSelect(
            id,
            &[astersql_types::datum::NewStringDatum("bb".into())],
            1,
            1,
        )
        .expect("rebind canonical index range after first locking statement");
    assert!(
        owner.LastPlanFromCache(),
        "successful indexed FOR UPDATE admits the prepared plan"
    );
    let mut rebound = stmt.clone();
    rebound.GoCtx = None;
    rebound.StmtNode.kind = StatementKind::Execute;
    let mut rebound_rows = rebound
        .Exec()
        .expect("execute cached indexed FOR UPDATE")
        .expect("buffer second locked indexed row");
    let second_record =
        astersql_tablecodec::EncodeRowKeyWithHandle(101, Box::new(kv::IntHandle(2)));
    assert!(
        peer.TryLockKeys(&[second_record.0]).is_err(),
        "cached range locks the new record"
    );
    rebound_rows
        .Next(&mut page)
        .expect("return cached indexed row");
    assert_eq!(page.GetRow(0).GetBytes(0), b"bb");
    assert_eq!(owner.HeldRowLockCount(), 3);
    rebound_rows
        .Close()
        .expect("finish cached indexed lock statement");
}

/// 固定返回同一 Schema 的 InfoSchema 加载器，避免真实元数据依赖。
struct CacheSchemaLoader {
    schema: SchemaRef,
}

impl InfoSchemaLoader for CacheSchemaLoader {
    fn load_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        Ok(LoadedInfoSchema::new(Arc::clone(&self.schema), 10))
    }

    fn load_snapshot_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
        timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        Ok(LoadedInfoSchema::new(Arc::clone(&self.schema), timestamp))
    }

    fn keyspace_exists(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        Ok(true)
    }
}

/// 构造含复合索引 `idx_a_b(a,b)` 的单表 InfoSchema，供计划缓存用例使用。
fn cache_info_schema() -> SchemaRef {
    let column = |id: i64, name: &str, offset: isize| {
        let mut field_type =
            astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeVarchar);
        field_type.SetCharset("utf8mb4".to_owned());
        field_type.SetCollate("utf8mb4_bin".to_owned());
        astersql_meta_model::ColumnInfo {
            ID: id,
            Name: astersql_parser_ast::NewCIStr(name),
            Offset: offset,
            State: astersql_meta_model::StatePublic,
            FieldType: field_type,
            ..Default::default()
        }
    };
    let columns = vec![column(1, "a", 0), column(2, "b", 1), column(3, "c", 2)];
    let model = Arc::new(astersql_meta_model::TableInfo {
        ID: 101,
        Name: astersql_parser_ast::NewCIStr("t"),
        Charset: "utf8mb4".to_owned(),
        Collate: "utf8mb4_bin".to_owned(),
        Columns: columns.clone(),
        Indices: vec![astersql_meta_model::IndexInfo {
            ID: 201,
            Name: astersql_parser_ast::NewCIStr("idx_a_b"),
            State: astersql_meta_model::StatePublic,
            Columns: vec![
                astersql_meta_model::IndexColumn {
                    Name: astersql_parser_ast::NewCIStr("a"),
                    Offset: 0,
                    Length: astersql_parser_types::UnspecifiedLength,
                    ..Default::default()
                },
                astersql_meta_model::IndexColumn {
                    Name: astersql_parser_ast::NewCIStr("b"),
                    Offset: 1,
                    Length: astersql_parser_types::UnspecifiedLength,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    });
    infoschema::infoschema::MockInfoSchema(vec![infoschema::infoschema::TableInfo {
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
    }])
}

#[test]
fn canonical_point_get_plan_opens_an_owned_lazy_record_getter() {
    use astersql_executor::adapter::{
        ExecStmt, FieldName, PlanInfo, PlanKind, Priority, SchemaColumn, StatementContext,
        StatementKind, StatementNode,
    };
    use astersql_infoschema::infoschema::{CiString, InfoSchema};
    let schema = cache_info_schema();
    let model = schema
        .ModelTableInfoByName(&CiString::new("test"), &CiString::new("t"))
        .expect("lookup canonical table metadata");
    let domain = Arc::new(Domain::new_mock(
        Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).unwrap())
            .unwrap_or_else(|_| panic!("unexpected storage owner")),
        Arc::new(CacheSchemaLoader { schema }),
    ));
    domain.init().unwrap();
    let key = astersql_tablecodec::EncodeRowKeyWithHandle(101, Box::new(kv::IntHandle(4)));
    let value = astersql_tablecodec::EncodeRow(
        Some(astersql_tablecodec::time::UTC),
        vec![
            astersql_tablecodec::types::NewStringDatum("aa".into()),
            astersql_tablecodec::types::NewStringDatum("b4".into()),
            astersql_tablecodec::types::NewStringDatum("c4".into()),
        ],
        vec![1, 2, 3],
        Vec::new(),
        None,
        None,
        astersql_tablecodec::rowcodec::Encoder::new(true),
    )
    .unwrap();
    let mut seed = domain
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .unwrap();
    seed.Set(kv::Key(key.0), value).unwrap();
    let second_key = astersql_tablecodec::EncodeRowKeyWithHandle(101, Box::new(kv::IntHandle(5)));
    let second_value = astersql_tablecodec::EncodeRow(
        Some(astersql_tablecodec::time::UTC),
        vec![
            astersql_tablecodec::types::NewStringDatum("bb".into()),
            astersql_tablecodec::types::NewStringDatum("b5".into()),
            astersql_tablecodec::types::NewStringDatum("c5".into()),
        ],
        vec![1, 2, 3],
        Vec::new(),
        None,
        None,
        astersql_tablecodec::rowcodec::Encoder::new(true),
    )
    .unwrap();
    seed.Set(kv::Key(second_key.0), second_value).unwrap();
    seed.Commit(&kv::Context::default()).unwrap();
    let session = ConcreteSession::new(Arc::clone(&domain));
    let mut point =
        astersql_planner_core_operator_physicalop::PointGetPlan::New(session.AdapterPlanContext());
    point.TblInfo = Some(model.as_ref().clone());
    point.Handle = Some(4);
    point.Columns = vec![model.Columns[0].clone()];
    let mut rebound_point =
        astersql_planner_core_operator_physicalop::PointGetPlan::New(session.AdapterPlanContext());
    rebound_point.TblInfo = Some(model.as_ref().clone());
    rebound_point.Handle = Some(5);
    rebound_point.Columns = vec![model.Columns[0].clone()];
    let mut ordinary_point =
        astersql_planner_core_operator_physicalop::PointGetPlan::New(session.AdapterPlanContext());
    ordinary_point.TblInfo = Some(model.as_ref().clone());
    ordinary_point.Handle = Some(4);
    ordinary_point.Columns = vec![model.Columns[0].clone()];
    let mut version = domain
        .storage()
        .with_storage(|storage| storage.CurrentVersion("global"))
        .unwrap();
    let ordinary_version = version;
    version.Ver = u64::MAX;
    let owner = Arc::new(crate::runtime::SessionBoundAdapterOwner::new(session));
    owner
        .BindTypedPhysicalPlan(Box::new(point), Vec::new(), version, 1, 1)
        .unwrap();
    let sql = "select a from t where rowid=4";
    let field_type = model.Columns[0].FieldType.clone();
    let mut stmt = ExecStmt {
        GoCtx: None,
        InfoSchema: 10,
        Plan: PlanInfo {
            id: 42,
            kind: PlanKind::PointGet,
            schema: vec![SchemaColumn {
                field_type: field_type.clone(),
            }],
            calculate_no_delay: false,
            projection_child: None,
            encoded: "point get".into(),
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
        Ctx: owner.clone(),
        LowerPriority: false,
        isPreparedStmt: false,
        isSelectForUpdate: false,
        retryCount: 0,
        retryStartTime: None,
        phaseBuildDurations: [std::time::Duration::ZERO; 2],
        phaseOpenDurations: [std::time::Duration::ZERO; 2],
        phaseNextDurations: [std::time::Duration::ZERO; 2],
        phaseLockDurations: [std::time::Duration::ZERO; 2],
        OutputNames: vec![FieldName {
            column_name: "a".into(),
            ..Default::default()
        }],
        PsStmt: Some("point-prepared-1".into()),
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
        .expect("open canonical point getter")
        .expect("lazy point result");
    let mut page = result.NewChunk();
    result.Next(&mut page).unwrap();
    assert_eq!(page.GetRow(0).GetBytes(0), b"aa");
    result.Next(&mut page).unwrap();
    assert_eq!(page.NumRows(), 0);
    result.Close().unwrap();
    assert_eq!(owner.LastFoundRows(), 1);
    assert_eq!(owner.Effects().process_sql, sql);
    owner
        .BindTypedPhysicalPlan(Box::new(rebound_point), Vec::new(), version, 1, 1)
        .unwrap();
    let mut rebound = stmt.clone();
    rebound.GoCtx = None;
    rebound.StmtNode.text = "select a from t where rowid=5".into();
    let mut cached = rebound
        .Exec()
        .expect("reuse closed MaxTS point actor")
        .expect("cached point result");
    let mut page = cached.NewChunk();
    cached.Next(&mut page).unwrap();
    assert_eq!(page.GetRow(0).GetBytes(0), b"bb");
    cached.Close().unwrap();
    assert!(
        owner
            .Effects()
            .events
            .iter()
            .any(|event| event == "point_get_cache_hit")
    );
    assert_eq!(owner.StatementTransactionStartTS(), u64::MAX);
    assert_eq!(owner.CanonicalTxnStartTS(), u64::MAX);
    owner
        .BindTypedPhysicalPlan(Box::new(ordinary_point), Vec::new(), ordinary_version, 1, 1)
        .unwrap();
    let previous_hits = owner
        .Effects()
        .events
        .iter()
        .filter(|event| *event == "point_get_cache_hit")
        .count();
    let mut ordinary = stmt.clone();
    ordinary.GoCtx = None;
    let mut ordinary_result = ordinary.Exec().unwrap().unwrap();
    let mut page = ordinary_result.NewChunk();
    ordinary_result.Next(&mut page).unwrap();
    assert_eq!(page.GetRow(0).GetBytes(0), b"aa");
    ordinary_result.Close().unwrap();
    assert_eq!(
        owner
            .Effects()
            .events
            .iter()
            .filter(|event| *event == "point_get_cache_hit")
            .count(),
        previous_hits,
        "finite read TS must build a fresh point executor"
    );
}

/// 短 IN 列表：首次未命中缓存，第二次命中并按新参数重建 IndexRange，无回退。
#[test]
fn prepared_index_range_cache_rebuilds_without_fallback() {
    let info_schema = cache_info_schema();
    let domain = Arc::new(Domain::new_mock(
        Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).expect("create mock storage"))
            .unwrap_or_else(|_| panic!("mock storage retained an unexpected owner")),
        Arc::new(CacheSchemaLoader {
            schema: Arc::clone(&info_schema),
        }),
    ));
    domain.init().expect("initialize cache-test domain");
    let mut session = ConcreteSession::new(Arc::clone(&domain));
    // 限制 range 体积配额，便于后续用例触发回退；本用例参数仍在配额内。
    session
        .SetSessionSystemVar(astersql_sessionctx_vardef::TiDBOptRangeMaxSize, "1330")
        .expect("set range quota");
    let snapshot = domain.storage().with_storage(|storage| {
        let version = storage.CurrentVersion("global").expect("current version");
        storage.GetSnapshot(version)
    });
    let statement_id = session
        .PreparePlannedKVSelect(
            "select * from t use index(idx_a_b) where a in (?, ?, ?, ?, ?)",
            Arc::clone(&info_schema),
        )
        .expect("prepare parameterized index select");

    let short = ["aa", "bb", "cc", "dd", "ee"]
        .map(|value| astersql_types::datum::NewStringDatum(value.to_owned()));
    let first = session
        .ExecutePreparedPlannedKVSelect(statement_id, &short, snapshot.as_ref())
        .expect("first prepared execution");
    assert!(!first.FromPlanCache);
    assert!(first.Warnings.is_empty());
    assert!(
        first
            .Plan
            .IndexRanges
            .iter()
            .any(|range| range.contains("aa"))
    );

    // 同一 Domain 的另一个会话首次执行同一语句即应命中实例缓存，并用自己的
    // Context/参数重建计划；不得共享首会话中的可变 range 状态。
    let mut other_session = ConcreteSession::new(Arc::clone(&domain));
    other_session
        .SetSessionSystemVar(astersql_sessionctx_vardef::TiDBOptRangeMaxSize, "1330")
        .expect("set other-session range quota");
    let other_statement_id = other_session
        .PreparePlannedKVSelect(
            "select * from t use index(idx_a_b) where a in (?, ?, ?, ?, ?)",
            info_schema,
        )
        .expect("prepare parameterized index select in another session");
    let other = ["oa", "ob", "oc", "od", "oe"]
        .map(|value| astersql_types::datum::NewStringDatum(value.to_owned()));
    let other_result = other_session
        .ExecutePreparedPlannedKVSelect(other_statement_id, &other, snapshot.as_ref())
        .expect("cross-session cached prepared execution");
    assert!(other_result.FromPlanCache);
    assert!(
        other_result
            .Plan
            .IndexRanges
            .iter()
            .any(|range| range.contains("oa") && range.contains("oe"))
    );

    // 第二次执行应命中计划缓存，并根据更长参数重建索引范围。
    let long = [
        "aaaaaaaaaa",
        "bbbbbbbbbb",
        "cccccccccc",
        "dddddddddd",
        "eeeeeeeeee",
    ]
    .map(|value| astersql_types::datum::NewStringDatum(value.to_owned()));
    let second = session
        .ExecutePreparedPlannedKVSelect(statement_id, &long, snapshot.as_ref())
        .expect("cached prepared execution");
    assert!(second.FromPlanCache);
    assert!(session.LastPlanFromCache());
    assert!(second.Warnings.is_empty());
    assert!(
        second
            .Plan
            .Operators
            .iter()
            .any(|name| name.contains("IndexRangeScan"))
    );
    assert!(
        second
            .Plan
            .IndexRanges
            .iter()
            .any(|range| range.contains("aaaaaaaaaa") && range.contains("eeeeeeeeee"))
    );
    assert_eq!(session.ProcessPlanSnapshot(), Some(second.Plan));
}

#[test]
fn prepared_physical_plan_binds_parameters_without_fetching_rows_and_admits_cache_on_finish() {
    let info_schema = cache_info_schema();
    let domain = Arc::new(Domain::new_mock(
        Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).expect("create storage"))
            .unwrap_or_else(|_| panic!("unexpected storage owner")),
        Arc::new(CacheSchemaLoader {
            schema: Arc::clone(&info_schema),
        }),
    ));
    domain.init().expect("initialize canonical domain");
    let session = ConcreteSession::new(domain);
    let statement_id = session
        .PreparePlannedKVSelect(
            "select * from t use index(idx_a_b) where a in (?, ?, ?, ?, ?)",
            info_schema,
        )
        .expect("prepare canonical parameterized SELECT");
    let first_params = ["aa", "bb", "cc", "dd", "ee"]
        .map(|value| astersql_types::datum::NewStringDatum(value.to_owned()));
    let mut first = session
        .PlanPreparedPlannedKVSelect(statement_id, &first_params)
        .expect("bind and optimize without a Retriever");
    assert!(!first.FromPlanCache);
    assert!(
        first
            .Snapshot
            .IndexRanges
            .iter()
            .any(|range| range.contains("aa"))
    );
    assert!(
        first.PendingCache.is_some(),
        "cache admission waits for execution"
    );
    session.FinishPreparedKVPhysicalPlan(&mut first, 0);
    assert!(first.PendingCache.is_none());
    let next_params = ["oa", "ob", "oc", "od", "oe"]
        .map(|value| astersql_types::datum::NewStringDatum(value.to_owned()));
    let mut second = session
        .PlanPreparedPlannedKVSelect(statement_id, &next_params)
        .expect("restore cached plan with new parameter values");
    assert!(second.FromPlanCache);
    assert!(
        second
            .Snapshot
            .IndexRanges
            .iter()
            .any(|range| range.contains("oa"))
    );
    session.FinishPreparedKVPhysicalPlan(&mut second, 0);
}

/// 复合 IN（a 与 b 各 5 个）超出 range 配额：产生告警且两次均不写入/命中计划缓存。
#[test]
fn prepared_compound_in_range_fallback_skips_cache_admission() {
    let info_schema = cache_info_schema();
    let domain = Arc::new(Domain::new_mock(
        Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).expect("create mock storage"))
            .unwrap_or_else(|_| panic!("mock storage retained an unexpected owner")),
        Arc::new(CacheSchemaLoader {
            schema: Arc::clone(&info_schema),
        }),
    ));
    domain.init().expect("initialize fallback-test domain");
    let mut session = ConcreteSession::new(Arc::clone(&domain));
    session
        .SetSessionSystemVar(astersql_sessionctx_vardef::TiDBOptRangeMaxSize, "1330")
        .expect("set range quota");
    let snapshot = domain.storage().with_storage(|storage| {
        let version = storage.CurrentVersion("global").expect("current version");
        storage.GetSnapshot(version)
    });
    let statement_id = session
        .PreparePlannedKVSelect(
            "select * from t use index(idx_a_b) where a in (?, ?, ?, ?, ?) and b in (?, ?, ?, ?, ?)",
            info_schema,
        )
        .expect("prepare compound in-list select");
    let parameters = ["a1", "a2", "a3", "a4", "a5", "b1", "b2", "b3", "b4", "b5"]
        .map(|value| astersql_types::datum::NewStringDatum(value.to_owned()));

    let first = session
        .ExecutePreparedPlannedKVSelect(statement_id, &parameters, snapshot.as_ref())
        .expect("first fallback execution");
    assert!(!first.FromPlanCache);
    assert!(
        first
            .Warnings
            .iter()
            .any(|warning| warning.contains("tidb_opt_range_max_size")),
        "missing range fallback warning: {:?}",
        first.Warnings
    );
    assert!(
        first
            .Warnings
            .iter()
            .any(|warning| { warning.contains("skip prepared plan-cache: in-list is too long") }),
        "missing plan-cache skip warning: {:?}",
        first.Warnings
    );
    // StmtCtx 中也应保留相同的“跳过计划缓存”告警。
    session.WithSessionVars(|variables| {
        assert!(variables.StmtCtx.GetWarnings().iter().any(|warning| {
            warning.Err.as_ref().is_some_and(|error| {
                error
                    .to_string()
                    .contains("skip prepared plan-cache: in-list is too long")
            })
        }));
    });

    // 再次执行仍不命中缓存，说明未准入缓存。
    let second = session
        .ExecutePreparedPlannedKVSelect(statement_id, &parameters, snapshot.as_ref())
        .expect("second fallback execution");
    assert!(!second.FromPlanCache);
    assert!(!session.LastPlanFromCache());
    assert!(
        second
            .Warnings
            .iter()
            .any(|warning| { warning.contains("skip prepared plan-cache: in-list is too long") }),
        "missing repeated plan-cache skip warning: {:?}",
        second.Warnings
    );
}
