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

// 逻辑优化规则（Logical Opt Rule）的公共基础设施与精简 IR。
//
// 定义表达式、计划节点、分区元数据及 `LogicalRule` trait，供同目录各规则文件共用。
// 对应 Go 包加载时 `init` 注册的桥接入口；此处用显式类型与默认规则名列表表达。

// 初始化只注册函数指针；具体规则实现仍由后续模块提供。
//
// Go 的 init 在包加载时执行；用显式函数表达同样的注册时机。
// pub fn init() {
// 三项注册分别对应谓词简化、Join 谓词简化和 PredicatePushDown flag 设置。
//     util::apply_predicate_simplification = apply_predicate_simplification;
//     util::apply_predicate_simplification_for_join = apply_predicate_simplification_for_join;
//     util::set_predicate_push_down_flag = set_predicate_push_down_flag;
// }
//
// rule/pkg 依赖 operator/pkg 完成类型检查；rule/util 保存规则侧的桥接入口。
// 以上 util 与回调是 Go 来源中的外部符号，暂不虚构 Rust 模块和函数签名。
// */
use std::collections::{BTreeMap, BTreeSet};

/// Register the three rule-owned hooks installed by Go's package `init`.
///
/// Rust has no package initializer, so planner entry points call this idempotent
/// function before executing logical rules. The underlying slots are `OnceLock`s.
pub fn init() {
    astersql_planner_core_operator_logicalop::InstallPredicateSimplificationPassthrough();
    let _ = astersql_planner_core_rule_util::RegisterSetPredicatePushDownFlag(
        crate::set_predicate_push_down_flag,
    );
}

/// 表达式求值结果的精简表示（对应 Go 侧 Datum / Value）。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
}

/// 列或表达式的字段类型（Field Type），用于类型推断与 JOIN 键重写。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldType {
    SignedInt,
    UnsignedInt,
    Float,
    Decimal,
    Text { charset: String, collation: String },
    DateTime,
    Bool,
}

/// 标量表达式树：列引用、常量、函数调用与显式 CAST。
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Column {
        id: i64,
        field_type: FieldType,
    },
    Constant(Value),
    Scalar {
        function: String,
        args: Vec<Expr>,
        field_type: FieldType,
    },
    Cast {
        expr: Box<Expr>,
        target: FieldType,
    },
}
impl Expr {
    /// 收集表达式中出现的全部列 ID。
    pub fn columns(&self) -> BTreeSet<i64> {
        match self {
            Expr::Column { id, .. } => BTreeSet::from([*id]),
            Expr::Constant(_) => BTreeSet::new(),
            Expr::Scalar { args, .. } => args.iter().flat_map(Expr::columns).collect(),
            Expr::Cast { expr, .. } => expr.columns(),
        }
    }
    /// 返回表达式结果类型；常量无独立类型信息时返回 None。
    pub fn field_type(&self) -> Option<&FieldType> {
        match self {
            Expr::Column { field_type, .. } | Expr::Scalar { field_type, .. } => Some(field_type),
            Expr::Cast { target, .. } => Some(target),
            Expr::Constant(_) => None,
        }
    }
    /// 判断表达式是否确定性（无 rand/uuid/now 等非确定函数）。
    pub fn deterministic(&self) -> bool {
        match self {
            Expr::Scalar { function, args, .. } => {
                !matches!(function.as_str(), "rand" | "uuid" | "now")
                    && args.iter().all(Expr::deterministic)
            }
            Expr::Cast { expr, .. } => expr.deterministic(),
            _ => true,
        }
    }
}

/// Join 类型：内连接、外连接、半连接与反半连接。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    LeftOuter,
    RightOuter,
    Semi,
    AntiSemi,
}

/// 聚合函数种类（MAX/MIN/SUM/COUNT/FIRST_ROW）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggKind {
    Max,
    Min,
    Sum,
    Count,
    FirstRow,
}

