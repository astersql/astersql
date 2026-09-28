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

// protobuf 风格值的十六进制美化打印。
//
// 对应 Go `pkg/util/logutil` 的 Hex / PrettyPrint：将类 protobuf 消息
// 渲染为 `{field:value ...}` 文本；字节字段输出小写十六进制，便于在
// 日志中观察 Region key、编码行等二进制内容。

use std::fmt;

/// protobuf 风格字段：名称 + 值。
#[derive(Clone, Debug, PartialEq)]
pub struct ProtoField {
    /// 字段名；以 `XXX` 开头的字段在 Display 时会被跳过（对齐 Go 生成字段）。
    pub name: String,
    /// 字段值。
    pub value: ProtoValue,
}

impl ProtoField {
    /// 构造字段，名称与值均接受 `Into`。
    pub fn new(name: impl Into<String>, value: impl Into<ProtoValue>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// 可嵌套的 protobuf 风格值，用于日志美化而非真实 protobuf 编解码。
#[derive(Clone, Debug, PartialEq)]
pub enum ProtoValue {
    /// 空值，显示为 `<nil>`。
    Nil,
    /// 布尔。
    Bool(bool),
    /// 有符号 64 位整数。
    I64(i64),
    /// 无符号 64 位整数。
    U64(u64),
    /// 普通字符串（原样输出）。
    String(String),
    /// 字节序列，Display 时转为小写 hex。
    Bytes(Vec<u8>),
    /// 列表，显示为 `[a b c]`。
    List(Vec<ProtoValue>),
    /// 消息（字段集合），显示为 `{k:v ...}`。
    Message(Vec<ProtoField>),
}

impl ProtoValue {
    /// 由字段列表构造消息值。
    pub fn message(fields: Vec<ProtoField>) -> Self {
        Self::Message(fields)
    }
}
impl From<bool> for ProtoValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}
impl From<i64> for ProtoValue {
    fn from(v: i64) -> Self {
        Self::I64(v)
    }
}
impl From<u64> for ProtoValue {
    fn from(v: u64) -> Self {
        Self::U64(v)
    }
}
impl From<String> for ProtoValue {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}
impl From<&str> for ProtoValue {
    fn from(v: &str) -> Self {
        Self::String(v.into())
    }
}
impl From<&[u8]> for ProtoValue {
    fn from(v: &[u8]) -> Self {
        Self::Bytes(v.to_vec())
    }
}
impl From<Vec<u8>> for ProtoValue {
    fn from(v: Vec<u8>) -> Self {
        Self::Bytes(v)
    }
}

impl fmt::Display for ProtoValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nil => f.write_str("<nil>"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::I64(v) => write!(f, "{v}"),
            Self::U64(v) => write!(f, "{v}"),
            Self::String(v) => f.write_str(v),
            Self::Bytes(v) => write_hex(f, v),
            Self::List(values) => {
                // 列表元素以空格分隔，对齐 Go 的 %#v 风格简化输出
                f.write_str("[")?;
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{value}")?;
                }
                f.write_str("]")
            }
            Self::Message(fields) => {
                // 跳过 XXX* 内部字段；与 Go 一样按原始字段索引决定分隔空格。
                f.write_str("{")?;
                for (index, field) in fields.iter().enumerate() {
                    if field.name.starts_with("XXX") {
                        continue;
                    }
                    if index != 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{}:{}", field.name, field.value)?;
                }
                f.write_str("}")
            }
        }
    }
}

/// 将字节序列写成连续小写十六进制字符。
fn write_hex(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        f.write_str(
            std::str::from_utf8(&[DIGITS[(byte >> 4) as usize], DIGITS[(byte & 0xf) as usize]])
                .expect("ASCII hex"),
        )?;
    }
    Ok(())
}

/// `Hex` 返回的惰性 Display 包装，延迟到格式化时再渲染。
pub struct HexStringer<'a>(&'a ProtoValue);
impl fmt::Display for HexStringer<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Go 风格入口：对消息做十六进制美化打印（返回 Display 适配器）。
#[allow(non_snake_case)]
pub fn Hex(message: &ProtoValue) -> HexStringer<'_> {
    HexStringer(message)
}

/// 直接得到美化后的字符串（等同 `Hex(...).to_string()`）。
pub fn pretty_print(value: &ProtoValue) -> String {
    value.to_string()
}
/// Go 风格别名：`prettyPrint`。
pub use pretty_print as prettyPrint;
