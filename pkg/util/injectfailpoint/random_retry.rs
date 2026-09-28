// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 随机失败注入（failpoint）：按概率返回错误，供 DXF 等路径做混沌/容错测试。
//
// Failpoint 是代码中预埋的可开关故障点；开启后可按概率注入错误或截断读结果，
// 验证上层重试与错误处理。本文件对应 Go `injectfailpoint` 的随机重试相关逻辑。

#![allow(non_snake_case)]

use std::error::Error as StdError;
use std::io;

use rand::Rng;

/// 本模块统一错误类型：可跨线程传递的动态错误盒。
pub type Error = Box<dyn StdError + Send + Sync + 'static>;

// getFunctionName mirrors runtime.Caller plus runtime.FuncForPC. Frames from
// this helper and the backtrace implementation itself are skipped.
/// 通过栈回溯取得调用方函数名（跳过本函数与 `backtrace::` 帧），用于错误消息定位。
#[inline(never)]
pub fn getFunctionName() -> String {
    let mut saw_this_function = false;
    let mut caller = None;
    // 遍历栈帧：先定位本函数，再取其后第一个非 backtrace 帧作为调用方。
    backtrace::trace(|frame| {
        backtrace::resolve_frame(frame, |symbol| {
            let Some(name) = symbol.name().map(|name| name.to_string()) else {
                return;
            };
            if name.contains("getFunctionName") {
                saw_this_function = true;
            } else if saw_this_function && !name.starts_with("backtrace::") {
                caller = Some(name);
            }
        });
        caller.is_none()
    });
    caller.unwrap_or_else(|| "unknown".to_owned())
}

/// 构造带调用方信息的注入错误。
fn injected_error() -> Error {
    Box::new(io::Error::new(
        io::ErrorKind::Other,
        format!("injected random error, caller: {}", getFunctionName()),
    ))
}

// DXFRandomErrorWithOnePercent returns an error with probability 0.01. It controls the DXF's failpoint.
/// 在 DXF 的 `DXFRandomError` failpoint 开启时，以约 1% 概率返回注入错误。
pub fn DXFRandomErrorWithOnePercent() -> Result<(), Error> {
    fail::fail_point!("DXFRandomError", |_| {
        RandomError(0.01, injected_error()).map_or(Ok(()), Err)
    });
    Ok(())
}

// DXFRandomErrorWithOnePercentWrapper returns an error with probability 0.01. It controls the DXF's failpoint.
/// 若已有错误则原样返回；否则在 failpoint 开启时以约 1% 概率注入新错误。
pub fn DXFRandomErrorWithOnePercentWrapper(err: Option<Error>) -> Option<Error> {
    // 已有错误时不覆盖，仅在成功路径上尝试注入。
    if err.is_some() {
        return err;
    }
    fail::fail_point!("DXFRandomError", |_| {
        RandomError(0.01, injected_error())
    });
    None
}

// DXFRandomErrorWithOnePerThousand returns an error with probability 0.001. It controls the DXF's failpoint.
/// 在 DXF failpoint 开启时，以约 0.1% 概率返回注入错误。
pub fn DXFRandomErrorWithOnePerThousand() -> Result<(), Error> {
    fail::fail_point!("DXFRandomError", |_| {
        RandomError(0.001, injected_error()).map_or(Ok(()), Err)
    });
    Ok(())
}

// RandomErrorForReadWithOnePerPercent returns a read error with probability 0.01. It controls the DXF's failpoint.
/// 读路径 failpoint：开启时可能把读长度 `n` 截短并附带 UnexpectedEof。
pub fn RandomErrorForReadWithOnePerPercent(n: i32, err: Option<Error>) -> (i32, Option<Error>) {
    // Evaluate the failpoint before moving the boxed error. Returning from the
    // triggered branch lets the disabled path retain ownership of `err`.
    // 先 eval failpoint 再决定是否移动 `err`，避免禁用路径丢失所有权。
    if fail::eval("DXFRandomError", |_| ()).is_some() {
        return random_read_error_with_rng(n, err, &mut rand::thread_rng());
    }
    (n, err)
}

// RandomError returns an error with the given probability.
/// 以给定概率返回 `err`，否则返回 `None`。
pub fn RandomError(probability: f64, err: Error) -> Option<Error> {
    random_error_with_rng(probability, err, &mut rand::thread_rng())
}

/// 可注入 RNG 的随机错误判定，便于单测。
pub(crate) fn random_error_with_rng<R: Rng + ?Sized>(
    probability: f64,
    err: Error,
    rng: &mut R,
) -> Option<Error> {
    if rng.r#gen::<f64>() < probability {
        return Some(err);
    }
    None
}

/// 读路径随机注入：约 1% 触发 UnexpectedEof，其中约 20% 将读长度置 0，其余截到 `[0,n)`。
pub(crate) fn random_read_error_with_rng<R: Rng + ?Sized>(
    n: i32,
    err: Option<Error>,
    rng: &mut R,
) -> (i32, Option<Error>) {
    // 已有错误、零长度或不命中 1% 概率时直接返回原值。
    if n == 0 || err.is_some() || rng.r#gen::<f64>() > 0.01 {
        return (n, err);
    }
    let unexpected_eof = || -> Error {
        Box::new(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "unexpected end of file",
        ))
    };
    if rng.r#gen::<f64>() < 0.2 {
        return (0, Some(unexpected_eof()));
    }
    (rng.gen_range(0..n), Some(unexpected_eof()))
}
