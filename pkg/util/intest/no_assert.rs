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

// 内部断言 API（禁用版）：普通构建下 `EnableAssert` 默认为 `false`。
//
// 仅当 `EnableInternalCheck` 显式开启时才执行检查，对应 Go 非断言 build tag。
// 对外符号与启用版一致，便于按 cfg 切换实现而不改调用方。

use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};

use super::assert_common::{
    AssertArg, EnableInternalCheck, doAssert, doAssertFunc, doAssertNoError, doAssertNotNil,
};

// EnableAssert is false in the normal build variant.
/// 断言总开关：本文件对应普通变体，默认 `false`。
pub static EnableAssert: AtomicBool = AtomicBool::new(false);

// In normal builds assertions run only when EnableInternalCheck is enabled.
/// 普通构建下仅在内部检查开启时断言条件。
pub fn Assert(cond: bool, msg_and_args: &[AssertArg]) {
    if EnableInternalCheck.load(Ordering::Relaxed) {
        doAssert(cond, msg_and_args);
    }
}

// AssertNoError asserts that err is None when internal checking is enabled.
/// 内部检查开启时断言无错误。
pub fn AssertNoError(err: Option<&dyn Display>, msg_and_args: &[AssertArg]) {
    if EnableInternalCheck.load(Ordering::Relaxed) {
        doAssertNoError(err, msg_and_args);
    }
}

// AssertNotNil asserts that obj is Some when internal checking is enabled.
/// 内部检查开启时断言对象非空。
pub fn AssertNotNil<T>(obj: Option<T>, msg_and_args: &[AssertArg]) {
    if EnableInternalCheck.load(Ordering::Relaxed) {
        doAssertNotNil(obj, msg_and_args);
    }
}

// AssertFunc checks the function only when internal checking is enabled.
/// 内部检查开启时断言函数存在且返回真。
pub fn AssertFunc(fn_check: Option<fn() -> bool>, msg_and_args: &[AssertArg]) {
    if EnableInternalCheck.load(Ordering::Relaxed) {
        doAssertFunc(fn_check, msg_and_args);
    }
}
