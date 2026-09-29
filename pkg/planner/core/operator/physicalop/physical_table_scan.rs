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

// 物理表扫描算子（Physical Table Scan）。
//
// 执行计划（physical plan）中直接访问表数据的叶子节点：按 Ranges 扫描行，
// 可选下推 AccessCondition，并在 TiDB 侧用 FilterCondition 过滤。
// StoreType 区分 TiKV（行存）与 TiFlash（列存）；MPP/BatchCop 表示分布式读路径。

use base::{ContextRef, PhysicalPlan, Plan};
use costusage::{CostVer2, PlanCostOption};
use expression::{Column, CorrelatedColumn, ExprBox, Schema};
use model::{ColumnInfo, IndexInfo, TableInfo};
use property::{HistCollRef, PhysicalProperty, StatsInfo, TaskType};

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// Extra metadata and protobuf query payload for a columnar index scan.
#[derive(Clone, Debug, Default)]
pub struct ColumnarIndexExtra {
    pub IndexInfo: IndexInfo,
    pub QueryInfo: tipb::ColumnarIndexInfo,
}

/// 物理表扫描：从表或分区读取行，产出 Schema 与访问对象信息。
pub struct PhysicalTableScan {
    /// Schema/统计等物理计划公共字段生产者。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 表元信息；分区扫描时仍指向逻辑表。
    pub Table: Option<TableInfo>,
    /// 需要读取的列定义。
    pub Columns: Vec<ColumnInfo>,
    /// 库名（用于 Explain/访问对象展示）。
    pub DBName: String,
    /// 表别名。
    pub TableAsName: String,
    /// 物理表/分区 ID；分区场景与逻辑表 ID 可能不同。
    pub PhysicalTableID: i64,
    /// 扫描区间（key range）；空或全范围表示全表扫描。
    pub Ranges: ranger::Ranges,
    /// 索引连接根据外表行动态决定的范围说明。
    pub RangeInfo: String,
    /// 可下推到存储层、用于构造 Ranges 的访问条件。
    pub AccessCondition: Vec<ExprBox>,
    /// 无法完全下推、需在扫描后过滤的条件。
    pub FilterCondition: Vec<ExprBox>,
    /// TiFlash 延迟物化时先下推到扫描的过滤条件。
    pub LateMaterializationFilterCondition: Vec<ExprBox>,
    /// 延迟物化条件应用于原始扫描行的选择率。
    pub LateMaterializationSelectivity: f64,
    /// 扫描后过滤完成的基数，用于物化 pushed-down Selection。
    pub FilterStats: Option<StatsInfo>,
    /// 目标存储类型（TiKV/TiFlash 等）。
    pub StoreType: kv::StoreType,
    /// 是否扫描分区表的某个物理分区。
    pub IsPartition: bool,
    /// 是否走 MPP 或 BatchCop 分布式读请求。
    pub IsMPPOrBatchCop: bool,
    /// 是否按降序扫描。
    pub Desc: bool,
    /// 是否要求输出保持索引/主键顺序。
    pub KeepOrder: bool,
    /// 是否使用聚簇索引公共句柄（common handle）。
    pub IsCommonHandle: bool,
    /// 表列直方图集合，供代价估算使用。
    pub TblColHists: Option<HistCollRef>,
    /// 物化时记录的物理属性（排序等）。
    pub Prop: Option<PhysicalProperty>,
}

impl PhysicalTableScan {
    /// Re-evaluate handle ranges against the current prepared parameter list.
    /// A cached physical plan's serialized Ranges reflect its first execution.
    pub fn RebuildRangesForPlanCache(&self) -> Result<ranger::Ranges, expression::Error> {
        let primary = self
            .Table
            .as_ref()
            .and_then(TableInfo::GetPkColInfo)
            .ok_or_else(|| expression::errors::New("table scan has no primary handle"))?;
        let mut context = self.s_ctx().GetRangerCtx().clone();
        let (ranges, _, remaining) = ranger::BuildTableRange(
            self.AccessCondition
                .iter()
                .map(|condition| condition.CloneExpr())
                .collect(),
            &mut context,
            &primary.FieldType,
            0,
        )
        .map_err(|error| expression::errors::New(error.to_string()))?;
        if !remaining.is_empty() {
            return Err(expression::errors::New(
                "prepared table scan access conditions did not rebuild completely",
            ));
        }
        Ok(ranges)
    }

