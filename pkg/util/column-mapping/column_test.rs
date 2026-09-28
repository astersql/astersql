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

// `column-mapping` 规则校验、缓存、partition ID 与大小写匹配的单元测试。
//
// 对应 Go `column_test.go`：覆盖 Valid/Adjust、HandleRowValue、查询缓存、
// 位段拼装与 case-sensitive 语义。

// 这段逻辑只描述 column mapping 规则校验、缓存、partition id 位运算和大小写匹配的测试语义。

#![allow(dead_code, non_snake_case)]

use super::*;
use crate::PARTITION_RULE_TEST_LOCK;

/// 覆盖未知表达式、缺失目标列、参数数量错误与合法规则分支。
// TestRule 对应 Go 的 TestRule：依次覆盖未知表达式、缺失目标列、参数数量错误和合法规则。
#[test]
fn TestRule() {
    // Go 使用 &Rule 字面量并逐步修改字段；这里保留同一个可变规则，以便观察 Valid 分支如何变化。
    let mut inValidRule = Rule {
        PatternSchema: "test*".to_string(),
        PatternTable: "abc*".to_string(),
        SourceColumn: "id".to_string(),
        TargetColumn: "id".to_string(),
        Expression: Expr::Other("Error".to_string()),
        Arguments: vec![],
        CreateTableQuery: "xxx".to_string(),
    };
    assert!(inValidRule.Valid().is_err());

    inValidRule.TargetColumn.clear();
    assert!(inValidRule.Valid().is_err());

    inValidRule.Expression = AddPrefix();
    inValidRule.TargetColumn = "id".to_string();
    assert!(inValidRule.Valid().is_err());

    inValidRule.Arguments = vec!["1".to_string()];
    assert!(inValidRule.Valid().is_ok());

    inValidRule.Expression = PartitionID();
    assert!(inValidRule.Valid().is_err());

    inValidRule.Arguments = vec!["1".to_string(), "test_".to_string(), "t_".to_string()];
    assert!(inValidRule.Valid().is_ok());
}

/// 验证加前缀映射、缓存命中、清缓存后错误与 DDL 未实现分支。
// TestHandle 对应 Go 的 TestHandle：初始化大小写不敏感 mapping，验证行值映射、缓存和 DDL 分支。
#[test]
fn TestHandle() {
    let rules = vec![Rule {
        PatternSchema: "Test*".to_string(),
        PatternTable: "xxx*".to_string(),
        SourceColumn: String::new(),
        TargetColumn: "id".to_string(),
        Expression: AddPrefix(),
        Arguments: vec!["instance_id:".to_string()],
        CreateTableQuery: "xx".to_string(),
    }];

    // initial column mapping
    let m = NewMapping(false, rules.clone()).expect("Go require.NoError");
    assert_eq!(0, m.cache.read().expect("cache read lock").len());

    // test add prefix, add suffix is similar
    let (vals, poss) = m
        .HandleRowValue(
            "test",
            "xxx",
            &["age".to_string(), "id".to_string()],
            vec![Value::Int(1), Value::String("1".to_string())],
        )
        .expect("Go require.NoError");
    assert_eq!(
        vec![Value::Int(1), Value::String("instance_id:1".to_string())],
        vals
    );
    assert_eq!(Some(vec![-1, 1]), poss);

    // test cache
    // Go 这里只传入 name 一列，但缓存里的 mappingInfo 仍会把第二个位置作为 id 处理。
    let (vals, poss) = m
        .HandleRowValue(
            "test",
            "xxx",
            &["name".to_string()],
            vec![Value::Int(1), Value::String("1".to_string())],
        )
        .expect("Go require.NoError");
    assert_eq!(
        vec![Value::Int(1), Value::String("instance_id:1".to_string())],
        vals
    );
    assert_eq!(Some(vec![-1, 1]), poss);

    // test resetCache
    // 清缓存后重新按 name 列查询会找不到 target column，因此 Go 断言返回 error。
    m.resetCache();
    assert!(
        m.HandleRowValue(
            "test",
            "xxx",
            &["name".to_string()],
            vec![Value::String("1".to_string())],
        )
        .is_err()
    );

    // test DDL
    assert!(
        m.HandleDDL(
            "test",
            "xxx",
            &["id".to_string(), "age".to_string()],
            "create table xxx".to_string(),
        )
        .is_err()
    );

    let (statement, poss) = m
        .HandleDDL(
            "abc",
            "xxx",
            &["id".to_string(), "age".to_string()],
            "create table xxx".to_string(),
        )
        .expect("Go require.NoError");
    assert_eq!("create table xxx", statement);
    assert!(poss.is_none());
}

