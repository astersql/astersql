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

use crate::adapter::SchemaColumn;
use crate::adapter::{AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext};
use crate::builder::*;
use crate::mview_complete_delta_apply::{MutationBatch, MutationRuntime};
use crate::mview_delta_merge_builder::*;
use astersql_parser_mysql::r#type::{TypeLonglong, TypeVarchar};
use astersql_parser_types::{FieldType, NewFieldType};
use astersql_planner_core::{MViewAggInfo, MViewAggKind as K, MViewDeltaMerge, PlanKind, PlanNode};
use astersql_types::datum::{Datum, NewIntDatum};
use astersql_util_chunk as chunk;
use astersql_util_memory::tracker::Tracker;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn info(kind: K, offset: usize, deps: &[usize]) -> MViewAggInfo {
    MViewAggInfo {
        kind,
        mv_offset: offset,
        arg_col_name: "v".into(),
        arg_not_null: false,
        dependencies: deps.to_vec(),
    }
}
fn fields(n: usize) -> Vec<FieldType> {
    vec![NewFieldType(TypeLonglong); n]
}
fn row(v: &[i64]) -> Vec<Datum> {
    v.iter().map(|v| NewIntDatum(*v)).collect()
}
fn plan() -> MViewDeltaMerge {
    MViewDeltaMerge {
        source: PlanNode::New(1, PlanKind::TableReader, vec![]),
        full_update_inner_source: None,
        full_update_inner_column_count: 0,
        full_update_index_ranges: vec![],
        full_update_key_off_to_index_off: vec![],
        full_update_key_result_col_indexes: vec![],
        full_update_output_mv_offsets: vec![],
        full_update_snapshot: None,
        mv_table_id: 10,
        base_table_id: 11,
        mlog_table_id: 12,
        mv_column_count: 3,
        delta_column_count: 2,
        mv_table_pk_cols: vec![2],
        group_key_mv_offsets: vec![0],
        count_star_mv_offset: 1,
        agg_infos: vec![info(K::Sum, 2, &[1, 3]), info(K::CountStar, 1, &[0])],
    }
}
fn minmax_plan() -> MViewDeltaMerge {
    let mut p = plan();
    p.delta_column_count = 5;
    p.mv_table_pk_cols = vec![5];
    p.agg_infos = vec![info(K::Min, 2, &[1, 2, 3, 4]), info(K::CountStar, 1, &[0])];
    p.full_update_inner_source = Some(PlanNode::New(2, PlanKind::IndexReader, vec![]));
    p.full_update_inner_column_count = 2;
    p.full_update_index_ranges = vec![vec!["key".into()]];
    p.full_update_key_off_to_index_off = vec![0];
    p.full_update_key_result_col_indexes = vec![1];
    p.full_update_output_mv_offsets = vec![2, 0];
    p
}

#[test]
fn mview_delta_merge_mapping_topology_and_errors() {
    let p = plan();
    let f = fields(5);
    let ordered = mappings(&p.agg_infos, &f, 2, 3).unwrap();
    assert_eq!(ordered[0].kind, K::CountStar);
    assert_eq!(ordered[1].dependencies, vec![1, 3]);
    let cases = vec![
        (vec![info(K::Sum, 2, &[1])], "COUNT(*)"),
        (
            vec![info(K::CountStar, 1, &[0]), info(K::Count, 1, &[1])],
            "duplicate",
        ),
        (
            vec![info(K::CountStar, 1, &[0]), info(K::Sum, 2, &[1, 2])],
            "does not map",
        ),
        (
            vec![
                info(K::CountStar, 0, &[0]),
                info(K::Sum, 1, &[1, 4]),
                info(K::Sum, 2, &[1, 3]),
            ],
            "cyclic",
        ),
        (
            vec![info(K::CountStar, 1, &[4]), info(K::Sum, 2, &[1])],
            "non-zero indegree",
        ),
        (
            vec![info(K::CountStar, 1, &[0]), info(K::Sum, 2, &[])],
            "count mismatch",
        ),
        (
            vec![info(K::CountStar, 1, &[0]), info(K::Sum, 2, &[9])],
            "out of range",
        ),
        (
            vec![info(K::CountStar, 1, &[0]), info(K::Sum, 2, &[3])],
            "computed side",
        ),
    ];
    for (infos, expected) in cases {
        let err = mappings(&infos, &f, 2, 3).unwrap_err().to_string();
        assert!(err.contains(expected), "{err} lacks {expected}");
    }
    let mut f = f;
    f[1] = NewFieldType(TypeVarchar);
    assert!(
        mappings(&p.agg_infos, &f, 2, 3)
            .unwrap_err()
            .to_string()
            .contains("type mismatch")
    );
    let mut f = fields(5);
    f[3].AddFlag(astersql_parser_mysql::r#type::UnsignedFlag);
    assert!(
        mappings(&p.agg_infos, &f, 2, 3)
            .unwrap_err()
            .to_string()
            .contains("signed")
    );
}

#[test]
fn mview_delta_merge_minmax_layout_validation() {
    let p = minmax_plan();
    let maps = mappings(&p.agg_infos, &fields(8), 5, 3).unwrap();
    let layout = recompute_layout(&p, &maps).unwrap().unwrap();
    assert_eq!(layout.key_inputs, vec![5]);
    assert_eq!(layout.results[&7], 0);
    let mut invalid = Vec::new();
    let mut v = p.clone();
    v.full_update_inner_source = None;
    invalid.push(v);
    let mut v = p.clone();
    v.full_update_inner_column_count = 0;
    invalid.push(v);
    let mut v = p.clone();
    v.full_update_index_ranges.clear();
    invalid.push(v);
    let mut v = p.clone();
    v.full_update_index_ranges[0].clear();
    invalid.push(v);
    let mut v = p.clone();
    v.group_key_mv_offsets.clear();
    invalid.push(v);
    let mut v = p.clone();
    v.group_key_mv_offsets[0] = 3;
    invalid.push(v);
    let mut v = p.clone();
    v.full_update_key_off_to_index_off[0] = 1;
    invalid.push(v);
    let mut v = p.clone();
    v.full_update_key_result_col_indexes[0] = 2;
    invalid.push(v);
    let mut v = p.clone();
    v.full_update_output_mv_offsets = vec![0, 0];
    invalid.push(v);
    let mut v = p.clone();
    v.full_update_output_mv_offsets = vec![2, 2];
    invalid.push(v);
    for v in invalid {
        assert!(recompute_layout(&v, &maps).is_err(), "{v:?}");
    }
}

struct Source {
    rows: Vec<Vec<Datum>>,
    schema: Vec<SchemaColumn>,
    returned: bool,
    require_context: bool,
}
impl Source {
    fn new(rows: Vec<Vec<Datum>>, fields: Vec<FieldType>) -> Self {
        Self {
            rows,
            schema: fields
                .into_iter()
                .map(|field_type| SchemaColumn { field_type })
                .collect(),
            returned: false,
            require_context: false,
        }
    }
}
impl ExecExecutor for Source {
    fn Open(&mut self) -> AdapterResult {
        self.returned = false;
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        Ok(())
    }
    fn NextWithContext(
        &mut self,
        _ctx: &ExecutionContext,
        out: &mut chunk::Chunk,
    ) -> AdapterResult {
        let required = self.require_context;
        self.require_context = false;
        let result = self.Next(out);
        self.require_context = required;
        result
    }
    fn Next(&mut self, out: &mut chunk::Chunk) -> AdapterResult {
        if self.require_context {
            return Err(astersql_errors::New("execution context was dropped"));
        }
        out.Reset();
        if !self.returned {
            for row in &self.rows {
                for (i, d) in row.iter().enumerate() {
                    out.AppendDatum(i, d);
                }
            }
            self.returned = true;
        }
        Ok(())
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        ChunkConfig {
            fields: self.schema.iter().map(|c| c.field_type.clone()).collect(),
            initial_capacity: 4,
            maximum_chunk_size: 1024,
        }
    }
    fn NewChunk(&self) -> chunk::Chunk {
        let c = self.ChunkConfig();
        *chunk::New(c.fields, c.initial_capacity, c.maximum_chunk_size)
    }
    fn Schema(&self) -> &[SchemaColumn] {
        &self.schema
    }
    fn CalculateNoDelay(&self) -> bool {
        false
    }
    fn IsWriteExecutor(&self) -> bool {
        false
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        Ok(())
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        vec![]
    }
    fn HasForeignKeyCascades(&self) -> bool {
        false
    }
    fn PrepareFKCascadeContext(&mut self) {}
    fn AddFKCheckLockDuration(&mut self, _: Duration) {}
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}
struct IS;
impl InfoSchema for IS {}
struct SS;
impl Snapshot for SS {
    fn set_option(&mut self, _: SnapshotOption) -> Result<(), BuildError> {
        Ok(())
    }
}
fn reader() -> ReaderSnapshot {
    ReaderSnapshot {
        ts: 200,
        snapshot: Mutex::new(Box::new(SS)),
        info_schema: Arc::new(IS),
    }
}
#[derive(Default, Clone)]
struct Writes {
    rows: Vec<Vec<Datum>>,
    touched: Vec<Vec<bool>>,
    sources: Vec<Vec<Datum>>,
}
impl MutationBatch for Writes {
    fn delete_from_source(&mut self, source: &[Datum], old: &[Datum]) -> Result<(), String> {
        self.sources.push(source.to_vec());
        self.delete(old)
    }
    fn update_from_source(
        &mut self,
        source: &[Datum],
        old: &[Datum],
        new: &[Datum],
        touched: &[bool],
    ) -> Result<(), String> {
        self.sources.push(source.to_vec());
        self.update(old, new, touched)
    }
    fn insert(&mut self, row: &[Datum]) -> Result<(), String> {
        self.rows.push(row.to_vec());
        Ok(())
    }
    fn delete(&mut self, row: &[Datum]) -> Result<(), String> {
        self.rows.retain(|r| !same_row(r, row));
        Ok(())
    }
    fn update(&mut self, old: &[Datum], new: &[Datum], touched: &[bool]) -> Result<(), String> {
        let i = self
            .rows
            .iter()
            .position(|r| same_row(r, old))
            .ok_or("missing old row")?;
        self.rows[i] = new.to_vec();
        self.touched.push(touched.to_vec());
        Ok(())
    }
}
fn same_row(a: &[Datum], b: &[Datum]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.Equals(b))
}
struct Resource {
    statement: Arc<astersql_sessionctx_stmtctx::StatementContext>,
    writes: Mutex<Writes>,
    strategy: RecomputeStrategy,
    reads: Mutex<Vec<(u64, usize)>>,
    fail: bool,
    target_fields: Vec<FieldType>,
    require_context: bool,
}
impl Resource {
    fn new(rows: Vec<Vec<Datum>>, strategy: RecomputeStrategy) -> Self {
        Self {
            statement: Arc::new({
                let mut statement = *astersql_sessionctx_stmtctx::NewStmtCtx();
                statement.RuntimeStatsColl = Some(Arc::new(
                    astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None),
                ));
                statement
            }),
            writes: Mutex::new(Writes {
                rows,
                ..Default::default()
            }),
            strategy,
            reads: Mutex::new(vec![]),
            fail: false,
            target_fields: fields(3),
            require_context: false,
        }
    }
}
impl MutationRuntime for Resource {
    fn run_atomic(
        &self,
        apply: &mut dyn FnMut(&mut dyn MutationBatch) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut writes = self.writes.lock().unwrap();
        let mut staged = writes.clone();
        apply(&mut staged)?;
        if self.fail {
            return Err("write failure".into());
        }
        *writes = staged;
        Ok(())
    }
}
impl Resources for Resource {
    fn refresh_context(&self) -> Option<(i32, Arc<astersql_sessionctx_stmtctx::StatementContext>)> {
        Some((37, self.statement.clone()))
    }
    fn complete_delta_target_columns(
        &self,
    ) -> Result<crate::mview_complete_delta_apply_builder::TargetColumns, BuildError> {
        Ok(complete_delta_apply::target_columns(&self.target_fields))
    }
    fn target_field_types(&self) -> Vec<FieldType> {
        self.target_fields.clone()
    }
    fn read_full_update(
        &self,
        _: &PlanNode,
        snapshot: &ReaderSnapshot,
        req: &LookupRequest,
    ) -> Result<ExecutorBox, BuildError> {
        assert_eq!(req.key_to_index, vec![0]);
        assert_eq!(req.ranges, vec![vec!["key".to_owned()]]);
        self.reads
            .lock()
            .unwrap()
            .push((snapshot.ts, req.keys.len()));
        let rows = req
            .keys
            .iter()
            .map(|k| vec![NewIntDatum(9), k[0].clone()])
            .collect();
        let mut source = Source::new(rows, fields(2));
        source.require_context = self.require_context;
        Ok(Box::new(source))
    }
    fn recompute_strategy(&self) -> RecomputeStrategy {
        self.strategy
    }
}

