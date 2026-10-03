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

use astersql_dumpformat::FieldKind;

pub fn append_value(
    dst: &mut Vec<u8>,
    val: &[u8],
    is_null: bool,
    kind: FieldKind,
    escape_backslash: bool,
) {
    if is_null {
        dst.extend_from_slice(b"NULL");
        return;
    }
    match kind {
        FieldKind::Number => dst.extend_from_slice(val),
        FieldKind::Bytes => {
            const HEX: &[u8] = b"0123456789abcdef";
            dst.extend_from_slice(b"x'");
            for &b in val {
                dst.push(HEX[(b >> 4) as usize]);
                dst.push(HEX[(b & 15) as usize]);
            }
            dst.push(b'\'');
        }
        FieldKind::String => {
            dst.push(b'\'');
            for &b in val {
                if escape_backslash {
                    let escaped = match b {
                        0 => Some(b'0'),
                        b'\n' => Some(b'n'),
                        b'\r' => Some(b'r'),
                        b'\\' => Some(b'\\'),
                        b'\'' => Some(b'\''),
                        b'"' => Some(b'"'),
                        26 => Some(b'Z'),
                        _ => None,
                    };
                    if let Some(e) = escaped {
                        dst.extend_from_slice(&[b'\\', e]);
                    } else {
                        dst.push(b);
                    }
                } else {
                    dst.push(b);
                    if b == b'\'' {
                        dst.push(b);
                    }
                }
            }
            dst.push(b'\'');
        }
    }
}
