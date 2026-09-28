// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn non_finite_factor_and_scale_formatting_matches_go() {
    let factor = CostVer2Factor {
        name: "cpu".to_owned(),
        value: f64::INFINITY,
    };
    assert_eq!(factor.to_string(), "cpu(+Inf)");
    assert_eq!(
        CostVer2Factor {
            name: "cpu".to_owned(),
            value: 1_000_000.0,
        }
        .to_string(),
        "cpu(1e+06)"
    );
    assert_eq!(
        CostVer2Factor {
            name: "cpu".to_owned(),
            value: 0.00001,
        }
        .to_string(),
        "cpu(1e-05)"
    );

    let option = new_default_plan_cost_option().with_cost_flag(COST_FLAG_TRACE);
    let cost = new_cost_ver2(
        Some(&option),
        CostVer2Factor {
            name: "cpu".to_owned(),
            value: 1.0,
        },
        1.0,
        || "rows*cpu".to_owned(),
    );

    let divided = div_cost_ver2(&cost, f64::INFINITY);
    assert_eq!(
        divided.get_trace().unwrap().get_formula(),
        "(rows*cpu)/+Inf"
    );

    let multiplied = mul_cost_ver2(&cost, f64::NEG_INFINITY);
    assert_eq!(
        multiplied.get_trace().unwrap().get_formula(),
        "(rows*cpu)*-Inf"
    );
}
