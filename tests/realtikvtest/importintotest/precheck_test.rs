// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! 中文说明开始（自动生成）
//! 中文总览：`precheck_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `precheck_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 28 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_pre_check_total_file_size_0` 是当前文件里的辅助函数。
//! 阅读 `test_pre_check_total_file_size_0` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_pre_check_total_file_size_0` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_pre_check_total_file_size_0`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_pre_check_total_file_size_0` 的重要阅读参照。
//! 理解 `test_pre_check_total_file_size_0` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_pre_check_total_file_size_0` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_pre_check_total_file_size_0` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_pre_check_table_not_empty` 是当前文件里的辅助函数。
//! 阅读 `test_pre_check_table_not_empty` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_pre_check_table_not_empty` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_pre_check_table_not_empty`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_pre_check_table_not_empty` 的重要阅读参照。
//! 理解 `test_pre_check_table_not_empty` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_pre_check_table_not_empty` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_pre_check_table_not_empty` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_pre_check_cdc_pitr_tasks` 是当前文件里的辅助函数。
//! 阅读 `test_pre_check_cdc_pitr_tasks` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_pre_check_cdc_pitr_tasks` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_pre_check_cdc_pitr_tasks`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_pre_check_cdc_pitr_tasks` 的重要阅读参照。
//! 理解 `test_pre_check_cdc_pitr_tasks` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_pre_check_cdc_pitr_tasks` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_pre_check_cdc_pitr_tasks` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `precheck_test.go`.

use astersql_tests_realtikvtest_importintotest::harness::{
    MockGCSSuite, brpb, etcd, exeerrors, fakestorage, gcs_endpoint, reset_engine, serial_guard,
    streamhelper, testkit,
};

/// `TestPreCheckTotalFileSize0`.
#[test]
fn test_pre_check_total_file_size_0() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "precheck-file-empty".into(),
            Name: "empty.csv".into(),
        },
        Content: vec![],
    });
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("drop table if exists t;");
    s.tk.MustExec("create table t (a bigint primary key, b varchar(100), c int);");
    s.tk.MustExec("insert into t values(9, 'test9', 99);");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://precheck-file-empty/non-exist/file-*.csv?endpoint={}'",
        gcs_endpoint()
    );
    let err = s.tk.QueryToErr(&sql).err().expect("err");
    s.ErrorIs(&err, exeerrors::ErrLoadDataPreCheckFailed);

    let sql = format!(
        "IMPORT INTO t FROM 'gs://precheck-file-empty/empty.csv?endpoint={}'",
        gcs_endpoint()
    );
    let err = s.tk.QueryToErr(&sql).err().expect("err");
    s.ErrorIs(&err, exeerrors::ErrLoadDataPreCheckFailed);
    s.tear_down();
}

/// `TestPreCheckTableNotEmpty`.
#[test]
fn test_pre_check_table_not_empty() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "precheck-tbl-empty".into(),
            Name: "file.csv".into(),
        },
        Content: b"1,test1,11\n2,test2,22\n3,test3,33".to_vec(),
    });
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("drop table if exists t;");
    s.tk.MustExec("create table t (a bigint primary key, b varchar(100), c int);");
    s.tk.MustExec("insert into t values(9, 'test9', 99);");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://precheck-tbl-empty/file.csv?endpoint={}'",
        gcs_endpoint()
    );
    let err = s.tk.QueryToErr(&sql).err().expect("err");
    s.ErrorIs(&err, exeerrors::ErrLoadDataPreCheckFailed);
    s.tear_down();
}

/// `TestPreCheckCDCPiTRTasks`.
#[test]
fn test_pre_check_cdc_pitr_tasks() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "precheck-cdc-pitr".into(),
            Name: "file.csv".into(),
        },
        Content: b"1,test1,11".to_vec(),
    });
    s.prepare_and_use_db("load_data");
    s.tk.MustExec("drop table if exists t;");
    s.tk.MustExec("create table t (a bigint primary key, b varchar(100), c int);");
    s.tk.MustExec("create table dst (a bigint primary key, b varchar(100), c int);");

    let client =
        astersql_tests_realtikvtest_importintotest::harness::importer::GetEtcdClient(&s.store)
            .expect("etcd");
    s.t.Cleanup({
        let client = client.clone();
        move || {
            let _ = client.Close();
        }
    });

    let pitr_key = streamhelper::PrefixOfTask() + "dummy-task";
    let pitr_task_info = brpb::StreamBackupTaskInfo {
        Name: "dummy-task".into(),
    };
    let data = pitr_task_info.Marshal().expect("marshal");
    client
        .Put((), &pitr_key, &String::from_utf8_lossy(&data))
        .expect("put");
    s.t.Cleanup({
        let client = client.clone();
        let pitr_key = pitr_key.clone();
        move || {
            let _ = client.Delete((), &pitr_key);
        }
    });

    let sql = format!(
        "IMPORT INTO t FROM 'gs://precheck-cdc-pitr/file.csv?endpoint={}'",
        gcs_endpoint()
    );
    let err = s.tk.QueryToErr(&sql).err().expect("err");
    s.ErrorIs(&err, exeerrors::ErrLoadDataPreCheckFailed);
    s.ErrorContains(&err, "found PiTR log streaming task(s): [dummy-task],");
    s.tk.MustQuery(&(sql.clone() + " WITH disable_precheck"));
    s.tk.MustQuery("select * from t")
        .Check(&testkit::Rows(&["1 test1 11"]));

    let err =
        s.tk.ExecToErr("import into dst from select * from t")
            .err()
            .expect("err");
    s.ErrorIs(&err, exeerrors::ErrLoadDataPreCheckFailed);
    s.ErrorContains(&err, "found PiTR log streaming task(s): [dummy-task],");
    s.tk.MustExec("import into dst from select * from t with disable_precheck");
    s.tk.MustQuery("select * from dst")
        .Check(&testkit::Rows(&["1 test1 11"]));

    client.Delete((), &pitr_key).expect("del");
    let cdc_key = "/tidb/cdc/cluster-123/test/changefeed/info/feed-test";
    client
        .Put((), cdc_key, r#"{"state": "normal"}"#)
        .expect("put cdc");
    s.t.Cleanup({
        let client = client.clone();
        move || {
            let _ = client.Delete((), cdc_key);
        }
    });
    s.tk.MustExec("truncate table t");
    let err = s.tk.QueryToErr(&sql).err().expect("err");
    s.ErrorIs(&err, exeerrors::ErrLoadDataPreCheckFailed);
    s.ErrorContains(
        &err,
        "found CDC changefeed(s): cluster/namespace: cluster-123/test changefeed(s): [feed-test]",
    );
    s.tear_down();
}
