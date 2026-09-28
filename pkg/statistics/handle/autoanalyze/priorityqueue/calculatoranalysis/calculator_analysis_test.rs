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

// Same-path Go->Rust mapping for `calculator_analysis_test.go`. It drives the
// real `priorityqueue::PriorityCalculator` (canonical `CalculateWeight`
// formula, shared with the Go implementation) over the same generated
// dataset and compares against the same-path golden CSV fixture.
//
// 用真实 `PriorityCalculator` 对生成数据集打分，按权重降序输出 CSV，
// 并与 Go 同路径 golden 文件逐字节比对；可通过环境变量更新 golden。

use std::any::Any;
use std::fmt;
use std::time::Duration;

use astersql_statistics_handle_autoanalyze_priorityqueue as priorityqueue;
use priorityqueue::{AnalysisJob, AnalysisJobJSON, FailureJobHook, Indicators, SuccessJobHook};

/// 基准变化率：每秒 0.1%。
const BASE_CHANGE_RATE: f64 = 0.001; // Base change rate: 0.1% per second.
/// 大表变化率随规模对数衰减的强度。
const CHANGE_RATE_DECAY_LOG: f64 = 3.0; // Controls how quickly the change rate decays for larger tables.
/// 小于该行数的表使用基准变化率。
const SMALL_TABLE_THRESHOLD: i64 = 100_000; // Tables smaller than this use the base change rate.
/// 最大变化量上限：表规模的 300%。
const MAX_CHANGE_PERCENTAGE: f64 = 3.0; // Maximum change capped at 300% of table size.

/// `TestJob` mirrors Go's `TestJob`: only the accessors `CalculateWeight`
/// depends on (`GetIndicators`, `GetTableID`, `HasNewlyAddedIndex`) carry real
/// behavior; every other `AnalysisJob` method panics exactly like the Go
/// counterpart because the golden-file test never calls them.
///
/// 仅实现权重计算所需访问器的测试作业；其余 `AnalysisJob` 方法与 Go 一样 panic。
#[derive(Clone)]
struct TestJob {
    /// 作业/表 ID。
    id: i32,
    /// 表规模。
    table_size: f64,
    /// 累计变化量。
    changes: f64,
    /// 距上次分析的秒数。
    time_since_last_analyze: f64,
}

impl fmt::Display for TestJob {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("unimplemented")
    }
}

impl AnalysisJob for TestJob {
    fn ValidateAndPrepare(
        &mut self,
        _runtime: &dyn priorityqueue::AnalysisRuntime,
    ) -> (bool, String) {
        panic!("unimplemented")
    }

    fn Analyze(&mut self, _runtime: &dyn priorityqueue::AnalysisRuntime) -> Result<(), String> {
        panic!("unimplemented")
    }

    fn SetWeight(&mut self, _weight: f64) {
        panic!("unimplemented")
    }

    fn GetWeight(&self) -> f64 {
        panic!("unimplemented")
    }

    fn HasNewlyAddedIndex(&self) -> bool {
        false
    }

    fn GetIndicators(&self) -> Indicators {
        Indicators {
            ChangePercentage: self.changes / self.table_size,
            TableSize: self.table_size,
            // Go first converts the float to `time.Duration`, truncating any
            // fractional part, and only then scales it from seconds.
            LastAnalysisDuration: priorityqueue::AnalysisDuration::from_secs(
                self.time_since_last_analyze as i64,
            ),
        }
    }

    fn SetIndicators(&mut self, _indicators: Indicators) {
        panic!("unimplemented")
    }

    fn GetTableID(&self) -> i64 {
        self.id as i64
    }

    fn RegisterSuccessHook(&mut self, _hook: SuccessJobHook) {
        panic!("unimplemented")
    }

    fn RegisterFailureHook(&mut self, _hook: FailureJobHook) {
        panic!("unimplemented")
    }

