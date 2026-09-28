// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// APPROX_PERCENTILE 聚合测试。
//
// 块注释内保留 Go `TestPercentile`、`TestFix26807`、`TestFix40463` 的用例轮廓；
// 可执行部分验证 50% 序数秩选择、忽略 NULL，以及合并后源缓冲被清空。

/*
// APPROX_PERCENTILE 聚合测试、PercentileForTesting 回归用例，以及 Enum/Set 按整数处理的回归用例。

// testSlice 对应 Go 的 []int 排序适配器，用于 PercentileForTesting 选择下标。
pub type testSlice = Vec<i32>;

// TestSliceOrder 保留 Go Len/Swap/Less 三个方法的意图，供 selection.Select 类逻辑使用。
pub trait TestSliceOrder {
    fn Len(&self) -> usize;
    fn Swap(&mut self, i: usize, j: usize);
    fn Less(&self, i: usize, j: usize) -> bool;
}

impl TestSliceOrder for testSlice {
    // Len 对应 Go testSlice.Len。
    fn Len(&self) -> usize {
        self.len()
    }

    // Swap 对应 Go testSlice.Swap。
    fn Swap(&mut self, i: usize, j: usize) {
        self.swap(i, j);
    }

    // Less 对应 Go testSlice.Less。
    fn Less(&self, i: usize, j: usize) -> bool {
        self[i] < self[j]
    }
}

// PercentileAggCaseDraft 对应 Go aggTest 的 APPROX_PERCENTILE 用例轮廓。
pub struct PercentileAggCaseDraft {
    pub funcName: &'static str,
    pub fieldType: &'static str,
    pub begin: i64,
    pub end: i64,
    pub input: &'static str,
    pub expect: &'static str,
}

// test_percentile 对应 Go TestPercentile，覆盖 int/float/decimal/date/duration 等输入类型。
#[test]
pub fn test_percentile() {
    let tests = vec![
        PercentileAggCaseDraft { funcName: "ast.AggFuncApproxPercentile", fieldType: "mysql.TypeLonglong", begin: 0, end: 5, input: "nil", expect: "2" },
        PercentileAggCaseDraft { funcName: "ast.AggFuncApproxPercentile", fieldType: "mysql.TypeFloat", begin: 0, end: 5, input: "nil", expect: "2.0" },
        PercentileAggCaseDraft { funcName: "ast.AggFuncApproxPercentile", fieldType: "mysql.TypeDouble", begin: 0, end: 5, input: "nil", expect: "2.0" },
        PercentileAggCaseDraft { funcName: "ast.AggFuncApproxPercentile", fieldType: "mysql.TypeNewDecimal", begin: 0, end: 5, input: "nil", expect: "types.NewDecFromFloatForTest(2.0)" },
        PercentileAggCaseDraft { funcName: "ast.AggFuncApproxPercentile", fieldType: "mysql.TypeDate", begin: 0, end: 5, input: "nil", expect: "types.TimeFromDays(367)" },
        PercentileAggCaseDraft { funcName: "ast.AggFuncApproxPercentile", fieldType: "mysql.TypeDuration", begin: 0, end: 5, input: "nil", expect: "types.Duration{Duration: time.Duration(2)}" },
    ];

    for (i, test) in tests.into_iter().enumerate() {
        // Go 子测试名为 fmt.Sprintf("%s_%d", test.funcName, i)，再委托 testAggFunc。
        let _subtest_name = format!("{}_{}", test.funcName, i);
        testAggFunc(test);
    }
}

// test_fix_26807 对应 Go TestFix26807，验证第 100 百分位选择不会越过最大元素。
#[test]
pub fn test_fix_26807() {
    let mut data: testSlice = Vec::new();
    let want = 28;
    for i in 1..=want {
        data.push(i);
    }
    for _ in 0..10 {
        let index = aggfuncs::PercentileForTesting(&mut data, 100);
        // Go require.Equal(t, want, data[index])；这里保留反复选择后的最大值断言。
        require::Equal(want, data[index]);
    }
}

// test_fix_40463 对应 Go TestFix40463，覆盖 Enum/Set 带 EnumSetAsIntFlag 的近似百分位聚合。
#[test]
pub fn test_fix_40463() {
    let types = ["mysql.TypeEnum", "mysql.TypeSet"];
    for tp in types {
        let mut test = buildAggTester("ast.AggFuncApproxPercentile", tp, 0, 5, "nil", "nil");
        // Go 在 keyType 上追加 EnumSetAsIntFlag，表示 Enum/Set 在该回归场景按整数路径处理。
        test.keyType.AddFlag("mysql.EnumSetAsIntFlag");
        testAggFunc(test);
    }
}
*/

