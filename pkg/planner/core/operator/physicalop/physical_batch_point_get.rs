// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// PointGet / BatchPointGet 物理计划：按主键 handle 或唯一索引值做点查（Point Lookup），
// 批量版本一次携带多组 handle/索引值，避免多次单点计划。

use base::{ContextRef, PhysicalPlan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::{Column, CorrelatedColumn, ExprBox, Schema};
use model::{ColumnInfo, IndexInfo, TableInfo};
use types::datum::Datum;
use types::metadata::NameSlice;

use crate::{BasePhysicalPlan, PhysicalSchemaProducer, RootTask};

/// 单点查询物理计划：指定表、可选分区、handle 或索引等值条件。
pub struct PointGetPlan {
    pub(crate) output_names: NameSlice,
    /// Schema 与基类物理计划。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 库名。
    pub DBName: String,
    /// 目标表元信息。
    pub TblInfo: Option<TableInfo>,
    /// 唯一索引元信息；None 表示走主键/handle。
    pub IndexInfo: Option<IndexInfo>,
    /// 命中的分区下标；非分区表为 None。
    pub PartitionIdx: Option<usize>,
    /// 行句柄（handle），主键点查时使用。
    pub Handle: Option<i64>,
    /// 索引等值条件对应的 Datum 值列表。
    pub IndexValues: Vec<Datum>,
    /// 索引列表达式列。
    pub IdxCols: Vec<Column>,
    /// 前缀索引长度；-1 或全长表示完整列。
    pub IdxColLens: Vec<i32>,
    /// 访问条件表达式，用于相关列提取与索引解析。
    pub AccessConditions: Vec<ExprBox>,
    /// handle 是否按无符号解释。
    pub UnsignedHandle: bool,
    /// 条件恒假时退化为 TableDual，代价为 0。
    pub IsTableDual: bool,
    /// 是否加锁读取（如 SELECT FOR UPDATE）。
    pub Lock: bool,
    /// 锁等待时间（毫秒语义与会话变量一致）。
    pub LockWaitTime: i64,
    /// 需要读出的列元信息。
    pub Columns: Vec<ColumnInfo>,
    /// 实际访问的列，用于行宽估算。
    pub AccessColumns: Vec<Column>,
    /// 缓存的点查代价。
    pub CostValue: f64,
}

impl PointGetPlan {
    /// 创建空的 TypePointGet 计划。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            output_names: NameSlice(Vec::new()),
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypePointGet,
                0,
            )),
            DBName: String::new(),
            TblInfo: None,
            IndexInfo: None,
            PartitionIdx: None,
            Handle: None,
            IndexValues: Vec::new(),
            IdxCols: Vec::new(),
            IdxColLens: Vec::new(),
            AccessConditions: Vec::new(),
            UnsignedHandle: false,
            IsTableDual: false,
            Lock: false,
            LockWaitTime: 0,
            Columns: Vec::new(),
            AccessColumns: Vec::new(),
            CostValue: 0.0,
        }
    }

    /// 安装基类计划类型、查询块偏移与统计信息。
    pub fn Init(mut self, ctx: ContextRef, stats: property::StatsInfo, offset: i32) -> Self {
        self.PhysicalSchemaProducer.BasePhysicalPlan =
            BasePhysicalPlan::New(ctx, plancodec::TypePointGet, offset);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        self
    }

    /// 深克隆 schema、表/索引元数据与访问表达式。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut producer = PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(new_ctx)?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            output_names: self.output_names.Shallow(),
            PhysicalSchemaProducer: producer,
            DBName: self.DBName.clone(),
            TblInfo: self.TblInfo.as_ref().map(TableInfo::Clone),
            IndexInfo: self.IndexInfo.as_ref().map(IndexInfo::Clone),
            PartitionIdx: self.PartitionIdx,
            Handle: self.Handle,
            IndexValues: self.IndexValues.clone(),
            IdxCols: self.IdxCols.iter().map(Column::Clone).collect(),
            IdxColLens: self.IdxColLens.clone(),
            AccessConditions: self
                .AccessConditions
                .iter()
                .map(|e| e.CloneExpr())
                .collect(),
            UnsignedHandle: self.UnsignedHandle,
            IsTableDual: self.IsTableDual,
            Lock: self.Lock,
            LockWaitTime: self.LockWaitTime,
            Columns: self.Columns.clone(),
            AccessColumns: self.AccessColumns.iter().map(Column::Clone).collect(),
            CostValue: self.CostValue,
        })
    }

    /// 输出 Schema。
    pub fn Schema(&self) -> &Schema {
        base::Plan::schema(self)
    }
    /// 设置输出 Schema。
    pub fn SetSchema(&mut self, schema: Schema) {
        self.PhysicalSchemaProducer.SetSchema(schema)
    }
    /// 统计信息。
    pub fn StatsInfo(&self) -> &property::StatsInfo {
        base::Plan::stats_info(self)
    }
    /// 估计行数。
    pub fn StatsCount(&self) -> f64 {
        1.0
    }
    /// 会话/计划上下文。
    pub fn GetCtx(&self) -> &ContextRef {
        base::Plan::s_ctx(self)
    }
    /// 替换计划上下文。
    pub fn SetCtx(&mut self, ctx: ContextRef) {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetSCtx(ctx);
    }
    /// 输出列名。
    pub fn OutputNames(&self) -> NameSlice {
        self.output_names.Shallow()
    }
    /// 设置输出列名。
    pub fn SetOutputNames(&mut self, names: NameSlice) {
        self.output_names = names;
    }
    /// 访问列切片。
    pub fn AccessCols(&self) -> &[Column] {
        &self.AccessColumns
    }
    /// 设置访问列。
    pub fn SetAccessCols(&mut self, columns: Vec<Column>) {
        self.AccessColumns = columns
    }
    /// 读取缓存代价。
    pub fn Cost(&self) -> f64 {
        self.CostValue
    }
    /// 写入缓存代价。
    pub fn SetCost(&mut self, cost: f64) {
        self.CostValue = cost
    }
    /// 子计划（点查通常无孩子）。
    pub fn Children(&self) -> Vec<&dyn PhysicalPlan> {
        base::PhysicalPlan::children(self)
    }
    /// 设置子计划。
    pub fn SetChildren(&mut self, children: Vec<Box<dyn PhysicalPlan>>) {
        base::PhysicalPlan::set_children(self, children)
    }
    /// 替换指定位置子计划。
    pub fn SetChild(&mut self, index: usize, child: Box<dyn PhysicalPlan>) {
        base::PhysicalPlan::set_child(self, index, child)
    }
    /// 子节点物理属性要求。
    pub fn GetChildReqProps(&self, index: usize) -> &property::PhysicalProperty {
        base::PhysicalPlan::get_child_req_props(self, index)
    }
    /// 设置探测侧父节点（运行时统计用）。
    pub fn SetProbeParents(&mut self, parents: Vec<Box<dyn PhysicalPlan>>) {
        base::PhysicalPlan::set_probe_parents(self, parents)
    }
    /// EXPLAIN 展示用估计行数。
    pub fn GetEstRowCountForDisplay(&self) -> f64 {
        base::PhysicalPlan::get_est_row_count_for_display(self)
    }
    /// 实际探测次数。
    pub fn GetActualProbeCnt(&self, stats: &execdetails::execdetails::RuntimeStatsColl) -> i64 {
        base::PhysicalPlan::get_actual_probe_count(self, stats)
    }

    /// 有效代价：TableDual 为 0，否则至少为 1。
    pub fn GetCost(&self) -> f64 {
        if self.IsTableDual {
            0.0
        } else {
            self.CostValue.max(1.0)
        }
    }

    /// EXPLAIN 明细（非规范化）。
    pub fn ExplainInfo(&self) -> String {
        let operator = self.OperatorInfo(false);
        if operator.is_empty() {
            self.AccessObject()
        } else {
            format!("{}, {operator}", self.AccessObject())
        }
    }
    /// 规范化 EXPLAIN（隐藏具体 handle）。
    pub fn ExplainNormalizedInfo(&self) -> String {
        let operator = self.OperatorInfo(true);
        if operator.is_empty() {
            self.AccessObject()
        } else {
            format!("{}, {operator}", self.AccessObject())
        }
    }
    /// 组装表名与 handle/索引值说明。
    pub fn OperatorInfo(&self, normalized: bool) -> String {
        let Some(handle) = self.Handle else {
            return if self.Lock {
                "lock".to_owned()
            } else {
                String::new()
            };
        };
        let handle = if normalized {
            "?".to_owned()
        } else if self.UnsignedHandle {
            (handle as u64).to_string()
        } else {
            handle.to_string()
        };
        if self.Lock {
            format!("handle:{handle}, lock")
        } else {
            format!("handle:{handle}")
        }
    }
    /// 访问对象描述：表、可选分区与可选索引。
    pub fn AccessObject(&self) -> String {
        let mut parts = vec![format!(
            "table:{}",
            self.TblInfo
                .as_ref()
                .map(|table| table.Name.O.as_str())
                .unwrap_or("unknown")
        )];
        if let (Some(table), Some(partition_index)) = (&self.TblInfo, self.PartitionIdx) {
            let partition = table
                .Partition
                .as_ref()
                .and_then(|partition| partition.Definitions.get(partition_index));
            parts.push(format!(
                "partition:{}",
                partition
                    .map(|definition| definition.Name.O.as_str())
                    .unwrap_or("dual")
            ));
        }
        if let Some(index) = &self.IndexInfo {
            parts.push(format!("index:{}", index.Name.O));
        }
        parts.join(", ")
    }
    /// 粗略平均行宽：访问列数 × 8。
    pub fn GetAvgRowSize(&self) -> f64 {
        self.AccessColumns.len() as f64 * 8.0
    }
    /// 点查路径预留的表统计加载钩子（当前为空）。
    pub fn LoadTableStats(&self) {}
    /// 从访问条件中提取相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        Vec::new()
    }
    /// 相对自身 Schema 重解访问条件列索引。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        let mut schema = self.Schema().Clone();
        let schema_snapshot = schema.Clone();
        crate::ResolveIndicesForVirtualColumn(&mut schema.Columns, &schema_snapshot)?;
        self.SetSchema(schema);
        Ok(())
    }
    /// 克隆自身并挂到 RootTask。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        let plan = self
            .clone_physical(self.GetCtx().clone())
            .expect("point get clone");
        Box::new(RootTask::New(plan, tasks.into_iter().next()))
    }
    /// 代价模型 v1。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }
    /// 代价模型 v2。
    pub fn GetPlanCostVer2(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task, option, inl)
    }
    /// 编码为 TableScan 形态的 tipb.Executor（点查下推占位）。
    pub fn ToPB(
        &self,
        _ctx: &mut base::BuildPBContext,
        _store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let mut scan = tipb::TableScan::new();
        scan.set_table_id(self.TblInfo.as_ref().map_or(0, |table| table.ID));
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeTableScan);
        executor.set_tbl_scan(scan);
        Ok(Box::new(executor))
    }
    /// 估算内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self.DBName.len() as i64
            + self.IndexValues.iter().map(Datum::MemUsage).sum::<i64>()
            + self.IdxCols.iter().map(Column::MemoryUsage).sum::<i64>()
            + self
                .AccessColumns
                .iter()
                .map(Column::MemoryUsage)
                .sum::<i64>()
            + self
                .AccessConditions
                .iter()
                .map(|e| e.MemoryUsage())
                .sum::<i64>()
    }
}

