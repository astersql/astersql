// Copyright 2026 AsterSQL.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use astersql_executor::builder::{
    BuildError, BuildTypedPhysicalPlan, BuildTypedPhysicalPlanWithBindings,
    BuildTypedPhysicalSelectLockPlan, ExecutorBox, TypedScanBinding,
};
use astersql_executor::typed_kv_scan::{KeyRange, TypedKVScan};
use astersql_kv as kv;
use astersql_meta_model::ColumnInfo;
use astersql_planner_core_base::PhysicalPlan;
use astersql_planner_core_operator_physicalop::{
    PhysicalIndexLookUpReader, PhysicalIndexReader, PhysicalIndexScan, PhysicalTableReader,
    PhysicalTableScan, PointGetPlan,
};

use super::ConcreteSession;

/// A pinned MVCC snapshot source owned by the result executor rather than by
/// the mutable ConcreteSession. Every iterator is created from the same
/// version, and the storage registry keeps the backend alive after the session
/// statement has returned.
pub struct OwnedKVSnapshotSource {
    domain: Arc<astersql_domain::Domain>,
    version: kv::Version,
}

/// The original statement keeps the canonical, thread-local session alive.
/// A detached scan holds only OwnedKVSnapshotSource and never retains this owner.
pub struct SessionBoundAdapterOwner {
    pub(super) session: Rc<ConcreteSession>,
    pub(super) scan: RefCell<Option<TypedScanSpec>>,
    pub(super) physical_scan: RefCell<Option<PhysicalScanBinding>>,
    pub(super) prepared_binding: RefCell<Option<PreparedBinding>>,
    pub(super) dml_sql: RefCell<Option<String>>,
    pub(super) analyze_sql: RefCell<Option<String>>,
    pub(super) fk_savepoint: RefCell<Option<String>>,
    pub(super) effects: RefCell<ScanAdapterEffects>,
    pub(super) statement_context: RefCell<astersql_executor::adapter::StatementContext>,
    pub(super) deadline: RefCell<Option<ExecutionDeadline>>,
    pub(super) statement_locks_before: RefCell<Option<HashSet<super::RuntimeRowLockKey>>>,
    pub(super) statement_mutation_stage: RefCell<Option<StatementMutationStage>>,
    pub(super) retry_lock_conflict_once: Cell<bool>,
    pub(super) top_sql_stats: RefCell<Option<Arc<astersql_util_topsql_stmtstats::StatementStats>>>,
    pub(super) top_sql_current: RefCell<Option<(Vec<u8>, Vec<u8>, std::time::SystemTime)>>,
    pub(super) runaway_checker:
        RefCell<Option<Arc<astersql_resourcegroup_runaway::checker::Checker>>>,
    pub(super) runaway_resource_group_override: RefCell<Option<String>>,
    pub(super) point_cache: RefCell<
        HashMap<String, Arc<std::sync::Mutex<astersql_executor::typed_point_get::TypedPointGet>>>,
    >,
}

impl Drop for SessionBoundAdapterOwner {
    fn drop(&mut self) {
        if let Some(stats) = self.top_sql_stats.get_mut().take() {
            stats.SetFinished();
        }
    }
}

pub(super) struct StatementMutationStage {
    pub handle: astersql_kv::StagingHandle,
    pub write_keys_before: HashSet<super::RuntimeRowLockKey>,
    pub buffer_keys_before: u64,
    pub buffer_bytes_before: u64,
    pub conflict_before: Option<(String, String)>,
    pub dml_report_before: Option<crate::dml_runtime::DmlExecutionReport>,
    pub pending_cascades_before: Vec<super::RuntimeForeignKeyDeleteCascade>,
}

