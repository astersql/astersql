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

// 列脱敏策略（masking policy）DDL 相关测试。
//

use crate::column::SchemaState;
use crate::executor::{
    ColumnInfo, ColumnKind, Executor, ExecutorError, Ident, MemoryJobBackend, OnExist,
    SessionContext, TableInfo,
};
use crate::masking_policy::{
    MaskingPolicyInfo, MaskingPolicyRestrictOps, MaskingPolicyStatus, MaskingPolicyStore,
    MaskingPolicyType, rewrite_masking_policy_expression_column_name,
};
use std::time::Duration;

fn masking_policy(table_id: i64, column_id: i64, name: &str) -> MaskingPolicyInfo {
    MaskingPolicyInfo {
        id: 0,
        name: name.into(),
        database_name: format!("db_{table_id}"),
        table_name: format!("t_{table_id}"),
        table_id,
        column_name: format!("c_{column_id}"),
        column_id,
        expression: format!("`c_{column_id}`"),
        status: MaskingPolicyStatus::Enable,
        masking_type: MaskingPolicyType::Custom,
        restrict_ops: MaskingPolicyRestrictOps::default(),
        created_at: 1,
        updated_at: 1,
        created_by: "root".into(),
        state: SchemaState::None,
    }
}

#[test]
fn duplicate_policy_names_are_scoped_to_a_table() {
    let mut store = MaskingPolicyStore::default();
    let first_id = store.create(masking_policy(10, 1, "p"), false).unwrap();
    let second_id = store.create(masking_policy(20, 1, "p"), false).unwrap();

    assert_ne!(first_id, second_id);
    assert_eq!(store.by_table(10).len(), 1);
    assert_eq!(store.by_table(20).len(), 1);
}

#[test]
fn renaming_a_column_does_not_rewrite_string_literals() {
    let rewritten = rewrite_masking_policy_expression_column_name(
        "concat(`secret`, 'secret')",
        "secret",
        "masked",
    )
    .unwrap();

    assert_eq!(rewritten, "concat(`masked`, 'secret')");
}

/// 跨库 rename 后列仍带脱敏策略引用，且改为不兼容类型时返回 Dependency 错误。
#[test]
fn masking_policy_survives_rename_and_blocks_incompatible_type_change() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    // 准备源库与目标库，创建带 masking_policy 引用的列。
    ddl.create_schema(&mut session, "one", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_schema(&mut session, "two", &[], None, OnExist::Error)
        .unwrap();
    let mut protected = ColumnInfo::integer("secret");
    protected.masking_policy = Some(42);
    ddl.create_table(
        &mut session,
        "one",
        TableInfo::new("t", vec![protected]),
        OnExist::Error,
    )
    .unwrap();
    // 跨库改名后策略依赖应随表迁移保留。
    ddl.rename_tables(
        &mut session,
        &[(Ident::new("one", "t"), Ident::new("two", "renamed"))],
    )
    .unwrap();
    // 改为 String 与既有策略不兼容，期望 Dependency("masking policy")。
    let mut replacement = ColumnInfo::integer("secret");
    replacement.kind = ColumnKind::String;
    assert!(matches!(
        ddl.modify_column(&mut session, &Ident::new("two", "renamed"), "secret", replacement),
        Err(ExecutorError::Dependency(message)) if message == "masking policy"
    ));
}
