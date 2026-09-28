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

// Apply 解相关规则的共享基类与轻量桩类型。
//
// 本文件为机械迁移阶段的自包含桩：提供 Pattern/Rule/LogicalPlan 等最小模型，
// 支撑 `XFDeCorrelateSimpleApply` 的 PreCheck、相关列抽取与变换逻辑单测，
// 而不依赖完整 Cascades Memo 运行时。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// 简单 Apply 解相关规则的固定 ID；与 cascades_rule::Type 保持一致。
pub const XF_DECORRELATE_SIMPLE_APPLY_ID: usize = 2;
/// 标记该 Apply 由解相关变换规则生成（中间 Apply），再次变换时可从 memo 移除。
pub const APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG: u64 = 1 << 0;

/// 本模块规则变换使用的简单错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(String);

impl Error {
    /// 由任意可转为 String 的消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}
/// 本模块统一的 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 解相关规则 Pattern 中使用的最小 Operand 集合。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operand {
    /// Apply 算子。
    Apply,
    /// 通配符。
    Any,
}

/// 规则允许生效的引擎侧。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Engine {
    /// 仅在 TiDB 侧匹配。
    TiDbOnly,
}

/// 轻量 Pattern：operand、引擎与子模式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    /// 本节点算子类别。
    pub operand: Operand,
    /// 允许的引擎。
    pub engine: Engine,
    /// 子模式列表。
    pub children: Vec<Pattern>,
}

impl Pattern {
    /// 创建无子节点的 Pattern。
    pub fn new(operand: Operand, engine: Engine) -> Self {
        Self {
            operand,
            engine,
            children: Vec::new(),
        }
    }

    /// 设置子模式列表。
    pub fn set_children(&mut self, children: Vec<Pattern>) {
        self.children = children;
    }
}

/// 规则元数据：规则 ID 与匹配 Pattern。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseRule {
    /// 规则唯一标识。
    pub id: usize,
    /// 匹配用模式树。
    pub pattern: Pattern,
}

impl BaseRule {
    /// 由 ID 与 Pattern 构造 BaseRule。
    pub fn new(id: usize, pattern: Pattern) -> Self {
        Self { id, pattern }
    }
}

/// 逻辑计划输出列集合（列名字符串列表）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Schema {
    /// 输出列名。
    pub columns: Vec<String>,
}

impl Schema {
    /// 判断 schema 是否包含指定列名。
    pub fn contains(&self, column: &str) -> bool {
        self.columns.iter().any(|candidate| candidate == column)
    }
}

/// 通用叶子逻辑节点：名称、schema 与相关列列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LogicalNode {
    /// 节点调试名。
    pub name: String,
    /// 输出 schema。
    pub schema: Schema,
    /// 引用外层的相关列（correlated columns）。
    pub correlated_columns: Vec<String>,
}

/// 逻辑连接节点桩：含 plan_id、类型名、schema、孩子与统计信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalJoin {
    /// Cascades 中的计划节点 ID。
    pub plan_id: u64,
    /// 计划类型名（如 "Join" / "Apply"）。
    pub plan_type: String,
    /// 输出 schema。
    pub schema: Schema,
    /// 左右孩子逻辑计划。
    pub children: Vec<LogicalPlan>,
    /// 派生统计信息占位；重新分配后清为 None。
    pub statistics: Option<String>,
}

impl LogicalJoin {
    /// 浅拷贝（当前实现即 clone），供变换时复制 Join 骨架。
    pub fn shallow_ref(&self) -> Self {
        self.clone()
    }

    /// 为 Cascades 重新分配 plan_id，并将类型改为 Join、清空统计。
    pub fn realloc_for_cascades(&mut self) {
        static NEXT_PLAN_ID: AtomicU64 = AtomicU64::new(1);
        // 全局递增分配新 plan_id，避免与 memo 中原节点冲突。
        self.plan_id = NEXT_PLAN_ID.fetch_add(1, Ordering::Relaxed);
        self.plan_type = "Join".to_owned();
        self.statistics = None;
    }
}

