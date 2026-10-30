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

use super::session::RuntimeTimeZone;
use super::*;
use astersql_executor::adapter::{ExecExecutor, SchemaColumn};
use astersql_executor::builder::{BuildError, ExecutorBox};
use astersql_executor::mview_complete_delta_apply::{MutationBatch, MutationRuntime};
use astersql_executor::mview_complete_delta_apply_builder::{TargetColumn, TargetColumns};
use astersql_executor::mview_delta_merge_builder::{LookupRequest, ReaderSnapshot, Resources};
use astersql_planner_core::mview_refresh_builder::*;
use astersql_types::datum::Datum;

fn identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}
fn datum_literal(value: &Datum) -> Result<String, String> {
    use astersql_types::datum::*;
    Ok(match value.Kind() {
        KindNull => "NULL".into(),
        KindInt64 => value.GetInt64().to_string(),
        KindUint64 => value.GetUint64().to_string(),
        KindFloat32 => value.GetFloat32().to_string(),
        KindFloat64 => value.GetFloat64().to_string(),
        KindMysqlDecimal => value.GetMysqlDecimal().String(),
        KindBytes | KindString => format!(
            "X'{}'",
            value
                .GetBytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
        _ => format!(
            "'{}'",
            value
                .ToString()
                .map_err(|e| e.to_string())?
                .replace('\\', "\\\\")
                .replace('\'', "''")
        ),
    })
}
fn literal_expression(value: &Datum) -> Result<ast::ExprNode, String> {
    let sql = format!("SELECT {}", datum_literal(value)?);
    let node = astersql_parser::New()
        .ParseOneStmt(&sql, "", "")
        .map_err(|e| e.to_string())?;
    Ok(node
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .unwrap()
        .Fields
        .Fields[0]
        .Expr
        .clone()
        .unwrap())
}
fn parse_select(sql: &str) -> SessionResult<Box<ast::SelectStmt>> {
    let node = astersql_parser::New()
        .ParseOneStmt(sql, "", "")
        .map_err(|e| session_error("parse materialized view definition", e))?;
    node.into_any()
        .downcast::<ast::SelectStmt>()
        .map_err(|_| SessionError::new("materialized view definition is not SELECT"))
}

pub(super) fn definition(
    session: &ConcreteSession,
    database: &str,
    table: &astersql_meta_model::TableInfo,
) -> SessionResult<(
    MViewDefinition,
    Box<ast::SelectStmt>,
    astersql_meta_model::TableInfo,
)> {
    let info = table
        .MaterializedView
        .as_ref()
        .ok_or_else(|| SessionError::new("target is not a materialized view"))?;
    if info.BaseTableIDs.len() != 1 {
        return Err(SessionError::new(
            "materialized view refresh requires exactly one base table",
        ));
    }
    let base = session
        .domain
        .info_schema()
        .TableByID(info.BaseTableIDs[0])
        .ok_or_else(|| SessionError::new("materialized view base table is missing"))?
        .ModelMeta()
        .map_err(|e| session_error("resolve refresh base table", e))?;
    let log_id = base
        .MaterializedViewBase
        .as_ref()
        .map_or(0, |base| base.MLogID);
    let log = session
        .domain
        .info_schema()
        .TableByID(log_id)
        .map(|table| table.ModelMeta())
        .transpose()
        .map_err(|e| session_error("resolve refresh MLog", e))?;
    let select = parse_select(&info.SQLContent)?;
    if select.Fields.Fields.len() != table.Columns.len() {
        return Err(SessionError::new(
            "materialized view definition/output column count mismatch",
        ));
    }
    let mut keys = Vec::new();
    let mut groups = Vec::new();
    let mut aggregates = Vec::new();
    for (offset, field) in select.Fields.Fields.iter().enumerate() {
        let expression = field
            .Expr
            .as_ref()
            .ok_or_else(|| SessionError::new("materialized view SELECT field is missing"))?;
        match &expression.Kind {
            ast::ExprKind::Column(column) => {
                keys.push(offset);
                groups.push(column.Name.L.clone());
            }
            ast::ExprKind::AggregateFunction {
                Name,
                Args,
                Distinct,
                Filter,
                ..
            } => {
                if *Distinct || Filter.is_some() {
                    return Err(SessionError::new(
                        "materialized view aggregate DISTINCT/FILTER is unsupported",
                    ));
                }
                let name = Name.to_ascii_lowercase();
                let star=name=="count" && Args.first().is_some_and(|arg| matches!(&arg.Kind,ast::ExprKind::Value(value) if !matches!(value.Datum,ast::ValueDatum::Null)));
                let kind = match name.as_str() {
                    "count" if star => astersql_planner_core::MViewAggKind::CountStar,
                    "count" => astersql_planner_core::MViewAggKind::Count,
                    "sum" => astersql_planner_core::MViewAggKind::Sum,
                    "min" => astersql_planner_core::MViewAggKind::Min,
                    "max" => astersql_planner_core::MViewAggKind::Max,
                    _ => {
                        return Err(SessionError::new(format!(
                            "unsupported materialized view aggregate {name}"
                        )));
                    }
                };
                let argument = if star {
                    String::new()
                } else {
                    Args.first()
                        .map(|arg| match &arg.Kind {
                            ast::ExprKind::Column(column) => Ok(column.Name.L.clone()),
                            _ => ast::sql_restore::restore_expr(arg),
                        })
                        .transpose()
                        .map_err(SessionError::new)?
                        .ok_or_else(|| SessionError::new("aggregate argument is missing"))?
                };
                aggregates.push(MViewAggregate {
                    kind,
                    mv_offset: offset,
                    argument,
                });
            }
            _ => {
                return Err(SessionError::new(
                    "materialized view field must be a group column or aggregate",
                ));
            }
        }
    }
    let primary = table.Indices.iter().find(|index| index.Primary);
    let handle = if table.PKIsHandle {
        MViewHandleKind::IntPrimaryKey {
            column_offset: table
                .Columns
                .iter()
                .position(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
                .ok_or_else(|| SessionError::new("materialized view primary key column missing"))?,
        }
    } else if table.IsCommonHandle {
        MViewHandleKind::CommonPrimaryKey
    } else {
        MViewHandleKind::ExtraRowId
    };
    let definition = MViewDefinition {
        database: database.into(),
        mv_table_id: table.ID,
        base_table_id: base.ID,
        mlog_table_id: log_id,
        mv_columns: table
            .Columns
            .iter()
            .map(|column| MViewColumn {
                name: column.Name.O.clone(),
                not_null: astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()),
            })
            .collect(),
        base_columns: base
            .Columns
            .iter()
            .map(|column| MViewColumn {
                name: column.Name.L.clone(),
                not_null: astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()),
            })
            .collect(),
        mlog_columns: log.as_ref().map_or_else(Vec::new, |log| {
            log.Columns
                .iter()
                .map(|column| column.Name.L.clone())
                .collect()
        }),
        group_key_mv_offsets: keys,
        group_key_base_columns: groups,
        aggregates,
        handle_kind: handle,
        common_handle_offsets: primary.map_or_else(Vec::new, |index| {
            index
                .Columns
                .iter()
                .map(|column| column.Offset as usize)
                .collect()
        }),
        base_indexes: base
            .Indices
            .iter()
            .map(|index| astersql_planner_core::MViewIndex {
                name: index.Name.O.clone(),
                columns: index
                    .Columns
                    .iter()
                    .map(|column| column.Name.L.clone())
                    .collect(),
                visible: !index.Invisible && index.State == astersql_meta_model::StatePublic,
            })
            .collect(),
    };
    Ok((definition, select, (*base).clone()))
}