#[test]
fn mview_delta_merge_executor_insert_update_delete_and_reopen() {
    let p = plan();
    let rt = Arc::new(Resource::new(
        vec![row(&[1, 2, 10]), row(&[2, 1, 4])],
        RecomputeStrategy::Batch,
    ));
    let child = Source::new(
        vec![
            row(&[1, 5, 1, 2, 10]),
            row(&[-1, -4, 2, 1, 4]),
            vec![
                NewIntDatum(1),
                NewIntDatum(8),
                NewIntDatum(3),
                Datum::default(),
                Datum::default(),
            ],
        ],
        fields(5),
    );
    let mut exec = build(&p, Box::new(child), rt.clone(), reader()).unwrap();
    assert!(exec.IsWriteExecutor());
    exec.Open().unwrap();
    let mut out = exec.NewChunk();
    exec.Next(&mut out).unwrap();
    assert_eq!(out.NumRows(), 0);
    {
        let w = rt.writes.lock().unwrap();
        assert_eq!(w.rows.len(), 2);
        assert!(same_row(&w.rows[0], &row(&[1, 3, 15])));
        assert!(same_row(&w.rows[1], &row(&[3, 1, 8])));
        assert_eq!(w.touched, vec![vec![false, true, true]]);
    }
    exec.Next(&mut out).unwrap();
    exec.Close().unwrap();
    exec.Open().unwrap();
    assert!(
        exec.Next(&mut out).is_err(),
        "reopen executes again against already-mutated target"
    );
    exec.Close().unwrap();
}

#[test]
fn mview_delta_merge_minmax_executes_single_and_batch_at_snapshot() {
    for strategy in [RecomputeStrategy::SingleRow, RecomputeStrategy::Batch] {
        let p = minmax_plan();
        let rt = Arc::new(Resource::new(
            vec![row(&[1, 2, 3]), row(&[2, 2, 3])],
            strategy,
        ));
        let child = Source::new(
            vec![
                minmax_row(&[-1, 0, 0, 3, 1, 1, 2, 3]),
                minmax_row(&[-1, 0, 0, 3, 1, 2, 2, 3]),
            ],
            fields(8),
        );
        let mut exec = build(&p, Box::new(child), rt.clone(), reader()).unwrap();
        exec.Open().unwrap();
        let mut out = exec.NewChunk();
        exec.Next(&mut out).unwrap();
        exec.Close().unwrap();
        let w = rt.writes.lock().unwrap();
        assert_eq!(w.rows[0][2].GetInt64(), 9);
        assert_eq!(w.rows[1][2].GetInt64(), 9);
        let reads = rt.reads.lock().unwrap();
        assert!(reads.iter().all(|r| r.0 == 200));
        assert_eq!(
            reads.len(),
            if strategy == RecomputeStrategy::Batch {
                1
            } else {
                2
            }
        );
    }
}

