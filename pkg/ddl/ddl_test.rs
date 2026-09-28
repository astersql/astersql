// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// DDL 模块单元测试。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE/ALTER/DROP 等
// 修改数据库对象元信息（schema）的语句。本文件测试 DDL 执行器中的
// 若干纯函数逻辑：
// - 重试间隔策略 `get_interval_from_policy`：DDL job（DDL 任务）轮询
//   等待时按策略取递增间隔，超出策略表长度后沿用最后一个间隔；
// - 建库/建表时的字符集与排序规则（charset/collation）解析，
//   重复且互相冲突的字符集选项需要报错；
// - 标识符长度校验 `check_identifier`：与 Go/TiDB 一致，
//   标识符（表名、列名等）最长 64 个字符。
//
// 文件前半部分是从 Go(TiDB) 机械迁移、尚未适配 Rust 的旧测试代码，
// 整体用块注释屏蔽，仅作迁移参考，不参与编译。

// 以下大块注释为 Go 迁移遗留的旧测试代码（覆盖 reorgCtx 复用、
// 列类型修改校验、重复约束名检查、DDL job 版本探测等场景），
// 因依赖尚未迁移完成的接口而被整体注释掉，保留以便后续恢复。
/*
//

// DDLForTest exports for testing.
pub trait DDLForTest {
    fn NewReorgCtx(&mut self, jobID: i64, rowCount: i64) -> *mut reorgCtx;
    fn GetReorgCtx(&self, jobID: i64) -> *mut reorgCtx;
    fn RemoveReorgCtx(&mut self, id: i64);
}

impl DDLForTest for ddl {
    // NewReorgCtx exports for testing.
    fn NewReorgCtx(&mut self, jobID: i64, rowCount: i64) -> *mut reorgCtx {
        self.newReorgCtx(jobID, rowCount)
    }

    // GetReorgCtx exports for testing.
    fn GetReorgCtx(&self, jobID: i64) -> *mut reorgCtx {
        self.getReorgCtx(jobID)
    }

    // RemoveReorgCtx exports for testing.
    fn RemoveReorgCtx(&mut self, id: i64) {
        self.removeReorgCtx(id);
    }
}

pub fn NewJobSubmitterForTest() -> *mut JobSubmitter {
    let sync_map = generic::NewSyncMap::<i64, chan::Chan<()>>(8);
    &mut JobSubmitter {
        ddlJobDoneChMap: &sync_map,
        ..Default::default()
    }
}

impl JobSubmitter {
    pub fn DDLJobDoneChMap(&self) -> *mut generic::SyncMap<i64, chan::Chan<()>> {
        self.ddlJobDoneChMap
    }

    // GenGIDAndInsertJobsWithRetry 保留 Go 的副作用说明：同一事务内分配全局 ID 并插入 DDL job。
    pub fn GenGIDAndInsertJobsWithRetry(
        &self,
        ctx: context::Context,
        ddl_se: *mut sess::Session,
        job_ws: Vec<*mut JobWrapper>,
    ) -> Result<(), error::Error> {
        jobsubmit::GenGIDAndInsertJobsWithRetry(
            ctx,
            ddl_se,
            jobWrappersToSpecs(job_ws),
            self.registerJobDoneChannels,
        )
    }
}

#[test]
fn test_get_interval_from_policy() {
    let policy = vec![time::Second, 2 * time::Second];

    let (val, changed) = getIntervalFromPolicy(policy.clone(), 0);
    require::Equal(t, val, time::Second);
    require::True(t, changed);

    let (val, changed) = getIntervalFromPolicy(policy.clone(), 1);
    require::Equal(t, val, 2 * time::Second);
    require::True(t, changed);

    let (val, changed) = getIntervalFromPolicy(policy.clone(), 2);
    require::Equal(t, val, 2 * time::Second);
    require::False(t, changed);

    let (val, changed) = getIntervalFromPolicy(policy, 3);
    require::Equal(t, val, 2 * time::Second);
    require::False(t, changed);
}

pub fn colDefStrToColInfo(str_: &str, ctx: *mut metabuild::Context) -> *mut model::ColumnInfo {
    let sql_a = format!("alter table t modify column a {}", str_);
    let (stmt, err) = parser::New().ParseOneStmt(sql_a, "", "");
    require::NoError(t, err);
    let col_def = stmt.(*ast::AlterTableStmt).Specs[0].NewColumns[0];
    let (chs, coll) = charset::GetDefaultCharsetAndCollate();
    let (col, _, err) = buildColumnAndConstraint(ctx, 0, col_def, None, chs, coll);
    require::NoError(t, err);
    col.ToInfo()
}

#[test]
fn test_modify_column() {
    let ctx = NewMetaBuildContextWithSctx(mock::NewContext());
    let tests = vec![
        ("int", "bigint", None),
        ("int", "int unsigned", None),
        ("varchar(10)", "text", None),
        ("varbinary(10)", "blob", None),
        ("text", "blob", Some(dbterror::ErrUnsupportedModifyCharset.GenWithStackByArgs("charset from utf8mb4 to binary"))),
        ("varchar(10)", "varchar(8)", None),
        ("varchar(10)", "varchar(11)", None),
        ("varchar(10) character set utf8 collate utf8_bin", "varchar(10) character set utf8", None),
        ("decimal(2,1)", "decimal(3,2)", None),
        ("decimal(2,1)", "decimal(2,2)", None),
        ("decimal(2,1)", "decimal(2,1)", None),
        ("decimal(2,1)", "int", None),
        ("decimal", "int", None),
        ("decimal(2,1)", "bigint", None),
        ("int", "varchar(10) character set gbk", Some(dbterror::ErrUnsupportedModifyCharset.GenWithStackByArgs("charset from binary to gbk"))),
        ("varchar(10) character set gbk", "int", Some(dbterror::ErrUnsupportedModifyCharset.GenWithStackByArgs("charset from gbk to binary"))),
        ("varchar(10) character set gbk", "varchar(10) character set utf8", Some(dbterror::ErrUnsupportedModifyCharset.GenWithStackByArgs("charset from gbk to utf8"))),
        ("varchar(10) character set gbk", "char(10) character set utf8", Some(dbterror::ErrUnsupportedModifyCharset.GenWithStackByArgs("charset from gbk to utf8"))),
        ("varchar(10) character set utf8", "char(10) character set gbk", Some(dbterror::ErrUnsupportedModifyCharset.GenWithStackByArgs("charset from utf8 to gbk"))),
        ("varchar(10) character set utf8", "varchar(10) character set gbk", Some(dbterror::ErrUnsupportedModifyCharset.GenWithStackByArgs("charset from utf8 to gbk"))),
        ("varchar(10) character set gbk", "varchar(255) character set gbk", None),
    ];
    for (origin, to, expected_err) in tests {
        let col_a = colDefStrToColInfo(origin, ctx);
        let col_b = colDefStrToColInfo(to, ctx);
        let err = checkModifyTypes(col_a, col_b, false);
        if err.is_none() {
            require::NoErrorf(t, expected_err, "origin:%v, to:%v", origin, to);
        } else {
            require::EqualError(t, err, expected_err.unwrap().Error());
        }
    }
}

#[test]
fn test_field_case() {
    let fields = vec!["field", "Field"];
    let mut col_objects = Vec::with_capacity(fields.len());
    for name in fields {
        col_objects.push(&model::ColumnInfo {
            Name: ast::NewCIStr(name),
            ..Default::default()
        });
    }
    let err = checkDuplicateColumn(col_objects);
    require::EqualError(t, err, infoschema::ErrColumnExists.GenWithStackByArgs("Field").Error());
}

#[test]
fn test_ignorable_spec() {
    let specs = vec![
        ast::AlterTableOption,
        ast::AlterTableAddColumns,
        ast::AlterTableAddConstraint,
        ast::AlterTableDropColumn,
        ast::AlterTableDropPrimaryKey,
        ast::AlterTableDropIndex,
        ast::AlterTableDropForeignKey,
        ast::AlterTableModifyColumn,
        ast::AlterTableChangeColumn,
        ast::AlterTableRenameTable,
        ast::AlterTableAlterColumn,
    ];
    for spec in specs {
        require::False(t, isIgnorableSpec(spec));
    }

    for spec in [ast::AlterTableLock, ast::AlterTableAlgorithm] {
        require::True(t, isIgnorableSpec(spec));
    }
}

#[test]
fn test_error() {
    let kv_errs = vec![
        dbterror::ErrDDLJobNotFound,
        dbterror::ErrCancelFinishedDDLJob,
        dbterror::ErrCannotCancelDDLJob,
    ];
    for err in kv_errs {
        let code = terror::ToSQLError(err).Code;
        require::NotEqual(t, mysql::ErrUnknown, code);
        require::Equal(t, err.Code() as u16, code);
    }
}

#[test]
fn test_check_duplicate_constraint() {
    let mut constr_names = map! {};

    let err = checkDuplicateConstraint(constr_names, "f1", ast::ConstraintForeignKey);
    require::NoError(t, err);
    let err = checkDuplicateConstraint(constr_names, "f1", ast::ConstraintForeignKey);
    require::EqualError(t, err, "[ddl:1826]Duplicate foreign key constraint name 'f1'");

    let err = checkDuplicateConstraint(constr_names, "c1", ast::ConstraintCheck);
    require::NoError(t, err);
    let err = checkDuplicateConstraint(constr_names, "c1", ast::ConstraintCheck);
    require::EqualError(t, err, "[ddl:3822]Duplicate check constraint name 'c1'.");

    let err = checkDuplicateConstraint(constr_names, "u1", ast::ConstraintUniq);
    require::NoError(t, err);
    let err = checkDuplicateConstraint(constr_names, "u1", ast::ConstraintUniq);
    require::EqualError(t, err, "[ddl:1061]Duplicate key name 'u1'");
}

#[test]
fn test_get_table_data_key_ranges() {
    let mut key_ranges = getTableDataKeyRanges(vec![]);
    require::Len(t, key_ranges, 1);
    require::Equal(t, key_ranges[0].StartKey, tablecodec::EncodeTablePrefix(0));
    require::Equal(t, key_ranges[0].EndKey, tablecodec::EncodeTablePrefix(metadef::MaxUserGlobalID));

    key_ranges = getTableDataKeyRanges(vec![3]);
    require::Len(t, key_ranges, 2);
    require::Equal(t, key_ranges[0].StartKey, tablecodec::EncodeTablePrefix(0));
    require::Equal(t, key_ranges[0].EndKey, tablecodec::EncodeTablePrefix(3));
    require::Equal(t, key_ranges[1].StartKey, tablecodec::EncodeTablePrefix(4));
    require::Equal(t, key_ranges[1].EndKey, tablecodec::EncodeTablePrefix(metadef::MaxUserGlobalID));

    key_ranges = getTableDataKeyRanges(vec![3, 5, 9]);
    require::Len(t, key_ranges, 4);
    require::Equal(t, key_ranges[0].StartKey, tablecodec::EncodeTablePrefix(0));
    require::Equal(t, key_ranges[0].EndKey, tablecodec::EncodeTablePrefix(3));
    require::Equal(t, key_ranges[1].StartKey, tablecodec::EncodeTablePrefix(4));
    require::Equal(t, key_ranges[1].EndKey, tablecodec::EncodeTablePrefix(5));
    require::Equal(t, key_ranges[2].StartKey, tablecodec::EncodeTablePrefix(6));
    require::Equal(t, key_ranges[2].EndKey, tablecodec::EncodeTablePrefix(9));
    require::Equal(t, key_ranges[3].StartKey, tablecodec::EncodeTablePrefix(10));
    require::Equal(t, key_ranges[3].EndKey, tablecodec::EncodeTablePrefix(metadef::MaxUserGlobalID));
}

#[test]
fn test_merge_continuous_key_ranges() {
    let cases = vec![
        (
            vec![keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![1], EndKey: vec![2] }, exclude: true }],
            vec![],
        ),
        (
            vec![keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![1], EndKey: vec![2] }, exclude: false }],
            vec![kv::KeyRange { StartKey: vec![1], EndKey: vec![2] }],
        ),
        (
            vec![
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![1], EndKey: vec![2] }, exclude: false },
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![3], EndKey: vec![4] }, exclude: false },
            ],
            vec![kv::KeyRange { StartKey: vec![1], EndKey: vec![4] }],
        ),
        (
            vec![
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![1], EndKey: vec![2] }, exclude: false },
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![3], EndKey: vec![4] }, exclude: true },
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![5], EndKey: vec![6] }, exclude: false },
            ],
            vec![
                kv::KeyRange { StartKey: vec![1], EndKey: vec![2] },
                kv::KeyRange { StartKey: vec![5], EndKey: vec![6] },
            ],
        ),
        (
            vec![
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![1], EndKey: vec![2] }, exclude: true },
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![3], EndKey: vec![4] }, exclude: true },
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![5], EndKey: vec![6] }, exclude: false },
            ],
            vec![kv::KeyRange { StartKey: vec![5], EndKey: vec![6] }],
        ),
        (
            vec![
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![1], EndKey: vec![2] }, exclude: false },
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![3], EndKey: vec![4] }, exclude: true },
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![5], EndKey: vec![6] }, exclude: true },
            ],
            vec![kv::KeyRange { StartKey: vec![1], EndKey: vec![2] }],
        ),
        (
            vec![
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![1], EndKey: vec![2] }, exclude: true },
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![3], EndKey: vec![4] }, exclude: false },
                keyRangeMayExclude { r: kv::KeyRange { StartKey: vec![5], EndKey: vec![6] }, exclude: true },
            ],
            vec![kv::KeyRange { StartKey: vec![3], EndKey: vec![4] }],
        ),
    ];

    for (i, (input, expect)) in cases.into_iter().enumerate() {
        let ranges = mergeContinuousKeyRanges(input);
        require::Equal(t, expect, ranges, "case {}", i);
    }
}

#[test]
fn test_detect_and_update_job_version() {
    let (ctx, cancel) = context::WithCancel(context::Background());
    t.Cleanup(cancel);
    let mut d = ddl { ddlCtx: &ddlCtx { ctx }, ..Default::default() };

    let reset = || {
        model::SetJobVerInUse(model::JobVersion1);
        model::SetGlobalIndexV1Supported(false);
    };
    t.Cleanup(reset);
    reset();
    require::Equal(t, model::JobVersion1, model::GetJobVerInUse());
    require::False(t, model::GetGlobalIndexV1Supported());

    t.Run("in ut", || {
        reset();
        d.detectAndUpdateJobVersion();
        if testargsv1::ForceV1 {
            require::Equal(t, model::JobVersion1, model::GetJobVerInUse());
        } else {
            require::Equal(t, model::JobVersion2, model::GetJobVerInUse());
        }
        require::True(t, model::GetGlobalIndexV1Supported());
    });

    d.etcdCli = &clientv3::Client {};
    let mock_get_all_server_info = |versions: &[&str]| {
        let mut server_infos = HashMap::with_capacity(versions.len());
        for (i, v) in versions.iter().enumerate() {
            server_infos.insert(
                format!("node{}", i),
                &serverinfo::ServerInfo {
                    StaticInfo: serverinfo::StaticInfo {
                        VersionInfo: serverinfo::VersionInfo { Version: v.to_string(), ..Default::default() },
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
        }
        let bytes = json::Marshal(server_infos).expect("marshal server infos");
        let in_terms = format!("return(`{}`)", String::from_utf8(bytes).unwrap());
        testfailpoint::Enable(t, "github.com/pingcap/tidb/pkg/domain/serverinfo/mockGetAllServerInfo", in_terms);
    };

    t.Run("all support v2 and global index v1", || {
        reset();
        mock_get_all_server_info(&[
            "8.0.11-TiDB-v8.5.6-alpha-228-g650888fea7-dirty",
            "8.0.11-TiDB-v9.0.0",
            "8.0.11-TiDB-8.5.6-beta",
        ]);
        d.detectAndUpdateJobVersionOnce();
        require::Equal(t, model::JobVersion2, model::GetJobVerInUse());
        require::True(t, model::GetGlobalIndexV1Supported());
    });

    t.Run("all support v2 but not global index v1", || {
        reset();
        mock_get_all_server_info(&["8.0.11-TiDB-v8.4.0", "8.0.11-TiDB-v8.5.5"]);
        d.detectAndUpdateJobVersionOnce();
        require::Equal(t, model::JobVersion2, model::GetJobVerInUse());
        require::False(t, model::GetGlobalIndexV1Supported());
    });

    t.Run("v1 first, later all support v2 and global index v1", || {
        reset();
        let interval_bak = detectJobVerInterval;
        t.Cleanup(|| {
            detectJobVerInterval = interval_bak;
        });
        detectJobVerInterval = time::Millisecond;
        mock_get_all_server_info(&["unknown"]);
        let mut iterate_cnt = 0;
        testfailpoint::EnableCall(
            t,
            "github.com/pingcap/tidb/pkg/ddl/afterDetectAndUpdateJobVersionOnce",
            || {
                iterate_cnt += 1;
                if iterate_cnt == 1 {
                    require::Equal(t, model::JobVersion1, model::GetJobVerInUse());
                    require::False(t, model::GetGlobalIndexV1Supported());
                    mock_get_all_server_info(&["9.0.0-xxx"]);
                } else if iterate_cnt == 2 {
                    require::Equal(t, model::JobVersion1, model::GetJobVerInUse());
                    require::False(t, model::GetGlobalIndexV1Supported());
                    mock_get_all_server_info(&["xxx"]);
                } else if iterate_cnt == 3 {
                    require::Equal(t, model::JobVersion1, model::GetJobVerInUse());
                    require::False(t, model::GetGlobalIndexV1Supported());
                    mock_get_all_server_info(&["8.0.11-TiDB-8.3.0"]);
                } else if iterate_cnt == 4 {
                    require::Equal(t, model::JobVersion1, model::GetJobVerInUse());
                    require::False(t, model::GetGlobalIndexV1Supported());
                    mock_get_all_server_info(&[
                        "8.0.11-TiDB-v8.3.0",
                        "8.0.11-TiDB-v8.3.0",
                        "8.0.11-TiDB-v8.4.0",
                    ]);
                } else if iterate_cnt == 5 {
                    require::Equal(t, model::JobVersion1, model::GetJobVerInUse());
                    require::False(t, model::GetGlobalIndexV1Supported());
                    mock_get_all_server_info(&[
                        "8.0.11-TiDB-v8.4.0",
                        "8.0.11-TiDB-v8.4.0",
                        "8.0.11-TiDB-v8.4.0",
                    ]);
                } else if iterate_cnt == 6 {
                    require::Equal(t, model::JobVersion2, model::GetJobVerInUse());
                    require::False(t, model::GetGlobalIndexV1Supported());
                    mock_get_all_server_info(&[
                        "8.0.11-TiDB-v8.5.6",
                        "8.0.11-TiDB-v8.5.6",
                        "8.0.11-TiDB-v8.5.6",
                    ]);
                } else {
                    require::Equal(t, model::JobVersion2, model::GetJobVerInUse());
                    require::True(t, model::GetGlobalIndexV1Supported());
                }
            },
        );
        d.detectAndUpdateJobVersion();
        d.wg.Wait();
        require::EqualValues(t, 7, iterate_cnt);
    });
}

#[test]
fn test_set_global_index_version_flag() {
    let tbl_info = &model::TableInfo::default(); // non-clustered (zero value)
    let idx_info = &mut model::IndexInfo { Global: true, Unique: false, ..Default::default() };

    model::SetGlobalIndexV1Supported(false);
    t.Cleanup(|| model::SetGlobalIndexV1Supported(false));

    setGlobalIndexVersion(tbl_info, idx_info);
    require::Equal(t, 0_u8, idx_info.GlobalIndexVersion);

    model::SetGlobalIndexV1Supported(true);
    setGlobalIndexVersion(tbl_info, idx_info);
    require::Equal(t, model::GlobalIndexVersionV1, idx_info.GlobalIndexVersion);
}
*/

