// Copyright 2026 AsterSQL.
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

// Job 及相关模型与 Go 行为对齐的完整单元测试。

use crate::group_2::serde_json;
use crate::group_3::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn job_action_state_and_rollback_rules_match_go() {
    assert_eq!(action_type_string(ACTION_ADD_INDEX), "add index");
    assert_eq!(action_type_string(200), "none");
    assert_eq!(
        modify_type_to_string(MODIFY_TYPE_REORG),
        "reorg row and index"
    );
    assert_eq!(str_to_job_state("rollback done"), JobState::RollbackDone);
    assert_eq!(str_to_job_state("unknown"), JobState::None);

    let mut job = Job::default();
    job.tp = ACTION_MODIFY_COLUMN;
    job.state = JobState::Running;
    job.need_reorg = true;
    job.schema_state = SchemaState::WriteOnly;
    assert!(job.may_need_reorg());
    assert!(job.is_rollbackable());
    assert!(job.is_pausable());
    job.schema_state = SchemaState::Public;
    assert!(!job.is_rollbackable());
    assert!(!job.is_pausable());
}

#[test]
fn job_pause_and_involving_schema_rules_match_go() {
    let mut job = Job::default();
    job.state = JobState::Paused;
    job.admin_operator = AdminCommandOperator::System;
    job.set_pause_reason(
        JOB_PAUSE_REASON_KV_DISK_FULL.to_owned(),
        "disk full".to_owned(),
    );
    assert!(job.is_paused_by_system_for_kv_disk_full());

    job.schema_name = "TeStDB".to_owned();
    job.table_name = "TaBlE".to_owned();
    job.normalize_involving_schema_info();
    assert_eq!(job.schema_name, "testdb");
    assert_eq!(job.table_name, "table");
    assert!(job.check_involving_schema_info().is_ok());

    job.involving_schema_info = vec![InvolvingSchemaInfo {
        database: "*".to_owned(),
        table: "t".to_owned(),
        ..Default::default()
    }];
    assert!(job.check_involving_schema_info().is_err());
}

#[test]
fn job_json_round_trip_and_clone_preserve_persisted_state() {
    let mut job = Job::default();
    job.id = 42;
    job.tp = ACTION_ADD_INDEX;
    job.schema_name = "db".to_owned();
    job.table_name = "t".to_owned();
    job.version = JobVersion::V1;
    job.set_row_count(7);
    job.reorg_meta = Some(DDLReorgMeta::default());
    let mut warnings = HashMap::new();
    warnings.insert(11, "warning".to_owned());
    job.set_warnings(warnings, HashMap::from([(11, 2)]));
    job.raw_args = serde_json::to_vec(&vec![serde_json::json!(1)]).unwrap();
    let bytes = job.encode(false).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["type"], ACTION_ADD_INDEX);
    assert_eq!(json["row_count"], 7);
    assert!(json.get("tp").is_none());
    let decoded = Job::decode(&bytes).unwrap();
    assert_eq!(decoded.id, 42);
    assert_eq!(decoded.get_row_count(), 7);
    assert_eq!(decoded.raw_args, job.raw_args);
    assert_eq!(decoded.get_warnings().0.get(&11).unwrap(), "warning");
    assert_eq!(decoded.get_warnings().1.get(&11), Some(&2));

    let cloned = job.clone_job().unwrap();
    assert_eq!(cloned.id, job.id);
    assert_eq!(cloned.get_row_count(), 7);
}

#[test]
fn placement_rendering_matches_go_order_escaping_and_duration() {
    let settings = PlacementSettings {
        PrimaryRegion: "us\"east".to_owned(),
        Regions: "us-east,us-west".to_owned(),
        Voters: 3,
        Followers: 2,
        ..Default::default()
    };
    assert_eq!(
        settings.String(),
        "PRIMARY_REGION=\"us\\\"east\" REGIONS=\"us-east,us-west\" VOTERS=3 FOLLOWERS=2"
    );

    let mut rendered = String::new();
    writeSettingDurationToBuilder(
        &mut rendered,
        "DURATION",
        Duration::from_millis(1500),
        &mut [],
    );
    assert_eq!(rendered, "DURATION=\"1.5s\"");
}

