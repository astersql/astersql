// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `estimate.rs` 中 NDV 估算的单元测试，覆盖 GEE 公式的上下界夹紧。

use crate::*;

/// 验证 EstimateNDVByGEE：GEE 修正、四舍五入及上下界与 Go 表格用例一致。
#[test]
fn gee_estimate_is_clamped_to_sample_ndv_and_rows() {
    assert_eq!(EstimateNDVByGEE(5, 0, 10, 100), 5);
    assert_eq!(EstimateNDVByGEE(5, 3, 10, 100), 11);
    assert_eq!(EstimateNDVByGEE(10, 3, 20, 80), 13);
    assert_eq!(EstimateNDVByGEE(10, 7, 20, 45), 14);
    assert_eq!(EstimateNDVByGEE(10, 7, 20, 10), 10);
    assert_eq!(EstimateNDVByGEE(100, 100, 100, 100), 100);
}

#[test]
#[should_panic(expected = "sampleSize should be greater than 0")]
fn gee_estimate_rejects_zero_sample_size() {
    EstimateNDVByGEE(1, 1, 0, 1);
}

#[test]
#[should_panic(expected = "sampleNDV should be greater than 0")]
fn gee_estimate_rejects_zero_sample_ndv() {
    EstimateNDVByGEE(0, 0, 1, 1);
}

#[test]
#[should_panic(expected = "rowCount should be greater than or equal to sampleNDV")]
fn gee_estimate_rejects_row_count_below_sample_ndv() {
    EstimateNDVByGEE(10, 3, 20, 9);
}

#[test]
fn calculate_estimate_ndv_matches_go_special_cases() {
    let all_singletons = topNHelper {
        sorted: (0..4)
            .map(|value| dataCnt {
                data: vec![value],
                cnt: 1,
            })
            .collect(),
        sampleSize: 4,
        singletonItems: 4,
        sumTopN: 0,
        actualNumTop: 0,
    };
    assert_eq!(calculateEstimateNDV(&all_singletons, 40), (40, 1));

    let no_singletons = topNHelper {
        sorted: vec![
            dataCnt {
                data: vec![1],
                cnt: 2,
            },
            dataCnt {
                data: vec![2],
                cnt: 2,
            },
        ],
        sampleSize: 4,
        singletonItems: 0,
        sumTopN: 0,
        actualNumTop: 0,
    };
    assert_eq!(calculateEstimateNDV(&no_singletons, 40), (2, 10));
}
