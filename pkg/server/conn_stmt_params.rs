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

// 预处理语句二进制参数解析。
//
// 实现 MySQL 二进制协议中参数类型常量、null bitmap 与 length-encoded
// 值的解码，并支持 COM_STMT_SEND_LONG_DATA 预先绑定的参数。

use std::fmt;

use astersql_server_internal_util::{InputDecoder, NewInputDecoder};

/// DECIMAL 类型码。
pub const TYPE_DECIMAL: u8 = 0;
/// TINYINT。
pub const TYPE_TINY: u8 = 1;
/// SMALLINT。
pub const TYPE_SHORT: u8 = 2;
/// INT。
pub const TYPE_LONG: u8 = 3;
/// FLOAT。
pub const TYPE_FLOAT: u8 = 4;
/// DOUBLE。
pub const TYPE_DOUBLE: u8 = 5;
/// NULL 类型。
pub const TYPE_NULL: u8 = 6;
/// TIMESTAMP。
pub const TYPE_TIMESTAMP: u8 = 7;
/// BIGINT。
pub const TYPE_LONGLONG: u8 = 8;
/// MEDIUMINT。
pub const TYPE_INT24: u8 = 9;
/// DATE。
pub const TYPE_DATE: u8 = 10;
/// TIME/DURATION。
pub const TYPE_DURATION: u8 = 11;
/// DATETIME。
pub const TYPE_DATETIME: u8 = 12;
/// YEAR。
pub const TYPE_YEAR: u8 = 13;
/// VARCHAR。
pub const TYPE_VARCHAR: u8 = 15;
/// BIT。
pub const TYPE_BIT: u8 = 16;
/// NEWDECIMAL。
pub const TYPE_NEW_DECIMAL: u8 = 246;
/// ENUM。
pub const TYPE_ENUM: u8 = 247;
/// SET。
pub const TYPE_SET: u8 = 248;
/// TINYBLOB。
pub const TYPE_TINY_BLOB: u8 = 249;
/// MEDIUMBLOB。
pub const TYPE_MEDIUM_BLOB: u8 = 250;
/// LONGBLOB。
pub const TYPE_LONG_BLOB: u8 = 251;
/// BLOB。
pub const TYPE_BLOB: u8 = 252;
/// VAR_STRING。
pub const TYPE_VAR_STRING: u8 = 253;
/// STRING。
pub const TYPE_STRING: u8 = 254;
/// GEOMETRY。
pub const TYPE_GEOMETRY: u8 = 255;
/// 未指定类型（与 DECIMAL 同值 0，按上下文解释）。
pub const TYPE_UNSPECIFIED: u8 = 0;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 参数解析错误：报文畸形或未知字段类型。
pub enum ParamError {
    MalformedPacket,
    UnknownFieldType(u8),
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedPacket => f.write_str("malformed packet"),
            Self::UnknownFieldType(tp) => write!(f, "stmt unknown field type {tp}"),
        }
    }
}

impl std::error::Error for ParamError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单个二进制协议参数的解码结果。
pub struct BinaryParam {
    pub tp: u8,
    pub is_unsigned: bool,
    pub is_null: bool,
    pub val: Vec<u8>,
}

/// 从参数值缓冲区按长度切片，返回切片与新游标位置。
pub fn takeBinaryParamValue(
    param_values: &[u8],
    pos: usize,
    length: u64,
) -> Result<(&[u8], usize), ParamError> {
    let length = usize::try_from(length).map_err(|_| ParamError::MalformedPacket)?;
    let end = pos.checked_add(length).ok_or(ParamError::MalformedPacket)?;
    let value = param_values
        .get(pos..end)
        .ok_or(ParamError::MalformedPacket)?;
    Ok((value, end))
}