/// Borrowed session and table boundaries for batch lookup pruning. Common handles
/// are returned to the executor, rather than persisted in the integer plan keys.
pub struct BatchPointGetPruningContext<'a> {
    pub StatementContext: &'a stmtctx::StatementContext,
    pub EvalContext: &'a dyn expression::EvalContext,
    pub PartitionedTable: Option<&'a dyn table::PartitionedTable>,
    pub SinglePartition: Option<usize>,
    pub PartitionNames: &'a [parser_ast::CIStr],
    pub HandleColOffset: usize,
}

impl table::CastContext for BatchPointGetPruningContext<'_> {
    fn TypeCtx(&self) -> expression::types::Context {
        self.EvalContext.TypeCtx()
    }
    fn ErrCtx(&self) -> expression::errctx::Context {
        self.EvalContext.ErrCtx()
    }
    fn SQLMode(&self) -> mysql::r#const::SQLMode {
        self.EvalContext.SQLMode()
    }
    fn ConnectionID(&self) -> u64 {
        self.EvalContext.CtxID()
    }
}

impl BatchPointGetPruningContext<'_> {
    fn encode_unique_values(
        &self,
        table: &TableInfo,
        index: &IndexInfo,
        values: &mut [Datum],
    ) -> Result<Option<Vec<u8>>, expression::Error> {
        if values.len() != index.Columns.len() {
            return Err(expression::errors::New("unique index value count mismatch"));
        }
        for (value, index_column) in values.iter_mut().zip(&index.Columns) {
            let column = table
                .Columns
                .get(index_column.Offset as usize)
                .ok_or_else(|| expression::errors::New("index column offset out of range"))?;
            match column.GetType() {
                mysql::r#type::TypeString
                | mysql::r#type::TypeVarString
                | mysql::r#type::TypeVarchar => {
                    let text = value.ToString()?;
                    value.SetString(text, value.Collation());
                }
                mysql::r#type::TypeEnum
                    if matches!(
                        value.Kind(),
                        types::datum::KindString
                            | types::datum::KindBytes
                            | types::datum::KindBinaryLiteral
                    ) =>
                {
                    let Ok(text) = value.ToString() else {
                        return Ok(None);
                    };
                    let Ok(parsed) = expression::types::ParseEnumName(
                        column.GetElems(),
                        &text,
                        column.GetCollate(),
                    ) else {
                        return Ok(None);
                    };
                    value.SetMysqlEnum(parsed, column.GetCollate().to_owned());
                }
                _ => match table::CastValue(self, value.clone(), column, true, false) {
                    Ok(converted) => *value = converted,
                    Err(error) if error.kind() == table::CastErrorKind::Truncated => {
                        return Ok(None);
                    }
                    Err(error) => return Err(expression::errors::New(error.to_string())),
                },
            }
        }
        match kv::codec::EncodeKey(
            self.StatementContext.TimeZone(),
            Vec::new(),
            values.to_vec(),
        ) {
            Ok(encoded) => Ok(Some(encoded)),
            Err(error) => match self.StatementContext.HandleError(Some(error)) {
                Some(error) => Err(expression::errors::New(error.to_string())),
                None => Ok(Some(Vec::new())),
            },
        }
    }
}

