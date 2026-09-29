// Copyright 2026 AsterSQL.

use super::group_1::{
    EngineAttribute, FlagEnableTiKVShortCircuitExpression, ParseEngineAttributeFromString,
    RegionSplitPolicy, StorageClassDef, StorageClassTierIA, StorageClassTransitRule,
    VectorIndexInfo, VectorIndexKindHNSW, buildStorageClassString,
};
use super::group_2 as args;
use super::group_3::*;
use args::JobArgsCompat;
use args::{serde, serde_json};

fn round_trip<A>(value: A, tp: ActionType, decode: fn(&mut Job) -> args::JobArgResult<A>)
where
    A: args::JobArgs + Clone + PartialEq + std::fmt::Debug + serde::Serialize + 'static,
{
    for version in [JobVersion::V1, JobVersion::V2] {
        let mut job = Job {
            tp,
            version,
            ..Default::default()
        };
        job.FillArgs(value.clone());
        let bytes = job.Encode(true).expect("encode job");
        let mut decoded = Job::decode(&bytes).expect("decode job");
        assert_eq!(decode(&mut decoded).expect("decode args"), value);
    }
}

#[test]
fn go_merge_15_materialized_view_job_states() {
    let mut job = Job {
        tp: ACTION_CREATE_MATERIALIZED_VIEW,
        ..Default::default()
    };
    assert!(job.may_need_reorg());
    assert!(job.is_rollbackable());
    job.schema_state = SchemaState::WriteReorganization;
    assert!(job.is_rollbackable());
    job.schema_state = SchemaState::Public;
    assert!(!job.is_rollbackable());
    for tp in [
        ACTION_DROP_MATERIALIZED_VIEW,
        ACTION_DROP_MATERIALIZED_VIEW_LOG,
        ACTION_DROP_MATERIALIZED_VIEW_SHADOW,
    ] {
        let mut drop_job = Job {
            tp,
            ..Default::default()
        };
        assert!(!drop_job.is_rollbackable());
        drop_job.schema_state = SchemaState::Public;
        assert!(drop_job.is_rollbackable());
    }
    let mut cutover = Job {
        tp: ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER,
        ..Default::default()
    };
    assert!(cutover.is_rollbackable());
    cutover.schema_state = SchemaState::Public;
    assert!(!cutover.is_rollbackable());
    assert_eq!(
        action_type_string(ACTION_CREATE_MATERIALIZED_VIEW_SHADOW),
        "create materialized view shadow table"
    );
    assert_eq!(
        action_type_string(ACTION_DROP_MATERIALIZED_VIEW_LOG),
        "drop materialized view log"
    );
    let multi = Job {
        tp: ACTION_MULTI_SCHEMA_CHANGE,
        multi_schema_info: Some(MultiSchemaInfo {
            sub_jobs: vec![SubJob {
                tp: ACTION_CREATE_MATERIALIZED_VIEW,
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(multi.may_need_reorg());
}

#[test]
fn go_merge_15_job_ru_and_sub_job_metadata() {
    let mut parent = Job {
        ru: 12.5,
        ..Default::default()
    };
    let bytes = parent.Encode(true).unwrap();
    assert_eq!(Job::decode(&bytes).unwrap().ru, 12.5);
    let mut zero = Job::default();
    assert!(
        !String::from_utf8(zero.Encode(true).unwrap())
            .unwrap()
            .contains("\"ru\"")
    );

    let info = InvolvingSchemaInfo {
        database: "db".into(),
        table: "t".into(),
        ..Default::default()
    };
    let sub = SubJob {
        involving_schema_info: vec![info.clone()],
        ..Default::default()
    };
    let proxy = sub.to_proxy_job(&parent, 0);
    assert_eq!(proxy.ru, 12.5);
    assert_eq!(proxy.involving_schema_info[0].database, info.database);
    let mut updated = SubJob::default();
    updated.from_proxy_job(&proxy, 10);
    assert_eq!(updated.involving_schema_info[0].database, info.database);
    let sub_json = serde_json::to_value(&sub).unwrap();
    assert_eq!(sub_json["involving_schema_info"][0]["database"], "db");
    let empty_json = serde_json::to_value(SubJob::default()).unwrap();
    assert!(empty_json.get("involving_schema_info").is_none());
    let cloned = parent.clone_job().unwrap();
    assert_eq!(cloned.ru, 12.5);
}

#[test]
fn go_merge_15_timezone_clone_preserves_cached_location() {
    let original = TimeZoneLocation {
        name: "UTC".into(),
        offset: 3600,
        location: std::sync::RwLock::new(None),
    };
    let cached = original.get_location().unwrap();
    let cloned = original.clone();
    assert_eq!(cloned.name, "UTC");
    assert_eq!(cloned.offset, 3600);
    assert!(std::sync::Arc::ptr_eq(
        &cached,
        &cloned.get_location().unwrap()
    ));
}

#[test]
fn go_merge_15_engine_attribute_and_index_json() {
    assert_eq!(
        ParseEngineAttributeFromString("").unwrap(),
        EngineAttribute::default()
    );
    assert!(ParseEngineAttributeFromString("{").is_err());
    let attr = ParseEngineAttributeFromString(r#"{"storage_class":{"defs":[]}}"#).unwrap();
    assert!(attr.StorageClass.is_some());
    let raw_null = ParseEngineAttributeFromString(r#"{"storage_class":null}"#).unwrap();
    assert_eq!(raw_null.StorageClass.unwrap().get(), "null");
    let raw_spaced =
        ParseEngineAttributeFromString(r#"{"storage_class": { "tier" : "IA" }}"#).unwrap();
    assert_eq!(
        raw_spaced.StorageClass.unwrap().get(),
        r#"{ "tier" : "IA" }"#
    );
    assert_eq!(
        ParseEngineAttributeFromString("null").unwrap(),
        EngineAttribute::default()
    );
    assert!(StorageClassDef::default().HasNoScopeDef());
    assert!(
        StorageClassDef {
            NamesIn: Some(vec![]),
            ..Default::default()
        }
        .HasNoScopeDef()
    );
    assert!(
        !StorageClassDef {
            NamesIn: Some(vec!["p0".into()]),
            ..Default::default()
        }
        .HasNoScopeDef()
    );
    assert!(
        !StorageClassDef {
            ValuesIn: Some(vec!["v0".into()]),
            ..Default::default()
        }
        .HasNoScopeDef()
    );
    assert!(
        !StorageClassDef {
            LessThan: Some(String::new()),
            ..Default::default()
        }
        .HasNoScopeDef()
    );
    let transition = StorageClassTransitRule {
        Tier: StorageClassTierIA.into(),
        AfterDays: 2,
        AfterSeconds: 3,
    };
    assert_eq!(transition.TotalSeconds(), 172803);
    assert_eq!(buildStorageClassString("STANDARD", &[]), "STANDARD");
    assert_eq!(
        buildStorageClassString("STANDARD", &[transition]),
        r#"{"tier":"STANDARD","transitions":[{"tier":"IA","after_days":2,"after_seconds":3}]}"#
    );

    assert_eq!(FlagEnableTiKVShortCircuitExpression, 1 << 12);
    let vector = VectorIndexInfo {
        Kind: VectorIndexKindHNSW.into(),
        ..Default::default()
    };
    assert_eq!(serde_json::to_value(&vector).unwrap()["kind"], "HNSW");
    let policy = RegionSplitPolicy {
        TimeZone: "Asia/Shanghai".into(),
        ..Default::default()
    };
    assert_eq!(
        serde_json::to_value(&policy).unwrap()["time_zone"],
        "Asia/Shanghai"
    );
    assert_eq!(policy.Clone(), policy);
}

#[test]
fn go_merge_15_materialized_view_args_both_versions() {
    round_trip(
        args::CreateMaterializedViewLogArgs {
            TableInfo: Some(Box::new(args::TableInfo {
                ID: 11,
                ..Default::default()
            })),
        },
        ACTION_CREATE_MATERIALIZED_VIEW_LOG,
        args::GetCreateMaterializedViewLogArgs,
    );
    round_trip(
        args::CreateMaterializedViewArgs {
            TableInfo: Some(Box::new(args::TableInfo {
                ID: 12,
                MaterializedView: Some(serde_json::json!({"base_table_ids":[88]})),
                Other: std::collections::BTreeMap::from([(
                    "comment".into(),
                    serde_json::json!("mv"),
                )]),
                ..Default::default()
            })),
            MLogTableIDs: vec![13],
        },
        ACTION_CREATE_MATERIALIZED_VIEW,
        args::GetCreateMaterializedViewArgs,
    );
    round_trip(
        args::CreateTableArgs {
            TableInfo: Some(Box::new(args::TableInfo {
                ID: 14,
                MaterializedViewShadow: Some(serde_json::json!({"source_mview_id":88})),
                ..Default::default()
            })),
            FKCheck: true,
            ..Default::default()
        },
        ACTION_CREATE_MATERIALIZED_VIEW_SHADOW,
        args::GetCreateTableArgs,
    );
    for tp in [
        ACTION_DROP_MATERIALIZED_VIEW,
        ACTION_DROP_MATERIALIZED_VIEW_LOG,
        ACTION_DROP_MATERIALIZED_VIEW_SHADOW,
    ] {
        round_trip(
            args::DropTableArgs {
                FKCheck: true,
                ..Default::default()
            },
            tp,
            args::GetDropTableArgs,
        );
        for version in [JobVersion::V1, JobVersion::V2] {
            let original = args::DropTableArgs {
                StartKey: vec![1, 2],
                OldPartitionIDs: vec![3, 4],
                OldRuleIDs: vec!["r".into()],
                ..Default::default()
            };
            let mut job = Job {
                tp,
                version,
                ..Default::default()
            };
            job.FillFinishedArgs(original.clone());
            let bytes = job.Encode(true).unwrap();
            let mut decoded = Job::decode(&bytes).unwrap();
            assert_eq!(
                args::GetFinishedDropTableArgs(&mut decoded).unwrap(),
                original
            );
        }
    }
    round_trip(
        args::AlterMaterializedViewRefreshArgs {
            RefreshMethod: "FAST".into(),
            RefreshScheduleSQLMode: 1,
            UpdateRefreshSchedule: true,
            ..Default::default()
        },
        ACTION_ALTER_MATERIALIZED_VIEW_REFRESH,
        args::GetAlterMaterializedViewRefreshArgs,
    );
    round_trip(
        args::AlterMaterializedViewLogPurgeArgs {
            PurgeMethod: "DEFERRED".into(),
            PurgeScheduleSQLMode: 2,
            UpdatePurgeSchedule: true,
            ..Default::default()
        },
        ACTION_ALTER_MATERIALIZED_VIEW_LOG_PURGE,
        args::GetAlterMaterializedViewLogPurgeArgs,
    );
    round_trip(
        args::AlterMaterializedViewAttributesArgs {
            AlertWarningSec: 10,
            AlertOverdueSec: 20,
            AlertRefreshFailed: true,
        },
        ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES,
        args::GetAlterMaterializedViewAttributesArgs,
    );
    round_trip(
        args::RefreshMaterializedViewCompleteOutOfPlaceCutoverArgs {
            OldMViewID: 1,
            ShadowTableID: 2,
            BuildReadTSO: 3,
            ExpectedOldMViewRevision: Some(4),
            ExpectedLastSuccessReadTSO: 5,
            NextRefreshUnixSeconds: Some(6),
            ShouldUpdateNextRefreshUnixSeconds: true,
            ..Default::default()
        },
        ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER,
        args::GetRefreshMaterializedViewCompleteOutOfPlaceCutoverArgs,
    );
    for should_update in [false, true] {
        round_trip(
            args::RefreshMaterializedViewCompleteOutOfPlaceCutoverArgs {
                OldMViewID: 1,
                ShadowTableID: 2,
                BuildReadTSO: 3,
                ExpectedLastSuccessReadTSONull: true,
                NextRefreshUnixSeconds: None,
                ShouldUpdateNextRefreshUnixSeconds: should_update,
                ..Default::default()
            },
            ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER,
            args::GetRefreshMaterializedViewCompleteOutOfPlaceCutoverArgs,
        );
    }
    round_trip(
        args::ModifyTableEngineAttributeArgs {
            EngineAttribute: "{}".into(),
        },
        ACTION_MODIFY_ENGINE_ATTRIBUTE,
        args::GetModifyTableEngineAttributeArgs,
    );
}

#[test]
fn go_merge_15_legacy_and_v2_only_args() {
    let mut null_table = Job {
        tp: ACTION_CREATE_MATERIALIZED_VIEW,
        version: JobVersion::V1,
        raw_args: br#"[null,null]"#.to_vec(),
        ..Default::default()
    };
    let null_args = args::GetCreateMaterializedViewArgs(&mut null_table).unwrap();
    assert_eq!(null_args.TableInfo.unwrap().ID, 0);
    assert!(null_args.MLogTableIDs.is_empty());
    let mut null_engine = Job {
        tp: ACTION_MODIFY_ENGINE_ATTRIBUTE,
        version: JobVersion::V1,
        raw_args: br#"[null]"#.to_vec(),
        ..Default::default()
    };
    assert_eq!(
        args::GetModifyTableEngineAttributeArgs(&mut null_engine)
            .unwrap()
            .EngineAttribute,
        ""
    );

    let mut legacy = Job {
        tp: ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES,
        version: JobVersion::V1,
        raw_args: br#"[10,20]"#.to_vec(),
        ..Default::default()
    };
    let attr = args::GetAlterMaterializedViewAttributesArgs(&mut legacy).unwrap();
    assert_eq!(attr.AlertWarningSec, 10);
    assert_eq!(attr.AlertOverdueSec, 20);
    assert!(!attr.AlertRefreshFailed);

    for version in [JobVersion::V1, JobVersion::V2] {
        let mut job = Job {
            tp: ACTION_SET_TIFLASH_REPLICA,
            version,
            ..Default::default()
        };
        job.FillArgs(args::SetTiFlashReplicaArgs {
            SkipColumnarStorageGate: true,
            ResetAvailable: true,
            ..Default::default()
        });
        let bytes = job.Encode(true).unwrap();
        let mut decoded = Job::decode(&bytes).unwrap();
        let value = args::GetSetTiFlashReplicaArgs(&mut decoded).unwrap();
        assert_eq!(value.SkipColumnarStorageGate, version == JobVersion::V2);
    }

    for version in [JobVersion::V1, JobVersion::V2] {
        let mut job = Job {
            tp: ACTION_ADD_INDEX,
            version,
            ..Default::default()
        };
        let index = args::IndexArg {
            AutoPreSplit: true,
            ..Default::default()
        };
        let value = serde_json::to_value(&index).unwrap();
        assert_eq!(value["auto_presplit"], true);
        assert!(value.get("split_opt").is_none());
        job.FillArgs(args::ModifyIndexArgs {
            IndexArgs: vec![index],
            ..Default::default()
        });
        let bytes = job.Encode(true).unwrap();
        let mut decoded = Job::decode(&bytes).unwrap();
        let round_tripped = args::GetModifyIndexArgs(&mut decoded).unwrap();
        assert_eq!(
            round_tripped.IndexArgs[0].AutoPreSplit,
            version == JobVersion::V2
        );
        assert!(round_tripped.IndexArgs[0].SplitOpt.is_none());
        assert!(!String::from_utf8_lossy(&job.raw_args).contains("split_opt"));
    }

    for version in [JobVersion::V1, JobVersion::V2] {
        let mut job = Job {
            tp: ACTION_ADD_INDEX,
            version,
            ..Default::default()
        };
        job.FillArgs(args::ModifyIndexArgs {
            IndexArgs: vec![args::IndexArg {
                SplitOpt: Some(Box::new(args::IndexArgSplitOpt {
                    Num: 4,
                    ..Default::default()
                })),
                ..Default::default()
            }],
            ..Default::default()
        });
        let bytes = job.Encode(true).unwrap();
        let mut decoded = Job::decode(&bytes).unwrap();
        let decoded_args = args::GetModifyIndexArgs(&mut decoded).unwrap();
        assert_eq!(
            decoded_args.IndexArgs[0]
                .SplitOpt
                .as_ref()
                .map(|opt| opt.Num),
            if version == JobVersion::V2 {
                Some(4)
            } else {
                None
            }
        );
    }
}
