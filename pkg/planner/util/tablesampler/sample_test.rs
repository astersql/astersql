// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// `TableSampleInfo` / `NewTableSampleInfo` 的单元测试。
//
// 核对：缺少 AST 时返回 None；有 AST 时克隆采样方法与 Schema，且 MemoryUsage 下界合理。

use std::sync::Arc;

use expression::{Column, NewSchema};
use parser_ast::{SampleMethodType, TableSample};
use table::PartitionedTable;

use super::*;

/// AST 为 None 时不得构造采样信息。
#[test]
fn new_table_sample_info_requires_an_ast_node() {
    let schema = NewSchema(Vec::new());
    assert!(NewTableSampleInfo(None, &schema, Vec::new()).is_none());
}

/// 有 AST 时克隆 SampleMethod 与 Schema，MemoryUsage 至少覆盖 TableSample 尺寸。
#[test]
fn new_table_sample_info_clones_ast_and_schema() {
    let node = TableSample {
        SampleMethod: SampleMethodType::System,
        ..TableSample::default()
    };
    let schema = NewSchema(Vec::new());
    let info = NewTableSampleInfo(Some(&node), &schema, Vec::new()).unwrap();

    assert_eq!(
        info.AstNode.as_ref().unwrap().SampleMethod,
        SampleMethodType::System
    );
    assert_eq!(info.FullSchema.as_ref().unwrap().Len(), schema.Len());
    let expected = size::SizeOfPointer * 2
        + size::SizeOfSlice
        + std::mem::size_of::<TableSample>() as i64
        + schema.MemoryUsage();
    assert_eq!(info.MemoryUsage(), expected);
}

/// 构造后修改原 Schema 不得影响保存的副本，与 Go 的 `fullSchema.Clone()` 一致。
#[test]
fn new_table_sample_info_owns_an_independent_schema_clone() {
    let node = TableSample::default();
    let mut schema = NewSchema(Vec::new());
    let info = NewTableSampleInfo(Some(&node), &schema, Vec::new()).unwrap();

    schema.Columns.push(Column::default());

    assert_eq!(schema.Len(), 1);
    assert_eq!(info.FullSchema.as_ref().unwrap().Len(), 0);
}

/// MemoryUsage must not charge optional AST/schema storage when either field is absent.
#[test]
fn memory_usage_omits_absent_optional_fields() {
    let info = TableSampleInfo {
        AstNode: None,
        FullSchema: None,
        Partitions: Vec::new(),
    };

    assert_eq!(
        info.MemoryUsage(),
        size::SizeOfPointer * 2 + size::SizeOfSlice
    );
}

/// Go 按切片容量（不是长度）计费，Rust 的 Vec 也必须保持这一契约。
#[test]
fn memory_usage_charges_partition_capacity() {
    let partition_capacity = 3;
    let partitions: Vec<Arc<dyn PartitionedTable>> = Vec::with_capacity(partition_capacity);
    let info = TableSampleInfo {
        AstNode: None,
        FullSchema: None,
        Partitions: partitions,
    };

    assert_eq!(
        info.MemoryUsage(),
        size::SizeOfPointer * 2
            + size::SizeOfSlice
            + partition_capacity as i64 * size::SizeOfInterface
    );
}
