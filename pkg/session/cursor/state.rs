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

// 会话游标状态定义。
//
// `State` 描述服务端游标（Cursor）打开时绑定的快照信息；
// `StartTS` 为开始时间戳，用于在 MVCC（多版本并发控制）下以固定版本读数据。

// State 对应 Go 的游标状态结构；StartTS 记录游标开始读取时使用的时间戳。
/// 游标打开时绑定的状态快照。
#[allow(non_snake_case)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct State {
    /// 游标开始读取时使用的开始时间戳（StartTS）。
    pub StartTS: u64,
}
