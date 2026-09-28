// Copyright 2022 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 资源管理器（Resource Manager）相关 Prometheus 指标。
//
// 覆盖 CPU 使用率的指数移动平均（EMA）与资源池并发度，供调度与限流侧观测。
// 本文件只构造指标句柄，不注册采集器、不连接数据库。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 这里只描述 Prometheus 指标元数据，不会注册采集器、连接数据库或执行任何 TiDB 业务动作。

// EMACPUUsageGauge 对应 Go 的指数移动平均 CPU 使用率仪表盘。
/// CPU 使用率的指数移动平均（EMA）仪表；EMA 对瞬时尖峰更平滑。
pub static mut EMACPUUsageGauge: Option<prometheus::Gauge> = None;

// PoolConcurrencyCounter 按 LblType 标签记录资源池当前并发度。
/// 资源池当前并发度，按类型标签区分不同池。
pub static mut PoolConcurrencyCounter: Option<prometheus::GaugeVec> = None;

// InitResourceManagerMetrics 对应 Go 的同名初始化函数，构造两个指标但不在此处注册。
/// 初始化资源管理器指标句柄（不执行 Prometheus 注册）。
pub fn InitResourceManagerMetrics() {
    // Go 的包级变量可直接重赋值；用 Option 表示初始化前尚无指标句柄。
    unsafe {
        EMACPUUsageGauge = Some(metricscommon::NewGauge(prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "rm",
            Name: "ema_cpu_usage",
            Help: "exponential moving average of CPU usage",
            ..Default::default()
        }));

        PoolConcurrencyCounter = Some(metricscommon::NewGaugeVec(
            prometheus::GaugeOpts {
                Namespace: "tidb",
                Subsystem: "rm",
                Name: "pool_concurrency",
                Help: "How many concurrency in the pool",
                ..Default::default()
            },
            &[LblType],
        ));
    }
}
