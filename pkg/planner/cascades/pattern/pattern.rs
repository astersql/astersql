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

// Cascades 规则匹配用的算子操作数（Operand）与模式树（Pattern）。
//
// Pattern 描述变换规则期望匹配的逻辑算子树形状；Operand 将具体逻辑计划节点
// （LogicalPlan）映射为可比较的算子类别。匹配时还可结合 EngineTypeSet，
// 限制规则只在 TiDB 或 TiKV 等引擎侧生效。

use logicalop::LogicalPlan;
use std::fmt;

/// 规则模式中的算子操作数：对应一类逻辑/物理计划节点，或通配符 Any。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Operand {
    /// 通配符：可匹配任意算子。
    Any,
    /// 连接（Join）。
    Join,
    /// 聚合（Aggregation）。
    Aggregation,
    /// 投影（Projection）。
    Projection,
    /// 选择/过滤（Selection）。
    Selection,
    /// 相关子查询应用（Apply）：外层每行驱动内层执行。
    Apply,
    /// 最多一行（MaxOneRow）保证。
    MaxOneRow,
    /// 空表/常量行源（TableDual）。
    TableDual,
    /// 数据源（DataSource）。
    DataSource,
    /// 联合扫描（UnionScan）。
    UnionScan,
    /// 并集全部（UnionAll）。
    UnionAll,
    /// 排序（Sort）。
    Sort,
    /// TopN：排序后取前 N 行。
    TopN,
    /// 行锁（Lock）。
    Lock,
    /// 限制行数（Limit）。
    Limit,
    /// TiKV 单点收集（TiKVSingleGather）。
    TiKVSingleGather,
    /// 内存表扫描（MemTableScan）。
    MemTableScan,
    /// 表扫描（TableScan）。
    TableScan,
    /// 索引扫描（IndexScan）。
    IndexScan,
    /// SHOW 语句逻辑节点。
    Show,
    /// 窗口函数（Window）。
    Window,
    /// 尚未映射到已知 Operand 的算子。
    Unsupported,
}

/// Operand::Any 的兼容别名（对应 Go 侧常量命名）。
pub const OperandAny: Operand = Operand::Any;
/// Operand::Join 的兼容别名。
pub const OperandJoin: Operand = Operand::Join;
/// Operand::Aggregation 的兼容别名。
pub const OperandAggregation: Operand = Operand::Aggregation;
/// Operand::Projection 的兼容别名。
pub const OperandProjection: Operand = Operand::Projection;
/// Operand::Selection 的兼容别名。
pub const OperandSelection: Operand = Operand::Selection;
/// Operand::Apply 的兼容别名。
pub const OperandApply: Operand = Operand::Apply;
/// Operand::MaxOneRow 的兼容别名。
pub const OperandMaxOneRow: Operand = Operand::MaxOneRow;
/// Operand::TableDual 的兼容别名。
pub const OperandTableDual: Operand = Operand::TableDual;
/// Operand::DataSource 的兼容别名。
pub const OperandDataSource: Operand = Operand::DataSource;
/// Operand::UnionScan 的兼容别名。
pub const OperandUnionScan: Operand = Operand::UnionScan;
/// Operand::UnionAll 的兼容别名。
pub const OperandUnionAll: Operand = Operand::UnionAll;
/// Operand::Sort 的兼容别名。
pub const OperandSort: Operand = Operand::Sort;
/// Operand::TopN 的兼容别名。
pub const OperandTopN: Operand = Operand::TopN;
/// Operand::Lock 的兼容别名。
pub const OperandLock: Operand = Operand::Lock;
/// Operand::Limit 的兼容别名。
pub const OperandLimit: Operand = Operand::Limit;
/// Operand::TiKVSingleGather 的兼容别名。
pub const OperandTiKVSingleGather: Operand = Operand::TiKVSingleGather;
/// Operand::MemTableScan 的兼容别名。
pub const OperandMemTableScan: Operand = Operand::MemTableScan;
/// Operand::TableScan 的兼容别名。
pub const OperandTableScan: Operand = Operand::TableScan;
/// Operand::IndexScan 的兼容别名。
pub const OperandIndexScan: Operand = Operand::IndexScan;
/// Operand::Show 的兼容别名。
pub const OperandShow: Operand = Operand::Show;
/// Operand::Window 的兼容别名。
pub const OperandWindow: Operand = Operand::Window;
/// Operand::Unsupported 的兼容别名。
pub const OperandUnsupported: Operand = Operand::Unsupported;