pub(super) struct ExecutionDeadline {
    pub started: std::time::SystemTime,
    pub maximum_ms: u64,
    cancel: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ExecutionDeadline {
    pub fn new(
        killer: Arc<astersql_domain::SQLKiller>,
        started: std::time::SystemTime,
        maximum_ms: u64,
    ) -> Self {
        let cancel = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let notified = Arc::clone(&cancel);
        let elapsed = started.elapsed().unwrap_or_default();
        let remaining = std::time::Duration::from_millis(maximum_ms).saturating_sub(elapsed);
        let worker = std::thread::spawn(move || {
            let (lock, condition) = &*notified;
            let cancelled = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (cancelled, timeout) = condition
                .wait_timeout_while(cancelled, remaining, |cancelled| !*cancelled)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !*cancelled && timeout.timed_out() {
                killer.SendKillSignal(astersql_util_sqlkiller::sqlkiller::MaxExecTimeExceeded);
            }
        });
        Self {
            started,
            maximum_ms,
            cancel,
            worker: Some(worker),
        }
    }
}

impl Drop for ExecutionDeadline {
    fn drop(&mut self) {
        let (lock, condition) = &*self.cancel;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        condition.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Default)]
pub struct ScanAdapterEffects {
    pub events: Vec<String>,
    pub process_sql: String,
    pub process_started: Option<std::time::SystemTime>,
    pub last_found_rows: u64,
    pub last_scanned_rows: usize,
    pub audited_sql: Vec<String>,
    pub slow_queries: Vec<(String, bool)>,
    pub summaries: Vec<String>,
    pub previous_statement: Option<(String, String)>,
    pub last_error: Option<String>,
    pub top_sql_started: usize,
    pub top_sql_finished: usize,
}

#[derive(Clone)]
pub struct TypedScanSpec {
    pub table_id: i64,
    pub pk_is_handle: bool,
    pub descending: bool,
    pub columns: Vec<ColumnInfo>,
    pub ranges: Vec<KeyRange>,
    pub version: kv::Version,
    pub initial_capacity: usize,
    pub maximum_chunk_size: usize,
}

pub(super) struct PhysicalScanBinding {
    pub plan: BoundPhysicalPlan,
    pub ranges: Vec<KeyRange>,
    pub leaf_ranges: Vec<(i64, Vec<KeyRange>)>,
    pub version: kv::Version,
    pub initial_capacity: usize,
    pub maximum_chunk_size: usize,
}

#[derive(Clone)]
pub(super) struct PreparedBinding {
    pub statement_id: u64,
    pub parameters: Vec<astersql_types::datum::Datum>,
    pub initial_capacity: usize,
    pub maximum_chunk_size: usize,
}

pub(super) enum BoundPhysicalPlan {
    Plain(Box<dyn PhysicalPlan>),
    Prepared(super::PreparedKVPhysicalPlan),
}

impl BoundPhysicalPlan {
    pub fn as_plan(&self) -> &dyn PhysicalPlan {
        match self {
            Self::Plain(plan) => plan.as_ref(),
            Self::Prepared(planned) => planned.Plan.as_ref(),
        }
    }

    pub fn is_prepared(&self) -> bool {
        matches!(self, Self::Prepared(_))
    }
}

pub(super) fn collect_table_scans<'a>(
    plan: &'a dyn PhysicalPlan,
    scans: &mut Vec<&'a PhysicalTableScan>,
) {
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalIndexLookUpReader>() {
        if let Some(table_plan) = reader.TablePlan.as_deref() {
            collect_table_scans(table_plan, scans);
        }
        return;
    }
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalTableReader>() {
        if let Some(child) = reader.GetTablePlan() {
            collect_table_scans(child, scans);
        }
        return;
    }
    if let Some(scan) = plan.as_any().downcast_ref::<PhysicalTableScan>() {
        scans.push(scan);
        return;
    }
    for child in plan.children() {
        collect_table_scans(child, scans);
    }
}

pub(super) fn find_index_lookup<'a>(
    plan: &'a dyn PhysicalPlan,
) -> Option<&'a PhysicalIndexLookUpReader> {
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalIndexLookUpReader>() {
        return Some(reader);
    }
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalTableReader>() {
        return reader.GetTablePlan().and_then(find_index_lookup);
    }
    plan.children().into_iter().find_map(find_index_lookup)
}

fn find_index_reader<'a>(plan: &'a dyn PhysicalPlan) -> Option<&'a PhysicalIndexReader> {
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalIndexReader>() {
        return Some(reader);
    }
    plan.children().into_iter().find_map(find_index_reader)
}

fn find_index_scan(plan: &dyn PhysicalPlan) -> Option<&PhysicalIndexScan> {
    if let Some(scan) = plan.as_any().downcast_ref::<PhysicalIndexScan>() {
        return Some(scan);
    }
    let children = plan.children();
    (children.len() == 1)
        .then(|| find_index_scan(children[0]))
        .flatten()
}

