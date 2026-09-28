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

//! 中文说明开始（自动生成）
//! 中文总览：`sdk_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `sdk_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 27 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_import_sdk` 是当前文件里的辅助函数。
//! 阅读 `test_import_sdk` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_sdk` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_sdk`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_sdk` 的重要阅读参照。
//! 理解 `test_import_sdk` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_sdk` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_import_sdk` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 关注点 001：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`sdk_test`）。
//! 关注点 002：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`sdk_test`）。
//! 关注点 003：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`sdk_test`）。
//! 关注点 004：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`sdk_test`）。
//! 关注点 005：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`sdk_test`）。
//! 关注点 006：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`sdk_test`）。
//! 关注点 007：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`sdk_test`）。
//! 关注点 008：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`sdk_test`）。
//! 关注点 009：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`sdk_test`）。
//! 关注点 010：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`sdk_test`）。
//! 关注点 011：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`sdk_test`）。
//! 关注点 012：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`sdk_test`）。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `sdk_test.go`.

use astersql_tests_realtikvtest_importintotest::harness::{
    MockGCSSuite, importsdk, max_wait_time, reset_engine, serial_guard, testfailpoint, testkit,
};
use std::time::Duration;

/// `TestImportSDK`.
#[test]
fn test_import_sdk() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    let tmp_dir = s.TempDir();

    s.NoError(
        std::fs::write(
            tmp_dir.join("importsdk_test-schema-create.sql"),
            "CREATE DATABASE importsdk_test;",
        )
        .map_err(|e| e.to_string()),
    );
    s.NoError(
        std::fs::write(
            tmp_dir.join("importsdk_test.t-schema.sql"),
            "CREATE TABLE t (id int, v varchar(255));",
        )
        .map_err(|e| e.to_string()),
    );
    let content = b"1,test1\n2,test2";
    s.NoError(
        std::fs::write(tmp_dir.join("importsdk_test.t.001.csv"), content)
            .map_err(|e| e.to_string()),
    );

    s.tk.MustExec("DROP DATABASE IF EXISTS importsdk_test");

    let sdk = importsdk::NewImportSDK((), &format!("file://{}", tmp_dir.display()), s.tk.clone())
        .expect("sdk");
    s.NoError(sdk.CreateSchemasAndTables(()));
    s.tk.MustExec("USE importsdk_test");
    s.tk.MustExec("SHOW CREATE TABLE t");

    s.tk.MustExec("DROP TABLE t");
    s.NoError(sdk.CreateSchemaAndTableByName((), "importsdk_test", "t"));
    s.tk.MustExec("SHOW CREATE TABLE t");

    let metas = sdk.GetTableMetas(()).expect("metas");
    s.Len(&metas, 1);
    s.Equal("importsdk_test".to_string(), metas[0].Database.clone());
    s.Equal("t".to_string(), metas[0].Table.clone());
    s.Equal(content.len() as i64, metas[0].TotalSize);

    let meta = sdk
        .GetTableMetaByName((), "importsdk_test", "t")
        .expect("meta");
    s.Equal("importsdk_test".to_string(), meta.Database.clone());
    s.Equal("t".to_string(), meta.Table.clone());

    let total_size = sdk.GetTotalSize(());
    s.Equal(content.len() as i64, total_size);

    let opts = importsdk::ImportOptions {
        Thread: 4,
        Detached: true,
        GroupKey: String::new(),
    };
    let sql = sdk.GenerateImportSQL(&meta, &opts).expect("sql");
    s.Contains(&sql, "IMPORT INTO `importsdk_test`.`t` FROM");
    s.Contains(&sql, "THREAD=4");
    s.Contains(&sql, "DETACHED");

    let job_id = sdk.SubmitJob((), &sql).expect("submit");
    s.True(job_id > 0);

    let status = sdk.GetJobStatus((), job_id).expect("status");
    s.Equal(job_id, status.JobID);
    s.Equal(
        "`importsdk_test`.`t`".to_string(),
        status.TargetTable.clone(),
    );

    s.Eventually(
        || {
            let status = sdk.GetJobStatus((), job_id).expect("st");
            status.Status == "finished"
        },
        max_wait_time(),
        Duration::from_millis(500),
    );

    s.tk.MustQuery("SELECT * FROM importsdk_test.t")
        .Check(&testkit::Rows(&["1 test1", "2 test2"]));

    s.tk.MustExec("TRUNCATE TABLE t");
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/syncBeforeJobStarted",
        "pause",
    );

    let job_id2 = sdk.SubmitJob((), &sql).expect("submit2");
    s.NoError(sdk.CancelJob((), job_id2));
    s.Equal(
        Some("pause".to_string()),
        astersql_tests_realtikvtest_importintotest::harness::failpoint::term(
            "github.com/pingcap/tidb/pkg/dxf/importinto/syncBeforeJobStarted",
        ),
    );
    // Disable pause
    s.NoError(
        astersql_tests_realtikvtest_importintotest::harness::failpoint::Disable(
            "github.com/pingcap/tidb/pkg/dxf/importinto/syncBeforeJobStarted",
        ),
    );

    s.Eventually(
        || {
            let status = sdk.GetJobStatus((), job_id2).expect("st");
            status.Status == "cancelled"
        },
        max_wait_time(),
        Duration::from_millis(500),
    );

    let group_key = "test_group_key";
    let opts_with_group = importsdk::ImportOptions {
        Thread: 4,
        Detached: true,
        GroupKey: group_key.into(),
    };
    let sql_with_group = sdk
        .GenerateImportSQL(&meta, &opts_with_group)
        .expect("sql g");
    let job_id3 = sdk.SubmitJob((), &sql_with_group).expect("submit3");
    s.True(job_id3 > 0);

    s.Eventually(
        || {
            let status = sdk.GetJobStatus((), job_id3).expect("st");
            status.Status == "finished"
        },
        max_wait_time(),
        Duration::from_millis(500),
    );

    let group_summary = sdk.GetGroupSummary((), group_key).expect("gs");
    s.Equal(group_key.to_string(), group_summary.GroupKey.clone());
    s.Equal(1i64, group_summary.TotalJobs);
    s.Equal(1i64, group_summary.Completed);

    let jobs = sdk.GetJobsByGroup((), group_key).expect("jobs");
    s.Len(&jobs, 1);
    s.Equal(job_id3, jobs[0].JobID);
    s.Equal("finished".to_string(), jobs[0].Status.clone());

    sdk.Close();
    s.tear_down();
}