/// 解析全部二进制参数：优先 bound long-data，再处理 null bitmap 与类型定长/变长值。
pub fn parseBinaryParams(
    params: &mut [BinaryParam],
    bound_params: &[Option<Vec<u8>>],
    null_bitmap: &[u8],
    param_types: &[u8],
    param_values: &[u8],
    decoder: Option<&InputDecoder>,
) -> Result<(), ParamError> {
    // bound 槽位或 null bitmap 不足以覆盖全部参数时视为畸形包。
    if bound_params.len() < params.len() || null_bitmap.len() * 8 < params.len() {
        return Err(ParamError::MalformedPacket);
    }
    let fallback_decoder = NewInputDecoder("utf8");
    let decoder = decoder.unwrap_or(&fallback_decoder);
    let mut pos = 0;

    for (index, param) in params.iter_mut().enumerate() {
        // 已通过 SEND_LONG_DATA 绑定的参数直接使用，必要时按文本类型解码。
        if let Some(bound) = &bound_params[index] {
            param.tp = TYPE_BLOB;
            param.is_unsigned = false;
            param.is_null = false;
            param.val = bound.clone();
            if index * 2 + 1 < param_types.len() {
                let tp = param_types[index * 2];
                match tp {
                    TYPE_VARCHAR | TYPE_VAR_STRING | TYPE_STRING | TYPE_BIT => {
                        param.tp = tp;
                        param.val = decoder.DecodeInput(bound);
                    }
                    TYPE_BLOB | TYPE_TINY_BLOB | TYPE_MEDIUM_BLOB | TYPE_LONG_BLOB => {
                        param.tp = tp;
                    }
                    _ => {}
                }
            }
            continue;
        }

        // null bitmap 置位则参数为 NULL。
        if null_bitmap[index >> 3] & (1 << (index & 7)) != 0 {
            *param = BinaryParam {
                tp: TYPE_NULL,
                is_null: true,
                ..BinaryParam::default()
            };
            continue;
        }
        if index * 2 + 1 >= param_types.len() {
            return Err(ParamError::MalformedPacket);
        }

        let tp = param_types[index * 2];
        let is_unsigned = param_types[index * 2 + 1] & 0x80 != 0;
        let mut is_null = false;
        let mut decode = false;
        // 按字段类型确定定长宽度或读取 length-encoded / 日期长度前缀。
        let length = match tp {
            TYPE_NULL => {
                is_null = true;
                0
            }
            TYPE_TINY => 1,
            TYPE_SHORT | TYPE_YEAR => 2,
            TYPE_INT24 | TYPE_LONG | TYPE_FLOAT => 4,
            TYPE_LONGLONG | TYPE_DOUBLE => 8,
            TYPE_DATE | TYPE_TIMESTAMP | TYPE_DATETIME | TYPE_DURATION => {
                let length = *param_values.get(pos).ok_or(ParamError::MalformedPacket)? as u64;
                pos += 1;
                length
            }
            TYPE_NEW_DECIMAL | TYPE_BLOB | TYPE_TINY_BLOB | TYPE_MEDIUM_BLOB | TYPE_LONG_BLOB => {
                let (length, null, used) = parse_length_encoded_int(&param_values[pos..])?;
                pos += used;
                is_null = null;
                length
            }
            TYPE_UNSPECIFIED | TYPE_VARCHAR | TYPE_VAR_STRING | TYPE_STRING | TYPE_ENUM
            | TYPE_SET | TYPE_GEOMETRY | TYPE_BIT => {
                let (length, null, used) = parse_length_encoded_int(&param_values[pos..])?;
                pos += used;
                is_null = null;
                decode = true;
                length
            }
            _ => return Err(ParamError::UnknownFieldType(tp)),
        };

        let (value, next) = takeBinaryParamValue(param_values, pos, length)?;
        *param = BinaryParam {
            tp,
            is_unsigned,
            is_null,
            val: if decode {
                decoder.DecodeInput(value)
            } else {
                value.to_vec()
            },
        };
        pos = next;
    }
    Ok(())
}

/// 解析 length-encoded integer，返回 (值, 是否 NULL, 消耗字节)。
fn parse_length_encoded_int(input: &[u8]) -> Result<(u64, bool, usize), ParamError> {
    let first = *input.first().ok_or(ParamError::MalformedPacket)?;
    match first {
        0..=250 => Ok((u64::from(first), false, 1)),
        251 => Ok((0, true, 1)),
        252 => read_le(input, 2).map(|value| (value, false, 3)),
        253 => read_le(input, 3).map(|value| (value, false, 4)),
        254 => read_le(input, 8).map(|value| (value, false, 9)),
        _ => Err(ParamError::MalformedPacket),
    }
}

/// 跳过首字节后按小端读取 `length` 字节为 u64。
fn read_le(input: &[u8], length: usize) -> Result<u64, ParamError> {
    let bytes = input.get(1..=length).ok_or(ParamError::MalformedPacket)?;
    Ok(bytes.iter().enumerate().fold(0, |value, (index, byte)| {
        value | (u64::from(*byte) << (index * 8))
    }))
}
