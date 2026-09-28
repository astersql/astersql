// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// config/kerneltype, parser/ast, parser/terror, testify/require。

// Job 模型与 Go 对齐的完整集成式单元测试。

use crate::group_2::serde_json;
use crate::group_3::*;

#[derive(Clone)]
struct RenameTableArgs {
    OldSchemaID: i64,
    NewTableName: ast::CIStr,
}

impl JobArgs for RenameTableArgs {
    fn get_args_v1(&self, _job: &Job) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!(self.OldSchemaID),
            serde_json::to_value(&self.NewTableName).unwrap(),
        ]
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "OldSchemaID": self.OldSchemaID,
            "NewTableName": self.NewTableName,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct TruncateTableArgs {
    FKCheck: bool,
}

impl JobArgs for TruncateTableArgs {
    fn get_args_v1(&self, _job: &Job) -> Vec<serde_json::Value> {
        vec![serde_json::json!(self.FKCheck)]
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"FKCheck": self.FKCheck})
    }
}
// jobSrc 对应 Go 的 //go:embed job.go；用 include_str! 保留“解析源码检查保留编号”的测试意图。
static JOB_SRC: &str = include_str!("job.go");

#[test]
fn test_job_start_time() {
    let job = Job {
        version: JobVersion::V1,
        id: 123,
        binlog_info: Some(HistoryInfo::default()),
        ..Default::default()
    };
    assert_eq!(ts_convert_to_time(job.start_ts), 0);
    assert_eq!(
        format!(
            "ID:123, Type:none, State:none, SchemaState:none, SchemaID:0, TableID:0, RowCount:0, ArgLen:0, start time: {}, Err:None, ErrCount:0, SnapshotVersion:0, Version: v1",
            0,
        ),
        job.to_string(),
    );
}

#[test]
fn test_state() {
    let job_tbl = [
        JobState::Running,
        JobState::Done,
        JobState::Cancelled,
        JobState::Rollingback,
        JobState::RollbackDone,
        JobState::Synced,
    ];
    for state in job_tbl {
        assert!(state.to_string().len() > 0);
    }
}

#[test]
fn test_job_codec() {
    let (tz_name, tz_offset) = ("UTC".to_owned(), 0);
    let reorg_location: super::group_3::TimeZoneLocation =
        serde_json::from_value(serde_json::json!({"name": tz_name, "offset": tz_offset}))
            .expect("build reorg timezone fixture");
    let mut job = Job {
        version: JobVersion::V1,
        id: 1,
        table_id: 2,
        schema_id: 1,
        binlog_info: Some(HistoryInfo::default()),
        reorg_meta: Some(DDLReorgMeta {
            Location: Some(Box::new(reorg_location)),
            ..Default::default()
        }),
        ..Default::default()
    };
    job.fill_args(&RenameTableArgs {
        OldSchemaID: 2,
        NewTableName: ast::CIStr {
            O: "table1".into(),
            L: "table1".into(),
        },
    });
    job.binlog_info.as_mut().unwrap().add_db_info(
        123,
        std::sync::Arc::new(DBInfo {
            ID: 1,
            Name: ast::CIStr {
                O: "test_history_db".into(),
                L: "test_history_db".into(),
            },
            ..Default::default()
        }),
    );
    job.binlog_info.as_mut().unwrap().add_table_info(
        123,
        std::sync::Arc::new(TableInfo {
            ID: 1,
            Name: ast::CIStr {
                O: "test_history_tbl".into(),
                L: "test_history_tbl".into(),
            },
            ..Default::default()
        }),
    );
    job.set_resume_reason(JOB_RESUME_REASON_KV_DISK_FULL.into());

    assert!(!job.is_cancelled());
    let b = job.encode(false).expect("encode job");
    let mut new_job = Job::decode(&b).expect("decode job");
    assert_eq!(
        job.binlog_info.as_ref().unwrap().schema_version,
        new_job.binlog_info.as_ref().unwrap().schema_version
    );
    assert_eq!(
        job.binlog_info
            .as_ref()
            .unwrap()
            .db_info
            .as_ref()
            .unwrap()
            .ID,
        new_job
            .binlog_info
            .as_ref()
            .unwrap()
            .db_info
            .as_ref()
            .unwrap()
            .ID
    );
    assert!(new_job.to_string().len() > 0);
    assert_eq!(
        new_job
            .reorg_meta
            .as_ref()
            .unwrap()
            .Location
            .as_ref()
            .unwrap()
            .name,
        tz_name
    );
    assert_eq!(
        new_job
            .reorg_meta
            .as_ref()
            .unwrap()
            .Location
            .as_ref()
            .unwrap()
            .offset,
        tz_offset
    );
    assert!(new_job.has_resume_reason(JOB_RESUME_REASON_KV_DISK_FULL));

    // Clean 后再次编码，Go 期望 BinlogInfo 回到空 HistoryInfo，同时 String 仍可打印。
    job.binlog_info.as_mut().unwrap().clean();
    let b1 = job.encode(true).expect("encode clean binlog job");
    new_job = Job::decode(&b1).expect("decode clean binlog job");
    assert_eq!(0, new_job.binlog_info.as_ref().unwrap().schema_version);
    assert!(new_job.binlog_info.as_ref().unwrap().db_info.is_none());
    assert!(new_job.to_string().len() > 0);

    let b2 = job.encode(true).expect("encode job second time");
    new_job = Job::decode(&b2).expect("decode job second time");
    assert!(new_job.to_string().len() > 0);

    job.state = JobState::Done;
    assert!(job.is_done());
    assert!(job.is_finished());
    assert!(!job.is_running());
    assert!(!job.is_synced());
    assert!(!job.is_rollback_done());
    job.set_row_count(3);
    assert_eq!(3, job.get_row_count());
}

