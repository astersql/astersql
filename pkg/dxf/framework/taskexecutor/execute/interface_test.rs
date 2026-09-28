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

// `SubtaskSummary::GetSpeedInTimeRange` 的表驱动速度估算测试。
//
// 覆盖采样点不足、时间窗无重叠、部分重叠、多段重叠与整段对齐等情形，
// 与 Go 侧用例期望值对齐。

use astersql_dxf_framework_taskexecutor_execute::{Progress, SubtaskSummary};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 单条速度估算用例：采样点、查询窗与期望速度。
struct SpeedTestCase {
    name: &'static str,
    progresses: Vec<(i64, SystemTime)>,
    end_time: SystemTime,
    duration: Duration,
    expected: i64,
    description: &'static str,
}

/// 由 UNIX 秒构造 SystemTime。
fn unix_time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// 构造 (Processed, UpdateTime) 采样对。
fn progress(processed: i64, update_time: SystemTime) -> (i64, SystemTime) {
    (processed, update_time)
}

#[test]
/// 跑完整用例表，断言 GetSpeedInTimeRange 结果。
fn test_subtask_summary_get_speed() {
    let base_time = unix_time(1000);
    let cases = vec![
        SpeedTestCase {
            name: "insufficient data points",
            progresses: vec![progress(100, base_time)],
            end_time: unix_time(1010),
            duration: Duration::from_secs(10),
            expected: 0,
            description: "should return 0 when less than 2 data points",
        },
        SpeedTestCase {
            name: "no overlap with data range",
            progresses: vec![
                progress(0, base_time),
                progress(100, base_time + Duration::from_secs(1)),
            ],
            end_time: unix_time(1010),
            duration: Duration::from_secs(1),
            expected: 0,
            description: "should return 0 when time range doesn't overlap with data",
        },
        SpeedTestCase {
            name: "partial time range overlap",
            progresses: vec![
                progress(0, base_time),
                progress(50, base_time + Duration::from_secs(1)),
                progress(100, base_time + Duration::from_secs(2)),
                progress(150, base_time + Duration::from_secs(3)),
            ],
            end_time: base_time + Duration::from_millis(2500),
            duration: Duration::from_secs(1),
            expected: 50,
            description: "should handle partial time range overlap correctly",
        },
        SpeedTestCase {
            name: "partial time range overlap",
            progresses: vec![
                progress(0, base_time),
                progress(30, base_time + Duration::from_secs(1)),
                progress(60, base_time + Duration::from_secs(2)),
                progress(90, base_time + Duration::from_secs(3)),
            ],
            end_time: unix_time(1004),
            duration: Duration::from_millis(1500),
            expected: 10,
            description: "should handle partial time range overlap correctly",
        },
        SpeedTestCase {
            name: "multiple overlapping",
            progresses: vec![
                progress(0, base_time),
                progress(60, base_time + Duration::from_secs(1)),
                progress(120, base_time + Duration::from_secs(2)),
                progress(180, base_time + Duration::from_secs(3)),
                progress(240, base_time + Duration::from_secs(4)),
            ],
            end_time: base_time + Duration::from_millis(4500),
            duration: Duration::from_secs(2),
            expected: 45,
            description: "should handle multiple overlapping segments correctly",
        },
        SpeedTestCase {
            name: "exact match the range",
            progresses: vec![
                progress(0, base_time),
                progress(60, base_time + Duration::from_secs(1)),
                progress(120, base_time + Duration::from_secs(2)),
                progress(180, base_time + Duration::from_secs(3)),
                progress(240, base_time + Duration::from_secs(4)),
            ],
            end_time: unix_time(1004),
            duration: Duration::from_secs(4),
            expected: 60,
            description: "should handle range correctly",
        },
        SpeedTestCase {
            name: "whole range",
            progresses: vec![
                progress(0, base_time + Duration::from_secs(1)),
                progress(60, base_time + Duration::from_secs(2)),
                progress(120, base_time + Duration::from_secs(3)),
                progress(180, base_time + Duration::from_secs(4)),
                progress(240, base_time + Duration::from_secs(5)),
            ],
            end_time: base_time + Duration::from_millis(6500),
            duration: Duration::from_secs(6),
            expected: 40,
            description: "should handle multiple overlapping segments correctly",
        },
    ];

    for case in cases {
        let summary = SubtaskSummary {
            Progresses: case
                .progresses
                .into_iter()
                .map(|(processed, update_time)| Progress {
                    RowCnt: 0,
                    Processed: processed,
                    UpdateTime: update_time,
                })
                .collect(),
            ..SubtaskSummary::default()
        };

        assert_eq!(
            case.expected,
            summary.GetSpeedInTimeRange(case.end_time, case.duration),
            "{}: {}",
            case.name,
            case.description
        );
    }
}