#[test]
fn mview_delta_merge_write_failure_does_not_publish_mutations() {
    let mut rt = Resource::new(vec![row(&[1, 2, 10])], RecomputeStrategy::Batch);
    rt.fail = true;
    let rt = Arc::new(rt);
    let mut exec = build(
        &plan(),
        Box::new(Source::new(vec![row(&[1, 5, 1, 2, 10])], fields(5))),
        rt.clone(),
        reader(),
    )
    .unwrap();
    exec.Open().unwrap();
    let mut out = exec.NewChunk();
    assert!(exec.Next(&mut out).is_err());
    assert!(same_row(
        &rt.writes.lock().unwrap().rows[0],
        &row(&[1, 2, 10])
    ));
    exec.Close().unwrap();
}

struct SC;
impl StatementContext for SC {}
impl SessionContext for SC {}
struct Leaf;
impl PlanData for Leaf {}
impl TableDualPlanData for Leaf {
    fn row_count(&self) -> usize {
        1
    }
}
struct MergePlan {
    data: MViewDeltaMerge,
    leaf: Leaf,
}
impl PlanData for MergePlan {}
impl UnaryPlanData for MergePlan {
    fn child_plan(&self) -> Plan<'_> {
        Plan::TableDual(&self.leaf)
    }
}
impl MViewDeltaMergePlanData for MergePlan {
    fn target_table_id(&self) -> i64 {
        self.data.mv_table_id
    }
    fn merge_plan(&self) -> &MViewDeltaMerge {
        &self.data
    }
}
struct Deps {
    rt: Arc<Resource>,
    calls: Mutex<Vec<String>>,
    fail_child: bool,
    complete: bool,
    fail_update_ts: bool,
    missing_table: bool,
}
macro_rules! unused_dependency_methods {
    ($(fn $name:ident($($args:tt)*) $(-> $ret:ty)?;)*) => {
        $(fn $name($($args)*) $(-> $ret)? { panic!("unexpected dependency: {}",stringify!($name)) })*
    };
}
impl ExecutorBuilderDependencies for Deps {
    fn is_statement_staleness(&self, _: &dyn SessionContext) -> bool {
        false
    }
    fn transaction_scope(&self, _: &dyn SessionContext) -> Result<String, BuildError> {
        Ok("global".into())
    }
    fn read_replica_scope(&self, _: &dyn SessionContext) -> Result<String, BuildError> {
        Ok("global".into())
    }
    fn update_for_update_ts(&self) -> Result<(), BuildError> {
        self.calls.lock().unwrap().push("update".into());
        if self.fail_update_ts {
            return Err(BuildError::new("for-update TS unavailable"));
        }
        Ok(())
    }
    fn statement_for_update_ts(&self) -> Result<u64, BuildError> {
        Ok(200)
    }
    fn statement_read_ts(&self) -> Result<u64, BuildError> {
        Ok(100)
    }
    fn snapshot_at_ts(&self, ts: u64) -> Result<Box<dyn Snapshot>, BuildError> {
        self.calls.lock().unwrap().push(format!("snapshot:{ts}"));
        Ok(Box::new(SS))
    }
    fn snapshot_with_for_update_ts(&self) -> Result<Box<dyn Snapshot>, BuildError> {
        self.calls.lock().unwrap().push("for-update".into());
        Ok(Box::new(SS))
    }
    fn snapshot_with_read_ts(&self) -> Result<Box<dyn Snapshot>, BuildError> {
        self.calls.lock().unwrap().push("read".into());
        Ok(Box::new(SS))
    }
    fn snapshot_session_options(&self) -> SnapshotSessionOptions {
        SnapshotSessionOptions {
            transaction_read_replica_scope: "global".into(),
            replica_read_is_closest: false,
            global_transaction_scope: "global".into(),
            dc_label_key: "zone".into(),
            task_id: 1,
            read_timeout_millis: 1000,
            resource_group_name: "default".into(),
            explicit_request_source_type: "mview".into(),
        }
    }
    fn resolve_table(&self, id: i64, _: bool) -> Result<ResolvedTable, BuildError> {
        if self.missing_table {
            return Err(BuildError::new(format!(
                "target table id {id} not found in infoschema"
            )));
        }
        Ok(ResolvedTable {
            id,
            name: "mv".into(),
            is_base_table: true,
        })
    }
    fn mview_resources(
        &self,
        table: &ResolvedTable,
        handles: &[usize],
    ) -> Result<Arc<dyn Resources>, BuildError> {
        assert_eq!(table.id, 10);
        assert_eq!(handles, &[2]);
        Ok(self.rt.clone())
    }
    fn build_executor(
        &self,
        _: ExecutorKind,
        _: &dyn PlanData,
        _: Vec<ExecutorBox>,
    ) -> Result<ExecutorBox, BuildError> {
        if self.fail_child {
            return Err(BuildError::new("child failed"));
        }
        if self.complete {
            return Ok(Box::new(Source::new(
                vec![row(&[1, 0, 0, 0, 2, 3, 4])],
                fields(7),
            )));
        }
        Ok(Box::new(Source::new(
            vec![row(&[1, 5, 1, 2, 10])],
            fields(5),
        )))
    }
    fn build_parallel_unary_executor(
        &self,
        kind: ExecutorKind,
        _: &dyn ParallelUnaryPlanData,
        child: ExecutorBox,
        workers: i64,
    ) -> Result<ExecutorBox, BuildError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{kind:?}:{workers}"));
        Ok(child)
    }
    unused_dependency_methods! {
    fn build_deferred_executor(
            &self,
            kind: DeferredPlanKind,
            plan: &dyn PlanData,
        ) -> Result<ExecutorBox, BuildError>;
    fn ddl_owner_id(&self, timeout: Duration) -> Result<String, BuildError>;
    fn ddl_self_id(&self) -> String;
    fn acquire_system_session(&self) -> Result<Box<dyn SystemSession>, BuildError>;
    fn ddl_info_with_new_transaction(
            &self,
            session: &mut dyn SystemSession,
        ) -> Result<Box<dyn DdlInfo>, BuildError>;
    fn release_system_session(&self, session: Box<dyn SystemSession>);
    fn build_show_ddl_executor(
            &self,
            plan: &dyn PlanData,
            owner_id: String,
            ddl_info: Box<dyn DdlInfo>,
            self_id: String,
        ) -> Result<ExecutorBox, BuildError>;
    fn filter_temporary_lock_tables(&self, plan: &dyn SelectLockPlanData)
        -> Result<(), BuildError>;
    fn start_transaction_for_show(&self, plan: &dyn ShowPlanData) -> Result<(), BuildError>;
    fn verify_execute_staleness(&self, plan: &dyn ExecutePlanData) -> Result<(), BuildError>;
    fn initialize_insert_columns(&self, plan: &dyn InsertPlanData) -> Result<(), BuildError>;
    fn build_foreign_key_checks(&self, plan: &dyn InsertPlanData) -> Result<(), BuildError>;
    fn build_foreign_key_cascades(&self, plan: &dyn InsertPlanData) -> Result<(), BuildError>;
    fn build_import_into_executor(
            &self,
            plan: &dyn ImportIntoPlanData,
            table: &ResolvedTable,
            child: Option<ExecutorBox>,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_load_data_executor(
            &self,
            plan: &dyn LoadDataPlanData,
            table: &ResolvedTable,
        ) -> Result<ExecutorBox, BuildError>;
    fn parse_replayer_statement(&self, sql: &str) -> Result<Box<dyn ParsedStatement>, BuildError>;
    fn build_plan_replayer_executor(
            &self,
            plan: &dyn PlanReplayerPlanData,
            parsed_statements: Vec<Box<dyn ParsedStatement>>,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_traffic_executor(
            &self,
            plan: &dyn TrafficPlanData,
            arguments: BTreeMap<String, String>,
        ) -> Result<ExecutorBox, BuildError>;
    fn initialize_runtime_stats(&self) -> Result<(), BuildError>;
    fn build_union_scan_from_reader(
            &self,
            plan: &dyn UnionScanPlanData,
            reader: ExecutorBox,
            in_write_statement: bool,
        ) -> Result<ExecutorBox, BuildError>;
    fn handle_cached_table(
            &self,
            in_write_statement: bool,
            in_explain_statement: bool,
        ) -> Result<(), BuildError>;
    fn build_merge_join_executor(
            &self,
            plan: &dyn MergeJoinPlanData,
            left: ExecutorBox,
            right: ExecutorBox,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_hash_join_executor(
            &self,
            kind: ExecutorKind,
            plan: &dyn HashJoinPlanData,
            left: ExecutorBox,
            right: ExecutorBox,
            config: HashJoinBuildConfig,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_hash_agg_executor(
            &self,
            plan: &dyn HashAggPlanData,
            child: ExecutorBox,
            config: HashAggBuildConfig,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_stream_agg_executor(
            &self,
            plan: &dyn StreamAggPlanData,
            child: ExecutorBox,
            config: StreamAggBuildConfig,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_top_n_executor(
            &self,
            plan: &dyn TopNPlanData,
            child: ExecutorBox,
            projection: Option<Vec<usize>>,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_apply_executor(
            &self,
            kind: ExecutorKind,
            plan: &dyn ApplyPlanData,
            left: ExecutorBox,
            right: ExecutorBox,
        ) -> Result<ExecutorBox, BuildError>;
    fn parallel_apply_is_buildable(&self, plan: &dyn ApplyPlanData) -> bool;
    fn build_mem_table_executor(&self, plan: &dyn PlanData) -> Result<ExecutorBox, BuildError>;
    fn build_update_executor(
            &self,
            plan: &dyn UpdatePlanData,
            child: ExecutorBox,
            assignment_flags: Vec<isize>,
        ) -> Result<ExecutorBox, BuildError>;
    fn validate_update_list(
            &self,
            plan: &dyn UpdatePlanData,
            assignment_flags: &[isize],
        ) -> Result<(), BuildError>;
    fn build_update_foreign_key_checks(&self, plan: &dyn UpdatePlanData) -> Result<(), BuildError>;
    fn build_update_foreign_key_cascades(
            &self,
            plan: &dyn UpdatePlanData,
        ) -> Result<(), BuildError>;
    fn build_delete_executor(
            &self,
            plan: &dyn DeletePlanData,
            child: ExecutorBox,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_delete_foreign_key_checks(&self, plan: &dyn DeletePlanData) -> Result<(), BuildError>;
    fn build_delete_foreign_key_cascades(
            &self,
            plan: &dyn DeletePlanData,
        ) -> Result<(), BuildError>;
    fn flush_stats_delta_for_analyze(&self, plan: &dyn AnalyzePlanData) -> Result<(), BuildError>;
    fn analyze_table_counts(
            &self,
            task: &dyn AnalyzeColumnsTaskData,
        ) -> Result<AnalyzeTableCounts, BuildError>;
    fn build_analyze_index_task(
            &self,
            task: &dyn AnalyzeIndexTaskData,
            snapshot_ts: u64,
            auto_analyze: bool,
        ) -> Result<Box<dyn AnalyzeTask>, BuildError>;
    fn build_analyze_columns_task(
            &self,
            task: &dyn AnalyzeColumnsTaskData,
            config: AnalyzeSamplingConfig,
        ) -> Result<Box<dyn AnalyzeTask>, BuildError>;
    fn append_analyze_note(&self, note: &str);
    fn build_analyze_executor(
            &self,
            plan: &dyn AnalyzePlanData,
            tasks: Vec<Box<dyn AnalyzeTask>>,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_index_join_executor(
            &self,
            kind: ExecutorKind,
            plan: &dyn IndexJoinPlanData,
            outer: ExecutorBox,
            reader_builder: DataReaderBuilder,
        ) -> Result<ExecutorBox, BuildError>;
    fn wrap_index_nested_loop_hash_join(
            &self,
            plan: &dyn IndexJoinPlanData,
            lookup_join: ExecutorBox,
        ) -> Result<ExecutorBox, BuildError>;
    fn validate_table_reader_access(
            &self,
            plan: &dyn TableReaderPlanData,
        ) -> Result<(), BuildError>;
    fn build_no_range_table_reader(
            &self,
            plan: &dyn TableReaderPlanData,
            snapshot_ts: u64,
        ) -> Result<Box<dyn TableReaderDraft>, BuildError>;
    fn build_mpp_gather_executor(
            &self,
            plan: &dyn TableReaderPlanData,
            snapshot_ts: u64,
        ) -> Result<ExecutorBox, BuildError>;
    fn align_empty_table_reader_schema(
            &self,
            plan: &dyn TableReaderPlanData,
        ) -> Result<(), BuildError>;
    fn mark_table_reader_store(&self, plan: &dyn TableReaderPlanData);
    fn warn_ignored_tiflash_replica_read(&self, plan: &dyn TableReaderPlanData);
    fn finalize_table_reader(
            &self,
            plan: &dyn TableReaderPlanData,
            reader: Box<dyn TableReaderDraft>,
            sorted_partition_ids: Option<Vec<i64>>,
        ) -> Result<ExecutorBox, BuildError>;
    fn prune_table_reader_partitions(
            &self,
            plan: &dyn TableReaderPlanData,
        ) -> Result<Vec<i64>, BuildError>;
    fn build_partition_index_ranges(
            &self,
            partition_id: i64,
            contents: &[IndexJoinLookUpContent],
        ) -> Result<Vec<KeyRange>, BuildError>;
    fn prune_inner_partitions(
            &self,
            contents: &[IndexJoinLookUpContent],
        ) -> Result<Vec<i64>, BuildError>;
    fn build_no_range_index_reader(
            &self,
            plan: &dyn IndexReaderPlanData,
            snapshot_ts: u64,
        ) -> Result<Box<dyn IndexReaderDraft>, BuildError>;
    fn finalize_index_reader(
            &self,
            plan: &dyn IndexReaderPlanData,
            reader: Box<dyn IndexReaderDraft>,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_table_request(
            &self,
            schema_len: usize,
            plan: &dyn DistributedPlanData,
        ) -> Result<Box<dyn DagRequest>, BuildError>;
    fn build_index_request(
            &self,
            column_count: usize,
            handle_len: usize,
            unnatural_order: bool,
        ) -> Result<Box<dyn DagRequest>, BuildError>;
    fn build_no_range_index_lookup_reader(
            &self,
            plan: &dyn IndexLookupReaderPlanData,
            snapshot_ts: u64,
        ) -> Result<Box<dyn IndexLookupReaderDraft>, BuildError>;
    fn finalize_index_lookup_reader(
            &self,
            plan: &dyn IndexLookupReaderPlanData,
            reader: Box<dyn IndexLookupReaderDraft>,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_no_range_index_merge_reader(
            &self,
            plan: &dyn IndexMergeReaderPlanData,
            snapshot_ts: u64,
        ) -> Result<Box<dyn IndexMergeReaderDraft>, BuildError>;
    fn finalize_index_merge_reader(
            &self,
            plan: &dyn IndexMergeReaderPlanData,
            reader: Box<dyn IndexMergeReaderDraft>,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_index_usage_reporter(
            &self,
            plan: &dyn DistributedPlanData,
            load_stats: bool,
        ) -> Result<Box<dyn IndexUsageReporter>, BuildError>;
    fn build_data_reader_executor(
            &self,
            kind: DataReaderBuildKind,
            plan: &dyn DataReaderPlanData,
            contents: &[IndexJoinLookUpContent],
            snapshot_ts: u64,
            range_mem_tracker: Option<Arc<Tracker>>,
        ) -> Result<ExecutorBox, BuildError>;
    fn open_index_join_hash_executor(&self, executor: &mut dyn Executor) -> Result<(), BuildError>;
    fn encode_key_ranges(
            &self,
            partition_id: i64,
            ranges: &[LogicalRange],
        ) -> Result<Vec<KeyRange>, BuildError>;
    fn build_table_reader_base_for_index_join(
            &self,
            snapshot_ts: u64,
            ranges: &[KeyRange],
        ) -> Result<ExecutorBox, BuildError>;
    fn build_shuffle_executor(
            &self,
            plan: &dyn ShufflePlanData,
            data_sources: Vec<ExecutorBox>,
            workers: Vec<ExecutorBox>,
        ) -> Result<ExecutorBox, BuildError>;
    fn prune_batch_point_get(
            &self,
            plan: &dyn BatchPointGetPlanData,
        ) -> Result<BatchPointGetPruneResult, BuildError>;
    fn build_batch_point_get_executor(
            &self,
            plan: &dyn BatchPointGetPlanData,
            snapshot: Box<dyn Snapshot>,
            prune_result: BatchPointGetPruneResult,
            lock: bool,
            capacity: usize,
            cache: Option<Box<dyn MemoryBuffer>>,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_table_sample_executor(
            &self,
            plan: &dyn TableSamplePlanData,
            snapshot_ts: u64,
            empty_sampler: bool,
        ) -> Result<ExecutorBox, BuildError>;
    fn load_cte_storages(&self, storage_id: i64) -> Result<Option<Arc<CTEStorages>>, BuildError>;
    fn create_and_store_cte_storages(
            &self,
            storage_id: i64,
        ) -> Result<Arc<CTEStorages>, BuildError>;
    fn build_cte_storage_producer(
            &self,
            plan: &dyn CtePlanData,
            storages: &Arc<CTEStorages>,
            seed: ExecutorBox,
            recursive: Option<ExecutorBox>,
        ) -> Result<(), BuildError>;
    fn build_cte_executor(
            &self,
            plan: &dyn CtePlanData,
            storages: Arc<CTEStorages>,
        ) -> Result<ExecutorBox, BuildError>;
    fn build_cte_table_reader_executor(
            &self,
            plan: &dyn CteTablePlanData,
            storages: Arc<CTEStorages>,
        ) -> Result<ExecutorBox, BuildError>;
    fn get_cache_table(
            &self,
            table: &dyn TableReadSpec,
            snapshot_ts: u64,
            may_update_read_lock: bool,
        ) -> Result<Option<Box<dyn MemoryBuffer>>, BuildError>;
    fn validate_compact_storage(&self) -> Result<(), BuildError>;
    fn resolve_compact_partition_ids(
            &self,
            plan: &dyn CompactTablePlanData,
        ) -> Result<Vec<i64>, BuildError>;
    fn build_compact_table_executor(
            &self,
            plan: &dyn CompactTablePlanData,
            partition_ids: Vec<i64>,
        ) -> Result<ExecutorBox, BuildError>;
        }
}
fn dependencies() -> Arc<Deps> {
    Arc::new(Deps {
        rt: Arc::new(Resource::new(
            vec![row(&[1, 2, 10])],
            RecomputeStrategy::Batch,
        )),
        calls: Mutex::new(vec![]),
        fail_child: false,
        complete: false,
        fail_update_ts: false,
        missing_table: false,
    })
}
fn builder(deps: Arc<Deps>) -> executorBuilder {
    newExecutorBuilder(Arc::new(SC), Arc::new(SC), Arc::new(IS), deps).unwrap()
}

#[test]
fn mview_delta_merge_dispatch_builds_and_executes_production_sink() {
    let deps = dependencies();
    let mut b = builder(deps.clone());
    let p = MergePlan {
        data: plan(),
        leaf: Leaf,
    };
    let mut exec = b.build(Some(Plan::MViewDeltaMerge(&p))).unwrap();
    assert!(b.error().is_none());
    assert_eq!(b.getSnapshotTS().unwrap(), 100);
    assert_eq!(*deps.calls.lock().unwrap(), vec!["update", "snapshot:200"]);
    exec.Open().unwrap();
    let mut out = exec.NewChunk();
    exec.Next(&mut out).unwrap();
    exec.Close().unwrap();
    assert!(same_row(
        &deps.rt.writes.lock().unwrap().rows[0],
        &row(&[1, 3, 15])
    ));
}
#[test]
fn mview_delta_merge_snapshot_explicit_and_state_restored_after_error() {
    let deps = dependencies();
    let mut b = builder(deps.clone());
    let mut p = MergePlan {
        data: plan(),
        leaf: Leaf,
    };
    p.data.full_update_snapshot = Some(astersql_planner_core::DataReaderSnapshot { ts: 300 });
    b.build(Some(Plan::MViewDeltaMerge(&p))).unwrap();
    assert_eq!(*deps.calls.lock().unwrap(), vec!["update", "snapshot:300"]);
    b.getSnapshot().unwrap();
    assert_eq!(deps.calls.lock().unwrap().last().unwrap(), "read");
    p.data.agg_infos.clear();
    assert!(b.build(Some(Plan::MViewDeltaMerge(&p))).is_none());
    assert_eq!(b.getSnapshotTS().unwrap(), 100);
    let mut deps = dependencies();
    Arc::get_mut(&mut deps).unwrap().fail_child = true;
    let mut b = builder(deps);
    assert!(b.build(Some(Plan::MViewDeltaMerge(&p))).is_none());
    assert_eq!(b.getSnapshotTS().unwrap(), 100);
}
#[test]
fn mview_delta_merge_snapshot_rejects_nil_zero_ts_and_nil_infoschema() {
    let b = builder(dependencies());
    assert!(
        b.newDataReaderSnapshot(None, 200, Some(Arc::new(IS)))
            .is_err()
    );
    assert!(
        b.newDataReaderSnapshot(Some(Box::new(SS)), 0, Some(Arc::new(IS)))
            .is_err()
    );
    assert!(
        b.newDataReaderSnapshot(Some(Box::new(SS)), 200, None)
            .is_err()
    );
    assert_eq!(
        b.newDataReaderSnapshot(Some(Box::new(SS)), 200, Some(Arc::new(IS)))
            .unwrap()
            .ts,
        200
    );
}

#[test]
fn mview_delta_merge_snapshot_uses_for_update_and_restores_nested_state() {
    let deps = dependencies();
    let mut b = builder(deps.clone());
    assert_eq!(b.getSnapshotTS().unwrap(), 100);
    b.inMViewDeltaMergeStmt = true;
    assert_eq!(b.getSnapshotTS().unwrap(), 200);
    b.getSnapshot().unwrap();
    assert_eq!(deps.calls.lock().unwrap().last().unwrap(), "for-update");
    let p = MergePlan {
        data: plan(),
        leaf: Leaf,
    };
    assert!(b.build(Some(Plan::MViewDeltaMerge(&p))).is_some());
    assert!(b.inMViewDeltaMergeStmt);
    b.forDataReaderBuilder = true;
    b.dataReaderTS = 400;
    assert_eq!(b.getSnapshotTS().unwrap(), 400);
}

#[test]
fn mview_delta_merge_minmax_and_sum_dependency_type_guards() {
    let p = minmax_plan();
    let f = fields(8);
    let mut infos = p.agg_infos.clone();
    infos[0].dependencies.pop();
    assert!(mappings(&infos, &f, 5, 3).is_err());
    let mut infos = p.agg_infos.clone();
    infos[0].arg_not_null = true;
    infos[0].dependencies.push(6);
    assert!(mappings(&infos, &f, 5, 3).is_err());
    let mut infos = p.agg_infos.clone();
    infos[0].dependencies.push(6);
    assert!(mappings(&infos, &f, 5, 3).is_ok());
    let mut f = f;
    f[3] = NewFieldType(TypeVarchar);
    assert!(mappings(&p.agg_infos, &f, 5, 3).is_err());
    let mut f = fields(8);
    f[2].AddFlag(astersql_parser_mysql::r#type::UnsignedFlag);
    assert!(mappings(&p.agg_infos, &f, 5, 3).is_err());
    let mut infos = plan().agg_infos;
    infos[0].dependencies[1] = 4;
    assert!(mappings(&infos, &fields(5), 2, 3).is_err());
}

#[test]
fn mview_delta_merge_minmax_fast_path_balanced_and_deleted_groups() {
    let p = minmax_plan();
    let rt = Arc::new(Resource::new(
        vec![row(&[1, 2, 3]), row(&[2, 1, 3])],
        RecomputeStrategy::Batch,
    ));
    let child = Source::new(
        vec![
            row(&[0, 3, 1, 3, 1, 1, 2, 3]),
            minmax_row(&[-1, 0, 0, 3, 1, 2, 1, 3]),
        ],
        fields(8),
    );
    let mut exec = build(&p, Box::new(child), rt.clone(), reader()).unwrap();
    exec.Open().unwrap();
    let mut out = exec.NewChunk();
    exec.Next(&mut out).unwrap();
    exec.Close().unwrap();
    assert!(rt.reads.lock().unwrap().is_empty());
    assert_eq!(rt.writes.lock().unwrap().rows.len(), 1);
    assert!(rt.writes.lock().unwrap().touched.is_empty());
}

struct BadResource {
    inner: Resource,
    duplicate: bool,
}
impl MutationRuntime for BadResource {
    fn run_atomic(
        &self,
        f: &mut dyn FnMut(&mut dyn MutationBatch) -> Result<(), String>,
    ) -> Result<(), String> {
        self.inner.run_atomic(f)
    }
}
impl Resources for BadResource {
    fn refresh_context(&self) -> Option<(i32, Arc<astersql_sessionctx_stmtctx::StatementContext>)> {
        self.inner.refresh_context()
    }
    fn target_field_types(&self) -> Vec<FieldType> {
        self.inner.target_fields.clone()
    }
    fn read_full_update(
        &self,
        _: &PlanNode,
        _: &ReaderSnapshot,
        _: &LookupRequest,
    ) -> Result<ExecutorBox, BuildError> {
        if self.duplicate {
            Ok(Box::new(Source::new(
                vec![row(&[9, 1]), row(&[9, 1])],
                fields(2),
            )))
        } else {
            Ok(Box::new(Source::new(vec![], fields(1))))
        }
    }
}
#[test]
fn mview_delta_merge_recompute_rejects_duplicate_groups_and_wrong_schema() {
    for duplicate in [false, true] {
        let rt = Arc::new(BadResource {
            inner: Resource::new(vec![row(&[1, 2, 3])], RecomputeStrategy::Batch),
            duplicate,
        });
        let child = Source::new(vec![minmax_row(&[-1, 0, 0, 3, 1, 1, 2, 3])], fields(8));
        let mut exec = build(&minmax_plan(), Box::new(child), rt.clone(), reader()).unwrap();
        exec.Open().unwrap();
        let mut out = exec.NewChunk();
        assert!(exec.Next(&mut out).is_err());
        exec.Close().unwrap();
        assert!(same_row(
            &rt.inner.writes.lock().unwrap().rows[0],
            &row(&[1, 2, 3])
        ));
    }
}

#[test]
fn mview_delta_merge_unsigned_sum_initializes_with_unsigned_type() {
    let mut f = fields(5);
    f[4].AddFlag(astersql_parser_mysql::r#type::UnsignedFlag);
    let mut rt = Resource::new(vec![], RecomputeStrategy::Batch);
    rt.target_fields[2].AddFlag(astersql_parser_mysql::r#type::UnsignedFlag);
    let rt = Arc::new(rt);
    let child = Source::new(
        vec![vec![
            NewIntDatum(1),
            NewIntDatum(8),
            NewIntDatum(3),
            Datum::default(),
            Datum::default(),
        ]],
        f,
    );
    let mut exec = build(&plan(), Box::new(child), rt.clone(), reader()).unwrap();
    exec.Open().unwrap();
    let mut out = exec.NewChunk();
    exec.Next(&mut out).unwrap();
    exec.Close().unwrap();
    let w = rt.writes.lock().unwrap();
    assert_eq!(w.rows[0][2].Kind(), astersql_types::datum::KindUint64);
    assert_eq!(w.rows[0][2].GetUint64(), 8);
}

fn minmax_row(v: &[i64]) -> Vec<Datum> {
    let mut r = row(v);
    if v[2] == 0 {
        r[1] = Datum::default();
    }
    if v[4] == 0 {
        r[3] = Datum::default();
    }
    r
}
#[test]
fn mview_delta_merge_minmax_dominance_matches_go_for_min_and_max() {
    use MinMaxDecision::*;
    let f = NewFieldType(TypeLonglong);
    for (kind, sign) in [(K::Min, 1), (K::Max, -1)] {
        for (old, add, remove, ac, rc, expected) in [
            (3, 2, 2, 1, 1, Recompute), // canceled extremum outranks old
            (3, 3, 3, 1, 1, Old),
            (3, 2, 2, 2, 1, Added),
            (3, 2, 3, 1, 1, Added),
            (3, 4, 3, 1, 1, Recompute),
            (3, 5, 4, 1, 1, Old),
        ] {
            assert_eq!(
                minmax_decision(
                    &NewIntDatum(old * sign),
                    &NewIntDatum(add * sign),
                    &NewIntDatum(remove * sign),
                    ac,
                    rc,
                    kind,
                    &f
                )
                .unwrap(),
                expected
            );
        }
        assert!(
            minmax_decision(
                &Datum::default(),
                &Datum::default(),
                &NewIntDatum(3),
                0,
                1,
                kind,
                &f
            )
            .is_err()
        );
    }
}

#[test]
fn mview_delta_merge_hidden_handle_retained_for_mutations() {
    let mut p = plan();
    p.mv_table_pk_cols = vec![5];
    let rt = Arc::new(Resource::new(
        vec![row(&[1, 2, 10])],
        RecomputeStrategy::Batch,
    ));
    let child = Source::new(vec![row(&[1, 5, 1, 2, 10, 777])], fields(6));
    let mut exec = build(&p, Box::new(child), rt.clone(), reader()).unwrap();
    exec.Open().unwrap();
    let mut out = exec.NewChunk();
    exec.Next(&mut out).unwrap();
    exec.Close().unwrap();
    assert_eq!(rt.writes.lock().unwrap().sources[0][5].GetInt64(), 777);
}

#[test]
fn mview_delta_merge_recompute_retains_execution_context() {
    let mut rt = Resource::new(vec![row(&[1, 2, 3])], RecomputeStrategy::Batch);
    rt.require_context = true;
    let rt = Arc::new(rt);
    let child = Source::new(vec![minmax_row(&[-1, 0, 0, 3, 1, 1, 2, 3])], fields(8));
    let mut exec = build(&minmax_plan(), Box::new(child), rt.clone(), reader()).unwrap();
    exec.Open().unwrap();
    let mut out = exec.NewChunk();
    exec.NextWithContext(&ExecutionContext::default(), &mut out)
        .unwrap();
    exec.Close().unwrap();
    assert_eq!(rt.writes.lock().unwrap().rows[0][2].GetInt64(), 9);
}
#[test]
fn mview_delta_merge_rejects_target_types_and_duplicate_key_result_layout() {
    let mut rt = Resource::new(vec![], RecomputeStrategy::Batch);
    rt.target_fields[2] = NewFieldType(TypeVarchar);
    assert!(
        build(
            &plan(),
            Box::new(Source::new(vec![], fields(5))),
            Arc::new(rt),
            reader()
        )
        .is_err()
    );
    let mut p = minmax_plan();
    let maps = mappings(&p.agg_infos, &fields(8), 5, 3).unwrap();
    p.group_key_mv_offsets = vec![0, 1];
    p.full_update_key_off_to_index_off = vec![0, 1];
    p.full_update_key_result_col_indexes = vec![1, 1];
    assert!(
        recompute_layout(&p, &maps)
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
}

#[path = "mview_complete_delta_apply_builder_test.rs"]
mod complete_delta_apply;

#[test]
fn port_37a4cd4f43_merge_publishes_actual_writes_and_noops() {
    let rt = Arc::new(Resource::new(
        vec![row(&[1, 2, 10]), row(&[2, 1, 4])],
        RecomputeStrategy::Batch,
    ));
    let child = Source::new(
        vec![
            row(&[1, 5, 1, 2, 10]),
            row(&[-1, -4, 2, 1, 4]),
            vec![
                NewIntDatum(1),
                NewIntDatum(8),
                NewIntDatum(3),
                Datum::default(),
                Datum::default(),
            ],
            row(&[0, 0, 1, 3, 15]),
        ],
        fields(5),
    );
    let mut exec = build(&plan(), Box::new(child), rt.clone(), reader()).unwrap();
    exec.Open().unwrap();
    let mut out = exec.NewChunk();
    exec.Next(&mut out).unwrap();
    assert_eq!(rt.statement.AffectedRows(), 3);
    assert_eq!(
        rt.statement.GetMessage(),
        "Rows inserted: 1  Updated: 1  Deleted: 1"
    );
    let stats = rt
        .statement
        .RuntimeStatsColl
        .as_ref()
        .unwrap()
        .GetRootStatsStringShared(37);
    assert!(
        stats.contains("rows:{insert:1, update:1, delete:1, noop:1}"),
        "{stats}"
    );
    exec.Next(&mut out).unwrap();
    assert_eq!(rt.statement.AffectedRows(), 3);
    exec.Close().unwrap();
}

fn port_key_rows() -> Vec<Vec<Datum>> {
    use astersql_types::datum::NewCollationStringDatum;
    let text = || NewCollationStringDatum("same".into(), "utf8mb4_bin".into());
    let null = Datum::default;
    vec![
        vec![NewIntDatum(1), text()],
        vec![NewIntDatum(2), text()],
        vec![null(), text()],
        vec![NewIntDatum(1), null()],
        vec![NewIntDatum(2), null()],
        vec![null(), null()],
        vec![
            NewIntDatum(1),
            NewCollationStringDatum("".into(), "utf8mb4_bin".into()),
        ],
        vec![NewIntDatum(1), text()],
    ]
}

#[test]
fn port_37a4cd4f43_keys_preserve_null_length_duplicates_and_reorder() {
    let rows = port_key_rows();
    let same = crate::mview_delta_merge_builder::same;
    for (i, a) in rows.iter().enumerate() {
        for (j, b) in rows.iter().enumerate() {
            assert_eq!(same(a, b), i == j || [i, j] == [0, 7] || [i, j] == [7, 0]);
        }
    }
    let mut reused = rows.clone();
    reused.clear();
    for i in [7, 3, 1, 4] {
        reused.push(rows[i].clone());
    }
    for (key, i) in reused.iter().zip([7, 3, 1, 4]) {
        assert!(same(key, &rows[i]));
    }
}

struct CompositeKeysResource {
    inner: Resource,
}
impl MutationRuntime for CompositeKeysResource {
    fn run_atomic(
        &self,
        f: &mut dyn FnMut(&mut dyn MutationBatch) -> Result<(), String>,
    ) -> Result<(), String> {
        self.inner.run_atomic(f)
    }
}
impl Resources for CompositeKeysResource {
    fn refresh_context(&self) -> Option<(i32, Arc<astersql_sessionctx_stmtctx::StatementContext>)> {
        self.inner.refresh_context()
    }
    fn target_field_types(&self) -> Vec<FieldType> {
        self.inner.target_fields.clone()
    }
    fn read_full_update(
        &self,
        _: &PlanNode,
        snapshot: &ReaderSnapshot,
        request: &LookupRequest,
    ) -> Result<ExecutorBox, BuildError> {
        assert_eq!(request.key_to_index, vec![0, 1]);
        self.inner
            .reads
            .lock()
            .unwrap()
            .push((snapshot.ts, request.keys.len()));
        let rows = request
            .keys
            .iter()
            .rev()
            .map(|k| vec![NewIntDatum(9), k[0].clone(), k[1].clone()])
            .collect();
        Ok(Box::new(Source::new(
            rows,
            vec![NewFieldType(TypeLonglong), NewFieldType(TypeLonglong), {
                let mut tp = NewFieldType(TypeVarchar);
                tp.SetCollate("utf8mb4_bin".into());
                tp
            }],
        )))
    }
    fn recompute_strategy(&self) -> RecomputeStrategy {
        self.inner.strategy
    }
}

#[test]
fn port_37a4cd4f43_composite_keys_reach_real_recompute_and_writer() {
    for strategy in [RecomputeStrategy::SingleRow, RecomputeStrategy::Batch] {
        let keys = port_key_rows();
        let mut target_types = fields(4);
        target_types[1] = NewFieldType(TypeVarchar);
        target_types[1].SetCollate("utf8mb4_bin".into());
        let mut p = minmax_plan();
        p.mv_column_count = 4;
        p.count_star_mv_offset = 2;
        p.group_key_mv_offsets = vec![0, 1];
        p.agg_infos = vec![info(K::Min, 3, &[1, 2, 3, 4]), info(K::CountStar, 2, &[0])];
        p.full_update_inner_column_count = 3;
        p.full_update_key_off_to_index_off = vec![0, 1];
        p.full_update_index_ranges = vec![vec!["key".into(), "key2".into()]];
        p.full_update_key_result_col_indexes = vec![1, 2];
        p.full_update_output_mv_offsets = vec![3, 0, 1];
        let old: Vec<_> = keys[..7]
            .iter()
            .map(|k| vec![k[0].clone(), k[1].clone(), NewIntDatum(1), NewIntDatum(5)])
            .collect();
        let input = old
            .iter()
            .map(|old| {
                let mut input = vec![
                    NewIntDatum(0),
                    Datum::default(),
                    NewIntDatum(0),
                    NewIntDatum(5),
                    NewIntDatum(1),
                ];
                input.extend(old.iter().cloned());
                input
            })
            .collect();
        let mut inner = Resource::new(old, strategy);
        inner.target_fields = target_types.clone();
        let rt = Arc::new(CompositeKeysResource { inner });
        let mut source_types = fields(5);
        source_types.extend(target_types);
        let mut exec = build(
            &p,
            Box::new(Source::new(input, source_types)),
            rt.clone(),
            reader(),
        )
        .unwrap();
        exec.Open().unwrap();
        let mut output = exec.NewChunk();
        exec.Next(&mut output).unwrap();
        exec.Close().unwrap();
        let writes = rt.inner.writes.lock().unwrap();
        assert_eq!(writes.rows.len(), 7);
        for (r, k) in writes.rows.iter().zip(keys) {
            assert!(same_row(&r[..2], &k));
            assert_eq!(r[3].GetInt64(), 9);
        }
        assert_eq!(rt.inner.statement.AffectedRows(), 7);
        let reads = rt.inner.reads.lock().unwrap();
        assert!(reads.iter().all(|(ts, _)| *ts == reader().ts));
        assert_eq!(reads.iter().map(|(_, n)| *n).sum::<usize>(), 7);
    }
}

struct RefreshOperatorsResource {
    inner: Resource,
    target_ts: u64,
}
impl MutationRuntime for RefreshOperatorsResource {
    fn run_atomic(
        &self,
        f: &mut dyn FnMut(&mut dyn MutationBatch) -> Result<(), String>,
    ) -> Result<(), String> {
        self.inner.run_atomic(f)
    }
}
impl Resources for RefreshOperatorsResource {
    fn refresh_context(&self) -> Option<(i32, Arc<astersql_sessionctx_stmtctx::StatementContext>)> {
        self.inner.refresh_context()
    }
    fn target_field_types(&self) -> Vec<FieldType> {
        self.inner.target_fields.clone()
    }
    fn complete_delta_target_columns(
        &self,
    ) -> Result<crate::mview_complete_delta_apply_builder::TargetColumns, BuildError> {
        Ok(complete_delta_apply::target_columns(
            &self.inner.target_fields,
        ))
    }
    fn read_full_update(
        &self,
        _: &PlanNode,
        snapshot: &ReaderSnapshot,
        request: &LookupRequest,
    ) -> Result<ExecutorBox, BuildError> {
        self.inner
            .reads
            .lock()
            .unwrap()
            .push((snapshot.ts, request.keys.len()));
        let rows = request
            .keys
            .iter()
            .map(|key| {
                let g = key[0].GetInt64();
                // A later committed value 1 must not replace value 5 at target_ts.
                let value = if g == 1 {
                    10
                } else if snapshot.ts == self.target_ts {
                    5
                } else {
                    1
                };
                row(&[value, value, g])
            })
            .collect();
        Ok(Box::new(Source::new(rows, fields(3))))
    }
}
fn optional_row(values: &[Option<i64>]) -> Vec<Datum> {
    values
        .iter()
        .map(|v| v.map_or_else(Datum::default, NewIntDatum))
        .collect()
}
#[test]
fn port_37a4cd4f43_refresh_operators_keep_go_rows_counts_and_target_snapshot() {
    for fixed in [false, true] {
        let mut snapshot = reader();
        if fixed {
            snapshot.ts = 42;
        }
        let mut inner = Resource::new(
            vec![row(&[1, 2, 2, 30, 10, 20]), row(&[2, 1, 1, 30, 30, 30])],
            RecomputeStrategy::Batch,
        );
        inner.target_fields = fields(6);
        let rt = Arc::new(RefreshOperatorsResource {
            inner,
            target_ts: snapshot.ts,
        });
        let mut p = minmax_plan();
        p.mv_column_count = 6;
        p.delta_column_count = 11;
        p.mv_table_pk_cols = vec![11];
        p.count_star_mv_offset = 1;
        p.agg_infos = vec![
            info(K::CountStar, 1, &[0]),
            info(K::Count, 2, &[1]),
            info(K::Sum, 3, &[2, 13]),
            info(K::Min, 4, &[3, 4, 5, 6, 13]),
            info(K::Max, 5, &[7, 8, 9, 10, 13]),
        ];
        p.full_update_inner_column_count = 3;
        p.full_update_key_result_col_indexes = vec![2];
        p.full_update_output_mv_offsets = vec![4, 5, 0];
        let deltas = vec![
            optional_row(&[
                Some(0),
                Some(-1),
                Some(-20),
                None,
                Some(0),
                Some(20),
                Some(1),
                None,
                Some(0),
                Some(20),
                Some(1),
                Some(1),
                Some(2),
                Some(2),
                Some(30),
                Some(10),
                Some(20),
            ]),
            row(&[0, 0, -25, 5, 1, 30, 1, 5, 1, 30, 1, 2, 1, 1, 30, 30, 30]),
            optional_row(&[
                Some(1),
                Some(1),
                Some(40),
                Some(40),
                Some(1),
                None,
                Some(0),
                Some(40),
                Some(1),
                None,
                Some(0),
                Some(3),
                None,
                None,
                None,
                None,
                None,
            ]),
        ];
        let mut exec = build(
            &p,
            Box::new(Source::new(deltas, fields(17))),
            rt.clone(),
            snapshot,
        )
        .unwrap();
        exec.Open().unwrap();
        let mut out = exec.NewChunk();
        exec.Next(&mut out).unwrap();
        exec.Close().unwrap();
        assert_eq!(rt.inner.statement.AffectedRows(), 3);
        let stats = rt
            .inner
            .statement
            .RuntimeStatsColl
            .as_ref()
            .unwrap()
            .GetRootStatsStringShared(37);
        assert!(
            stats.contains("rows:{insert:1, update:2, delete:0, noop:0}"),
            "{stats}"
        );
        let fast_rows = vec![
            row(&[1, 2, 1, 10, 10, 10]),
            row(&[2, 1, 1, 5, 5, 5]),
            row(&[3, 1, 1, 40, 40, 40]),
        ];
        {
            let writes = rt.inner.writes.lock().unwrap();
            for (a, b) in writes.rows.iter().zip(&fast_rows) {
                assert!(same_row(a, b));
            }
            assert_eq!(writes.rows.len(), 3);
        }
        assert!(
            rt.inner
                .reads
                .lock()
                .unwrap()
                .iter()
                .all(|(ts, _)| *ts == rt.target_ts)
        );
        let final_rows = vec![
            optional_row(&[Some(1), Some(1), Some(0), None, None, None]),
            row(&[3, 2, 2, 100, 40, 60]),
            row(&[4, 1, 1, 7, 7, 7]),
        ];
        let mut input = Vec::new();
        for (op, old, new) in [
            (3, fast_rows[0].clone(), final_rows[0].clone()),
            (2, fast_rows[1].clone(), vec![Datum::default(); 6]),
            (3, fast_rows[2].clone(), final_rows[1].clone()),
            (1, vec![Datum::default(); 6], final_rows[2].clone()),
        ] {
            let mut r = vec![NewIntDatum(op)];
            r.extend(old);
            r.extend(new);
            input.push(r);
        }
        let mut complete = complete_delta_apply::complete_plan();
        complete.mv_column_count = 6;
        complete.current_row_input_col_ids = (1..7).collect();
        complete.recomputed_row_input_col_ids = (7..13).collect();
        let mut exec = crate::mview_complete_delta_apply_builder::build(
            &complete,
            Box::new(Source::new(input, fields(13))),
            rt.clone(),
        )
        .unwrap();
        exec.Open().unwrap();
        let mut out = exec.NewChunk();
        exec.Next(&mut out).unwrap();
        exec.Close().unwrap();
        assert_eq!(rt.inner.statement.AffectedRows(), 4);
        let stats = rt
            .inner
            .statement
            .RuntimeStatsColl
            .as_ref()
            .unwrap()
            .GetRootStatsStringShared(37);
        assert!(
            stats.contains("rows:{insert:1, update:2, delete:1}"),
            "{stats}"
        );
        let writes = rt.inner.writes.lock().unwrap();
        assert_eq!(writes.rows.len(), 3);
        for (a, b) in writes.rows.iter().zip(&final_rows) {
            assert!(same_row(a, b));
        }
    }
}

#[test]
fn port_37a4cd4f43_writer_failure_registers_progress_without_success_result() {
    for complete in [false, true] {
        let mut resource = Resource::new(vec![], RecomputeStrategy::Batch);
        resource.fail = true;
        resource.statement.SetAffectedRows(17);
        resource.statement.SetMessage("prior result");
        let rt = Arc::new(resource);
        let mut exec = if complete {
            crate::mview_complete_delta_apply_builder::build(
                &complete_delta_apply::complete_plan(),
                Box::new(Source::new(vec![row(&[1, 0, 0, 0, 3, 1, 8])], fields(7))),
                rt.clone(),
            )
            .unwrap()
        } else {
            build(
                &plan(),
                Box::new(Source::new(
                    vec![vec![
                        NewIntDatum(1),
                        NewIntDatum(8),
                        NewIntDatum(3),
                        Datum::default(),
                        Datum::default(),
                    ]],
                    fields(5),
                )),
                rt.clone(),
                reader(),
            )
            .unwrap()
        };
        exec.Open().unwrap();
        let mut output = exec.NewChunk();
        assert!(
            exec.Next(&mut output)
                .unwrap_err()
                .to_string()
                .contains("write failure")
        );
        assert!(rt.writes.lock().unwrap().rows.is_empty());
        assert_eq!(rt.statement.AffectedRows(), 17);
        assert_eq!(rt.statement.GetMessage(), "prior result");
        let coll = rt.statement.RuntimeStatsColl.as_ref().unwrap();
        let observed = coll.GetRootStatsStringShared(37);
        assert!(
            observed.contains("chunks:1, row_ops:1, rows:{insert:1, update:0, delete:0"),
            "{observed}"
        );
        exec.Next(&mut output).unwrap();
        assert_eq!(coll.GetRootStatsStringShared(37), observed);
        exec.Close().unwrap();
    }
}