#[test]
fn job_decode_uses_go_zero_values_for_missing_fields() {
    let job = Job::decode(br#"{"id":42}"#).expect("Go accepts omitted Job fields as zero values");
    assert_eq!(42, job.id);
    assert_eq!(ACTION_NONE, job.tp);
    assert_eq!(0, job.schema_id);
    assert_eq!(JobState::None, job.state);
    assert!(job.raw_args.is_empty());

    assert!(Job::decode(br#"{"id":42"#).is_err());
}

#[test]
fn test_ddl_reorg_meta_use_new_collate() {
    let mut meta = DDLReorgMeta::default();
    assert!(meta.GetUseNewCollateOrDefault(true));
    assert!(!meta.GetUseNewCollateOrDefault(false));

    meta.setUseNewCollate(false);
    assert!(!meta.GetUseNewCollateOrDefault(true));

    let data = serde_json::to_vec(&meta).expect("marshal DDLReorgMeta");
    assert!(
        String::from_utf8_lossy(&data).contains("\"use_new_collate\":false"),
        "{}",
        String::from_utf8_lossy(&data),
    );
    let mut decoded: DDLReorgMeta = serde_json::from_slice(&data).expect("unmarshal DDLReorgMeta");
    assert!(!decoded.GetUseNewCollateOrDefault(true));

    decoded.setUseNewCollate(true);
    assert!(decoded.GetUseNewCollateOrDefault(false));
}

#[test]
fn test_location() {
    let mut loc = TimeZoneLocation {
        name: "UTC".into(),
        offset: 0,
        location: std::sync::RwLock::new(None),
    };
    let mut n_loc = loc.get_location().expect("UTC location");
    assert_eq!(n_loc.name, "UTC");

    // Go 注释中的 loc.location != nil 场景：设置 Name 后仍复用 UTC 缓存。
    loc.name = "Asia/Shanghai".into();
    n_loc = loc.get_location().expect("cached location");
    assert_eq!(n_loc.name, "UTC");

    let loc1 = TimeZoneLocation {
        name: "UTC".into(),
        offset: 18000,
        location: std::sync::RwLock::new(None),
    };
    let loc1_byte = serde_json::to_vec(&loc1).expect("marshal timezone");
    let loc2: TimeZoneLocation = serde_json::from_slice(&loc1_byte).expect("unmarshal timezone");
    assert_eq!(loc2.offset, loc1.offset);
    assert_eq!(loc2.name, loc1.name);
    n_loc = loc2.get_location().expect("fixed timezone");
    assert_eq!(n_loc.name, "UTC");
    assert_eq!(n_loc.offset, loc1.offset);
}

#[test]
fn test_job_clone() {
    let mut job = Job {
        version: JobVersion::V1,
        id: 100,
        tp: ACTION_CREATE_TABLE,
        schema_id: 101,
        table_id: 102,
        schema_name: "test".into(),
        table_name: "t".into(),
        state: JobState::Done,
        multi_schema_info: None,
        resume_reason: Some(JobResumeReason {
            reason_type: JOB_RESUME_REASON_KV_DISK_FULL.into(),
        }),
        ..Default::default()
    };
    let clone = job.clone_job().expect("clone job through codec");
    assert_eq!(job.id, clone.id);
    assert_eq!(job.tp, clone.tp);
    assert_eq!(job.schema_id, clone.schema_id);
    assert_eq!(job.table_id, clone.table_id);
    assert_eq!(job.schema_name, clone.schema_name);
    assert_eq!(job.table_name, clone.table_name);
    assert_eq!(job.state, clone.state);
    assert_eq!(
        job.multi_schema_info.is_none(),
        clone.multi_schema_info.is_none()
    );
    assert_eq!(
        job.resume_reason.as_ref().unwrap().reason_type,
        clone.resume_reason.as_ref().unwrap().reason_type
    );
}

#[test]
fn test_sub_job_to_proxy_job_with_resume_reason() {
    let parent_job = Job {
        id: 100,
        resume_reason: Some(JobResumeReason {
            reason_type: JOB_RESUME_REASON_KV_DISK_FULL.into(),
        }),
        ..Default::default()
    };
    let sub_job = SubJob {
        tp: ACTION_ADD_INDEX,
        state: JobState::Queueing,
        ..Default::default()
    };
    let proxy_job = sub_job.to_proxy_job(&parent_job, 0);
    assert!(proxy_job.has_resume_reason(JOB_RESUME_REASON_KV_DISK_FULL));
}

#[test]
fn test_job_size() {
    let msg =
        "Please make sure that SubJob.FromProxyJob() and SubJob.to_proxy_job() work as expected";
    // Go 用 unsafe.Sizeof 作为增删字段的间接哨兵；Rust 直接验证两个转换
    // 方向的字段契约，避免所依赖类型的布局变化产生假失败。
    let parent = Job {
        id: 11,
        schema_id: 12,
        table_id: 13,
        schema_name: "db".into(),
        table_name: "tbl".into(),
        start_ts: 14,
        dependency_id: 15,
        query: "alter table tbl add index idx(a)".into(),
        version: JobVersion::V2,
        priority: 16,
        seq_num: 17,
        charset: "utf8mb4".into(),
        collate: "utf8mb4_bin".into(),
        admin_operator: AdminCommandOperator::System,
        sql_mode: 18,
        session_vars: [("time_zone".into(), "UTC".into())].into(),
        ..Default::default()
    };
    let sub = SubJob {
        tp: ACTION_ADD_INDEX,
        raw_args: br#"[{\"index_name\":\"idx\"}]"#.to_vec(),
        schema_state: SchemaState::WriteOnly,
        snapshot_ver: 21,
        real_start_ts: 22,
        revertible: true,
        state: JobState::Running,
        row_count: 23,
        need_reorg: true,
        ..Default::default()
    };

    let mut proxy = sub.to_proxy_job(&parent, 24);
    assert_eq!(
        (proxy.id, proxy.schema_id, proxy.table_id),
        (11, 12, 13),
        "{}",
        msg
    );
    assert_eq!(
        (proxy.tp, proxy.state),
        (ACTION_ADD_INDEX, JobState::Running),
        "{}",
        msg
    );
    assert_eq!(
        (proxy.snapshot_ver, proxy.real_start_ts),
        (21, 22),
        "{}",
        msg
    );
    assert_eq!(proxy.get_row_count(), 23, "{}", msg);
    assert!(proxy.need_reorg, "{}", msg);
    assert_eq!(proxy.multi_schema_info.as_ref().unwrap().seq, 24, "{}", msg);
    assert_eq!(
        proxy.session_vars.get("time_zone").map(String::as_str),
        Some("UTC"),
        "{}",
        msg
    );

    proxy.schema_state = SchemaState::Public;
    proxy.snapshot_ver = 31;
    proxy.real_start_ts = 32;
    proxy.state = JobState::Done;
    proxy.set_row_count(33);
    let mut restored = SubJob::default();
    restored.from_proxy_job(&proxy, 34);
    assert!(restored.revertible, "{}", msg);
    assert_eq!(restored.schema_state, SchemaState::Public, "{}", msg);
    assert_eq!(
        (restored.snapshot_ver, restored.real_start_ts),
        (31, 32),
        "{}",
        msg
    );
    assert_eq!(
        (restored.state, restored.row_count, restored.schema_ver),
        (JobState::Done, 33, 34),
        "{}",
        msg
    );
}

#[test]
fn test_backfill_meta_codec() {
    let jm = super::group_3::JobMeta {
        schema_id: 1,
        table_id: 2,
        query: "alter table t add index idx(a)".into(),
        priority: 1,
        tp: ACTION_ADD_INDEX,
    };
    let bm = BackfillMeta {
        EndInclude: true,
        Error: Some(Box::new("result undetermined".into())),
        JobMeta: Some(Box::new(jm)),
        ..Default::default()
    };
    let bm_bytes = bm.Encode().expect("encode backfill meta");
    let mut bm_ret = BackfillMeta::default();
    bm_ret.Decode(&bm_bytes).expect("decode backfill meta");
    assert_eq!(bm.EndInclude, bm_ret.EndInclude);
    assert_eq!(bm.Error, bm_ret.Error);
}

#[test]
fn test_may_need_reorg() {
    let reorg_job_types = [
        ACTION_REORGANIZE_PARTITION,
        ACTION_REMOVE_PARTITIONING,
        ACTION_ALTER_TABLE_PARTITIONING,
        ACTION_ADD_INDEX,
        ACTION_ADD_PRIMARY_KEY,
    ];
    let general_job_types = [ACTION_CREATE_TABLE, ACTION_DROP_TABLE];
    let mut job = Job {
        version: JobVersion::V1,
        id: 100,
        tp: ACTION_CREATE_TABLE,
        schema_id: 101,
        table_id: 102,
        schema_name: "test".into(),
        table_name: "t".into(),
        state: JobState::Done,
        multi_schema_info: None,
        ..Default::default()
    };
    for job_type in reorg_job_types {
        job.tp = job_type;
        assert!(job.may_need_reorg());
    }
    for job_type in general_job_types {
        job.tp = job_type;
        assert!(!job.may_need_reorg());
    }
}

#[test]
fn test_in_final_state() {
    for (s, v) in [
        (JobState::Synced, true),
        (JobState::Cancelled, true),
        (JobState::Paused, true),
        (JobState::Cancelling, false),
        (JobState::RollbackDone, false),
    ] {
        assert_eq!(
            v,
            Job {
                state: s,
                ..Default::default()
            }
            .in_final_state()
        );
    }
}

#[test]
fn test_schema_state() {
    let schema_tbl = [
        SchemaState::DeleteOnly,
        SchemaState::WriteOnly,
        SchemaState::WriteReorganization,
        SchemaState::DeleteReorganization,
        SchemaState::Public,
        SchemaState::GlobalTxnOnly,
    ];
    for state in schema_tbl {
        assert!(state.to_string().len() > 0);
    }
}

#[test]
fn test_action_type_reserved() {
    let reserved_start = 200_i64;
    let reserved_end = 256_i64;

    // Go 通过 AST 扫描显式 ActionType 数值；这里对嵌入源码执行同一保留区检查。
    for line in JOB_SRC
        .lines()
        .filter(|line| line.contains("Action") && line.contains("ActionType"))
    {
        let Some((name, expression)) = line.split_once('=') else {
            continue;
        };
        let expression = expression
            .trim()
            .trim_start_matches("ActionType(")
            .trim_end_matches(')');
        let Ok(value) = expression.parse::<i64>() else {
            continue;
        };
        assert!(
            !(value >= reserved_start && value < reserved_end),
            "action {} must not be in reserved range [{}, {}), but got {}",
            name.trim(),
            reserved_start,
            reserved_end,
            value,
        );
    }
}

#[test]
fn test_string() {
    let acts = [
        (ACTION_NONE, "none"),
        (ACTION_ADD_FOREIGN_KEY, "add foreign key"),
        (ACTION_DROP_FOREIGN_KEY, "drop foreign key"),
        (ACTION_TRUNCATE_TABLE, "truncate table"),
        (ACTION_MODIFY_COLUMN, "modify column"),
        (ACTION_RENAME_TABLE, "rename table"),
        (ACTION_RENAME_TABLES, "rename tables"),
        (ACTION_SET_DEFAULT_VALUE, "set default value"),
        (ACTION_CREATE_SCHEMA, "create schema"),
        (ACTION_DROP_SCHEMA, "drop schema"),
        (ACTION_CREATE_TABLE, "create table"),
        (ACTION_DROP_TABLE, "drop table"),
        (ACTION_ADD_INDEX, "add index"),
        (ACTION_DROP_INDEX, "drop index"),
        (ACTION_ADD_COLUMN, "add column"),
        (ACTION_DROP_COLUMN, "drop column"),
        (
            ACTION_MODIFY_SCHEMA_CHARSET_AND_COLLATE,
            "modify schema charset and collate",
        ),
        (ACTION_ALTER_TABLE_PLACEMENT, "alter table placement"),
        (
            ACTION_ALTER_TABLE_PARTITION_PLACEMENT,
            "alter table partition placement",
        ),
        (ACTION_ALTER_NO_CACHE_TABLE, "alter table nocache"),
        (ACTION_ALTER_TABLE_AFFINITY, "alter table affinity"),
        (
            ACTION_ALTER_TABLE_SOFT_DELETE_INFO,
            "alter soft delete info",
        ),
        (
            ACTION_MODIFY_SCHEMA_SOFT_DELETE_AND_ACTIVE_ACTIVE,
            "modify schema soft delete and active active",
        ),
    ];
    for (act, result) in acts {
        assert_eq!(result, action_type_string(act));
    }
}

#[test]
fn test_job_encode_v2() {
    let mut j = Job {
        version: JobVersion::V2,
        tp: ACTION_TRUNCATE_TABLE,
        ..Default::default()
    };
    j.fill_args(&TruncateTableArgs {
        FKCheck: true,
        ..Default::default()
    });
    let _ = j.encode(false).expect("V2 encode without updateRawArgs");
    assert!(j.raw_args.is_empty());
    let _ = j.encode(true).expect("V2 encode with updateRawArgs");
    assert!(!j.raw_args.is_empty());
    let args: serde_json::Value =
        serde_json::from_slice(&j.raw_args).expect("unmarshal raw truncate args");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&j.raw_args).unwrap(),
        args
    );
    assert_eq!(args["FKCheck"], true);
}

#[test]
fn test_job_ver_in_use() {
    if !kerneltype::is_next_gen() {
        assert_eq!(JobVersion::V1, get_job_ver_in_use());
    } else {
        assert_eq!(JobVersion::V2, get_job_ver_in_use());
    }
}

#[test]
fn test_job_check_involving_schema_info() {
    let cases = vec![
        (
            Job {
                schema_name: "".into(),
                table_name: "".into(),
                ..Default::default()
            },
            "must involve only one type of object",
        ),
        (
            Job {
                schema_name: "".into(),
                table_name: "t1".into(),
                ..Default::default()
            },
            "must have non-empty name set",
        ),
        (
            Job {
                schema_name: "".into(),
                table_name: "*".into(),
                ..Default::default()
            },
            "must have non-empty name set",
        ),
        (
            Job {
                schema_name: "test".into(),
                table_name: "".into(),
                ..Default::default()
            },
            "",
        ),
        (
            Job {
                schema_name: "test".into(),
                table_name: "t".into(),
                ..Default::default()
            },
            "",
        ),
        (
            Job {
                schema_name: "test".into(),
                table_name: "*".into(),
                ..Default::default()
            },
            "",
        ),
        (
            Job {
                schema_name: "*".into(),
                table_name: "".into(),
                ..Default::default()
            },
            "",
        ),
        (
            Job {
                schema_name: "*".into(),
                table_name: "t".into(),
                ..Default::default()
            },
            "operating on all databases, must not set table name",
        ),
        (
            Job {
                schema_name: "*".into(),
                table_name: "*".into(),
                ..Default::default()
            },
            "",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    policy: "p".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    policy: "*".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    resource_group: "r".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    resource_group: "*".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    policy: "p".into(),
                    resource_group: "r".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "must involve only one type of object",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    policy: "p".into(),
                    database: "d".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "must involve only one type of object",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    database: "d".into(),
                    resource_group: "r".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "must involve only one type of object",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    policy: "p".into(),
                    database: "d".into(),
                    resource_group: "r".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "must involve only one type of object",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    database: "".into(),
                    table: "".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "must involve only one type of object",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    database: "".into(),
                    table: "t".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "must have non-empty name set",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    database: "".into(),
                    table: "*".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "must have non-empty name set",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    database: "d".into(),
                    table: "".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "must have non-empty name set",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    database: "d".into(),
                    table: "t".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    database: "d".into(),
                    table: "*".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "",
        ),
        // 显式 InvolvingSchemaInfo 不自动把 * 空表名调整为 *.*，需按 Go 行为报错。
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    database: "*".into(),
                    table: "".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "must have non-empty name set",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    database: "*".into(),
                    table: "t".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "operating on all databases, must not set table name",
        ),
        (
            Job {
                involving_schema_info: vec![InvolvingSchemaInfo {
                    database: "*".into(),
                    table: "*".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            "",
        ),
    ];

    for (i, c) in cases.into_iter().enumerate() {
        let err = c.0.check_involving_schema_info();
        if c.1.is_empty() {
            assert!(err.is_ok(), "case-{}", i);
        } else {
            assert!(err.unwrap_err().to_string().contains(c.1), "case-{}", i);
        }
    }

    let mut job = Job {
        schema_name: "TestDB".into(),
        table_name: "T1".into(),
        involving_schema_info: vec![
            InvolvingSchemaInfo {
                database: "TestDB".into(),
                table: "T1".into(),
                ..Default::default()
            },
            InvolvingSchemaInfo {
                database: "AnotherDB".into(),
                table: INVOLVING_ALL.into(),
                ..Default::default()
            },
            InvolvingSchemaInfo {
                database: INVOLVING_ALL.into(),
                table: INVOLVING_ALL.into(),
                ..Default::default()
            },
            InvolvingSchemaInfo {
                database: INVOLVING_NONE.into(),
                table: INVOLVING_NONE.into(),
                ..Default::default()
            },
            InvolvingSchemaInfo {
                policy: "PolicyName".into(),
                ..Default::default()
            },
            InvolvingSchemaInfo {
                resource_group: "ResourceGroupName".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    job.normalize_involving_schema_info();
    assert_eq!("testdb", job.schema_name);
    assert_eq!("t1", job.table_name);
    let expected = vec![
        InvolvingSchemaInfo {
            database: "testdb".into(),
            table: "t1".into(),
            ..Default::default()
        },
        InvolvingSchemaInfo {
            database: "anotherdb".into(),
            table: INVOLVING_ALL.into(),
            ..Default::default()
        },
        InvolvingSchemaInfo {
            database: INVOLVING_ALL.into(),
            table: INVOLVING_ALL.into(),
            ..Default::default()
        },
        InvolvingSchemaInfo {
            database: INVOLVING_NONE.into(),
            table: INVOLVING_NONE.into(),
            ..Default::default()
        },
        InvolvingSchemaInfo {
            policy: "policyname".into(),
            ..Default::default()
        },
        InvolvingSchemaInfo {
            resource_group: "resourcegroupname".into(),
            ..Default::default()
        },
    ];
    assert_eq!(expected.len(), job.involving_schema_info.len());
    for (expected, actual) in expected.iter().zip(&job.involving_schema_info) {
        assert_eq!(expected.database, actual.database);
        assert_eq!(expected.table, actual.table);
        assert_eq!(expected.policy, actual.policy);
        assert_eq!(expected.resource_group, actual.resource_group);
    }
}
// 实际可执行测试使用 group3 导出的 Job / JobState。
use crate::group_3::{Job, JobState};

#[test]
/// 终态与回滚相关 JobState 的 Display 不得为 "none"。
fn job_state_strings_cover_terminal_and_rollback_states() {
    for state in [
        JobState::Running,
        JobState::Done,
        JobState::Cancelled,
        JobState::Rollingback,
        JobState::RollbackDone,
        JobState::Synced,
    ] {
        let mut job = Job::default();
        job.state = state;
        assert_ne!(job.state.to_string(), "none");
    }
}
