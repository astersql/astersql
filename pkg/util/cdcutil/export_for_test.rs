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

// 测试专用导出：为 `CDCNameSet` 提供与 Go `TESTGetChangefeedNames` 对齐的访问器。
//
// 仅在 `#[cfg(test)]` 下由 `lib.rs` 引入，避免污染生产 API。

use super::cdc::CDCNameSet;

#[allow(non_snake_case)]
impl CDCNameSet {
    /// Test-only equivalent of Go's `TESTGetChangefeedNames` export.
    /// 测试专用：返回展平后的 changefeed 路径名列表（与 Go 导出一致）。
    pub fn TESTGetChangefeedNames(&self) -> Vec<String> {
        self.changefeed_names()
    }
}