/// Stable in-place compaction drops rejected keys and keeps all parallel arrays
/// aligned. Moving rows releases their rejected allocations without suffix shifts.
pub(crate) fn compact_batch_partition_values(
    values: &mut Vec<Vec<Datum>>,
    handles: &mut Vec<Box<dyn kv::Handle>>,
    indexes: &[i32],
    output_indexes: &mut Vec<usize>,
    single_partition: Option<usize>,
    partition_info: &model::PartitionInfo,
    names: &[parser_ast::CIStr],
) -> usize {
    let mut position = 0;
    let has_handles = !handles.is_empty();
    for (source, &index) in indexes.iter().enumerate() {
        if index < 0
            || single_partition.is_some_and(|selected| selected != index as usize)
            || (!names.is_empty()
                && !partition_info
                    .Definitions
                    .get(index as usize)
                    .is_some_and(|definition| names.iter().any(|name| name.L == definition.Name.L)))
        {
            continue;
        }
        if !values.is_empty() {
            values.swap(position, source);
        }
        if has_handles {
            handles.swap(position, source);
        }
        if single_partition.is_none() {
            output_indexes.push(index as usize);
        }
        position += 1;
    }
    values.truncate(position);
    if has_handles {
        handles.truncate(position);
    }
    position
}

/// 批量点查：在 PointGetPlan 上叠加多组 handle / 索引值行 / 分区下标。
pub struct BatchPointGetPlan {
    /// 共享的单点计划配置与表元数据。
    pub PointGetPlan: PointGetPlan,
    /// 批量主键 handle 列表。
    pub Handles: Vec<i64>,
    /// 批量索引等值行。
    pub IndexValueRows: Vec<Vec<Datum>>,
    /// 与每行对应的分区下标。
    pub PartitionIdxs: Vec<usize>,
    /// Whether lookup results preserve key order.
    pub KeepOrder: bool,
    /// Whether preserved order is descending.
    pub Desc: bool,
    /// Whether rows are read with a lock.
    pub Lock: bool,
}

