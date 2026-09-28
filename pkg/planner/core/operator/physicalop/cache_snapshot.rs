// Copyright 2026 AsterSQL.

//! Thread-safe, context-free snapshots of fields shared by physical plans.

use base::{ContextRef, PhysicalPlan as _};
use baseimpl::{NewBasePlan, Plan};

use crate::physical_index_hash_join::PhysicalIndexHashJoin;
use crate::{
    AggMppRunMode, BasePhysicalAgg, BasePhysicalJoin, BasePhysicalPlan, BatchPointGetPlan, Delete,
    DeleteIndexLayout, DeleteIndexRowLayout, Insert, LegacyPhysicalLock, PhysicalHashAgg,
    PhysicalHashJoin, PhysicalIndexJoin, PhysicalIndexLookUpReader, PhysicalIndexMergeReader,
    PhysicalIndexReader, PhysicalIndexScan, PhysicalLimit, PhysicalMergeJoin, PhysicalProjection,
    PhysicalSchemaProducer, PhysicalSelection, PhysicalStreamAgg, PhysicalTableDual,
    PhysicalTableReader, PhysicalTableScan, PhysicalTopN, PhysicalUnionAll, PhysicalUnionScan,
    PointGetPlan, ReadReqType, SimpleSchemaProducer, TblColPosInfo, Update,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheSnapshotError(String);

impl std::fmt::Display for CacheSnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CacheSnapshotError {}

fn unsupported(message: impl Into<String>) -> CacheSnapshotError {
    CacheSnapshotError(message.into())
}

/// Context-free form of `baseimpl::Plan`.
#[derive(Clone)]
pub struct CachedContext {
    plan_stats: Option<property::StatsInfo>,
    tp: String,
    id: i32,
    query_block_offset: i32,
    noncacheable_reason: String,
}

impl CachedContext {
    fn capture(plan: &Plan) -> Self {
        Self {
            plan_stats: plan.StatsInfo().cloned(),
            tp: plan.TP(&[]),
            id: plan.ID(),
            query_block_offset: plan.QueryBlockOffset(),
            noncacheable_reason: plan.GetNoncacheableReason(),
        }
    }

    fn restore(&self, context: ContextRef) -> Plan {
        let mut plan = NewBasePlan(context, self.tp.clone(), self.query_block_offset);
        plan.SetID(self.id);
        plan.SetStats(self.plan_stats.clone().map(std::sync::Arc::new));
        plan.SetNoncacheableReason(self.noncacheable_reason.clone());
        plan
    }
}

#[derive(Clone)]
struct CachedSortItem {
    column: expression::CachedColumn,
    desc: bool,
}

#[derive(Clone)]
struct CachedMppColumn {
    column: expression::CachedColumn,
    collate_id: i32,
}

#[derive(Clone)]
struct CachedVectorSearch {
    distance_fn_name: String,
    fn_pb_code: tipb::ScalarFuncSig,
    vector: std::sync::Arc<expression::types::VectorFloat32>,
    column: expression::CachedColumn,
}

#[derive(Clone)]
struct CachedIndexJoinProperty {
    other_conditions: Vec<expression::CachedExpression>,
    outer_join_keys: Vec<expression::CachedColumn>,
    inner_join_keys: Vec<expression::CachedColumn>,
    average_inner_row_count: f64,
    table_range_scan: bool,
}

/// Owned required-property snapshot. The hash cache is intentionally rebuilt lazily.
#[derive(Clone)]
pub struct CachedPhysicalProperty {
    sort_items: Vec<CachedSortItem>,
    task_type: property::TaskType,
    expected_count: f64,
    can_add_enforcer: bool,
    mpp_partition_columns: Vec<CachedMppColumn>,
    mpp_partition_type: property::MPPPartitionType,
    partition_sort_items: Vec<CachedSortItem>,
    cte_producer_status: property::cteProducerStatus,
    vector_search: Option<CachedVectorSearch>,
    vector_top_k: u32,
    index_join: Option<CachedIndexJoinProperty>,
    no_cop_push_down: bool,
    partial_order: Option<Vec<CachedSortItem>>,
    advisory_sort_items: Vec<CachedSortItem>,
    prefer_tiflash: bool,
}

impl CachedPhysicalProperty {
    fn capture(value: &property::PhysicalProperty) -> Result<Self, CacheSnapshotError> {
        let sort_items = |items: &[property::SortItem]| {
            items
                .iter()
                .map(|item| {
                    Ok(CachedSortItem {
                        column: expression::CachedColumn::try_from_column(&item.Col)
                            .map_err(|error| unsupported(error.to_string()))?,
                        desc: item.Desc,
                    })
                })
                .collect::<Result<Vec<_>, CacheSnapshotError>>()
        };
        let columns = |items: &[expression::Column]| {
            items
                .iter()
                .map(|column| {
                    expression::CachedColumn::try_from_column(column)
                        .map_err(|error| unsupported(error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(Self {
            sort_items: sort_items(&value.SortItems)?,
            task_type: value.TaskTp,
            expected_count: value.ExpectedCnt,
            can_add_enforcer: value.CanAddEnforcer,
            mpp_partition_columns: value
                .MPPPartitionCols
                .iter()
                .map(|column| {
                    Ok(CachedMppColumn {
                        column: expression::CachedColumn::try_from_column(&column.Col)
                            .map_err(|error| unsupported(error.to_string()))?,
                        collate_id: column.CollateID,
                    })
                })
                .collect::<Result<_, CacheSnapshotError>>()?,
            mpp_partition_type: value.MPPPartitionTp,
            partition_sort_items: sort_items(&value.SortItemsForPartition)?,
            cte_producer_status: value.CTEProducerStatus,
            vector_search: value
                .VectorProp
                .VSInfo
                .as_ref()
                .map(|vector| {
                    Ok(CachedVectorSearch {
                        distance_fn_name: vector.DistanceFnName.clone(),
                        fn_pb_code: vector.FnPbCode,
                        vector: vector.Vec.clone(),
                        column: expression::CachedColumn::try_from_column(&vector.Column)
                            .map_err(|error| unsupported(error.to_string()))?,
                    })
                })
                .transpose()?,
            vector_top_k: value.VectorProp.TopK,
            index_join: value
                .IndexJoinProp
                .as_ref()
                .map(|join| {
                    Ok(CachedIndexJoinProperty {
                        other_conditions: join
                            .OtherConditions
                            .iter()
                            .map(|condition| {
                                expression::CachedExpression::try_from_expression(
                                    condition.as_ref(),
                                )
                                .map_err(|error| unsupported(error.to_string()))
                            })
                            .collect::<Result<_, _>>()?,
                        outer_join_keys: columns(&join.OuterJoinKeys)?,
                        inner_join_keys: columns(&join.InnerJoinKeys)?,
                        average_inner_row_count: join.AvgInnerRowCnt,
                        table_range_scan: join.TableRangeScan,
                    })
                })
                .transpose()?,
            no_cop_push_down: value.NoCopPushDown,
            partial_order: value
                .PartialOrderInfo
                .as_ref()
                .map(|partial| sort_items(&partial.SortItems))
                .transpose()?,
            advisory_sort_items: sort_items(&value.AdvisorySortItems)?,
            prefer_tiflash: value.PreferTiFlash,
        })
    }

    fn restore(
        &self,
        context: &dyn expression::BuildContext,
    ) -> Result<property::PhysicalProperty, CacheSnapshotError> {
        let sort_items = |items: &[CachedSortItem]| {
            items
                .iter()
                .map(|item| {
                    Ok(property::SortItem {
                        Col: item
                            .column
                            .restore_column(context)
                            .map_err(|error| unsupported(error.to_string()))?,
                        Desc: item.desc,
                    })
                })
                .collect::<Result<Vec<_>, CacheSnapshotError>>()
        };
        let columns = |items: &[expression::CachedColumn]| {
            items
                .iter()
                .map(|column| {
                    column
                        .restore_column(context)
                        .map_err(|error| unsupported(error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let mut restored = property::PhysicalProperty::default();
        restored.SortItems = sort_items(&self.sort_items)?;
        restored.TaskTp = self.task_type;
        restored.ExpectedCnt = self.expected_count;
        restored.CanAddEnforcer = self.can_add_enforcer;
        restored.MPPPartitionCols = self
            .mpp_partition_columns
            .iter()
            .map(|column| {
                Ok(property::MPPPartitionColumn {
                    Col: column
                        .column
                        .restore_column(context)
                        .map_err(|error| unsupported(error.to_string()))?,
                    CollateID: column.collate_id,
                })
            })
            .collect::<Result<_, CacheSnapshotError>>()?;
        restored.MPPPartitionTp = self.mpp_partition_type;
        restored.SortItemsForPartition = sort_items(&self.partition_sort_items)?;
        restored.CTEProducerStatus = self.cte_producer_status;
        restored.VectorProp = property::VectorProperty {
            VSInfo: self
                .vector_search
                .as_ref()
                .map(|vector| {
                    Ok(property::VectorSearchInfo {
                        DistanceFnName: vector.distance_fn_name.clone(),
                        FnPbCode: vector.fn_pb_code,
                        Vec: vector.vector.clone(),
                        Column: vector
                            .column
                            .restore_column(context)
                            .map_err(|error| unsupported(error.to_string()))?,
                    })
                })
                .transpose()?,
            TopK: self.vector_top_k,
        };
        restored.IndexJoinProp = self
            .index_join
            .as_ref()
            .map(|join| {
                Ok(property::IndexJoinRuntimeProp {
                    OtherConditions: join
                        .other_conditions
                        .iter()
                        .map(|condition| {
                            condition
                                .restore(context)
                                .map_err(|error| unsupported(error.to_string()))
                        })
                        .collect::<Result<_, _>>()?,
                    OuterJoinKeys: columns(&join.outer_join_keys)?,
                    InnerJoinKeys: columns(&join.inner_join_keys)?,
                    AvgInnerRowCnt: join.average_inner_row_count,
                    TableRangeScan: join.table_range_scan,
                })
            })
            .transpose()?;
        restored.NoCopPushDown = self.no_cop_push_down;
        restored.PartialOrderInfo = self
            .partial_order
            .as_ref()
            .map(|items| {
                sort_items(items).map(|SortItems| property::PartialOrderInfo { SortItems })
            })
            .transpose()?;
        restored.AdvisorySortItems = sort_items(&self.advisory_sort_items)?;
        restored.PreferTiFlash = self.prefer_tiflash;
        Ok(restored)
    }
}

/// Owned statistics snapshot. Histogram collections are immutable `Send + Sync` handles.
#[derive(Clone)]
pub struct CachedStats(property::StatsInfo);

/// Recursive base-only node used until concrete cached-plan variants are added.
#[derive(Clone)]
pub enum CachedPlan {
    Update(CachedUpdate),
    Delete(CachedDelete),
    Insert(CachedInsert),
    Base(CachedPlanBase),
    TableDual(CachedTableDual),
    TableScan(CachedTableScan),
    IndexScan(CachedIndexScan),
    PointGet(CachedPointGet),
    BatchPointGet(CachedBatchPointGet),
    Selection(CachedSelection),
    Projection(CachedProjection),
    TopN(CachedTopN),
    Limit(CachedLimit),
    SelectLock(CachedSelectLock),
    StreamAgg(CachedStreamAgg),
    HashAgg(CachedHashAgg),
    UnionAll(CachedUnionAll),
    UnionScan(CachedUnionScan),
    HashJoin(CachedHashJoin),
    MergeJoin(CachedMergeJoin),
    IndexJoin(CachedIndexJoin),
    IndexReader(CachedIndexReader),
    TableReader(CachedTableReader),
    IndexLookupReader(CachedIndexLookupReader),
    IndexMergeReader(CachedIndexMergeReader),
}

impl CachedPlan {
    pub fn try_capture_plan(plan: &dyn base::Plan) -> Result<Self, CacheSnapshotError> {
        if let Some(value) = plan.as_any().downcast_ref::<Update>() {
            return Ok(Self::Update(CachedUpdate::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<Delete>() {
            return Ok(Self::Delete(CachedDelete::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<Insert>() {
            return Ok(Self::Insert(CachedInsert::capture(value)?));
        }
        Err(unsupported(format!(
            "plan {} has no cached snapshot variant",
            plan.tp(&[])
        )))
    }

    pub fn try_capture(plan: &dyn base::PhysicalPlan) -> Result<Self, CacheSnapshotError> {
        if let Some(value) = plan.as_any().downcast_ref::<LegacyPhysicalLock>() {
            return Ok(Self::SelectLock(CachedSelectLock::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalIndexReader>() {
            return Ok(Self::IndexReader(CachedIndexReader::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalTableReader>() {
            return Ok(Self::TableReader(CachedTableReader::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalIndexLookUpReader>() {
            return Ok(Self::IndexLookupReader(CachedIndexLookupReader::capture(
                value,
            )?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalIndexMergeReader>() {
            return Ok(Self::IndexMergeReader(CachedIndexMergeReader::capture(
                value,
            )?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalHashJoin>() {
            return Ok(Self::HashJoin(CachedHashJoin::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalMergeJoin>() {
            return Ok(Self::MergeJoin(CachedMergeJoin::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalIndexJoin>() {
            return Ok(Self::IndexJoin(CachedIndexJoin::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalSelection>() {
            return Ok(Self::Selection(CachedSelection::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalProjection>() {
            return Ok(Self::Projection(CachedProjection::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalTopN>() {
            return Ok(Self::TopN(CachedTopN::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalLimit>() {
            return Ok(Self::Limit(CachedLimit::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalStreamAgg>() {
            return Ok(Self::StreamAgg(CachedStreamAgg::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalHashAgg>() {
            return Ok(Self::HashAgg(CachedHashAgg::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalUnionAll>() {
            return Ok(Self::UnionAll(CachedUnionAll::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalUnionScan>() {
            return Ok(Self::UnionScan(CachedUnionScan::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalTableDual>() {
            return Ok(Self::TableDual(CachedTableDual::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalTableScan>() {
            return Ok(Self::TableScan(CachedTableScan::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PhysicalIndexScan>() {
            return Ok(Self::IndexScan(CachedIndexScan::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<BatchPointGetPlan>() {
            return Ok(Self::BatchPointGet(CachedBatchPointGet::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<PointGetPlan>() {
            return Ok(Self::PointGet(CachedPointGet::capture(value)?));
        }
        if let Some(value) = plan.as_any().downcast_ref::<BasePhysicalPlan>() {
            return Ok(Self::Base(CachedPlanBase::try_from_base(value)?));
        }
        Err(unsupported(format!(
            "plan {} has no cached snapshot variant",
            plan.tp(&[])
        )))
    }

    pub fn restore(
        &self,
        context: ContextRef,
    ) -> Result<Box<dyn base::PhysicalPlan>, CacheSnapshotError> {
        match self {
            Self::Update(_) | Self::Delete(_) | Self::Insert(_) => Err(unsupported(
                "DML cached snapshots restore as plans, not physical child plans",
            )),
            Self::Base(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::TableDual(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::TableScan(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::IndexScan(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::PointGet(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::BatchPointGet(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::Selection(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::Projection(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::TopN(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::Limit(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::SelectLock(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::StreamAgg(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::HashAgg(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::UnionAll(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::UnionScan(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::HashJoin(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::MergeJoin(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::IndexJoin(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::IndexReader(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::TableReader(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::IndexLookupReader(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::IndexMergeReader(plan) => Ok(Box::new(plan.restore(context)?)),
        }
    }

    pub fn restore_plan(
        &self,
        context: ContextRef,
    ) -> Result<Box<dyn base::Plan>, CacheSnapshotError> {
        match self {
            Self::Update(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::Delete(plan) => Ok(Box::new(plan.restore(context)?)),
            Self::Insert(plan) => Ok(Box::new(plan.restore(context)?)),
            _ => Ok(self.restore(context)?),
        }
    }
}

#[derive(Clone)]
pub struct CachedSelectLock {
    producer: CachedSchemaProducer,
    lock_type: String,
    wait_seconds: u64,
}

impl CachedSelectLock {
    fn capture(value: &LegacyPhysicalLock) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            lock_type: value.LockType.clone(),
            wait_seconds: value.WaitSeconds,
        })
    }

    fn restore(&self, context: ContextRef) -> Result<LegacyPhysicalLock, CacheSnapshotError> {
        Ok(LegacyPhysicalLock {
            PhysicalSchemaProducer: self.producer.restore(context)?,
            LockType: self.lock_type.clone(),
            WaitSeconds: self.wait_seconds,
        })
    }
}

#[derive(Clone)]
struct CachedSimpleSchemaProducer {
    context: CachedContext,
    schema: Option<expression::CachedSchema>,
    names: Vec<Option<std::sync::Arc<types::metadata::FieldName>>>,
}

impl CachedSimpleSchemaProducer {
    fn capture(value: &SimpleSchemaProducer) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            context: CachedContext::capture(&value.Plan),
            schema: value
                .SchemaRef()
                .map(expression::CachedSchema::try_from_schema)
                .transpose()
                .map_err(|error| unsupported(error.to_string()))?,
            names: value.OutputNames().0,
        })
    }

    fn restore(&self, context: ContextRef) -> Result<SimpleSchemaProducer, CacheSnapshotError> {
        let mut producer = SimpleSchemaProducer::New(
            context.clone(),
            self.context.tp.clone(),
            self.context.query_block_offset,
        );
        producer.Plan = self.context.restore(context.clone());
        if let Some(schema) = &self.schema {
            producer.SetSchema(
                schema
                    .restore(context.GetExprCtx())
                    .map_err(|error| unsupported(error.to_string()))?,
            );
        }
        producer.SetOutputNames(types::metadata::NameSlice(self.names.clone()));
        Ok(producer)
    }
}

#[derive(Clone)]
struct CachedAssignment {
    column: expression::CachedColumn,
    column_name: parser_ast::CIStr,
    expression: expression::CachedExpression,
    lazy_error: Option<expression::Error>,
}

impl CachedAssignment {
    fn capture(value: &expression::Assignment) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            column: expression::CachedColumn::try_from_column(&value.Col)
                .map_err(|error| unsupported(error.to_string()))?,
            column_name: value.ColName.clone(),
            expression: expression::CachedExpression::try_from_expression(value.Expr.as_ref())
                .map_err(|error| unsupported(error.to_string()))?,
            lazy_error: value.LazyErr.clone(),
        })
    }

    fn restore(
        &self,
        context: &dyn expression::BuildContext,
    ) -> Result<expression::Assignment, CacheSnapshotError> {
        Ok(expression::Assignment {
            Col: self
                .column
                .restore_column(context)
                .map_err(|error| unsupported(error.to_string()))?,
            ColName: self.column_name.clone(),
            Expr: self
                .expression
                .restore(context)
                .map_err(|error| unsupported(error.to_string()))?,
            LazyErr: self.lazy_error.clone(),
        })
    }
}

fn capture_assignments(
    values: &[expression::Assignment],
) -> Result<Vec<CachedAssignment>, CacheSnapshotError> {
    values.iter().map(CachedAssignment::capture).collect()
}

fn restore_assignments(
    values: &[CachedAssignment],
    context: &dyn expression::BuildContext,
) -> Result<Vec<expression::Assignment>, CacheSnapshotError> {
    values.iter().map(|value| value.restore(context)).collect()
}

#[derive(Clone)]
pub struct CachedUpdate {
    producer: CachedSimpleSchemaProducer,
    ordered_list: Vec<CachedAssignment>,
    all_assignments_are_constant: bool,
    virtual_assignments_offset: usize,
    ignore_error: bool,
    select_plan: Box<CachedPlan>,
}

impl CachedUpdate {
    fn capture(value: &Update) -> Result<Self, CacheSnapshotError> {
        if value.HasForeignKeyPlans() {
            return Err(unsupported(
                "Update with foreign-key checks or cascades is not cacheable",
            ));
        }
        Ok(Self {
            producer: CachedSimpleSchemaProducer::capture(&value.SimpleSchemaProducer)?,
            ordered_list: capture_assignments(&value.OrderedList)?,
            all_assignments_are_constant: value.AllAssignmentsAreConstant,
            virtual_assignments_offset: value.VirtualAssignmentsOffset,
            ignore_error: value.IgnoreError,
            select_plan: Box::new(CachedPlan::try_capture(value.SelectPlan.as_ref())?),
        })
    }
    fn restore(&self, context: ContextRef) -> Result<Update, CacheSnapshotError> {
        Ok(Update {
            SimpleSchemaProducer: self.producer.restore(context.clone())?,
            OrderedList: restore_assignments(&self.ordered_list, context.GetExprCtx())?,
            AllAssignmentsAreConstant: self.all_assignments_are_constant,
            VirtualAssignmentsOffset: self.virtual_assignments_offset,
            IgnoreError: self.ignore_error,
            SelectPlan: self.select_plan.restore(context)?,
            FKChecks: Vec::new(),
            FKCascades: Vec::new(),
        })
    }
}

#[derive(Clone)]
struct CachedTblColPosInfo {
    table_id: i64,
    start: usize,
    end: usize,
    handle_columns: Vec<expression::CachedColumn>,
    index_layouts: Option<Vec<DeleteIndexLayout>>,
}

#[derive(Clone)]
pub struct CachedDelete {
    producer: CachedSimpleSchemaProducer,
    is_multi_table: bool,
    select_plan: Box<CachedPlan>,
    table_column_positions: Vec<CachedTblColPosInfo>,
    ignore_error: bool,
}

impl CachedDelete {
    fn capture(value: &Delete) -> Result<Self, CacheSnapshotError> {
        if value.HasForeignKeyPlans() {
            return Err(unsupported(
                "Delete with foreign-key checks or cascades is not cacheable",
            ));
        }
        Ok(Self {
            producer: CachedSimpleSchemaProducer::capture(&value.SimpleSchemaProducer)?,
            is_multi_table: value.IsMultiTable,
            select_plan: Box::new(CachedPlan::try_capture(value.SelectPlan.as_ref())?),
            table_column_positions: value
                .TblColPosInfos
                .iter()
                .map(|info| {
                    Ok(CachedTblColPosInfo {
                        table_id: info.TblID,
                        start: info.Start,
                        end: info.End,
                        handle_columns: capture_columns(&info.HandleCols)?,
                        index_layouts: info
                            .IndexesRowLayout
                            .as_ref()
                            .map(|layouts| layouts.Iter().cloned().collect()),
                    })
                })
                .collect::<Result<_, CacheSnapshotError>>()?,
            ignore_error: value.IgnoreErr,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<Delete, CacheSnapshotError> {
        Ok(Delete {
            SimpleSchemaProducer: self.producer.restore(context.clone())?,
            IsMultiTable: self.is_multi_table,
            SelectPlan: self.select_plan.restore(context.clone())?,
            TblColPosInfos: self
                .table_column_positions
                .iter()
                .map(|info| {
                    Ok(TblColPosInfo {
                        TblID: info.table_id,
                        Start: info.start,
                        End: info.end,
                        HandleCols: restore_columns(&info.handle_columns, context.GetExprCtx())?,
                        IndexesRowLayout: info.index_layouts.clone().map(DeleteIndexRowLayout::New),
                    })
                })
                .collect::<Result<_, CacheSnapshotError>>()?,
            IgnoreErr: self.ignore_error,
            FKChecks: Vec::new(),
            FKCascades: Vec::new(),
        })
    }
}

#[derive(Clone)]
pub struct CachedInsert {
    producer: CachedSimpleSchemaProducer,
    table_schema: Option<expression::CachedSchema>,
    table_column_names: Vec<Option<std::sync::Arc<types::metadata::FieldName>>>,
    columns: Vec<parser_ast::ColumnName>,
    lists: Vec<Vec<expression::CachedExpression>>,
    on_duplicate: Vec<CachedAssignment>,
    duplicate_schema: Option<expression::CachedSchema>,
    duplicate_names: Vec<Option<std::sync::Arc<types::metadata::FieldName>>>,
    generated_expressions: Vec<expression::CachedExpression>,
    generated_assignments: Vec<CachedAssignment>,
    select_plan: Option<Box<CachedPlan>>,
    is_replace: bool,
    ignore_error: bool,
    need_fill_default: bool,
    all_assignments_are_constant: bool,
    row_len: isize,
}

impl CachedInsert {
    fn capture(value: &Insert) -> Result<Self, CacheSnapshotError> {
        if value.HasForeignKeyPlans() {
            return Err(unsupported(
                "Insert with foreign-key checks or cascades is not cacheable",
            ));
        }
        if value.Table.is_some() {
            return Err(unsupported(
                "Insert with a session-bound runtime table is not cacheable",
            ));
        }
        Ok(Self {
            producer: CachedSimpleSchemaProducer::capture(&value.SimpleSchemaProducer)?,
            table_schema: value
                .TableSchema
                .as_ref()
                .map(expression::CachedSchema::try_from_schema)
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            table_column_names: value.TableColNames.0.clone(),
            columns: value.Columns.iter().map(|v| (**v).clone()).collect(),
            lists: value
                .Lists
                .iter()
                .map(|row| capture_expressions(row))
                .collect::<Result<_, _>>()?,
            on_duplicate: value
                .OnDuplicate
                .iter()
                .map(|v| CachedAssignment::capture(v))
                .collect::<Result<_, _>>()?,
            duplicate_schema: value
                .Schema4OnDuplicate
                .as_ref()
                .map(expression::CachedSchema::try_from_schema)
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            duplicate_names: value.Names4OnDuplicate.0.clone(),
            generated_expressions: capture_expressions(&value.GenCols.Exprs)?,
            generated_assignments: value
                .GenCols
                .OnDuplicates
                .iter()
                .map(|v| CachedAssignment::capture(v))
                .collect::<Result<_, _>>()?,
            select_plan: value
                .SelectPlan
                .as_deref()
                .map(CachedPlan::try_capture)
                .transpose()?
                .map(Box::new),
            is_replace: value.IsReplace,
            ignore_error: value.IgnoreErr,
            need_fill_default: value.NeedFillDefaultValue,
            all_assignments_are_constant: value.AllAssignmentsAreConstant,
            row_len: value.RowLen,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<Insert, CacheSnapshotError> {
        let expr_context = context.GetExprCtx();
        Ok(Insert {
            SimpleSchemaProducer: self.producer.restore(context.clone())?,
            Table: None,
            TableSchema: self
                .table_schema
                .as_ref()
                .map(|v| v.restore(expr_context))
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            TableColNames: expression::types::NameSlice(self.table_column_names.clone()),
            Columns: self.columns.iter().cloned().map(Box::new).collect(),
            Lists: self
                .lists
                .iter()
                .map(|row| restore_expressions(row, expr_context))
                .collect::<Result<_, _>>()?,
            OnDuplicate: restore_assignments(&self.on_duplicate, expr_context)?
                .into_iter()
                .map(Box::new)
                .collect(),
            Schema4OnDuplicate: self
                .duplicate_schema
                .as_ref()
                .map(|v| v.restore(expr_context))
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            Names4OnDuplicate: expression::types::NameSlice(self.duplicate_names.clone()),
            GenCols: crate::InsertGeneratedColumns {
                Exprs: restore_expressions(&self.generated_expressions, expr_context)?,
                OnDuplicates: restore_assignments(&self.generated_assignments, expr_context)?
                    .into_iter()
                    .map(Box::new)
                    .collect(),
            },
            SelectPlan: self
                .select_plan
                .as_deref()
                .map(|v| v.restore(context.clone()))
                .transpose()?,
            IsReplace: self.is_replace,
            IgnoreErr: self.ignore_error,
            NeedFillDefaultValue: self.need_fill_default,
            AllAssignmentsAreConstant: self.all_assignments_are_constant,
            RowLen: self.row_len,
            FKChecks: Vec::new(),
            FKCascades: Vec::new(),
        })
    }
}

fn capture_sort_items(
    values: &[property::SortItem],
) -> Result<Vec<CachedSortItem>, CacheSnapshotError> {
    values
        .iter()
        .map(|item| {
            Ok(CachedSortItem {
                column: expression::CachedColumn::try_from_column(&item.Col)
                    .map_err(|error| unsupported(error.to_string()))?,
                desc: item.Desc,
            })
        })
        .collect()
}

fn restore_sort_items(
    values: &[CachedSortItem],
    context: &dyn expression::BuildContext,
) -> Result<Vec<property::SortItem>, CacheSnapshotError> {
    values
        .iter()
        .map(|item| {
            Ok(property::SortItem {
                Col: item
                    .column
                    .restore_column(context)
                    .map_err(|error| unsupported(error.to_string()))?,
                Desc: item.desc,
            })
        })
        .collect()
}

#[derive(Clone)]
struct CachedByItem {
    expression: expression::CachedExpression,
    desc: bool,
}

fn capture_by_items(
    values: &[planner_util::ByItems],
) -> Result<Vec<CachedByItem>, CacheSnapshotError> {
    values
        .iter()
        .map(|item| {
            Ok(CachedByItem {
                expression: expression::CachedExpression::try_from_expression(item.Expr.as_ref())
                    .map_err(|error| unsupported(error.to_string()))?,
                desc: item.Desc,
            })
        })
        .collect()
}

fn restore_by_items(
    values: &[CachedByItem],
    context: &dyn expression::BuildContext,
) -> Result<Vec<planner_util::ByItems>, CacheSnapshotError> {
    values
        .iter()
        .map(|item| {
            Ok(planner_util::ByItems {
                Expr: item
                    .expression
                    .restore(context)
                    .map_err(|error| unsupported(error.to_string()))?,
                Desc: item.desc,
            })
        })
        .collect()
}

fn capture_expressions(
    values: &[expression::ExprBox],
) -> Result<Vec<expression::CachedExpression>, CacheSnapshotError> {
    values
        .iter()
        .map(|value| {
            expression::CachedExpression::try_from_expression(value.as_ref())
                .map_err(|error| unsupported(error.to_string()))
        })
        .collect()
}

fn restore_expressions(
    values: &[expression::CachedExpression],
    context: &dyn expression::BuildContext,
) -> Result<Vec<expression::ExprBox>, CacheSnapshotError> {
    values
        .iter()
        .map(|value| {
            value
                .restore(context)
                .map_err(|error| unsupported(error.to_string()))
        })
        .collect()
}

fn capture_columns(
    values: &[expression::Column],
) -> Result<Vec<expression::CachedColumn>, CacheSnapshotError> {
    values
        .iter()
        .map(|value| {
            expression::CachedColumn::try_from_column(value)
                .map_err(|error| unsupported(error.to_string()))
        })
        .collect()
}

fn restore_columns(
    values: &[expression::CachedColumn],
    context: &dyn expression::BuildContext,
) -> Result<Vec<expression::Column>, CacheSnapshotError> {
    values
        .iter()
        .map(|value| {
            value
                .restore_column(context)
                .map_err(|error| unsupported(error.to_string()))
        })
        .collect()
}

fn capture_scalar_functions(
    values: &[expression::ScalarFunction],
) -> Result<Vec<expression::CachedExpression>, CacheSnapshotError> {
    values
        .iter()
        .map(|value| {
            expression::CachedExpression::try_from_expression(value)
                .map_err(|error| unsupported(error.to_string()))
        })
        .collect()
}

fn restore_scalar_functions(
    values: &[expression::CachedExpression],
    context: &dyn expression::BuildContext,
) -> Result<Vec<expression::ScalarFunction>, CacheSnapshotError> {
    values
        .iter()
        .map(|value| {
            let restored = value
                .restore(context)
                .map_err(|error| unsupported(error.to_string()))?;
            restored
                .as_any()
                .downcast_ref::<expression::ScalarFunction>()
                .map(expression::ScalarFunction::clone_scalar)
                .ok_or_else(|| unsupported("cached scalar function restored as another expression"))
        })
        .collect()
}

#[derive(Clone)]
struct CachedBasePhysicalJoin {
    producer: CachedSchemaProducer,
    join_type: base::JoinType,
    left_conditions: Vec<expression::CachedExpression>,
    right_conditions: Vec<expression::CachedExpression>,
    other_conditions: Vec<expression::CachedExpression>,
    inner_child_index: usize,
    outer_join_keys: Vec<expression::CachedColumn>,
    inner_join_keys: Vec<expression::CachedColumn>,
    left_join_keys: Vec<expression::CachedColumn>,
    right_join_keys: Vec<expression::CachedColumn>,
    is_null_equal: Vec<bool>,
    default_values: Vec<types::datum::Datum>,
    left_null_aware_join_keys: Vec<expression::CachedColumn>,
    right_null_aware_join_keys: Vec<expression::CachedColumn>,
}

impl CachedBasePhysicalJoin {
    fn capture(value: &BasePhysicalJoin) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            join_type: value.JoinType,
            left_conditions: capture_expressions(&value.LeftConditions)?,
            right_conditions: capture_expressions(&value.RightConditions)?,
            other_conditions: capture_expressions(&value.OtherConditions)?,
            inner_child_index: value.InnerChildIdx,
            outer_join_keys: capture_columns(&value.OuterJoinKeys)?,
            inner_join_keys: capture_columns(&value.InnerJoinKeys)?,
            left_join_keys: capture_columns(&value.LeftJoinKeys)?,
            right_join_keys: capture_columns(&value.RightJoinKeys)?,
            is_null_equal: value.IsNullEQ.clone(),
            default_values: value.DefaultValues.clone(),
            left_null_aware_join_keys: capture_columns(&value.LeftNAJoinKeys)?,
            right_null_aware_join_keys: capture_columns(&value.RightNAJoinKeys)?,
        })
    }

    fn restore(&self, context: ContextRef) -> Result<BasePhysicalJoin, CacheSnapshotError> {
        let expression_context = context.GetExprCtx();
        Ok(BasePhysicalJoin {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            JoinType: self.join_type,
            LeftConditions: restore_expressions(&self.left_conditions, expression_context)?,
            RightConditions: restore_expressions(&self.right_conditions, expression_context)?,
            OtherConditions: restore_expressions(&self.other_conditions, expression_context)?,
            InnerChildIdx: self.inner_child_index,
            OuterJoinKeys: restore_columns(&self.outer_join_keys, expression_context)?,
            InnerJoinKeys: restore_columns(&self.inner_join_keys, expression_context)?,
            LeftJoinKeys: restore_columns(&self.left_join_keys, expression_context)?,
            RightJoinKeys: restore_columns(&self.right_join_keys, expression_context)?,
            IsNullEQ: self.is_null_equal.clone(),
            DefaultValues: self.default_values.clone(),
            LeftNAJoinKeys: restore_columns(&self.left_null_aware_join_keys, expression_context)?,
            RightNAJoinKeys: restore_columns(&self.right_null_aware_join_keys, expression_context)?,
        })
    }
}

#[derive(Clone)]
pub struct CachedHashJoin {
    base: CachedBasePhysicalJoin,
    concurrency: u64,
    equal_conditions: Vec<expression::CachedExpression>,
    null_aware_equal_conditions: Vec<expression::CachedExpression>,
    use_outer_to_build: bool,
    store_type: kv::StoreType,
    mpp_shuffle_join: bool,
    from_hash_join_hint: bool,
    has_table_alias: bool,
    runtime_filter_types: Vec<crate::RuntimeFilterType>,
}

impl CachedHashJoin {
    fn capture(value: &PhysicalHashJoin) -> Result<Self, CacheSnapshotError> {
        if !value.RuntimeFilterList.is_empty() {
            return Err(unsupported(
                "hash join with runtime filters is not cacheable",
            ));
        }
        Ok(Self {
            base: CachedBasePhysicalJoin::capture(&value.BasePhysicalJoin)?,
            concurrency: value.Concurrency,
            equal_conditions: capture_scalar_functions(&value.EqualConditions)?,
            null_aware_equal_conditions: capture_scalar_functions(&value.NAEqualConditions)?,
            use_outer_to_build: value.UseOuterToBuild,
            store_type: value.StoreTp,
            mpp_shuffle_join: value.MppShuffleJoin,
            from_hash_join_hint: value.FromHashJoinHint,
            has_table_alias: value.HasTableAlias,
            runtime_filter_types: value.RuntimeFilterTypes.clone(),
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalHashJoin, CacheSnapshotError> {
        Ok(PhysicalHashJoin {
            BasePhysicalJoin: self.base.restore(context.clone())?,
            Concurrency: self.concurrency,
            EqualConditions: restore_scalar_functions(
                &self.equal_conditions,
                context.GetExprCtx(),
            )?,
            NAEqualConditions: restore_scalar_functions(
                &self.null_aware_equal_conditions,
                context.GetExprCtx(),
            )?,
            UseOuterToBuild: self.use_outer_to_build,
            StoreTp: self.store_type,
            MppShuffleJoin: self.mpp_shuffle_join,
            FromHashJoinHint: self.from_hash_join_hint,
            HasTableAlias: self.has_table_alias,
            RuntimeFilterList: Vec::new(),
            RuntimeFilterTypes: self.runtime_filter_types.clone(),
        })
    }
}

#[derive(Clone)]
pub struct CachedMergeJoin {
    base: CachedBasePhysicalJoin,
    descending: bool,
}

impl CachedMergeJoin {
    fn capture(value: &PhysicalMergeJoin) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            base: CachedBasePhysicalJoin::capture(&value.BasePhysicalJoin)?,
            descending: value.Desc,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalMergeJoin, CacheSnapshotError> {
        Ok(PhysicalMergeJoin {
            BasePhysicalJoin: self.base.restore(context)?,
            Desc: self.descending,
        })
    }
}

#[derive(Clone)]
pub struct CachedIndexJoin {
    base: CachedBasePhysicalJoin,
    inner_plan: Option<Box<CachedPlan>>,
    ranges: ranger::Ranges,
    key_offset_to_index_offset: Vec<i32>,
    index_column_lengths: Vec<i32>,
    compare_filters: Option<CachedCompareFilters>,
    outer_hash_keys: Vec<expression::CachedColumn>,
    inner_hash_keys: Vec<expression::CachedColumn>,
    equal_conditions: Vec<expression::CachedExpression>,
    from_decorrelated_apply: bool,
}

#[derive(Clone)]
struct CachedCompareFilters {
    target_column: Option<expression::CachedColumn>,
    column_length: i32,
    operation_types: Vec<String>,
    operation_arguments: Vec<expression::CachedExpression>,
    temporary_constants: Vec<expression::CachedExpression>,
    affected_schema: expression::CachedSchema,
}

impl CachedCompareFilters {
    fn capture(
        value: &crate::physical_index_join::ColWithCmpFuncManager,
    ) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            target_column: value
                .TargetCol
                .as_ref()
                .map(expression::CachedColumn::try_from_column)
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            column_length: value.ColLength,
            operation_types: value.OpType.clone(),
            operation_arguments: capture_expressions(&value.OpArg)?,
            temporary_constants: value
                .TmpConstant
                .iter()
                .map(|constant| {
                    expression::CachedExpression::try_from_expression(constant)
                        .map_err(|e| unsupported(e.to_string()))
                })
                .collect::<Result<_, _>>()?,
            affected_schema: expression::CachedSchema::try_from_schema(&value.AffectedColSchema)
                .map_err(|e| unsupported(e.to_string()))?,
        })
    }

    fn restore(
        &self,
        context: &dyn expression::BuildContext,
    ) -> Result<crate::physical_index_join::ColWithCmpFuncManager, CacheSnapshotError> {
        let temporary_constants = self
            .temporary_constants
            .iter()
            .map(|constant| {
                let expression = constant
                    .restore(context)
                    .map_err(|e| unsupported(e.to_string()))?;
                expression
                    .as_any()
                    .downcast_ref::<expression::Constant>()
                    .map(expression::Constant::Clone)
                    .ok_or_else(|| {
                        unsupported("cached comparison constant restored as another expression")
                    })
            })
            .collect::<Result<_, _>>()?;
        Ok(
            crate::physical_index_join::ColWithCmpFuncManager::restore_cache_snapshot(
                self.target_column
                    .as_ref()
                    .map(|column| column.restore_column(context))
                    .transpose()
                    .map_err(|e| unsupported(e.to_string()))?,
                self.column_length,
                self.operation_types.clone(),
                restore_expressions(&self.operation_arguments, context)?,
                temporary_constants,
                self.affected_schema
                    .restore(context)
                    .map_err(|e| unsupported(e.to_string()))?,
            ),
        )
    }
}

impl CachedIndexJoin {
    fn capture(value: &PhysicalIndexJoin) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            base: CachedBasePhysicalJoin::capture(&value.BasePhysicalJoin)?,
            inner_plan: value
                .InnerPlan
                .as_deref()
                .map(CachedPlan::try_capture)
                .transpose()?
                .map(Box::new),
            ranges: value.Ranges.clone(),
            key_offset_to_index_offset: value.KeyOff2IdxOff.clone(),
            index_column_lengths: value.IdxColLens.clone(),
            compare_filters: value
                .CompareFilters
                .as_ref()
                .map(CachedCompareFilters::capture)
                .transpose()?,
            outer_hash_keys: capture_columns(&value.OuterHashKeys)?,
            inner_hash_keys: capture_columns(&value.InnerHashKeys)?,
            equal_conditions: capture_scalar_functions(&value.EqualConditions)?,
            from_decorrelated_apply: value.FromDecorrelatedApply,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalIndexJoin, CacheSnapshotError> {
        Ok(PhysicalIndexJoin {
            BasePhysicalJoin: self.base.restore(context.clone())?,
            InnerPlan: self
                .inner_plan
                .as_ref()
                .map(|plan| plan.restore(context.clone()))
                .transpose()?,
            Ranges: self.ranges.clone(),
            KeyOff2IdxOff: self.key_offset_to_index_offset.clone(),
            IdxColLens: self.index_column_lengths.clone(),
            CompareFilters: self
                .compare_filters
                .as_ref()
                .map(|filters| filters.restore(context.GetExprCtx()))
                .transpose()?,
            OuterHashKeys: restore_columns(&self.outer_hash_keys, context.GetExprCtx())?,
            InnerHashKeys: restore_columns(&self.inner_hash_keys, context.GetExprCtx())?,
            EqualConditions: restore_scalar_functions(
                &self.equal_conditions,
                context.GetExprCtx(),
            )?,
            FromDecorrelatedApply: self.from_decorrelated_apply,
        })
    }
}

#[derive(Clone)]
pub struct CachedIndexHashJoin {
    value: PhysicalIndexHashJoin,
}

impl CachedIndexHashJoin {
    pub fn capture(value: &PhysicalIndexHashJoin) -> Self {
        Self {
            value: value.clone(),
        }
    }
    pub fn restore(&self) -> PhysicalIndexHashJoin {
        self.value.clone()
    }
}

#[derive(Clone)]
pub struct CachedSelection {
    producer: CachedSchemaProducer,
    conditions: Vec<expression::CachedExpression>,
    from_data_source: bool,
}

impl CachedSelection {
    fn capture(value: &PhysicalSelection) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            conditions: capture_expressions(&value.Conditions)?,
            from_data_source: value.FromDataSource,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalSelection, CacheSnapshotError> {
        Ok(PhysicalSelection {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            Conditions: restore_expressions(&self.conditions, context.GetExprCtx())?,
            FromDataSource: self.from_data_source,
        })
    }
}

#[derive(Clone)]
pub struct CachedProjection {
    producer: CachedSchemaProducer,
    expressions: Vec<expression::CachedExpression>,
    calculate_no_delay: bool,
    avoid_column_evaluator: bool,
}

impl CachedProjection {
    fn capture(value: &PhysicalProjection) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            expressions: capture_expressions(&value.Exprs)?,
            calculate_no_delay: value.CalculateNoDelay,
            avoid_column_evaluator: value.AvoidColumnEvaluator,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalProjection, CacheSnapshotError> {
        Ok(PhysicalProjection {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            Exprs: restore_expressions(&self.expressions, context.GetExprCtx())?,
            CalculateNoDelay: self.calculate_no_delay,
            AvoidColumnEvaluator: self.avoid_column_evaluator,
        })
    }
}

#[derive(Clone)]
pub struct CachedTopN {
    producer: CachedSchemaProducer,
    by_items: Vec<CachedByItem>,
    partition_by: Vec<CachedSortItem>,
    offset: u64,
    count: u64,
    prefix_column: Option<expression::CachedColumn>,
    prefix_length: usize,
}

impl CachedTopN {
    fn capture(value: &PhysicalTopN) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            by_items: capture_by_items(&value.ByItems)?,
            partition_by: capture_sort_items(&value.PartitionBy)?,
            offset: value.Offset,
            count: value.Count,
            prefix_column: value
                .PrefixCol
                .as_ref()
                .map(expression::CachedColumn::try_from_column)
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            prefix_length: value.PrefixLen,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalTopN, CacheSnapshotError> {
        Ok(PhysicalTopN {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            ByItems: restore_by_items(&self.by_items, context.GetExprCtx())?,
            PartitionBy: restore_sort_items(&self.partition_by, context.GetExprCtx())?,
            Offset: self.offset,
            Count: self.count,
            PrefixCol: self
                .prefix_column
                .as_ref()
                .map(|v| v.restore_column(context.GetExprCtx()))
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            PrefixLen: self.prefix_length,
        })
    }
}

#[derive(Clone)]
pub struct CachedLimit {
    producer: CachedSchemaProducer,
    partition_by: Vec<CachedSortItem>,
    offset: u64,
    count: u64,
    offset_param: Option<usize>,
    count_param: Option<usize>,
    count_includes_offset: bool,
    prefix_column: Option<expression::CachedColumn>,
    prefix_length: usize,
}

impl CachedLimit {
    fn capture(value: &PhysicalLimit) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            partition_by: capture_sort_items(&value.PartitionBy)?,
            offset: value.Offset,
            count: value.Count,
            offset_param: value.OffsetParam,
            count_param: value.CountParam,
            count_includes_offset: value.CountIncludesOffset,
            prefix_column: value
                .PrefixCol
                .as_ref()
                .map(expression::CachedColumn::try_from_column)
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            prefix_length: value.PrefixLen,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalLimit, CacheSnapshotError> {
        let bound_offset = self
            .offset_param
            .map(|index| context.prepared_limit_value(index).map_err(unsupported))
            .transpose()?
            .unwrap_or(self.offset);
        let bound_count = self
            .count_param
            .map(|index| context.prepared_limit_value(index).map_err(unsupported))
            .transpose()?
            .unwrap_or(self.count);
        // The cop-side LIMIT produced from a parameterized root LIMIT has
        // offset zero and count OFFSET+COUNT. Retain that derived shape when
        // restoring a cached plan with different bound marker values.
        let (offset, count) = if self.count_includes_offset {
            (0, bound_offset.saturating_add(bound_count))
        } else {
            (
                bound_offset,
                bound_count.min(u64::MAX.saturating_sub(bound_offset)),
            )
        };
        Ok(PhysicalLimit {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            PartitionBy: restore_sort_items(&self.partition_by, context.GetExprCtx())?,
            Offset: offset,
            Count: count,
            OffsetParam: self.offset_param,
            CountParam: self.count_param,
            CountIncludesOffset: self.count_includes_offset,
            PrefixCol: self
                .prefix_column
                .as_ref()
                .map(|v| v.restore_column(context.GetExprCtx()))
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            PrefixLen: self.prefix_length,
        })
    }
}

#[derive(Clone)]
struct CachedAggFunc {
    name: String,
    arguments: Vec<expression::CachedExpression>,
    return_type: Option<expression::types::FieldType>,
    mode: aggregation::AggFunctionMode,
    has_distinct: bool,
    order_by: Vec<CachedByItem>,
    grouping_id: isize,
}

impl CachedAggFunc {
    fn capture(value: &aggregation::AggFuncDesc) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            name: value.Name.clone(),
            arguments: capture_expressions(&value.Args)?,
            return_type: value.RetTp.clone(),
            mode: value.Mode,
            has_distinct: value.HasDistinct,
            order_by: capture_by_items(&value.OrderByItems)?,
            grouping_id: value.GroupingID,
        })
    }
    fn restore(
        &self,
        context: &dyn expression::BuildContext,
    ) -> Result<aggregation::AggFuncDesc, CacheSnapshotError> {
        Ok(aggregation::AggFuncDesc {
            baseFuncDesc: aggregation::baseFuncDesc {
                Name: self.name.clone(),
                Args: restore_expressions(&self.arguments, context)?,
                RetTp: self.return_type.clone(),
            },
            Mode: self.mode,
            HasDistinct: self.has_distinct,
            OrderByItems: restore_by_items(&self.order_by, context)?,
            GroupingID: self.grouping_id,
        })
    }
}

#[derive(Clone)]
struct CachedPhysicalAgg {
    producer: CachedSchemaProducer,
    functions: Vec<CachedAggFunc>,
    group_by: Vec<expression::CachedExpression>,
    mpp_run_mode: AggMppRunMode,
    mpp_partition_columns: Vec<CachedMppColumn>,
}

impl CachedPhysicalAgg {
    fn capture(value: &BasePhysicalAgg) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            functions: value
                .AggFuncs
                .iter()
                .map(CachedAggFunc::capture)
                .collect::<Result<_, _>>()?,
            group_by: capture_expressions(&value.GroupByItems)?,
            mpp_run_mode: value.MppRunMode,
            mpp_partition_columns: value
                .MppPartitionCols
                .iter()
                .map(|column| {
                    Ok(CachedMppColumn {
                        column: expression::CachedColumn::try_from_column(&column.Col)
                            .map_err(|e| unsupported(e.to_string()))?,
                        collate_id: column.CollateID,
                    })
                })
                .collect::<Result<_, CacheSnapshotError>>()?,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<BasePhysicalAgg, CacheSnapshotError> {
        Ok(BasePhysicalAgg {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            AggFuncs: self
                .functions
                .iter()
                .map(|function| function.restore(context.GetExprCtx()))
                .collect::<Result<_, _>>()?,
            GroupByItems: restore_expressions(&self.group_by, context.GetExprCtx())?,
            MppRunMode: self.mpp_run_mode,
            MppPartitionCols: self
                .mpp_partition_columns
                .iter()
                .map(|column| {
                    Ok(property::MPPPartitionColumn {
                        Col: column
                            .column
                            .restore_column(context.GetExprCtx())
                            .map_err(|e| unsupported(e.to_string()))?,
                        CollateID: column.collate_id,
                    })
                })
                .collect::<Result<_, CacheSnapshotError>>()?,
        })
    }
}

#[derive(Clone)]
pub struct CachedStreamAgg {
    aggregate: CachedPhysicalAgg,
}
impl CachedStreamAgg {
    fn capture(value: &PhysicalStreamAgg) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            aggregate: CachedPhysicalAgg::capture(&value.BasePhysicalAgg)?,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalStreamAgg, CacheSnapshotError> {
        Ok(PhysicalStreamAgg {
            BasePhysicalAgg: self.aggregate.restore(context)?,
        })
    }
}

#[derive(Clone)]
pub struct CachedHashAgg {
    aggregate: CachedPhysicalAgg,
    tiflash_pre_agg_mode: String,
}
impl CachedHashAgg {
    fn capture(value: &PhysicalHashAgg) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            aggregate: CachedPhysicalAgg::capture(&value.BasePhysicalAgg)?,
            tiflash_pre_agg_mode: value.TiflashPreAggMode.clone(),
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalHashAgg, CacheSnapshotError> {
        Ok(PhysicalHashAgg {
            BasePhysicalAgg: self.aggregate.restore(context)?,
            TiflashPreAggMode: self.tiflash_pre_agg_mode.clone(),
        })
    }
}

#[derive(Clone)]
pub struct CachedUnionAll {
    producer: CachedSchemaProducer,
    mpp: bool,
}
impl CachedUnionAll {
    fn capture(value: &PhysicalUnionAll) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            mpp: value.Mpp,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalUnionAll, CacheSnapshotError> {
        Ok(PhysicalUnionAll {
            PhysicalSchemaProducer: self.producer.restore(context)?,
            Mpp: self.mpp,
        })
    }
}

#[derive(Clone)]
enum CachedHandleCols {
    Int(expression::CachedColumn),
    Common {
        table: model::TableInfo,
        index: model::IndexInfo,
        columns: Vec<expression::CachedColumn>,
    },
}

impl CachedHandleCols {
    fn capture(value: &dyn planner_util::HandleCols) -> Result<Self, CacheSnapshotError> {
        let columns = value
            .IterColumns()
            .map(expression::CachedColumn::try_from_column)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| unsupported(e.to_string()))?;
        if value.IsInt() {
            return columns
                .into_iter()
                .next()
                .map(Self::Int)
                .ok_or_else(|| unsupported("integer handle has no column"));
        }
        let (table, index) = value
            .CacheCommonHandleMetadata()
            .ok_or_else(|| unsupported("common handle metadata unavailable"))?;
        Ok(Self::Common {
            table,
            index,
            columns,
        })
    }
    fn restore(
        &self,
        context: &dyn expression::BuildContext,
    ) -> Result<Box<dyn planner_util::HandleCols>, CacheSnapshotError> {
        match self {
            Self::Int(column) => Ok(planner_util::NewIntHandleCols(
                column
                    .restore_column(context)
                    .map_err(|e| unsupported(e.to_string()))?,
            )),
            Self::Common {
                table,
                index,
                columns,
            } => Ok(Box::new(
                planner_util::NewCommonHandlesColsWithoutColsAlign(
                    table.clone(),
                    index.clone(),
                    restore_columns(columns, context)?,
                ),
            )),
        }
    }
}

#[derive(Clone)]
pub struct CachedUnionScan {
    producer: CachedSchemaProducer,
    conditions: Vec<expression::CachedExpression>,
    handle_columns: CachedHandleCols,
}
impl CachedUnionScan {
    fn capture(value: &PhysicalUnionScan) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            conditions: capture_expressions(&value.Conditions)?,
            handle_columns: CachedHandleCols::capture(value.HandleCols.as_ref())?,
        })
    }
    fn restore(&self, context: ContextRef) -> Result<PhysicalUnionScan, CacheSnapshotError> {
        Ok(PhysicalUnionScan {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            Conditions: restore_expressions(&self.conditions, context.GetExprCtx())?,
            HandleCols: self.handle_columns.restore(context.GetExprCtx())?,
        })
    }
}

pub struct CachedTableDual {
    producer: CachedSchemaProducer,
    row_count: i32,
    names: types::metadata::NameSlice,
}

impl Clone for CachedTableDual {
    fn clone(&self) -> Self {
        Self {
            producer: self.producer.clone(),
            row_count: self.row_count,
            names: types::metadata::NameSlice(self.names.0.clone()),
        }
    }
}

impl CachedTableDual {
    fn capture(value: &PhysicalTableDual) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            row_count: value.RowCount,
            names: types::metadata::NameSlice(value.cache_names().0.clone()),
        })
    }

    fn restore(&self, context: ContextRef) -> Result<PhysicalTableDual, CacheSnapshotError> {
        let mut value = PhysicalTableDual::New(context.clone(), self.row_count);
        value.PhysicalSchemaProducer = self.producer.restore(context)?;
        value.restore_cached_names(types::metadata::NameSlice(self.names.0.clone()));
        Ok(value)
    }
}

#[derive(Clone)]
pub struct CachedTableScan {
    producer: CachedSchemaProducer,
    table: Option<model::TableInfo>,
    columns: Vec<model::ColumnInfo>,
    db_name: String,
    table_as_name: String,
    physical_table_id: i64,
    ranges: ranger::Ranges,
    range_info: String,
    access_condition: Vec<expression::CachedExpression>,
    filter_condition: Vec<expression::CachedExpression>,
    late_materialization_filter_condition: Vec<expression::CachedExpression>,
    late_materialization_selectivity: f64,
    filter_stats: Option<property::StatsInfo>,
    store_type: kv::StoreType,
    is_partition: bool,
    is_mpp_or_batch_cop: bool,
    desc: bool,
    keep_order: bool,
    is_common_handle: bool,
    table_column_histograms: Option<property::HistCollRef>,
    property: Option<CachedPhysicalProperty>,
}

impl CachedTableScan {
    fn capture(value: &PhysicalTableScan) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            table: value.Table.clone(),
            columns: value.Columns.clone(),
            db_name: value.DBName.clone(),
            table_as_name: value.TableAsName.clone(),
            physical_table_id: value.PhysicalTableID,
            ranges: value.Ranges.clone(),
            range_info: value.RangeInfo.clone(),
            access_condition: capture_expressions(&value.AccessCondition)?,
            filter_condition: capture_expressions(&value.FilterCondition)?,
            late_materialization_filter_condition: capture_expressions(
                &value.LateMaterializationFilterCondition,
            )?,
            late_materialization_selectivity: value.LateMaterializationSelectivity,
            filter_stats: value.FilterStats.clone(),
            store_type: value.StoreType,
            is_partition: value.IsPartition,
            is_mpp_or_batch_cop: value.IsMPPOrBatchCop,
            desc: value.Desc,
            keep_order: value.KeepOrder,
            is_common_handle: value.IsCommonHandle,
            table_column_histograms: value.TblColHists.clone(),
            property: value
                .Prop
                .as_ref()
                .map(CachedPhysicalProperty::capture)
                .transpose()?,
        })
    }

    fn restore(&self, context: ContextRef) -> Result<PhysicalTableScan, CacheSnapshotError> {
        let expression_context = context.GetExprCtx();
        Ok(PhysicalTableScan {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            Table: self.table.clone(),
            Columns: self.columns.clone(),
            DBName: self.db_name.clone(),
            TableAsName: self.table_as_name.clone(),
            PhysicalTableID: self.physical_table_id,
            Ranges: self.ranges.clone(),
            RangeInfo: self.range_info.clone(),
            AccessCondition: restore_expressions(&self.access_condition, expression_context)?,
            FilterCondition: restore_expressions(&self.filter_condition, expression_context)?,
            LateMaterializationFilterCondition: restore_expressions(
                &self.late_materialization_filter_condition,
                expression_context,
            )?,
            LateMaterializationSelectivity: self.late_materialization_selectivity,
            FilterStats: self.filter_stats.clone(),
            StoreType: self.store_type,
            IsPartition: self.is_partition,
            IsMPPOrBatchCop: self.is_mpp_or_batch_cop,
            Desc: self.desc,
            KeepOrder: self.keep_order,
            IsCommonHandle: self.is_common_handle,
            TblColHists: self.table_column_histograms.clone(),
            Prop: self
                .property
                .as_ref()
                .map(|value| value.restore(expression_context))
                .transpose()?,
        })
    }
}

#[derive(Clone)]
pub struct CachedIndexScan {
    producer: CachedSchemaProducer,
    access_condition: Vec<expression::CachedExpression>,
    filter_condition: Vec<expression::CachedExpression>,
    table: Option<model::TableInfo>,
    index: Option<model::IndexInfo>,
    index_columns: Vec<expression::CachedColumn>,
    index_column_lengths: Vec<i32>,
    ranges: ranger::Ranges,
    columns: Vec<model::ColumnInfo>,
    db_name: String,
    table_as_name: String,
    data_source_schema: Option<expression::CachedSchema>,
    range_info: String,
    physical_table_id: i64,
    is_partition: bool,
    desc: bool,
    keep_order: bool,
    double_read: bool,
    need_common_handle: bool,
    table_column_histograms: Option<property::HistCollRef>,
    primary_key_handle_column: Option<expression::CachedColumn>,
    constant_columns_by_condition: Vec<bool>,
    property: Option<CachedPhysicalProperty>,
}

impl CachedIndexScan {
    fn capture(value: &PhysicalIndexScan) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            access_condition: capture_expressions(&value.AccessCondition)?,
            filter_condition: capture_expressions(&value.FilterCondition)?,
            table: value.Table.clone(),
            index: value.Index.clone(),
            index_columns: capture_columns(&value.IdxCols)?,
            index_column_lengths: value.IdxColLens.clone(),
            ranges: value.Ranges.clone(),
            columns: value.Columns.clone(),
            db_name: value.DBName.clone(),
            table_as_name: value.TableAsName.clone(),
            data_source_schema: value
                .DataSourceSchema
                .as_ref()
                .map(expression::CachedSchema::try_from_schema)
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            range_info: value.RangeInfo.clone(),
            physical_table_id: value.PhysicalTableID,
            is_partition: value.IsPartition,
            desc: value.Desc,
            keep_order: value.KeepOrder,
            double_read: value.DoubleRead,
            need_common_handle: value.NeedCommonHandle,
            table_column_histograms: value.TblColHists.clone(),
            primary_key_handle_column: value
                .PKIsHandleCol
                .as_ref()
                .map(expression::CachedColumn::try_from_column)
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            constant_columns_by_condition: value.ConstColsByCond.clone(),
            property: value
                .Prop
                .as_ref()
                .map(CachedPhysicalProperty::capture)
                .transpose()?,
        })
    }

    fn restore(&self, context: ContextRef) -> Result<PhysicalIndexScan, CacheSnapshotError> {
        let expression_context = context.GetExprCtx();
        Ok(PhysicalIndexScan {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            AccessCondition: restore_expressions(&self.access_condition, expression_context)?,
            FilterCondition: restore_expressions(&self.filter_condition, expression_context)?,
            Table: self.table.clone(),
            Index: self.index.clone(),
            IdxCols: restore_columns(&self.index_columns, expression_context)?,
            IdxColLens: self.index_column_lengths.clone(),
            Ranges: self.ranges.clone(),
            Columns: self.columns.clone(),
            DBName: self.db_name.clone(),
            TableAsName: self.table_as_name.clone(),
            DataSourceSchema: self
                .data_source_schema
                .as_ref()
                .map(|v| v.restore(expression_context))
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            RangeInfo: self.range_info.clone(),
            PhysicalTableID: self.physical_table_id,
            IsPartition: self.is_partition,
            Desc: self.desc,
            KeepOrder: self.keep_order,
            DoubleRead: self.double_read,
            NeedCommonHandle: self.need_common_handle,
            TblColHists: self.table_column_histograms.clone(),
            PKIsHandleCol: self
                .primary_key_handle_column
                .as_ref()
                .map(|v| v.restore_column(expression_context))
                .transpose()
                .map_err(|e| unsupported(e.to_string()))?,
            ConstColsByCond: self.constant_columns_by_condition.clone(),
            Prop: self
                .property
                .as_ref()
                .map(|v| v.restore(expression_context))
                .transpose()?,
        })
    }
}

#[derive(Clone)]
pub struct CachedPointGet {
    output_names: Vec<Option<std::sync::Arc<types::metadata::FieldName>>>,
    producer: CachedSchemaProducer,
    db_name: String,
    table: Option<model::TableInfo>,
    index: Option<model::IndexInfo>,
    partition_index: Option<usize>,
    handle: Option<i64>,
    index_values: Vec<types::datum::Datum>,
    index_columns: Vec<expression::CachedColumn>,
    index_column_lengths: Vec<i32>,
    access_conditions: Vec<expression::CachedExpression>,
    unsigned_handle: bool,
    is_table_dual: bool,
    lock: bool,
    lock_wait_time: i64,
    columns: Vec<model::ColumnInfo>,
    access_columns: Vec<expression::CachedColumn>,
    cost: f64,
}

impl CachedPointGet {
    fn capture(value: &PointGetPlan) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            output_names: value.OutputNames().0,
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            db_name: value.DBName.clone(),
            table: value.TblInfo.clone(),
            index: value.IndexInfo.clone(),
            partition_index: value.PartitionIdx,
            handle: value.Handle,
            index_values: value.IndexValues.clone(),
            index_columns: capture_columns(&value.IdxCols)?,
            index_column_lengths: value.IdxColLens.clone(),
            access_conditions: capture_expressions(&value.AccessConditions)?,
            unsigned_handle: value.UnsignedHandle,
            is_table_dual: value.IsTableDual,
            lock: value.Lock,
            lock_wait_time: value.LockWaitTime,
            columns: value.Columns.clone(),
            access_columns: capture_columns(&value.AccessColumns)?,
            cost: value.CostValue,
        })
    }

    fn restore(&self, context: ContextRef) -> Result<PointGetPlan, CacheSnapshotError> {
        let expression_context = context.GetExprCtx();
        Ok(PointGetPlan {
            output_names: types::metadata::NameSlice(self.output_names.clone()),
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            DBName: self.db_name.clone(),
            TblInfo: self.table.clone(),
            IndexInfo: self.index.clone(),
            PartitionIdx: self.partition_index,
            Handle: self.handle,
            IndexValues: self.index_values.clone(),
            IdxCols: restore_columns(&self.index_columns, expression_context)?,
            IdxColLens: self.index_column_lengths.clone(),
            AccessConditions: restore_expressions(&self.access_conditions, expression_context)?,
            UnsignedHandle: self.unsigned_handle,
            IsTableDual: self.is_table_dual,
            Lock: self.lock,
            LockWaitTime: self.lock_wait_time,
            Columns: self.columns.clone(),
            AccessColumns: restore_columns(&self.access_columns, expression_context)?,
            CostValue: self.cost,
        })
    }
}

#[derive(Clone)]
pub struct CachedBatchPointGet {
    point_get: CachedPointGet,
    handles: Vec<i64>,
    index_value_rows: Vec<Vec<types::datum::Datum>>,
    partition_indexes: Vec<usize>,
    keep_order: bool,
    desc: bool,
    lock: bool,
}

impl CachedBatchPointGet {
    fn capture(value: &BatchPointGetPlan) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            point_get: CachedPointGet::capture(&value.PointGetPlan)?,
            handles: value.Handles.clone(),
            index_value_rows: value.IndexValueRows.clone(),
            partition_indexes: value.PartitionIdxs.clone(),
            keep_order: value.KeepOrder,
            desc: value.Desc,
            lock: value.Lock,
        })
    }

    fn restore(&self, context: ContextRef) -> Result<BatchPointGetPlan, CacheSnapshotError> {
        Ok(BatchPointGetPlan {
            PointGetPlan: self.point_get.restore(context)?,
            Handles: self.handles.clone(),
            IndexValueRows: self.index_value_rows.clone(),
            PartitionIdxs: self.partition_indexes.clone(),
            KeepOrder: self.keep_order,
            Desc: self.desc,
            Lock: self.lock,
        })
    }
}

#[derive(Clone)]
struct CachedPlanPartInfo {
    pruning_conditions: Vec<expression::CachedExpression>,
    partition_names: Vec<parser_ast::CIStr>,
    columns: Vec<expression::CachedColumn>,
    column_names: Vec<Option<std::sync::Arc<types::metadata::FieldName>>>,
}

impl CachedPlanPartInfo {
    fn capture(value: &crate::PhysPlanPartInfo) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            pruning_conditions: capture_expressions(&value.PruningConds)?,
            partition_names: value.PartitionNames.clone(),
            columns: capture_columns(&value.Columns)?,
            column_names: value.ColumnNames.0.clone(),
        })
    }

    fn restore(
        &self,
        context: &dyn expression::BuildContext,
    ) -> Result<crate::PhysPlanPartInfo, CacheSnapshotError> {
        Ok(crate::PhysPlanPartInfo {
            PruningConds: restore_expressions(&self.pruning_conditions, context)?,
            PartitionNames: self.partition_names.clone(),
            Columns: restore_columns(&self.columns, context)?,
            ColumnNames: types::metadata::NameSlice(self.column_names.clone()),
        })
    }
}

#[derive(Clone)]
pub struct CachedIndexReader {
    producer: CachedSchemaProducer,
    index_plan: Option<Box<CachedPlan>>,
    output_columns: Vec<expression::CachedColumn>,
    partition: Option<CachedPlanPartInfo>,
}

impl CachedIndexReader {
    fn capture(value: &PhysicalIndexReader) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            index_plan: value
                .IndexPlan
                .as_deref()
                .map(CachedPlan::try_capture)
                .transpose()?
                .map(Box::new),
            output_columns: capture_columns(&value.OutputColumns)?,
            partition: value
                .PlanPartInfo
                .as_ref()
                .map(CachedPlanPartInfo::capture)
                .transpose()?,
        })
    }

    fn restore(&self, context: ContextRef) -> Result<PhysicalIndexReader, CacheSnapshotError> {
        Ok(PhysicalIndexReader {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            IndexPlan: self
                .index_plan
                .as_deref()
                .map(|plan| plan.restore(context.clone()))
                .transpose()?,
            OutputColumns: restore_columns(&self.output_columns, context.GetExprCtx())?,
            PlanPartInfo: self
                .partition
                .as_ref()
                .map(|info| info.restore(context.GetExprCtx()))
                .transpose()?,
        })
    }
}

#[derive(Clone)]
pub struct CachedTableReader {
    producer: CachedSchemaProducer,
    table_plan: Option<Box<CachedPlan>>,
    store_type: kv::StoreType,
    read_request_type: ReadReqType,
    is_common_handle: bool,
    partition: Option<CachedPlanPartInfo>,
}

impl CachedTableReader {
    fn capture(value: &PhysicalTableReader) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            table_plan: value
                .TablePlan
                .as_deref()
                .map(CachedPlan::try_capture)
                .transpose()?
                .map(Box::new),
            store_type: value.StoreType,
            read_request_type: value.ReadReqType,
            is_common_handle: value.IsCommonHandle,
            partition: value
                .PlanPartInfo
                .as_ref()
                .map(CachedPlanPartInfo::capture)
                .transpose()?,
        })
    }

    fn restore(&self, context: ContextRef) -> Result<PhysicalTableReader, CacheSnapshotError> {
        Ok(PhysicalTableReader {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            TablePlan: self
                .table_plan
                .as_deref()
                .map(|plan| plan.restore(context.clone()))
                .transpose()?,
            StoreType: self.store_type,
            ReadReqType: self.read_request_type,
            IsCommonHandle: self.is_common_handle,
            PlanPartInfo: self
                .partition
                .as_ref()
                .map(|info| info.restore(context.GetExprCtx()))
                .transpose()?,
        })
    }
}

#[derive(Clone)]
pub struct CachedIndexLookupReader {
    producer: CachedSchemaProducer,
    push_down: bool,
    index_plan: Option<Box<CachedPlan>>,
    table_plan: Option<Box<CachedPlan>>,
    paging: bool,
    extra_handle_column: Option<expression::CachedColumn>,
    pushed_limit: Option<crate::physical_plan_misc::PushedDownLimit>,
    common_handle_columns: Vec<expression::CachedColumn>,
    partition: Option<CachedPlanPartInfo>,
    expected_count: u64,
    keep_order: bool,
}

impl CachedIndexLookupReader {
    fn capture(value: &PhysicalIndexLookUpReader) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            push_down: value.IndexLookUpPushDown,
            index_plan: value
                .IndexPlan
                .as_deref()
                .map(CachedPlan::try_capture)
                .transpose()?
                .map(Box::new),
            table_plan: value
                .TablePlan
                .as_deref()
                .map(CachedPlan::try_capture)
                .transpose()?
                .map(Box::new),
            paging: value.Paging,
            extra_handle_column: value
                .ExtraHandleCol
                .as_ref()
                .map(expression::CachedColumn::try_from_column)
                .transpose()
                .map_err(|error| unsupported(error.to_string()))?,
            pushed_limit: value.PushedLimit,
            common_handle_columns: capture_columns(&value.CommonHandleCols)?,
            partition: value
                .PlanPartInfo
                .as_ref()
                .map(CachedPlanPartInfo::capture)
                .transpose()?,
            expected_count: value.ExpectedCnt,
            keep_order: value.KeepOrder,
        })
    }

    fn restore(
        &self,
        context: ContextRef,
    ) -> Result<PhysicalIndexLookUpReader, CacheSnapshotError> {
        Ok(PhysicalIndexLookUpReader {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            IndexLookUpPushDown: self.push_down,
            IndexPlan: self
                .index_plan
                .as_deref()
                .map(|plan| plan.restore(context.clone()))
                .transpose()?,
            TablePlan: self
                .table_plan
                .as_deref()
                .map(|plan| plan.restore(context.clone()))
                .transpose()?,
            Paging: self.paging,
            ExtraHandleCol: self
                .extra_handle_column
                .as_ref()
                .map(|column| column.restore_column(context.GetExprCtx()))
                .transpose()
                .map_err(|error| unsupported(error.to_string()))?,
            PushedLimit: self.pushed_limit,
            CommonHandleCols: restore_columns(&self.common_handle_columns, context.GetExprCtx())?,
            PlanPartInfo: self
                .partition
                .as_ref()
                .map(|info| info.restore(context.GetExprCtx()))
                .transpose()?,
            ExpectedCnt: self.expected_count,
            KeepOrder: self.keep_order,
        })
    }
}

#[derive(Clone)]
pub struct CachedIndexMergeReader {
    producer: CachedSchemaProducer,
    intersection: bool,
    access_mv_index: bool,
    pushed_limit: Option<crate::physical_plan_misc::PushedDownLimit>,
    by_items: Vec<CachedByItem>,
    partial_plans: Vec<CachedPlan>,
    table_plan: Option<Box<CachedPlan>>,
    partition: Option<CachedPlanPartInfo>,
    keep_order: bool,
}

impl CachedIndexMergeReader {
    fn capture(value: &PhysicalIndexMergeReader) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            producer: CachedSchemaProducer::try_from_producer(&value.PhysicalSchemaProducer)?,
            intersection: value.IsIntersectionType,
            access_mv_index: value.AccessMVIndex,
            pushed_limit: value.PushedLimit,
            by_items: capture_by_items(&value.ByItems)?,
            partial_plans: value
                .PartialPlansRaw
                .iter()
                .map(|plan| CachedPlan::try_capture(plan.as_ref()))
                .collect::<Result<_, _>>()?,
            table_plan: value
                .TablePlan
                .as_deref()
                .map(CachedPlan::try_capture)
                .transpose()?
                .map(Box::new),
            partition: value
                .PlanPartInfo
                .as_ref()
                .map(CachedPlanPartInfo::capture)
                .transpose()?,
            keep_order: value.KeepOrder,
        })
    }

    fn restore(&self, context: ContextRef) -> Result<PhysicalIndexMergeReader, CacheSnapshotError> {
        Ok(PhysicalIndexMergeReader {
            PhysicalSchemaProducer: self.producer.restore(context.clone())?,
            IsIntersectionType: self.intersection,
            AccessMVIndex: self.access_mv_index,
            PushedLimit: self.pushed_limit,
            ByItems: restore_by_items(&self.by_items, context.GetExprCtx())?,
            PartialPlansRaw: self
                .partial_plans
                .iter()
                .map(|plan| plan.restore(context.clone()))
                .collect::<Result<_, _>>()?,
            TablePlan: self
                .table_plan
                .as_deref()
                .map(|plan| plan.restore(context.clone()))
                .transpose()?,
            PlanPartInfo: self
                .partition
                .as_ref()
                .map(|info| info.restore(context.GetExprCtx()))
                .transpose()?,
            KeepOrder: self.keep_order,
        })
    }
}

#[derive(Clone)]
pub struct CachedLocalIndexLookup {
    value: crate::physical_indexlookup::PhysicalLocalIndexLookup,
}

impl CachedLocalIndexLookup {
    pub fn capture(value: &crate::physical_indexlookup::PhysicalLocalIndexLookup) -> Self {
        Self {
            value: value.clone(),
        }
    }

    pub fn restore(&self) -> crate::physical_indexlookup::PhysicalLocalIndexLookup {
        self.value.clone()
    }
}

/// Snapshot of `BasePhysicalPlan`, including ordered children and probe parents.
#[derive(Clone)]
pub struct CachedPlanBase {
    context: CachedContext,
    children_req_props: Vec<CachedPhysicalProperty>,
    children: Vec<CachedPlan>,
    plan_cost_init: bool,
    plan_cost: f64,
    plan_cost_ver2: Option<costusage::CostVer2>,
    probe_parents: Vec<CachedPlan>,
    tiflash_fine_grained_shuffle_stream_count: u64,
    schema: expression::CachedSchema,
    stats: CachedStats,
    stats_table_name: Option<String>,
    store_type: Option<kv::StoreType>,
}

impl CachedPlanBase {
    pub fn try_from_base(base: &BasePhysicalPlan) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            context: CachedContext::capture(&base.Plan),
            children_req_props: base
                .cache_children_req_props()
                .iter()
                .map(|property| CachedPhysicalProperty::capture(property))
                .collect::<Result<_, _>>()?,
            children: base
                .Children()
                .into_iter()
                .map(CachedPlan::try_capture)
                .collect::<Result<_, _>>()?,
            plan_cost_init: base.PlanCostInit,
            plan_cost: base.PlanCost,
            plan_cost_ver2: base.cache_plan_cost_ver2().cloned(),
            probe_parents: base
                .cache_probe_parents()
                .iter()
                .map(|parent| CachedPlan::try_capture(parent.as_ref()))
                .collect::<Result<_, _>>()?,
            tiflash_fine_grained_shuffle_stream_count: base.TiFlashFineGrainedShuffleStreamCount,
            schema: expression::CachedSchema::try_from_schema(base.cache_schema())
                .map_err(|error| unsupported(error.to_string()))?,
            stats: CachedStats(base.cache_stats().clone()),
            stats_table_name: base.cache_stats_table_name().map(str::to_owned),
            store_type: base.StoreType(),
        })
    }

    pub fn restore(&self, context: ContextRef) -> Result<BasePhysicalPlan, CacheSnapshotError> {
        let mut base = BasePhysicalPlan::New(context.clone(), "", 0);
        base.Plan = self.context.restore(context.clone());
        base.SetChildrenReqProps(
            self.children_req_props
                .iter()
                .map(|property| property.restore(context.GetExprCtx()).map(Box::new))
                .collect::<Result<_, _>>()?,
        );
        base.SetChildren(
            self.children
                .iter()
                .map(|child| child.restore(context.clone()))
                .collect::<Result<_, _>>()?,
        );
        base.PlanCostInit = self.plan_cost_init;
        base.PlanCost = self.plan_cost;
        base.TiFlashFineGrainedShuffleStreamCount = self.tiflash_fine_grained_shuffle_stream_count;
        base.SetStoreType(self.store_type);
        let schema = self
            .schema
            .restore(context.GetExprCtx())
            .map_err(|error| unsupported(error.to_string()))?;
        let probe_parents = self
            .probe_parents
            .iter()
            .map(|parent| parent.restore(context.clone()))
            .collect::<Result<_, _>>()?;
        base.restore_cached_fields(
            self.plan_cost_ver2.clone(),
            self.stats.0.clone(),
            self.stats_table_name.clone(),
            schema,
            probe_parents,
        );
        Ok(base)
    }
}

/// Snapshot of the schema-producing physical-plan base.
#[derive(Clone)]
pub struct CachedSchemaProducer {
    base: CachedPlanBase,
    schema: Option<expression::CachedSchema>,
}

impl CachedSchemaProducer {
    pub fn try_from_producer(
        producer: &PhysicalSchemaProducer,
    ) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            base: CachedPlanBase::try_from_base(&producer.BasePhysicalPlan)?,
            schema: producer
                .SchemaRef()
                .map(expression::CachedSchema::try_from_schema)
                .transpose()
                .map_err(|error| unsupported(error.to_string()))?,
        })
    }

    pub fn restore(
        &self,
        context: ContextRef,
    ) -> Result<PhysicalSchemaProducer, CacheSnapshotError> {
        let mut producer = PhysicalSchemaProducer::New(self.base.restore(context.clone())?);
        if let Some(schema) = &self.schema {
            producer.SetSchema(
                schema
                    .restore(context.GetExprCtx())
                    .map_err(|error| unsupported(error.to_string()))?,
            );
        }
        Ok(producer)
    }
}