use std::time::Duration;

// 被测函数均来自 DDL 执行器模块：
// - check_identifier: 校验标识符（表名/列名等）长度合法性；
// - get_interval_from_policy: 按重试策略表返回轮询间隔；
// - resolve_charset_collation: 解析字符集与排序规则选项。
use crate::ddl::{ActionType, Job, drop_or_truncate_table_info_from_jobs, recover_snapshot_ts};
use crate::executor::{check_identifier, get_interval_from_policy, resolve_charset_collation};

#[test]
fn recover_snapshot_prefers_real_start_ts_like_go() {
    let mut job = Job::new(1, 2, 3, "drop table t");
    job.start_ts = 10;
    job.real_start_ts = 20;

    assert_eq!(recover_snapshot_ts(&job), 20);
    job.real_start_ts = 0;
    assert_eq!(recover_snapshot_ts(&job), 10);
}

#[test]
fn recover_candidates_filter_actions_validate_gc_and_short_circuit_like_go() {
    let mut create = Job::new(1, 1, 1, "create table t(a int)");
    create.action_type = ActionType::Other;
    create.real_start_ts = 1;
    let mut drop = Job::new(2, 1, 2, "drop table t");
    drop.action_type = ActionType::DropTable;
    drop.start_ts = 20;
    let mut truncate = Job::new(3, 1, 3, "truncate table t");
    truncate.action_type = ActionType::TruncateTable;
    truncate.real_start_ts = 30;

    let mut visited = Vec::new();
    assert!(
        drop_or_truncate_table_info_from_jobs(&[create.clone(), drop, truncate], 10, |job| {
            visited.push(job.id);
            job.id == 3
        })
        .unwrap()
    );
    assert_eq!(visited, vec![2, 3]);

    create.action_type = ActionType::DropTable;
    assert!(drop_or_truncate_table_info_from_jobs(&[create], 10, |_| false).is_err());
}

