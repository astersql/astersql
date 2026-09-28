// Copyright 2018 PingCAP, Inc.
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

// Owner（所有者）管理相关 Prometheus 指标。
//
// 覆盖创建 etcd session 耗时、watch owner 事件结果，以及竞选 owner 的结果计数。
// Owner 机制用于在多 TiDB 实例间选举唯一执行 DDL / 统计信息等后台任务的节点。

use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// Metrics
// Option 对应 Go 中初始化前为 nil 的指标指针；字符串常量保持原事件标签值。
/// 新建 owner session 的耗时直方图。
pub static mut NEW_SESSION_HISTOGRAM: Option<prometheus::HistogramVec> = None;

/// watch 因 watcher 关闭而结束。
pub const WATCHER_CLOSED: &str = "watcher_closed";
/// watch 被主动取消。
pub const CANCELLED: &str = "cancelled";
/// watch 观察到 key 删除。
pub const DELETED: &str = "deleted";
/// watch 观察到 key 写入。
pub const PUT_VALUE: &str = "put_value";
/// etcd session 结束导致 watch 退出。
pub const SESSION_DONE: &str = "session_done";
/// 上下文取消导致 watch 退出。
pub const CTX_DONE: &str = "context_done";
/// watch owner 事件计数（按类型与结果标签）。
pub static mut WATCH_OWNER_COUNTER: Option<prometheus::CounterVec> = None;

/// 竞选过程中发现本节点已不再是 owner。
pub const NO_LONGER_OWNER: &str = "no_longer_owner";
/// 竞选 owner 结果计数。
pub static mut CAMPAIGN_OWNER_COUNTER: Option<prometheus::CounterVec> = None;

// init_owner_metrics 对应 Go 的 InitOwnerMetrics：依次初始化新会话耗时、watch 结果与竞选结果指标。
// static mut 仅用于呈现 Go 包级赋值；真实实现应使用 OnceLock 避免并发初始化的数据竞争。
/// 初始化本文件全部 owner 相关指标。
pub unsafe fn init_owner_metrics() {
    NEW_SESSION_HISTOGRAM = Some(metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "owner",
            Name: "new_session_duration_seconds",
            Help: "Bucketed histogram of processing time (s) of new session.",
            // 保留 Go 的指数桶：从 0.5ms 起翻倍 22 次，覆盖到约 1048 秒。
            Buckets: prometheus::ExponentialBuckets(0.0005, 2.0, 22),
        },
        &[LblType, LblResult],
    ));

    WATCH_OWNER_COUNTER = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "owner",
            Name: "watch_owner_total",
            Help: "Counter of watch owner.",
        },
        &[LblType, LblResult],
    ));

    CAMPAIGN_OWNER_COUNTER = Some(metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "owner",
            Name: "campaign_owner_total",
            Help: "Counter of campaign owner.",
        },
        &[LblType, LblResult],
    ));
}
