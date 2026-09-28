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

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `test_alter_thread_right_after_job_finish` 负责 调整 thread right after 任务 finish。
// 中文总览：函数 `test_alter_job_on_dxf` 负责 调整 任务 on dxf。

//! Go-equivalent tests for `alter_job_test.go`.
//!
//! Mapping:
//! - `TestAlterThreadRightAfterJobFinish` → [`test_alter_thread_right_after_job_finish`]
//! - `TestAlterJobOnDXF` → [`test_alter_job_on_dxf`]

use astersql_tests_realtikvtest_addindextest2::harness::TestCtx;
use astersql_tests_realtikvtest_addindextest2::harness::testkit::NewTestKit;
use astersql_tests_realtikvtest_addindextest2::harness::{
    FailCtx, create_store, ingestctrl, kerneltype, model, operator, proto, require, reset_engine,
    serial_guard, testfailpoint, testutil,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `TestAlterThreadRightAfterJobFinish`.
// 该用例覆盖 调整 thread right after 任务 finish。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。
// 对于异步任务、外部存储或全局配置，本 case 的成功标准通常是多层状态同时一致。
// 这比单纯检查返回值更接近真实产品语义，也是迁移测试最需要保留的部分。
// 若未来扩展更多子场景，优先复用相同的上下文搭建方式，减少重复且保持可比性。
// 只要这些观察点仍然成立，就能说明迁移后的 Rust case 还在覆盖原始 Go 意图。
// 维护者阅读此处时，可把它视为场景说明书，而不是逐行复述 SQL 文本的注解。
// 当前注释刻意把注意力放在为什么这样验证，而不是每一条语句的语法细节。
// 当出现回归时，先检查这里记录的环境边界，往往比修改最终断言更有效。

#[test]
fn test_alter_thread_right_after_job_finish() {
    let _serial = serial_guard();
    if kerneltype::IsNextGen() {
        // Go: t.Skip("DXF is always enabled on nextgen")
        return;
    }
    reset_engine();
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = NewTestKit(&t, store.clone());
    tk.MustExec("use test");
    tk.MustExec("set global tidb_enable_dist_task=0;");
    t.Cleanup({
        let tk = NewTestKit(&t, store.clone());
        move || {
            tk.MustExec("set global tidb_enable_dist_task=1;");
        }
    });
    tk.MustExec("drop table if exists t;");
    tk.MustExec("create table t (c1 int primary key, c2 int)");
    tk.MustExec("insert t values (1, 1), (2, 2), (3, 3);");

    let updated = Arc::new(AtomicBool::new(false));
    {
        let updated = updated.clone();
        let store = store.clone();
        let t2 = t.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/checkJobCancelled",
            move |ctx| {
                if let FailCtx::Job(job) = ctx {
                    if !updated.load(Ordering::SeqCst)
                        && job.Type == model::ActionAddIndex
                        && job.SchemaState == model::StateWriteReorganization
                    {
                        updated.store(true, Ordering::SeqCst);
                        let tk2 = NewTestKit(&t2, store.clone());
                        tk2.MustExec(&format!("admin alter ddl jobs {} thread = 1", job.ID));
                    }
                }
            },
        );
    }

    let pipe_closed = Arc::new(AtomicBool::new(false));
    {
        let pipe_closed = pipe_closed.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/afterPipeLineClose",
            move |_ctx| {
                pipe_closed.store(true, Ordering::SeqCst);
                // Go sleeps 5s to widen the race window between pipeline close and param update.
                std::thread::sleep(Duration::from_secs(5));
            },
        );
    }

    let on_update = Arc::new(AtomicBool::new(false));
    {
        let on_update = on_update.clone();
        let pipe_closed = pipe_closed.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/onUpdateJobParam",
            move |_ctx| {
                if !on_update.swap(true, Ordering::SeqCst) {
                    while !pipe_closed.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            },
        );
    }

    tk.MustExec("alter table t add index idx(c2)");
    require::True(&t, updated.load(Ordering::SeqCst));
    require::True(&t, pipe_closed.load(Ordering::SeqCst));
    t.run_cleanups();
}

/// `TestAlterJobOnDXF`.
// 该用例覆盖 调整 任务 on dxf。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。
// 对于异步任务、外部存储或全局配置，本 case 的成功标准通常是多层状态同时一致。
// 这比单纯检查返回值更接近真实产品语义，也是迁移测试最需要保留的部分。
// 若未来扩展更多子场景，优先复用相同的上下文搭建方式，减少重复且保持可比性。
// 只要这些观察点仍然成立，就能说明迁移后的 Rust case 还在覆盖原始 Go 意图。
// 维护者阅读此处时，可把它视为场景说明书，而不是逐行复述 SQL 文本的注解。
// 当前注释刻意把注意力放在为什么这样验证，而不是每一条语句的语法细节。
// 当出现回归时，先检查这里记录的环境边界，往往比修改最终断言更有效。

