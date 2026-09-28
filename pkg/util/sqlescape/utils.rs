// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// SQL 字符串与参数转义工具。
//
// 对应 Go `pkg/util/sqlescape`：实现 `%?`（按类型自动转换）、`%n`（标识符）、
// `%%`（字面量百分号）占位符；错误语义与 Go `errors.Errorf` 文案对齐。

// SQL 字符串和参数转义工具。实现保持 Go 版本的占位符、类型分支和错误语义。

#![allow(non_snake_case)]
#![allow(dead_code)]

use std::io::{self, Write};

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

/// 本模块的单元测试（含 reserveBuffer、转义与 EscapeSQL 表驱动）。
#[cfg(test)]
#[path = "utils_test.rs"]
mod utils_test;

/// 转义过程中可能返回的错误：缺参、标识符类型、不支持参数、IO。
// SqlEscapeError 对应 Go 代码里通过 errors.Errorf 构造并向上返回的错误。
// 把几类错误显式化，便于保留缺参、标识符类型错误、Writer IO 错误等 Go 语义。
#[derive(Debug)]
pub enum SqlEscapeError {
    /// 模板需要第 `need` 个参数，但只提供了 `got` 个。
    MissingArguments { need: usize, got: usize },
    /// `%n` 期望字符串标识符，实际类型用 `got` 描述。
    ExpectedStringIdentifier { got: String },
    /// 第 `position` 个参数类型不受支持。
    UnsupportedArgument { position: usize, value: String },
    /// 写入 `io::Writer` 失败时的错误文本。
    Io(String),
}

impl std::fmt::Display for SqlEscapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // 对应 Go: errors.Errorf("missing arguments, need %d-th arg, but only got %d args", ...)
            SqlEscapeError::MissingArguments { need, got } => {
                write!(
                    f,
                    "missing arguments, need {need}-th arg, but only got {got} args"
                )
            }
            // 对应 Go: errors.Errorf("expect a string identifier, got %v", arg)
            SqlEscapeError::ExpectedStringIdentifier { got } => {
                write!(f, "expect a string identifier, got {got}")
            }
            // 对应 Go: errors.Errorf("unsupported %d-th argument: %v", argPos, arg)
            SqlEscapeError::UnsupportedArgument { position, value } => {
                write!(f, "unsupported {position}-th argument: {value}")
            }
            // FormatSQL 把 io.Writer 的错误继续返回；这里用字符串保存原始 IO 错误。
            SqlEscapeError::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SqlEscapeError {}

impl From<io::Error> for SqlEscapeError {
    fn from(err: io::Error) -> Self {
        SqlEscapeError::Io(err.to_string())
    }
}

/// 保存 EscapeSQL 所需的 Go `time.Time` 格式化结果（零日期或带微秒文本）。
// GoTime 保存 EscapeSQL 所需的 time.Time 格式化结果。
#[derive(Clone, Debug)]
pub struct GoTime {
    is_zero: bool,
    formatted: String,
}

impl GoTime {
    /// 构造 Go 零值时间，格式化时输出 MySQL 零日期 `'0000-00-00'`。
    pub fn zero() -> Self {
        Self {
            is_zero: true,
            formatted: String::new(),
        }
    }

    /// 从 `NaiveDateTime` 生成与 Go layout `2006-01-02 15:04:05.999999` 同形文本。
    pub fn from_naive(value: NaiveDateTime) -> Self {
        let base = value.format("%Y-%m-%d %H:%M:%S").to_string();
        let micros = value.and_utc().timestamp_subsec_micros();
        let formatted = if micros == 0 {
            base
        } else {
            format!("{base}.{}", format!("{micros:06}").trim_end_matches('0'))
        };
        Self {
            is_zero: false,
            formatted,
        }
    }

    /// 按年月日时分秒纳秒构造；任一分量非法则返回 `None`。
    #[allow(clippy::too_many_arguments)]
    pub fn from_components(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        nanosecond: u32,
    ) -> Option<Self> {
        let date = NaiveDate::from_ymd_opt(year, month, day)?;
        let time = NaiveTime::from_hms_nano_opt(hour, minute, second, nanosecond)?;
        Some(Self::from_naive(NaiveDateTime::new(date, time)))
    }
}

