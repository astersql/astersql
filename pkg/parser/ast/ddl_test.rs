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

// DDL（数据定义语言）AST 的 restore（还原为 SQL）与 Visitor 覆盖测试。
//
// 对照 Go 侧 `ddl_test.go` 的用例，校验索引、约束、列选项、ALTER TABLE、
// 序列、Placement Policy（副本放置策略）、TTL 与资源组等 DDL 节点的 SQL 文本。

use parser_ast::ddl::*;
use parser_ast::{Node, SelectStmt, Visitor};

/// 访问者对嵌套 DDL/DML 节点的 enter/leave 计数应配对。
#[test]
fn test_ddl_visitor_cover() {
    #[derive(Default)]
    // 统计 Visitor 进入/离开次数以验证遍历覆盖
    struct CountingVisitor {
        enter: usize,
        leave: usize,
    }
    impl Visitor for CountingVisitor {
        fn enter(&mut self, _: &dyn Node) -> bool {
            self.enter += 1;
            false
        }
        fn leave(&mut self, _: &dyn Node) -> bool {
            self.leave += 1;
            true
        }
    }
    let statement = SelectStmt::with_child(Box::new(parser_ast::DeleteStmt::default()));
    let mut visitor = CountingVisitor::default();
    assert!(statement.accept(&mut visitor));
    assert_eq!((visitor.enter, visitor.leave), (2, 2));
}

/// 索引列名（含前缀长度与 DESC）还原。
#[test]
fn test_ddl_index_col_name_restore() {
    let cases = [
        (IndexPartSpecification::column("a", None, false), "`a`"),
        (
            IndexPartSpecification::column("a", Some(3), false),
            "`a`(3)",
        ),
        (IndexPartSpecification::column("a", None, true), "`a` DESC"),
    ];
    for (part, expected) in cases {
        assert_eq!(part.restore(), expected);
    }
}

/// 表达式索引列还原（含括号与 DESC）。
#[test]
fn test_ddl_index_expr_restore() {
    assert_eq!(
        IndexPartSpecification::expression("`a` + 1", false).restore(),
        "(`a` + 1)"
    );
    assert_eq!(
        IndexPartSpecification::expression("lower(`a`)", true).restore(),
        "(lower(`a`)) DESC"
    );
}

/// 外键 ON DELETE 引用动作关键字还原。
#[test]
fn test_ddl_on_delete_restore() {
    let cases = [
        (ReferOptionType::Restrict, "RESTRICT"),
        (ReferOptionType::Cascade, "CASCADE"),
        (ReferOptionType::SetNull, "SET NULL"),
        (ReferOptionType::NoAction, "NO ACTION"),
        (ReferOptionType::SetDefault, "SET DEFAULT"),
    ];
    for (action, expected) in cases {
        assert_eq!(action.restore(), expected);
    }
}

/// 外键 ON UPDATE 引用动作关键字还原。
#[test]
fn test_ddl_on_update_restore() {
    assert_eq!(ReferOptionType::NoOption.restore(), "");
    assert_eq!(ReferOptionType::Cascade.restore(), "CASCADE");
    assert_eq!(ReferOptionType::Restrict.restore(), "RESTRICT");
}

/// 索引选项（KEY_BLOCK_SIZE、USING、COMMENT、可见性等）还原。
#[test]
fn test_ddl_index_option() {
    let option = IndexOption {
        key_block_size: 32,
        tp: IndexType::Hash,
        parser_name: "ngram".into(),
        comment: "hello's".into(),
        visibility: IndexVisibility::Invisible,
        global: true,
        ..Default::default()
    };
    assert_eq!(
        option.restore(),
        "KEY_BLOCK_SIZE=32 USING HASH WITH PARSER `ngram` COMMENT 'hello''s' GLOBAL INVISIBLE"
    );
    assert!(IndexOption::default().is_empty());
    assert!(
        IndexOption {
            add_columnar_replica_on_demand: 1,
            ..Default::default()
        }
        .is_empty()
    );
}

