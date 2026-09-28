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

// mock 上下文键：在 `context` 中定位 MockRestrictedSQLExecutor 记录器。
//
// 字符串身份必须与 Go `__MockRestrictedSQLExecutor` 完全一致。

use std::fmt;

// RestrictedSQLExecutorKey is the key to represent MockRestrictedSQLExecutorMockRecorder in ctx.
/// 在 ctx 中标识 `MockRestrictedSQLExecutorMockRecorder` 的键类型。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct RestrictedSQLExecutorKey;

// String implements the string.Stringer interface.
impl RestrictedSQLExecutorKey {
    /// 返回与 Go Stringer 相同的固定键名。
    pub fn String(&self) -> &'static str {
        "__MockRestrictedSQLExecutor"
    }
}

impl fmt::Display for RestrictedSQLExecutorKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.String())
    }
}
