// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 指数移动平均（EMA，Exponential Moving Average）测量实现。
//
// 预热窗口内用算术平均，之后按 `factor` 做指数平滑；非线程安全。
// 常用于对延迟、QPS 等指标做平滑观测。

// ExponentialMovingAverage is an exponential moving average measurement implementation.
// It is not thread-safe.
/// 指数移动平均状态：当前值、预热累加和、平滑因子与计数。
pub struct ExponentialMovingAverage {
    value: f64,
    sum: f64,
    factor: f64,
    warmup_window: isize,
    count: isize,
}

// NewExponentialMovingAverage creates an ExponentialMovingAverage.
/// 创建 EMA；`factor` 必须落在开区间 (0, 1)。
pub fn NewExponentialMovingAverage(
    factor: f64,
    warmup_window: isize,
) -> Box<ExponentialMovingAverage> {
    if factor >= 1.0 || factor <= 0.0 {
        panic!("factor must be (0, 1)");
    }
    Box::new(ExponentialMovingAverage {
        value: 0.0,
        sum: 0.0,
        factor,
        warmup_window,
        count: 0,
    })
}

impl ExponentialMovingAverage {
    // Add a single sample and update the internal state.
    /// 追加一个样本并更新内部状态。
    pub fn Add(&mut self, value: f64) {
        // 预热窗口内使用算术平均值，保持 Go 的 count/sum/value 更新顺序。
        if self.count < self.warmup_window {
            self.count += 1;
            self.sum += value;
            self.value = self.sum / self.count as f64;
        } else {
            // warmup 结束后切换为指数移动平均公式：
            // 新值 = 旧平均值 * (1 - factor) + 样本值 * factor。
            self.value = self.value * (1.0 - self.factor) + value * self.factor;
        }
    }

    // Get the current value.
    /// 返回当前平滑后的平均值。
    pub fn Get(&self) -> f64 {
        self.value
    }
}