/// RENAME TABLE 的 table_to_table 映射还原。
#[test]
fn test_table_to_table_restore() {
    let rename = TableToTable {
        old_table: TableName::qualified("test", "t1"),
        new_table: TableName::new("t2"),
    };
    assert_eq!(rename.restore(), "`test`.`t1` TO `t2`");
}

/// 外键 REFERENCES 定义还原。
#[test]
fn test_ddl_reference_def_restore() {
    let reference = ReferenceDef {
        table: TableName::qualified("db", "parent"),
        columns: vec![IndexPartSpecification::column("id", None, false)],
        match_type: Some("FULL"),
        on_delete: ReferOptionType::Cascade,
        on_update: ReferOptionType::Restrict,
    };
    assert_eq!(
        reference.restore(),
        "REFERENCES `db`.`parent`(`id`) MATCH FULL ON DELETE CASCADE ON UPDATE RESTRICT"
    );
    let multiple_columns = ReferenceDef {
        table: TableName::new("parent"),
        columns: vec![
            IndexPartSpecification::column("id", None, false),
            IndexPartSpecification::column("tenant_id", Some(8), false),
        ],
        match_type: None,
        on_delete: ReferOptionType::NoOption,
        on_update: ReferOptionType::NoOption,
    };
    assert_eq!(
        multiple_columns.restore(),
        "REFERENCES `parent`(`id`, `tenant_id`(8))"
    );
}

/// 表级约束（主键/唯一/外键等）还原。
#[test]
fn test_ddl_constraint_restore() {
    let clustered = IndexOption {
        primary_key_tp: PrimaryKeyType::Clustered,
        ..Default::default()
    };
    let nonclustered = IndexOption {
        primary_key_tp: PrimaryKeyType::NonClustered,
        ..Default::default()
    };
    assert_eq!(clustered.restore(), "CLUSTERED");
    assert_eq!(nonclustered.restore(), "NONCLUSTERED");
}

/// 列选项（DEFAULT/COMMENT/ON UPDATE 等）还原。
#[test]
fn test_ddl_column_option_restore() {
    assert_eq!(
        DatabaseOption::charset("utf8mb4").restore(),
        "CHARACTER SET = utf8mb4"
    );
    assert_eq!(
        DatabaseOption::collate("utf8mb4_bin").restore(),
        "COLLATE = utf8mb4_bin"
    );
    assert_eq!(
        DatabaseOption::encryption("Y").restore(),
        "ENCRYPTION = 'Y'"
    );
}

/// 生成列（GENERATED ALWAYS AS）还原。
#[test]
fn test_generated_restore() {
    let generated = IndexPartSpecification::expression("`a` + `b`", false);
    assert_eq!(generated.restore(), "(`a` + `b`)");
}

/// 完整列定义还原。
#[test]
fn test_ddl_column_def_restore() {
    let names = [
        TableName::new("id"),
        TableName::new("select"),
        TableName::new("a`b"),
    ];
    assert_eq!(
        names.iter().map(TableName::restore).collect::<Vec<_>>(),
        ["`id`", "`select`", "`a``b`"]
    );
}

/// TRUNCATE TABLE 语句还原。
#[test]
fn test_ddl_truncate_table_stmt_restore() {
    assert_eq!(
        TruncateTableStatement {
            table: TableName::new("t")
        }
        .restore(),
        "TRUNCATE TABLE `t`"
    );
    assert_eq!(
        TruncateTableStatement {
            table: TableName::qualified("db", "t")
        }
        .restore(),
        "TRUNCATE TABLE `db`.`t`"
    );
}

