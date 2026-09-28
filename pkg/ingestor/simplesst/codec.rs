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

// SST RangeProperty 编解码。
//
// 将连续键范围的首/末键、文件偏移、字节数与键数编码为长度前缀二进制，
// 供 simple SST 元数据读写；字段顺序与 Go `encodeProp`/`decodeProp` 对齐。

use crate::{Error, Result};

/// 属性中除两个 key 正文外的固定开销：2×u32 长度 + 3×u64 数值。
pub const PROPERTY_LENGTH_EXCEPT_KEYS: usize = 4 * 2 + 8 * 3;

/// SST 中一段连续键范围的统计：边界键、文件偏移、字节数与键数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RangeProperty {
    pub FirstKey: Vec<u8>,
    pub LastKey: Vec<u8>,
    pub Offset: u64,
    pub Size: u64,
    pub Keys: u64,
}

/// 从 `data` 按游标切出 `count` 字节；越界或溢出返回 InvalidData。
fn take<'a>(data: &'a [u8], cursor: &mut usize, count: usize) -> Result<&'a [u8]> {
    let end = cursor
        .checked_add(count)
        .ok_or_else(|| Error::InvalidData("property length overflow".into()))?;
    if end > data.len() {
        return Err(Error::InvalidData("truncated range property".into()));
    }
    let out = &data[*cursor..end];
    *cursor = end;
    Ok(out)
}

/// 解码单条属性正文：首键、末键、Size、Keys、Offset。
///
/// 与 Go `decodeProp` 一致，规定字段之后的字节由调用方管理，本函数不拒绝。
pub fn decode_prop(data: &[u8]) -> Result<RangeProperty> {
    let mut cursor = 0;
    let first_len = u32::from_be_bytes(take(data, &mut cursor, 4)?.try_into().unwrap()) as usize;
    let first = take(data, &mut cursor, first_len)?.to_vec();
    let last_len = u32::from_be_bytes(take(data, &mut cursor, 4)?.try_into().unwrap()) as usize;
    let last = take(data, &mut cursor, last_len)?.to_vec();
    // 数值字段顺序固定为 Size、Keys、Offset（大端）
    let size = u64::from_be_bytes(take(data, &mut cursor, 8)?.try_into().unwrap());
    let keys = u64::from_be_bytes(take(data, &mut cursor, 8)?.try_into().unwrap());
    let offset = u64::from_be_bytes(take(data, &mut cursor, 8)?.try_into().unwrap());
    Ok(RangeProperty {
        FirstKey: first,
        LastKey: last,
        Offset: offset,
        Size: size,
        Keys: keys,
    })
}

/// 将单条属性按 Go 字段顺序追加到 `buf`（不含外层总长度）。
pub fn encode_prop(buf: &mut Vec<u8>, property: &RangeProperty) -> Result<()> {
    let first_len = u32::try_from(property.FirstKey.len())
        .map_err(|_| Error::InvalidData("first key too large".into()))?;
    let last_len = u32::try_from(property.LastKey.len())
        .map_err(|_| Error::InvalidData("last key too large".into()))?;
    buf.extend_from_slice(&first_len.to_be_bytes());
    buf.extend_from_slice(&property.FirstKey);
    buf.extend_from_slice(&last_len.to_be_bytes());
    buf.extend_from_slice(&property.LastKey);
    buf.extend_from_slice(&property.Size.to_be_bytes());
    buf.extend_from_slice(&property.Keys.to_be_bytes());
    buf.extend_from_slice(&property.Offset.to_be_bytes());
    Ok(())
}

/// 多属性编码：每条前写入 4 字节正文长度，再跟 `encode_prop` 正文。
pub fn encode_multi_props(props: &[RangeProperty]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for property in props {
        let length = PROPERTY_LENGTH_EXCEPT_KEYS
            .checked_add(property.FirstKey.len())
            .and_then(|n| n.checked_add(property.LastKey.len()))
            .ok_or_else(|| Error::InvalidData("property length overflow".into()))?;
        out.extend_from_slice(
            &u32::try_from(length)
                .map_err(|_| Error::InvalidData("property too large".into()))?
                .to_be_bytes(),
        );
        encode_prop(&mut out, property)?;
    }
    Ok(out)
}

/// 逐条消费 `<4-byte length><property>` 序列并解码。
pub fn decode_multi_props(mut data: &[u8]) -> Result<Vec<RangeProperty>> {
    let mut result = Vec::new();
    while !data.is_empty() {
        if data.len() < 4 {
            return Err(Error::InvalidData("truncated property length".into()));
        }
        let length = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
        if data.len() < 4 + length {
            return Err(Error::InvalidData("truncated property".into()));
        }
        result.push(decode_prop(&data[4..4 + length])?);
        data = &data[4 + length..];
    }
    Ok(result)
}

/// Go 风格别名：转发到 `decode_prop`。
pub fn decodeProp(data: &[u8]) -> Result<RangeProperty> {
    decode_prop(data)
}
/// Go 风格别名：在拥有型缓冲上编码单条属性并返回。
pub fn encodeProp(mut buf: Vec<u8>, property: &RangeProperty) -> Result<Vec<u8>> {
    encode_prop(&mut buf, property)?;
    Ok(buf)
}
/// Go 风格别名：转发到 `decode_multi_props`。
pub fn decodeMultiProps(data: &[u8]) -> Result<Vec<RangeProperty>> {
    decode_multi_props(data)
}
/// Go 风格别名：将多属性编码结果追加到已有缓冲。
pub fn encodeMultiProps(mut buf: Vec<u8>, props: &[RangeProperty]) -> Result<Vec<u8>> {
    buf.extend_from_slice(&encode_multi_props(props)?);
    Ok(buf)
}
