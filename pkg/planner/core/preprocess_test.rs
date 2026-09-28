// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 预处理（preprocess）阶段单元测试。
//
// 覆盖列/索引选项校验、CREATE TABLE 语法检查、表别名冲突、
// SQL 末尾分号擦除，以及为 SELECT 注入额外 Limit 的边界行为。

use std::collections::HashMap;

use crate::planbuilder::Statement;
use crate::preprocess::{
    ColumnDef, ColumnOption, EraseLastSemicolonInSQL, IndexOption, IndexPart, Preprocess,
    PreprocessNode, TableName, TableOption, TryAddExtraLimit, bindableStmtType,
    checkAutoIncrementOp, checkColumn, checkColumnOptions, checkIndexInfo, checkIndexOptions,
    checkIndexSpecs, checkTableEngine, isTableAliasDuplicate,
};
use crate::task::{PlanKind, PlanNode};

/// 构造带给定选项的列定义，便于断言校验函数。
fn column(name: &str, data_type: &str, options: Vec<ColumnOption>) -> ColumnDef {
    ColumnDef {
        name: name.to_owned(),
        data_type: data_type.to_owned(),
        generated: options.contains(&ColumnOption::Generated),
        stored: false,
        options,
    }
}

#[test]
/// 验证自增、空值冲突、索引部件与向量/全局索引选项等规则边界。
fn validator_preserves_column_and_index_rules() {
    let auto = column(
        "id",
        "bigint",
        vec![ColumnOption::NotNull, ColumnOption::AutoIncrement],
    );
    assert!(checkAutoIncrementOp(&auto, 0).expect("valid auto increment"));
    assert!(checkColumn(&auto).is_ok());
    assert!(
        checkAutoIncrementOp(
            &column("id", "varchar", vec![ColumnOption::AutoIncrement]),
            0
        )
        .is_err()
    );
    assert!(checkColumnOptions(false, &[ColumnOption::Null, ColumnOption::NotNull]).is_err());
    assert!(checkColumnOptions(true, &[ColumnOption::AutoRandom]).is_err());

    let parts = vec![
        IndexPart {
            column: "a".to_owned(),
            length: None,
            expression: None,
        },
        IndexPart {
            column: "b".to_owned(),
            length: Some(8),
            expression: None,
        },
    ];
    assert!(checkIndexSpecs(&IndexOption::default(), &parts).is_ok());
    assert!(checkIndexInfo("idx_ab", &parts).is_ok());
    assert!(checkIndexInfo("idx", &[parts[0].clone(), parts[0].clone()]).is_err());
    assert!(
        checkIndexOptions(
            false,
            &IndexOption {
                index_type: "vector".to_owned(),
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        checkIndexOptions(
            true,
            &IndexOption {
                global: true,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
/// CREATE TABLE 预处理应通过合法引擎/主键定义，并拒绝重复列名与非法引擎。
fn preprocess_create_table_runs_real_grammar_checks() {
    let table = TableName {
        schema: "test".to_owned(),
        name: "t".to_owned(),
        ..Default::default()
    };
    let node = PreprocessNode::CreateTable {
        table: table.clone(),
        columns: vec![column("id", "bigint", vec![ColumnOption::PrimaryKey])],
        constraints: Vec::new(),
        options: vec![TableOption::Engine("InnoDB".to_owned())],
    };
    assert!(Preprocess(&node, HashMap::new(), &[]).is_ok());

    // 列名大小写不敏感，a/A 视为重复。
    let duplicate = PreprocessNode::CreateTable {
        table,
        columns: vec![column("a", "int", vec![]), column("A", "int", vec![])],
        constraints: Vec::new(),
        options: Vec::new(),
    };
    assert!(Preprocess(&duplicate, HashMap::new(), &[]).is_err());
    assert!(checkTableEngine("InnoDB").is_ok());
    assert!(checkTableEngine("unknown").is_err());
}

#[test]
/// 别名冲突、擦除末尾分号、以及非 FOR UPDATE 时注入 Limit 的行为对齐 Go。
fn aliases_semicolons_and_select_limit_match_go_boundaries() {
    let mut aliases = HashMap::new();
    let tables = vec![
        TableName {
            name: "t1".to_owned(),
            alias: "x".to_owned(),
            ..Default::default()
        },
        TableName {
            name: "t2".to_owned(),
            alias: "X".to_owned(),
            ..Default::default()
        },
    ];
    assert!(isTableAliasDuplicate(&tables, &mut aliases).is_err());
    assert_eq!(EraseLastSemicolonInSQL(" select 1;  "), " select 1;  ");

    let select = Statement::Select {
        plan: PlanNode::new(PlanKind::Projection),
        for_update: false,
    };
    // 非 for_update 时应包一层 Limit。
    let Statement::Select { plan, .. } = TryAddExtraLimit(&select, 10, false) else {
        panic!("expected select");
    };
    assert_eq!(plan.kind, PlanKind::Limit);
    assert_eq!(plan.count, 10);
    // for_update 场景不注入额外 Limit。
    let Statement::Select { plan, .. } = TryAddExtraLimit(&select, 10, true) else {
        panic!("expected select");
    };
    assert_eq!(plan.kind, PlanKind::Projection);
}

#[test]
fn validator_matches_go_edge_semantics() {
    // Go only removes a semicolon when it is the final byte; it never trims whitespace.
    assert_eq!(EraseLastSemicolonInSQL("select 1;"), "select 1");
    assert_eq!(EraseLastSemicolonInSQL("select 1;  "), "select 1;  ");

    // TiDB accepts the complete MySQL-compatible engine-name set during preprocessing.
    assert!(checkTableEngine("MyISAM").is_ok());
    assert!(checkTableEngine("BLACKHOLE").is_ok());
    assert_eq!(
        bindableStmtType(&Statement::Ddl {
            kind: "create table".into(),
            table: None,
        }),
        0
    );

    // Go permits FLOAT/DOUBLE AUTO_INCREMENT and a NULL default after AUTO_INCREMENT.
    let float_auto = column(
        "id",
        "double",
        vec![
            ColumnOption::AutoIncrement,
            ColumnOption::Default("null".into()),
        ],
    );
    assert!(checkAutoIncrementOp(&float_auto, 0).expect("Go-compatible auto increment"));

    // A virtual generated column cannot be a primary key; a stored one can.
    let virtual_primary = column(
        "g",
        "int",
        vec![ColumnOption::Generated, ColumnOption::PrimaryKey],
    );
    assert!(checkColumn(&virtual_primary).is_err());
    let mut stored_primary = virtual_primary;
    stored_primary.stored = true;
    assert!(checkColumn(&stored_primary).is_ok());

    let vector_expr = [IndexPart {
        column: String::new(),
        length: None,
        expression: Some("vec_cosine_distance(v)".into()),
    }];
    assert!(
        checkIndexOptions(
            true,
            &IndexOption {
                index_type: "vector".into(),
                ..Default::default()
            }
        )
        .is_ok()
    );
    assert!(
        checkIndexSpecs(
            &IndexOption {
                index_type: "vector".into(),
                ..Default::default()
            },
            &vector_expr,
        )
        .is_ok()
    );
    assert!(
        checkIndexOptions(
            false,
            &IndexOption {
                index_type: "vector".into(),
                ..Default::default()
            }
        )
        .is_err()
    );
}
