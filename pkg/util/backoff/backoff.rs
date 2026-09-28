// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 指数退避（backoff）工具：按重试次数计算下一次等待时长。
//
// 对应 Go `pkg/util/backoff`。退避（backoff）指失败重试前故意等待一段时间，
// 以减轻对下游的冲击；本模块提供无抖动的指数退避实现，时间语义按纳秒整数运算对齐 Go。

// 本文件由 pkg/util/backoff/backoff.go 迁移而来，保留 Go 实现结构和纳秒级计算语义。
//

use std::time::Duration;

// Backoffer 对应 Go 的同名接口：根据 retryCnt 返回下一次重试前应等待的时长。
/// 退避策略接口：按 `retryCnt`（从 0 起）返回下次重试前应等待的时长。
pub trait Backoffer {
    // Backoff returns the duration to wait for the retryCnt-th retry.
    // retryCnt starts from 0.
    /// 返回第 `retryCnt` 次重试前的等待时长；`retryCnt` 从 0 开始。
    fn Backoff(&mut self, retryCnt: usize) -> Duration;
}

// Exponential implements the exponential backoff algorithm without jitter.
// should create one instance for each operation that need retry.
// Exponential 对应 Go 结构体，内部保存基础退避、倍率、上限和下一次退避状态。
/// 无抖动的指数退避：每次将 `nextBackoff` 乘以 `multiplier`，并截断到 `maxBackoff`。
/// 每次需要重试的操作应使用独立实例，避免状态串扰。
pub struct Exponential {
    /// 基础退避时长（第 0 次及复位后的起点）。
    pub baseBackoff: Duration,
    /// 每次增长的倍率（对纳秒整数值做浮点乘法后再截断）。
    pub multiplier: f64,
    /// 退避上限；增长结果与此取较小值。
    pub maxBackoff: Duration,

    /// 下一次将返回的退避时长（状态字段）。
    pub nextBackoff: Duration,
}

// Go 中的 var _ Backoffer = &Exponential{} 是接口实现断言；Rust 通过 impl Backoffer 表达。

// NewExponential creates a new Exponential backoff.
// NewExponential 对应 Go 构造函数，nextBackoff 初始等于 baseBackoff。
/// 创建指数退避实例，`nextBackoff` 初始化为 `baseBackoff`。
pub fn NewExponential(baseBackoff: Duration, multiplier: f64, maxBackoff: Duration) -> Exponential {
    Exponential {
        baseBackoff,
        multiplier,
        maxBackoff,
        nextBackoff: baseBackoff,
    }
}

impl Backoffer for Exponential {
    // Backoff returns the duration to wait for the retryCnt-th retry.
    // retryCnt starts from 0.
    /// 第 0 次复位到 `baseBackoff`；之后按倍率增长并受 `maxBackoff` 限制。
    fn Backoff(&mut self, retryCnt: usize) -> Duration {
        if retryCnt == 0 {
            // Go 在第 0 次重试时重置状态，避免复用实例后继承旧的 nextBackoff。
            self.nextBackoff = self.baseBackoff;
            return self.nextBackoff;
        }

        // Go 先把以纳秒为单位的 time.Duration 转为 float64，乘法后再截断为整数纳秒。
        // Duration::from_secs_f64 会舍入到最近的纳秒，因此这里显式保留 Go 的截断语义。
        let scaled_nanos = (self.nextBackoff.as_nanos() as f64 * self.multiplier) as u64;
        let scaled = Duration::from_nanos(scaled_nanos);
        self.nextBackoff = scaled.min(self.maxBackoff);
        self.nextBackoff
    }
}

impl Exponential {
    // Backoff 方法保留 Go 接收者方法的调用入口，并转发到 Backoffer trait 实现。
    /// 保留 Go 风格的方法入口，转发到 `Backoffer::Backoff`。
    pub fn Backoff(&mut self, retryCnt: usize) -> Duration {
        <Self as Backoffer>::Backoff(self, retryCnt)
    }
}