/// 验证规则不匹配时 ignore，匹配时预计算 instance/schema/table 三段 ID。
// TestQueryColumnInfo 对应 Go 的 TestQueryColumnInfo：验证规则不匹配时 ignore，匹配时预计算三段 ID。
#[test]
fn TestQueryColumnInfo() {
    let _guard = PARTITION_RULE_TEST_LOCK.lock().unwrap();
    SetPartitionRule(4, 7, 8);
    let rules = vec![Rule {
        PatternSchema: "test*".to_string(),
        PatternTable: "xxx*".to_string(),
        SourceColumn: String::new(),
        TargetColumn: "id".to_string(),
        Expression: PartitionID(),
        Arguments: vec!["8".to_string(), "test_".to_string(), "xxx_".to_string()],
        CreateTableQuery: "xx".to_string(),
    }];

    // initial column mapping
    let m = NewMapping(false, rules.clone()).expect("Go require.NoError");

    // test mismatch
    let info = m
        .queryColumnInfo("test_2", "t_1", &["id".to_string(), "name".to_string()])
        .expect("Go require.NoError");
    assert!(info.ignore);

    // test matched
    let info = m
        .queryColumnInfo("test_2", "xxx_1", &["id".to_string(), "name".to_string()])
        .expect("Go require.NoError");
    assert_eq!(-1, info.sourcePosition);
    assert_eq!(0, info.targetPosition);
    assert_eq!(8_i64 << 59, info.instanceID);
    assert_eq!(2_i64 << 52, info.schemaID);
    assert_eq!(1_i64 << 44, info.tableID);

    // 调整位宽后要清理缓存，否则 Go 会复用前一轮 mappingInfo。
    m.resetCache();
    SetPartitionRule(0, 0, 3);
    let info = m
        .queryColumnInfo("test_2", "xxx_1", &["id".to_string(), "name".to_string()])
        .expect("Go require.NoError");
    assert_eq!(-1, info.sourcePosition);
    assert_eq!(0, info.targetPosition);
    assert_eq!(0_i64, info.instanceID);
    assert_eq!(0_i64, info.schemaID);
    assert_eq!(1_i64 << 60, info.tableID);
}