/// 对应 Go type switch default 中通过 reflect 处理底层 Kind 的慢路径值。
// ReflectedValue 对应 Go type switch default 分支里通过 reflect 处理底层类型的慢路径。
// 例如测试里的 myInt/myStr 不是内建 int/string，但 Kind 仍然能落到 Int/String。
#[derive(Clone, Debug)]
pub enum ReflectedValue {
    Int(i64),
    Uint(u64),
    Float32(f64),
    Float64(f64),
    Bool(bool),
    String(String),
    Unsupported(String),
}

/// 机械表达 Go `args ...any`：枚举列出原文件 type switch 支持的类型分支。
// SqlArg 机械表达 Go 的 args ...any。
// Rust 没有 Go interface{} + type switch 的同形运行时机制，所以这里用枚举列出原文件支持的类型分支。
#[derive(Clone, Debug)]
pub enum SqlArg {
    Nil,
    Int(isize),
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Uint(usize),
    Uint8(u8),
    Uint16(u16),
    Uint32(u32),
    Uint64(u64),
    Float32(f32),
    Float64(f64),
    Bool(bool),
    Time(GoTime),
    JsonRawMessage(Vec<u8>),
    Bytes(Option<Vec<u8>>),
    String(String),
    StringSlice(Vec<String>),
    Float32Slice(Vec<f32>),
    Float64Slice(Vec<f64>),
    Reflected(ReflectedValue),
    Unsupported(String),
}

impl SqlArg {
    // debug_value 模拟 Go %v 只用于错误文本；不承诺完全等同 fmt 包格式。
    fn debug_value(&self) -> String {
        format!("{self:?}")
    }
}

/// 保证缓冲区至少还能再写入 `appendSize` 字节；扩容公式对齐 Go。
// reserveBuffer 对应 Go 的 reserveBuffer。
// Go 版本扩容时分配 len(buf)*2+appendSize，并返回长度增加 appendSize 的切片。
fn reserveBuffer(mut buf: Vec<u8>, appendSize: usize) -> Vec<u8> {
    let newSize = buf.len() + appendSize;
    if buf.capacity() < newSize {
        // 这里保留 Go 的扩容公式；Rust Vec 只保证 capacity，不暴露与 Go slice 完全一致的 cap 语义。
        let mut newBuf = Vec::with_capacity(buf.len() * 2 + appendSize);
        newBuf.extend_from_slice(&buf);
        buf = newBuf;
    }
    // Go 返回 buf[:newSize]，新增区域可被后续按下标写入；Rust 用 0 填充后再覆盖。
    buf.resize(newSize, 0);
    buf
}

/// 按 MySQL 风格把字节切片反斜杠转义后追加到缓冲区。
// escapeBytesBackslash will escape []byte into the buffer, with backslash.
// escapeBytesBackslash 按 Go switch 顺序转义字节切片。
// 它只处理 SQL 字面量需要特殊处理的若干字节，其它字节保持原样写回缓冲区。
fn escapeBytesBackslash(buf: Vec<u8>, v: &[u8]) -> Vec<u8> {
    let mut pos = buf.len();
    // Go 预留 len(v)*2，因为最坏情况下每个输入字节都会变成反斜杠加一个标记字节。
    let mut buf = reserveBuffer(buf, v.len() * 2);

    for &c in v {
        match c {
            b'\x00' => {
                // MySQL 风格把 NUL 写成 \0。
                buf[pos] = b'\\';
                buf[pos + 1] = b'0';
                pos += 2;
            }
            b'\n' => {
                buf[pos] = b'\\';
                buf[pos + 1] = b'n';
                pos += 2;
            }
            b'\r' => {
                buf[pos] = b'\\';
                buf[pos + 1] = b'r';
                pos += 2;
            }
            b'\x1a' => {
                // 0x1a 是 MySQL 文本协议里需要特殊处理的 substitute 字符。
                buf[pos] = b'\\';
                buf[pos + 1] = b'Z';
                pos += 2;
            }
            b'\'' => {
                buf[pos] = b'\\';
                buf[pos + 1] = b'\'';
                pos += 2;
            }
            b'"' => {
                buf[pos] = b'\\';
                buf[pos + 1] = b'"';
                pos += 2;
            }
            b'\\' => {
                buf[pos] = b'\\';
                buf[pos + 1] = b'\\';
                pos += 2;
            }
            _ => {
                // 默认分支保留原始字节；这也意味着非 UTF-8 字节会按 Go []byte 语义继续传递。
                buf[pos] = c;
                pos += 1;
            }
        }
    }

    // Go 返回 buf[:pos]，丢弃预留但没有实际写入的尾部空间。
    buf.truncate(pos);
    buf
}

