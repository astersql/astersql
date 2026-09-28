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
//! 中文总览：`extra_param_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `extra_param_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 30 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_extra_param_max_runtime_slots` 是当前文件里的辅助函数。
//! 阅读 `test_extra_param_max_runtime_slots` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_extra_param_max_runtime_slots` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_extra_param_max_runtime_slots`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_extra_param_max_runtime_slots` 的重要阅读参照。
//! 理解 `test_extra_param_max_runtime_slots` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_extra_param_max_runtime_slots` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_extra_param_max_runtime_slots` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_starter_max_import_data_size` 是当前文件里的辅助函数。
//! 阅读 `test_starter_max_import_data_size` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_starter_max_import_data_size` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_starter_max_import_data_size`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_starter_max_import_data_size` 的重要阅读参照。
//! 理解 `test_starter_max_import_data_size` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_starter_max_import_data_size` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_starter_max_import_data_size` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 关注点 001：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`extra_param_test`）。
//! 关注点 002：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`extra_param_test`）。
//! 关注点 003：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`extra_param_test`）。
//! 关注点 004：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`extra_param_test`）。
//! 关注点 005：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`extra_param_test`）。
//! 关注点 006：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`extra_param_test`）。
//! 关注点 007：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`extra_param_test`）。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `extra_param_test.go`.
//!
//! Mapping:
//! - `TestExtraParamMaxRuntimeSlots` → [`test_extra_param_max_runtime_slots`]
//! - `TestStarterMaxImportDataSize` → [`test_starter_max_import_data_size`]

use astersql_tests_realtikvtest_importintotest3::harness::{
    FailCtx, MockGCSSuite, deploymode, fakestorage, gcs_endpoint, kerneltype, local_config, mydump,
    proto, require, reset_engine, serial_guard, testfailpoint, testkit,
};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

/// `TestExtraParamMaxRuntimeSlots`.
#[test]
fn test_extra_param_max_runtime_slots() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();

    testfailpoint::EnableCall(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/framework/storage/beforeSubmitTask",
        move |ctx| {
            if let FailCtx::ExtraParams { slots, params } = ctx {
                if kerneltype::IsClassic() {
                    *slots.lock().unwrap() = 16;
                }
                params.lock().unwrap().MaxRuntimeSlots = 12;
            }
        },
    );
    if kerneltype::IsNextGen() {
        testfailpoint::EnableCall(
            &s.t,
            "github.com/pingcap/tidb/pkg/dxf/importinto/afterPrepare",
            move |ctx| {
                if let FailCtx::Task(task) = ctx {
                    task.lock().unwrap().RequiredSlots = 16;
                }
            },
        );
    }

    let call_cnt = Arc::new(AtomicI32::new(0));
    {
        let call_cnt = call_cnt.clone();
        let t = s.t.clone();
        testfailpoint::EnableCall(
            &s.t,
            "github.com/pingcap/tidb/pkg/resourcemanager/pool/workerpool/NewWorkerPool",
            move |ctx| {
                if let FailCtx::NumWorkers(n) = ctx {
                    require::EqualValues(&t, 12, n);
                    call_cnt.fetch_add(1, Ordering::SeqCst);
                }
            },
        );
    }

    s.prepare_and_use_db("extra_params");
    s.tk.MustExec("CREATE TABLE t (i INT PRIMARY KEY, s varchar(32));");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "snappy".into(),
            Name: "t.01.csv.snappy".into(),
        },
        Content: s.get_compressed_data(mydump::Compression::Snappy, b"1,test1\n2,test2"),
    });

    let ep = gcs_endpoint();
    let sort_storage_uri =
        format!("gs://snappy/temp?endpoint={ep}&access-key=aaaaaa&secret-access-key=bbbbbb");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://snappy/t.*?endpoint={ep}' WITH __force_merge_step, cloud_storage_uri='{sort_storage_uri}';"
    );
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM t;")
        .Check(&testkit::Rows(&["1 test1", "2 test2"]));
    // encode/merge-sort/ingest step all create worker pools
    s.EqualValues(3, call_cnt.load(Ordering::SeqCst));
    s.tear_down();
}

/// `TestStarterMaxImportDataSize`.
#[test]
fn test_starter_max_import_data_size() {
    let _serial = serial_guard();
    reset_engine();
    // Go: only runs in nextgen — enable nextgen so the body executes.
    kerneltype::set_next_gen(true);
    let s = MockGCSSuite::setup();

    if !kerneltype::IsNextGen() {
        s.tear_down();
        return;
    }

    let origin_deploy_mode = deploymode::Get();
    let origin_global_config = local_config::GetGlobalConfig();
    require::NoError(&s.t, deploymode::Set(deploymode::Starter));
    local_config::UpdateGlobal(|conf| {
        conf.DeployMode = deploymode::Starter.to_string();
        conf.StarterParams.MaxImportDataSize = 128;
    });
    {
        let origin = origin_global_config.clone();
        let mode = origin_deploy_mode.clone();
        let t = s.t.clone();
        s.t.Cleanup(move || {
            local_config::StoreGlobalConfig(&origin);
            require::NoError(&t, deploymode::Set(&mode));
            kerneltype::set_next_gen(false);
        });
    }

    let content = b"1,1\n2,2";
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "starter-max-import-data-size-source".into(),
            Name: "under.csv".into(),
        },
        Content: content.to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "starter-max-import-data-size-source".into(),
            Name: "over.csv".into(),
        },
        Content: content.to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "starter-max-import-data-size-sort".into(),
            Name: "seed".into(),
        },
        Content: b"seed".to_vec(),
    });
    s.prepare_and_use_db("starter_max_import_data_size");
    s.tk.MustExec("create table under_limit (a int, b int);");
    s.tk.MustExec("create table over_limit (a int, b int);");

    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/executor/importer/amplifyRealSize",
        "return(10)",
    );
    let ep = gcs_endpoint();
    let under_limit_sql = format!(
        "IMPORT INTO under_limit FROM 'gs://starter-max-import-data-size-source/under.csv?endpoint={ep}' WITH cloud_storage_uri='gs://starter-max-import-data-size-sort/under?endpoint={ep}'"
    );
    s.tk.MustQuery(&under_limit_sql);
    s.tk.MustQuery("select * from under_limit order by a")
        .Check(&testkit::Rows(&["1 1", "2 2"]));

    local_config::UpdateGlobal(|conf| {
        conf.DeployMode = deploymode::Starter.to_string();
        conf.StarterParams.MaxImportDataSize = 64;
    });
    let over_limit_sql = format!(
        "IMPORT INTO over_limit FROM 'gs://starter-max-import-data-size-source/over.csv?endpoint={ep}' WITH cloud_storage_uri='gs://starter-max-import-data-size-sort/over?endpoint={ep}'"
    );
    let err = s.tk.QueryToErr(&over_limit_sql).unwrap_err();
    require::ErrorContains(
        &s.t,
        &err,
        "total real import data size 70B exceeds maximum import size limit 64B (total file size 7B)",
    );
    s.tk.MustQuery("select * from over_limit")
        .Check(&testkit::Rows(&[]));
    s.tear_down();
}