/// 单个聚合表达式描述。
#[derive(Clone, Debug, PartialEq)]
pub struct AggregateExpr {
    pub kind: AggKind,
    pub args: Vec<Expr>,
    pub distinct: bool,
}

/// 表分区方式（Hash/Key/Range/List 及其按列变体）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartitionKind {
    Hash,
    Key,
    Range,
    RangeColumns,
    List,
    ListColumns,
}

/// 单个分区定义：上界（Range）或枚举值列表（List）。
#[derive(Clone, Debug, PartialEq)]
pub struct PartitionDefinition {
    pub id: i64,
    pub name: String,
    pub less_than: Vec<Value>,
    pub in_values: Vec<Vec<Value>>,
}

/// 表的分区元信息：类型、分区列与各分区定义。
#[derive(Clone, Debug, PartialEq)]
pub struct PartitionInfo {
    pub kind: PartitionKind,
    pub columns: Vec<i64>,
    pub definitions: Vec<PartitionDefinition>,
}

/// 逻辑计划节点种类（精简 IR，对应 Go Logical* 算子）。
#[derive(Clone, Debug, PartialEq)]
pub enum PlanKind {
    DataSource {
        table_id: i64,
        indexes: BTreeMap<i64, Vec<i64>>,
        partition: Option<PartitionInfo>,
        selected_partitions: Option<BTreeSet<usize>>,
    },
    Selection,
    Projection {
        expressions: Vec<Expr>,
    },
    Join {
        join_type: JoinType,
        equal_conditions: Vec<Expr>,
        other_conditions: Vec<Expr>,
    },
    Aggregation {
        aggregates: Vec<AggregateExpr>,
        group_by: Vec<Expr>,
    },
    UnionAll,
    Sort {
        by: Vec<Expr>,
    },
    Limit {
        count: usize,
    },
    PartitionUnion,
    TableDual {
        rows: usize,
    },
    Other,
}

/// 逻辑计划树节点：算子种类、输出 schema、子节点、谓词与统计占位。
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub kind: PlanKind,
    pub schema: Vec<i64>,
    pub children: Vec<Plan>,
    pub predicates: Vec<Expr>,
    pub keys: Vec<Vec<i64>>,
    pub estimated_rows: f64,
    pub used_stats: BTreeMap<i64, UsedStats>,
}

/// 计划已请求/使用的列与索引统计信息摘要。
#[derive(Clone, Debug, PartialEq)]
pub struct UsedStats {
    pub table_id: i64,
    pub columns: BTreeSet<i64>,
    pub indexes: BTreeSet<i64>,
    pub full_load: bool,
    pub pseudo: bool,
    pub version: u64,
}

impl Plan {
    /// 后序遍历并可变访问每个节点。
    pub fn walk_mut(&mut self, function: &mut impl FnMut(&mut Plan)) {
        for child in &mut self.children {
            child.walk_mut(function);
        }
        function(self);
    }
    /// 返回本节点 schema 中的列 ID 集合。
    pub fn all_columns(&self) -> BTreeSet<i64> {
        self.schema.iter().copied().collect()
    }
}

/// 逻辑优化规则接口：稳定注册名 + 对计划树做一次变换。
pub trait LogicalRule {
    fn name(&self) -> &'static str;
    fn optimize(&self, plan: Plan) -> Result<(Plan, bool), String>;
}

/// 返回默认逻辑优化流水线中规则的注册名顺序。
pub fn default_rule_names() -> Vec<&'static str> {
    vec![
        "column_pruner",
        "build_key_solver",
        "decorrelate",
        "aggregation_eliminate",
        "projection_eliminate",
        "max_min_eliminate",
        "predicate_simplification",
        "constant_propagation",
        "outer_join_to_semi_join",
        "join_key_type_cast",
        "partition_processor",
        "order_aware_join_reorder",
        "collect_predicate_columns",
        "sync_wait_stats_load",
    ]
}
