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

// 资源组（Resource Group）迁移期单元测试。
//
// 对照 Go 包行为，验证 `NewGroupFromOptions` 的错误顺序、
// RU 模式 protobuf 字段形状、Runaway/Background 字段映射，
// 以及错误文案与 Go 哨兵错误一致。

use std::sync::Arc;

use protobuf::Message;

use super::{MAX_GROUP_NAME_LENGTH, NewGroupFromOptions, ResourceGroupError, ast, model};

/// 校验 Resource Manager 引用的 KeyspaceIdentity 默认值与 protobuf 往返。
#[test]
fn keyspace_identity_default_and_round_trip() {
    let default_identity = super::apipb::KeyspaceIdentity::new();
    assert_eq!(default_identity.get_namespace_id(), 0);
    assert_eq!(default_identity.get_keyspace_id(), 0);

    let mut identity = super::apipb::KeyspaceIdentity::new();
    identity.set_namespace_id(42);
    identity.set_keyspace_id(7);

    let mut value = super::rmpb::KeyspaceIdValue::new();
    value.set_keyspace_identity(identity);
    let encoded = value.write_to_bytes().expect("serialize keyspace identity");
    let decoded: super::rmpb::KeyspaceIdValue =
        protobuf::parse_from_bytes(&encoded).expect("deserialize keyspace identity");

    assert_eq!(decoded.get_keyspace_identity().get_namespace_id(), 42);
    assert_eq!(decoded.get_keyspace_identity().get_keyspace_id(), 7);
}

/// 校验空设置、未知模式、超长名称、RU/Raw 模式冲突等错误路径。
#[test]
fn validation_errors_match_go_behavior() {
    assert_eq!(
        NewGroupFromOptions("test".to_owned(), None),
        Err(ResourceGroupError::InvalidGroupSettings)
    );
    assert_eq!(
        NewGroupFromOptions(
            "test".to_owned(),
            Some(&model::ResourceGroupSettings::default())
        ),
        Err(ResourceGroupError::UnknownResourceGroupMode)
    );

    // Go `TestNewResourceGroupFromOptions` covers these three Raw-mode
    // combinations. Raw mode remains unsupported regardless of whether the
    // spelling would otherwise be accepted by its parser.
    for raw_settings in [
        model::ResourceGroupSettings {
            CPULimiter: "8".to_owned(),
            IOReadBandwidth: "3000MB/s".to_owned(),
            IOWriteBandwidth: "3000Mi".to_owned(),
            ..Default::default()
        },
        model::ResourceGroupSettings {
            CPULimiter: "8c".to_owned(),
            IOReadBandwidth: "3000Mi".to_owned(),
            IOWriteBandwidth: "3000Mi".to_owned(),
            ..Default::default()
        },
        model::ResourceGroupSettings {
            CPULimiter: "8".to_owned(),
            IOReadBandwidth: "3000G".to_owned(),
            IOWriteBandwidth: "3000MB".to_owned(),
            ..Default::default()
        },
    ] {
        assert_eq!(
            NewGroupFromOptions("test".to_owned(), Some(&raw_settings)),
            Err(ResourceGroupError::UnknownResourceGroupMode)
        );
    }

    let ru_settings = model::ResourceGroupSettings {
        RURate: 1_000,
        ..Default::default()
    };
    assert_eq!(
        NewGroupFromOptions("x".repeat(MAX_GROUP_NAME_LENGTH + 1), Some(&ru_settings)),
        Err(ResourceGroupError::TooLongResourceGroupName)
    );

    // 同时设置 RURate 与 Raw 模式 CPU/IO 限流应报模式冲突。
    let duplicated = model::ResourceGroupSettings {
        RURate: 1_000,
        CPULimiter: "8".to_owned(),
        IOReadBandwidth: "3000Mi".to_owned(),
        IOWriteBandwidth: "3000Mi".to_owned(),
        ..Default::default()
    };
    assert_eq!(
        NewGroupFromOptions("test".to_owned(), Some(&duplicated)),
        Err(ResourceGroupError::InvalidResourceGroupDuplicatedMode)
    );
}

/// 校验 RU 模式下名称、优先级、fill_rate、burst_limit 与 Go protobuf 一致。
#[test]
fn ru_mode_matches_go_protobuf_shape() {
    for (ru_rate, priority, burst_limit) in [(2_000, 0, 0), (5_000, 8, 0), (5_000, 8, -2)] {
        let settings = model::ResourceGroupSettings {
            RURate: ru_rate,
            Priority: priority,
            BurstLimit: burst_limit,
            ..Default::default()
        };

        let group = NewGroupFromOptions("test".to_owned(), Some(&settings)).unwrap();
        assert_eq!(group.get_name(), "test");
        assert_eq!(group.get_mode(), super::rmpb::GroupMode::RuMode);
        assert_eq!(group.get_priority(), priority as u32);
        let token_limit = group.get_r_u_settings().get_r_u().get_settings();
        assert_eq!(token_limit.get_fill_rate(), ru_rate);
        assert_eq!(token_limit.get_burst_limit(), burst_limit);
    }
}

