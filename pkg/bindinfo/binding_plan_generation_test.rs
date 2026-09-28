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

/// 从 Go 侧 TiDB 测试机械迁移而来的测试代码草稿，以原始字符串常量形式
/// 保存，尚未接入编译。其中覆盖三个 Go 测试：
/// - `TestAdjustFixes`：fix-control 值的调整规则；
/// - `TestAdjustVars`：cost factor 放大、ratio 平移与不支持变量报错；
/// - `TestStartState`：默认变量组合的初始状态编码结果。
const GO_PLAN_GENERATION_TEST_DRAFT: &str = r########################################"
fn round_to_4_decimal(num: f64) -> f64 {
    (num * 1e4).round() / 1e4
}

// TestAdjustFixes 对应 Go 测试：不同 fix-control 的值应按生成器规则向相反方向或安全范围调整。
#[test]
fn test_adjust_fixes() {
    let (v, err) = adjustFix(fixcontrol::Fix44855, "on");
    assert!(err.is_none());
    assert_eq!(v, vardef::Off);

    let (v, err) = adjustFix(fixcontrol::Fix44855, "off      ");
    assert!(err.is_none());
    assert_eq!(v, vardef::On);

    // Fix45132 是数值型 fix，Go 逻辑对大值减半，对较小值保持原值。
    let (v, err) = adjustFix(fixcontrol::Fix45132, "1000");
    assert!(err.is_none());
    assert_eq!(v, "500");
    let (v, err) = adjustFix(fixcontrol::Fix45132, "30");
    assert!(err.is_none());
    assert_eq!(v, "15");
    let (v, err) = adjustFix(fixcontrol::Fix45132, "8");
    assert!(err.is_none());
    assert_eq!(v, "8");
}

// TestAdjustVars 对应 Go 测试：验证 cost factor 放大、ratio 平移和不支持变量错误。
#[test]
fn test_adjust_vars() {
    // adjust cost factor
    let (v, err) = adjustVar(vardef::TiDBOptIndexScanCostFactor, 1.0);
    assert!(err.is_none());
    assert_eq!(v, 5.0);
    let (v, err) = adjustVar(vardef::TiDBOptIndexScanCostFactor, 5.0);
    assert!(err.is_none());
    assert_eq!(v, 25.0);
    let (v, err) = adjustVar(vardef::TiDBOptIndexScanCostFactor, 1e5);
    assert!(err.is_none());
    assert_eq!(v, 5e5);
    let (v, err) = adjustVar(vardef::TiDBOptIndexScanCostFactor, 2e6);
    assert!(err.is_none());
    assert_eq!(v, 2e6);

    // ratio 变量在 Go 中通过 type assertion 取回 float64；直接保留浮点断言。
    let (v, err) = adjustVar(vardef::TiDBOptOrderingIdxSelRatio, -1.0);
    assert!(err.is_none());
    assert_eq!(round_to_4_decimal(v.as_f64()), 0.1);
    let (v, err) = adjustVar(vardef::TiDBOptOrderingIdxSelRatio, 0.2);
    assert!(err.is_none());
    assert_eq!(round_to_4_decimal(v.as_f64()), 0.3);
    let (v, err) = adjustVar(vardef::TiDBOptOrderingIdxSelRatio, 0.55);
    assert!(err.is_none());
    assert_eq!(round_to_4_decimal(v.as_f64()), 0.65);
    let (v, err) = adjustVar(vardef::TiDBOptOrderingIdxSelRatio, 0.95);
    assert!(err.is_none());
    assert_eq!(round_to_4_decimal(v.as_f64()), 0.95);

    let (_, err) = adjustVar(vardef::TiFlashReplicaRead, -1.0);
    assert!(err.is_some()); // unsupported
}

// TestStartState 对应 Go 测试：确认默认 optimizer vars 与 fix-control 组合的 Encode 输出稳定。
#[test]
fn test_start_state() {
    let vars = vec![
        vardef::TiDBOptIndexScanCostFactor,
        vardef::TiDBOptIndexReaderCostFactor,
        vardef::TiDBOptTableReaderCostFactor,
        vardef::TiDBOptTableFullScanCostFactor,
        vardef::TiDBOptTableRangeScanCostFactor,
        vardef::TiDBOptTableRowIDScanCostFactor,
        vardef::TiDBOptTableTiFlashScanCostFactor,
        vardef::TiDBOptIndexLookupCostFactor,
        vardef::TiDBOptIndexMergeCostFactor,
        vardef::TiDBOptSortCostFactor,
        vardef::TiDBOptTopNCostFactor,
        vardef::TiDBOptLimitCostFactor,
        vardef::TiDBOptStreamAggCostFactor,
        vardef::TiDBOptHashAggCostFactor,
        vardef::TiDBOptMergeJoinCostFactor,
        vardef::TiDBOptHashJoinCostFactor,
        vardef::TiDBOptIndexJoinCostFactor,
        vardef::TiDBOptOrderingIdxSelRatio,
        vardef::TiDBOptRiskEqSkewRatio,
        vardef::TiDBOptRiskRangeSkewRatio,
        vardef::TiDBOptRiskGroupNDVSkewRatio,
        vardef::TiDBOptSelectivityFactor,
        vardef::TiDBOptPreferRangeScan,
        vardef::TiDBOptEnableNoDecorrelateInSelect,
        vardef::TiDBOptEnableSemiJoinRewrite,
        vardef::TiDBOptCartesianJoinOrderThreshold,
    ];
    let fixes = vec![
        fixcontrol::Fix44855,
        fixcontrol::Fix45132,
        fixcontrol::Fix52869,
    ];

    let (state, err) = getStartState(vars, fixes, 0);
    assert!(err.is_none());
    assert_eq!(
        state.Encode(),
        "1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,1.0000,0.0100,0.0000,0.0000,0.0000,0.8000,true,false,false,0.0000,OFF,1000,OFF",
    );
}
"########################################;

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
