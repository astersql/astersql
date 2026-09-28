// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 列映射迁移补充单元测试。
//
// 相对 Go 原测试，额外覆盖：完整 selector pattern、表级优先于 schema 级、
// 多种整数形态的 partition ID，以及 Update/Remove/校验与 Adjust 语义。

use crate::PARTITION_RULE_TEST_LOCK;
use crate::{
    AddPrefix, AddSuffix, Expr, Mapping, NewMapping, PartitionID, Rule, SetPartitionRule, Value,
};

/// 构造一条测试用 `Rule`，省略空的 SourceColumn / CreateTableQuery。
fn rule(schema: &str, table: &str, target: &str, expression: Expr, arguments: &[&str]) -> Rule {
    Rule {
        PatternSchema: schema.to_owned(),
        PatternTable: table.to_owned(),
        SourceColumn: String::new(),
        TargetColumn: target.to_owned(),
        Expression: expression,
        Arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
        CreateTableQuery: String::new(),
    }
}

/// 对单列 `id` 调用 `HandleRowValue`，便于断言映射结果。
fn map_id(
    mapping: &Mapping,
    schema: &str,
    table: &str,
    value: Value,
) -> (Vec<Value>, Option<Vec<isize>>) {
    mapping
        .HandleRowValue(schema, table, &["id".to_owned()], vec![value])
        .expect("mapping should succeed")
}

/// 验证 schema/table 完整 pattern 匹配，以及表级规则优先于 schema 级。
#[test]
fn migration_uses_full_selector_patterns_and_table_priority() {
    let mapping = NewMapping(
        false,
        vec![
            rule("tenant?", "", "id", AddSuffix(), &["-schema"]),
            rule("tenant?", "orders_[0-9]", "id", AddPrefix(), &["table-"]),
        ],
    )
    .unwrap();

    let (values, positions) = map_id(&mapping, "TENANTA", "ORDERS_7", Value::String("9".into()));
    assert_eq!(values, vec![Value::String("table-9".into())]);
    assert_eq!(positions, Some(vec![-1, 0]));

    let (values, _) = map_id(&mapping, "tenantb", "archive", Value::String("9".into()));
    assert_eq!(values, vec![Value::String("9-schema".into())]);
}

/// 验证 partition ID 接受 Go 侧各类整数形态，并拒绝负数 / 溢出。
#[test]
fn migration_partition_id_accepts_all_go_integer_shapes() {
    let _guard = PARTITION_RULE_TEST_LOCK.lock().unwrap();
    SetPartitionRule(4, 7, 8);
    let mapping = NewMapping(
        true,
        vec![rule(
            "test*",
            "t*",
            "id",
            PartitionID(),
            &["2", "test", "t", "_"],
        )],
    )
    .unwrap();
    let high_bits = (2_i64 << 59) | (1_i64 << 52) | (3_i64 << 44);

    for value in [
        Value::Int(7),
        Value::Int8(7),
        Value::Int32(7),
        Value::Uint(7),
        Value::Uint16(7),
        Value::Uint32(7),
        Value::Uint64(7),
    ] {
        let (values, _) = map_id(&mapping, "test_1", "t_3", value);
        assert_eq!(values, vec![Value::Int(high_bits | 7)]);
    }

    let (values, _) = map_id(&mapping, "test_1", "t_3", Value::String("7".into()));
    assert_eq!(values, vec![Value::String((high_bits | 7).to_string())]);
    assert!(map_id_result(&mapping, Value::Int(-1)).is_err());
    assert!(map_id_result(&mapping, Value::Uint64(u64::MAX)).is_err());
}

/// 包装 `HandleRowValue`，便于断言错误路径。
fn map_id_result(
    mapping: &Mapping,
    value: Value,
) -> Result<(Vec<Value>, Option<Vec<isize>>), String> {
    mapping.HandleRowValue("test_1", "t_3", &["id".to_owned()], vec![value])
}

/// 验证 UpdateRule / RemoveRule 与缓存失效后的 selector 语义。
#[test]
fn migration_update_remove_and_cache_follow_go_selector_semantics() {
    let mapping = NewMapping(false, vec![rule("db*", "t*", "id", AddPrefix(), &["old-"])]).unwrap();
    assert_eq!(
        map_id(&mapping, "DB1", "T1", Value::String("1".into())).0,
        vec![Value::String("old-1".into())]
    );

    let updated = rule("db*", "t*", "id", AddSuffix(), &["-new"]);
    mapping.UpdateRule(Some(updated.clone())).unwrap();
    assert_eq!(
        map_id(&mapping, "db1", "t1", Value::String("1".into())).0,
        vec![Value::String("1-new".into())]
    );

    mapping.RemoveRule(Some(updated)).unwrap();
    let (values, positions) = map_id(&mapping, "db1", "t1", Value::String("1".into()));
    assert_eq!(values, vec![Value::String("1".into())]);
    assert_eq!(positions, None);
}

/// 验证 Valid / Adjust 与未知表达式错误，对齐 Go 规则校验。
#[test]
fn migration_rule_validation_and_adjustment_match_go() {
    let mut partition = rule("db", "t", "id", PartitionID(), &["1", "db", "t"]);
    assert!(partition.Valid().is_ok());
    partition.Adjust();
    assert_eq!(partition.Arguments, vec!["1", "db", "t", ""]);

    assert!(rule("db", "t", "", AddPrefix(), &["x"]).Valid().is_err());
    assert!(rule("db", "t", "id", AddPrefix(), &[]).Valid().is_err());
    assert!(
        rule("db", "t", "id", Expr::Other("unknown".into()), &[])
            .Valid()
            .is_err()
    );
}