/// 仅做字符串反斜杠转义（不加单引号）；常规场景请用 `EscapeSQL`。
// EscapeString is used by session/bootstrap.go, which has some
// dynamic query building cases not well handled by this package.
// For normal usage, please use EscapeSQL instead!
// EscapeString 只做字符串反斜杠转义；它不负责给结果加单引号。
pub fn EscapeString(s: &str) -> String {
    let buf = Vec::with_capacity(s.len());
    String::from_utf8_lossy(&escapeStringBackslash(buf, s)).into_owned()
}

/// 将字符串按原始字节做反斜杠转义（对应 Go `hack.Slice`）。
// escapeStringBackslash will escape string into the buffer, with backslash.
// escapeStringBackslash 对应 Go 里的 hack.Slice(v)。
// Go hack.Slice 避免字符串到 []byte 的拷贝；用 as_bytes 借用表达同一个“按原始字节转义”的意图。
fn escapeStringBackslash(buf: Vec<u8>, v: &str) -> Vec<u8> {
    escapeBytesBackslash(buf, v.as_bytes())
}

/// 扫描 SQL 模板百分号占位符，按参数类型把参数追加进输出缓冲区。
// escapeSQL is the internal impl of EscapeSQL and FormatSQL.
// escapeSQL 是 EscapeSQL 和 FormatSQL 的内部实现。
// 它扫描 SQL 模板中的百分号占位符，并按参数类型把参数追加进输出缓冲区。
fn escapeSQL(sql: &str, args: &[SqlArg]) -> Result<Vec<u8>, SqlEscapeError> {
    let mut buf = Vec::with_capacity(sql.len());
    let mut argPos = 0usize;
    let sqlBytes = sql.as_bytes();
    let mut i = 0usize;

    while i < sqlBytes.len() {
        // Go 使用 strings.IndexByte(sql[i:], '%') 在字节层面查找百分号。
        // Rust 这里也基于 bytes 扫描，避免 UTF-8 字符宽度影响 Go 的下标语义。
        let Some(q) = sqlBytes[i..].iter().position(|&b| b == b'%') else {
            buf.extend_from_slice(&sqlBytes[i..]);
            break;
        };
        buf.extend_from_slice(&sqlBytes[i..i + q]);
        i += q;

        // ch 为百分号后的 specifier；模板以单独 '%' 结尾时保持 Go 的 byte(0) 哨兵。
        let ch = if i + 1 < sqlBytes.len() {
            sqlBytes[i + 1]
        } else {
            0
        };
        match ch {
            b'n' => {
                let arg = next_arg(args, &mut argPos)?;

                let SqlArg::String(v) = arg else {
                    return Err(SqlEscapeError::ExpectedStringIdentifier {
                        got: arg.debug_value(),
                    });
                };
                buf.push(b'`');
                // %n 表示 identifier；Go 用 strings.ReplaceAll(v, "`", "``") 转义反引号。
                buf.extend_from_slice(v.replace('`', "``").as_bytes());
                buf.push(b'`');
                // Go 的 for 循环体内 i++ 跳过 specifier；while 直接前进两个字节。
                i += 2;
            }
            b'?' => {
                let arg = next_arg(args, &mut argPos)?;
                appendSQLArg(&mut buf, arg, argPos)?;
                i += 2;
            }
            b'%' => {
                // %% 输出一个字面量百分号，不消费任何参数。
                buf.push(b'%');
                i += 2;
            }
            _ => {
                // 未知 specifier 或截断的 '%' 按 Go 语义只输出原始 '%'。
                buf.push(b'%');
                i += 1;
            }
        }
    }
    Ok(buf)
}

/// 布尔参数：`true` 写 `1`，`false` 写 `0`。
// appendSQLArgBool 对应 Go helper：true 写 1，false 写 0。
fn appendSQLArgBool(buf: &mut Vec<u8>, v: bool) {
    if v {
        buf.push(b'1');
    } else {
        buf.push(b'0');
    }
}