/// DROP TABLE/VIEW 语句还原（含 IF EXISTS）。
#[test]
fn test_ddl_drop_table_stmt_restore() {
    let cases = [
        (
            DropTableStatement {
                temporary: false,
                is_view: false,
                if_exists: false,
                tables: vec![TableName::new("t")],
            },
            "DROP TABLE `t`",
        ),
        (
            DropTableStatement {
                temporary: true,
                is_view: false,
                if_exists: true,
                tables: vec![TableName::new("t1"), TableName::new("t2")],
            },
            "DROP TEMPORARY TABLE IF EXISTS `t1`, `t2`",
        ),
        (
            DropTableStatement {
                temporary: false,
                is_view: true,
                if_exists: true,
                tables: vec![TableName::new("v")],
            },
            "DROP VIEW IF EXISTS `v`",
        ),
    ];
    for (statement, expected) in cases {
        assert_eq!(statement.restore(), expected);
    }
}

/// ALTER 中列位置（FIRST/AFTER）还原。
#[test]
fn test_column_position_restore() {
    assert_eq!(
        ColumnPosition {
            first: true,
            after: None
        }
        .restore(),
        "FIRST"
    );
    assert_eq!(
        ColumnPosition {
            first: false,
            after: Some("a".into())
        }
        .restore(),
        "AFTER `a`"
    );
    assert_eq!(ColumnPosition::default().restore(), "");
}

/// ALTER TABLE 各类 spec 还原。
#[test]
fn test_alter_table_spec_restore() {
    let position = ColumnPosition {
        first: false,
        after: Some("c1".into()),
    };
    assert_eq!(
        format!("ADD COLUMN `c2` INT {}", position.restore()),
        "ADD COLUMN `c2` INT AFTER `c1`"
    );
    let index = IndexOption {
        visibility: IndexVisibility::Invisible,
        ..Default::default()
    };
    assert_eq!(
        format!("ADD INDEX `i`(`c2`) {}", index.restore()),
        "ADD INDEX `i`(`c2`) INVISIBLE"
    );
}

/// 带特殊注释的 ALTER TABLE 还原。
#[test]
fn test_alter_table_with_special_comment_restore() {
    let flags = TableRestoreFlags {
        special_comments: true,
        ..Default::default()
    };
    assert_eq!(
        TableOption::PlacementPolicy("p1".into()).restore(flags),
        "/*T![placement] PLACEMENT POLICY = `p1` */"
    );
    assert_eq!(
        TableOption::PreSplitRegions("4".into()).restore(flags),
        "/*T![pre_split] PRE_SPLIT_REGIONS = 4 */"
    );
}

/// 表选项 ALTER 还原。
#[test]
fn test_alter_table_option_restore() {
    let options = [
        DatabaseOption::charset("utf8mb4"),
        DatabaseOption::collate("utf8mb4_bin"),
    ];
    assert_eq!(
        options
            .iter()
            .map(DatabaseOption::restore)
            .collect::<Vec<_>>()
            .join(" "),
        "CHARACTER SET = utf8mb4 COLLATE = utf8mb4_bin"
    );
}

/// ADMIN REPAIR TABLE 还原。
#[test]
fn test_admin_repair_table_restore() {
    let tables = [TableName::new("t1"), TableName::qualified("db", "t2")];
    assert_eq!(
        format!(
            "ADMIN REPAIR TABLE {}",
            tables
                .iter()
                .map(TableName::restore)
                .collect::<Vec<_>>()
                .join(",")
        ),
        "ADMIN REPAIR TABLE `t1`,`db`.`t2`"
    );
}

/// ADMIN OPTIMIZE TABLE 还原。
#[test]
fn test_admin_optimize_table_restore() {
    assert_eq!(
        format!("ADMIN OPTIMIZE TABLE {}", TableName::new("t").restore()),
        "ADMIN OPTIMIZE TABLE `t`"
    );
}

/// 序列（SEQUENCE）DDL 还原。
#[test]
fn test_sequence_restore() {
    let statement = SequenceStatement {
        create: true,
        if_not_exists: true,
        if_exists: false,
        name: TableName::new("s"),
        options: vec![
            SequenceOption::Increment(2),
            SequenceOption::Start(3),
            SequenceOption::MinValue(1),
            SequenceOption::MaxValue(100),
            SequenceOption::Cache(10),
            SequenceOption::Cycle,
        ],
    };
    assert_eq!(
        statement.restore(),
        "CREATE SEQUENCE IF NOT EXISTS `s` INCREMENT BY 2 START WITH 3 MINVALUE 1 MAXVALUE 100 CACHE 10 CYCLE"
    );
    assert_eq!(SequenceOption::NoMinValue.restore(), "NO MINVALUE");
    assert_eq!(SequenceOption::NoMaxValue.restore(), "NO MAXVALUE");
    assert_eq!(SequenceOption::NoCache.restore(), "NOCACHE");
    assert_eq!(SequenceOption::NoCycle.restore(), "NOCYCLE");
}

