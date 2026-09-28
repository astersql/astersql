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

// costusage 迁移单元测试：标志位、懒追踪、求和/缩放与 get_cost 边界行为。

use super::*;
use std::cell::Cell;

/// 构造开启 TRACE 的带公式 CostVer2。
fn traced_cost(name: &str, value: f64, formula: &str) -> CostVer2 {
    let option = new_default_plan_cost_option().with_cost_flag(COST_FLAG_TRACE);
    new_cost_ver2(
        Some(&option),
        CostVer2Factor {
            name: name.to_owned(),
            value,
        },
        value,
        || formula.to_owned(),
    )
}

/// 验证标志位检测，以及未开 TRACE 时 lazy_formula 不被求值。
#[test]
fn migration_flags_and_lazy_trace_match_go() {
    assert!(has_cost_flag(
        COST_FLAG_TRACE | COST_FLAG_RECALCULATE,
        COST_FLAG_TRACE
    ));
    assert!(!has_cost_flag(COST_FLAG_RECALCULATE, COST_FLAG_TRACE));
    assert!(!trace_cost(None));

    let called = Cell::new(false);
    let plain = new_cost_ver2(
        None,
        CostVer2Factor {
            name: "cpu".to_owned(),
            value: 2.5,
        },
        7.5,
        || {
            called.set(true);
            "must stay lazy".to_owned()
        },
    );
    assert!(!called.get());
    assert!(plain.get_trace().is_none());
    assert_eq!(plain.get_cost(), 7.5);
}

/// 验证零代价常量、求和合并因子，以及空 formula 不拼入表达式。
#[test]
fn migration_trace_sum_and_zero_formula_match_go() {
    assert_eq!(ZERO_COST_VER2.get_cost(), 0.0);
    assert!(ZERO_COST_VER2.get_trace().is_none());

    let cpu = traced_cost("cpu", 2.0, "rows*cpu");
    let cpu_again = traced_cost("cpu", 3.0, "more*cpu");
    let zero = new_zero_cost_ver2(true);
    let sum = sum_cost_ver2(&[cpu, zero, cpu_again]);

    assert_eq!(sum.get_cost(), 5.0);
    let trace = sum.get_trace().expect("summed trace");
    assert_eq!(trace.get_factor_costs().get("cpu"), Some(&5.0));
    assert_eq!(trace.get_formula(), "(rows*cpu) + (more*cpu)");
}

/// 验证除法/乘法同步缩放因子成本与公式字符串。
#[test]
fn migration_division_and_multiplication_scale_trace() {
    let base = traced_cost("network", 12.0, "rows*network");
    let divided = div_cost_ver2(&base, 4.0);
    assert_eq!(divided.get_cost(), 3.0);
    let trace = divided.get_trace().unwrap();
    assert_eq!(trace.get_factor_costs().get("network"), Some(&3.0));
    assert_eq!(trace.get_formula(), "(rows*network)/4.00");

    let multiplied = mul_cost_ver2(&divided, 2.5);
    assert_eq!(multiplied.get_cost(), 7.5);
    let trace = multiplied.get_trace().unwrap();
    assert_eq!(trace.get_factor_costs().get("network"), Some(&7.5));
    assert_eq!(trace.get_formula(), "((rows*network)/4.00)*2.50");
}

/// 验证 get_cost 将负数钳制为 0，并原样保留 NaN。
#[test]
fn migration_get_cost_clamps_negative_and_preserves_nan() {
    let negative = new_cost_ver2(
        None,
        CostVer2Factor {
            name: "cpu".to_owned(),
            value: -1.0,
        },
        -1.0,
        String::new,
    );
    assert_eq!(negative.get_cost(), 0.0);

    let nan = new_cost_ver2(
        None,
        CostVer2Factor {
            name: "cpu".to_owned(),
            value: f64::NAN,
        },
        f64::NAN,
        String::new,
    );
    assert!(nan.get_cost().is_nan());
}

/// 验证因子 Display 格式，以及无追踪加数不改动公式/因子。
#[test]
fn migration_factor_display_and_untraced_tie_breaker_match_go() {
    let factor = CostVer2Factor {
        name: "cpu".to_owned(),
        value: 2.5,
    };
    assert_eq!(factor.to_string(), "cpu(2.5)");

    let cost = add_cost_without_trace(traced_cost("cpu", 2.0, "rows*cpu"), 0.25);
    assert_eq!(cost.get_cost(), 2.25);
    let trace = cost.get_trace().unwrap();
    assert_eq!(trace.get_formula(), "rows*cpu");
    assert_eq!(trace.get_factor_costs().get("cpu"), Some(&2.0));
}
