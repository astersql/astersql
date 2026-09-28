// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// DDL（数据定义语言）列变更相关的单元测试。
//
// 本模块验证以下三类列变更逻辑：
// - 新增列：新列在各个 schema 状态（DeleteOnly/WriteOnly/WriteReorganization/Public）
//   下默认值应保持不变；这些状态源自在线 DDL（Online Schema Change，
//   即不停机修改表结构）的多阶段状态机。
// - 修改 AUTO_RANDOM 列：AUTO_RANDOM 是一种在主键高位注入随机分片位
//   （shard bits）以打散写入热点的机制，测试其分片位数变更的合法性校验。
// - 修改分区列：分区表（把一张逻辑表按规则拆分成多个物理分区）的
//   分区键列不允许重命名等破坏分区表达式的修改。

use crate::add_column::{ColumnDefinition, advance_add_column, start_add_column};
use crate::column::{
    ColumnInfo, ColumnPosition, DefaultValue, FieldType, SchemaState, TableInfo,
    init_and_add_column_to_table, remove_column_and_single_indices,
};
use crate::modify_column::{
    ModifyColumnContext, ModifyColumnError, PartitionInfo, PartitionType, check_auto_random,
    check_partition_column_modifiable,
};

/// 构造一个指定名称的整数类型列信息，作为测试用的辅助函数。
fn integer(name: &str) -> ColumnInfo {
    ColumnInfo::new(name, FieldType::integer())
}

/// 测试新增列：带默认值的新列在在线 DDL 各阶段状态下默认值保持稳定，
/// 且删除该列后表中剩余列不受影响。
#[test]
fn test_column_add() {
    // 建表并先加入两个普通整数列 c1、c2。
    let mut table = TableInfo::new(1, "t");
    init_and_add_column_to_table(&mut table, integer("c1"));
    init_and_add_column_to_table(&mut table, integer("c2"));
    for column in &mut table.columns {
        column.state = SchemaState::Public;
    }
    // 通过生产加列入口新增 c3，而不是在测试中手工构造状态。
    let added = ColumnDefinition {
        name: "c3".into(),
        field_type: FieldType::integer(),
        constraints: Vec::new(),
        default_value: Some(DefaultValue::Integer(3)),
        comment: String::new(),
        generated: None,
    };
    let position = ColumnPosition::None;
    let id = start_add_column(&mut table, &added, &position, 512, false, false).unwrap();
    // 遍历在线 DDL 状态机的各个阶段，验证默认值在状态迁移中不丢失：
    // DeleteOnly（只处理删除）-> WriteOnly（可写不可读）->
    // WriteReorganization（写入并回填历史数据）-> Public（对外可见）。
    let mut schema_version = 0;
    for (step, state) in [
        SchemaState::DeleteOnly,
        SchemaState::WriteOnly,
        SchemaState::WriteReorganization,
        SchemaState::Public,
    ]
    .into_iter()
    .enumerate()
    {
        let outcome =
            advance_add_column(&mut table, id, &position, &mut schema_version, false).unwrap();
        assert_eq!(state, outcome.schema_state);
        assert_eq!((step + 1) as i64, outcome.schema_version);
        assert_eq!(state == SchemaState::Public, outcome.finished);
        assert_eq!(
            Some(DefaultValue::Integer(3)),
            table
                .columns
                .iter()
                .find(|column| column.id == id)
                .unwrap()
                .default_value
        );
    }
    // 移除新列后，表中应只剩最初的 c1、c2 两列。
    assert!(
        remove_column_and_single_indices(&mut table, id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        vec!["c1", "c2"],
        table
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>()
    );

    // Go 用例还覆盖未显式指定 DEFAULT 的加列；Rust 元数据必须保持 None。
    let no_default = ColumnDefinition {
        name: "c3".into(),
        field_type: FieldType::integer(),
        constraints: Vec::new(),
        default_value: None,
        comment: String::new(),
        generated: None,
    };
    let id = start_add_column(&mut table, &no_default, &position, 512, false, false).unwrap();
    for _ in 0..4 {
        advance_add_column(&mut table, id, &position, &mut schema_version, false).unwrap();
    }
    let column = table.columns.iter().find(|column| column.id == id).unwrap();
    assert_eq!(SchemaState::Public, column.state);
    assert_eq!(None, column.default_value);
}

/// 测试修改 AUTO_RANDOM 列时的分片位（shard bits）校验：
/// 分片位只能增大不能减小，且不能超过上限。
#[test]
fn test_modify_auto_rand_column_with_meta_key_changed() {
    let old = integer("a");
    let new = integer("a");
    // 构造修改列的上下文：严格 SQL 模式、分片位上限 15、范围位默认 64。
    let context = ModifyColumnContext {
        strict_sql_mode: true,
        partition: None,
        disable_lossy_optimization: false,
        auto_random_range_bits_default: 64,
        auto_random_shard_bits_max: 15,
    };
    // 分片位从 5 增大到 10：允许，返回新的分片位数。
    assert_eq!(
        Ok(10),
        check_auto_random(5, 64, 10, 64, &old, &new, false, &context)
    );
    // 分片位从 10 缩小到 5：会导致已有主键冲突风险，应报错。
    assert!(check_auto_random(10, 64, 5, 64, &old, &new, false, &context).is_err());
    // 分片位 16 超过上限 15：应报错。
    assert!(check_auto_random(5, 64, 16, 64, &old, &new, false, &context).is_err());
}

/// 回归测试 issue #40135：分区键列被重命名（a -> anew）时，
/// 即使只是拓宽字段长度也必须被拒绝，否则分区表达式将失效。
#[test]
fn test_issue40135() {
    // 旧列 a，显示宽度（flen）为 8。
    let mut old = integer("a");
    old.field_type.flen = 8;
    // 新列改名为 anew 且宽度拓宽到 16。
    let mut wider = integer("anew");
    wider.field_type.flen = 16;
    // Hash 分区表，分区表达式引用列 a。
    let partition = PartitionInfo {
        partition_type: PartitionType::Hash,
        columns: Vec::new(),
        expression: "a".into(),
    };
    // 期望返回“禁止重命名分区列”错误。
    assert_eq!(
        Err(ModifyColumnError::PartitionColumnRename),
        check_partition_column_modifiable(&partition, &old, &wider, &[], &[])
    );
}
