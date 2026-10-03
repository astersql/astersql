// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::{BinaryFormat, Config, FieldKind};
pub(crate) fn append_field(dst: &mut Vec<u8>, val: Option<&[u8]>, kind: FieldKind, cfg: &Config) {
    let Some(val) = val else {
        dst.extend_from_slice(&cfg.null_value);
        return;
    };
    if matches!(kind, FieldKind::Number) {
        dst.extend_from_slice(val);
        return;
    }
    dst.extend_from_slice(&cfg.fields_enclosed_by);
    match (kind, cfg.binary_format) {
        (FieldKind::Bytes, BinaryFormat::HEX) => {
            const HEX: &[u8] = b"0123456789abcdef";
            for &b in val {
                dst.extend_from_slice(&[HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]);
            }
        }
        (FieldKind::Bytes, BinaryFormat::Base64) => {
            dst.extend_from_slice(base64_encode(val).as_bytes())
        }
        _ => append_escaped(dst, val, cfg),
    }
    dst.extend_from_slice(&cfg.fields_enclosed_by);
}
fn append_escaped(dst: &mut Vec<u8>, val: &[u8], cfg: &Config) {
    if let Some(&esc) = cfg.fields_escaped_by.first() {
        let spec = cfg
            .fields_enclosed_by
            .first()
            .or(cfg.fields_terminated_by.first())
            .copied()
            .unwrap_or(0);
        for &b in val {
            let replacement = match b {
                0 => Some(b'0'),
                b'\r' => Some(b'r'),
                b'\n' => Some(b'n'),
                _ if b == esc || b == spec => Some(b),
                _ => None,
            };
            if let Some(b) = replacement {
                dst.extend_from_slice(&[esc, b]);
            } else {
                dst.push(b);
            }
        }
    } else if !cfg.fields_enclosed_by.is_empty() {
        let enclosure = &cfg.fields_enclosed_by;
        let mut i = 0;
        while i < val.len() {
            if val[i..].starts_with(enclosure) {
                dst.extend_from_slice(enclosure);
                dst.extend_from_slice(enclosure);
                i += enclosure.len();
            } else {
                dst.push(val[i]);
                i += 1;
            }
        }
    } else {
        dst.extend_from_slice(val);
    }
}
fn base64_encode(data: &[u8]) -> String {
    // Base64 字母表。
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as u32;
        let b1 = if i + 1 < data.len() {
            data[i + 1] as u32
        } else {
            0
        };
        let b2 = if i + 2 < data.len() {
            data[i + 2] as u32
        } else {
            0
        };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        if i + 1 < data.len() {
            out.push(T[((n >> 6) & 63) as usize] as char);
        } else {
            // Base64 padding。
            out.push('=');
        }
        if i + 2 < data.len() {
            out.push(T[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
        // 每 3 字节一组编码。
        i += 3;
    }
    out
}