/// 校验 Runaway 规则/动作/Watch 与 Background 任务类型、利用率字段映射。
#[test]
fn runaway_and_background_settings_match_go_fields() {
    let settings = model::ResourceGroupSettings {
        RURate: 2_000,
        Priority: 16,
        Runaway: Some(Arc::new(model::ResourceGroupRunawaySettings {
            ExecElapsedTimeMs: 1_500,
            ProcessedKeys: 100,
            RequestUnit: 300,
            Action: ast::RunawayActionSwitchGroup,
            SwitchGroupName: "fallback".to_owned(),
            WatchType: ast::WatchExact,
            WatchDurationMs: 60_000,
        })),
        Background: Some(Arc::new(model::ResourceGroupBackgroundSettings {
            JobTypes: vec!["br".to_owned(), "ddl".to_owned()],
            ResourceUtilLimit: 30,
        })),
        ..Default::default()
    };

    let group = NewGroupFromOptions("test".to_owned(), Some(&settings)).unwrap();
    let runaway = group.get_runaway_settings();
    let rule = runaway.get_rule();
    assert_eq!(rule.get_exec_elapsed_time_ms(), 1_500);
    assert_eq!(rule.get_processed_keys(), 100);
    assert_eq!(rule.get_request_unit(), 300);
    assert_eq!(
        runaway.get_action(),
        super::rmpb::RunawayAction::SwitchGroup
    );
    assert_eq!(runaway.get_switch_group_name(), "fallback");
    assert_eq!(
        runaway.get_watch().get_type(),
        super::rmpb::RunawayWatchType::Exact
    );
    assert_eq!(runaway.get_watch().get_lasting_duration_ms(), 60_000);

    let background = group.get_background_settings();
    assert_eq!(background.get_job_types(), &["br", "ddl"]);
    assert_eq!(background.get_utilization_limit(), 30);
}

/// 校验 Runaway 校验顺序：空规则 → 未知动作 → 空 SwitchGroup 名。
#[test]
fn runaway_validation_matches_go_order() {
    let empty_rule = model::ResourceGroupSettings {
        Runaway: Some(Arc::new(model::ResourceGroupRunawaySettings {
            Action: ast::RunawayActionKill,
            ..Default::default()
        })),
        ..Default::default()
    };
    assert_eq!(
        NewGroupFromOptions("test".to_owned(), Some(&empty_rule)),
        Err(ResourceGroupError::ResourceGroupRunawayRuleIsEmpty)
    );

    let no_action = model::ResourceGroupSettings {
        Runaway: Some(Arc::new(model::ResourceGroupRunawaySettings {
            ExecElapsedTimeMs: 1,
            ..Default::default()
        })),
        ..Default::default()
    };
    assert_eq!(
        NewGroupFromOptions("test".to_owned(), Some(&no_action)),
        Err(ResourceGroupError::UnknownResourceGroupRunawayAction)
    );

    let empty_switch_group = model::ResourceGroupSettings {
        Runaway: Some(Arc::new(model::ResourceGroupRunawaySettings {
            ExecElapsedTimeMs: 1,
            Action: ast::RunawayActionSwitchGroup,
            ..Default::default()
        })),
        ..Default::default()
    };
    assert_eq!(
        NewGroupFromOptions("test".to_owned(), Some(&empty_switch_group)),
        Err(ResourceGroupError::UnknownResourceGroupRunawaySwitchGroupName)
    );
}

/// 校验各错误变体的 `Display` 文案与 Go 哨兵错误字符串一致。
#[test]
fn error_messages_match_go_sentinels() {
    let cases = [
        (
            ResourceGroupError::InvalidGroupSettings,
            "invalid group settings",
        ),
        (
            ResourceGroupError::TooLongResourceGroupName,
            "resource group name too long",
        ),
        (
            ResourceGroupError::InvalidResourceGroupFormat,
            "group settings with invalid format",
        ),
        (
            ResourceGroupError::InvalidResourceGroupDuplicatedMode,
            "cannot set RU mode and Raw mode options at the same time",
        ),
        (
            ResourceGroupError::UnknownResourceGroupMode,
            "unknown resource group mode",
        ),
        (
            ResourceGroupError::DroppingInternalResourceGroup,
            "can't drop reserved resource group",
        ),
        (
            ResourceGroupError::ResourceGroupRunawayRuleIsEmpty,
            "please set at least one field(exec_elapsed_time_ms, processed_keys, ru)",
        ),
        (
            ResourceGroupError::UnknownResourceGroupRunawayAction,
            "unknown resource group runaway action",
        ),
        (
            ResourceGroupError::UnknownResourceGroupRunawaySwitchGroupName,
            "unknown resource group runaway switch group name",
        ),
    ];

    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
}
