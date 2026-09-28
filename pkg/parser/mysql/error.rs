// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 构造面向 MySQL 客户端的 `SQLError`，并按模板格式化/脱敏参数。
//
// 流程：查 SQLSTATE → 取默认或自定义消息模板 → 在写入结果前按脱敏位置处理参数。
// SQLSTATE 是五字符状态码，用于向客户端表达错误类别（如 23000 完整性约束）。

// 本文件创建内存中的错误值。
use super::errname::MySQLErrName;
use super::state::{DefaultMySQLState, MySQLState};
use crate::errors::{ErrorArg, Errorf, RedactErrorArg};

/// 对应 Go `any` 格式参数，保留字符串、整数、布尔值等运行时类型。
pub type Arg = ErrorArg;

/// ErrBadConn 对应 Go 的可移植连接错误；函数形式避免在这里虚构可常量初始化的外部错误类型。
pub fn ErrBadConn() -> std::io::Error {
    std::io::Error::other("connection was bad")
}

/// ErrMalformPacket 对应 Go 的畸形协议包错误；这里只构造错误，不读取网络输入。
pub fn ErrMalformPacket() -> std::io::Error {
    std::io::Error::other("malform packet error")
}

/// SQLError 对应 Go 中执行 SQL 时暴露给 MySQL 客户端的错误信息。
#[derive(Debug)]
pub struct SQLError {
    /// MySQL 数字错误码（与 errcode 常量对应）。
    pub Code: u16,
    /// 已格式化（并可能已脱敏）的人类可读消息。
    pub Message: String,
    /// SQLSTATE 五字符状态码；无专用映射时为 HY000。
    pub State: String,
}

impl SQLError {
    /// Error 对应 Go error 接口方法，保持 `ERROR code (state): message` 输出形状。
    pub fn Error(&self) -> String {
        format!("ERROR {} ({}): {}", self.Code, self.State, self.Message)
    }
}

impl std::fmt::Display for SQLError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.Error())
    }
}

impl std::error::Error for SQLError {}

/// NewErr 对应 Go 的默认模板错误构造函数。
/// 先选择 SQLSTATE，再按错误码查默认消息；脱敏必须发生在格式化之前，避免敏感参数进入结果。
pub fn NewErr(err_code: u16, args: Vec<Arg>) -> SQLError {
    let state = MySQLState()
        .get(&err_code)
        .copied()
        .map(str::to_owned)
        .unwrap_or_else(|| DefaultMySQLState.to_owned());

    let message = if let Some(sql_err) = MySQLErrName().get(&err_code) {
        format_string_args(&sql_err.Raw, &sql_err.RedactArgPos, args)
    } else {
        sprint_args(&args)
    };

    SQLError {
        Code: err_code,
        Message: message,
        State: state,
    }
}

/// NewErrf 对应 Go 的自定义模板错误构造函数。
/// SQLSTATE 回退规则与 NewErr 相同，调用方显式给出的脱敏位置在格式化前应用。
pub fn NewErrf(
    err_code: u16,
    format_text: &str,
    redact_arg_pos: &[usize],
    args: Vec<Arg>,
) -> SQLError {
    let state = MySQLState()
        .get(&err_code)
        .copied()
        .map(str::to_owned)
        .unwrap_or_else(|| DefaultMySQLState.to_owned());

    let message = format_string_args(format_text, redact_arg_pos, args);

    SQLError {
        Code: err_code,
        Message: message,
        State: state,
    }
}

/// 先按 Go `RedactErrorArg` 处理敏感位置，再用共享格式化器解释 `%s`、`%d`、`%v`
/// 及字符串精度；保留参数类型以生成与 Go 一致的类型不匹配诊断。
fn format_string_args(template: &str, redact_arg_pos: &[usize], mut args: Vec<Arg>) -> String {
    RedactErrorArg(&mut args, redact_arg_pos);
    Errorf(template, &args).to_string()
}

/// 对应 `fmt.Sprint(args...)`：逐个参数使用默认 `%v` 展示且不插入分隔符。
fn sprint_args(args: &[Arg]) -> String {
    args.iter()
        .map(|argument| Errorf("%v", std::slice::from_ref(argument)).to_string())
        .collect()
}
