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

// 逻辑优化规则位掩码（bit flags）定义。
//
// 每个 `FLAG_*` 对应一条逻辑改写规则在优化器流水线中的开关位。
// 位值仅用于进程内，不持久化或在线路上传输；新规则在末尾追加以避免无谓重编号。

// 优化规则位掩码常量。

// 执行顺序由优化器规则列表及显式位映射定义。
/// 生成列（Generated Column）表达式替换。
pub const FLAG_GC_SUBSTITUTE: u64 = 1 << 0;
/// 列裁剪：删除未引用列。
pub const FLAG_PRUNE_COLUMNS: u64 = 1 << 1;
/// 稳定化结果顺序（stabilize results）。
pub const FLAG_STABILIZE_RESULTS: u64 = 1 << 2;
/// 构建唯一键 / 候选键信息。
pub const FLAG_BUILD_KEY_INFO: u64 = 1 << 3;
/// 解除相关子查询（decorrelate）。
pub const FLAG_DECORRELATE: u64 = 1 << 4;
/// 半连接（Semi Join）改写。
pub const FLAG_SEMI_JOIN_REWRITE: u64 = 1 << 5;
/// 消除冗余聚合。
pub const FLAG_ELIMINATE_AGG: u64 = 1 << 6;
/// 倾斜 DISTINCT 聚合处理。
pub const FLAG_SKEW_DISTINCT_AGG: u64 = 1 << 7;
/// 消除冗余 Projection。
pub const FLAG_ELIMINATE_PROJECTION: u64 = 1 << 8;
/// MAX/MIN 聚合消除。
pub const FLAG_MAX_MIN_ELIMINATE: u64 = 1 << 9;
/// 常量传播。
pub const FLAG_CONSTANT_PROPAGATION: u64 = 1 << 10;
/// 谓词下推（Predicate Push Down）。
pub const FLAG_PREDICATE_PUSH_DOWN: u64 = 1 << 11;
/// Join 键类型强制转换。
pub const FLAG_JOIN_KEY_TYPE_CAST: u64 = 1 << 12;
/// 消除外连接。
pub const FLAG_ELIMINATE_OUTER_JOIN: u64 = 1 << 13;
/// 分区表处理器。
pub const FLAG_PARTITION_PROCESSOR: u64 = 1 << 14;
/// 收集谓词列统计加载点。
pub const FLAG_COLLECT_PREDICATE_COLUMNS_POINT: u64 = 1 << 15;
/// 聚合下推。
pub const FLAG_PUSH_DOWN_AGG: u64 = 1 << 16;
/// 从窗口函数推导 TopN。
pub const FLAG_DERIVE_TOP_N_FROM_WINDOW: u64 = 1 << 17;
/// 谓词简化。
pub const FLAG_PREDICATE_SIMPLIFICATION: u64 = 1 << 18;
/// TopN 下推。
pub const FLAG_PUSH_DOWN_TOP_N: u64 = 1 << 19;
/// 保序感知 Join 重排。
pub const FLAG_ORDER_AWARE_JOIN_REORDER: u64 = 1 << 20;
/// 同步等待统计加载点。
pub const FLAG_SYNC_WAIT_STATS_LOAD_POINT: u64 = 1 << 21;
/// Join 重排。
pub const FLAG_JOIN_REORDER: u64 = 1 << 22;
/// 外连接转半连接。
pub const FLAG_OUTER_JOIN_TO_SEMI_JOIN: u64 = 1 << 23;
/// 相关化（correlate）。
pub const FLAG_CORRELATE: u64 = 1 << 24;
/// 二次列裁剪。
pub const FLAG_PRUNE_COLUMNS_AGAIN: u64 = 1 << 25;
/// Sequence 下推。
pub const FLAG_PUSH_DOWN_SEQUENCE: u64 = 1 << 26;
/// 消除 UnionAll 中的 Dual 项。
pub const FLAG_ELIMINATE_UNION_ALL_DUAL_ITEM: u64 = 1 << 27;
/// 空 Selection 消除。
pub const FLAG_EMPTY_SELECTION_ELIMINATOR: u64 = 1 << 28;
/// 解析 Expand 算子。
pub const FLAG_RESOLVE_EXPAND: u64 = 1 << 29;
/// 全文索引：解析 WHERE。
pub const FLAG_FULLTEXT_INDEX_RESOLVE_WHERE: u64 = 1 << 30;
/// 全文索引：解析 TopN。
pub const FLAG_FULLTEXT_INDEX_RESOLVE_TOP_N: u64 = 1 << 31;
/// 全文索引：解析 Projection。
pub const FLAG_FULLTEXT_INDEX_RESOLVE_PROJECTION: u64 = 1 << 32;
/// 全文索引：拒绝不支持路径。
pub const FLAG_FULLTEXT_INDEX_RESOLVE_REJECT: u64 = 1 << 33;

// setPredicatePushDownFlag 对应 Go 的按位或操作，保留其它规则位。
/// 在已有规则位集合上开启谓词下推标志。
pub fn set_predicate_push_down_flag(flags: u64) -> u64 {
    flags | FLAG_PREDICATE_PUSH_DOWN
}
