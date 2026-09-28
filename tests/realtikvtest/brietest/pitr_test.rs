// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 中文总览：本文件承担 BR 备份恢复、日志备份、注册表与调度器 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：查询与断言保持 Go 参数绑定语义，避免把表名值误当成 SQL 标识符。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `test_pitr_and_backup_in_sql` 负责 pitr and 备份 in sql。
// 中文总览：函数 `test_pitr_and_restore_from_mid` 负责 pitr and 恢复 from mid。
// 中文总览：函数 `test_pitr_and_many_backups` 负责 pitr and many backups。
// 中文总览：函数 `test_pitr_and_encrypted_full_backup` 负责 pitr and encrypted full 备份。
// 中文总览：函数 `test_pitr_and_encrypted_log_backup` 负责 pitr and encrypted log 备份。
// 中文总览：函数 `test_pitr_and_both_encrypted` 负责 pitr and both encrypted。
// 中文总览：函数 `test_pitr_and_failure_restore` 负责 pitr and 失败恢复 恢复。

//! Go-equivalent tests for `pitr_test.go`.
//!
//! Mapping:
//! - `TestPiTRAndBackupInSQL` → [`test_pitr_and_backup_in_sql`]
//! - `TestPiTRAndRestoreFromMid` → [`test_pitr_and_restore_from_mid`]
//! - `TestPiTRAndManyBackups` → [`test_pitr_and_many_backups`]
//! - `TestPiTRAndEncryptedFullBackup` → [`test_pitr_and_encrypted_full_backup`]
//! - `TestPiTRAndEncryptedLogBackup` → [`test_pitr_and_encrypted_log_backup`]
//! - `TestPiTRAndBothEncrypted` → [`test_pitr_and_both_encrypted`]
//! - `TestPiTRAndFailureRestore` → [`test_pitr_and_failure_restore`]
//! - `TestPiTRAndIncrementalRestore` → [`test_pitr_and_incremental_restore`]
//! - `TestPiTRPauseMessage` → [`test_pitr_pause_message`]

use astersql_tests_realtikvtest_brietest::harness::{
    LogBackupKit, TestCtx, failpoint, require, reset_engine, serial_guard, task, testkit,
};
use std::sync::Arc;

/// `TestPiTRAndBackupInSQL`.
// 该用例覆盖 pitr and 备份 in sql。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_pitr_and_backup_in_sql() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let kit = LogBackupKit::new(&t);
    let s = kit.simpleWorkload();
    s.createSimpleTableWithData(&kit);
    s.insertSimpleIncreaseData(&kit);

    let task_name = "test_pitr_and_backup_in_sql";
    kit.RunFullBackup(|_| {});
    s.cleanSimpleData(&kit);

    let ts = kit.TSO();
    kit.RunFullBackup(|bc| {
        bc.Storage = kit.LocalURI("full2");
        bc.BackupTS = ts;
    });
    kit.RunLogStart(task_name, |sc| {
        sc.StartTS = ts;
    });
    let _ = kit.tk.MustQuery(&format!(
        "RESTORE TABLE test.{} FROM '{}'",
        s.tbl,
        kit.LocalURI("full")
    ));
    s.verifySimpleData(&kit);
    kit.forceFlushAndWait(task_name);

    s.cleanSimpleData(&kit);
    kit.StopTaskIfExists(task_name);
    kit.RunStreamRestore(|rc| {
        rc.FullBackupStorage = kit.LocalURI("full2");
    });
    s.verifySimpleData(&kit);
}

