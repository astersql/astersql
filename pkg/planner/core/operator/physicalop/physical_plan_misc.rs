// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 物理计划杂项类型：下推 Limit、分区剪枝信息、以及 Runtime Filter（运行时过滤器）。
//
// Runtime Filter 由 HashJoin 构建侧生成，在探测前过滤 TableScan 输入，减少无效探测；
// 模式含 Off / Local / Global。

use std::fmt;

use base::PhysicalPlan;
use expression::{Column, ExprBox, Expression as ExpressionTrait, ScalarFunction};
use parser_ast::CIStr;
use types::metadata::NameSlice;

/// 已下推到存储/读算子的 Limit 参数（offset + count）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PushedDownLimit {
    pub Offset: u64,
    pub Count: u64,
}

impl PushedDownLimit {
    /// 装箱克隆，对齐 Go 指针语义。
    /// 装箱深克隆。
    pub fn Clone(&self) -> Box<Self> {
        Box::new(*self)
    }
    /// 固定结构体尺寸作为内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        pushedDownLimitSize
    }
}

/// `PushedDownLimit` 的 sizeof，供 MemoryUsage 复用。
pub const pushedDownLimitSize: i64 = std::mem::size_of::<PushedDownLimit>() as i64;

/// 将 Selection 条件拆成不含/含虚拟列两组；含虚拟列的通常不能下推到存储。
pub fn SplitSelCondsWithVirtualColumn(conditions: &[ExprBox]) -> (Vec<ExprBox>, Vec<ExprBox>) {
    let mut without_virtual = Vec::new();
    let mut with_virtual = Vec::new();
    for condition in conditions {
        if expression::ContainVirtualColumn(std::slice::from_ref(condition)) {
            with_virtual.push(condition.CloneExpr());
        } else {
            without_virtual.push(condition.CloneExpr());
        }
    }
    (without_virtual, with_virtual)
}

/// 物理计划携带的分区剪枝信息：谓词、分区名与相关列。
pub struct PhysPlanPartInfo {
    /// 用于分区剪枝的条件表达式。
    pub PruningConds: Vec<ExprBox>,
    /// 显式指定的分区名列表。
    pub PartitionNames: Vec<CIStr>,
    /// 剪枝涉及的列。
    pub Columns: Vec<Column>,
    /// 列名切片。
    pub ColumnNames: NameSlice,
}

/// 空 `PhysPlanPartInfo` 结构体尺寸基线。
pub const emptyPartitionInfoSize: i64 = std::mem::size_of::<PhysPlanPartInfo>() as i64;

impl PhysPlanPartInfo {
    /// 返回剪枝条件。
    pub fn GetPruningConds(&self) -> &[ExprBox] {
        &self.PruningConds
    }
    /// 返回分区名列表。
    pub fn GetPartitionNames(&self) -> &[CIStr] {
        &self.PartitionNames
    }
    /// 返回剪枝列。
    pub fn GetColumns(&self) -> &[Column] {
        &self.Columns
    }
    /// 浅拷贝列名切片。
    pub fn GetColumnNames(&self) -> NameSlice {
        self.ColumnNames.Shallow()
    }

    /// 计划缓存克隆入口。
    pub fn CloneForPlanCache(&self) -> Box<Self> {
        Box::new(Self {
            PruningConds: self.PruningConds.clone(),
            PartitionNames: self.PartitionNames.clone(),
            Columns: self.Columns.clone(),
            ColumnNames: self.ColumnNames.Shallow(),
        })
    }

    /// 深克隆表达式与分区名，列名浅拷贝。
    pub fn Clone(&self) -> Box<Self> {
        Box::new(Self {
            PruningConds: self.PruningConds.clone(),
            PartitionNames: self.PartitionNames.clone(),
            Columns: self.Columns.clone(),
            ColumnNames: NameSlice(
                self.ColumnNames
                    .0
                    .iter()
                    .map(|name| {
                        name.as_ref()
                            .map(|name| std::sync::Arc::new(name.as_ref().Clone()))
                    })
                    .collect(),
            ),
        })
    }

    /// 累加表达式、分区名与列的内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        emptyPartitionInfoSize
            + self
                .PruningConds
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + self
                .PartitionNames
                .iter()
                .map(CIStr::memory_usage)
                .sum::<i64>()
            + self.Columns.iter().map(Column::MemoryUsage).sum::<i64>()
            + self
                .ColumnNames
                .0
                .iter()
                .flatten()
                .map(|name| name.MemoryUsage())
                .sum::<i64>()
    }
}

/// 表扫描计划与其分区剪枝信息的捆绑。
pub struct TableScanAndPartitionInfo {
    pub TableScan: Option<Box<dyn PhysicalPlan>>,
    pub PhysPlanPartInfo: Box<PhysPlanPartInfo>,
}