impl BatchPointGetPlan {
    /// 创建空批量点查。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PointGetPlan: PointGetPlan::New(ctx),
            Handles: Vec::new(),
            IndexValueRows: Vec::new(),
            PartitionIdxs: Vec::new(),
            KeepOrder: false,
            Desc: false,
            Lock: false,
        }
    }
    /// 初始化后把计划类型改为 TypeBatchPointGet。
    pub fn Init(mut self, ctx: ContextRef, stats: property::StatsInfo, offset: i32) -> Self {
        self.PointGetPlan = self.PointGetPlan.Init(ctx, stats, offset);
        self.PointGetPlan
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetTP(plancodec::TypeBatchPointGet);
        self
    }
    /// 克隆内嵌 PointGet 与批量键列表。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        Ok(Self {
            PointGetPlan: self.PointGetPlan.Clone(new_ctx)?,
            Handles: self.Handles.clone(),
            IndexValueRows: self.IndexValueRows.clone(),
            PartitionIdxs: self.PartitionIdxs.clone(),
            KeepOrder: self.KeepOrder,
            Desc: self.Desc,
            Lock: self.Lock,
        })
    }
    /// 仅保留允许分区的下标。
    pub fn PrunePartitions(&mut self, allowed: &[usize]) {
        self.PartitionIdxs.retain(|idx| allowed.contains(idx));
    }
    /// 按允许分区同步裁剪分区下标、handle 与索引值行。
    pub fn PrunePrecomputedPartitionsAndValues(&mut self, allowed: &[usize]) {
        let partitions = std::mem::take(&mut self.PartitionIdxs);
        let handles = std::mem::take(&mut self.Handles);
        let values = std::mem::take(&mut self.IndexValueRows);
        // 按原位置对齐三路切片，仅保留 allowed 中的分区及其对应键值。
        for (position, partition) in partitions.into_iter().enumerate() {
            if !allowed.contains(&partition) {
                continue;
            }
            self.PartitionIdxs.push(partition);
            if let Some(handle) = handles.get(position) {
                self.Handles.push(*handle);
            }
            if let Some(row) = values.get(position) {
                self.IndexValueRows.push(row.clone());
            }
        }
    }
    /// Session-aware Go pruning path. The legacy precomputed-partition method is
    /// retained for callers that already own aligned integer keys.
    pub fn PrunePartitionsAndValues(
        &mut self,
        context: &BatchPointGetPruningContext<'_>,
    ) -> Result<(Vec<Box<dyn kv::Handle>>, bool), expression::Error> {
        use std::collections::HashSet;
        let table =
            self.PointGetPlan.TblInfo.as_ref().ok_or_else(|| {
                expression::errors::New("batch point get requires table metadata")
            })?;
        let index = self.PointGetPlan.IndexInfo.as_ref();
        let partition_info = if index.is_some_and(|index| index.Global) {
            None
        } else {
            table.GetPartitionInfo()
        };
        if partition_info.is_some() && context.SinglePartition.is_none() {
            self.PartitionIdxs.clear();
        }
        let common = table.IsCommonHandle && index.is_some_and(|index| index.Primary);
        let mut handles: Vec<Box<dyn kv::Handle>> = Vec::new();
        if let Some(index) = index {
            if common {
                let mut seen = HashSet::new();
                let mut position = 0;
                for source in 0..self.IndexValueRows.len() {
                    if self.IndexValueRows[source].iter().any(Datum::IsNull) {
                        continue;
                    }
                    let Some(encoded) = context.encode_unique_values(
                        table,
                        index,
                        &mut self.IndexValueRows[source],
                    )?
                    else {
                        continue;
                    };
                    let handle = kv::NewCommonHandle(encoded)
                        .map_err(|error| expression::errors::New(error.to_string()))?;
                    if !seen.insert(kv::Handle::Encoded(&handle)) {
                        continue;
                    }
                    handles.push(Box::new(handle));
                    self.IndexValueRows.swap(position, source);
                    position += 1;
                }
                self.IndexValueRows.truncate(position);
            } else {
                self.IndexValueRows
                    .retain(|row| !row.iter().any(Datum::IsNull));
            }
        } else {
            let mut seen = HashSet::new();
            handles.extend(
                self.Handles
                    .iter()
                    .filter(|handle| seen.insert(**handle))
                    .map(|handle| Box::new(kv::IntHandle(*handle)) as Box<dyn kv::Handle>),
            );
        }
        if let Some(partition_info) = partition_info {
            let partitioned_table = context.PartitionedTable.ok_or_else(|| {
                expression::errors::New("partitioned batch point get requires a partition table")
            })?;
            let mut indexes = Vec::new();
            let mut row =
                vec![Datum::default(); table.Columns.len().max(context.HandleColOffset + 1)];
            let count = if index.is_some() {
                self.IndexValueRows.len()
            } else {
                handles.len()
            };
            for source in 0..count {
                if let Some(index) = index {
                    for (value, column) in self.IndexValueRows[source].iter().zip(&index.Columns) {
                        row[column.Offset as usize] = value.clone();
                    }
                } else {
                    let column = table.Columns.get(context.HandleColOffset).ok_or_else(|| {
                        expression::errors::New("handle column offset out of range")
                    })?;
                    row[context.HandleColOffset] =
                        if mysql::r#type::HasUnsignedFlag(column.GetFlag()) {
                            types::datum::NewUintDatum(handles[source].IntValue() as u64)
                        } else {
                            types::datum::NewIntDatum(handles[source].IntValue())
                        };
                }
                let (index, error) =
                    match partitioned_table.GetPartitionIdxByRow(context.EvalContext, &row) {
                        Ok(index) => (index, None),
                        Err(error) => {
                            if self.PointGetPlan.IndexInfo.is_none()
                                && !table::ErrNoPartitionForGivenValue.Equal(Some(&error))
                            {
                                return Err(expression::errors::New(error.to_string()));
                            }
                            (-1, Some(model::errors::Errorf(error.to_string())))
                        }
                    };
                let (index, error) =
                    partition_info.ReplaceWithOverlappingPartitionIdx(index, error);
                indexes.push(if error.is_some() { -1 } else { index });
            }
            // Integer/secondary paths preserve their original values on the early
            // TableDual return; the common path always compacts both arrays first.
            let matches = indexes.iter().any(|&index| {
                index >= 0
                    && context
                        .SinglePartition
                        .is_none_or(|selected| selected == index as usize)
                    && (context.PartitionNames.is_empty()
                        || partition_info.Definitions.get(index as usize).is_some_and(
                            |definition| {
                                context
                                    .PartitionNames
                                    .iter()
                                    .any(|name| name.L == definition.Name.L)
                            },
                        ))
            });
            if !common && !matches {
                return Ok((Vec::new(), true));
            }
            let found = compact_batch_partition_values(
                &mut self.IndexValueRows,
                &mut handles,
                &indexes,
                &mut self.PartitionIdxs,
                context.SinglePartition,
                partition_info,
                context.PartitionNames,
            );
            if found == 0 {
                return Ok((Vec::new(), true));
            }
        }
        if index.is_none() {
            self.Handles = handles.iter().map(|handle| handle.IntValue()).collect();
        }
        Ok((handles, false))
    }
    /// EXPLAIN：批量大小 + 内嵌点查说明。
    pub fn ExplainInfo(&self) -> String {
        format!("{}, {}", self.AccessObject(), self.OperatorInfo(false))
    }
    /// 规范化说明固定为 batch point get。
    pub fn ExplainNormalizedInfo(&self) -> String {
        format!("{}, {}", self.AccessObject(), self.OperatorInfo(true))
    }
    /// 按是否规范化选择说明文本。
    pub fn OperatorInfo(&self, normalized: bool) -> String {
        let mut parts = Vec::new();
        if self.PointGetPlan.IndexInfo.is_none() {
            if normalized {
                parts.push("handle:?".to_owned());
            } else {
                let handles = self
                    .Handles
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(" ");
                parts.push(format!("handle:[{handles}]"));
            }
        }
        parts.push(format!("keep order:{}", self.KeepOrder));
        parts.push(format!("desc:{}", self.Desc));
        if self.Lock {
            parts.push("lock".to_owned());
        }
        parts.join(", ")
    }
    /// 访问对象委托内嵌点查。
    pub fn AccessObject(&self) -> String {
        let mut object = self.PointGetPlan.AccessObject();
        if let (Some(table), Some(partitions)) = (
            &self.PointGetPlan.TblInfo,
            self.PointGetPlan
                .TblInfo
                .as_ref()
                .and_then(|table| table.Partition.as_ref()),
        ) {
            let mut indexes = self.PartitionIdxs.clone();
            indexes.sort_unstable();
            indexes.dedup();
            let names = indexes
                .into_iter()
                .filter_map(|index| partitions.Definitions.get(index))
                .map(|definition| definition.Name.O.as_str())
                .collect::<Vec<_>>();
            if !names.is_empty() {
                let table_prefix = format!("table:{}", table.Name.O);
                object = object.replacen(
                    &table_prefix,
                    &format!("{table_prefix}, partition:{}", names.join(",")),
                    1,
                );
            }
        }
        object
    }
    /// 相关列委托内嵌点查。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        Vec::new()
    }
    /// 索引解析委托内嵌点查。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PointGetPlan.ResolveIndices()
    }
    /// 平均行宽委托内嵌点查。
    pub fn GetAvgRowSize(&self) -> f64 {
        self.PointGetPlan.GetAvgRowSize()
    }
    /// 代价 ≈ 单点代价 × 批量基数。
    pub fn GetCost(&self) -> f64 {
        self.PointGetPlan.GetCost() * self.Handles.len().max(self.IndexValueRows.len()) as f64
    }
    /// 代价 v1 委托。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PointGetPlan.GetPlanCostVer1(task, option)
    }
    /// 代价 v2 委托。
    pub fn GetPlanCostVer2(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.PointGetPlan.GetPlanCostVer2(task, option, inl)
    }
    /// PB 编码委托内嵌点查。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.PointGetPlan.ToPB(ctx, store)
    }
    /// 内存：内嵌计划 + handle 容量 + 索引值 Datum。
    pub fn MemoryUsage(&self) -> i64 {
        self.PointGetPlan.MemoryUsage()
            + self.Handles.capacity() as i64 * 8
            + self
                .IndexValueRows
                .iter()
                .flatten()
                .map(Datum::MemUsage)
                .sum::<i64>()
    }
}