fn encode_index_ranges(
    scan: &PhysicalIndexScan,
    table_id: i64,
) -> Result<Vec<KeyRange>, BuildError> {
    let index = scan
        .Index
        .as_ref()
        .ok_or_else(|| BuildError::new("PhysicalIndexScan has no IndexInfo"))?;
    let ranges = scan
        .RebuildRangesForPlanCache()
        .map_err(|error| BuildError::new(error.to_string()))?;
    ranges
        .iter()
        .map(|range| {
            let mut low = astersql_util_codec::EncodeKey(
                astersql_tablecodec::time::UTC,
                Vec::new(),
                range.LowVal.clone(),
            )
            .map_err(|error| BuildError::new(error.to_string()))?;
            if range.LowExclude {
                low = kv::Key(low).PrefixNext().0;
            }
            let mut high = astersql_util_codec::EncodeKey(
                astersql_tablecodec::time::UTC,
                Vec::new(),
                range.HighVal.clone(),
            )
            .map_err(|error| BuildError::new(error.to_string()))?;
            if !range.HighExclude {
                high = kv::Key(high).PrefixNext().0;
            }
            Ok(KeyRange {
                start: kv::Key(
                    astersql_tablecodec::EncodeIndexSeekKey(table_id, index.ID, Some(low)).0,
                ),
                end: kv::Key(
                    astersql_tablecodec::EncodeIndexSeekKey(table_id, index.ID, Some(high)).0,
                ),
            })
        })
        .collect()
}

fn encode_table_record_ranges(
    scan: &PhysicalTableScan,
    table_id: i64,
) -> Result<Vec<KeyRange>, BuildError> {
    let table = scan
        .Table
        .as_ref()
        .ok_or_else(|| BuildError::new("prepared PhysicalTableScan has no TableInfo"))?;
    if !table.PKIsHandle || scan.IsCommonHandle {
        return Err(BuildError::new(
            "prepared typed KV range encoder requires an integer handle range",
        ));
    }
    let ranges = scan
        .RebuildRangesForPlanCache()
        .map_err(|error| BuildError::new(error.to_string()))?;
    ranges
        .iter()
        .map(|range| {
            let [low] = range.LowVal.as_slice() else {
                return Err(BuildError::new(
                    "prepared typed KV range encoder requires one lower handle",
                ));
            };
            let [high] = range.HighVal.as_slice() else {
                return Err(BuildError::new(
                    "prepared typed KV range encoder requires one upper handle",
                ));
            };
            if low.Kind() != astersql_types::datum::KindInt64
                || high.Kind() != astersql_types::datum::KindInt64
            {
                return Err(BuildError::new(
                    "prepared typed KV range encoder requires signed integer handles",
                ));
            }
            let mut start = astersql_tablecodec::EncodeRowKeyWithHandle(
                table_id,
                Box::new(kv::IntHandle(low.GetInt64())),
            );
            if range.LowExclude {
                start = start.PrefixNext();
            }
            let mut end = astersql_tablecodec::EncodeRowKeyWithHandle(
                table_id,
                Box::new(kv::IntHandle(high.GetInt64())),
            );
            if !range.HighExclude {
                end = end.PrefixNext();
            }
            Ok(KeyRange { start, end })
        })
        .collect()
}

impl SessionBoundAdapterOwner {
    pub fn PreparedResultMetadata(
        &self,
    ) -> Option<(bool, Vec<String>, super::ProcessPlanSnapshot)> {
        let binding = self.physical_scan.borrow();
        let binding = binding.as_ref()?;
        let BoundPhysicalPlan::Prepared(prepared) = &binding.plan else {
            return None;
        };
        Some((
            prepared.FromPlanCache,
            prepared.Warnings.clone(),
            prepared.Snapshot.clone(),
        ))
    }

