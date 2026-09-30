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

// Column TopN keys use the flattened storage representation. Restore the
// column datum kind before comparing against typed histogram bounds.

use crate::TopNMeta;

/// Decode a TopN key as an index byte string or a typed column datum.
pub fn topNMetaToDatum(
    value: &TopNMeta,
    field_type: &types::FieldType,
    is_index: bool,
    location: chrono_tz::Tz,
) -> Result<types::Datum, astersql_errors::SharedError> {
    if is_index {
        return Ok(types::NewBytesDatum(value.Encoded.clone()));
    }
    let (_, decoded) = codec::DecodeOne(&value.Encoded)?;
    tablecodec::Unflatten(decoded, Box::new(field_type.clone()), Some(location))
}

/// Decode a column TopN key while preserving raw comparison bytes for strings.
pub fn DecodeColumnTopNValue(
    encoded: &[u8],
    field_type: &types::FieldType,
    location: chrono_tz::Tz,
) -> Result<types::Datum, astersql_errors::SharedError> {
    let (_, decoded) = codec::DecodeOne(encoded)?;
    if types_field::IsString(field_type.GetType()) {
        return Ok(decoded);
    }
    tablecodec::Unflatten(decoded, Box::new(field_type.clone()), Some(location))
}
