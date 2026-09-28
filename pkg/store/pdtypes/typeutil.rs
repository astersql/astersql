// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// PD 类型工具：友好的 JSON 编解码辅助。
//
// `StringSlice` 将字符串向量序列化为逗号分隔的单个 JSON 字符串，
// 对齐 Go 侧对部分 PD 配置/标签字段的编码习惯。

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// StringSlice is more friendly to JSON encode/decode.
/// 字符串切片的 JSON 友好包装：编解码时用逗号拼接/拆分，而非 JSON 数组。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StringSlice(pub Vec<String>);

impl StringSlice {
    /// MarshalJSON joins the values with commas and returns a JSON string.
    /// 将内部字符串用逗号拼接后，再编码为 JSON 字符串字面量。
    pub fn MarshalJSON(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&self.0.join(",")).context("marshal StringSlice as JSON string")
    }

    /// UnmarshalJSON parses a comma-delimited JSON string without mutating on error.
    /// 从逗号分隔的 JSON 字符串解析；解析失败时不修改 `self`。
    pub fn UnmarshalJSON(&mut self, text: &[u8]) -> Result<()> {
        let data: String = serde_json::from_slice(text).context("unquote StringSlice JSON")?;
        // 空串表示空切片；否则按逗号拆分。
        self.0 = if data.is_empty() {
            Vec::new()
        } else {
            data.split(',').map(str::to_owned).collect()
        };
        Ok(())
    }
}

impl Serialize for StringSlice {
    /// serde 序列化：输出逗号拼接的单个字符串。
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.join(","))
    }
}

impl<'de> Deserialize<'de> for StringSlice {
    /// serde 反序列化：从单个字符串按逗号拆回向量。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data = String::deserialize(deserializer)?;
        Ok(Self(if data.is_empty() {
            Vec::new()
        } else {
            data.split(',').map(str::to_owned).collect()
        }))
    }
}