/// 验证 `SetPartitionRule` 更新位宽后 `maxOriginID` 的重算。
// TestSetPartitionRule 对应 Go 的全局位宽设置测试，关注 maxOriginID 的重算。
#[test]
fn TestSetPartitionRule() {
    let _guard = PARTITION_RULE_TEST_LOCK.lock().unwrap();
    SetPartitionRule(4, 7, 8);
    assert_eq!(
        4,
        instanceIDBitSize.load(std::sync::atomic::Ordering::SeqCst)
    );
    assert_eq!(7, schemaIDBitSize.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(8, tableIDBitSize.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        1_i64 << 44,
        maxOriginID.load(std::sync::atomic::Ordering::SeqCst)
    );

    SetPartitionRule(0, 3, 4);
    assert_eq!(
        0,
        instanceIDBitSize.load(std::sync::atomic::Ordering::SeqCst)
    );
    assert_eq!(3, schemaIDBitSize.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(4, tableIDBitSize.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        1_i64 << 56,
        maxOriginID.load(std::sync::atomic::Ordering::SeqCst)
    );
}

/// 覆盖前缀、分隔符、默认 0 与错误文本的分区 ID 解析。
// TestComputePartitionID 对应 Go 的分区 ID 解析测试，保留前缀、分隔符、默认 0 和错误文本校验。
#[test]
fn TestComputePartitionID() {
    let _guard = PARTITION_RULE_TEST_LOCK.lock().unwrap();
    SetPartitionRule(4, 7, 8);

    let mut rule = Rule {
        PatternSchema: String::new(),
        PatternTable: String::new(),
        SourceColumn: String::new(),
        TargetColumn: String::new(),
        Expression: PartitionID(),
        Arguments: vec!["test".to_string(), "t".to_string()],
        CreateTableQuery: String::new(),
    };
    assert!(computePartitionID("test_1", "t_1", &rule).is_err());
    assert!(computePartitionID("test", "t", &rule).is_err());

    rule.Arguments = vec![
        "2".to_string(),
        "test".to_string(),
        "t".to_string(),
        "_".to_string(),
    ];
    let (instanceID, schemaID, tableID) =
        computePartitionID("test_1", "t_1", &rule).expect("Go require.NoError");
    assert_eq!(2_i64 << 59, instanceID);
    assert_eq!(1_i64 << 52, schemaID);
    assert_eq!(1_i64 << 44, tableID);

    // test default partition ID to zero
    let (instanceID, schemaID, tableID) =
        computePartitionID("test", "t_3", &rule).expect("Go require.NoError");
    assert_eq!(2_i64 << 59, instanceID);
    assert_eq!(0_i64, schemaID);
    assert_eq!(3_i64 << 44, tableID);

    let (instanceID, schemaID, tableID) =
        computePartitionID("test_5", "t", &rule).expect("Go require.NoError");
    assert_eq!(2_i64 << 59, instanceID);
    assert_eq!(5_i64 << 52, schemaID);
    assert_eq!(0_i64, tableID);

    assert_error_matches(
        computePartitionID("unrelated", "t_6", &rule),
        "test_ is not the prefix of unrelated",
    );
    assert_error_matches(
        computePartitionID("test", "x", &rule),
        "t_ is not the prefix of x",
    );
    assert_error_matches(
        computePartitionID("test_0", "t_0xa", &rule),
        "the suffix of 0xa can't be converted to int64",
    );
    // Go 原注释指出这里的错误消息还可以更好；保留同一校验意图。
    assert_error_matches(
        computePartitionID("test_0", "t_", &rule),
        "t_ is not the prefix of t_",
    );
    assert_error_matches(
        computePartitionID("testx", "t_3", &rule),
        "test_ is not the prefix of testx",
    );

    SetPartitionRule(4, 0, 8);
    rule.Arguments = vec![
        "2".to_string(),
        "test_".to_string(),
        "t_".to_string(),
        String::new(),
    ];
    let (instanceID, schemaID, tableID) =
        computePartitionID("test_1", "t_1", &rule).expect("Go require.NoError");
    assert_eq!(2_i64 << 59, instanceID);
    assert_eq!(0_i64, schemaID);
    assert_eq!(1_i64 << 51, tableID);

    let (instanceID, schemaID, tableID) =
        computePartitionID("test_", "t_", &rule).expect("Go require.NoError");
    assert_eq!(2_i64 << 59, instanceID);
    assert_eq!(0_i64, schemaID);
    assert_eq!(0_i64, tableID);

    // test ignore instance ID
    SetPartitionRule(4, 7, 8);
    rule.Arguments = vec![
        String::new(),
        "test_".to_string(),
        "t_".to_string(),
        String::new(),
    ];
    let (instanceID, schemaID, tableID) =
        computePartitionID("test_1", "t_1", &rule).expect("Go require.NoError");
    assert_eq!(0_i64, instanceID);
    assert_eq!(1_i64 << 56, schemaID);
    assert_eq!(1_i64 << 48, tableID);

    // test ignore schema ID
    rule.Arguments = vec![
        "2".to_string(),
        String::new(),
        "t_".to_string(),
        String::new(),
    ];
    let (instanceID, schemaID, tableID) =
        computePartitionID("test_1", "t_1", &rule).expect("Go require.NoError");
    assert_eq!(2_i64 << 59, instanceID);
    assert_eq!(0_i64, schemaID);
    assert_eq!(1_i64 << 51, tableID);

    // test ignore table ID
    rule.Arguments = vec![
        "2".to_string(),
        "test_".to_string(),
        String::new(),
        String::new(),
    ];
    let (instanceID, schemaID, tableID) =
        computePartitionID("test_1", "t_1", &rule).expect("Go require.NoError");
    assert_eq!(2_i64 << 59, instanceID);
    assert_eq!(1_i64 << 52, schemaID);
    assert_eq!(0_i64, tableID);
}

/// 验证目标列类型、原始 ID 上限与字符串格式保持。
// TestPartitionID 对应 Go 的 TestPartitionID：验证目标列类型、原始 ID 上限和字符串格式保持。
#[test]
fn TestPartitionID() {
    let _guard = PARTITION_RULE_TEST_LOCK.lock().unwrap();
    SetPartitionRule(4, 7, 8);
    let mut info = mappingInfo {
        instanceID: 2_i64 << 59,
        schemaID: 1_i64 << 52,
        tableID: 1_i64 << 44,
        targetPosition: 1,
        ..Default::default()
    };

    // test wrong type
    assert!(partitionID(&info, vec![Value::Int(1), Value::String("ha".to_string())]).is_err());

    // test exceed maxOriginID
    assert!(
        partitionID(
            &info,
            vec![Value::String("ha".to_string()), Value::Int(1_i64 << 44)]
        )
        .is_err()
    );

    let vals = partitionID(&info, vec![Value::String("ha".to_string()), Value::Int(1)])
        .expect("Go require.NoError");
    assert_eq!(
        vec![
            Value::String("ha".to_string()),
            Value::Int((2_i64 << 59) | (1_i64 << 52) | (1_i64 << 44) | 1)
        ],
        vals,
    );

    // Go 对字符串输入会保持字符串类型，用 fmt.Sprintf 输出合成后的十进制 ID。
    info.instanceID = 0;
    let vals = partitionID(
        &info,
        vec![
            Value::String("ha".to_string()),
            Value::String("123".to_string()),
        ],
    )
    .expect("Go require.NoError");
    assert_eq!(
        vec![
            Value::String("ha".to_string()),
            Value::String(format!("{}", (1_i64 << 52) | (1_i64 << 44) | 123)),
        ],
        vals,
    );
}

/// 验证大小写敏感时 `Test*` 不匹配 `test`，不会改写 id 列。
// TestCaseSensitive 对应 Go 的大小写敏感测试，确认 Test* 不匹配 test 时不会改写 id 列。
#[test]
fn TestCaseSensitive() {
    // we test case insensitive in TestHandle
    let rules = vec![Rule {
        PatternSchema: "Test*".to_string(),
        PatternTable: "xxx*".to_string(),
        SourceColumn: String::new(),
        TargetColumn: "id".to_string(),
        Expression: AddPrefix(),
        Arguments: vec!["instance_id:".to_string()],
        CreateTableQuery: "xx".to_string(),
    }];

    // case sensitive
    // initial column mapping
    let m = NewMapping(true, rules).expect("Go require.NoError");
    assert_eq!(0, m.cache.read().expect("cache read lock").len());

    // test add prefix, add suffix is similar
    let (vals, poss) = m
        .HandleRowValue(
            "test",
            "xxx",
            &["age".to_string(), "id".to_string()],
            vec![Value::Int(1), Value::String("1".to_string())],
        )
        .expect("Go require.NoError");
    assert_eq!(vec![Value::Int(1), Value::String("1".to_string())], vals);
    assert!(poss.is_none());
}

// assert_error_matches 对应 Go 的 require.Regexp，这里只保留“错误文本包含关键片段”的人工审查语义。
fn assert_error_matches<T: std::fmt::Debug>(result: Result<T, String>, pattern: &str) {
    let err = result.expect_err("Go require.Regexp expects an error");
    assert!(
        err.contains(pattern),
        "expected error {err:?} to contain {pattern:?}"
    );
}