/// 字符串参数：加单引号后再做反斜杠转义。
// appendSQLArgString 对应 Go helper：字符串参数先加单引号，再做反斜杠转义。
fn appendSQLArgString(buf: &mut Vec<u8>, s: &str) {
    buf.push(b'\'');
    *buf = escapeStringBackslash(std::mem::take(buf), s);
    buf.push(b'\'');
}

/// 将参数转义进 SQL 模板并返回字符串；支持 `%?`/`%%`/`%n`。
// EscapeSQL will escape input arguments into the sql string, doing necessary processing.
// It works like printf() in c, there are following format specifiers:
// 1. %?: automatic conversion by the type of arguments. E.g. []string -> ('s1','s2'..)
// 2. %%: output %
// 3. %n: for identifiers, for example ("use %n", db)
// But it does not prevent you from doing:
/*
    EscapeSQL("select '%?", ";SQL injection!;") => "select '';SQL injection!;'".
*/
// It is still your responsibility to write safe SQL.
// EscapeSQL 是公开字符串接口；它复用 escapeSQL 的字节缓冲结果并转换为字符串返回。
pub fn EscapeSQL(sql: &str, args: &[SqlArg]) -> Result<String, SqlEscapeError> {
    let str = escapeSQL(sql, args)?;
    Ok(String::from_utf8_lossy(&str).into_owned())
}

/// `EscapeSQL` 的 panic-on-error 包装，适合静态保证参数不会出错的场景。
// MustEscapeSQL is a helper around EscapeSQL. The error returned from escapeSQL can be avoided statically if you do not pass interface{}.
// MustEscapeSQL 保留 Go 的 panic-on-error 语义，适合调用方静态保证参数不会出错的场景。
pub fn MustEscapeSQL(sql: &str, args: &[SqlArg]) -> String {
    match EscapeSQL(sql, args) {
        Ok(r) => r,
        Err(err) => panic!("{err}"),
    }
}

/// `EscapeSQL` 的 `io::Writer` 版本：把已转义字节写入传入 writer。
// FormatSQL is the io.Writer version of EscapeSQL. Please refer to EscapeSQL for details.
// FormatSQL 对应 Go 的 io.Writer 版本。
// 这是本文件唯一的 IO 边界：它只把已经转义好的字节写入传入 writer，不负责打开文件或网络连接。
pub fn FormatSQL<W: Write>(w: &mut W, sql: &str, args: &[SqlArg]) -> Result<(), SqlEscapeError> {
    let buf = escapeSQL(sql, args)?;
    // Go 调用 Writer.Write 一次并忽略返回的字节数。
    let _ = w.write(&buf)?;
    Ok(())
}

/// `FormatSQL` 的 Must 版；用 `Vec<u8>` 表达不会失败的内存缓冲 writer。
// MustFormatSQL is a helper around FormatSQL, like MustEscapeSQL. But it asks that the writer must be strings.Builder,
// which will not return error when w.Write(...).
// MustFormatSQL 对应 Go 的 strings.Builder 特化版本。
// Rust 用 Vec<u8> 表达“不会失败的内存缓冲 writer”，从而保留 Go 中忽略写入错误风险的前提。
pub fn MustFormatSQL(w: &mut Vec<u8>, sql: &str, args: &[SqlArg]) {
    if let Err(err) = FormatSQL(w, sql, args) {
        panic!("{err}");
    }
}

/// 取下一个参数并递增 `argPos`；不足则返回 `MissingArguments`。
// next_arg 保留 Go 中 argPos 的递增语义。
// 缺参时返回与 Go errors.Errorf 对应的 MissingArguments 错误。
fn next_arg<'a>(args: &'a [SqlArg], argPos: &mut usize) -> Result<&'a SqlArg, SqlEscapeError> {
    if *argPos >= args.len() {
        return Err(SqlEscapeError::MissingArguments {
            need: *argPos + 1,
            got: args.len(),
        });
    }
    let arg = &args[*argPos];
    *argPos += 1;
    Ok(arg)
}

