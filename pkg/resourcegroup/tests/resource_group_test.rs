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

// Copyright 2026 AsterSQL.

//! 对照 Go `resource_group_test.go` 中可由 Rust 资源组 DDL 转换层真实执行的场景。
//!
//! SQL 执行使用具有真实 Session/Domain 的 `TestKit`；PD/TiKV、runaway 异步刷表和
//! server 生命周期仅在对应生产能力存在时才可进行端到端校验。转换层用例直接对齐
//! Go `TestNewResourceGroupFromOptions` 及 basic/burst/runaway 场景依赖的映射。

use std::sync::Arc;

use astersql_ddl_resourcegroup::{NewGroupFromOptions, ResourceGroupError, ast, model, rmpb};
use astersql_testkit::{Rows, TestKit, mockstore::CreateMockStoreAndDomain};

/// 对齐 Go `TestResourceGroupBasic` 的 `IF NOT EXISTS` 契约：已有组不得被覆盖。
#[test]
fn resource_group_create_if_not_exists_preserves_existing_settings() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);

    tk.MustExec(
        "create resource group x RU_PER_SEC=1000 PRIORITY=LOW",
        Vec::new(),
    );
    tk.MustExec(
        "create resource group if not exists x RU_PER_SEC=10000 PRIORITY=HIGH",
        Vec::new(),
    );
    tk.MustQuery("show warnings", Vec::new())
        .Check(Rows(&["Note 8248 Resource group 'x' already exists"]));
    tk.MustQuery(
        "select name, ru_per_sec, priority from information_schema.resource_groups where name = 'x'",
        Vec::new(),
    )
    .Check(Rows(&["x 1000 LOW"]));
}

/// 对齐 Go `TestResourceGroupBasic` 的生命周期：`DROP` 后 InfoSchema 不再可见。
#[test]
fn resource_group_drop_removes_group_from_information_schema() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);

    tk.MustExec("create resource group x RU_PER_SEC=1000", Vec::new());
    tk.MustExec("drop resource group x", Vec::new());
    tk.MustQuery(
        "select name from information_schema.resource_groups where name = 'x'",
        Vec::new(),
    )
    .Check(Rows(&[]));
}

/// 对齐 Go `TestResourceGroupBasic` 的不存在分支和 `IF EXISTS` Note。
#[test]
fn resource_group_missing_object_errors_and_notes_match_go() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);

    let alter_error = tk.ExecToErr("alter resource group missing RU_PER_SEC=2000");
    assert_eq!(
        alter_error.message(),
        "resource group missing does not exist"
    );
    tk.MustExec(
        "alter resource group if exists missing RU_PER_SEC=2000",
        Vec::new(),
    );
    tk.MustQuery("show warnings", Vec::new())
        .Check(Rows(&["Note 8249 Unknown resource group 'missing'"]));

    let drop_error = tk.ExecToErr("drop resource group missing");
    assert_eq!(
        drop_error.message(),
        "resource group missing does not exist"
    );
    tk.MustExec("drop resource group if exists missing", Vec::new());
}

/// 对照 Go `TestNewResourceGroupFromOptions` 的 RU 正常路径，并保留 burst limit 三种语义。
#[test]
fn resource_group_ru_and_burst_modes_match_go() {
    for (name, rate, priority, burst) in [
        ("test", 2_000, 0, 0),
        ("test", 5_000, 8, 0),
        ("x", 1_000, 16, 1_000),
        ("x", 1_000, 16, -1),
        ("x", 1_000, 16, -2),
    ] {
        let settings = model::ResourceGroupSettings {
            RURate: rate,
            BurstLimit: burst,
            Priority: priority,
            ..Default::default()
        };

        let group = NewGroupFromOptions(name.into(), Some(&settings)).unwrap();
        assert_eq!(group.get_name(), name);
        assert_eq!(group.get_mode(), rmpb::GroupMode::RuMode);
        assert_eq!(group.get_priority(), priority as u32);
        let limit = group.get_r_u_settings().get_r_u().get_settings();
        assert_eq!(limit.get_fill_rate(), rate);
        assert_eq!(limit.get_burst_limit(), burst);
    }
}