impl ConcreteSession {
    pub(super) fn compile_mview_source(
        &self,
        select: ast::SelectStmt,
        read_ts: u64,
    ) -> Result<ExecutorBox, BuildError> {
        self.compile_mview_source_with_stats(select, read_ts, None)
    }

    pub(super) fn compile_mview_source_with_stats(
        &self,
        select: ast::SelectStmt,
        read_ts: u64,
        stats: Option<&astersql_executor::builder::TypedRuntimeStats>,
    ) -> Result<ExecutorBox, BuildError> {
        let mut variables = astersql_sessionctx_variable::session::SessionVars::default();
        variables.EnableFullOuterJoin = true;
        variables.InRestrictedSQL = true;
        variables.SetCurrentDB(self.current_database());
        for name in [
            "sql_mode",
            "time_zone",
            "div_precision_increment",
            "tidb_isolation_read_engines",
            "tidb_mem_quota_query",
        ] {
            if let Some(value) = self.session_vars.GetSystemVar(name) {
                variables
                    .SetHintSystemVarWithOldState(name, &value)
                    .map_err(|e| BuildError::new(e.to_string()))?;
            }
        }
        let sql_mode = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .map_err(|error| BuildError::new(error.to_string()))?;
        let location = *self.time_zone.borrow();
        let context = planning::mview_plan_context(Arc::new(variables), sql_mode, location);
        let node = ast::NodeRef::new(Box::new(select));
        let (mut builder, _) = astersql_planner_core::NewPlanBuilder().Init(
            context.clone(),
            self.domain.info_schema(),
            astersql_util_hint::NewQBHintHandler(None),
        );
        let mut logical = builder
            .buildResultSetNode(astersql_planner_core::context::TODO(), &node, false)
            .map_err(|e| BuildError::new(e.to_string()))?;
        let (mut physical, _) = astersql_planner_core::DoOptimize(
            astersql_planner_core::context::TODO(),
            &context,
            builder.GetOptFlag(),
            &mut logical,
        )
        .map_err(|e| BuildError::new(e.to_string()))?;
        physical
            .resolve_indices()
            .map_err(|error| BuildError::new(error.to_string()))?;
        let mut scans = Vec::new();
        fn collect(
            plan: &dyn astersql_planner_core_base::PhysicalPlan,
            scans: &mut Vec<(i64, Vec<astersql_executor::typed_kv_scan::KeyRange>)>,
            indexes: &mut HashMap<i64, i64>,
            plan_rows: &mut Vec<Vec<String>>,
        ) -> Result<(), BuildError> {
            plan_rows.push(vec![
                plan.explain_id(&[]).to_string(),
                plan.stats_info().RowCount.to_string(),
                "root".into(),
                plan.explain_info(),
            ]);
            if let Some(scan) =
                plan.as_any()
                    .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalTableScan>()
            {
                if let Some(table) = scan.Table.as_ref() {
                    let start = kv::Key(astersql_tablecodec::GenTableRecordPrefix(table.ID).0);
                    let id = if scan.PhysicalTableID != 0 {
                        scan.PhysicalTableID
                    } else {
                        table.ID
                    };
                    let ranges = if table.PKIsHandle && !scan.IsCommonHandle {
                        super::typed_adapter_bridge::encode_table_record_ranges(scan, id)?
                    } else {
                        vec![astersql_executor::typed_kv_scan::KeyRange {
                            end: start.PrefixNext(),
                            start,
                        }]
                    };
                    scans.push((id, ranges));
                }
                return Ok(());
            }
            if let Some(reader) = plan
                .as_any()
                .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalTableReader>(
            ) {
                if let Some(child) = reader.GetTablePlan() {
                    collect(child, scans, indexes, plan_rows)?;
                }
                return Ok(());
            }
            if let Some(reader) = plan
                .as_any()
                .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexReader>(
            ) {
                if let Some(child) = reader.IndexPlan.as_deref() {
                    collect(child, scans, indexes, plan_rows)?;
                }
                return Ok(());
            }
            if let Some(reader) = plan.as_any().downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexLookUpReader>() {
                if let Some(child) = reader.IndexPlan.as_deref() { collect(child, scans, indexes, plan_rows)?; }
                if let Some(child) = reader.TablePlan.as_deref() { collect(child, scans, indexes, plan_rows)?; }
                return Ok(());
            }
            if let Some(scan) =
                plan.as_any()
                    .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexScan>()
            {
                if let (Some(table), Some(index)) = (scan.Table.as_ref(), scan.Index.as_ref()) {
                    indexes.insert(table.ID, index.ID);
                    let id = if scan.PhysicalTableID != 0 {
                        scan.PhysicalTableID
                    } else {
                        table.ID
                    };
                    let ranges = if table.IsCommonHandle && index.Primary {
                        super::typed_adapter_bridge::encode_common_handle_ranges(scan, id)?
                    } else {
                        super::typed_adapter_bridge::encode_index_ranges(scan, id)?
                    };
                    scans.push((id, ranges));
                }
                return Ok(());
            }
            for child in plan.children() {
                collect(child, scans, indexes, plan_rows)?;
            }
            Ok(())
        }
        let mut indexes = HashMap::new();
        let mut plan_rows = Vec::new();
        collect(physical.as_ref(), &mut scans, &mut indexes, &mut plan_rows)?;
        *self.mview_plan_rows.borrow_mut() = plan_rows;
        let bindings = scans
            .into_iter()
            .map(|(id, ranges)| {
                let count = self
                    .mview_scan
                    .borrow()
                    .as_ref()
                    .filter(|(table, _)| *table == id)
                    .map(|(_, count)| count.clone());
                let index = indexes.get(&id).copied().and_then(|index| {
                    self.internal_collectors
                        .borrow()
                        .indexes
                        .clone()
                        .map(|collector| {
                            Arc::new(IndexObservation {
                                table: id,
                                index,
                                collector,
                                rows: AtomicU64::new(0),
                                requests: AtomicU64::new(0),
                            })
                        })
                });
                astersql_executor::builder::TypedScanBinding {
                    table_id: id,
                    ranges,
                    retriever: Arc::new(ObservedSource {
                        inner: super::typed_adapter_bridge::OwnedKVSnapshotSource::new(
                            self.domain.clone(),
                            kv::Version { Ver: read_ts },
                        ),
                        count,
                        index,
                    }),
                }
            })
            .collect();
        astersql_executor::builder::BuildTypedPhysicalPlanWithRuntimeStats(
            physical.as_ref(),
            bindings,
            32,
            1024,
            stats,
        )
    }
}