/// 验证重试间隔策略：索引在策略表范围内时返回对应间隔且标记 `changed=true`；
/// 索引超出策略表后固定返回最后一个间隔并标记 `changed=false`。
/// 该机制用于 DDL job 等待时的退避（backoff）轮询。
#[test]
fn interval_policy_uses_last_value_after_exhaustion() {
    let policy = [Duration::from_secs(1), Duration::from_secs(2)];
    assert_eq!(
        (Duration::from_secs(1), true),
        get_interval_from_policy(&policy, 0)
    );
    assert_eq!(
        (Duration::from_secs(2), true),
        get_interval_from_policy(&policy, 1)
    );
    assert_eq!(
        (Duration::from_secs(2), false),
        get_interval_from_policy(&policy, 2)
    );
    assert_eq!(
        (Duration::from_secs(2), false),
        get_interval_from_policy(&policy, 3)
    );
}

/// 验证字符集/排序规则解析：
/// - 未指定选项时回落到默认排序规则推导出的字符集（utf8mb4/utf8mb4_bin）；
/// - 显式指定字符集时采用该字符集及其默认排序规则；
/// - 同时指定多个互相冲突的字符集（utf8 与 utf8mb4）时必须返回错误，
///   对应 MySQL 中 CREATE DATABASE ... CHARACTER SET 选项冲突的语义。
#[test]
fn schema_charset_options_reject_conflicting_duplicates() {
    assert_eq!(
        ("utf8mb4".into(), "utf8mb4_bin".into()),
        resolve_charset_collation(&[], "utf8mb4_bin").unwrap()
    );
    assert_eq!(
        ("utf8".into(), "utf8_bin".into()),
        resolve_charset_collation(&[(Some("utf8".into()), None)], "utf8mb4_bin").unwrap()
    );
    assert!(
        resolve_charset_collation(
            &[(Some("utf8".into()), None), (Some("utf8mb4".into()), None),],
            "utf8mb4_bin",
        )
        .is_err()
    );
}

/// 验证标识符长度边界与 Go/TiDB 保持一致：
/// 空标识符非法，长度恰为 64 个字符合法，65 个字符则超限报错
/// （对应 MySQL 的最大标识符长度限制）。
#[test]
fn ddl_identifiers_enforce_the_go_length_boundary() {
    assert!(check_identifier("table_name", "table").is_ok());
    assert!(check_identifier("", "table").is_err());
    assert!(check_identifier(&"x".repeat(64), "table").is_ok());
    assert!(check_identifier(&"x".repeat(65), "table").is_err());
}