    fn AsJSON(&self) -> AnalysisJobJSON {
        panic!("unimplemented")
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[test]
fn test_job_string_panics_like_go() {
    let job = TestJob {
        id: 1,
        table_size: 1_000.0,
        changes: 10.0,
        time_since_last_analyze: 10.0,
    };

    assert!(
        std::panic::catch_unwind(|| job.to_string()).is_err(),
        "Go TestJob.String panics because the method is deliberately unimplemented"
    );
}

#[test]
fn test_job_indicators_truncate_fractional_seconds_like_go() {
    let job = TestJob {
        id: 1,
        table_size: 1_000.0,
        changes: 10.0,
        time_since_last_analyze: 1.9,
    };

    assert_eq!(
        job.GetIndicators().LastAnalysisDuration,
        Duration::from_secs(1),
        "Go converts the floating-point value to time.Duration before multiplying by time.Second"
    );
}

struct JobWithPriority {
    /// 原始测试作业。
    job: TestJob,
    /// 计算出的优先级权重。
    priority: f64,
    /// 变化率 = changes / table_size。
    change_ratio: f64,
}

/// `calculateNewPriorities` in Go: run the real calculator over every job and
/// record its change ratio alongside the computed weight.
fn calculate_new_priorities(
    jobs: &[TestJob],
    calculator: &priorityqueue::PriorityCalculator,
) -> Vec<JobWithPriority> {
    jobs.iter()
        .map(|job| {
            let priority = calculator.CalculateWeight(job);
            let change_ratio = job.changes / job.table_size;
            JobWithPriority {
                job: job.clone(),
                priority,
                change_ratio,
            }
        })
        .collect()
}

/// `sortPrioritiesByWeight` in Go: `slices.SortStableFunc` descending by
/// priority. `sort_by` in Rust is also a stable sort, matching Go's guarantee.
fn sort_priorities_by_weight(priorities: &mut [JobWithPriority]) {
    priorities.sort_by(|left, right| right.priority.total_cmp(&left.priority));
}

/// `calculateMaxChange` in Go: small tables use the base change rate; larger
/// tables decay the rate logarithmically, and the result is capped at
/// `MAX_CHANGE_PERCENTAGE` of the table size.
fn calculate_max_change(table_size: i64, time_since_last_analyze: i64) -> i64 {
    let change_rate = if table_size < SMALL_TABLE_THRESHOLD {
        BASE_CHANGE_RATE
    } else {
        BASE_CHANGE_RATE * 0.5_f64.powf((table_size as f64).log10() / CHANGE_RATE_DECAY_LOG)
    };
    let max_change = table_size as f64 * change_rate * time_since_last_analyze as f64;
    max_change.min(table_size as f64 * MAX_CHANGE_PERCENTAGE) as i64
}

/// `generateCombinations` in Go: enumerate table sizes and analyze intervals,
/// emitting six change amounts (10%..300% of the reasonable maximum) per
/// combination, discarding non-positive or over-sized samples.
fn generate_combinations(table_sizes: &[i64], analyze_times: &[i64]) -> Vec<[i64; 4]> {
    let mut combinations = Vec::new();
    let mut id: i64 = 1;
    for &size in table_sizes {
        for &time_since_last_analyze in analyze_times {
            let max_change = calculate_max_change(size, time_since_last_analyze);
            let changes = [
                max_change / 10,
                max_change / 5,
                max_change / 2,
                max_change,
                max_change * 2,
                max_change * 3,
            ];
            for change in changes {
                if change > 0 && change <= size * 3 {
                    combinations.push([id, size, change, time_since_last_analyze]);
                    id += 1;
                }
            }
        }
    }
    combinations
}

/// `generateTestData` in Go: the fixed table-size and analyze-time grids used
/// to build the golden-file dataset.
fn generate_test_data() -> Vec<TestJob> {
    let table_sizes = [
        1_000_i64,
        5_000,
        10_000,
        50_000,
        100_000,
        500_000,
        1_000_000,
        5_000_000,
        10_000_000,
        50_000_000,
        100_000_000,
    ];
    let analyze_times = [
        10_i64, 60, 300, 900, 1_800, 3_600, 7_200, 14_400, 28_800, 43_200, 86_400, 172_800, 259_200,
    ];

    generate_combinations(&table_sizes, &analyze_times)
        .into_iter()
        .map(|combo| TestJob {
            id: combo[0] as i32,
            table_size: combo[1] as f64,
            changes: combo[2] as f64,
            time_since_last_analyze: combo[3] as f64,
        })
        .collect()
}

const GOLDEN_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/calculated_priorities.golden.csv"
);

/// `TestPriorityCalculatorWithGeneratedData` in Go: compute priorities for
/// the generated dataset with the real `PriorityCalculator`, sort by weight
/// descending, format as CSV, and compare byte-for-byte against the same
/// golden fixture the Go test reads.
///
/// Go's `-update` flag rewrites the golden file on demand; the Rust test
/// mirrors that through the `UPDATE_CALCULATOR_GOLDEN` environment variable
/// so the fixture can still be regenerated without a CLI flag parser.
///
/// 生成数据集 → 真实计算器打分 → 按权重降序写 CSV → 与 golden 比对；
/// 设置 `UPDATE_CALCULATOR_GOLDEN` 可重写 golden。
#[test]
fn test_priority_calculator_with_generated_data() {
    let jobs = generate_test_data();
    let calculator = priorityqueue::NewPriorityCalculator();

    let mut new_priorities = calculate_new_priorities(&jobs, &calculator);
    sort_priorities_by_weight(&mut new_priorities);

    let mut csv =
        String::from("ID,CalculatedPriority,TableSize,Changes,TimeSinceLastAnalyze,ChangeRatio\n");
    for p in &new_priorities {
        csv.push_str(&format!(
            "{},{:.4},{:.0},{:.0},{:.0},{:.4}\n",
            p.job.id,
            p.priority,
            p.job.table_size,
            p.job.changes,
            p.job.time_since_last_analyze,
            p.change_ratio,
        ));
    }

    // 与 Go `-update` 等价：存在该环境变量时回写 golden。
    if std::env::var_os("UPDATE_CALCULATOR_GOLDEN").is_some() {
        std::fs::write(GOLDEN_FILE, &csv).expect("failed to update golden file");
    }

    let want = std::fs::read_to_string(GOLDEN_FILE).expect("failed to read golden file");
    assert_eq!(want, csv);
}

#[test]
fn test_job_indicators_keep_negative_seconds_like_go() {
    let job = TestJob {
        id: 1,
        table_size: 1000.0,
        changes: 10.0,
        time_since_last_analyze: -1.9,
    };
    assert_eq!(
        job.GetIndicators().LastAnalysisDuration.as_nanos(),
        -1_000_000_000
    );
}
