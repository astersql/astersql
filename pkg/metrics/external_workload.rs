// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 外部工作负载（external workload）管理器相关 Prometheus 指标。
//
// 外部工作负载管理器调度独立于常规 SQL 会话的后台 worker；本模块按 worker 类型
// 与生命周期 action（初始化、注册、回收、中止）统计事件次数。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// Action labels used by the external workload manager.
// 四个常量逐项对应 worker 生命周期事件，字符串会作为 Prometheus action 标签值。
/// Worker 生命周期：初始化。
pub const WorkerActionInit: &str = "init";
/// Worker 生命周期：注册。
pub const WorkerActionRegister: &str = "register";
/// Worker 生命周期：回收。
pub const WorkerActionRecycle: &str = "recycle";
/// Worker 生命周期：中止。
pub const WorkerActionAbort: &str = "abort";

/// 外部工作负载管理器事件计数（按 worker type 与 action）。
// ExternalWorkloadTaskCounter counts register / recycle / init / abort events
// emitted by the external workload manager, broken down by worker type.
pub static mut ExternalWorkloadTaskCounter: Option<prometheus::CounterVec> = None;

/// 初始化外部工作负载指标 collector。
// InitExternalWorkloadMetrics 对应 Go 初始化函数：创建按 worker type 与 action 拆分的事件 counter。
pub fn InitExternalWorkloadMetrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    let task_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "external_workload",
            Name: "task_total",
            Help: "Total external workload manager events by worker type and action.",
        },
        // 标签顺序属于指标接口的一部分，必须保持 type 在 action 之前。
        vec![LblType, LblAction],
    );

    // 仅保留 Go 包级变量的初始化形状；不处理多线程读取和重复初始化。
    unsafe {
        ExternalWorkloadTaskCounter = Some(task_counter);
    }
}