/// 对应 Go `%?` 分支下的 type switch，按 `SqlArg` 变体写入缓冲区。
// appendSQLArg 对应 Go 中 %? 分支下的 type switch。
// 单独拆出函数只是为了让每个 Go case 在 Rust 里更容易阅读和对照。
fn appendSQLArg(buf: &mut Vec<u8>, arg: &SqlArg, argPos: usize) -> Result<(), SqlEscapeError> {
    match arg {
        SqlArg::Nil => {
            buf.extend_from_slice(b"NULL");
        }
        SqlArg::Int(v) => appendGoInt(buf, *v as i64),
        SqlArg::Int8(v) => appendGoInt(buf, *v as i64),
        SqlArg::Int16(v) => appendGoInt(buf, *v as i64),
        SqlArg::Int32(v) => appendGoInt(buf, *v as i64),
        SqlArg::Int64(v) => appendGoInt(buf, *v),
        SqlArg::Uint(v) => appendGoUint(buf, *v as u64),
        SqlArg::Uint8(v) => appendGoUint(buf, *v as u64),
        SqlArg::Uint16(v) => appendGoUint(buf, *v as u64),
        SqlArg::Uint32(v) => appendGoUint(buf, *v as u64),
        SqlArg::Uint64(v) => appendGoUint(buf, *v),
        SqlArg::Float32(v) => appendGoFloat32(buf, *v),
        SqlArg::Float64(v) => appendGoFloat64(buf, *v),
        SqlArg::Bool(v) => appendSQLArgBool(buf, *v),
        SqlArg::Time(v) => {
            if v.is_zero {
                // Go time.Time{} 被格式化为 MySQL 零日期。
                buf.extend_from_slice(b"'0000-00-00'");
            } else {
                buf.push(b'\'');
                // Go 使用 layout "2006-01-02 15:04:05.999999"；要求调用者传入同形文本。
                buf.extend_from_slice(v.formatted.as_bytes());
                buf.push(b'\'');
            }
        }
        SqlArg::JsonRawMessage(v) => {
            // json.RawMessage 本质是 []byte，但 Go 分支会作为带引号字符串处理。
            buf.push(b'\'');
            *buf = escapeBytesBackslash(std::mem::take(buf), v);
            buf.push(b'\'');
        }
        SqlArg::Bytes(v) => {
            if let Some(bytes) = v {
                // 非 nil []byte 按 Go 语义写成 _binary'...'；空切片仍然输出 _binary''。
                buf.extend_from_slice(b"_binary'");
                *buf = escapeBytesBackslash(std::mem::take(buf), bytes);
                buf.push(b'\'');
            } else {
                // Go 里 nil []byte 与 nil interface 分支一样输出 NULL。
                buf.extend_from_slice(b"NULL");
            }
        }
        SqlArg::String(v) => appendSQLArgString(buf, v),
        SqlArg::StringSlice(v) => {
            for (i, k) in v.iter().enumerate() {
                if i > 0 {
                    buf.push(b',');
                }
                buf.push(b'\'');
                *buf = escapeStringBackslash(std::mem::take(buf), k);
                buf.push(b'\'');
            }
        }
        SqlArg::Float32Slice(v) => {
            for (i, k) in v.iter().enumerate() {
                if i > 0 {
                    buf.push(b',');
                }
                appendGoFloat32(buf, *k);
            }
        }
        SqlArg::Float64Slice(v) => {
            for (i, k) in v.iter().enumerate() {
                if i > 0 {
                    buf.push(b',');
                }
                appendGoFloat64(buf, *k);
            }
        }
        SqlArg::Reflected(v) => {
            // slow path based on reflection
            // Go 对没有命中特定 case 的别名类型使用 reflect.Kind 再判断一次。
            appendReflectedSQLArg(buf, v, argPos)?;
        }
        SqlArg::Unsupported(value) => {
            return Err(SqlEscapeError::UnsupportedArgument {
                position: argPos,
                value: value.clone(),
            });
        }
    }
    Ok(())
}

/// 保留 Go default 分支中的 reflect.Kind switch（用 `ReflectedValue` 占位）。
// appendReflectedSQLArg 保留 Go default 分支中的 reflect.Kind switch。
// 这里不做真实反射，只消费 ReflectedValue 占位，表示该值来自 Go 的底层 kind。
fn appendReflectedSQLArg(
    buf: &mut Vec<u8>,
    value: &ReflectedValue,
    argPos: usize,
) -> Result<(), SqlEscapeError> {
    match value {
        ReflectedValue::Int(v) => appendGoInt(buf, *v),
        ReflectedValue::Uint(v) => appendGoUint(buf, *v),
        ReflectedValue::Float32(v) => appendGoFloat32(buf, *v as f32),
        ReflectedValue::Float64(v) => appendGoFloat64(buf, *v),
        ReflectedValue::Bool(v) => appendSQLArgBool(buf, *v),
        ReflectedValue::String(v) => appendSQLArgString(buf, v),
        ReflectedValue::Unsupported(value) => {
            return Err(SqlEscapeError::UnsupportedArgument {
                position: argPos,
                value: value.clone(),
            });
        }
    }
    Ok(())
}

