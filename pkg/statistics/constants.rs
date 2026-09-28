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

// 统计信息模块的默认常量。
//
// TopN（频率最高的 N 个取值）与直方图（Histogram，按值域分桶统计行数）
// 是查询优化器估算选择率（selectivity）与行数的核心结构；
// ANALYZE 未显式指定参数时使用本文件中的默认规模。

/// TopN 默认保留的条目数。
///
/// ANALYZE 收集列/索引统计时，若未指定 `TOPN`，最多保留该数量的高频值及其出现次数。
pub const DefaultTopNValue: usize = 100;
/// 直方图默认桶数。
///
/// ANALYZE 未指定 `BUCKETS` 时，等深（equal-depth）直方图按该桶数切分值域。
pub const DefaultHistogramBuckets: usize = 256;
