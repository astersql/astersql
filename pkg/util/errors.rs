// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 错误链根因追溯工具。
//
// 对应 Go `pkg/util/errors`：沿 `Error::source` 走到最深一层，便于日志与重试判定忽略包装层。

use std::error::Error;

// OriginError returns the deepest source, while preserving Go's nil behavior.
/// 沿 `Error::source` 链追溯到最深一层根因；`None` 对应 Go 的 nil 行为。
pub fn OriginError<'a>(
    err: Option<&'a (dyn Error + 'static)>,
) -> Option<&'a (dyn Error + 'static)> {
    // 先解包 Option；再反复下钻 source，直到没有更深层。
    let mut current = err?;
    while let Some(source) = current.source() {
        current = source;
    }
    Some(current)
}
