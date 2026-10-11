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

// 绑定（binding）计划生成的测试模块。
//
// “绑定”是数据库中把某条 SQL 固定到特定执行计划（执行计划指优化器为 SQL
// 选择的具体物理执行方式，如走索引扫描还是全表扫描）的机制。本模块验证
// 计划生成器在探索不同计划时，对优化器变量（optimizer vars，即影响优化
// 器代价估算的系统变量）与 fix-control（以编号标识的优化器行为修正开关）
// 所做的“调整”逻辑是否正确，包括：
// - 布尔型开关取反（如 on 变 off）；
// - 数值型代价因子（cost factor，代价模型中衡量某类算子开销的乘数）按倍数放大；
// - 比例型（ratio）变量按步长平移并限制在安全范围内；
// - 初始状态（start state）的编码（Encode）输出保持稳定。

/// 已接入 Rust 编译的规范化测试：一次性覆盖布尔变量取反、ratio 平移、
/// cost factor 放大、fix-control 调整以及初始状态编码等核心路径。
#[test]
fn canonical_plan_state_adjustments_cover_bool_ratio_cost_and_fix_controls() {
    // 布尔型优化器变量的调整应为取反：false 调整后变为 true。
    assert_eq!(
        crate::adjustVar(
            "tidb_opt_enable_semi_join_rewrite",
            crate::AnyValue::Bool(false),
        )
        .unwrap(),
        crate::AnyValue::Bool(true)
    );
    // ratio 变量按步长 0.1 平移；用 let-else 解构确保返回的确实是浮点值。
    let crate::AnyValue::Float(ratio) = crate::adjustVar(
        "tidb_opt_ordering_index_selectivity_ratio",
        crate::AnyValue::Float(0.2),
    )
    .unwrap() else {
        panic!("ratio adjustment returned non-float")
    };
    assert!((ratio - 0.3).abs() < f64::EPSILON * 2.0);
    // cost factor 类变量按 5 倍放大：5.0 -> 25.0。
    assert_eq!(
        crate::adjustVar(
            "tidb_opt_index_scan_cost_factor",
            crate::AnyValue::Float(5.0),
        )
        .unwrap(),
        crate::AnyValue::Float(25.0)
    );
    // fix-control 调整：布尔型 fix 44855 取反（off -> ON）；
    // 数值型 fix 45132 减半（30 -> 15）。
    assert_eq!(crate::adjustFix(44855, "off").unwrap(), "ON");
    assert_eq!(crate::adjustFix(45132, "30").unwrap(), "15");
    // 由一组变量与 fix-control 构造初始状态，并要求携带 2 个索引提示
    // （index hint，指引导优化器使用特定索引的提示）。
    let state = crate::getStartState(
        &[("ratio".to_owned(), crate::AnyValue::Float(0.8))],
        &[(52869, "OFF".to_owned())],
        2,
    )
    .unwrap();
    assert_eq!(state.indexHints.len(), 2);
    // Encode 将状态序列化为逗号分隔的字符串，用于状态去重与比较。
    assert_eq!(state.Encode(), "0.8000,OFF");
}

/// Go dispatches optimizer variables by an exact vardef whitelist. Similar-looking
/// names and arbitrary booleans must not silently enlarge the search space.
#[test]
fn adjust_var_rejects_names_outside_the_go_whitelist() {
    assert!(crate::adjustVar("custom_cost_factor", crate::AnyValue::Float(1.0)).is_err());
    assert!(crate::adjustVar("custom_ratio", crate::AnyValue::Float(0.2)).is_err());
    assert!(crate::adjustVar("custom_switch", crate::AnyValue::Bool(false)).is_err());
}

/// Go's strconv.ParseInt does not trim Fix45132 values, and values at or below
/// ten are returned byte-for-byte when parsing succeeds.
#[test]
fn adjust_fix_45132_preserves_go_parsing_and_return_contract() {
    assert_eq!(crate::adjustFix(45132, "+8").unwrap(), "+8");
    assert!(crate::adjustFix(45132, " 8").is_err());
}

/// Go includes a nil index-hint option for every table so BFS can clear one
/// table's hint while retaining hints on other tables.
#[test]
fn index_hint_options_preserve_the_go_empty_choice() {
    let spec = crate::GenerationSpec {
        index_hint_options: vec![vec![None]],
        ..crate::GenerationSpec::default()
    };
    assert_eq!(crate::extractSelectIndexHints(&spec), vec![vec![None]]);
}