/// 逻辑 Apply：在 LogicalJoin 之上叠加解相关控制标志。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalApply {
    /// 内嵌的 Join 结构（共享孩子与 schema）。
    pub logical_join: LogicalJoin,
    /// 为 true 时禁止对本 Apply 做解相关。
    pub no_decorrelate: bool,
    /// 位标志集合（如 APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG）。
    pub flags: u64,
}

impl LogicalApply {
    /// 测试指定位标志是否已设置。
    pub fn has_flag(&self, flag: u64) -> bool {
        self.flags & flag != 0
    }
}

/// 本模块轻量逻辑计划枚举：Apply / Join / 叶子 Node。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogicalPlan {
    /// Apply 节点。
    Apply(LogicalApply),
    /// Join 节点。
    Join(LogicalJoin),
    /// 通用叶子节点。
    Node(LogicalNode),
}

impl LogicalPlan {
    /// 取得节点输出 schema。
    pub fn schema(&self) -> &Schema {
        match self {
            Self::Apply(apply) => &apply.logical_join.schema,
            Self::Join(join) => &join.schema,
            Self::Node(node) => &node.schema,
        }
    }

    /// 取得相关列；仅 Node 携带，其余返回空切片。
    pub fn correlated_columns(&self) -> &[String] {
        match self {
            Self::Node(node) => &node.correlated_columns,
            _ => &[],
        }
    }

    /// 若为 Apply 则返回引用，否则 None。
    pub fn as_apply(&self) -> Option<&LogicalApply> {
        match self {
            Self::Apply(apply) => Some(apply),
            _ => None,
        }
    }
}

/// Memo 中的组表达式桩：包装逻辑计划及其孩子组表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupExpression {
    /// 本表达式包装的逻辑计划。
    pub logical_plan: LogicalPlan,
    /// 孩子组表达式（通常为 Apply 的外层/内层）。
    pub children: Vec<GroupExpression>,
}

impl GroupExpression {
    /// 取得包装的逻辑计划引用。
    pub fn wrapped_logical_plan(&self) -> &LogicalPlan {
        &self.logical_plan
    }
}

/// Cascades 变换规则接口：ID、元数据、预检与变换。
pub trait Rule {
    /// 规则 ID。
    fn id(&self) -> usize;
    /// 规则元数据（含 Pattern）。
    fn base_rule(&self) -> &BaseRule;
    /// 变换前预检：返回 false 则跳过本规则。
    fn pre_check(&self, expression: &GroupExpression) -> bool;
    /// 执行变换，返回新逻辑计划列表以及是否应移除原表达式。
    fn xform(&self, expression: &GroupExpression) -> Result<(Vec<LogicalPlan>, bool)>;
}

/// Shared base for Apply decorrelation rules.
/// Apply 解相关规则的共享基类：封装 BaseRule 与通用 PreCheck。
pub struct XFDeCorrelateApplyBase {
    /// 规则元数据。
    pub base_rule: BaseRule,
}

impl XFDeCorrelateApplyBase {
    /// 预检：要求表达式为 LogicalApply，且未设置 no_decorrelate。
    pub fn pre_check(&self, apply_expression: &GroupExpression) -> bool {
        let apply = apply_expression
            .wrapped_logical_plan()
            .as_apply()
            .expect("XFDeCorrelateApplyBase requires a LogicalApply pattern");
        !apply.no_decorrelate
    }

    /// Go 风格命名别名，转发到 `pre_check`。
    #[allow(non_snake_case)]
    pub fn PreCheck(&self, apply_expression: &GroupExpression) -> bool {
        self.pre_check(apply_expression)
    }
}

/// 从内层计划抽取落在外层 schema 中的相关列。
pub fn extract_correlated_columns(inner: &LogicalPlan, outer_schema: &Schema) -> Vec<String> {
    inner
        .correlated_columns()
        .iter()
        .filter(|column| outer_schema.contains(column))
        .cloned()
        .collect()
}
