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

// 仅测试可见的 mock 会话上下文工厂。
//
// 对应 Go `fortest.go`：在测试构建下导出 `NewContext`。

use crate::{Context, newContext};

/// NewContext creates a test-only mocked session context.
/// 创建仅用于测试的 mock 会话上下文（Session Context）。
pub fn NewContext() -> Box<Context> {
    newContext()
}