/// Count consumed KV rows at the actual snapshot iterator, before SQL
/// filtering/aggregation. This is the MLog scan count, not delta output rows.
struct ObservedSource {
    inner: super::typed_adapter_bridge::OwnedKVSnapshotSource,
    count: Option<Arc<AtomicU64>>,
    index: Option<Arc<IndexObservation>>,
}
struct IndexObservation {
    table: i64,
    index: i64,
    collector: Arc<astersql_statistics_handle_usage::SessionIndexUsageCollector>,
    rows: AtomicU64,
    requests: AtomicU64,
}
impl Drop for IndexObservation {
    fn drop(&mut self) {
        let requests = self.requests.load(Ordering::Relaxed);
        if requests > 0 {
            self.collector.update(
                self.table,
                self.index,
                astersql_statistics_handle_usage::IndexUsageSample {
                    last_used_at: SystemTime::now(),
                    query_total: 1,
                    kv_req_total: requests,
                    row_access_total: self.rows.load(Ordering::Relaxed),
                    ..Default::default()
                },
            );
        }
    }
}
impl kv::Getter for ObservedSource {
    fn Get(
        &self,
        context: &kv::Context,
        key: kv::Key,
        options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, astersql_errors::SharedError> {
        kv::Getter::Get(&self.inner, context, key, options)
    }
}
impl ObservedSource {
    fn observe(&self, inner: Box<dyn kv::Iterator>) -> Box<dyn kv::Iterator> {
        if let Some(index) = &self.index {
            index.requests.fetch_add(1, Ordering::Relaxed);
        }
        let mut iterator = ObservedIterator {
            inner,
            count: self.count.clone(),
            index: self.index.clone(),
            closed: false,
        };
        iterator.record();
        Box::new(iterator)
    }
}
impl kv::Retriever for ObservedSource {
    fn Iter(
        &self,
        start: kv::Key,
        end: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, astersql_errors::SharedError> {
        Ok(self.observe(kv::Retriever::Iter(&self.inner, start, end)?))
    }
    fn IterReverse(
        &self,
        start: Option<kv::Key>,
        end: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, astersql_errors::SharedError> {
        Ok(self.observe(kv::Retriever::IterReverse(&self.inner, start, end)?))
    }
}
struct ObservedIterator {
    inner: Box<dyn kv::Iterator>,
    count: Option<Arc<AtomicU64>>,
    index: Option<Arc<IndexObservation>>,
    closed: bool,
}
impl ObservedIterator {
    fn record(&mut self) {
        if self.inner.Valid() {
            if let Some(index) = &self.index {
                index.rows.fetch_add(1, Ordering::Relaxed);
            }
            if let Some(count) = &self.count {
                count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.inner.Close();
    }
}
impl kv::Iterator for ObservedIterator {
    fn Valid(&self) -> bool {
        self.inner.Valid()
    }
    fn Key(&self) -> kv::Key {
        self.inner.Key()
    }
    fn Value(&self) -> Vec<u8> {
        self.inner.Value()
    }
    fn Next(&mut self) -> Result<(), astersql_errors::SharedError> {
        self.inner.Next()?;
        self.record();
        Ok(())
    }
    fn Close(&mut self) {
        self.close();
    }
}
impl Drop for ObservedIterator {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) struct RefreshResources {
    pub session: ConcreteSession,
    pub database: String,
    pub table: astersql_meta_model::TableInfo,
    pub definition: MViewDefinition,
    pub handles: Vec<usize>,
    pub read_ts: u64,
    pub statement: Arc<astersql_sessionctx_stmtctx::StatementContext>,
}
struct Batch<'a>(&'a RefreshResources);
impl RefreshResources {
    fn execute_write(&self, statement: &str) -> SessionResult<()> {
        super::mview_refresh::sql(&self.session, statement)
    }
    fn target(&self) -> String {
        format!(
            "{}.{}",
            identifier(&self.database),
            identifier(&self.table.Name.O)
        )
    }
    fn predicate(&self, source: &[Datum], old: &[Datum]) -> Result<String, String> {
        if !self.table.PKIsHandle && !self.table.IsCommonHandle {
            return Ok(format!(
                "_tidb_rowid={}",
                datum_literal(
                    source
                        .get(
                            *self
                                .handles
                                .first()
                                .ok_or("refresh handle mapping is missing")?
                        )
                        .ok_or("refresh row handle missing")?
                )?
            ));
        }
        let offsets = if self.table.PKIsHandle {
            vec![
                self.table
                    .Columns
                    .iter()
                    .position(|column| {
                        astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag())
                    })
                    .ok_or("refresh primary key missing")?,
            ]
        } else {
            self.definition.common_handle_offsets.clone()
        };
        offsets
            .into_iter()
            .map(|offset| {
                Ok(format!(
                    "{}<=>{}",
                    identifier(&self.table.Columns[offset].Name.O),
                    datum_literal(&old[offset])?
                ))
            })
            .collect::<Result<Vec<_>, String>>()
            .map(|parts| parts.join(" AND "))
    }
}
impl MutationBatch for Batch<'_> {
    fn insert(&mut self, row: &[Datum]) -> Result<(), String> {
        let names = self
            .0
            .table
            .Columns
            .iter()
            .map(|column| identifier(&column.Name.O))
            .collect::<Vec<_>>()
            .join(",");
        let values = row
            .iter()
            .map(datum_literal)
            .collect::<Result<Vec<_>, _>>()?
            .join(",");
        self.0
            .execute_write(&format!(
                "INSERT INTO {} ({names}) VALUES ({values})",
                self.0.target()
            ))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn delete(&mut self, row: &[Datum]) -> Result<(), String> {
        self.delete_from_source(row, row)
    }
    fn update(&mut self, old: &[Datum], new: &[Datum], touched: &[bool]) -> Result<(), String> {
        self.update_from_source(old, old, new, touched)
    }
    fn delete_from_source(&mut self, source: &[Datum], old: &[Datum]) -> Result<(), String> {
        self.0
            .execute_write(&format!(
                "DELETE FROM {} WHERE {}",
                self.0.target(),
                self.0.predicate(source, old)?
            ))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn update_from_source(
        &mut self,
        source: &[Datum],
        old: &[Datum],
        new: &[Datum],
        touched: &[bool],
    ) -> Result<(), String> {
        let assignments = touched
            .iter()
            .enumerate()
            .filter(|(_, touched)| **touched)
            .map(|(offset, _)| {
                Ok(format!(
                    "{}={}",
                    identifier(&self.0.table.Columns[offset].Name.O),
                    datum_literal(&new[offset])?
                ))
            })
            .collect::<Result<Vec<_>, String>>()?
            .join(",");
        self.0
            .execute_write(&format!(
                "UPDATE {} SET {assignments} WHERE {}",
                self.0.target(),
                self.0.predicate(source, old)?
            ))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
impl MutationRuntime for RefreshResources {
    fn run_atomic(
        &self,
        apply: &mut dyn FnMut(&mut dyn MutationBatch) -> Result<(), String>,
    ) -> Result<(), String> {
        let (stage, deltas) = {
            let mut state = self.session.state.borrow_mut();
            let tx = state
                .transaction
                .as_mut()
                .ok_or("refresh writer requires an active transaction")?;
            let stage = tx.StageStatement().map_err(|e| e.to_string())?;
            (stage, state.pending_stats_deltas.clone())
        };
        let result = apply(&mut Batch(self));
        let mut state = self.session.state.borrow_mut();
        let tx = state
            .transaction
            .as_mut()
            .ok_or("refresh transaction disappeared")?;
        if result.is_ok() {
            tx.ReleaseStatement(stage).map_err(|e| e.to_string())?;
        } else {
            tx.CleanupStatement(stage).map_err(|e| e.to_string())?;
            state.pending_stats_deltas = deltas;
        }
        result
    }
}
impl Resources for RefreshResources {
    fn refresh_context(&self) -> Option<(i32, Arc<astersql_sessionctx_stmtctx::StatementContext>)> {
        Some((1, self.statement.clone()))
    }
    fn target_field_types(&self) -> Vec<astersql_parser_types::FieldType> {
        self.table
            .Columns
            .iter()
            .map(|column| column.FieldType.clone())
            .collect()
    }
    fn complete_delta_target_columns(&self) -> Result<TargetColumns, BuildError> {
        Ok(TargetColumns {
            public_count: self.table.Columns.len(),
            writable: self
                .table
                .Columns
                .iter()
                .map(|column| {
                    Some(TargetColumn {
                        name: column.Name.O.clone(),
                        offset: column.Offset as isize,
                        public: column.State == astersql_meta_model::StatePublic,
                        field_type: column.FieldType.clone(),
                    })
                })
                .collect(),
        })
    }
    fn read_full_update(
        &self,
        _: &astersql_planner_core::PlanNode,
        snapshot: &ReaderSnapshot,
        request: &LookupRequest,
    ) -> Result<ExecutorBox, BuildError> {
        if snapshot.ts != self.read_ts {
            return Err(BuildError::new("MIN/MAX lookup snapshot changed"));
        }
        let (_, select, base) = definition(&self.session, &self.database, &self.table)
            .map_err(|e| BuildError::new(e.to_string()))?;
        let keys = request
            .keys
            .iter()
            .map(|key| {
                key.iter()
                    .map(literal_expression)
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(BuildError::new)?;
        let source = astersql_planner_core::mview_refresh_source::BuildBoundMinMaxLookup(
            &self.definition,
            &base.Name.O,
            &select,
            &keys,
        )
        .map_err(BuildError::new)?;
        self.session.compile_mview_source(source, self.read_ts)
    }
}
