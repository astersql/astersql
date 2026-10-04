// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 统计信息 crate：直方图、草图、采样、ANALYZE 与表级统计结构。
//
// 为优化器提供列/索引选择率估计所需的数据结构与构建、合并、编解码工具；
// 测试模块按 Go 包测试文件一对一映射。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

// —— 生产模块 ——
mod analyze;
mod analyze_jobs;
mod builder;
mod cmsketch;
mod cmsketch_util;
mod column;
mod constants;
mod estimate;
mod fmsketch;
mod histogram;
mod index;
mod merge_global;
mod row_sampler;
mod runtime_stats_builder;
mod sample;
mod scalar;
mod table;

// —— 对外重导出 ——
pub use analyze::*;
pub use analyze_jobs::*;
pub use builder::*;
pub use cmsketch::*;
pub use cmsketch_util::*;
pub use column::*;
pub use constants::*;
pub use estimate::*;
pub use fmsketch::*;
pub use histogram::*;
pub use index::*;
pub use merge_global::*;
pub use row_sampler::*;
pub use runtime_stats_builder::*;
pub use sample::*;
pub use scalar::*;
pub use table::*;

// —— 测试模块（路径映射到独立测试源文件）——
#[cfg(test)]
#[path = "analyze_test.rs"]
mod analyze_test;
#[cfg(test)]
#[path = "bench_daily_test.rs"]
mod bench_daily_test;
#[cfg(test)]
#[path = "builder_test.rs"]
mod builder_test;
#[cfg(test)]
#[path = "cmsketch_test.rs"]
mod cmsketch_test;
#[cfg(test)]
#[path = "cmsketch_util_test.rs"]
mod cmsketch_util_test;
#[cfg(test)]
#[path = "estimate_test.rs"]
mod estimate_test;
#[cfg(test)]
#[path = "fmsketch_test.rs"]
mod fmsketch_test;
#[cfg(test)]
#[path = "go_merge_47_test.rs"]
mod go_merge_47_test;
#[cfg(test)]
#[path = "histogram_bench_test.rs"]
mod histogram_bench_test;
#[cfg(test)]
#[path = "histogram_test.rs"]
mod histogram_test;
#[cfg(test)]
#[path = "index_test.rs"]
mod index_test;
#[cfg(test)]
#[path = "integration_test.rs"]
mod integration_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "row_sampler_test.rs"]
mod row_sampler_test;
#[cfg(test)]
#[path = "sample_test.rs"]
mod sample_test;
#[cfg(test)]
#[path = "scalar_test.rs"]
mod scalar_test;
#[cfg(test)]
#[path = "statistics_aster_unit_test.rs"]
mod statistics_aster_unit_test;
#[cfg(test)]
#[path = "statistics_test.rs"]
mod statistics_test;
#[cfg(test)]
#[path = "table_test.rs"]
mod table_test;

#[cfg(test)]
#[path = "merge_global_test.rs"]
mod merge_global_test;
