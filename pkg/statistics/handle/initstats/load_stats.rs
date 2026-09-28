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

// 初始化加载统计时的并发度计算。
//
// 对应 Go `GetConcurrency`：按 CPU 核数与 `ForceInitStats` 开关，
// 将并发度钳制在 [2, 16]，避免挤占 GC / 统计缓存等系统任务或业务负载。

use crate::config::get_global_config;

/// 按是否强制初始化与可用处理器数计算加载并发度（结果钳制到 [2, 16]）。
pub fn get_concurrency_for(force_init_stats: bool, processors: usize) -> usize {
    let processors = isize::try_from(processors).unwrap_or(isize::MAX);
    // 强制初始化：尽量用满核但预留 2；否则只用一半核，降低对业务影响。
    let concurrency = if force_init_stats {
        processors - 2
    } else {
        processors / 2
    };
    concurrency.clamp(2, 16) as usize
}

// GetConcurrency gets the concurrency of loading stats.
// the concurrency is from 2 to 16.
// when Performance.ForceInitStats is true, the concurrency is from 2 to GOMAXPROCS(0)-2.
// -2 is to ensure that the system has enough resources to handle other tasks. such as GC and stats cache internal.
// when Performance.ForceInitStats is false, the concurrency is from 2 to GOMAXPROCS(0)/2.
// it is to ensure that concurrency doesn't affect the performance of customer's business.
/// 读取全局配置与可用并行度，返回当前应使用的加载并发度。
pub fn GetConcurrency() -> usize {
    let processors = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    get_concurrency_for(get_global_config().performance.force_init_stats, processors)
}
