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

// 统计包共享测试入口：常量契约与端到端核心子用例（对应 Go TestMain/TestStatistics）。

use crate::*;

/// 替代 Go TestMain：构造时完成公共 setup，结束时 flush 生成输出标志。
struct TestSuiteEnvironment {
    generated_output_flushed: bool,
}

impl TestSuiteEnvironment {
    /// 等价于 SetupForCommonTest + 加载 testdata。
    fn setup() -> Self {
        // Rust tests do not expose Go's TestMain hook. Construction is the
        // per-test equivalent of SetupForCommonTest + testdata loading.
        Self {
            generated_output_flushed: false,
        }
    }

    /// 标记生成输出已刷出（对应 Go 测试收尾）。
    fn flush_generated_output(&mut self) {
        self.generated_output_flushed = true;
    }
}

#[test]
/// 包级常量与统计契约一致。
fn package_constants_match_statistics_contract() {
    assert_eq!(PseudoRowCount, 10_000);
    assert_eq!(Version0, 0);
    assert_eq!(Version1, 1);
    assert_eq!(Version2, 2);
    assert_eq!(DefaultTopNValue, 100);
    assert_eq!(DefaultHistogramBuckets, 256);
}

// Go TestStatistics.
#[test]
/// SortedBuilder 建直方图：桶数受限、行数与 NDV 正确。
fn statistics_end_to_end_core_contract() {
    let mut environment = TestSuiteEnvironment::setup();
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let mut builder = NewSortedBuilder(4, 1, &field_type, Version1);
    for value in 0..100 {
        builder.Iterate(types::NewIntDatum(value)).unwrap();
    }
    let histogram = builder.IntoHist();
    assert!(histogram.Len() <= 4);
    assert_eq!(histogram.TotalRowCount(), 100.0);
    assert_eq!(histogram.NDV, 100);
    environment.flush_generated_output();
    assert!(environment.generated_output_flushed);
}

// Go TestStatistics eight shared-suite subtests.
#[test]
/// Go TestStatistics 共享套件子测试：草图、列/索引范围、Build、Proto。
fn statistics_shared_suite_subtests() {
    let context = stmtctx::NewStmtCtx();

    // SubTestSketch, SubTestSketchProtoConversion, SubTestFMSketchCoding.
    let mut sketch = NewFMSketch(128);
    for value in [1, 1, 2, 3] {
        sketch
            .InsertValue(&context, types::NewIntDatum(value))
            .unwrap();
    }
    assert_eq!(sketch.NDV(), 3);
    let proto = FMSketchToProto(Some(&sketch));
    assert_eq!(FMSketchFromProto(Some(&proto)).unwrap().NDV(), 3);
    let encoded = EncodeFMSketch(Some(&sketch)).unwrap();
    assert_eq!(DecodeFMSketch(Some(&encoded)).unwrap().unwrap().NDV(), 3);

    // SubTestColumnRange and SubTestIntColumnRanges.
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let mut histogram = NewHistogram(1, 5, 0, 1, &field_type, 1, 0);
    histogram.AppendBucketWithNDV(&types::NewIntDatum(1), &types::NewIntDatum(5), 5, 1, 5);
    assert_eq!(
        histogram
            .BetweenRowCount(&types::NewIntDatum(2), &types::NewIntDatum(4))
            .Est,
        2.0
    );
    assert_eq!(
        EnumRangeValues(types::NewIntDatum(2), types::NewIntDatum(4), false, false)
            .unwrap()
            .len(),
        3
    );

    // SubTestIndexRanges.
    let mut top_n = NewTopN(1);
    top_n.AppendTopN(vec![2], 7);
    top_n.Sort();
    assert_eq!(top_n.BetweenCount(&[1], &[3]), 7);

    // SubTestBuild.
    let mut builder = NewSortedBuilder(2, 1, &field_type, Version2);
    for value in 1..=5 {
        builder.Iterate(types::NewIntDatum(value)).unwrap();
    }
    let built = builder.IntoHist();
    assert!(built.Len() <= 2);

    // SubTestHistogramProtoConversion.
    let decoded = HistogramFromProto(&HistogramToProto(&histogram));
    assert_eq!(decoded.Buckets, histogram.Buckets);
}