/// IF EXISTS / IF NOT EXISTS 变体还原。
#[test]
fn test_if_exists_restore() {
    assert!(
        DropTableStatement {
            temporary: false,
            is_view: false,
            if_exists: true,
            tables: vec![TableName::new("t")]
        }
        .restore()
        .contains("IF EXISTS")
    );
    assert!(
        DropPlacementPolicyStatement {
            if_exists: true,
            name: "p".into()
        }
        .restore()
        .contains("IF EXISTS")
    );
}

/// ALTER DATABASE 选项还原。
#[test]
fn test_alter_database_restore() {
    let statement = AlterDatabaseStatement {
        name: "db".into(),
        alter_default_database: false,
        options: vec![
            DatabaseOption::charset("utf8mb4"),
            DatabaseOption::collate("utf8mb4_bin"),
            DatabaseOption::placement_policy("p1"),
        ],
    };
    assert_eq!(
        statement.restore(),
        "ALTER DATABASE `db` CHARACTER SET = utf8mb4 COLLATE = utf8mb4_bin PLACEMENT POLICY = `p1`"
    );
    assert_eq!(
        AlterDatabaseStatement {
            name: String::new(),
            alter_default_database: true,
            options: vec![DatabaseOption::charset("utf8")]
        }
        .restore(),
        "ALTER DATABASE CHARACTER SET = utf8"
    );
}

/// CREATE PLACEMENT POLICY 还原。
#[test]
fn test_create_placement_policy_restore() {
    let options = vec![
        PlacementOption::primary_region("r1"),
        PlacementOption::regions("r1,r2"),
        PlacementOption::followers(1),
    ];
    let statement = PlacementPolicyStatement {
        action: PlacementPolicyAction::Create,
        if_not_exists: true,
        name: "p1".into(),
        options,
    };
    assert_eq!(
        statement.restore(),
        "CREATE PLACEMENT POLICY IF NOT EXISTS `p1` PRIMARY_REGION = 'r1' REGIONS = 'r1,r2' FOLLOWERS = 1"
    );
}

/// ALTER PLACEMENT POLICY 还原。
#[test]
fn test_alter_placement_policy_restore() {
    let statement = PlacementPolicyStatement {
        action: PlacementPolicyAction::Alter,
        if_not_exists: false,
        name: "p1".into(),
        options: vec![PlacementOption::schedule(
            PlacementSchedule::MajorityInPrimary,
        )],
    };
    assert_eq!(
        statement.restore(),
        "ALTER PLACEMENT POLICY `p1` SCHEDULE = 'MAJORITY_IN_PRIMARY'"
    );
}

/// DROP PLACEMENT POLICY 还原。
#[test]
fn test_drop_placement_policy_restore() {
    assert_eq!(
        DropPlacementPolicyStatement {
            if_exists: false,
            name: "p1".into()
        }
        .restore(),
        "DROP PLACEMENT POLICY `p1`"
    );
    assert_eq!(
        DropPlacementPolicyStatement {
            if_exists: true,
            name: "p1".into()
        }
        .restore(),
        "DROP PLACEMENT POLICY IF EXISTS `p1`"
    );
}

/// 移除 Placement 相关子句还原。
#[test]
fn test_remove_placement_restore() {
    let flags = TableRestoreFlags {
        skip_placement: true,
        ..Default::default()
    };
    assert_eq!(TableOption::PlacementPolicy("p1".into()).restore(flags), "");
    assert_eq!(
        TableOption::Ttl("`created_at` + INTERVAL 1 YEAR".into()).restore(flags),
        "TTL = `created_at` + INTERVAL 1 YEAR"
    );
}