/// `TestPiTRAndRestoreFromMid`.
// 该用例覆盖 pitr and 恢复 from mid。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_pitr_and_restore_from_mid() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let kit = LogBackupKit::new(&t);
    let mut s = kit.simpleWorkload();
    s.createSimpleTableWithData(&kit);
    s.insertSimpleIncreaseData(&kit);
    let task_name = "test_pitr_and_restore_from_mid";

    kit.RunFullBackup(|bc| {
        kit.SetFilter(&mut bc.Config, &[&format!("test.{}", s.tbl)]);
        bc.Storage = kit.LocalURI("fulla");
    });
    s.cleanSimpleData(&kit);

    let mut s2 = kit.simpleWorkload();
    s2.tbl += "2";
    s2.createSimpleTableWithData(&kit);
    s2.insertSimpleIncreaseData(&kit);
    kit.RunFullBackup(|bc| {
        kit.SetFilter(&mut bc.Config, &[&format!("test.{}", s2.tbl)]);
        bc.Storage = kit.LocalURI("fullb");
    });
    s2.cleanSimpleData(&kit);

    kit.RunLogStart(task_name, |_| {});
    kit.RunFullRestore(|rc| {
        rc.Storage = kit.LocalURI("fulla");
        kit.SetFilter(&mut rc.Config, &[&format!("test.{}", s.tbl)]);
    });
    s.cleanSimpleData(&kit);

    let ts2 = kit.TSO();
    kit.RunFullBackup(|bc| {
        bc.Storage = kit.LocalURI("pitr_base_2");
        bc.BackupTS = ts2;
    });
    kit.RunFullRestore(|rc| {
        rc.Storage = kit.LocalURI("fullb");
        kit.SetFilter(&mut rc.Config, &[&format!("test.{}", s2.tbl)]);
    });

    kit.forceFlushAndWait(task_name);
    s.cleanSimpleData(&kit);
    s2.cleanSimpleData(&kit);
    kit.StopTaskIfExists(task_name);
    kit.RunStreamRestore(|rc| {
        rc.FullBackupStorage = kit.LocalURI("pitr_base_2");
    });
    s2.verifySimpleData(&kit);
    kit.tk.MustQuery(&table_absence_query(&s.tbl)).Check(&[]);
}

#[test]
fn table_absence_query_treats_table_name_as_a_value() {
    let t = TestCtx::new();
    require::Equal(
        &t,
        "SELECT * FROM information_schema.tables WHERE table_name = 'orders'".to_string(),
        table_absence_query("orders"),
    );
    require::Equal(
        &t,
        "SELECT * FROM information_schema.tables WHERE table_name = 'o''rders'".to_string(),
        table_absence_query("o'rders"),
    );
}

fn table_absence_query(table_name: &str) -> String {
    format!(
        "SELECT * FROM information_schema.tables WHERE table_name = '{}'",
        table_name.replace('\'', "''")
    )
}

/// `TestPiTRAndManyBackups`.
// 该用例覆盖 pitr and many backups。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_pitr_and_many_backups() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let kit = LogBackupKit::new(&t);
    let mut s = kit.simpleWorkload();
    s.createSimpleTableWithData(&kit);
    s.insertSimpleIncreaseData(&kit);
    let task_name = "test_pitr_and_many_backups";

    kit.RunFullBackup(|bc| {
        kit.SetFilter(&mut bc.Config, &[&format!("test.{}", s.tbl)]);
        bc.Storage = kit.LocalURI("fulla");
    });
    s.cleanSimpleData(&kit);

    let mut s2 = kit.simpleWorkload();
    s2.tbl += "2";
    s2.createSimpleTableWithData(&kit);
    s2.insertSimpleIncreaseData(&kit);
    kit.RunFullBackup(|bc| {
        kit.SetFilter(&mut bc.Config, &[&format!("test.{}", s2.tbl)]);
        bc.Storage = kit.LocalURI("fullb");
    });
    s2.cleanSimpleData(&kit);

    let ts = kit.TSO();
    kit.RunFullBackup(|bc| {
        bc.Storage = kit.LocalURI("pitr_base");
        bc.BackupTS = ts;
    });
    kit.RunLogStart(task_name, |sc| {
        sc.StartTS = ts;
    });
    kit.RunFullRestore(|rc| {
        rc.Storage = kit.LocalURI("fulla");
        kit.SetFilter(&mut rc.Config, &[&format!("test.{}", s.tbl)]);
    });
    kit.RunFullRestore(|rc| {
        rc.Storage = kit.LocalURI("fullb");
        kit.SetFilter(&mut rc.Config, &[&format!("test.{}", s2.tbl)]);
    });

    kit.forceFlushAndWait(task_name);
    s.cleanSimpleData(&kit);
    s2.cleanSimpleData(&kit);
    kit.StopTaskIfExists(task_name);
    kit.RunStreamRestore(|rc| {
        rc.FullBackupStorage = kit.LocalURI("pitr_base");
    });
    s.verifySimpleData(&kit);
    s2.verifySimpleData(&kit);
}

