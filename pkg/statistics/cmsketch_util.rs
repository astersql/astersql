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

// CMSketch/TopN 相关的 Datum 解码缓存工具。
//
// 将 TopN 中的编码字节解码为 `Datum`（列/表达式运行时的通用值包装）时，
// 用哈希表按编码键缓存结果，避免同一 TopN 值被重复解码。

use std::collections::HashMap;

use crate::TopNMeta;

/// 按编码字节缓存已解码的 `Datum`，加速 TopN 查询路径上的重复解码。
pub struct DatumMapCache {
    datumMap: HashMap<Vec<u8>, types::Datum>,
}

/// 创建空的 Datum 映射缓存。
pub fn NewDatumMapCache() -> DatumMapCache {
    DatumMapCache {
        datumMap: HashMap::new(),
    }
}

impl DatumMapCache {
    /// 按编码键查找已缓存的 Datum；未命中返回 `None`。
    pub fn Get(&self, key: &[u8]) -> Option<types::Datum> {
        self.datumMap.get(key).cloned()
    }

    /// 将 TopNMeta 解码为 Datum 后写入缓存，并返回该 Datum。
    ///
    /// `is_index` 为真时直接把编码字节当作字节 Datum（索引键本身即编码形式）；
    /// 否则按列字段类型解码时间、浮点或通用编码。
    pub fn Put(
        &mut self,
        value: &TopNMeta,
        encoded_value: Vec<u8>,
        field_type: u8,
        is_index: bool,
        location: chrono_tz::Tz,
    ) -> Result<types::Datum, astersql_errors::SharedError> {
        let datum = topNMetaToDatum(value, field_type, is_index, location)?;
        self.datumMap.insert(encoded_value, datum.clone());
        Ok(datum)
    }
}

/// 把 TopNMeta 的编码载荷解码为 Datum。
///
/// 索引路径直接包装字节；列路径按 MySQL 字段类型选择时间/浮点/通用解码器。
fn topNMetaToDatum(
    value: &TopNMeta,
    field_type: u8,
    is_index: bool,
    location: chrono_tz::Tz,
) -> Result<types::Datum, astersql_errors::SharedError> {
    // 索引 TopN 存的是编码键本身，无需再按列类型解码。
    if is_index {
        return Ok(types::NewBytesDatum(value.Encoded.clone()));
    }
    let (_, datum) = if types_field::IsTypeTime(field_type) {
        codec::DecodeAsDateTime(&value.Encoded, field_type, location)?
    } else if field_type == types::mysql::TypeFloat {
        codec::DecodeAsFloat32(&value.Encoded, field_type)?
    } else {
        codec::DecodeOne(&value.Encoded)?
    };
    Ok(datum)
}
