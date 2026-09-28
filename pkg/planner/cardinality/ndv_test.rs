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

// NDV 估算相关单元测试：ScaleNDV 均匀/偏斜混合、风险变量，以及列组
// 指数退避与 GroupNDV 优先路径。

use std::collections::HashMap;

use crate::*;

/// Lightweight plan context matching the Go test's `mock.Context`: NDV tests
/// only consume session variables, so expression and range access stay invalid.
struct TestContext {
    vars: variable::SessionVars,
}

impl TestContext {
    fn with_group_ndv_skew_ratio(ratio: f64) -> Self {
        let mut vars = variable::SessionVars::default();
        vars.RiskGroupNDVSkewRatio = ratio;
        Self { vars }
    }
}

impl CardinalityContext for TestContext {
    fn GetSessionVars(&self) -> &variable::SessionVars {
        &self.vars
    }

    fn GetExprCtx(&self) -> &dyn planctx_dependency::exprctx::ExprContext {
        panic!("NDV estimator tests do not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx_dependency::rangerctx::RangerContext<'_> {
        panic!("NDV estimator tests do not build ranges")
    }
}

/// 将浮点结果格式化为两位小数字符串，便于与 Go 期望值逐字比较。
fn rounded(value: f64) -> String {
    format!("{value:.2}")
}

/// 在 RiskScaleNDVSkewRatio=0（纯均匀模型）下核对 ScaleNDV 的若干边界与中间点。
#[test]
fn test_scale_ndv() {
    crate::main_test::setup_for_cardinality_test();
    let mut vars = variable::SessionVars::default();
    vars.RiskScaleNDVSkewRatio = 0.0;
    let cases = [
        (0.0, 0.0, 0.0, 0.0),
        (10.0, 0.0, 100.0, 0.0),
        (10.0, 100.0, 100.0, 10.0),
        (10.0, 100.0, 1.0, 1.0),
        (10.0, 100.0, 2.0, 1.83),
        (10.0, 100.0, 10.0, 6.51),
        (10.0, 100.0, 50.0, 9.99),
        (10.0, 100.0, 80.0, 10.00),
        (10.0, 100.0, 90.0, 10.00),
    ];
    for (original_ndv, original_rows, selected_rows, expected) in cases {
        assert_eq!(
            rounded(expected),
            rounded(ScaleNDV(
                Some(&vars),
                original_ndv,
                original_rows,
                selected_rows,
            )),
            "original_ndv={original_ndv}, original_rows={original_rows}, selected_rows={selected_rows}",
        );
    }
}

/// 验证调整 RiskScaleNDVSkewRatio 时 ScaleNDV 在均匀与线性偏斜模型间插值。
#[test]
fn test_opt_scale_ndv_skew_ratio_set_var() {
    crate::main_test::setup_for_cardinality_test();
    // This is the exact estimator input produced by the Go test's analyzed
    // table: NDV(a)=20, 100 rows, and 51 rows selected by b<50.
    // 与 Go 分析表输入一致：NDV(a)=20、100 行、谓词选中 51 行。
    let mut vars = variable::SessionVars::default();
    let expected = [(0.0, "19.44"), (0.5, "14.82"), (1.0, "10.20")];
    for (risk_ratio, expected_rows) in expected {
        vars.RiskScaleNDVSkewRatio = risk_ratio;
        assert_eq!(
            expected_rows,
            rounded(ScaleNDV(Some(&vars), 20.0, 100.0, 51.0)),
        );
    }
}

/// 回归 issue 54812：偏斜数据上均匀模型缩放 NDV 的期望值。
#[test]
fn test_issue_54812() {
    crate::main_test::setup_for_cardinality_test();
    // The Go fixture has 1,100 rows and 101 analyzed distinct values. The
    // predicate selects the 100 rows in the non-skewed group.
    // Go fixture：1100 行、101 个 distinct；谓词选中非偏斜组的 100 行。
    let mut vars = variable::SessionVars::default();
    vars.RiskScaleNDVSkewRatio = 0.0;
    assert_eq!(
        "65.23",
        rounded(ScaleNDV(Some(&vars), 101.0, 1100.0, 100.0)),
    );
}

/// 在给定 skew ratio 下通过正式入口估算列组 NDV。
fn estimate_with_ratio(
    columns: &[expression::Column],
    schema: &expression::Schema,
    stats: &property::StatsInfo,
    ratio: f64,
) -> (f64, usize) {
    let ctx = TestContext::with_group_ndv_skew_ratio(ratio);
    EstimateColsNDVWithMatchedLen(Some(&ctx), columns, schema, stats)
}

/// 覆盖单列、精确 GroupNDV、多列指数退避以及空键的 NDV 估算路径。
#[test]
fn test_estimate_cols_ndv_with_exponential_backoff() {
    crate::main_test::setup_for_cardinality_test();
    let col_a = expression::Column::new(*types::NewFieldType(mysql::TypeLonglong), 1, 1, 0);
    let col_b = expression::Column::new(*types::NewFieldType(mysql::TypeLonglong), 2, 2, 0);
    let col_c = expression::Column::new(*types::NewFieldType(mysql::TypeLonglong), 3, 3, 0);
    let schema = expression::NewSchema(vec![col_a.clone(), col_b.clone(), col_c.clone()]);
    let stats = property::StatsInfo {
        RowCount: 100_000.0,
        ColNDVs: HashMap::from([(1, 1000.0), (2, 500.0), (3, 10.0)]),
        GroupNDVs: vec![property::GroupNDV {
            Cols: vec![1, 2, 3],
            NDV: 5000.0,
        }],
        ..Default::default()
    };

    for (column, expected) in [(&col_a, 1000.0), (&col_b, 500.0), (&col_c, 10.0)] {
        assert_eq!(
            (expected, 1),
            EstimateColsNDVWithMatchedLen(None, std::slice::from_ref(column), &schema, &stats)
        );
    }
    assert_eq!(
        (5000.0, 3),
        EstimateColsNDVWithMatchedLen(
            None,
            &[col_a.clone(), col_b.clone(), col_c.clone()],
            &schema,
            &stats
        )
    );

    let ab = [col_a.clone(), col_b.clone()];
    let expected_ab = 1000.0 * 500.0_f64.sqrt();
    let (disabled, matched) = estimate_with_ratio(&ab, &schema, &stats, 0.0);
    assert!((disabled - 1000.0).abs() <= 0.1);
    assert_eq!(matched, 1);
    let (enabled, matched) = estimate_with_ratio(&ab, &schema, &stats, 1.0);
    assert!((enabled - expected_ab).abs() <= 0.1);
    assert_eq!(matched, 1);
    let (blended, matched) = estimate_with_ratio(&ab, &schema, &stats, 0.5);
    assert!((blended - (1000.0 + (expected_ab - 1000.0) * 0.5)).abs() <= 0.1);
    assert_eq!(matched, 1);
    assert!(disabled < blended && blended < enabled);

    for (columns, expected) in [
        (vec![col_a.clone(), col_c.clone()], 1000.0 * 10.0_f64.sqrt()),
        (vec![col_b.clone(), col_c.clone()], 500.0 * 10.0_f64.sqrt()),
    ] {
        let (actual, matched) = estimate_with_ratio(&columns, &schema, &stats, 1.0);
        assert!((actual - expected).abs() <= 0.1);
        assert_eq!(matched, 1);
    }

    let stats_without_group = property::StatsInfo {
        GroupNDVs: Vec::new(),
        ..stats.clone()
    };
    let abc = [col_a.clone(), col_b.clone(), col_c.clone()];
    let expected_abc = 1000.0 * 500.0_f64.sqrt() * 10.0_f64.sqrt().sqrt();
    let (actual, matched) = estimate_with_ratio(&abc, &schema, &stats_without_group, 1.0);
    assert!((actual - expected_abc).abs() <= 0.1);
    assert_eq!(matched, 1);
    assert_eq!(
        (1.0, 1),
        EstimateColsNDVWithMatchedLen(None, &[], &schema, &stats_without_group)
    );
    assert_eq!(
        (1000.0, 1),
        EstimateColsNDVWithMatchedLen(
            None,
            std::slice::from_ref(&col_a),
            &schema,
            &stats_without_group
        )
    );
}
