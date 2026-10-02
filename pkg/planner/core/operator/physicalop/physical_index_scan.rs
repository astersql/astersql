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

// 物理算子：索引扫描（PhysicalIndexScan）。
//
// 在存储层按索引范围（Range）读取索引行；可全表索引扫描或范围扫描。
// DoubleRead 表示需要回表取完整行。计划缓存路径会重算范围并校验配额。

use base::{ContextRef, PhysicalPlan, Plan};
use costusage::{CostVer2, PlanCostOption};
use expression::{Column, CorrelatedColumn, ExprBox, Schema};
use model::{ColumnInfo, IndexInfo, TableInfo};
use property::{HistCollRef, PhysicalProperty, StatsInfo, TaskType};

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// 物理索引扫描：访问/过滤条件、表与索引元信息、范围与直方图等。
pub struct PhysicalIndexScan {
    /// Schema 与基座计划。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 可下推为索引范围的访问条件。
    pub AccessCondition: Vec<ExprBox>,
    /// 扫描后仍需过滤的残余谓词。
    pub FilterCondition: Vec<ExprBox>,
    /// 表元信息。
    pub Table: Option<TableInfo>,
    /// 索引元信息。
    pub Index: Option<IndexInfo>,
    /// 索引列表达式。
    pub IdxCols: Vec<Column>,
    /// 索引列前缀长度。
    pub IdxColLens: Vec<i32>,
    /// 当前索引扫描范围集合。
    pub Ranges: ranger::Ranges,
    /// 涉及的列信息。
    pub Columns: Vec<ColumnInfo>,
    /// 库名。
    pub DBName: String,
    /// 表别名。
    pub TableAsName: String,
    /// 数据源侧 Schema（列裁剪前参考）。
    pub DataSourceSchema: Option<Schema>,
    /// 范围来源说明（EXPLAIN 用）。
    pub RangeInfo: String,
    /// 物理表/分区 ID。
    pub PhysicalTableID: i64,
    /// 是否扫描分区表的某一物理分区。
    pub IsPartition: bool,
    /// 是否降序扫描。
    pub Desc: bool,
    /// 是否要求输出保持索引序。
    pub KeepOrder: bool,
    /// 是否需要二次回表（Double Read）取完整行。
    pub DoubleRead: bool,
    /// 是否需要公共句柄（Common Handle，聚簇索引主键）。
    pub NeedCommonHandle: bool,
    /// 表列直方图集合引用。
    pub TblColHists: Option<HistCollRef>,
    /// 整数主键句柄列（若适用）。
    pub PKIsHandleCol: Option<Column>,
    /// 各索引列是否被条件固定为常量。
    pub ConstColsByCond: Vec<bool>,
    /// 满足的物理属性（排序等）。
    pub Prop: Option<PhysicalProperty>,
}