    /// Construct the statement metadata from the bound prepared physical plan.
    /// This is the canonical session entry for executing a planned KV SELECT
    /// through ExecStmt instead of duplicating the plan in a summary-only path.
    pub fn BuildPreparedExecStmt(
        self: &Arc<Self>,
    ) -> Result<astersql_executor::adapter::ExecStmt, BuildError> {
        use astersql_executor::adapter::{
            FieldName, PlanInfo, PlanKind, SchemaColumn, StatementKind, StatementNode,
        };

        let binding = self.physical_scan.borrow();
        let Some(binding) = binding.as_ref() else {
            return Err(BuildError::new("prepared physical plan was not bound"));
        };
        let BoundPhysicalPlan::Prepared(prepared) = &binding.plan else {
            return Err(BuildError::new("bound plan is not a prepared SELECT"));
        };
        let plan = prepared.Plan.as_ref();
        let evaluation = plan.s_ctx().GetExprCtx().GetEvalCtx();
        let columns = &plan.schema().Columns;
        let schema = columns
            .iter()
            .map(|column| SchemaColumn {
                field_type: column.GetType(evaluation).clone(),
            })
            .collect();
        let output_names = columns
            .iter()
            .map(|column| FieldName {
                column_name: column.OrigName.clone(),
                ..Default::default()
            })
            .collect();
        let sql = prepared.SQLText.clone();
        let summary = PlanInfo {
            id: plan.id(),
            kind: PlanKind::Query,
            schema,
            calculate_no_delay: false,
            projection_child: None,
            encoded: prepared.Snapshot.Operators.join(" -> "),
            binary: String::new(),
            hints: String::new(),
        };
        self.BuildExecStmt(
            summary,
            StatementNode {
                kind: StatementKind::Execute,
                original_text: sql.clone(),
                text: sql.clone(),
                secure_text: sql,
                prepared_text: None,
            },
            output_names,
            true,
        )
    }

    /// Build an executable statement from the physical plan currently bound to
    /// this session owner. The typed tree is cloned from the actual plan used
    /// by the executor, so RU traversal never falls back to `PlanInfo` text.
    pub fn BuildExecStmt(
        self: &Arc<Self>,
        plan_summary: astersql_executor::adapter::PlanInfo,
        statement: astersql_executor::adapter::StatementNode,
        output_names: Vec<astersql_executor::adapter::FieldName>,
        prepared: bool,
    ) -> Result<astersql_executor::adapter::ExecStmt, BuildError> {
        use astersql_executor::adapter::{ExecStmt, StatementContext};

        let binding = self.physical_scan.borrow();
        let physical = binding
            .as_ref()
            .ok_or_else(|| BuildError::new("typed physical plan was not bound"))?;
        let plan = physical.plan.as_plan();
        let cloned = plan
            .clone_physical(plan.s_ctx().clone())
            .map_err(|error| BuildError::new(error.to_string()))?;
        let typed_plan: Box<dyn astersql_planner_core_base::Plan> = cloned;
        let typed_plan = Arc::from(typed_plan);
        let sql = statement.text.clone();
        let statement_kind = statement.kind;
        Ok(ExecStmt {
            GoCtx: None,
            InfoSchema: 0,
            Plan: plan_summary.clone(),
            TypedPlan: Some(typed_plan),
            StmtNode: statement,
            Ctx: self.clone(),
            LowerPriority: false,
            isPreparedStmt: prepared,
            isSelectForUpdate: false,
            retryCount: 0,
            retryStartTime: None,
            phaseBuildDurations: [std::time::Duration::ZERO; 2],
            phaseOpenDurations: [std::time::Duration::ZERO; 2],
            phaseNextDurations: [std::time::Duration::ZERO; 2],
            phaseLockDurations: [std::time::Duration::ZERO; 2],
            OutputNames: output_names,
            PsStmt: None,
            Ti: None,
            StatementCtx: StatementContext {
                statement_type: format!("{statement_kind:?}"),
                sql_normalized: sql,
                plan: Some(plan_summary),
                ..Default::default()
            },
        })
    }

    pub fn new(session: ConcreteSession) -> Self {
        Self {
            session: Rc::new(session),
            scan: RefCell::new(None),
            physical_scan: RefCell::new(None),
            prepared_binding: RefCell::new(None),
            dml_sql: RefCell::new(None),
            analyze_sql: RefCell::new(None),
            fk_savepoint: RefCell::new(None),
            effects: RefCell::new(ScanAdapterEffects::default()),
            statement_context: RefCell::new(astersql_executor::adapter::StatementContext::default()),
            deadline: RefCell::new(None),
            statement_locks_before: RefCell::new(None),
            statement_mutation_stage: RefCell::new(None),
            retry_lock_conflict_once: Cell::new(false),
            top_sql_stats: RefCell::new(None),
            top_sql_current: RefCell::new(None),
            runaway_checker: RefCell::new(None),
            runaway_resource_group_override: RefCell::new(None),
            point_cache: RefCell::new(HashMap::new()),
        }
    }

    pub fn Effects(&self) -> ScanAdapterEffects {
        self.effects.borrow().clone()
    }

    pub fn LastFoundRows(&self) -> u64 {
        self.session.WithSessionVars(|vars| vars.GetLastFoundRows())
    }