/// `TestPiTRAndEncryptedFullBackup`.
// 该用例覆盖 pitr and encrypted full 备份。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_pitr_and_encrypted_full_backup() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let kit = LogBackupKit::new(&t);
    let s = kit.simpleWorkload();
    s.createSimpleTableWithData(&kit);
    let key = hex_decode("9d4cf8f268514d2c38836197008eded1050a5806afa632f7ab1e313bb6697da2");

    kit.RunFullBackup(|bc| {
        bc.CipherInfo = task::CipherInfo {
            CipherType: task::encryptionpb::EncryptionMethod_AES256_CTR,
            CipherKey: key.clone(),
        };
    });
    s.cleanSimpleData(&kit);
    kit.RunLogStart("enc_full", |_| {});
    {
        let t = t.clone();
        kit.WithChecker(
            move |err| {
                require::ErrorContains(&t, err, "the data you want to restore is encrypted");
            },
            || {
                kit.RunFullRestore(|rc| {
                    rc.CipherInfo = task::CipherInfo {
                        CipherType: task::encryptionpb::EncryptionMethod_AES256_CTR,
                        CipherKey: key.clone(),
                    };
                });
            },
        );
    }
}

/// `TestPiTRAndEncryptedLogBackup`.
// 该用例覆盖 pitr and encrypted log 备份。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_pitr_and_encrypted_log_backup() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let kit = LogBackupKit::new(&t);
    let s = kit.simpleWorkload();
    s.createSimpleTableWithData(&kit);
    let key = hex_decode("0ae31c060ff933cabe842430e1716185cc9c6b5cdde8e56976afaff41b92528f");
    let key_file = kit.tempFile("KEY", &key);

    kit.RunFullBackup(|_| {});
    s.cleanSimpleData(&kit);
    kit.RunLogStart("enc_log", |sc| {
        sc.MasterKeyConfig.EncryptionType = task::encryptionpb::EncryptionMethod_AES256_CTR;
        sc.MasterKeyConfig.MasterKeys.push(task::MasterKey {
            file_path: key_file.clone(),
        });
    });
    {
        let t = t.clone();
        kit.WithChecker(
            move |err| {
                require::ErrorContains(&t, err, "the running log backup task is encrypted");
            },
            || {
                kit.RunFullRestore(|_| {});
            },
        );
    }
}

/// `TestPiTRAndBothEncrypted`.
// 该用例覆盖 pitr and both encrypted。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_pitr_and_both_encrypted() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let kit = LogBackupKit::new(&t);
    let s = kit.simpleWorkload();
    s.createSimpleTableWithData(&kit);
    let key = hex_decode("319b4a104651746f1bf1ad67c9ba7d635d8c4769b03f3e5c63f1da93891ce4f9");
    let key_file = kit.tempFile("KEY", &key);

    kit.RunFullBackup(|bc| {
        bc.CipherInfo = task::CipherInfo {
            CipherType: task::encryptionpb::EncryptionMethod_AES256_CTR,
            CipherKey: key.clone(),
        };
    });
    s.cleanSimpleData(&kit);
    kit.RunLogStart("both_enc", |sc| {
        sc.MasterKeyConfig.EncryptionType = task::encryptionpb::EncryptionMethod_AES256_CTR;
        sc.MasterKeyConfig.MasterKeys.push(task::MasterKey {
            file_path: key_file.clone(),
        });
    });
    {
        let t = t.clone();
        kit.WithChecker(
            move |err| {
                require::ErrorContains(&t, err, "encrypted");
            },
            || {
                kit.RunFullRestore(|rc| {
                    rc.CipherInfo = task::CipherInfo {
                        CipherType: task::encryptionpb::EncryptionMethod_AES256_CTR,
                        CipherKey: key.clone(),
                    };
                });
            },
        );
    }
}

