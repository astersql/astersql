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

// 预处理语句计划缓存（Plan Cache）在会话运行时的数据结构。
//
// 计划缓存复用已编译的物理执行计划，避免重复优化；此处定义过程列表快照、
// 执行结果以及内部缓存条目，供 `runtime` 中的 Prepare/Execute 路径使用。

#![allow(non_snake_case)]

use std::sync::Arc;

use astersql_parser_ast as ast;
use astersql_planner_core::PlanCacheStmt;

/// 执行计划过程列表快照：算子名与索引范围字符串，便于测试与 SHOW 类输出。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessPlanSnapshot {
    /// 物理计划算子名称列表。
    pub Operators: Vec<String>,
    /// 索引范围（Index Range）描述字符串。
    pub IndexRanges: Vec<String>,
}

/// 预处理 KV Select 的一次执行结果：行集、是否命中缓存、告警与计划快照。
#[derive(Clone, Debug)]
pub struct PreparedPlannedKVResult {
    /// 查询返回的行。
    pub Rows: Vec<astersql_executor_sortexec::Row>,
    /// 本次是否从计划缓存取出计划（`true` 表示命中缓存）。
    pub FromPlanCache: bool,
    /// 执行过程产生的告警（如 range 过大跳过缓存）。
    pub Warnings: Vec<String>,
    /// 本次使用的过程列表快照。
    pub Plan: ProcessPlanSnapshot,
}

/// Parameter-bound canonical physical plan before any KV rows are fetched.
/// Both the existing materialized API and the typed adapter consume this same
/// plan, preserving cache-key, parameter-type, and range-fallback decisions.
pub struct PreparedKVPhysicalPlan {
    pub Plan: Box<dyn astersql_planner_core_base::PhysicalPlan>,
    pub FromPlanCache: bool,
    pub Warnings: Vec<String>,
    pub Snapshot: ProcessPlanSnapshot,
    pub SelectLimit: u64,
    pub SQLText: String,
    pub(crate) CachedValue: Option<Arc<astersql_planner_core::PlanCacheValue>>,
    pub(crate) PendingCache: Option<(String, astersql_planner_core::PlanCacheValue)>,
}

/// 已 Prepare 的 KV Select 内部状态：AST、元信息、缓存键与可选缓存计划。
pub(crate) struct PreparedPlannedKVSelect {
    /// 预处理语句的 AST 根节点。
    pub Ast: ast::NodeRef,
    /// 绑定到该语句的信息模式（InfoSchema，表结构元数据视图）。
    pub InfoSchema: Arc<dyn astersql_infoschema::infoschema::InfoSchema>,
    /// 计划缓存语句描述（参数化后的计划键组件）。
    pub Statement: PlanCacheStmt,
    /// 绑定参数个数。
    pub ParameterCount: usize,
}
