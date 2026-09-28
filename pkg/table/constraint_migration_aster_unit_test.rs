// Copyright 2026 AsterSQL.
// Copyright 2023-2023 PingCAP, Inc.
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

// CHECK 约束加载、自增列检测与外键引用动作冲突的迁移单测。

use model_dependency::group_4 as constraint_model;
use parser_ast_dependency as ast;

use crate::{ContainsAutoIncrementCol, HasForeignKeyRefAction, LoadCheckConstraint};

/// 构造 model 层 CIStr（大小写不敏感标识符）。
fn constraint_name(value: &str) -> constraint_model::ast::CIStr {
    constraint_model::ast::NewCIStr(value)
}

/// 构造 parser AST 层 CIStr。
fn ast_name(value: &str) -> ast::CIStr {
    ast::NewCIStr(value)
}

/// 加载时应剔除引用缺失列或非 Public 列的 CHECK，仅保留合法约束。
#[test]
fn load_check_constraint_removes_missing_and_non_public_columns() {
    let mut table = constraint_model::TableInfo::default();
    table.Columns = vec![
        constraint_model::ColumnInfo {
            Name: constraint_name("visible"),
            Offset: 0,
            State: constraint_model::StatePublic,
            ..Default::default()
        },
        constraint_model::ColumnInfo {
            Name: constraint_name("hidden_state"),
            Offset: 1,
            State: constraint_model::StateWriteOnly,
            ..Default::default()
        },
    ];
    table.Constraints = vec![
        constraint_model::ConstraintInfo {
            Name: constraint_name("valid"),
            ConstraintCols: vec![constraint_name("visible")],
            ..Default::default()
        },
        constraint_model::ConstraintInfo {
            Name: constraint_name("missing"),
            ConstraintCols: vec![constraint_name("absent")],
            ..Default::default()
        },
        constraint_model::ConstraintInfo {
            Name: constraint_name("non_public"),
            ConstraintCols: vec![constraint_name("hidden_state")],
            ..Default::default()
        },
    ];

    let loaded = LoadCheckConstraint(&mut table).unwrap();

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].ConstraintInfo.Name.L, "valid");
    assert_eq!(table.Constraints.len(), 1);
    assert_eq!(table.Constraints[0].Name.L, "valid");
}

/// AUTO_INCREMENT 列名匹配应忽略大小写。
#[test]
fn contains_auto_increment_column_matches_case_insensitive_name() {
    let mut auto = model_dependency::ColumnInfo::default();
    auto.Name = ast_name("RecordID");
    auto.AddFlag(parser_mysql_dependency::r#type::AutoIncrementFlag);
    let table = model_dependency::TableInfo {
        Columns: vec![auto],
        ..Default::default()
    };

    assert!(ContainsAutoIncrementCol(&[ast_name("recordid")], &table));
    assert!(!ContainsAutoIncrementCol(&[ast_name("other")], &table));
    assert!(!ContainsAutoIncrementCol(&[], &table));
}

/// 仅当 CHECK 依赖的列确实出现在带引用动作的 FK 上时才拒绝。
#[test]
fn foreign_key_metadata_action_rejects_only_depended_fk_columns() {
    let check = ast::Constraint {
        Name: "positive_value".to_owned(),
        Tp: ast::ConstraintType::Check,
        ..Default::default()
    };
    let no_action = constraint_model::FKInfo {
        Cols: vec![constraint_name("parent_id")],
        ..Default::default()
    };
    assert!(
        HasForeignKeyRefAction(
            Some(vec![Box::new(no_action)]),
            &[],
            &check,
            &[ast_name("parent_id")],
        )
        .is_ok()
    );

    let cascading = constraint_model::FKInfo {
        Cols: vec![constraint_name("parent_id")],
        OnDelete: 2,
        ..Default::default()
    };
    assert!(
        HasForeignKeyRefAction(
            Some(vec![Box::new(cascading.clone())]),
            &[],
            &check,
            &[ast_name("unrelated")],
        )
        .is_ok()
    );
    let error = HasForeignKeyRefAction(
        Some(vec![Box::new(cascading)]),
        &[],
        &check,
        &[ast_name("parent_id")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("parent_id"));
    assert!(error.to_string().contains("positive_value"));
}

/// `None` 走 AST 外键分支；`Some(空)` 表示已有元数据路径且无 FK，应对齐 Go nil 语义。
#[test]
fn foreign_key_ast_action_obeys_none_vs_non_none_metadata_branch() {
    let check = ast::Constraint {
        Name: "check_child".to_owned(),
        Tp: ast::ConstraintType::Check,
        ..Default::default()
    };
    let foreign_key = ast::Constraint {
        Tp: ast::ConstraintType::ForeignKey,
        Keys: vec![ast::IndexPartSpecification {
            Column: Some(ast::ColumnName {
                Name: ast_name("child_id"),
                ..Default::default()
            }),
            ..Default::default()
        }],
        Refer: Some(ast::ReferenceDef {
            OnUpdate: ast::OnUpdateOpt {
                ReferOpt: ast::ReferOptionType::Cascade,
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let constraints = vec![Box::new(foreign_key)];

    assert!(HasForeignKeyRefAction(None, &constraints, &check, &[ast_name("other")]).is_ok());
    assert!(HasForeignKeyRefAction(None, &constraints, &check, &[ast_name("child_id")]).is_err());
    assert!(
        HasForeignKeyRefAction(
            Some(Vec::new()),
            &constraints,
            &check,
            &[ast_name("child_id")]
        )
        .is_ok()
    );
}