    /// 构造默认表扫描节点（类型码 TableScan，Ranges 为空）。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeTableScan,
                0,
            )),
            Table: None,
            Columns: Vec::new(),
            DBName: String::new(),
            TableAsName: String::new(),
            PhysicalTableID: 0,
            Ranges: ranger::Ranges::default(),
            RangeInfo: String::new(),
            AccessCondition: Vec::new(),
            FilterCondition: Vec::new(),
            LateMaterializationFilterCondition: Vec::new(),
            LateMaterializationSelectivity: 1.0,
            FilterStats: None,
            StoreType: kv::StoreType::TiKV,
            IsPartition: false,
            IsMPPOrBatchCop: false,
            Desc: false,
            KeepOrder: false,
            IsCommonHandle: false,
            TblColHists: None,
            Prop: None,
        }
    }

    /// 用查询块偏移重新初始化基类计划元数据。
    pub fn Init(mut self, ctx: ContextRef, offset: i32) -> Self {
        let plan = &mut self.PhysicalSchemaProducer.BasePhysicalPlan;
        plan.Plan.SetSCtx(ctx);
        plan.SetTP(plancodec::TypeTableScan);
        plan.Plan.SetQueryBlockOffset(offset);
        self
    }

    /// 深拷贝表达式与 Schema，换绑新的 PlanContext。
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
            Table: self.Table.as_ref().map(TableInfo::Clone),
            Columns: self.Columns.clone(),
            DBName: self.DBName.clone(),
            TableAsName: self.TableAsName.clone(),
            PhysicalTableID: self.PhysicalTableID,
            Ranges: self.Ranges.clone(),
            RangeInfo: self.RangeInfo.clone(),
            AccessCondition: self.AccessCondition.iter().map(|e| e.CloneExpr()).collect(),
            FilterCondition: self.FilterCondition.iter().map(|e| e.CloneExpr()).collect(),
            LateMaterializationFilterCondition: self
                .LateMaterializationFilterCondition
                .iter()
                .map(|e| e.CloneExpr())
                .collect(),
            LateMaterializationSelectivity: self.LateMaterializationSelectivity,
            FilterStats: self.FilterStats.clone(),
            StoreType: self.StoreType,
            IsPartition: self.IsPartition,
            IsMPPOrBatchCop: self.IsMPPOrBatchCop,
            Desc: self.Desc,
            KeepOrder: self.KeepOrder,
            IsCommonHandle: self.IsCommonHandle,
            TblColHists: self.TblColHists.clone(),
            Prop: self.Prop.clone(),
        })
    }

    /// 生成访问对象描述（表名；分区时附带物理分区 ID）。
    pub fn AccessObject(&self) -> String {
        self.access_object(false)
    }

    fn access_object(&self, normalized: bool) -> String {
        let table = if !self.TableAsName.is_empty() {
            self.TableAsName.as_str()
        } else {
            self.Table
                .as_ref()
                .map(|table| table.Name.O.as_str())
                .unwrap_or("unknown")
        };
        if self.IsPartition {
            let partition = if normalized {
                "?".to_owned()
            } else {
                self.Table
                    .as_ref()
                    .and_then(|table| table.Partition.as_ref())
                    .map_or_else(String::new, |info| info.GetNameByID(self.PhysicalTableID))
            };
            if partition.is_empty() {
                format!("table:{table}")
            } else {
                format!("table:{table}, partition:{partition}")
            }
        } else {
            format!("table:{table}")
        }
    }

    /// 细分扫描类型：RowID 扫描 / 全表扫描 / 范围扫描。
    pub fn TP(&self) -> String {
        if base::Plan::tp(&self.PhysicalSchemaProducer.BasePhysicalPlan, &[])
            == plancodec::TypeTableRowIDScan
        {
            plancodec::TypeTableRowIDScan.to_owned()
        } else if self.IsFullScan() {
            plancodec::TypeTableFullScan.to_owned()
        } else {
            plancodec::TypeTableRangeScan.to_owned()
        }
    }

    /// EXPLAIN 节点 ID：类型名 + 计划 ID。
    pub fn ExplainID(&self) -> String {
        format!("{}_{}", self.TP(), base::Plan::id(self))
    }

    /// 算子细节：range、keep order、desc；normalized 时隐藏具体区间值。
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
        if self.s_ctx().GetSessionVars().EnableLateMaterialization
            && self.StoreType == kv::StoreType::TiFlash
            && !self.FilterCondition.is_empty()
            && !self.LateMaterializationFilterCondition.is_empty()
        {
            info.push_str(", pushed down filter:");
            let filters = if normalized {
                expression::SortedExplainNormalizedExpressionList(
                    &self.LateMaterializationFilterCondition,
                )
            } else {
                expression::SortedExplainExpressionList(
                    self.s_ctx().GetExprCtx().GetEvalCtx(),
                    &self.LateMaterializationFilterCondition,
                )
            };
            info.push_str(&String::from_utf8_lossy(&filters));
        }
        if self.Desc {
            info.push_str(", desc");
        }
        info
    }

    /// 完整 Explain 文本（访问对象 + 算子细节）。
    pub fn ExplainInfo(&self) -> String {
        format!("{}, {}", self.AccessObject(), self.OperatorInfo(false))
    }

    /// 归一化 Explain（用于 Plan Digest 等）。
    pub fn ExplainNormalizedInfo(&self) -> String {
        format!("{}, {}", self.access_object(true), self.OperatorInfo(true))
    }

    /// 从范围访问条件抽取相关子查询列（普通过滤条件不参与范围重建）。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.AccessCondition
            .iter()
            .flat_map(|expr| expression::ExtractCorColumns(expr.as_ref()))
            .map(CorrelatedColumn::Clone)
            .collect()
    }

    /// 无相关列且区间为空或全范围时视为全表扫描。
    pub fn IsFullScan(&self) -> bool {
        let unsigned_int_handle = self.Table.as_ref().is_some_and(|table| {
            table.PKIsHandle
                && table
                    .GetPkColInfo()
                    .is_some_and(|primary| mysql::r#type::HasUnsignedFlag(primary.GetFlag()))
        });
        self.RangeInfo.is_empty()
            && self.ExtractCorrelatedCols().is_empty()
            && (self.Ranges.is_empty()
                || self
                    .Ranges
                    .iter()
                    .all(|range| range.IsFullRange(unsigned_int_handle)))
    }

    /// 返回分区标记与物理表 ID。
    pub fn IsPartition(&self) -> (bool, i64) {
        (self.IsPartition, self.PhysicalTableID)
    }

    /// 设置分区扫描目标。
    pub fn SetIsPartition(&mut self, is_partition: bool, physical_id: i64) {
        self.IsPartition = is_partition;
        self.PhysicalTableID = physical_id;
    }

    /// 追加隐藏 handle 列到 Schema 与 Columns（回表/排序需要）。
    pub fn AppendExtraHandleCol(&mut self, column: Column, info: ColumnInfo) {
        let mut schema = self
            .PhysicalSchemaProducer
            .SchemaRef()
            .map(Schema::Clone)
            .unwrap_or_else(|| expression::NewSchema(Vec::new()));
        schema.Append([column]);
        self.Columns.push(info);
        self.PhysicalSchemaProducer.SetSchema(schema);
    }

    /// 将选择条件下沉为本扫描的 FilterCondition。
    pub fn BuildPushedDownSelection(&mut self, conditions: Vec<ExprBox>) {
        self.FilterCondition.extend(conditions);
    }

    /// 为 IndexMerge 构造表侧扫描：换 Ranges 且关闭 KeepOrder。
    pub fn BuildIndexMergeTableScan(&self, ranges: ranger::Ranges) -> Self {
        let mut cloned = self
            .Clone(self.PhysicalSchemaProducer.BasePhysicalPlan.s_ctx().clone())
            .expect("table scan clone");
        cloned.Ranges = ranges;
        cloned.KeepOrder = false;
        cloned
    }

    /// 解析相关列索引，委托 ResolveIndicesItself。
    pub fn ResolveCorrelatedColumns(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndicesItself()
    }

    /// 将本节点条件中的列引用解析为 Schema 下标。
    pub fn ResolveIndicesItself(&mut self) -> Result<(), expression::Error> {
        if let Some(mut schema) = self.PhysicalSchemaProducer.SchemaRef().map(Schema::Clone) {
            for (index, column) in schema.Columns.iter_mut().enumerate() {
                column.Index = index as isize;
            }
            for condition in &mut self.LateMaterializationFilterCondition {
                *condition = condition.ResolveIndices(&schema)?;
            }
            self.PhysicalSchemaProducer.SetSchema(schema);
        }
        Ok(())
    }

    /// 先解析子树 Schema，再解析本节点条件。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        self.ResolveIndicesItself()
    }

    /// 粗略估算扫描行宽（列数 × 8），供代价模型使用。
    pub fn GetScanRowSize(&self) -> f64 {
        self.PhysicalSchemaProducer
            .SchemaRef()
            .map_or(0.0, |schema| schema.Len() as f64 * 8.0)
    }

    /// 序列化为 tipb::TableScan；分区用物理 ID，否则用表 ID。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        _store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let table = self
            .Table
            .as_ref()
            .ok_or_else(|| expression::errors::New("table scan has no TableInfo"))?;
        let mut scan = tipb::TableScan::new();
        scan.set_table_id(if self.IsPartition {
            self.PhysicalTableID
        } else {
            table.ID
        });
        scan.set_desc(self.Desc);
        scan.set_columns(
            self.Columns
                .iter()
                .map(|column| {
                    let field_type = expression::ToPBFieldType(&column.FieldType);
                    let mut encoded = tipb::ColumnInfo::new();
                    encoded.set_column_id(column.ID);
                    encoded.set_tp(field_type.get_tp());
                    encoded.set_collation(field_type.get_collate());
                    encoded.set_column_len(field_type.get_flen());
                    encoded.set_decimal(field_type.get_decimal());
                    encoded.set_flag(field_type.get_flag() as i32);
                    encoded.set_elems(field_type.get_elems().to_vec().into());
                    encoded.set_array(column.FieldType.IsArray());
                    encoded.set_pk_handle(
                        table.PKIsHandle
                            && table
                                .GetPkColInfo()
                                .is_some_and(|primary| primary.ID == column.ID),
                    );
                    encoded
                })
                .collect::<Vec<_>>()
                .into(),
        );
        if !self.LateMaterializationFilterCondition.is_empty() {
            let client = ctx
                .GetClient()
                .ok_or_else(|| expression::errors::New("PB client is required"))?;
            scan.set_pushed_down_filter_conditions(
                expression::ExpressionsToPBList(
                    ctx.GetExprCtx().GetEvalCtx(),
                    &self.LateMaterializationFilterCondition,
                    client.as_ref(),
                )?
                .into(),
            );
        }
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeTableScan);
        executor.set_tbl_scan(scan);
        Ok(Box::new(executor))
    }

    /// 代价模型 v1：委托基类计算。
    pub fn GetPlanCostVer1(
        &mut self,
        task: TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

    /// 代价模型 v2：委托基类计算。
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

    /// 估算本节点及表达式/区间占用的内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self.Columns.capacity() as i64 * std::mem::size_of::<ColumnInfo>() as i64
            + self
                .AccessCondition
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + self
                .FilterCondition
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + self
                .LateMaterializationFilterCondition
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + self.Ranges.iter().map(ranger::Range::MemUsage).sum::<i64>()
            + self.RangeInfo.len() as i64
            + self.DBName.len() as i64
            + self.TableAsName.len() as i64
    }
}

/// 由逻辑表扫描物化：注入 Schema 与统计信息。
pub fn GetPhysicalScan4LogicalTableScan(
    ctx: ContextRef,
    schema: Schema,
    stats: StatsInfo,
) -> PhysicalTableScan {
    let mut scan = PhysicalTableScan::New(ctx);
    scan.PhysicalSchemaProducer.SetSchema(schema);
    scan.PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(stats);
    scan
}

/// 按父层要求的有序属性构造初始表扫描（Desc/KeepOrder）。
pub fn GetOriginalPhysicalTableScan(
    ctx: ContextRef,
    property: PhysicalProperty,
) -> PhysicalTableScan {
    let mut scan = PhysicalTableScan::New(ctx);
    scan.Desc = property.SortItems.first().is_some_and(|item| item.Desc);
    scan.KeepOrder = !property.SortItems.is_empty();
    scan.Prop = Some(property);
    scan
}