#[test]
fn resource_group_rendering_adjustment_and_clone_match_go() {
    let runaway = Arc::new(ResourceGroupRunawaySettings {
        ExecElapsedTimeMs: 1500,
        ProcessedKeys: 10,
        RequestUnit: 20,
        Action: ast::RunawayActionSwitchGroup,
        SwitchGroupName: "oltp".to_owned(),
        WatchType: ast::WatchExact,
        WatchDurationMs: 0,
    });
    let mut settings = NewResourceGroupSettings();
    settings.RURate = 100;
    settings.BurstLimit = 0;
    settings.Runaway = Some(runaway.clone());
    settings.Adjust();
    assert_eq!(settings.BurstLimit, 100);
    assert_eq!(settings.GetBurstLimitAdjusted(), 100);
    assert!(settings.String().contains("EXEC_ELAPSED=\"1.5s\""));
    assert!(settings.String().contains("ACTION=SWITCH_GROUP(oltp)"));
    assert!(settings.String().contains("DURATION=UNLIMITED"));
    assert!(Arc::ptr_eq(
        settings.Clone().Runaway.as_ref().unwrap(),
        &runaway
    ));
}

#[test]
fn reorg_state_atomic_settings_and_json_match_go() {
    assert_eq!(
        BackfillState::BackfillStateReadyToMerge.String(),
        "backfill state ready to merge"
    );
    assert!(ReorgType::ReorgTypeIngest.NeedMergeProcess());
    assert!(!ReorgType::ReorgTypeTxn.NeedMergeProcess());

    let meta = DDLReorgMeta::default();
    meta.SetConcurrency(8);
    meta.SetBatchSize(256);
    meta.SetMaxWriteSpeed(1024);
    assert_eq!(meta.GetConcurrency(), 8);
    assert_eq!(meta.GetBatchSize(), 256);
    assert_eq!(meta.GetMaxWriteSpeed(), 1024);

    let mut backfill = BackfillMeta::default();
    backfill.RowCount = 9;
    backfill.StartKey = vec![1, 2];
    let encoded = backfill.Encode().unwrap();
    let mut decoded = BackfillMeta::default();
    decoded.Decode(&encoded).unwrap();
    assert_eq!(decoded.RowCount, 9);
    assert_eq!(decoded.StartKey, vec![1, 2]);
}

#[test]
fn masking_policy_and_table_mode_values_match_go() {
    assert_eq!(
        MaskingPolicyStatus::MaskingPolicyStatusDisable.String(),
        "DISABLED"
    );
    assert_eq!(MaskingPolicyTypeMaskPartial, "MASK_PARTIAL");

    let mut policy = MaskingPolicyInfo::default();
    policy.ID = 5;
    policy.Expression = "mask(col)".to_owned();
    let clone = policy.Clone();
    assert_eq!(clone.ID, 5);
    assert_eq!(clone.Expression, "mask(col)");
    let json = serde_json::to_value(&policy).unwrap();
    assert_eq!(json["id"], 5);
    assert!(json.get("ID").is_none());

    assert_eq!(TableMode::TableModeNormal.String(), "Normal");
    assert!(TableMode::TableModeNormal.CanTransitionTo(TableMode::TableModeImport));
    assert!(!TableMode::TableModeImport.CanTransitionTo(TableMode::TableModeRestore));
}
use crate::group_3::{
    ACTION_ADD_INDEX, ACTION_MODIFY_COLUMN, Job, JobState, SchemaState, action_type_string,
};

#[test]
/// 校验动作展示名、改列在 WriteOnly 下的 reorg 与可回滚性。
fn job_action_state_and_reorg_rules_match_go() {
    assert_eq!(action_type_string(ACTION_ADD_INDEX), "add index");
    let mut job = Job::default();
    job.tp = ACTION_MODIFY_COLUMN;
    job.state = JobState::Running;
    job.need_reorg = true;
    job.schema_state = SchemaState::WriteOnly;
    assert!(job.may_need_reorg());
    assert!(job.is_rollbackable());
}