impl fmt::Display for Operand {
    /// 输出与 Go 侧常量名一致的字符串，便于日志与调试对照。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Operand::Any => "OperandAny",
            Operand::Join => "OperandJoin",
            Operand::Aggregation => "OperandAggregation",
            Operand::Projection => "OperandProjection",
            Operand::Selection => "OperandSelection",
            Operand::Apply => "OperandApply",
            Operand::MaxOneRow => "OperandMaxOneRow",
            Operand::TableDual => "OperandTableDual",
            Operand::DataSource => "OperandDataSource",
            Operand::UnionScan => "OperandUnionScan",
            Operand::UnionAll => "OperandUnionAll",
            Operand::Sort => "OperandSort",
            Operand::TopN => "OperandTopN",
            Operand::Lock => "OperandLock",
            Operand::Limit => "OperandLimit",
            Operand::TiKVSingleGather => "OperandTiKVSingleGather",
            Operand::MemTableScan => "OperandMemTableScan",
            Operand::TableScan => "OperandTableScan",
            Operand::IndexScan => "OperandIndexScan",
            Operand::Show => "OperandShow",
            Operand::Window => "OperandWindow",
            Operand::Unsupported => "OperandUnsupported",
        })
    }
}

/// 将逻辑计划节点映射为对应 Operand；未知类型返回 OperandUnsupported。
///
/// 按具体类型做 downcast 判别，Apply 优先于 Join（Apply 内嵌 Join 语义）。
pub fn GetOperand(plan: &dyn LogicalPlan) -> Operand {
    let plan = plan.as_any();
    // 按算子具体类型依次判别；Apply 须先于 Join，因其底层常复用 Join 结构。
    if plan.is::<logicalop::LogicalApply>() {
        OperandApply
    } else if plan.is::<logicalop::LogicalJoin>() {
        OperandJoin
    } else if plan.is::<logicalop::LogicalAggregation>() {
        OperandAggregation
    } else if plan.is::<logicalop::LogicalProjection>() {
        OperandProjection
    } else if plan.is::<logicalop::LogicalSelection>() {
        OperandSelection
    } else if plan.is::<logicalop::LogicalMaxOneRow>() {
        OperandMaxOneRow
    } else if plan.is::<logicalop::LogicalTableDual>() {
        OperandTableDual
    } else if plan.is::<logicalop::DataSource>() {
        OperandDataSource
    } else if plan.is::<logicalop::LogicalUnionScan>() {
        OperandUnionScan
    } else if plan.is::<logicalop::LogicalUnionAll>() {
        OperandUnionAll
    } else if plan.is::<logicalop::LogicalSort>() {
        OperandSort
    } else if plan.is::<logicalop::LogicalTopN>() {
        OperandTopN
    } else if plan.is::<logicalop::LogicalLock>() {
        OperandLock
    } else if plan.is::<logicalop::LogicalLimit>() {
        OperandLimit
    } else if plan.is::<logicalop::TiKVSingleGather>() {
        OperandTiKVSingleGather
    } else if plan.is::<logicalop::LogicalTableScan>() {
        OperandTableScan
    } else if plan.is::<logicalop::LogicalMemTable>() {
        OperandMemTableScan
    } else if plan.is::<logicalop::LogicalIndexScan>() {
        OperandIndexScan
    } else if plan.is::<logicalop::LogicalShow>() {
        OperandShow
    } else if plan.is::<logicalop::LogicalWindow>() {
        OperandWindow
    } else {
        OperandUnsupported
    }
}

impl Operand {
    /// 判断两个 Operand 是否匹配：任一方为 Any，或两者相等。
    pub fn Match(self, target: Operand) -> bool {
        self == OperandAny || target == OperandAny || self == target
    }
}

/// 规则匹配模式树：当前节点的 Operand、允许的引擎集合，以及子模式列表。
#[derive(Clone, Debug)]
pub struct Pattern {
    /// 本节点期望匹配的算子类别。
    pub Operand: Operand,
    /// 本节点允许生效的执行引擎集合（如仅 TiDB、仅 TiKV 或全部）。
    pub EngineTypeSet: crate::EngineTypeSet,
    /// 子模式；为空表示不约束子树形状。
    pub Children: Vec<Pattern>,
}

impl Pattern {
    /// 检查给定 Operand 与引擎是否同时满足本模式节点。
    pub fn Match(&self, operand: Operand, engine: crate::EngineType) -> bool {
        self.EngineTypeSet.Contains(engine) && self.Operand.Match(operand)
    }

    /// 检查本模式是否为 OperandAny，且引擎落在允许集合内。
    pub fn MatchOperandAny(&self, engine: crate::EngineType) -> bool {
        self.EngineTypeSet.Contains(engine) && self.Operand == OperandAny
    }

    /// 替换本模式的子模式列表。
    pub fn SetChildren(&mut self, children: Vec<Pattern>) {
        self.Children = children;
    }
}

/// 创建无子节点的 Pattern。
pub fn NewPattern(operand: Operand, engines: crate::EngineTypeSet) -> Pattern {
    Pattern {
        Operand: operand,
        EngineTypeSet: engines,
        Children: Vec::new(),
    }
}

/// 创建带给定子模式列表的 Pattern。
pub fn BuildPattern(
    operand: Operand,
    engines: crate::EngineTypeSet,
    children: Vec<Pattern>,
) -> Pattern {
    Pattern {
        Operand: operand,
        EngineTypeSet: engines,
        Children: children,
    }
}
