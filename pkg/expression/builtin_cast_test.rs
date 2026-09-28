// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// CAST 表达式标量单元测试入口。
//
// 将 `builtin_cast_kernel` 的公开符号再导出，并经 `#[path]` 挂载
// `builtin_cast_4_aster_unit_test.rs` 中与 Go 对齐的用例集合。

/// 再导出 CAST 内核符号，供本测试包与 `go_parity` 子模块共用。
pub use crate::builtin_cast_kernel::*;

/// 与 Go `builtin_cast_test.go` 对齐的 CAST 语义用例。
#[path = "builtin_cast_4_aster_unit_test.rs"]
mod go_parity;
