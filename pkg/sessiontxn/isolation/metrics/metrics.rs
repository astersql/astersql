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

// Read Committed（RC，读已提交）检查时间戳写冲突指标。
//
// 从全局 `CounterVec` 按固定标签切出读/写两条 Counter，供 RC 路径在检测到
// WriteConflict（写冲突）时递增。对应 Go `sessiontxn/isolation/metrics`。

use crate::metrics;

/// RC 读检查时间戳路径上的写冲突计数器句柄。
pub static mut RC_READ_CHECK_TS_WRITE_CONFLICT_COUNTER: Option<prometheus::Counter> = None;
/// RC 写检查时间戳路径上的写冲突计数器句柄。
pub static mut RC_WRITE_CHECK_TS_WRITE_CONFLICT_COUNTER: Option<prometheus::Counter> = None;

/// 包级初始化入口，转发到 `init_metrics_vars`。
pub fn init() {
    init_metrics_vars();
}

// InitMetricsVars 对应 Go 初始化两个带固定标签的冲突计数器。
/// 从上游 `RCCheckTSWriteConfilictCounter` 绑定 `read_check` / `write_check` 两个 Counter。
pub fn init_metrics_vars() {
    unsafe {
        // 上游 CounterVec 必须已由 metrics::InitMetrics 注入，否则这里 panic。
        let counter = metrics::RCCheckTSWriteConfilictCounter
            .as_ref()
            .expect("metrics::InitMetrics must run before isolation metrics initialization");
        RC_READ_CHECK_TS_WRITE_CONFLICT_COUNTER =
            Some(counter.with_label_values(&[metrics::LblRCReadCheckTS]));
        RC_WRITE_CHECK_TS_WRITE_CONFLICT_COUNTER =
            Some(counter.with_label_values(&[metrics::LblRCWriteCheckTS]));
    }
}