/// 验证 50% 百分位对 \[9,1,5,3,7\]（忽略 NULL）选出中位 5，且 merge 清空源。
#[test]
fn percentile_selects_one_based_ordinal_and_ignores_nulls() {
    // 有效样本 5 个：ceil(0.5*5)=3 → 第 3 小为 5；None 不进入样本。
    let mut percentile = crate::func_percentile::Percentile::new(50);
    percentile.update([Some(9), None, Some(1), Some(5), Some(3), Some(7)]);
    assert_eq!(percentile.result(), Some(&5));
    // 合并后源侧 samples 应被 append 清空。
    let mut source = crate::func_percentile::Percentile::new(50);
    source.update([Some(11)]);
    percentile.merge_from(&mut source);
    assert!(source.values().is_empty());
}

#[test]
fn percentile_matches_go_boundaries_memory_and_reset_lifecycle() {
    use crate::func_percentile::{DEF_SLICE_SIZE, Percentile};
    use std::mem::size_of;

    assert_eq!(DEF_SLICE_SIZE, size_of::<Vec<()>>() as i64);
    assert_eq!(crate::func_percentile::ordinal_rank(0, 100), 0);
    assert_eq!(crate::func_percentile::ordinal_rank(28, 100), 28);

    let mut percentile = Percentile::new(100);
    assert_eq!(
        percentile.update([Some(1_i64), None, Some(28)]),
        2 * size_of::<i64>() as i64
    );
    assert_eq!(percentile.result(), Some(&28));
    percentile.reset();
    assert!(percentile.values().is_empty());
    assert_eq!(
        percentile.capacity(),
        0,
        "Go reset releases the backing slice"
    );

    let mut zero = Percentile::new(0);
    zero.update([Some(1_i64)]);
    assert_eq!(zero.result(), None);
}

#[test]
fn percentile_executes_all_go_typed_paths() {
    use crate::func_max_min::{DurationValue, TimeValue};
    use crate::func_percentile::Percentile;
    use crate::func_sum::Decimal;

    let mut real32 = Percentile::new(50);
    real32.update([Some(4.0_f32), Some(2.0), Some(3.0)]);
    assert_eq!(real32.result_float32(), Some(&3.0));

    let mut real64 = Percentile::new(50);
    real64.update([Some(4.0_f64), Some(2.0), Some(3.0)]);
    assert_eq!(real64.result_float64(), Some(&3.0));

    let mut decimal = Percentile::new(50);
    decimal.update([
        Some(Decimal::new(400, 2)),
        Some(Decimal::new(200, 2)),
        Some(Decimal::new(300, 2)),
    ]);
    assert_eq!(decimal.result(), Some(&Decimal::new(300, 2)));

    let mut time = Percentile::new(50);
    time.update([4, 2, 3].map(|packed| {
        Some(TimeValue {
            packed,
            kind: 1,
            fsp: 0,
        })
    }));
    assert_eq!(time.result().map(|value| value.packed), Some(3));

    let mut duration = Percentile::new(50);
    duration.update([4, 2, 3].map(|nanos| Some(DurationValue { nanos, fsp: 0 })));
    assert_eq!(duration.result().map(|value| value.nanos), Some(3));
}
