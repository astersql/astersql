// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 向量化算术内置函数（`builtin_arithmetic_vec`）的测试入口模块。
//
// 对应 Go 的 `builtin_arithmetic_vec_test.go`。本文件仅负责再导出向量化内核符号，
// 并把 Go 对等用例放到 `builtin_arithmetic_vec_1_aster_unit_test.rs` 子模块中执行。

pub use crate::builtin_arithmetic_vec_kernel::*;

#[path = "builtin_arithmetic_vec_1_aster_unit_test.rs"]
/// 挂载 Go 对等向量化算术单元测试（`builtin_arithmetic_vec_1_aster_unit_test.rs`）。
mod go_parity;
