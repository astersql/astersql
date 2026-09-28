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

// 自动 ANALYZE 优先级权重计算器。
//
// 对应 Go `calculator.go`。按变化率、表规模、距上次分析时长与是否有新索引，
// 对队列中的 `AnalysisJob` 打分，分数越高越优先执行。

use crate::job::AnalysisJob;

/// 无特殊事件时的加分（0）。
pub const EVENT_NONE: f64 = 0.0;
/// 存在新增索引时的固定加分，抬高新索引分析优先级。
pub const EVENT_NEW_INDEX: f64 = 2.0;
/// 变化率项权重。
pub const CHANGE_RATIO_WEIGHT: f64 = 0.6;
/// 表规模项权重（规模越大该项越小，降低大表优先级）。
pub const SIZE_WEIGHT: f64 = 0.1;
/// 距上次分析时长项权重。
pub const ANALYSIS_INTERVAL: f64 = 0.3;

/// 优先级计算器（无状态）。
#[derive(Default)]
pub struct PriorityCalculator;

/// 构造 `PriorityCalculator`。
pub fn NewPriorityCalculator() -> PriorityCalculator {
    PriorityCalculator
}

impl PriorityCalculator {
    /// 按指标加权求和：变化率↑、规模↓、间隔↑、新索引加分。
    pub fn CalculateWeight(&self, job: &dyn AnalysisJob) -> f64 {
        let indicators = job.GetIndicators();
        // 变化率放大 100 倍后再取 log10，与 Go 公式一致。
        let change_ratio = 100.0 * indicators.ChangePercentage;
        CHANGE_RATIO_WEIGHT * (1.0 + change_ratio).log10()
            + SIZE_WEIGHT * (1.0 - (1.0 + indicators.TableSize).log10())
            + ANALYSIS_INTERVAL
                * (1.0 + indicators.LastAnalysisDuration.as_secs_f64().sqrt()).log10()
            + self.GetSpecialEvent(job)
    }

    /// 返回特殊事件加分：有新索引则 `EVENT_NEW_INDEX`，否则 `EVENT_NONE`。
    pub fn GetSpecialEvent(&self, job: &dyn AnalysisJob) -> f64 {
        if job.HasNewlyAddedIndex() {
            EVENT_NEW_INDEX
        } else {
            EVENT_NONE
        }
    }
}
