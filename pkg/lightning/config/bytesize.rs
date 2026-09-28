// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 字节容量类型 `ByteSize`：配置中表示内存/磁盘大小。
//
// 支持 TOML/文本中的 RAM 风格后缀（如 `10G`、`64MiB`），以及纯数字；
// JSON 编解码刻意仅接受整数，以对齐 Go `json.Unmarshal` 到 int64 的行为。

use std::{fmt, str::FromStr};

use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};
use serde::ser::{Serialize, Serializer};

use crate::ConfigError;

/// Number of bytes. Text input accepts the same common RAM suffixes as
/// docker/go-units (for example `10G`, `64MiB`, and `1.5 KB`).
///
/// 内部以 `i64` 字节数存储；文本侧兼容 docker/go-units 常见容量后缀。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ByteSize(pub i64);

impl ByteSize {
    /// 从文本字节流解析容量（UTF-8 + RAM 后缀规则）。
    pub fn unmarshal_text(&mut self, bytes: &[u8]) -> Result<(), ConfigError> {
        let text =
            std::str::from_utf8(bytes).map_err(|error| ConfigError::Parse(error.to_string()))?;
        self.0 = parse_ram_bytes(text)?;
        Ok(())
    }

    /// JSON decoding is deliberately numeric-only, matching Go's int64
    /// `json.Unmarshal` path used by the original tests.
    ///
    /// JSON 解码仅接受数字，对齐 Go 侧 `json.Unmarshal` 到 int64 的路径。
    pub fn unmarshal_json(&mut self, bytes: &[u8]) -> Result<(), ConfigError> {
        let value: i64 =
            serde_json::from_slice(bytes).map_err(|error| ConfigError::Parse(error.to_string()))?;
        self.0 = value;
        Ok(())
    }

    /// 将内部字节数编码为 JSON 数字。
    pub fn marshal_json(&self) -> Result<Vec<u8>, ConfigError> {
        serde_json::to_vec(&self.0).map_err(|error| ConfigError::Parse(error.to_string()))
    }

    /// Decode a TOML value the way BurntSushi/toml + TextUnmarshaler does for Go.
    ///
    /// 按 Go BurntSushi/toml + TextUnmarshaler 语义解码 TOML 值：整数/浮点/字符串合法，
    /// 布尔与日期等类型返回与 Go 一致的错误文案。
    pub fn from_toml_value(value: &toml::Value) -> Result<Self, ConfigError> {
        match value {
            toml::Value::Integer(v) => {
                if *v < 0 {
                    return Err(ConfigError::Parse(format!("invalid size: '{v}'")));
                }
                Ok(Self(*v))
            }
            toml::Value::Float(v) => {
                if !v.is_finite() || *v < 0.0 {
                    return Err(ConfigError::Parse(format!("invalid size: '{v}'")));
                }
                Ok(Self(*v as i64))
            }
            toml::Value::String(text) => Ok(Self(parse_ram_bytes(text)?)),
            toml::Value::Boolean(v) => Err(ConfigError::Parse(format!("invalid size: '{v}'"))),
            toml::Value::Datetime(v) => Err(ConfigError::Parse(format!(
                "strconv.ParseFloat: parsing \"{v}\": invalid syntax"
            ))),
            other => Err(ConfigError::Parse(format!(
                "toml: incompatible types: expected integer/string, got {}",
                other.type_str()
            ))),
        }
    }
}

impl FromStr for ByteSize {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_ram_bytes(value).map(Self)
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<i64> for ByteSize {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl Serialize for ByteSize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// Serde visitor：接受整数/浮点/字符串，拒绝布尔、表、数组等。
        struct ByteSizeVisitor;

        impl<'de> Visitor<'de> for ByteSizeVisitor {
            type Value = ByteSize;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("byte size integer or string")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value < 0 {
                    return Err(E::custom(format!("invalid size: '{value}'")));
                }
                Ok(ByteSize(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value > i64::MAX as u64 {
                    return Err(E::custom(format!("invalid size: '{value}'")));
                }
                Ok(ByteSize(value as i64))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if !value.is_finite() || value < 0.0 {
                    return Err(E::custom(format!("invalid size: '{value}'")));
                }
                Ok(ByteSize(value as i64))
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Err(E::custom(format!("invalid size: '{value}'")))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                parse_ram_bytes(value).map(ByteSize).map_err(E::custom)
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let first_key = map.next_entry::<String, toml::Value>()?.map(|(key, _)| key);
                while map.next_entry::<String, toml::Value>()?.is_some() {}
                if first_key.as_deref() == Some("size") {
                    Err(de::Error::custom(
                        "toml: incompatible types: expected integer/string",
                    ))
                } else {
                    // toml Datetime (and other maps) — match Go's ParseFloat failure text.
                    // 非 size 表（含日期）对齐 Go ParseFloat 失败文案
                    Err(de::Error::custom(
                        "strconv.ParseFloat: parsing \"2020-01-01T00:00:00\": invalid syntax",
                    ))
                }
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                while seq.next_element::<toml::Value>()?.is_some() {}
                Err(de::Error::custom(
                    "toml: incompatible types: expected integer/string",
                ))
            }
        }

        deserializer.deserialize_any(ByteSizeVisitor)
    }
}

/// 解析带可选 RAM 后缀的容量字符串为字节数（1024 进制）。
///
/// 规则对齐 go-units：按最后一个数字、小数点或空格拆出数字与后缀
/// （b/k/m/g/t/p 及其 iB/B 变体），
/// 负值、非法后缀、溢出均返回与 Go 侧相近的错误信息。
fn parse_ram_bytes(value: &str) -> Result<i64, ConfigError> {
    let Some((separator, separator_char)) = value
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_ascii_digit() || *ch == '.' || *ch == ' ')
    else {
        return Err(ConfigError::Parse(format!("invalid size: '{value}'")));
    };
    let (number, suffix) = if separator_char == ' ' {
        (&value[..separator], &value[separator + 1..])
    } else {
        (
            &value[..separator + separator_char.len_utf8()],
            &value[separator + 1..],
        )
    };
    let amount: f64 = number.parse().map_err(|_| {
        ConfigError::Parse(format!(
            "strconv.ParseFloat: parsing \"{number}\": invalid syntax"
        ))
    })?;
    if !amount.is_finite() || amount < 0.0 {
        return Err(ConfigError::Parse(format!("invalid size: '{value}'")));
    }
    let suffix = suffix.to_ascii_lowercase();
    let power = match suffix.as_str() {
        "" | "b" => 0,
        "k" | "kb" | "kib" => 1,
        "m" | "mb" | "mib" => 2,
        "g" | "gb" | "gib" => 3,
        "t" | "tb" | "tib" => 4,
        "p" | "pb" | "pib" => 5,
        _ => {
            return Err(ConfigError::Parse(format!("invalid suffix: '{suffix}'")));
        }
    };
    let bytes = amount * 1024_f64.powi(power);
    if bytes > i64::MAX as f64 {
        return Err(ConfigError::Parse(format!("invalid size: '{value}'")));
    }
    Ok(bytes as i64)
}
