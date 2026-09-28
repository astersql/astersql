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

// 跨 Keyspace 场景下的最小 start_ts 上报器。
// start_ts 是事务开始时间戳（TSO/时间戳序的一部分），用于跨实例可见性协调；
// 当前 Go 侧尚无实体实现，此处保留空操作以维持接口形态。

/// Go 实现故意尚无函数体；显式保留 no-op，以维持 server-info 上报接口，且不虚构 TSO。
/// The Go implementation intentionally has no body yet. Keeping this explicit
/// no-op preserves the server-info reporter interface without inventing a TSO.
#[derive(Default)]
pub struct MinStartTsReporter;
impl MinStartTsReporter {
    /// 向 store/session 上报当前最小 start_ts；占位实现为空操作。
    pub fn report_min_start_ts<S, T>(&self, _store: &S, _session: &T) {}
}
