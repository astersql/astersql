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

// 内部断言 API（启用版）：`intest` / `enableassert` / 测试构建下默认打开。
//
// 对应 Go `intest` 在带断言 build tag 时的实现。任一开关（`EnableAssert` 或
// `EnableInternalCheck`）为真时才会真正执行检查并可能 panic。

use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};

use super::assert_common::{
    AssertArg, EnableInternalCheck, doAssert, doAssertFunc, doAssertNoError, doAssertNotNil,
};

// EnableAssert is true for the `intest` or `enableassert` build variants.
/// 断言总开关：本文件对应启用变体，默认 `true`。
pub static EnableAssert: AtomicBool = AtomicBool::new(true);

// Assert asserts that cond is true when either assertion switch is enabled.
/// 断言 `cond` 为真；任一开关开启时失败则 panic。
pub fn Assert(cond: bool, msg_and_args: &[AssertArg]) {
    if EnableAssert.load(Ordering::Relaxed) || EnableInternalCheck.load(Ordering::Relaxed) {
        doAssert(cond, msg_and_args);
    }
}

// AssertNoError asserts that err is None.
/// 断言无错误（`err` 为 `None`）。
pub fn AssertNoError(err: Option<&dyn Display>, msg_and_args: &[AssertArg]) {
    if EnableAssert.load(Ordering::Relaxed) || EnableInternalCheck.load(Ordering::Relaxed) {
        doAssertNoError(err, msg_and_args);
    }
}

// AssertNotNil asserts that obj is Some, including typed pointer-like values.
/// 断言对象非空（`Option` 为 `Some`），对应 Go 的非 nil 检查。
pub fn AssertNotNil<T>(obj: Option<T>, msg_and_args: &[AssertArg]) {
    if EnableAssert.load(Ordering::Relaxed) || EnableInternalCheck.load(Ordering::Relaxed) {
        doAssertNotNil(obj, msg_and_args);
    }
}

// AssertFunc asserts that a function exists and returns true.
/// 断言函数存在且调用返回 `true`。
pub fn AssertFunc(fn_check: Option<fn() -> bool>, msg_and_args: &[AssertArg]) {
    if EnableAssert.load(Ordering::Relaxed) || EnableInternalCheck.load(Ordering::Relaxed) {
        doAssertFunc(fn_check, msg_and_args);
    }
}
