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

use astersql_parser_types::{FieldType, mysql::*, types::*};
use astersql_util_chunk::Column;

/// Mark changed update ordinals, preserving existing bits. SQL collation is deliberately
/// ignored: strings and ENUM/SET names use the binary row-update contract.
/// Row indices and bitmap layout are validated by the caller, as in Go.
pub fn MarkTouchedRowsByColumn(
    update_rows: &[usize],
    bitmap: &mut [u8],
    stride: usize,
    bit_pos: usize,
    old: &Column,
    new: &Column,
    field: Option<&FieldType>,
    not_null: bool,
    context: &str,
) -> Result<(), String> {
    let field = field.ok_or_else(|| format!("field type is nil when comparing {context}"))?;
    if update_rows.is_empty() {
        return Ok(());
    }
    let mark = |equal: &dyn Fn(usize) -> bool, bitmap: &mut [u8]| {
        for (ordinal, &row) in update_rows.iter().enumerate() {
            let changed = if !not_null && (old.IsNull(row) || new.IsNull(row)) {
                old.IsNull(row) != new.IsNull(row)
            } else {
                !equal(row)
            };
            if changed {
                let offset = if stride == 1 {
                    ordinal
                } else {
                    ordinal * stride + (bit_pos >> 3)
                };
                bitmap[offset] |= 1 << (bit_pos & 7);
            }
        }
    };
    match field.EvalType() {
        ETInt if field.GetType() == TypeBit && (!old.IsFixed() || !new.IsFixed()) => {
            mark(&|r| old.GetRaw(r) == new.GetRaw(r), bitmap)
        }
        ETInt if HasUnsignedFlag(field.GetFlag()) => {
            mark(&|r| old.GetUint64(r) == new.GetUint64(r), bitmap)
        }
        ETInt => mark(&|r| old.GetInt64(r) == new.GetInt64(r), bitmap),
        ETReal if field.GetType() == TypeFloat => {
            mark(&|r| old.GetFloat32(r) == new.GetFloat32(r), bitmap)
        }
        ETReal => mark(&|r| old.GetFloat64(r) == new.GetFloat64(r), bitmap),
        ETDecimal => mark(
            &|r| old.GetDecimal(r).Compare(&new.GetDecimal(r)) == 0,
            bitmap,
        ),
        ETString if matches!(field.GetType(), TypeEnum | TypeSet) =>
        // Names follow the eight-byte numeric payload. Compare original bytes,
        // without decoding, folding case, or comparing numeric identities.
        {
            mark(
                &|r| {
                    old.GetRaw(r).get(8..).unwrap_or_default()
                        == new.GetRaw(r).get(8..).unwrap_or_default()
                },
                bitmap,
            )
        }
        ETString | ETJson | ETVectorFloat32 => mark(&|r| old.GetRaw(r) == new.GetRaw(r), bitmap),
        ETDatetime | ETTimestamp => mark(&|r| old.GetTime(r) == new.GetTime(r), bitmap),
        ETDuration => mark(&|r| old.GetInt64(r) == new.GetInt64(r), bitmap),
        other => {
            return Err(format!(
                "unsupported eval type {} in {context} comparison",
                other.0
            ));
        }
    }
    Ok(())
}
