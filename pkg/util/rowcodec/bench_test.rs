// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// rowcodec 编解码与 checksum 微基准入口。
//
// 提供 Checksum / Encode / EncodeFromOldRow / Decode 的循环基准函数，以及
// 日常冒烟测试 `TestBenchDaily`（各跑 1 次，确认路径可执行）。

#![allow(non_snake_case)]

use super::*;

/// 构造指定 MySQL 类型码的 FieldType。
fn field_type(tp: u8) -> types::FieldType {
    *types::NewFieldType(tp)
}

/// 对固定三列样本反复计算行 checksum。
pub fn BenchmarkChecksum(iterations: usize) {
    let datums = [
        types::NewIntDatum(1),
        types::NewStringDatum("abc".to_owned()),
        types::NewFloat64Datum(1.1),
    ];
    let columns = [
        model::ColumnInfo {
            ID: 1,
            FieldType: field_type(mysql::TypeLong),
            ..Default::default()
        },
        model::ColumnInfo {
            ID: 2,
            FieldType: field_type(mysql::TypeVarchar),
            ..Default::default()
        },
        model::ColumnInfo {
            ID: 3,
            FieldType: field_type(mysql::TypeDouble),
            ..Default::default()
        },
    ];
    let cols = (0..3)
        .map(|index| rowcodec::ColData {
            ColumnInfo: &columns[index],
            Datum: &datums[index],
        })
        .collect();
    let mut row = rowcodec::RowData {
        Cols: cols,
        Data: Vec::new(),
    };
    for _ in 0..iterations {
        row.Checksum(Some(&time::UTC))
            .expect("checksum should succeed");
    }
}

/// 对固定三列 Datum 反复做新行格式编码。
pub fn BenchmarkEncode(iterations: usize) {
    let datums = vec![
        types::NewIntDatum(1),
        types::NewStringDatum("abc".to_owned()),
        types::NewFloat64Datum(1.1),
    ];
    // Go uses Encoder's zero value in this benchmark (`Enable == false`).
    let mut encoder = rowcodec::Encoder::new(false);
    let mut buf = Vec::new();
    for _ in 0..iterations {
        buf.clear();
        buf = encoder
            .Encode(None, vec![1, 2, 3], datums.clone(), None, buf)
            .expect("encode should succeed");
    }
}

/// 从旧行格式（old row）转换并反复编码为新格式。
pub fn BenchmarkEncodeFromOldRow(iterations: usize) {
    let datums = vec![
        types::NewIntDatum(1),
        types::NewStringDatum("abc".to_owned()),
        types::NewFloat64Datum(1.1),
    ];
    let old_row = tablecodec::EncodeOldRow(
        codec::NewEncoder(collate::NewCollationEnabled()),
        None,
        datums,
        vec![1, 2, 3],
        Vec::new(),
        None,
    )
    .expect("old row encoding should succeed");
    // Go uses Encoder's zero value in this benchmark (`Enable == false`).
    let mut encoder = rowcodec::Encoder::new(false);
    let mut buf = Vec::new();
    for _ in 0..iterations {
        buf = encode_from_old_row(&mut encoder, None, &old_row, buf)
            .expect("old row conversion should succeed");
    }
}

/// 对预编码行反复解码写入 Chunk。
pub fn BenchmarkDecode(iterations: usize) {
    let datums = vec![
        types::NewIntDatum(1),
        types::NewStringDatum("abc".to_owned()),
        types::NewFloat64Datum(1.1),
    ];
    let field_types = vec![
        field_type(mysql::TypeLonglong),
        field_type(mysql::TypeString),
        field_type(mysql::TypeDouble),
    ];
    // Go uses Encoder's zero value in this benchmark (`Enable == false`).
    let mut encoder = rowcodec::Encoder::new(false);
    let encoded = encoder
        .Encode(None, vec![-1, 2, 3], datums, None, Vec::new())
        .expect("encode should succeed");
    let columns = field_types
        .iter()
        .enumerate()
        .map(|(index, field_type)| rowcodec::ColInfo {
            ID: [-1, 2, 3][index],
            Ft: field_type.clone(),
            // Go leaves this field at its zero value; handle fallback is selected by
            // `handle_col_ids`, not by `IsPKHandle`.
            IsPKHandle: false,
            VirtualGenCol: false,
        })
        .collect();
    let mut decoder = rowcodec::NewChunkDecoder(columns, vec![-1], None, Some(time::UTC));
    let mut chunk = chunk::NewChunkWithCapacity(field_types, 1);
    let handle = kv::IntHandle(1);
    for _ in 0..iterations {
        chunk.Reset();
        decoder
            .DecodeToChunk(&encoded, 0, Some(&handle), &mut chunk)
            .expect("decode should succeed");
    }
}

/// 日常冒烟：各基准路径执行 1 次，确保可跑通。
#[test]
fn TestBenchDaily() {
    BenchmarkEncode(1);
    BenchmarkDecode(1);
    BenchmarkEncodeFromOldRow(1);
}
