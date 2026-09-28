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

// 算术内置函数（`builtin_arithmetic`）的测试入口模块。
//
// 本文件本身不含用例实现，而是把可执行的 Go 对等测试放到独立子模块
// `builtin_arithmetic_2_aster_unit_test.rs` 中，避免生产内核代码与测试耦合。
// 对外再导出 `builtin_arithmetic_kernel` 的公开符号，方便测试侧直接调用。

// The executable Go-parity cases live in a separate Rust test module so the
// production arithmetic implementation remains free of test-only code.
pub use crate::builtin_arithmetic_kernel::*;

#[path = "builtin_arithmetic_2_aster_unit_test.rs"]
/// 挂载 Go 对等算术单元测试（`builtin_arithmetic_2_aster_unit_test.rs`）。
mod go_parity;
