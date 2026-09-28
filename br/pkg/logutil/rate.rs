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

//! Rate tracing helpers ported from `br/pkg/logutil/rate.go`.
//!
//! 平均速率追踪：对齐 Go `br/pkg/logutil/rate.go`。
//! 以创建时刻的计数为基线，用「当前值 − base」/ 经过秒数得到 ops/s。
//! Counter 为空时返回 NaN；时间差保留 Go 浮点除法的零值与负值语义。

use std::time::Instant;

use astersql_lightning_metric::read_counter;
use prometheus::Counter;

use crate::logging::{Field, Logger, default_logger, log};

/// Average-rate tracer backed by a Prometheus counter.
/// 基于 Prometheus Counter 的平均速率追踪器；`base` 为创建时读数。
#[derive(Clone)]
pub struct RateTracer {
    pub start: Instant,
    pub base: f64,
    pub counter: Option<Counter>,
}

/// Creates a rate tracer that ignores the counter's current value as the baseline.
/// 以当前计数值为基线创建追踪器，之后增量才计入速率（对齐 TraceRateOver）。
pub fn TraceRateOver(counter: Counter) -> RateTracer {
    RateTracer {
        start: Instant::now(),
        base: read_counter(&counter),
        counter: Some(counter),
    }
}

impl RateTracer {
    /// 计数器 +1；无 counter 时静默忽略。
    pub fn Inc(&self) {
        if let Some(counter) = &self.counter {
            counter.inc();
        }
    }

    /// 计数器增加指定值；无 counter 时静默忽略。
    pub fn Add(&self, value: f64) {
        if let Some(counter) = &self.counter {
            counter.inc_by(value);
        }
    }

    /// 从创建到此刻的平均速率。
    pub fn Rate(&self) -> f64 {
        self.RateAt(Instant::now())
    }

    /// 算到指定时刻的速率；仍读 Counter「当前值」（Go WARN 同款，便于测试注入时间）。
    pub fn RateAt(&self, instant: Instant) -> f64 {
        let Some(counter) = &self.counter else {
            return f64::NAN;
        };
        let elapsed = if instant >= self.start {
            instant.duration_since(self.start).as_secs_f64()
        } else {
            -self.start.duration_since(instant).as_secs_f64()
        };
        (read_counter(counter) - self.base) / elapsed
    }

    /// 附带当前 speed 字段的 Logger，便于进度日志直接打印 ops/s。
    pub fn L(&self) -> Logger {
        log::L().With([Field::string("speed", format!("{:.2} ops/s", self.Rate()))])
    }
}
