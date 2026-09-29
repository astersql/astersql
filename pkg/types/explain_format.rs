// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `EXPLAIN` 输出格式名称常量。
//
// 定义 brief/dot/json/row 等格式字符串，以及全部合法格式列表，
// 供解析器与执行计划展示（执行计划：优化器选定的算子树）使用。

/// 简要格式（brief）。
pub const ExplainFormatBrief: &str = "brief";
/// Graphviz DOT 格式。
pub const ExplainFormatDOT: &str = "dot";
/// 优化器 Hint 格式。
pub const ExplainFormatHint: &str = "hint";
/// JSON 格式。
pub const ExplainFormatJSON: &str = "json";
/// 按行表格格式（row）。
pub const ExplainFormatROW: &str = "row";
/// 详细格式（verbose）。
pub const ExplainFormatVerbose: &str = "verbose";
/// 传统表格格式。
pub const ExplainFormatTraditional: &str = "traditional";
/// 带真实基数代价的格式。
pub const ExplainFormatTrueCardCost: &str = "true_card_cost";
/// 二进制格式。
pub const ExplainFormatBinary: &str = "binary";
/// TiDB 扩展 JSON 格式。
pub const ExplainFormatTiDBJSON: &str = "tidb_json";
/// 代价追踪格式。
pub const ExplainFormatCostTrace: &str = "cost_trace";
/// 计划缓存相关格式。
pub const ExplainFormatPlanCache: &str = "plan_cache";
/// 计划树格式。
pub const ExplainFormatPlanTree: &str = "plan_tree";
/// RU 代价输出格式。
pub const ExplainFormatRU: &str = "ru";

/// 全部合法 EXPLAIN 格式名称列表。
pub const ExplainFormats: &[&str] = &[
    ExplainFormatBrief,
    ExplainFormatDOT,
    ExplainFormatHint,
    ExplainFormatJSON,
    ExplainFormatROW,
    ExplainFormatVerbose,
    ExplainFormatTraditional,
    ExplainFormatTrueCardCost,
    ExplainFormatBinary,
    ExplainFormatTiDBJSON,
    ExplainFormatCostTrace,
    ExplainFormatPlanCache,
    ExplainFormatPlanTree,
    ExplainFormatRU,
];
