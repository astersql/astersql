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

// 按 `-short` / `-long` 测试标志条件跳过用例的辅助函数。
//
// 由 `pkg/util/skip/skip.go` 迁移。抽象出 Go `testing.T` 的 helper/skip 子集，
// 以便在 Rust 侧复现相同的跳过文案与命令行解析语义。

use std::any::Any;
use std::ffi::OsStr;

use crate::testkit::testflag;

/// The parts of Go's `testing.T` used by this package.
/// 本包用到的 Go `testing.T` 子集：`helper` 标记调用栈，`skip` 跳过当前用例。
pub trait TestContext {
    /// 标记当前函数为测试辅助函数（对应 `t.Helper()`）。
    fn helper(&mut self);
    /// 跳过当前测试并附带任意参数；与 `t.Skip` 一样不得返回调用者。
    fn skip(&mut self, args: &[&dyn Any]) -> !;
}

/// 解析 Go 风格布尔字面量（1/t/true 与 0/f/false 等）；非法返回 None。
fn parse_go_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Some(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Some(false),
        _ => None,
    }
}

/// Reads the short-test flag from an argument sequence whose first item is the
/// executable name. Both Go's internal `test.short` spelling and the user-facing
/// `short` spelling are accepted.
/// 从参数序列（首项为可执行名）读取 short 测试标志；兼容 `test.short` 与 `short` 写法。
pub fn short_from_args<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut short = false;

    // 跳过 argv[0]，按出现顺序解析；同名标志以后者为准。
    for argument in args.into_iter().skip(1) {
        let argument = argument.as_ref().to_string_lossy();
        match argument.as_ref() {
            "-test.short" | "--test.short" | "-short" | "--short" => short = true,
            _ => {
                // 形如 -test.short=false / --short=1 的键值形式。
                let value = argument
                    .strip_prefix("-test.short=")
                    .or_else(|| argument.strip_prefix("--test.short="))
                    .or_else(|| argument.strip_prefix("-short="))
                    .or_else(|| argument.strip_prefix("--short="));
                if let Some(value) = value.and_then(parse_go_bool) {
                    short = value;
                }
            }
        }
    }

    short
}

/// 在 short=true 时跳过测试；先调 helper，再以 Go 固定前缀拼接额外参数调用 skip。
pub fn under_short_with<T>(t: &mut T, short: bool, args: &[&dyn Any])
where
    T: TestContext + ?Sized,
{
    t.helper();
    if short {
        let reason = "disabled under -short";
        let mut skip_args: Vec<&dyn Any> = Vec::with_capacity(args.len() + 1);
        skip_args.push(&reason);
        skip_args.extend(args.iter().copied());
        t.skip(&skip_args);
    }
}

// UnderShort skips the test if the -short flag is set.
/// 若进程参数含 `-short`，则跳过当前测试（读取真实 `env::args_os`）。
#[allow(non_snake_case)]
pub fn UnderShort<T>(t: &mut T, args: &[&dyn Any])
where
    T: TestContext + ?Sized,
{
    under_short_with(t, short_from_args(std::env::args_os()), args);
}

/// 在 long=false（非长时间测试）时跳过；文案前缀与 Go `disabled not under -short` 一致。
pub fn not_under_long_with<T>(t: &mut T, long: bool, args: &[&dyn Any])
where
    T: TestContext + ?Sized,
{
    t.helper();
    if !long {
        let reason = "disabled not under -short";
        let mut skip_args: Vec<&dyn Any> = Vec::with_capacity(args.len() + 1);
        skip_args.push(&reason);
        skip_args.extend(args.iter().copied());
        t.skip(&skip_args);
    }
}

// NotUnderLong skips the test if the -long flag is not set
/// 若未设置 `-long`（经 `testflag::Long()`），则跳过当前测试。
#[allow(non_snake_case)]
pub fn NotUnderLong<T>(t: &mut T, args: &[&dyn Any])
where
    T: TestContext + ?Sized,
{
    not_under_long_with(t, testflag::Long(), args);
}