impl TableScanAndPartitionInfo {
    /// 表扫描 + 分区信息合计内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysPlanPartInfo.MemoryUsage()
            + self
                .TableScan
                .as_ref()
                .map_or(0, |scan| scan.memory_usage())
    }
}

/// Runtime Filter 种类：IN 列表或 Min/Max 范围。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RuntimeFilterType {
    #[default]
    In,
    MinMax,
}

impl fmt::Display for RuntimeFilterType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::In => "IN",
            Self::MinMax => "MIN_MAX",
        })
    }
}

/// Runtime Filter 作用范围：关闭、本地、全局。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RuntimeFilterMode {
    #[default]
    Off,
    Local,
    Global,
}

impl fmt::Display for RuntimeFilterMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Off => "OFF",
            Self::Local => "LOCAL",
            Self::Global => "GLOBAL",
        })
    }
}

/// Adapter implemented by PhysicalHashJoin without coupling the foundation to that module.
/// 由 PhysicalHashJoin 实现的构建侧适配器，避免 foundation 直接依赖 Join 模块。
pub trait RuntimeFilterBuildNode {
    fn id(&self) -> i32;
    fn right_is_build_side(&self) -> bool;
    fn runtime_filter_types(&self) -> &[RuntimeFilterType];
    fn register_runtime_filter(&mut self, id: i32);
}

/// Adapter implemented by PhysicalTableScan.
/// 由 PhysicalTableScan 实现的目标侧适配器。
pub trait RuntimeFilterTargetNode {
    fn id(&self) -> i32;
    fn runtime_filter_count(&self) -> usize;
    fn set_max_wait_time_ms(&mut self, milliseconds: u64);
    fn register_runtime_filter(&mut self, id: i32);
}

/// 单个 Runtime Filter：连接构建侧源列与探测侧目标列。
pub struct RuntimeFilter {
    id: i32,
    /// 构建侧（源）列表达式。
    src_expr_list: Vec<Column>,
    /// 探测侧（目标）列表达式。
    target_expr_list: Vec<Column>,
    rf_type: RuntimeFilterType,
    pub RfMode: RuntimeFilterMode,
    build_node_id: i32,
    target_node_id: Option<i32>,
}

/// 为 Runtime Filter 分配递增 ID。
#[derive(Clone, Debug, Default)]
pub struct RuntimeFilterIDGenerator {
    next_id: i32,
}

impl RuntimeFilterIDGenerator {
    /// 从给定起始 ID 创建生成器。
    pub fn New(start: i32) -> Self {
        Self { next_id: start }
    }
    /// 取出下一个 ID 并自增（饱和加法）。
    pub fn GetNextID(&mut self) -> i32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }
}

/// 根据等值条件与构建侧节点，为每种 RF 类型各生成一个未绑定目标的过滤器。
/// 返回过滤器列表以及目标列 UniqueID。
pub fn NewRuntimeFilter(
    ids: &mut RuntimeFilterIDGenerator,
    equality: &ScalarFunction,
    build_node: &mut dyn RuntimeFilterBuildNode,
) -> (Vec<RuntimeFilter>, i64) {
    let (left, right) = expression::ExtractColumnsFromColOpCol(equality);
    let (left, right) = (
        left.expect("runtime-filter equality left operand must be a column"),
        right.expect("runtime-filter equality right operand must be a column"),
    );
    // 构建侧在右则源列为右键，目标 UniqueID 取左键，反之亦然。
    let (source, target_unique_id) = if build_node.right_is_build_side() {
        (right.Clone(), left.UniqueID)
    } else {
        (left.Clone(), right.UniqueID)
    };
    let types = build_node.runtime_filter_types().to_vec();
    let mut filters = Vec::with_capacity(types.len());
    for rf_type in types {
        let id = ids.GetNextID();
        filters.push(RuntimeFilter {
            id,
            src_expr_list: vec![source.Clone()],
            target_expr_list: Vec::new(),
            rf_type,
            RfMode: RuntimeFilterMode::Off,
            build_node_id: build_node.id(),
            target_node_id: None,
        });
    }
    (filters, target_unique_id)
}

impl RuntimeFilter {
    /// 过滤器唯一 ID。
    pub fn ID(&self) -> i32 {
        self.id
    }