    pub fn StatementFoundRows(&self) -> u64 {
        self.session
            .WithSessionVars(|vars| vars.StmtCtx.FoundRows())
    }

    pub fn StatementTransactionStartTS(&self) -> u64 {
        self.session.state.borrow().statement_txn_start_ts
    }

    pub fn CanonicalTxnStartTS(&self) -> u64 {
        self.session.WithSessionVars(|vars| vars.TxnCtx.StartTS())
    }

    pub fn HeldRowLockCount(&self) -> usize {
        self.session.HeldRowLockCount()
    }

    pub fn TryLockKeys(&self, keys: &[Vec<u8>]) -> Result<(), astersql_errors::SharedError> {
        let domain_id = Arc::as_ptr(&self.session.domain) as usize;
        let keys = keys
            .iter()
            .cloned()
            .map(|key| super::RuntimeRowLockKey { domain_id, key })
            .collect();
        self.session
            .acquire_row_locks(keys, true, None, false)
            .map_err(|error| astersql_errors::New(error.to_string()))
    }

    pub fn LastPlanFromCache(&self) -> bool {
        self.session.LastPlanFromCache()
    }

    pub fn PlanPreparedPlannedKVSelect(
        &self,
        statement_id: u64,
        parameters: &[astersql_types::datum::Datum],
    ) -> super::SessionResult<super::PreparedKVPhysicalPlan> {
        self.session
            .PlanPreparedPlannedKVSelect(statement_id, parameters)
    }

    pub fn BindTypedScan(&self, scan: TypedScanSpec) {
        *self.physical_scan.borrow_mut() = None;
        self.prepared_binding.borrow_mut().take();
        *self.scan.borrow_mut() = Some(scan);
    }

    /// Bind the canonical DML AST text before ExecStmt builds its lazy writer.
    pub fn BindDMLStatement(&self, sql: &str) -> super::SessionResult<()> {
        let mode =
            astersql_parser_mysql::r#const::GetSQLMode(&self.session.state.borrow().sql_mode)
                .map_err(|error| super::SessionError::new(error.to_string()))?;
        let statements = super::parse_with_sql_mode(sql, mode)?;
        if statements.len() != 1
            || !statements[0]
                .as_any()
                .is::<astersql_parser_ast::InsertStmt>()
                && !statements[0]
                    .as_any()
                    .is::<astersql_parser_ast::UpdateStmt>()
                && !statements[0]
                    .as_any()
                    .is::<astersql_parser_ast::DeleteStmt>()
        {
            return Err(super::SessionError::new(
                "adapter DML binding requires one INSERT, UPDATE or DELETE",
            ));
        }
        *self.dml_sql.borrow_mut() = Some(sql.to_owned());
        Ok(())
    }

    pub fn BindAnalyzeStatement(&self, sql: &str) -> super::SessionResult<()> {
        let mode =
            astersql_parser_mysql::r#const::GetSQLMode(&self.session.state.borrow().sql_mode)
                .map_err(|error| super::SessionError::new(error.to_string()))?;
        let statements = super::parse_with_sql_mode(sql, mode)?;
        if statements.len() != 1
            || !statements[0]
                .as_any()
                .is::<astersql_parser_ast::AnalyzeTableStmt>()
        {
            return Err(super::SessionError::new(
                "adapter ANALYZE binding requires one ANALYZE TABLE",
            ));
        }
        *self.analyze_sql.borrow_mut() = Some(sql.to_owned());
        Ok(())
    }

