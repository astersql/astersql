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

// `PriorityCalculator` 单元测试。
//
// 对应 Go `calculator_test.go`：验证变化率、表规模、分析间隔对权重的单调性，
// 以及新索引特殊事件加分。

use std::collections::HashMap;
use std::time::Duration;

use crate::{
    AnalysisJob, EVENT_NEW_INDEX, EVENT_NONE, NewDynamicPartitionedTableAnalysisJob,
    NewNonPartitionedTableAnalysisJob, NewPriorityCalculator, PriorityCalculator,
};

/// 单组权重用例的输入指标。
struct TestData {
    /// 变化率。
    change_percentage: f64,
    /// 表规模估计。
    table_size: f64,
    /// 距上次分析时长。
    last_analysis_duration: Duration,
}

/// 一小时，便于构造时长用例。
const HOUR: Duration = Duration::from_secs(3600);

// testWeightCalculation 对应 Go 辅助函数：每组数据要求权重严格递增且大于 0。
fn test_weight_calculation(pc: &PriorityCalculator, group: &[TestData]) {
    let mut prev_weight = -1.0;
    for tc in group {
        let job = NewNonPartitionedTableAnalysisJob(
            0,
            HashMap::new(),
            0,
            false,
            tc.change_percentage,
            tc.table_size,
            tc.last_analysis_duration,
        );
        let weight = pc.CalculateWeight(&job);
        assert!(weight > 0.0);
        assert!(weight > prev_weight);
        prev_weight = weight;
    }
}

/// 分维度验证权重单调性：变化率↑、规模↓、间隔↑，以及“刚分析过”不被过高加权。
#[test]
fn TestCalculateWeight() {
    // Note: all groups are sorted by weight in ascending order.
    // 各组已按期望权重升序排列。
    let pc = NewPriorityCalculator();
    // Only focus on change percentage. Bigger change percentage, higher weight.
    // 仅变化率增大时权重应严格递增。
    let change_percentage_group = [
        TestData {
            change_percentage: 0.6,
            table_size: 1000.0,
            last_analysis_duration: HOUR,
        },
        TestData {
            change_percentage: 1.0,
            table_size: 1000.0,
            last_analysis_duration: HOUR,
        },
        TestData {
            change_percentage: 10.0,
            table_size: 1000.0,
            last_analysis_duration: HOUR,
        },
    ];
    test_weight_calculation(&pc, &change_percentage_group);
    // Only focus on table size. Bigger table size, lower weight.
    // 表越大权重越低（同变化率与间隔时）。
    let table_size_group = [
        TestData {
            change_percentage: 0.6,
            table_size: 100000.0,
            last_analysis_duration: HOUR,
        },
        TestData {
            change_percentage: 0.6,
            table_size: 10000.0,
            last_analysis_duration: HOUR,
        },
        TestData {
            change_percentage: 0.6,
            table_size: 1000.0,
            last_analysis_duration: HOUR,
        },
    ];
    test_weight_calculation(&pc, &table_size_group);
    // Only focus on last analysis duration. Longer duration, higher weight.
    // 距上次分析越久权重越高。
    let last_analysis_duration_group = [
        TestData {
            change_percentage: 0.6,
            table_size: 1000.0,
            last_analysis_duration: HOUR,
        },
        TestData {
            change_percentage: 0.6,
            table_size: 1000.0,
            last_analysis_duration: 12 * HOUR,
        },
        TestData {
            change_percentage: 0.6,
            table_size: 1000.0,
            last_analysis_duration: 24 * HOUR,
        },
    ];
    test_weight_calculation(&pc, &last_analysis_duration_group);
    // The system should not assign a higher weight to a recently analyzed table,
    // even if it has undergone significant changes.
    // 刚分析过的表即使变化更大，也不应比间隔更长的表权重更高。
    let just_being_analyzed_group = [
        TestData {
            change_percentage: 0.5,
            table_size: 1000.0,
            last_analysis_duration: 2 * HOUR,
        },
        TestData {
            change_percentage: 1.0,
            table_size: 1000.0,
            last_analysis_duration: Duration::from_secs(10 * 60),
        },
    ];
    test_weight_calculation(&pc, &just_being_analyzed_group);
}

/// 验证动态/非分区作业在有无新索引时分别得到 `EVENT_NEW_INDEX` / `EVENT_NONE`。
#[test]
fn TestGetSpecialEvent() {
    let pc = NewPriorityCalculator();

    let job_with_index_1 = NewDynamicPartitionedTableAnalysisJob(
        0,
        HashMap::new(),
        HashMap::from([(1i64, vec![1i64, 2i64])]),
        0,
        false,
        0.0,
        0.0,
        Duration::ZERO,
    );
    assert_eq!(EVENT_NEW_INDEX, pc.GetSpecialEvent(&job_with_index_1));

    let job_with_index_2 = NewNonPartitionedTableAnalysisJob(
        0,
        HashMap::from([(1i64, ())]),
        0,
        false,
        0.0,
        0.0,
        Duration::ZERO,
    );
    assert_eq!(EVENT_NEW_INDEX, pc.GetSpecialEvent(&job_with_index_2));

    let job_without_index = NewDynamicPartitionedTableAnalysisJob(
        0,
        HashMap::new(),
        HashMap::new(),
        0,
        false,
        0.0,
        0.0,
        Duration::ZERO,
    );
    assert_eq!(EVENT_NONE, pc.GetSpecialEvent(&job_without_index));
}

/// 轻量回归：新索引作业应有特殊事件加分，且权重高于普通高变化率作业以外的 plain 作业。
#[test]
fn calculator_rewards_new_index_and_increasing_change_ratio() {
    let calculator = NewPriorityCalculator();
    let plain = NewNonPartitionedTableAnalysisJob(
        1,
        HashMap::new(),
        2,
        false,
        0.2,
        100.0,
        Duration::from_secs(60),
    );
    let indexed = NewNonPartitionedTableAnalysisJob(
        2,
        HashMap::from([(9, ())]),
        2,
        false,
        0.8,
        100.0,
        Duration::from_secs(60),
    );
    assert_eq!(EVENT_NONE, calculator.GetSpecialEvent(&plain));
    assert_eq!(EVENT_NEW_INDEX, calculator.GetSpecialEvent(&indexed));
    assert!(calculator.CalculateWeight(&indexed) > calculator.CalculateWeight(&plain));
    assert!(!plain.HasNewlyAddedIndex());
}
