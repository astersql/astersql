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

// 测试用 `-long` / `--long` 布尔标志解析。
//
// 对齐 Go `flag` 包对布尔字面量的拼写；公开 [`long_from_args`] 以便单元测试
// 注入参数序列，而不必改写进程全局 `std::env::args`。

use std::ffi::OsStr;

/// 解析 Go `flag` 包接受的布尔字面量；无法识别时返回 `None`。
fn parse_go_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Some(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Some(false),
        _ => None,
    }
}

/// Reads the `long` flag from an argument sequence whose first item is the
/// executable name. This mirrors the boolean spellings accepted by Go's
/// `flag` package and is public so native Cargo tests can exercise both flag
/// states without mutating process-global arguments.
///
/// 从参数序列（首项为可执行文件名）读取 `long` 标志；后出现的合法写法覆盖先前值。
/// 与 Go `flag` 一致，位置参数、`--`、未知标志或非法值会停止标志解析。
pub fn long_from_args<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut long = false;

    // 跳过 argv[0]，按出现顺序解析 -long / --long[=bool]。
    for argument in args.into_iter().skip(1) {
        let argument = argument.as_ref().to_string_lossy();
        let argument = argument.as_ref();

        if argument == "-long" || argument == "--long" {
            long = true;
            continue;
        }

        if let Some(value) = argument
            .strip_prefix("-long=")
            .or_else(|| argument.strip_prefix("--long="))
        {
            match parse_go_bool(value) {
                Some(value) => long = value,
                None => {
                    // strconv.ParseBool returns false together with its error;
                    // flag parsing then stops at that invalid value.
                    long = false;
                    break;
                }
            }
            continue;
        }

        // Go's FlagSet stops at the first positional argument or `--`, and
        // returns immediately on unknown or malformed flags.
        break;
    }

    long
}

// Long returns whether the -long flag is set.
// Long 对应 Go 的同名函数：读取进程参数中注册的 long 布尔标志。
/// 读取当前进程参数中的 `-long` 标志。
#[allow(non_snake_case)]
pub fn Long() -> bool {
    long_from_args(std::env::args_os())
}