/// 对应 `strconv.AppendInt(buf, ..., 10)`。
// appendGoInt 对应 strconv.AppendInt(buf, ..., 10)。
fn appendGoInt(buf: &mut Vec<u8>, v: i64) {
    buf.extend_from_slice(v.to_string().as_bytes());
}

/// 对应 `strconv.AppendUint(buf, ..., 10)`。
// appendGoUint 对应 strconv.AppendUint(buf, ..., 10)。
fn appendGoUint(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(v.to_string().as_bytes());
}

/// 将 f32 格式化为与 Go strconv 最短往返同形的文本。
fn appendGoFloat32(buf: &mut Vec<u8>, v: f32) {
    let mut formatter = ryu::Buffer::new();
    appendGoFloatText(buf, formatter.format(v));
}

/// 将 f64 格式化为与 Go strconv 最短往返同形的文本。
fn appendGoFloat64(buf: &mut Vec<u8>, v: f64) {
    let mut formatter = ryu::Buffer::new();
    appendGoFloatText(buf, formatter.format(v));
}

/// 把 ryu 文本调整为 Go 风格（Inf/NaN、显式 `e+` 指数等）。
// ryu 生成与 strconv 相同的最短往返数字；Go 的正指数带显式 '+'。
fn appendGoFloatText(buf: &mut Vec<u8>, value: &str) {
    match value {
        "inf" => {
            buf.extend_from_slice(b"+Inf");
            return;
        }
        "-inf" => {
            buf.extend_from_slice(b"-Inf");
            return;
        }
        "NaN" => {
            buf.extend_from_slice(b"NaN");
            return;
        }
        _ => {}
    }

    if let Some((mantissa, exponent)) = value.split_once('e') {
        let exponent = exponent
            .parse::<i32>()
            .expect("ryu emits a numeric exponent");
        appendGoExponent(buf, mantissa.trim_end_matches(".0"), exponent);
        return;
    }

    let (negative, unsigned) = value
        .strip_prefix('-')
        .map_or((false, value), |v| (true, v));
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let significant_exponent = if let Some(index) = integer.bytes().position(|byte| byte != b'0') {
        integer.len() as i32 - index as i32 - 1
    } else if let Some(index) = fraction.bytes().position(|byte| byte != b'0') {
        -(index as i32) - 1
    } else {
        if negative {
            buf.push(b'-');
        }
        buf.push(b'0');
        return;
    };

    if !(-4..6).contains(&significant_exponent) {
        let mut digits = integer
            .bytes()
            .chain(fraction.bytes())
            .skip_while(|byte| *byte == b'0')
            .collect::<Vec<_>>();
        while digits.len() > 1 && digits.last() == Some(&b'0') {
            digits.pop();
        }
        if negative {
            buf.push(b'-');
        }
        buf.push(digits[0]);
        if digits.len() > 1 {
            buf.push(b'.');
            buf.extend_from_slice(&digits[1..]);
        }
        appendGoExponentSuffix(buf, significant_exponent);
        return;
    }

    if negative {
        buf.push(b'-');
    }
    buf.extend_from_slice(unsigned.trim_end_matches(".0").as_bytes());
}

/// 写出尾数后接 Go 风格指数后缀。
fn appendGoExponent(buf: &mut Vec<u8>, mantissa: &str, exponent: i32) {
    buf.extend_from_slice(mantissa.as_bytes());
    appendGoExponentSuffix(buf, exponent);
}

/// 写出 `e`/`e+`/`e-` 及至少两位指数（与 Go strconv 对齐）。
fn appendGoExponentSuffix(buf: &mut Vec<u8>, exponent: i32) {
    buf.push(b'e');
    buf.push(if exponent < 0 { b'-' } else { b'+' });
    let magnitude = exponent.unsigned_abs();
    if magnitude < 10 {
        buf.push(b'0');
    }
    buf.extend_from_slice(magnitude.to_string().as_bytes());
}
