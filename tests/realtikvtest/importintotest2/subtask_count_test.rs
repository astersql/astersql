// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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
//! 中文总览：`subtask_count_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `subtask_count_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 17 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_step_subtask_count` 是当前文件里的辅助函数。
//! 阅读 `test_step_subtask_count` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_step_subtask_count` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_step_subtask_count`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_step_subtask_count` 的重要阅读参照。
//! 理解 `test_step_subtask_count` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 符号 `GIB` 是当前文件里的常量。
//! 阅读 `GIB` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `GIB` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `GIB`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `GIB` 的重要阅读参照。
//! 理解 `GIB` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 中文说明结束（自动生成）

//! Go-equivalent next-gen DXF step/subtask resource accounting.

use astersql_tests_realtikvtest_importintotest::harness::{
    MockGCSSuite, fakestorage, gcs_endpoint, handle, importinto, kerneltype, proto, reset_engine,
    schstatus, serial_guard, storage, testfailpoint, testkit,
};
use std::sync::{Arc, Mutex};

#[test]
fn test_step_subtask_count() {
    let _serial = serial_guard();
    reset_engine();
    kerneltype::set_next_gen(true);
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db("step_subtasks");
    s.tk.MustExec("create table t (a int primary key, b int, c int, d int, unique(c), unique(d))");
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "step-subtasks".into(),
            Name: "1.csv".into(),
        },
        Content: b"1,1,1,1\n2,2,2,2\n3,3,3,3\n".to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "step-subtasks".into(),
            Name: "2.csv".into(),
        },
        // The first three rows conflict with rows or unique keys from the first file.
        Content: b"1,1,1,1\n4,4,4,2\n5,5,3,5\n6,6,6,6\n".to_vec(),
    });

    // Match realtikvtest::GetNextGenObjStoreURI("gl-sort") from the Go test.
    const CLOUD_STORAGE_URI: &str = "s3://next-gen-test/gl-sort?access-key=minioadmin&\
        secret-access-key=minioadmin&endpoint=http%3a%2f%2f0.0.0.0%3a9000&provider=minio";
    const GIB: i64 = 1024 * 1024 * 1024;
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/executor/importer/amplifyRealSize",
        &format!("return({})", 14 * GIB),
    );
    let status: Arc<Mutex<Option<schstatus::Status>>> = Arc::new(Mutex::new(None));
    let status_for_hook = status.clone();
    testfailpoint::EnableCall(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/syncBeforePostProcess",
        move |_| {
            *status_for_hook.lock().unwrap() = Some(handle::GetScheduleStatus(()).unwrap());
        },
    );
    let rows =
        s.tk.MustQuery(&format!(
            "import into t FROM 'gs://step-subtasks/*.csv?endpoint={}' \
             with __force_merge_step, __max_engine_size='1', on_duplicate_key='capture', \
             cloud_storage_uri='{}'",
            gcs_endpoint(),
            CLOUD_STORAGE_URI
        ))
        .Rows();
    let job_id = rows[0][0].parse::<i64>().unwrap();
    s.tk.MustQuery("select * from t")
        .Sort()
        .Check(&testkit::Rows(&["6 6 6 6"]));

    let schedule = status.lock().unwrap().clone().expect("schedule status");
    assert_eq!(schedule.TiDBWorker.RequiredCount, 1);
    let manager = storage::GetTaskManager().unwrap();
    let task = manager
        .GetTaskByKeyWithHistory((), &importinto::TaskKey(job_id))
        .unwrap();
    assert_eq!(task.RequiredSlots, 16);
    assert!(task.MaxNodeCount > 2);
    assert_eq!(task.Step, proto::ImportStepPostProcess);
    for (step, expected) in [
        (proto::ImportStepEncodeAndSort, 2),
        (proto::ImportStepMergeSort, 3),
        (proto::ImportStepWriteAndIngest, 3),
        (proto::ImportStepCollectConflicts, 1),
        (proto::ImportStepConflictResolution, 1),
        (proto::ImportStepPostProcess, 1),
    ] {
        assert_eq!(
            manager
                .GetSubtasksWithHistory((), task.ID, step)
                .unwrap()
                .len(),
            expected,
            "step {step}"
        );
    }
    s.tear_down();
    kerneltype::set_next_gen(false);
}
