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

// DDL（数据定义语言，如 CREATE/ALTER TABLE 等修改表结构的操作）模块的基准测试夹具。
//
// Go 文件通过真实 cop context 和 tables.Index 压测索引回填热点。Rust 测试环境尚未
// 提供完整 SQL mock store，因此这里保留相同的表/索引/列值输入，并直接调用本 crate
// 已迁移的生产入口，避免用测试内的简化算法掩盖生产实现缺陷。

use crate::index_cop::{CopError, Datum, ScanRow, extract_datum_by_offsets};
use crate::split_region::encode_index_key;

/// 基准夹具：重复提取列值，验证输出缓冲区容量稳定（无重复分配）。
#[test]
fn benchmark_extract_datum_by_offsets_fixture() {
    // 对齐 Go fixture：构造 8 行、每行两列且列值相同，并提取索引列 b。
    let rows = (0..8)
        .map(|value| ScanRow {
            key: vec![value as u8],
            columns: vec![Datum::Int(value), Datum::Int(value)],
        })
        .collect::<Vec<_>>();
    let offsets = [1];
    let mut output = Vec::with_capacity(offsets.len());
    let mut extracted = Vec::new();
    // 循环一万次模拟基准压力，每次都复用同一个输出缓冲区。
    for _ in 0..10_000 {
        extracted = extract_datum_by_offsets(&rows[0], &offsets, &mut output)
            .expect("the index-column offset from metadata must be valid");
    }
    assert_eq!(vec![Datum::Int(0)], extracted);
    assert_eq!(extracted, output);
    // 容量仍等于初始预分配值，说明循环中没有触发扩容。
    assert_eq!(offsets.len(), output.capacity());

    // Rust 生产入口显式报告损坏的元数据偏移，测试不得以切片 panic 替代该契约。
    assert_eq!(
        Err(CopError::ColumnOffset(2)),
        extract_datum_by_offsets(&rows[0], &[2], &mut output),
    );
}

/// 基准夹具：重复生成索引键，验证正式编码入口的表/索引前缀和列值布局。
#[test]
fn benchmark_generate_index_kv_fixture() {
    let values = ["10".to_owned()];
    let mut key = Vec::new();
    for _ in 0..10_000 {
        key = encode_index_key(42, 7, &values);
    }

    let mut expected = vec![b't'];
    expected.extend_from_slice(&42_i64.to_be_bytes());
    expected.extend_from_slice(b"_i");
    expected.extend_from_slice(&7_i64.to_be_bytes());
    expected.extend_from_slice(b"10\0");
    assert_eq!(expected, key);
}