impl PhysicalIndexScan {
    /// 创建默认空扫描节点。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeIdxScan,
                0,
            )),
            AccessCondition: Vec::new(),
            FilterCondition: Vec::new(),
            Table: None,
            Index: None,
            IdxCols: Vec::new(),
            IdxColLens: Vec::new(),
            Ranges: ranger::Ranges::default(),
            Columns: Vec::new(),
            DBName: String::new(),
            TableAsName: String::new(),
            DataSourceSchema: None,
            RangeInfo: String::new(),
            PhysicalTableID: 0,
            IsPartition: false,
            Desc: false,
            KeepOrder: false,
            DoubleRead: false,
            NeedCommonHandle: false,
            TblColHists: None,
            PKIsHandleCol: None,
            ConstColsByCond: Vec::new(),
            Prop: None,
        }
    }

    /// 按偏移重设基座计划。
    pub fn Init(mut self, ctx: ContextRef, offset: i32) -> Self {
        let plan = &mut self.PhysicalSchemaProducer.BasePhysicalPlan;
        plan.Plan.SetSCtx(ctx);
        plan.SetTP(plancodec::TypeIdxScan);
        plan.Plan.SetQueryBlockOffset(offset);
        self
    }

    /// 深拷贝并切换上下文。
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
            PhysicalSchemaProducer: producer,
            AccessCondition: self.AccessCondition.iter().map(|e| e.CloneExpr()).collect(),
            FilterCondition: self.FilterCondition.iter().map(|e| e.CloneExpr()).collect(),
            Table: self.Table.as_ref().map(TableInfo::Clone),
            Index: self.Index.as_ref().map(IndexInfo::Clone),
            IdxCols: self.IdxCols.iter().map(Column::Clone).collect(),
            IdxColLens: self.IdxColLens.clone(),
            Ranges: self.Ranges.clone(),
            Columns: self.Columns.clone(),
            DBName: self.DBName.clone(),
            TableAsName: self.TableAsName.clone(),
            DataSourceSchema: self.DataSourceSchema.as_ref().map(Schema::Clone),
            RangeInfo: self.RangeInfo.clone(),
            PhysicalTableID: self.PhysicalTableID,
            IsPartition: self.IsPartition,
            Desc: self.Desc,
            KeepOrder: self.KeepOrder,
            DoubleRead: self.DoubleRead,
            NeedCommonHandle: self.NeedCommonHandle,
            TblColHists: self.TblColHists.clone(),
            PKIsHandleCol: self.PKIsHandleCol.as_ref().map(Column::Clone),
            ConstColsByCond: self.ConstColsByCond.clone(),
            Prop: self.Prop.clone(),
        })
    }

    /// 从访问/过滤条件抽取相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.AccessCondition
            .iter()
            .flat_map(|expr| expression::ExtractCorColumns(expr.as_ref()))
            .map(CorrelatedColumn::Clone)
            .collect()
    }

    /// 按索引列构造输出 Schema，并记录是否回表与 Common Handle。
    pub fn InitSchema(&mut self, index_columns: &[Option<Column>], double_read: bool) {
        self.DoubleRead = double_read;
        let mut columns = self.IdxCols.iter().map(Column::Clone).collect::<Vec<_>>();
        if let (Some(table), Some(index)) = (&self.Table, &self.Index) {
            for (position, index_column) in index.Columns.iter().enumerate().skip(columns.len()) {
                if let Some(column) = index_columns.get(position).and_then(Option::as_ref) {
                    columns.push(column.Clone());
                    continue;
                }
                if let Some(info) = table.Columns.get(index_column.Offset as usize) {
                    columns.push(Column::new(
                        info.FieldType.clone(),
                        info.ID,
                        self.PhysicalSchemaProducer
                            .BasePhysicalPlan
                            .s_ctx()
                            .GetSessionVars()
                            .AllocPlanColumnID(),
                        columns.len() as isize,
                    ));
                }
            }
            self.NeedCommonHandle = table.IsCommonHandle;

            if self.NeedCommonHandle && self.IdxCols.len() <= index.Columns.len() {
                columns.extend(
                    index_columns
                        .iter()
                        .skip(index.Columns.len())
                        .filter_map(Option::as_ref)
                        .map(Column::Clone),
                );
            }

            let mut has_handle = columns.len() > index.Columns.len();
            if !has_handle {
                for column_info in &self.Columns {
                    let is_handle = (mysql::r#type::HasPriKeyFlag(column_info.GetFlag())
                        && table.PKIsHandle)
                        || column_info.ID == model::ExtraHandleID;
                    if !is_handle {
                        continue;
                    }
                    if let Some(handle) = self.DataSourceSchema.as_ref().and_then(|schema| {
                        schema.Columns.iter().find(|col| col.ID == column_info.ID)
                    }) {
                        columns.push(handle.Clone());
                        has_handle = true;
                        break;
                    }
                }
            }

            let extra_physical_table_column = self.DataSourceSchema.as_ref().and_then(|schema| {
                schema
                    .Columns
                    .iter()
                    .find(|column| column.ID == model::ExtraPhysTblID)
                    .map(Column::Clone)
            });
            if double_read || index.Global {
                if !has_handle && !table.IsCommonHandle {
                    let info = model::NewExtraHandleColInfo();
                    let mut handle = Column::new(
                        info.FieldType,
                        info.ID,
                        self.s_ctx().GetSessionVars().AllocPlanColumnID(),
                        columns.len() as isize,
                    );
                    handle.OrigName = info.Name.O;
                    columns.push(handle);
                }
                if index.Global && extra_physical_table_column.is_none() {
                    let info = model::NewExtraPhysTblIDColInfo();
                    let mut physical_table = Column::new(
                        info.FieldType,
                        info.ID,
                        self.s_ctx().GetSessionVars().AllocPlanColumnID(),
                        columns.len() as isize,
                    );
                    physical_table.OrigName = info.Name.O;
                    columns.push(physical_table);
                }
            }
            if let Some(extra) = extra_physical_table_column {
                columns.push(extra);
            }
        }
        self.PhysicalSchemaProducer
            .SetSchema(expression::NewSchema(columns));
    }

    /// 访问对象：table/index 名称。
    pub fn AccessObject(&self) -> String {
        self.access_object(false)
    }

    fn access_object(&self, normalized: bool) -> String {
        let Some(table_info) = self.Table.as_ref() else {
            return "table:unknown".to_owned();
        };
        let table = if self.TableAsName.is_empty() {
            table_info.Name.O.as_str()
        } else {
            self.TableAsName.as_str()
        };
        let mut object = format!("table:{table}");
        if self.IsPartition {
            let partition = if normalized {
                "?".to_owned()
            } else {
                table_info
                    .Partition
                    .as_ref()
                    .map_or_else(String::new, |info| info.GetNameByID(self.PhysicalTableID))
            };
            if !partition.is_empty() {
                object.push_str(", partition:");
                object.push_str(&partition);
            }
        }
        if let Some(index) = self
            .Index
            .as_ref()
            .filter(|index| !index.Columns.is_empty())
        {
            let columns = index
                .Columns
                .iter()
                .map(|index_column| {
                    let table_column = &table_info.Columns[index_column.Offset as usize];
                    if table_column.Hidden {
                        table_column.GeneratedExprString.clone()
                    } else {
                        index_column.Name.O.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            object.push_str(", index:");
            object.push_str(&index.Name.O);
            object.push('(');
            object.push_str(&columns);
            object.push(')');
        }
        object
    }

    /// 算子类型：全索引扫描或范围扫描。
    pub fn TP(&self) -> String {
        if self.IsFullScan() {
            plancodec::TypeIndexFullScan.to_owned()
        } else {
            plancodec::TypeIndexRangeScan.to_owned()
        }
    }

    /// EXPLAIN 节点 ID：类型_数字ID。
    pub fn ExplainID(&self) -> String {
        format!("{}_{}", self.TP(), base::Plan::id(self))
    }

    /// 算子详情：范围、保序与升降序。
    pub fn OperatorInfo(&self, normalized: bool) -> String {
        let mut info = String::new();
        if !self.RangeInfo.is_empty() {
            if !normalized {
                info.push_str("range: decided by ");
                info.push_str(&self.RangeInfo);
                info.push_str(", ");
            }
        } else if !self.ExtractCorrelatedCols().is_empty() {
            info.push_str("range: decided by ");
            if normalized {
                info.push_str(&String::from_utf8_lossy(
                    &expression::SortedExplainNormalizedExpressionList(&self.AccessCondition),
                ));
            } else {
                let eval_ctx = self.s_ctx().GetExprCtx().GetEvalCtx();
                info.push('[');
                info.push_str(
                    &self
                        .AccessCondition
                        .iter()
                        .map(|condition| {
                            condition.StringWithCtx(Some(eval_ctx), &eval_ctx.GetTiDBRedactLog())
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                info.push(']');
            }
            info.push_str(", ");
        } else if !self.Ranges.is_empty() {
            if normalized {
                info.push_str("range:[?,?], ");
            } else if !self.IsFullScan() {
                let redact = self.s_ctx().GetExprCtx().GetEvalCtx().GetTiDBRedactLog();
                info.push_str("range:");
                for range in &self.Ranges {
                    info.push_str(&range.Redact(&redact));
                    info.push_str(", ");
                }
            }
        }
        info.push_str(&format!("keep order:{}", self.KeepOrder));
        if self.Desc {
            info.push_str(", desc");
        }
        info
    }

    /// 详细 EXPLAIN。
    pub fn ExplainInfo(&self) -> String {
        format!("{}, {}", self.AccessObject(), self.OperatorInfo(false))
    }

    /// 归一化 EXPLAIN。
    pub fn ExplainNormalizedInfo(&self) -> String {
        format!("{}, {}", self.access_object(true), self.OperatorInfo(true))
    }

    /// 判断是否等价于全索引扫描（无有效范围收窄）。
    pub fn IsFullScan(&self) -> bool {
        self.RangeInfo.is_empty()
            && self.ExtractCorrelatedCols().is_empty()
            && (self.Ranges.is_empty() || self.Ranges.iter().all(|range| range.IsFullRange(false)))
    }

    /// 用执行上下文重算参数标记并重建完整索引范围；配额刻意传 0，避免缓存命中因更宽值退化成全范围。
    /// Re-evaluates cached parameter markers with the execution context and
    /// rebuilds the complete index range. Go deliberately passes a zero quota
    /// here: a cache hit must not degrade to a full range because the values of
    /// the current EXECUTE are wider than those used to populate the cache.
    /// 按给定 range_max_size 配额分离条件并构建索引范围。
    fn build_ranges_with_quota(
        &self,
        range_max_size: i64,
    ) -> Result<ranger::Ranges, expression::Error> {
        // 范围回退可能把后缀索引谓词从访问条件挪到过滤；缓存准入重建必须看完整谓词集。
        // Range fallback can move a suffix-index predicate from access
        // conditions to filters. A cache-admission rebuild must reconsider the
        // complete predicate set; rebuilding only the already-truncated access
        // list cannot observe that the original plan exceeded the quota.
        let conditions = self
            .AccessCondition
            .iter()
            .chain(self.FilterCondition.iter())
            .map(|condition| condition.CloneExpr())
            .collect();
        let detached = ranger::DetachCondAndBuildRangeForIndex(
            self.s_ctx().GetRangerCtx(),
            conditions,
            self.IdxCols.iter().map(Column::Clone).collect(),
            self.IdxColLens.clone(),
            range_max_size,
        )?;
        Ok(detached.Ranges)
    }

    /// 计划缓存路径：以配额 0 重建完整范围。
    pub fn RebuildRangesForPlanCache(&self) -> Result<ranger::Ranges, expression::Error> {
        self.build_ranges_with_quota(0)
    }

    /// 将范围格式化为 EXPLAIN 用字符串（非 SQL 谓词还原）。
    fn ranges_to_string(&self, ranges: &ranger::Ranges) -> Result<String, expression::Error> {
        // 物理扫描 EXPLAIN 使用 Range.String；勿用 RangesToString（那是 SQL 谓词还原）。
        // Physical scan explain output uses ranger.Range.String, matching Go's
        // `range:[low,high]` representation. RangesToString is deliberately a
        // different API: it restores ranges as SQL predicates and therefore
        // must not be used for cached-plan scan diagnostics or quota equality.
        Ok(ranges
            .iter()
            .map(ranger::Range::String)
            .collect::<Vec<_>>()
            .join(", "))
    }

    /// 计划缓存诊断：重建范围并格式化。
    pub fn PlanCacheRangeString(&self) -> Result<String, expression::Error> {
        let ranges = self.RebuildRangesForPlanCache()?;
        self.ranges_to_string(&ranges)
    }

    /// 若优化器配额导致回退范围而非完整可复用范围，则返回 false。
    /// Returns false when the configured optimizer quota produced a fallback
    /// range instead of the complete range required by a reusable plan.
    /// 比较限配额与完整范围字符串是否一致，判断是否仍适合进计划缓存。
    pub fn PlanCacheRangesFitQuota(&self, range_max_size: i64) -> Result<bool, expression::Error> {
        if range_max_size == 0 {
            return Ok(true);
        }
        let limited = self.build_ranges_with_quota(range_max_size)?;
        let complete = self.RebuildRangesForPlanCache()?;
        Ok(self.ranges_to_string(&limited)? == self.ranges_to_string(&complete)?)
    }

    /// 按重建后的范围决定缓存计划中的扫描类型名。
    pub fn PlanCacheTP(&self) -> Result<String, expression::Error> {
        let ranges = self.RebuildRangesForPlanCache()?;
        Ok(if self.RangeInfo.is_empty()
            && self.ExtractCorrelatedCols().is_empty()
            && (ranges.is_empty() || ranges.iter().all(|range| range.IsFullRange(false)))
        {
            plancodec::TypeIndexFullScan
        } else {
            plancodec::TypeIndexRangeScan
        }
        .to_owned())
    }

    /// 估算扫描行宽：列数 × 8。
    pub fn GetScanRowSize(&self) -> f64 {
        self.PhysicalSchemaProducer
            .SchemaRef()
            .map_or(0.0, |schema| schema.Len() as f64 * 8.0)
    }

    /// 分区表全局索引是否需要额外输出物理表 ID 列。
    pub fn NeedExtraOutputCol(&self) -> bool {
        self.Table
            .as_ref()
            .is_some_and(|table| table.Partition.is_some())
            && self.Index.as_ref().is_some_and(|index| index.Global)
    }

    /// 返回是否分区扫描及物理表 ID。
    pub fn IsPartitionTable(&self) -> (bool, i64) {
        (self.IsPartition, self.PhysicalTableID)
    }

    /// 唯一索引且单点非空范围时，可视为 Point Get。
    pub fn IsPointGetByUniqueKey(&self, ctx: types::Context) -> bool {
        self.Index.as_ref().is_some_and(|index| {
            index.Unique
                && self.Ranges.len() == 1
                && self.Ranges[0].LowVal.len() == index.Columns.len()
                && self.Ranges[0].IsPointNonNullable(ctx)
        })
    }

    /// 全局索引附加 Selection 条件；缺物理表 ID 输出列则报错。
    pub fn AddSelectionConditionForGlobalIndex(
        &self,
        conditions: Vec<ExprBox>,
    ) -> Result<Vec<ExprBox>, expression::Error> {
        if !self.Index.as_ref().is_some_and(|index| index.Global) {
            return Ok(conditions);
        }
        if !self.NeedExtraOutputCol() {
            return Err(expression::errors::New(
                "global index requires physical table id output column",
            ));
        }
        Ok(conditions)
    }

    /// 编码 tipb::IndexScan（表/索引 ID、唯一性、升降序）。
    pub fn ToPB(
        &self,
        _ctx: &mut base::BuildPBContext,
        _store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let table = self
            .Table
            .as_ref()
            .ok_or_else(|| expression::errors::New("index scan has no TableInfo"))?;
        let index = self
            .Index
            .as_ref()
            .ok_or_else(|| expression::errors::New("index scan has no IndexInfo"))?;
        let mut scan = tipb::IndexScan::new();
        // 分区扫描用物理分区 ID；否则用表 ID。
        scan.set_table_id(if self.IsPartition {
            self.PhysicalTableID
        } else {
            table.ID
        });
        scan.set_index_id(index.ID);
        scan.set_unique(
            index.Unique
                && self.Ranges.iter().all(|range| {
                    range.LowVal.len() == index.Columns.len()
                        && range.LowVal.iter().all(|value| !value.IsNull())
                        && range.HighVal.iter().all(|value| !value.IsNull())
                }),
        );
        let schema = self
            .PhysicalSchemaProducer
            .SchemaRef()
            .ok_or_else(|| expression::errors::New("index scan has no schema"))?;
        let mut columns = Vec::with_capacity(schema.Len());
        for column in &schema.Columns {
            let column_info = match column.ID {
                model::ExtraHandleID => model::NewExtraHandleColInfo(),
                model::ExtraPhysTblID => model::NewExtraPhysTblIDColInfo(),
                id => model::FindColumnInfoByID(&table.Columns, id)
                    .cloned()
                    .ok_or_else(|| {
                        expression::errors::New("index scan schema column missing from table")
                    })?,
            };
            let field_type = expression::ToPBFieldType(&column_info.FieldType);
            let mut encoded = tipb::ColumnInfo::new();
            encoded.set_column_id(column_info.ID);
            encoded.set_tp(field_type.get_tp());
            encoded.set_collation(field_type.get_collate());
            encoded.set_column_len(field_type.get_flen());
            encoded.set_decimal(field_type.get_decimal());
            encoded.set_flag(field_type.get_flag() as i32);
            encoded.set_elems(field_type.get_elems().to_vec().into());
            encoded.set_array(column_info.FieldType.IsArray());
            encoded.set_pk_handle(
                table.PKIsHandle
                    && table
                        .GetPkColInfo()
                        .is_some_and(|primary| primary.ID == column_info.ID),
            );
            columns.push(encoded);
        }
        scan.set_columns(columns.into());
        scan.set_desc(self.Desc);
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeIndexScan);
        executor.set_idx_scan(scan);
        Ok(Box::new(executor))
    }

    /// 计划代价 V1。
    pub fn GetPlanCostVer1(
        &mut self,
        task: TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

    /// 计划代价 V2。
    pub fn GetPlanCostVer2(
        &mut self,
        task: TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task, option, inl)
    }

    /// 解析 Schema 生产器列下标。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()
    }

    /// 估算扫描节点内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .AccessCondition
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + self.IdxCols.iter().map(Column::MemoryUsage).sum::<i64>()
            + self.Ranges.iter().map(ranger::Range::MemUsage).sum::<i64>()
            + self.DBName.len() as i64
            + self.TableAsName.len() as i64
            + self.RangeInfo.len() as i64
    }
}

/// 由逻辑索引扫描的 Schema/统计构造物理扫描。
pub fn GetPhysicalIndexScan4LogicalIndexScan(
    ctx: ContextRef,
    schema: Schema,
    stats: StatsInfo,
) -> PhysicalIndexScan {
    let mut scan = PhysicalIndexScan::New(ctx);
    scan.PhysicalSchemaProducer.SetSchema(schema);
    scan.PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(stats);
    scan
}

/// 按所需物理属性（排序项）构造原始索引扫描。
pub fn GetOriginalPhysicalIndexScan(
    ctx: ContextRef,
    property: PhysicalProperty,
) -> PhysicalIndexScan {
    let mut scan = PhysicalIndexScan::New(ctx);
    scan.Desc = property.SortItems.first().is_some_and(|item| item.Desc);
    scan.KeepOrder = !property.SortItems.is_empty();
    scan.Prop = Some(property);
    scan
}

/// 转为部分索引扫描占位：当前原样返回扫描与条件。
pub fn ConvertToPartialIndexScan(
    scan: PhysicalIndexScan,
    conditions: Vec<ExprBox>,
) -> (PhysicalIndexScan, Vec<ExprBox>) {
    (scan, conditions)
}
