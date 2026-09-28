// Copyright 2026 AsterSQL.
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

// `intest` crate 入口：按 feature/测试 cfg 选择断言与 `InTest` 实现并再导出。
//
// 结构镜像 Go 互斥 build-tag 文件：`assert`/`no_assert`、`in_unittest`/`not_in_unittest`
// 分别对应启用/禁用断言与是否处于 intest 变体。

#![allow(non_snake_case, non_upper_case_globals)]

/// 断言公共类型与实现（始终编译）。
pub(crate) mod assert_common;

// These cfg branches explicitly mirror Go's mutually exclusive build-tag files.
/// 启用断言实现（测试 / intest / enableassert）。
#[cfg(any(test, feature = "intest", feature = "enableassert"))]
pub(crate) mod assert;
/// 禁用断言实现（生产默认路径）。
#[cfg(not(any(test, feature = "intest", feature = "enableassert")))]
pub(crate) mod no_assert;

/// `InTest = true` 变体。
#[cfg(any(test, feature = "intest"))]
mod in_unittest;
/// `InTest = false` 变体。
#[cfg(not(any(test, feature = "intest")))]
mod not_in_unittest;

#[cfg(any(test, feature = "intest", feature = "enableassert"))]
pub use assert::{Assert, AssertFunc, AssertNoError, AssertNotNil, EnableAssert};
pub use assert_common::{AssertArg, EnableInternalCheck};
#[cfg(any(test, feature = "intest"))]
pub use in_unittest::InTest;
#[cfg(not(any(test, feature = "intest", feature = "enableassert")))]
pub use no_assert::{Assert, AssertFunc, AssertNoError, AssertNotNil, EnableAssert};
#[cfg(not(any(test, feature = "intest")))]
pub use not_in_unittest::InTest;

#[cfg(test)]
#[path = "assert_test.rs"]
mod assert_test;

#[cfg(test)]
#[path = "assert_common_test.rs"]
mod assert_common_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
