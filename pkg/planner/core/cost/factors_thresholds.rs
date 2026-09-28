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

// Optimizer cost factors and thresholds matching `factors_thresholds.go`.
//
// 优化器代价因子与阈值常量，与 Go 版 `factors_thresholds.go` 对齐。
// 代价模型（cost model）用这些常量估算选择率、distinct 基数、聚合开销及
// 带 LIMIT 时的「小扫描」边界，从而在候选物理计划间比较相对成本。

#![allow(non_upper_case_globals)]

use crate::ast;
use std::collections::HashMap;
use std::sync::LazyLock;

/// 无法精确估算 Selection / JoinCondition 选择率时的默认因子。
// SelectionFactor 是无法精确估算 Selection 或 JoinCondition 选择率时使用的默认因子。
pub const SelectionFactor: f64 = 0.8;

/// distinct 基数估算的默认折减比例。
// DistinctFactor 对应 Go 的同名常量，作为 distinct 基数估算的默认折减比例。
pub const DistinctFactor: f64 = 0.8;

/// 浮点比较容差，吸收代价计算中的精度误差。
// ToleranceFactor 用于部分浮点比较，吸收计算过程中产生的精度误差。
pub const ToleranceFactor: f64 = 0.00001;

/// 各聚合函数的基础成本权重；未列出的函数读 `"default"` 项。
// AggFuncFactor 保存各聚合函数的基础成本权重。
// 未列出的聚合函数由调用方读取 "default" 项，行为与 Go map 保持一致。
pub static AggFuncFactor: LazyLock<HashMap<&'static str, f64>> = LazyLock::new(|| {
    HashMap::from([
        (ast::AggFuncCount, 1.0),
        (ast::AggFuncSum, 1.0),
        (ast::AggFuncSumInt, 1.0),
        (ast::AggFuncAvg, 2.0),
        (ast::AggFuncFirstRow, 0.1),
        (ast::AggFuncMax, 1.0),
        (ast::AggFuncMin, 1.0),
        (ast::AggFuncGroupConcat, 1.0),
        (ast::AggFuncBitOr, 0.9),
        (ast::AggFuncBitXor, 0.9),
        (ast::AggFuncBitAnd, 0.9),
        (ast::AggFuncVarPop, 3.0),
        (ast::AggFuncVarSamp, 3.0),
        (ast::AggFuncStddevPop, 3.0),
        (ast::AggFuncStddevSamp, 3.0),
        ("default", 1.5),
    ])
});

/// 存在 LIMIT 时判定「小扫描」的行数边界；超出则不宜套用 DescScanFactor。
// SmallScanThreshold 表示存在 LIMIT 时判定“小扫描”的行数边界。
// 实际行数远高于 limit 时，无序扫描可能更昂贵，因此调用方不会应用 DescScanFactor。
pub const SmallScanThreshold: i32 = 10000;