    /// 将过滤器绑定到构建/目标节点，并在双方登记该 ID。
    pub fn Assign(
        &mut self,
        build_node: &mut dyn RuntimeFilterBuildNode,
        target_node: &mut dyn RuntimeFilterTargetNode,
        target_expression: Column,
    ) {
        // 目标节点首次挂 RF 时设置默认最大等待 10s。
        if target_node.runtime_filter_count() == 0 {
            target_node.set_max_wait_time_ms(10_000);
        }
        self.build_node_id = build_node.id();
        self.target_node_id = Some(target_node.id());
        self.target_expr_list.push(target_expression);
        build_node.register_runtime_filter(self.id);
        target_node.register_runtime_filter(self.id);
    }

    /// EXPLAIN 片段：按构建/目标侧选择源或目标列列表。
    pub fn ExplainInfo(&self, is_build_node: bool, ctx: &dyn expression::EvalContext) -> String {
        let expressions = if is_build_node {
            &self.src_expr_list
        } else {
            &self.target_expr_list
        };
        let arrow = if is_build_node { " <- " } else { " -> " };
        let rendered = expressions
            .iter()
            .map(|expr| ExpressionTrait::ExplainInfo(expr, ctx))
            .collect::<Vec<_>>()
            .join(",");
        format!("{}[{}]{}{}", self.id, self.rf_type, arrow, rendered)
    }

    /// 调试字符串：ID、节点、列、类型与模式。
    pub fn String(&self) -> String {
        let source = self
            .src_expr_list
            .iter()
            .map(|expression| format!("{},", expression.String()))
            .collect::<String>();
        let target = self
            .target_expr_list
            .iter()
            .map(|expression| format!("{},", expression.String()))
            .collect::<String>();
        let target_id = self
            .target_node_id
            .map_or_else(|| "nil".to_owned(), |id| id.to_string());
        let mode = if self.RfMode == RuntimeFilterMode::Off {
            "nil".to_owned()
        } else {
            self.RfMode.to_string()
        };
        format!(
            "id={}, buildNodeID={}, targetNodeID={}, srcColumn={}, targetColumn={}, rfType={}, rfMode={}.",
            self.id, self.build_node_id, target_id, source, target, self.rf_type, mode
        )
    }

    pub fn Clone(&self) -> Box<Self> {
        Box::new(Self {
            id: self.id,
            src_expr_list: self.src_expr_list.clone(),
            target_expr_list: self.target_expr_list.clone(),
            rf_type: self.rf_type,
            RfMode: self.RfMode,
            build_node_id: self.build_node_id,
            target_node_id: self.target_node_id,
        })
    }

    /// 编码为 tipb::RuntimeFilter，供存储侧执行。
    pub fn ToPB(
        &self,
        ctx: &base::BuildPBContext,
        client: &dyn kv::Client,
    ) -> Result<tipb::RuntimeFilter, expression::Error> {
        let expr_ctx = ctx.GetExprCtx();
        let eval_ctx = expr_ctx.GetEvalCtx();
        let converter = expression::NewPBConverter(client, eval_ctx);
        let mut sources = Vec::with_capacity(self.src_expr_list.len());
        for expression in &self.src_expr_list {
            sources.push(converter.ExprToPB(expression).ok_or_else(|| {
                expression::errors::New(format!(
                    "failed to transform src expr {} to pb in runtime filter",
                    expression.String()
                ))
            })?);
        }
        let mut targets = Vec::with_capacity(self.target_expr_list.len());
        for expression in &self.target_expr_list {
            targets.push(converter.ExprToPB(expression).ok_or_else(|| {
                expression::errors::New(format!(
                    "failed to transform target expr {} to pb in runtime filter",
                    expression.String()
                ))
            })?);
        }
        let mut result = tipb::RuntimeFilter::new();
        result.set_id(self.id);
        result.set_source_expr_list(sources.into());
        result.set_target_expr_list(targets.into());
        result.set_source_executor_id(self.build_node_id.to_string());
        result.set_target_executor_id(self.target_node_id.unwrap_or_default().to_string());
        result.set_rf_type(match self.rf_type {
            RuntimeFilterType::In => tipb::RuntimeFilterType::In,
            RuntimeFilterType::MinMax => tipb::RuntimeFilterType::MinMax,
        });
        result.set_rf_mode(match self.RfMode {
            RuntimeFilterMode::Global => tipb::RuntimeFilterMode::Global,
            RuntimeFilterMode::Off | RuntimeFilterMode::Local => tipb::RuntimeFilterMode::Local,
        });
        Ok(result)
    }
}

/// 批量将 Runtime Filter 列表转为 PB。
pub fn RuntimeFilterListToPB(
    ctx: &base::BuildPBContext,
    filters: &[RuntimeFilter],
    client: &dyn kv::Client,
) -> Result<Vec<tipb::RuntimeFilter>, expression::Error> {
    filters
        .iter()
        .map(|filter| filter.ToPB(ctx, client))
        .collect()
}