#[test]
fn test_alter_job_on_dxf() {
    let _serial = serial_guard();
    if kerneltype::IsNextGen() {
        // Go: t.Skip("resource params are calculated automatically on nextgen...")
        return;
    }
    reset_engine();
    let t = TestCtx::new();
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/util/cpu/mockNumCpu",
        "return(16)",
    );
    testutil::ReduceCheckInterval(&t);

    let store = create_store(&t);
    let tk = NewTestKit(&t, store.clone());
    tk.MustExec("drop database if exists test;");
    tk.MustExec("create database test;");
    tk.MustExec("use test;");
    if kerneltype::IsClassic() {
        tk.MustExec("set global tidb_enable_dist_task=1;");
    }
    tk.MustExec("create table t1(a bigint auto_random primary key);");
    for _ in 0..16 {
        tk.MustExec("insert into t1 values (), (), (), ()");
    }
    tk.MustExec("split table t1 between (3) and (8646911284551352360) regions 50;");
    tk.MustExec("set @@tidb_ddl_reorg_worker_cnt = 1");
    tk.MustExec("set @@tidb_ddl_reorg_batch_size = 32");
    tk.MustExec("set @@global.tidb_cloud_storage_uri = \"\"");
    if kerneltype::IsClassic() {
        tk.MustExec("set global tidb_ddl_reorg_max_write_speed = 16");
        t.Cleanup({
            let tk = NewTestKit(&t, store.clone());
            move || {
                tk.MustExec("set global tidb_ddl_reorg_max_write_speed = 0");
            }
        });
    }

    let pipe_closed = Arc::new(AtomicBool::new(false));
    {
        let pipe_closed = pipe_closed.clone();
        let t_assert = t.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/afterPipeLineClose",
            move |ctx| {
                if let FailCtx::Pipeline(pipe) = ctx {
                    pipe_closed.store(true, Ordering::SeqCst);
                    let (reader, writer) = pipe.GetReaderAndWriter();
                    require::EqualValues(&t_assert, 4, reader.GetWorkerPoolSize());
                    require::EqualValues(&t_assert, 6, writer.GetWorkerPoolSize());
                }
            },
        );
    }

    let finished_subtasks = Arc::new(Mutex::new(0_i32));
    {
        let finished_subtasks = finished_subtasks.clone();
        let t_assert = t.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/mockDMLExecutionAddIndexSubTaskFinish",
            move |ctx| {
                if let FailCtx::Backend(be) = ctx {
                    *finished_subtasks.lock().unwrap() += 1;
                    require::EqualValues(&t_assert, 1024, be.GetWriteSpeedLimit());
                }
            },
        );
    }

    let modified = Arc::new(AtomicBool::new(false));
    {
        let modified = modified.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterDetectAndHandleParamModify",
            move |_ctx| {
                modified.store(true, Ordering::SeqCst);
            },
        );
    }

    let once = Arc::new(AtomicBool::new(false));
    {
        let once = once.clone();
        let modified = modified.clone();
        let store = store.clone();
        let t2 = t.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/scanRecordExec",
            move |ctx| {
                if once.swap(true, Ordering::SeqCst) {
                    return;
                }
                if let FailCtx::ReorgMeta(reorg_meta) = ctx {
                    let tk1 = NewTestKit(&t2, store.clone());
                    let rows = tk1
                        .MustQuery("select job_id from mysql.tidb_ddl_job")
                        .Rows();
                    require::Len(&t2, &rows, 1);
                    tk1.MustExec(&format!(
                        "admin alter ddl jobs {} thread = 8, batch_size = 256, max_write_speed=1024",
                        rows[0][0]
                    ));
                    require::Eventually(
                        &t2,
                        || modified.load(Ordering::SeqCst),
                        Duration::from_secs(20),
                        Duration::from_millis(100),
                    );
                    require::Equal(&t2, 256, reorg_meta.lock().unwrap().GetBatchSize());
                }
            },
        );
    }

    tk.MustExec("alter table t1 add index idx(a);");
    require::True(&t, pipe_closed.load(Ordering::SeqCst));
    require::EqualValues(&t, 1, *finished_subtasks.lock().unwrap());
    require::True(&t, modified.load(Ordering::SeqCst));
    tk.MustExec("admin check index t1 idx;");
    t.run_cleanups();
}