/// 对照 Go 的 QUERY_LIMIT 字段：规则、动作、切换组和 watch 必须逐项写入 protobuf。
#[test]
fn resource_group_runaway_fields_match_go_ddl_semantics() {
    let settings = model::ResourceGroupSettings {
        RURate: 2_000,
        Runaway: Some(Arc::new(model::ResourceGroupRunawaySettings {
            ExecElapsedTimeMs: 15_000,
            ProcessedKeys: 100,
            RequestUnit: 300,
            Action: ast::RunawayActionSwitchGroup,
            SwitchGroupName: "fallback".into(),
            WatchType: ast::WatchSimilar,
            WatchDurationMs: 600_000,
        })),
        ..Default::default()
    };

    let group = NewGroupFromOptions("x".into(), Some(&settings)).unwrap();
    let runaway = group.get_runaway_settings();
    assert_eq!(runaway.get_rule().get_exec_elapsed_time_ms(), 15_000);
    assert_eq!(runaway.get_rule().get_processed_keys(), 100);
    assert_eq!(runaway.get_rule().get_request_unit(), 300);
    assert_eq!(runaway.get_action(), rmpb::RunawayAction::SwitchGroup);
    assert_eq!(runaway.get_switch_group_name(), "fallback");
    assert_eq!(
        runaway.get_watch().get_type(),
        rmpb::RunawayWatchType::Similar
    );
    assert_eq!(runaway.get_watch().get_lasting_duration_ms(), 600_000);
}

/// 对照 Go 表驱动用例的 raw-mode、重复模式、名称长度和 runaway 失败分支。
#[test]
fn resource_group_options_reject_all_go_invalid_cases() {
    for settings in [
        model::ResourceGroupSettings {
            CPULimiter: "8".into(),
            IOReadBandwidth: "3000MB/s".into(),
            IOWriteBandwidth: "3000Mi".into(),
            ..Default::default()
        },
        model::ResourceGroupSettings {
            CPULimiter: "8c".into(),
            IOReadBandwidth: "3000Mi".into(),
            IOWriteBandwidth: "3000Mi".into(),
            ..Default::default()
        },
        model::ResourceGroupSettings {
            CPULimiter: "8".into(),
            IOReadBandwidth: "3000G".into(),
            IOWriteBandwidth: "3000MB".into(),
            ..Default::default()
        },
    ] {
        assert_eq!(
            NewGroupFromOptions("test".into(), Some(&settings)),
            Err(ResourceGroupError::UnknownResourceGroupMode)
        );
    }

    let duplicate_mode = model::ResourceGroupSettings {
        RURate: 1_000,
        CPULimiter: "8".into(),
        IOReadBandwidth: "3000Mi".into(),
        IOWriteBandwidth: "3000Mi".into(),
        ..Default::default()
    };
    assert_eq!(
        NewGroupFromOptions("test".into(), Some(&duplicate_mode)),
        Err(ResourceGroupError::InvalidResourceGroupDuplicatedMode)
    );
    assert_eq!(
        NewGroupFromOptions("x".repeat(33), Some(&duplicate_mode)),
        Err(ResourceGroupError::TooLongResourceGroupName)
    );

    let missing_switch_target = model::ResourceGroupSettings {
        Runaway: Some(Arc::new(model::ResourceGroupRunawaySettings {
            ExecElapsedTimeMs: 1_000,
            Action: ast::RunawayActionSwitchGroup,
            ..Default::default()
        })),
        ..Default::default()
    };
    assert_eq!(
        NewGroupFromOptions("test".into(), Some(&missing_switch_target)),
        Err(ResourceGroupError::UnknownResourceGroupRunawaySwitchGroupName)
    );
}

/// 空设置与非法 runaway 配置必须保留 Go 哨兵错误的分类和检查顺序。
#[test]
fn resource_group_validation_rejects_go_invalid_cases() {
    assert_eq!(
        NewGroupFromOptions("x".into(), None),
        Err(ResourceGroupError::InvalidGroupSettings)
    );
    let empty = model::ResourceGroupSettings::default();
    assert_eq!(
        NewGroupFromOptions("x".into(), Some(&empty)),
        Err(ResourceGroupError::UnknownResourceGroupMode)
    );

    let empty_runaway = model::ResourceGroupSettings {
        RURate: 1_000,
        Runaway: Some(Arc::new(model::ResourceGroupRunawaySettings {
            Action: ast::RunawayActionKill,
            ..Default::default()
        })),
        ..Default::default()
    };
    assert_eq!(
        NewGroupFromOptions("x".into(), Some(&empty_runaway)),
        Err(ResourceGroupError::ResourceGroupRunawayRuleIsEmpty)
    );

    let no_action = model::ResourceGroupSettings {
        RURate: 1_000,
        Runaway: Some(Arc::new(model::ResourceGroupRunawaySettings {
            ExecElapsedTimeMs: 1,
            ..Default::default()
        })),
        ..Default::default()
    };
    assert_eq!(
        NewGroupFromOptions("x".into(), Some(&no_action)),
        Err(ResourceGroupError::UnknownResourceGroupRunawayAction)
    );
}
