// Copyright 2026 AsterSQL.

use crate::group_2::serde_json;
use crate::group_3::{
    NewResourceGroupSettings, ResourceGroupBackgroundSettings, ResourceGroupInfo,
    ResourceGroupRunawaySettings, ResourceGroupSettings, ast, unlimitedRURate,
};
use std::sync::Arc;

#[test]
fn resource_group_background_uses_go_json_field_name() {
    let info = ResourceGroupInfo {
        ResourceGroupSettings: ResourceGroupSettings {
            Runaway: Some(Arc::new(ResourceGroupRunawaySettings {
                ExecElapsedTimeMs: 1_500,
                ProcessedKeys: 2,
                RequestUnit: 3,
                ..Default::default()
            })),
            Background: Some(Arc::new(ResourceGroupBackgroundSettings {
                JobTypes: vec!["ddl".into()],
                ResourceUtilLimit: 75,
            })),
            ..Default::default()
        },
        ..Default::default()
    };

    let json = serde_json::to_value(info).expect("serialize resource group info");
    let background = &json["background"];
    assert_eq!(background["utilization_limit"], 75, "{json}");
    assert!(background.get("resource_util_limit").is_none());
    assert_eq!(background["job_types"][0], "ddl");
    assert_eq!(json["runaway"]["exec_elapsed_time_ms"], 1_500);
    assert_eq!(json["runaway"]["processed_keys"], 2);
    assert_eq!(json["runaway"]["request_unit"], 3);
    for go_key in [
        "priority",
        "burst_limit",
        "runaway",
        "background",
        "name",
        "state",
    ] {
        assert!(json.get(go_key).is_some(), "missing {go_key} in {json}");
    }
}

#[test]
fn resource_group_formats_every_go_branch() {
    let settings = ResourceGroupSettings {
        RURate: 100,
        Priority: ast::MediumPriorityValue,
        CPULimiter: "8C".into(),
        IOReadBandwidth: "1GB/s".into(),
        IOWriteBandwidth: "2GB/s".into(),
        BurstLimit: -2,
        Runaway: Some(Arc::new(ResourceGroupRunawaySettings {
            ExecElapsedTimeMs: 1500,
            ProcessedKeys: 20,
            RequestUnit: 30,
            Action: ast::RunawayActionSwitchGroup,
            SwitchGroupName: "quarantine".into(),
            WatchType: ast::WatchExact,
            WatchDurationMs: 2_000,
        })),
        Background: Some(Arc::new(ResourceGroupBackgroundSettings {
            JobTypes: vec!["lightning".into(), "ddl".into()],
            ResourceUtilLimit: 75,
        })),
    };

    assert_eq!(
        settings.String(),
        "RU_PER_SEC=100, PRIORITY=MEDIUM, CPU=\"8C\", IO_READ_BANDWIDTH=\"1GB/s\", IO_WRITE_BANDWIDTH=\"2GB/s\", BURSTABLE(MODERATED), QUERY_LIMIT=(EXEC_ELAPSED=\"1.5s\" PROCESSED_KEYS=20 RU=30 ACTION=SWITCH_GROUP(quarantine) WATCH=EXACT DURATION=\"2s\"), BACKGROUND=(TASK_TYPES='lightning,ddl', UTILIZATION_LIMIT=75)"
    );
}

#[test]
fn resource_group_adjust_and_clone_match_go_pointer_semantics() {
    let mut regular = NewResourceGroupSettings();
    regular.RURate = 100;
    regular.BurstLimit = 0;
    regular.Adjust();
    assert_eq!(regular.BurstLimit, 100);

    regular.RURate = unlimitedRURate;
    regular.BurstLimit = 7;
    regular.Adjust();
    assert_eq!(regular.BurstLimit, 7);
    assert_eq!(regular.GetBurstLimitAdjusted(), -1);

    let runaway = Arc::new(ResourceGroupRunawaySettings::default());
    regular.Runaway = Some(runaway.clone());
    let cloned = regular.Clone();
    assert!(Arc::ptr_eq(cloned.Runaway.as_ref().unwrap(), &runaway));
}
