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

// 聚合函数测试导出（export_test）模块。
//
// 对应 Go 的 `export_test.go`：把包内仅供测试使用的符号暴露给外部测试 crate。
// `PercentileForTesting` 直接重导出生产侧的百分位序数实现，避免测试复制算法。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables,
    unused_mut
)]

/// Go `PercentileForTesting = percentile` 的 crate 内测试导出。
pub(crate) use crate::func_percentile::ordinal_rank as PercentileForTesting;

/// 校验百分位序数秩（ordinal rank）在边界与中位百分位上的规范取值。
///
/// 参数含义：集合大小 n=5 时，p=0/50/100 分别对应秩 0/3/5，
/// 与 Go `percentile` 包内算法对齐，供测试侧间接验证导出路径。
#[test]
fn percentile_export_uses_the_canonical_ordinal_rank() {
    // 0%：秩为 0；50%：中间秩；100%：等于元素个数。
    assert_eq!(crate::func_percentile::ordinal_rank(5, 0), 0);
    assert_eq!(crate::func_percentile::ordinal_rank(5, 50), 3);
    assert_eq!(crate::func_percentile::ordinal_rank(5, 100), 5);

    // Go 的 `PercentileForTesting` 是 `percentile` 的直接别名；Rust 测试导出也必须
    // 调用同一生产实现，而不是复制一份测试算法。
    assert_eq!(PercentileForTesting(5, 50), 3);
}
