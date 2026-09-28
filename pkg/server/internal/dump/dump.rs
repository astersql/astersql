// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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
// Copyright 2013 The Go-MySQL-Driver Authors. All rights reserved.

// MySQL 线协议基础 dump 原语。
//
// 提供长度编码整数/字符串、小端定长整数，以及 TIME / DATETIME / DATE
// 的二进制结果集编码，供 column 等上层模块拼接协议包。

#![allow(non_snake_case)]

// LengthEncodedString 先写长度前缀，再追加原始字节。
/// 先写长度编码整数，再追加原始字节，构成长度编码字符串。
pub fn LengthEncodedString(mut buffer: Vec<u8>, bytes: &[u8]) -> Vec<u8> {
    buffer = LengthEncodedInt(buffer, bytes.len() as u64);
    buffer.extend_from_slice(bytes);
    buffer
}

// LengthEncodedInt 按 MySQL 协议选择 1/3/4/9 字节整数编码。
/// 按 MySQL 长度编码整数规则写出：≤250 单字节，否则 0xfc/0xfd/0xfe 前缀。
pub fn LengthEncodedInt(mut buffer: Vec<u8>, n: u64) -> Vec<u8> {
    match n {
        0..=250 => buffer.push(n as u8),
        251..=0xffff => {
            buffer.push(0xfc);
            buffer.extend_from_slice(&(n as u16).to_le_bytes());
        }
        0x10000..=0xffffff => {
            buffer.push(0xfd);
            buffer.extend_from_slice(&(n as u32).to_le_bytes()[..3]);
        }
        _ => {
            buffer.push(0xfe);
            buffer.extend_from_slice(&n.to_le_bytes());
        }
    }
    buffer
}

/// 以小端序追加 u16。
pub fn Uint16(mut buffer: Vec<u8>, n: u16) -> Vec<u8> {
    buffer.extend_from_slice(&n.to_le_bytes());
    buffer
}

/// 以小端序追加 u32。
pub fn Uint32(mut buffer: Vec<u8>, n: u32) -> Vec<u8> {
    buffer.extend_from_slice(&n.to_le_bytes());
    buffer
}

/// 以小端序追加 u64。
pub fn Uint64(mut buffer: Vec<u8>, n: u64) -> Vec<u8> {
    buffer.extend_from_slice(&n.to_le_bytes());
    buffer
}

// BinaryTime 对应 Go 的 duration 编码；微秒和天数布局必须保持协议顺序。
/// 将 duration 编码为二进制 TIME：零值为单字节 0；否则含符号、天、时分秒与可选微秒。
pub fn BinaryTime(dur: time::Duration) -> Vec<u8> {
    if dur.is_zero() {
        return vec![0];
    }

    const NANOS_PER_MICROSECOND: i64 = 1_000;
    const NANOS_PER_SECOND: i64 = 1_000_000_000;
    const NANOS_PER_MINUTE: i64 = 60 * NANOS_PER_SECOND;
    const NANOS_PER_HOUR: i64 = 60 * NANOS_PER_MINUTE;
    const NANOS_PER_DAY: i64 = 24 * NANOS_PER_HOUR;

    // Go time.Duration 的底层是 int64；MinInt64 取负会回绕并仍保持负数。
    let mut remaining = dur.whole_nanoseconds() as i64;
    let negative = remaining < 0;
    if negative {
        remaining = remaining.wrapping_neg();
    }
    let days = remaining / NANOS_PER_DAY;
    remaining -= days * NANOS_PER_DAY;
    let hours = remaining / NANOS_PER_HOUR;
    remaining -= hours * NANOS_PER_HOUR;
    let minutes = remaining / NANOS_PER_MINUTE;
    remaining -= minutes * NANOS_PER_MINUTE;
    let seconds = remaining / NANOS_PER_SECOND;
    remaining -= seconds * NANOS_PER_SECOND;

    let mut data = vec![0; 13];
    data[0] = 12;
    data[1] = u8::from(negative);
    data[2] = days as u8;
    data[6] = hours as u8;
    data[7] = minutes as u8;
    data[8] = seconds as u8;
    // 无小数秒时长度改为 8，截断微秒字段。
    if remaining == 0 {
        data[0] = 8;
        data.truncate(9);
        return data;
    }
    data[9..13].copy_from_slice(&((remaining / NANOS_PER_MICROSECOND) as u32).to_le_bytes());
    data
}

// BinaryDateTime 保留零时间、日期、时分秒和微秒四种 Go 分支。
/// 按类型写出 DATETIME/TIMESTAMP/DATE：零值、仅日期、到秒、带微秒四种长度形态。
pub fn BinaryDateTime(mut data: Vec<u8>, t: types::Time) -> Vec<u8> {
    let year = t.Year();
    let month = t.Month();
    let day = t.Day();
    match t.Type() {
        types::mysql::TypeTimestamp | types::mysql::TypeDatetime => {
            if t.IsZero() {
                data.push(0);
            } else if t.Microsecond() != 0 {
                data.push(11);
                data = Uint16(data, year as u16);
                data.extend_from_slice(&[
                    month as u8,
                    day as u8,
                    t.Hour() as u8,
                    t.Minute() as u8,
                    t.Second() as u8,
                ]);
                data = Uint32(data, t.Microsecond() as u32);
            } else if t.Hour() != 0 || t.Minute() != 0 || t.Second() != 0 {
                data.push(7);
                data = Uint16(data, year as u16);
                data.extend_from_slice(&[
                    month as u8,
                    day as u8,
                    t.Hour() as u8,
                    t.Minute() as u8,
                    t.Second() as u8,
                ]);
            } else {
                data.push(4);
                data = Uint16(data, year as u16);
                data.extend_from_slice(&[month as u8, day as u8]);
            }
        }
        types::mysql::TypeDate => {
            if t.IsZero() {
                data.push(0);
            } else {
                data.push(4);
                data = Uint16(data, year as u16);
                data.extend_from_slice(&[month as u8, day as u8]);
            }
        }
        _ => {}
    }
    data
}