    pub fn BindPhysicalTableScan(
        &self,
        plan: PhysicalTableScan,
        ranges: Vec<KeyRange>,
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<(), BuildError> {
        self.BindTypedPhysicalPlan(
            Box::new(plan),
            ranges,
            version,
            initial_capacity,
            maximum_chunk_size,
        )
    }

    pub fn BindTypedPhysicalPlan(
        &self,
        plan: Box<dyn PhysicalPlan>,
        ranges: Vec<KeyRange>,
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<(), BuildError> {
        self.prepared_binding.borrow_mut().take();
        self.bind_physical_plan(
            BoundPhysicalPlan::Plain(plan),
            ranges,
            version,
            initial_capacity,
            maximum_chunk_size,
        )
    }

    /// Bind an existing canonical prepared statement through the same
    /// parameterized optimizer/cache path used by ExecutePreparedPlannedKVSelect.
    /// Encodes the current bound record or index ranges before opening KV.
    pub fn BindPreparedPlannedKVSelect(
        &self,
        statement_id: u64,
        parameters: &[astersql_types::datum::Datum],
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<(), BuildError> {
        let planned = self
            .session
            .PlanPreparedPlannedKVSelect(statement_id, parameters)
            .map_err(|error| BuildError::new(error.to_string()))?;
        let mut scans = Vec::new();
        collect_table_scans(planned.Plan.as_ref(), &mut scans);
        let ranges = if planned.Plan.as_any().is::<PointGetPlan>() {
            Vec::new()
        } else if let Some(reader) = find_index_lookup(planned.Plan.as_ref()) {
            if scans.len() != 1 {
                return Err(BuildError::new(
                    "prepared index lookup requires one table scan",
                ));
            }
            let table = scans[0]
                .Table
                .as_ref()
                .ok_or_else(|| BuildError::new("prepared table scan has no TableInfo"))?;
            let table_id = if scans[0].PhysicalTableID != 0 {
                scans[0].PhysicalTableID
            } else {
                table.ID
            };
            let index_scan = reader
                .IndexPlan
                .as_deref()
                .and_then(find_index_scan)
                .ok_or_else(|| {
                    BuildError::new("prepared index lookup requires one PhysicalIndexScan")
                })?;
            encode_index_ranges(index_scan, table_id)?
        } else if let Some(reader) = find_index_reader(planned.Plan.as_ref()) {
            let index_scan = reader
                .IndexPlan
                .as_deref()
                .and_then(find_index_scan)
                .ok_or_else(|| {
                    BuildError::new("prepared IndexReader requires one PhysicalIndexScan")
                })?;
            let table = index_scan
                .Table
                .as_ref()
                .ok_or_else(|| BuildError::new("prepared IndexScan has no TableInfo"))?;
            let table_id = if index_scan.PhysicalTableID != 0 {
                index_scan.PhysicalTableID
            } else {
                table.ID
            };
            encode_index_ranges(index_scan, table_id)?
        } else {
            if scans.is_empty() {
                return Err(BuildError::new(
                    "prepared typed KV range encoder requires one table scan",
                ));
            }
            if scans.len() > 1 {
                Vec::new()
            } else {
                let scan = scans[0];
                let table = scan
                    .Table
                    .as_ref()
                    .ok_or_else(|| BuildError::new("prepared table scan has no TableInfo"))?;
                let table_id = if scan.PhysicalTableID != 0 {
                    scan.PhysicalTableID
                } else {
                    table.ID
                };
                if scan.AccessCondition.is_empty() {
                    let start = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table_id).0);
                    vec![KeyRange {
                        end: start.PrefixNext(),
                        start,
                    }]
                } else {
                    encode_table_record_ranges(scan, table_id)?
                }
            }
        };
        let version = self
            .session
            .domain
            .storage()
            .with_storage(|storage| storage.CurrentVersion("global"))
            .map_err(|error| BuildError::new(error.to_string()))?;
        self.bind_physical_plan(
            BoundPhysicalPlan::Prepared(planned),
            ranges,
            version,
            initial_capacity,
            maximum_chunk_size,
        )?;
        *self.prepared_binding.borrow_mut() = Some(PreparedBinding {
            statement_id,
            parameters: parameters.to_vec(),
            initial_capacity,
            maximum_chunk_size,
        });
        Ok(())
    }

    fn bind_physical_plan(
        &self,
        plan: BoundPhysicalPlan,
        ranges: Vec<KeyRange>,
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<(), BuildError> {
        if let Some(point) = plan.as_plan().as_any().downcast_ref::<PointGetPlan>() {
            let table = point
                .TblInfo
                .as_ref()
                .ok_or_else(|| BuildError::new("PointGet has no TableInfo"))?;
            let table_id = if let Some(index) = point.PartitionIdx {
                table
                    .Partition
                    .as_ref()
                    .and_then(|partition| partition.Definitions.get(index))
                    .map(|definition| definition.ID)
                    .ok_or_else(|| BuildError::new("PointGet partition index is invalid"))?
            } else {
                table.ID
            };
            self.BindTypedScan(TypedScanSpec {
                table_id,
                pk_is_handle: table.PKIsHandle,
                descending: false,
                columns: point.Columns.clone(),
                ranges: Vec::new(),
                version,
                initial_capacity,
                maximum_chunk_size,
            });
            *self.physical_scan.borrow_mut() = Some(PhysicalScanBinding {
                plan,
                ranges: Vec::new(),
                leaf_ranges: Vec::new(),
                version,
                initial_capacity,
                maximum_chunk_size,
            });
            return Ok(());
        }
        let mut scans = Vec::new();
        collect_table_scans(plan.as_plan(), &mut scans);
        if scans.is_empty() && find_index_reader(plan.as_plan()).is_none() {
            return Err(BuildError::new(format!(
                "typed physical builder requires at least one table scan, got {}",
                scans.len()
            )));
        }
        let (table_id, pk_is_handle, descending, columns) = if let Some(scan) = scans.first() {
            let table = scan
                .Table
                .as_ref()
                .ok_or_else(|| BuildError::new("PhysicalTableScan has no TableInfo"))?;
            (
                if scan.PhysicalTableID != 0 {
                    scan.PhysicalTableID
                } else {
                    table.ID
                },
                table.PKIsHandle,
                scan.Desc,
                scan.Columns.clone(),
            )
        } else {
            let reader = find_index_reader(plan.as_plan())
                .ok_or_else(|| BuildError::new("typed IndexReader is absent"))?;
            let scan = reader
                .IndexPlan
                .as_deref()
                .and_then(find_index_scan)
                .ok_or_else(|| {
                    BuildError::new("typed IndexReader requires one PhysicalIndexScan")
                })?;
            let table = scan
                .Table
                .as_ref()
                .ok_or_else(|| BuildError::new("PhysicalIndexScan has no TableInfo"))?;
            (
                if scan.PhysicalTableID != 0 {
                    scan.PhysicalTableID
                } else {
                    table.ID
                },
                table.PKIsHandle,
                scan.Desc,
                scan.Columns.clone(),
            )
        };
        self.BindTypedScan(TypedScanSpec {
            table_id,
            pk_is_handle,
            descending,
            columns,
            ranges: ranges.clone(),
            version,
            initial_capacity,
            maximum_chunk_size,
        });
        let leaf_ranges = if scans.len() <= 1 {
            vec![(table_id, ranges.clone())]
        } else {
            scans
                .iter()
                .map(|scan| {
                    if !scan.AccessCondition.is_empty() {
                        return Err(BuildError::new(
                            "typed multi-scan binding requires canonical full record scans",
                        ));
                    }
                    let table = scan
                        .Table
                        .as_ref()
                        .ok_or_else(|| BuildError::new("PhysicalTableScan has no TableInfo"))?;
                    let table_id = if scan.PhysicalTableID != 0 {
                        scan.PhysicalTableID
                    } else {
                        table.ID
                    };
                    let start = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table_id).0);
                    Ok((
                        table_id,
                        vec![KeyRange {
                            end: start.PrefixNext(),
                            start,
                        }],
                    ))
                })
                .collect::<Result<Vec<_>, BuildError>>()?
        };
        *self.physical_scan.borrow_mut() = Some(PhysicalScanBinding {
            plan,
            ranges,
            leaf_ranges,
            version,
            initial_capacity,
            maximum_chunk_size,
        });
        Ok(())
    }

