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

// DDL（数据定义语言）AST 相关的单元测试。
//
// 校验 `DatabaseOption`、外键引用动作（ReferOption）、索引选项、
// Placement（副本放置策略）与资源组（Resource Group）选项的 `restore`
//（将 AST 还原为 SQL 文本）结果与 Go/TiDB 侧一致。

use crate::ddl::*;

/// 校验各类数据库选项还原出的 SQL 关键字顺序与字面量与 Go 一致。
#[test]
fn database_options_restore_like_go() {
    let options = [
        DatabaseOption::charset("utf8mb4"),
        DatabaseOption::collate("utf8mb4_bin"),
        DatabaseOption::encryption("Y"),
        DatabaseOption::placement_policy("p1"),
        DatabaseOption::tiflash_replica(2, vec!["zone".into(), "rack".into()]),
    ];
    let sql: Vec<_> = options.iter().map(DatabaseOption::restore).collect();
    assert_eq!(
        sql,
        [
            "CHARACTER SET = utf8mb4",
            "COLLATE = utf8mb4_bin",
            "ENCRYPTION = 'Y'",
            "PLACEMENT POLICY = `p1`",
            "SET TIFLASH REPLICA 2 LOCATION LABELS 'zone', 'rack'",
        ]
    );
}

/// 无效的 `DatabaseOptionType::None` 在 try_restore 时应返回明确错误。
#[test]
fn invalid_database_option_is_rejected() {
    let option = DatabaseOption {
        tp: DatabaseOptionType::None,
        ..DatabaseOption::default()
    };
    assert_eq!(
        option.try_restore().unwrap_err(),
        "invalid DatabaseOptionType: 0"
    );
}

/// 校验外键 ON DELETE/UPDATE 动作关键字及索引选项还原格式。
#[test]
fn reference_actions_and_index_options_match_go() {
    assert_eq!(ReferOptionType::Restrict.restore(), "RESTRICT");
    assert_eq!(ReferOptionType::Cascade.restore(), "CASCADE");
    assert_eq!(ReferOptionType::SetNull.restore(), "SET NULL");
    assert_eq!(ReferOptionType::NoAction.restore(), "NO ACTION");
    assert_eq!(ReferOptionType::SetDefault.restore(), "SET DEFAULT");

    let option = IndexOption {
        key_block_size: 32,
        tp: IndexType::Hash,
        comment: "hello".into(),
        ..IndexOption::default()
    };
    assert_eq!(
        option.restore(),
        "KEY_BLOCK_SIZE=32 USING HASH COMMENT 'hello'"
    );
}

/// 校验 Placement 与资源组选项按 Go 约定的顺序拼接还原。
#[test]
fn placement_and_resource_options_restore_in_go_order() {
    let placement = vec![
        PlacementOption::primary_region("r1"),
        PlacementOption::regions("r1,r2"),
        PlacementOption::followers(1),
        PlacementOption::schedule(PlacementSchedule::MajorityInPrimary),
    ];
    assert_eq!(
        restore_placement_options(&placement),
        "PRIMARY_REGION = 'r1' REGIONS = 'r1,r2' FOLLOWERS = 1 SCHEDULE = 'MAJORITY_IN_PRIMARY'"
    );

    let resource = vec![
        ResourceGroupOption::ru_per_sec(500),
        ResourceGroupOption::burstable(BurstableType::Moderated),
        ResourceGroupOption::priority(ResourceGroupPriority::High),
    ];
    assert_eq!(
        restore_resource_group_options(&resource),
        "RU_PER_SEC = 500, BURSTABLE = MODERATED, PRIORITY = HIGH"
    );
}