/// `TestPiTRAndFailureRestore`.
// 该用例覆盖 pitr and 失败恢复 恢复。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_pitr_and_failure_restore() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let kit = LogBackupKit::new(&t);
    let s = kit.simpleWorkload();
    s.createSimpleTableWithData(&kit);
    s.insertSimpleIncreaseData(&kit);
    let task_name = "test_pitr_and_failure_restore";
    kit.RunFullBackup(|_| {});
    s.cleanSimpleData(&kit);

    let ts = kit.TSO();
    kit.RunFullBackup(|bc| {
        bc.Storage = kit.LocalURI("full2");
        bc.BackupTS = ts;
    });
    kit.RunLogStart(task_name, |sc| {
        sc.StartTS = ts;
    });
    require::NoError(
        &t,
        failpoint::EnableErrCall(
            "github.com/pingcap/tidb/br/pkg/task/run-snapshot-restore-about-to-finish",
            Arc::new(|e| {
                *e = Some("not my fault".into());
            }),
        ),
    );
    {
        let t = t.clone();
        kit.WithChecker(
            move |err| {
                require::Error(&t, err);
            },
            || {
                kit.RunFullRestore(|rc| {
                    rc.UseCheckpoint = false;
                });
            },
        );
    }
    kit.forceFlushAndWait(task_name);
    s.cleanSimpleData(&kit);
    require::NoError(
        &t,
        failpoint::Disable(
            "github.com/pingcap/tidb/br/pkg/task/run-snapshot-restore-about-to-finish",
        ),
    );
    kit.StopTaskIfExists(task_name);
    kit.RunStreamRestore(|rc| {
        rc.FullBackupStorage = kit.LocalURI("full2");
    });
    kit.tk
        .MustQuery(&format!("SELECT COUNT(*) FROM test.{}", s.tbl))
        .Check(&testkit::Rows(&["0"]));
}

/// `TestPiTRAndIncrementalRestore`.
// 该用例覆盖 pitr and incremental 恢复。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_pitr_and_incremental_restore() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let kit = LogBackupKit::new(&t);
    let s = kit.simpleWorkload();
    s.createSimpleTableWithData(&kit);
    kit.RunFullBackup(|bc| {
        kit.SetFilter(&mut bc.Config, &[&format!("test.{}", s.tbl)]);
    });
    s.insertSimpleIncreaseData(&kit);
    let ts = kit.TSO();
    kit.RunFullBackup(|bc| {
        kit.SetFilter(&mut bc.Config, &[&format!("test.{}", s.tbl)]);
        bc.Storage = kit.LocalURI("incr-legacy");
        bc.LastBackupTS = ts;
    });
    s.cleanSimpleData(&kit);
    kit.RunLogStart("dummy", |_| {});
    kit.RunFullRestore(|_| {});
    {
        let t = t.clone();
        kit.WithChecker(
            move |err| {
                require::ErrorContains(&t, err, "BR:Stream:ErrStreamLogTaskExist");
            },
            || {
                kit.RunFullRestore(|rc| {
                    rc.Storage = kit.LocalURI("incr-legacy");
                });
            },
        );
    }
}

/// `TestPiTRPauseMessage`.
// 该用例覆盖 pitr 暂停 信息。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_pitr_pause_message() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let kit = LogBackupKit::new(&t);
    kit.RunLogStart("nothing", |_| {});
    kit.RunLogPause("nothing", |sc| {
        sc.Message = "nothing paused".into();
    });
    let s = kit.RunLogStatus(|_| {});
    require::Len(&t, &s, 1);
    let pl = s[0].PauseV2.GetPayload().unwrap();
    let hn = hostname();
    require::Equal(&t, "PAUSE", s[0].StatusString());
    require::Equal(&t, hn.clone(), s[0].PauseV2.OperatorHostName.clone());
    require::Equal(&t, "nothing paused".to_string(), pl);

    kit.StopTaskIfExists("nothing");
    kit.RunLogStart("nothing2", |_| {});
    kit.RunLogPause("nothing2", |sc| {
        sc.AsError = true;
        sc.Message = "nothing is on fire".into();
    });
    let s = kit.RunLogStatus(|_| {});
    require::Len(&t, &s, 1);
    let pl = s[0].PauseV2.GetPayload().unwrap();
    require::Equal(&t, "ERROR", s[0].StatusString());
    require::Equal(&t, hn, s[0].PauseV2.OperatorHostName.clone());
    require::Equal(&t, "nothing is on fire".to_string(), pl);
}

// 该辅助函数负责 hex decode。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

// 该辅助函数负责 hostname。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "localhost".into())
}