    pub fn CurrentDatabase(&self) -> String {
        self.session.WithSessionVars(|vars| vars.CurrentDB())
    }

    pub fn KillSignal(&self) -> Result<(), astersql_errors::SharedError> {
        self.session.SQLKiller().HandleSignal()
    }

    pub fn OpenTypedPhysicalTableScan(
        &self,
        scan: &dyn PhysicalPlan,
        ranges: Vec<KeyRange>,
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<ExecutorBox, astersql_executor::builder::BuildError> {
        self.session.OpenTypedPhysicalPlan(
            scan,
            ranges,
            version,
            initial_capacity,
            maximum_chunk_size,
        )
    }

    pub fn OpenTypedPhysicalSelectLockPlan(
        &self,
        plan: &dyn PhysicalPlan,
        ranges: Vec<KeyRange>,
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<ExecutorBox, astersql_executor::builder::BuildError> {
        self.session.OpenTypedPhysicalSelectLockPlan(
            plan,
            ranges,
            version,
            initial_capacity,
            maximum_chunk_size,
        )
    }

    pub fn OpenTypedKVSnapshotScan(
        &self,
        table_id: i64,
        pk_is_handle: bool,
        descending: bool,
        columns: Vec<ColumnInfo>,
        ranges: Vec<KeyRange>,
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> ExecutorBox {
        self.session.OpenTypedKVSnapshotScan(
            table_id,
            pk_is_handle,
            descending,
            columns,
            ranges,
            version,
            initial_capacity,
            maximum_chunk_size,
        )
    }
}

impl OwnedKVSnapshotSource {
    pub fn new(domain: Arc<astersql_domain::Domain>, version: kv::Version) -> Self {
        Self { domain, version }
    }
}

impl kv::Getter for OwnedKVSnapshotSource {
    fn Get(
        &self,
        context: &kv::context::Context,
        key: kv::Key,
        options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, astersql_errors::SharedError> {
        self.domain
            .storage()
            .with_storage(|storage| storage.GetSnapshot(self.version).Get(context, key, options))
    }
}

impl kv::Retriever for OwnedKVSnapshotSource {
    fn Iter(
        &self,
        start: kv::Key,
        end: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, astersql_errors::SharedError> {
        self.domain
            .storage()
            .with_storage(|storage| storage.GetSnapshot(self.version).Iter(start, end))
    }

    fn IterReverse(
        &self,
        start: Option<kv::Key>,
        end: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, astersql_errors::SharedError> {
        self.domain
            .storage()
            .with_storage(|storage| storage.GetSnapshot(self.version).IterReverse(start, end))
    }
}

impl ConcreteSession {
    pub fn AdapterPlanContext(&self) -> astersql_planner_core_base::ContextRef {
        super::planning::plan_context_with_params(Arc::clone(&self.session_vars), &[], false)
    }
    /// Build the typed executor directly from a canonical optimized scan node.
    pub fn OpenTypedPhysicalTableScan(
        &self,
        scan: &PhysicalTableScan,
        ranges: Vec<KeyRange>,
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<ExecutorBox, astersql_executor::builder::BuildError> {
        let source = Arc::new(OwnedKVSnapshotSource::new(
            Arc::clone(&self.domain),
            version,
        ));
        BuildTypedPhysicalPlan(scan, source, ranges, initial_capacity, maximum_chunk_size)
    }

    pub fn OpenTypedPhysicalPlan(
        &self,
        plan: &dyn PhysicalPlan,
        ranges: Vec<KeyRange>,
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<ExecutorBox, astersql_executor::builder::BuildError> {
        let source = Arc::new(OwnedKVSnapshotSource::new(
            Arc::clone(&self.domain),
            version,
        ));
        BuildTypedPhysicalPlan(plan, source, ranges, initial_capacity, maximum_chunk_size)
    }

    pub fn OpenTypedPhysicalPlanWithBindings(
        &self,
        plan: &dyn PhysicalPlan,
        leaf_ranges: &[(i64, Vec<KeyRange>)],
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<ExecutorBox, astersql_executor::builder::BuildError> {
        let bindings = leaf_ranges
            .iter()
            .map(|(table_id, ranges)| TypedScanBinding {
                table_id: *table_id,
                retriever: Arc::new(OwnedKVSnapshotSource::new(
                    Arc::clone(&self.domain),
                    version,
                )),
                ranges: ranges.clone(),
            })
            .collect();
        BuildTypedPhysicalPlanWithBindings(plan, bindings, initial_capacity, maximum_chunk_size)
    }

    pub fn OpenTypedPhysicalSelectLockPlan(
        &self,
        plan: &dyn PhysicalPlan,
        ranges: Vec<KeyRange>,
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> Result<ExecutorBox, astersql_executor::builder::BuildError> {
        let source = Arc::new(OwnedKVSnapshotSource::new(
            Arc::clone(&self.domain),
            version,
        ));
        BuildTypedPhysicalSelectLockPlan(plan, source, ranges, initial_capacity, maximum_chunk_size)
    }

    /// Open an owned typed table scan against this session's canonical Domain.
    /// The caller supplies the optimized scan's encoded ranges and columns;
    /// the session pins the MVCC version and hands the result to the executor.
    pub fn OpenTypedKVSnapshotScan(
        &self,
        table_id: i64,
        pk_is_handle: bool,
        descending: bool,
        columns: Vec<ColumnInfo>,
        ranges: Vec<KeyRange>,
        version: kv::Version,
        initial_capacity: usize,
        maximum_chunk_size: usize,
    ) -> ExecutorBox {
        let source = Arc::new(OwnedKVSnapshotSource::new(
            Arc::clone(&self.domain),
            version,
        ));
        Box::new(TypedKVScan::new(
            source,
            table_id,
            pk_is_handle,
            descending,
            columns,
            ranges,
            initial_capacity,
            maximum_chunk_size,
        ))
    }
}
