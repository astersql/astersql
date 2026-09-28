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

// MockTiKV 后端入口：以 `StoreType::MockTiKv` 身份复用嵌入式协议服务。
//
// 工作区内仅有一套进程内 TiKV 协议 server；MockTiKV 走易失模式，但保留
// 独立后端标识，并共用 inspector / hijacker / keyspace 选项流水线。

use crate::mockstore::{MockOptions, MockStorage, Result, StoreType};

/// The Rust workspace has one canonical in-process TiKV protocol server. The
/// MockTiKV backend uses it in volatile mode while retaining its distinct
/// backend identity and the same inspector/hijacker/keyspace option pipeline.
///
/// 创建 MockTiKV 后端的 MockStorage（委托 `build_embedded`）。
pub fn new_mock_tikv_store(options: &MockOptions) -> Result<MockStorage> {
    crate::unistore::build_embedded(options, StoreType::MockTiKv)
}

/// Go 风格命名别名，等价于 `new_mock_tikv_store`。
pub fn newMockTikvStore(options: &MockOptions) -> Result<MockStorage> {
    new_mock_tikv_store(options)
}