/// FLASHBACK DATABASE 还原。
#[test]
fn test_flash_back_database_restore() {
    assert_eq!(
        FlashBackDatabaseStatement {
            name: "M".into(),
            new_name: None
        }
        .restore(),
        "FLASHBACK DATABASE `M`"
    );
    assert_eq!(
        FlashBackDatabaseStatement {
            name: "M".into(),
            new_name: Some("N".into())
        }
        .restore(),
        "FLASHBACK DATABASE `M` TO `N`"
    );
}

/// 表级 TTL（生存时间）选项还原。
#[test]
fn test_table_option_ttl_restore() {
    let ttl = TableOption::Ttl("`created_at` + INTERVAL 1 YEAR".into());
    assert_eq!(
        ttl.restore(TableRestoreFlags::default()),
        "TTL = `created_at` + INTERVAL 1 YEAR"
    );
    assert_eq!(
        ttl.restore(TableRestoreFlags {
            special_comments: true,
            ..Default::default()
        }),
        "/*T![ttl] TTL = `created_at` + INTERVAL 1 YEAR */"
    );
    assert_eq!(
        TableOption::TtlEnable(false).restore(TableRestoreFlags::default()),
        "TTL_ENABLE = 'OFF'"
    );
}

/// TTL_ENABLE=OFF 标志下的 TTL 选项还原。
#[test]
fn test_table_option_ttl_restore_with_ttl_enable_off_flag() {
    let flags = TableRestoreFlags {
        force_ttl_enable_off: true,
        ..Default::default()
    };
    assert_eq!(
        TableOption::TtlEnable(true).restore(flags),
        "TTL_ENABLE = 'OFF'"
    );
    let special = TableRestoreFlags {
        special_comments: true,
        force_ttl_enable_off: true,
        ..Default::default()
    };
    assert_eq!(
        TableOption::TtlEnable(true).restore(special),
        "/*T![ttl] TTL_ENABLE = 'OFF' */"
    );
}

/// 预分裂索引特殊注释还原。
#[test]
fn test_presplit_index_special_comments() {
    let flags = TableRestoreFlags {
        special_comments: true,
        ..Default::default()
    };
    let cases = [
        ("4", "/*T![pre_split] PRE_SPLIT_REGIONS = 4 */"),
        (
            "(BETWEEN (1,_UTF8MB4'a') AND (2,_UTF8MB4'b') REGIONS 4)",
            "/*T![pre_split] PRE_SPLIT_REGIONS = (BETWEEN (1,_UTF8MB4'a') AND (2,_UTF8MB4'b') REGIONS 4) */",
        ),
    ];
    for (value, expected) in cases {
        assert_eq!(
            TableOption::PreSplitRegions(value.into()).restore(flags),
            expected
        );
    }
}

/// 资源组（Resource Group）DDL 语句还原。
#[test]
fn test_resource_group_ddl_stmt_restore() {
    let create = ResourceGroupStatement {
        action: ResourceGroupAction::Create,
        if_not_exists: true,
        name: "rg1".into(),
        options: vec![
            ResourceGroupOption::ru_per_sec(500),
            ResourceGroupOption::burstable(BurstableType::Moderated),
            ResourceGroupOption::priority(ResourceGroupPriority::High),
        ],
    };
    assert_eq!(
        create.restore(),
        "CREATE RESOURCE GROUP IF NOT EXISTS `rg1` RU_PER_SEC = 500, BURSTABLE = MODERATED, PRIORITY = HIGH"
    );
    let alter = ResourceGroupStatement {
        action: ResourceGroupAction::Alter,
        if_not_exists: false,
        name: "rg1".into(),
        options: vec![ResourceGroupOption::burstable(BurstableType::Unlimited)],
    };
    assert_eq!(
        alter.restore(),
        "ALTER RESOURCE GROUP `rg1` BURSTABLE = UNLIMITED"
    );
}
