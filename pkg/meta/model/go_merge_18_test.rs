// Copyright 2026 AsterSQL.

use crate::group_1::*;
use crate::group_2::serde_json;

#[test]
fn go_merge_18_storage_class_metadata_round_trip_and_clone() {
    let rule = StorageClassTransitRule {
        Tier: "IA".to_owned(),
        AfterDays: 30,
        AfterSeconds: 0,
    };
    let table = TableInfo {
        StorageClassTier: "STANDARD".to_owned(),
        StorageClassTransitions: vec![rule.clone()],
        Partition: Some(PartitionInfo {
            Definitions: vec![PartitionDefinition {
                StorageClassTransitions: vec![rule],
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let encoded = serde_json::to_value(&table).unwrap();
    assert_eq!(encoded["storage_class_transitions"][0]["after_seconds"], 0);
    assert_eq!(
        encoded["partition"]["definitions"][0]["storage_class_transitions"][0]["after_seconds"],
        0
    );
    assert_eq!(
        table.StorageClassString(),
        r#"{"tier":"STANDARD","transitions":[{"tier":"IA","after_days":30,"after_seconds":0}]}"#
    );
    let mut cloned = table.Clone();
    cloned.StorageClassTransitions[0].Tier = "STANDARD".to_owned();
    cloned.StorageClassTransitions[0].AfterSeconds = 17;
    assert_eq!(
        serde_json::to_value(&cloned).unwrap()["storage_class_transitions"][0]["after_seconds"],
        17
    );
    assert_eq!(table.StorageClassTransitions[0].Tier, "IA");
    assert_eq!(
        cloned.Partition.as_ref().unwrap().Definitions[0].StorageClassString(),
        r#"{"tier":"","transitions":[{"tier":"IA","after_days":30,"after_seconds":0}]}"#
    );
    assert_eq!(StarterDefaultTTLJobInterval, "15m");
    assert_eq!(
        duration::ParseDuration(StarterDefaultTTLJobInterval)
            .unwrap()
            .as_secs(),
        900
    );
}

#[test]
fn go_merge_18_materialized_view_metadata_round_trip_and_clone() {
    let table = TableInfo {
        MaterializedViewBase: Some(MaterializedViewBaseInfo {
            MLogID: 9,
            MViewIDs: vec![2, 3],
        }),
        MaterializedView: Some(MaterializedViewInfo {
            BaseTableIDs: vec![1, 2],
            SQLContent: "select 1".to_owned(),
            DefinitionSQLMode: mysql::ModeANSIQuotes,
            RefreshScheduleSQLMode: mysql::ModePipesAsConcat,
            DefinitionDivPrecisionIncrement: 4,
            DefinitionTimeZone: TimeZoneLocation {
                name: "UTC".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        }),
        MaterializedViewShadow: Some(MaterializedViewShadowInfo { SourceMViewID: 3 }),
        MaterializedViewLog: Some(MaterializedViewLogInfo {
            BaseTableID: 1,
            DependentMViewIDs: vec![2],
            LogAccumulationAlertRows: Some(7),
            ..Default::default()
        }),
        ..Default::default()
    };
    table
        .MaterializedView
        .as_ref()
        .unwrap()
        .DefinitionTimeZone
        .get_location()
        .unwrap();
    let mut clone = table.Clone();
    clone.MaterializedViewBase.as_mut().unwrap().MViewIDs[0] = 99;
    clone.MaterializedView.as_mut().unwrap().BaseTableIDs[0] = 99;
    clone
        .MaterializedViewLog
        .as_mut()
        .unwrap()
        .DependentMViewIDs[0] = 99;
    clone
        .MaterializedViewLog
        .as_mut()
        .unwrap()
        .LogAccumulationAlertRows = Some(9);
    assert_eq!(
        table.MaterializedViewBase.as_ref().unwrap().MViewIDs,
        vec![2, 3]
    );
    assert_eq!(
        table.MaterializedView.as_ref().unwrap().BaseTableIDs,
        vec![1, 2]
    );
    assert_eq!(
        table
            .MaterializedViewLog
            .as_ref()
            .unwrap()
            .DependentMViewIDs,
        vec![2]
    );
    let zone = clone
        .MaterializedView
        .as_ref()
        .unwrap()
        .DefinitionTimeZone
        .get_location()
        .unwrap();
    assert_eq!(zone.name, "UTC");
    assert_eq!(
        table
            .MaterializedViewLog
            .as_ref()
            .unwrap()
            .LogAccumulationAlertRows,
        Some(7)
    );
    let encoded = serde_json::to_value(&table).unwrap();
    assert_eq!(encoded["materialized_view"]["definition_sql_mode"], 4);
    assert_eq!(
        encoded["materialized_view"]["definition_time_zone"]["name"],
        "UTC"
    );
    let decoded: TableInfo = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        decoded
            .MaterializedViewLog
            .unwrap()
            .EffectiveLogAccumulationAlertRows(),
        (7, true)
    );
}

#[test]
fn go_merge_18_materialized_view_state_log_and_name_edges() {
    assert!(MViewInitBuildReady.IsReady());
    assert!(!MViewInitBuildDeferred.IsReady());
    assert_eq!(MViewInitBuildBuilding.to_string(), "building");
    assert_eq!(MViewInitBuildState(255).to_string(), "unknown(255)");
    assert!(
        MViewInitBuildDeferred
            .AccessErrorMessage("v")
            .contains("not ready")
    );
    assert!(
        MViewInitBuildBuilding
            .AccessErrorMessage("v")
            .contains("in progress")
    );
    assert_eq!(MViewInitBuildReady.AccessErrorMessage("v"), "");
    assert_eq!(
        MaterializedViewLogInfo::default().EffectiveLogAccumulationAlertRows(),
        (0, false)
    );
    assert_eq!(
        MaterializedViewLogInfo {
            LogAccumulationAlertRows: Some(0),
            ..Default::default()
        }
        .EffectiveLogAccumulationAlertRows(),
        (0, false)
    );
    let name = ast::NewCIStr(&"中".repeat(64));
    let log_name = MaterializedViewLogTableName(&name);
    assert_eq!(log_name.O.chars().count(), mysql::MaxTableNameLength);
    assert!(log_name.O.starts_with(MaterializedViewLogTableNamePrefix));
}
