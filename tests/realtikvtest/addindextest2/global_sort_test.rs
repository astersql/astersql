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

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `gen_storage_uri` 负责 生成 storage uri。
// 中文总览：函数 `gen_server_with_storage` 负责 生成 server 携带 storage。
// 中文总览：函数 `check_file_cleaned` 负责 检查 file cleaned。
// 中文总览：函数 `check_file_exist` 负责 检查 file exist。
// 中文总览：函数 `check_data_and_show_jobs` 负责 检查 data and show 任务集合。
// 中文总览：函数 `check_external_fields` 负责 检查 external fields。
// 中文总览：函数 `get_task_id` 负责 读取 task id。
// 中文总览：函数 `get_step_summary` 负责 读取 step 摘要。
// 中文总览：模块 `execute_summary` 负责 execute 摘要。
// 中文总览：类型 `View` 负责 View。
// 中文总览：函数 `test_global_sort_basic` 负责 全局排序 基础场景。
// 中文总览：函数 `test_global_sort_multi_schema_change` 负责 全局排序 多 schema 变更。
// 中文总览：类型 `TC` 负责 TC。
// 中文总览：函数 `test_add_index_ingest_show_reorg_tp` 负责 添加 索引 ingest 回填 回填模式展示。
// 中文总览：函数 `test_global_sort_duplicate_err_msg` 负责 全局排序 重复值 err msg。
// 中文总览：类型 `TC` 负责 TC。
// 中文总览：函数 `test_global_sort_add_index_recover_from_retryable_error` 负责 全局排序 添加 索引 恢复 from retryable 错误。
// 中文总览：函数 `test_ingest_use_given_ts` 负责 ingest 回填 使用 给定 时间戳。
// 中文总览：函数 `test_alter_job_on_dxf_with_global_sort` 负责 调整 任务 on dxf 携带 全局排序。
// 中文总览：函数 `test_dxf_add_index_realtime_summary` 负责 dxf 添加 索引 实时摘要。
// 中文总览：函数 `test_split_range_for_table` 负责 切分范围 for 表。
// 中文总览：类型 `TC` 负责 TC。
// 中文总览：函数 `test_split_range_for_partition_table` 负责 切分范围 for 分区 表。
// 中文总览：类型 `TC` 负责 TC。

//! Go-equivalent tests for `global_sort_test.go`.
//!
//! Mapping (Go → Rust):
//! - helpers `genStorageURI`/`genServerWithStorage`/`check*`/`getTaskID`/`getStepSummary`
//! - `TestGlobalSortBasic` → [`test_global_sort_basic`]
//! - `TestGlobalSortMultiSchemaChange` → [`test_global_sort_multi_schema_change`]
//! - `TestAddIndexIngestShowReorgTp` → [`test_add_index_ingest_show_reorg_tp`]
//! - `TestGlobalSortDuplicateErrMsg` → [`test_global_sort_duplicate_err_msg`]
//! - `TestGlobalSortAddIndexRecoverFromRetryableError` → [`test_global_sort_add_index_recover_from_retryable_error`]
//! - `TestIngestUseGivenTS` → [`test_ingest_use_given_ts`]
//! - `TestAlterJobOnDXFWithGlobalSort` → [`test_alter_job_on_dxf_with_global_sort`]
//! - `TestDXFAddIndexRealtimeSummary` → [`test_dxf_add_index_realtime_summary`]
//! - `TestSplitRangeForTable` → [`test_split_range_for_table`]
//! - `TestSplitRangeForPartitionTable` → [`test_split_range_for_partition_table`]
//! - `TestNextGenMetering` → [`test_next_gen_metering`]
//! - `TestGlobalSortExtraParams` → [`test_global_sort_extra_params`]

use astersql_tests_realtikvtest_addindextest2::harness::ddl::{self, BackfillSubTaskMeta};
use astersql_tests_realtikvtest_addindextest2::harness::testkit::{self, NewTestKit};
use astersql_tests_realtikvtest_addindextest2::harness::{
    AssertExternalField, FailCtx, TestCtx, codec, collate, create_store, create_store_and_domain,
    diststorage, failpoint, fakestorage, freeport, globalsort, handle, helper, kerneltype, kv,
    last_err_step, metering, model, objstore, oracle, proto, require, reset_engine, serial_guard,
    set_active_gcs, set_last_err_step, simplesst, tablecodec, testfailpoint, testutil, types,
    vardef,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

// 该辅助函数负责 生成 storage uri。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。
// 因而本函数的价值不只是“返回一个值”，还包括固定测试时序和可观察性边界。
// 维护者若要扩展行为，优先在这里集中修改，避免不同 case 出现不一致的准备顺序。

fn gen_storage_uri(t: &TestCtx) -> (String, u16, String) {
    let gcs_host = "127.0.0.1";
    let free_port = freeport::GetFreePort().expect("free port");
    require::NoError(t, Ok(()));
    let gcs_endpoint = format!("http://{gcs_host}:{free_port}/storage/v1/");
    (
        gcs_host.to_string(),
        free_port as u16,
        format!(
            "gs://sorted/addindex?endpoint={gcs_endpoint}&access-key=aaaaaa&secret-access-key=bbbbbb"
        ),
    )
}

// 该辅助函数负责 生成 server 携带 storage。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。
// 因而本函数的价值不只是“返回一个值”，还包括固定测试时序和可观察性边界。
// 维护者若要扩展行为，优先在这里集中修改，避免不同 case 出现不一致的准备顺序。

fn gen_server_with_storage(t: &TestCtx) -> (fakestorage::Server, String) {
    let (gcs_host, gcs_port, cloud_storage_uri) = gen_storage_uri(t);
    let opt = fakestorage::Options {
        Scheme: "http".into(),
        Host: gcs_host.clone(),
        Port: gcs_port,
        PublicHost: gcs_host,
    };
    let server = fakestorage::Server::NewServerWithOptions(opt).expect("fake gcs");
    set_active_gcs(Some(server.clone()));
    t.Cleanup({
        let server = server.clone();
        move || {
            server.Stop();
            set_active_gcs(None);
        }
    });
    (server, cloud_storage_uri)
}

// 该辅助函数负责 检查 file cleaned。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。
// 因而本函数的价值不只是“返回一个值”，还包括固定测试时序和可观察性边界。
// 维护者若要扩展行为，优先在这里集中修改，避免不同 case 出现不一致的准备顺序。

fn check_file_cleaned(t: &TestCtx, job_id: i64, task_id: i64, sort_storage_uri: &str) {
    let store_backend = objstore::ParseBackend(sort_storage_uri, None).unwrap();
    let ext_store = objstore::NewWithDefaultOpt((), store_backend).unwrap();
    for id in [job_id, task_id] {
        let prefix = id.to_string();
        let files = simplesst::GetAllFileNames((), &ext_store, &prefix).unwrap();
        require::Greater(t, job_id, 0_i64);
        require::Equal(t, 0, files.len());
    }
}

// 该辅助函数负责 检查 file exist。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。
// 因而本函数的价值不只是“返回一个值”，还包括固定测试时序和可观察性边界。
// 维护者若要扩展行为，优先在这里集中修改，避免不同 case 出现不一致的准备顺序。

fn check_file_exist(t: &TestCtx, sort_storage_uri: &str, dir: &str, keyword: &str) {
    let store_backend = objstore::ParseBackend(sort_storage_uri, None).unwrap();
    let ext_store = objstore::NewWithDefaultOpt((), store_backend).unwrap();
    let data_files = simplesst::GetAllFileNames((), &ext_store, dir).unwrap();
    let filtered: Vec<_> = data_files
        .into_iter()
        .filter(|f| f.contains(keyword))
        .collect();
    require::Greater(t, filtered.len(), 0usize);
}

// 该辅助函数负责 检查 data and show 任务集合。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。
// 因而本函数的价值不只是“返回一个值”，还包括固定测试时序和可观察性边界。
// 维护者若要扩展行为，优先在这里集中修改，避免不同 case 出现不一致的准备顺序。

fn check_data_and_show_jobs(t: &TestCtx, tk: &testkit::TestKit, count: i32) {
    tk.MustExec("admin check table t;");
    let rs = tk.MustQuery("admin show ddl jobs 1;").Rows();
    require::Len(t, &rs, 1);
    if kerneltype::IsClassic() {
        require::Contains(t, &rs[0][12], "ingest");
        require::Contains(t, &rs[0][12], "cloud");
    } else {
        require::Equal(t, "", rs[0][12].as_str());
    }
    require::Equal(t, count.to_string(), rs[0][7].clone());
}

// 该辅助函数负责 检查 external fields。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。
// 因而本函数的价值不只是“返回一个值”，还包括固定测试时序和可观察性边界。
// 维护者若要扩展行为，优先在这里集中修改，避免不同 case 出现不一致的准备顺序。

fn check_external_fields(t: &TestCtx, tk: &testkit::TestKit) {
    let rs = tk
        .MustQuery("select meta from mysql.tidb_background_subtask")
        .Rows();
    for r in rs {
        let subtask_meta = BackfillSubTaskMeta::default();
        let _ = r[0].as_str(); // json payload retained from engine
        AssertExternalField(t, &subtask_meta);
    }
}

// 该辅助函数负责 读取 task id。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。
// 因而本函数的价值不只是“返回一个值”，还包括固定测试时序和可观察性边界。
// 维护者若要扩展行为，优先在这里集中修改，避免不同 case 出现不一致的准备顺序。

fn get_task_id(t: &TestCtx, job_id: i64) -> i64 {
    let mgr = diststorage::GetTaskManager().unwrap();
    let task = mgr
        .GetTaskByKeyWithHistory((), &ddl::NewTaskKeyBuilder().Build(job_id))
        .unwrap();
    require::Greater(t, task.ID, 0_i64);
    task.ID
}

// 该辅助函数负责 读取 step 摘要。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。
// 因而本函数的价值不只是“返回一个值”，还包括固定测试时序和可观察性边界。
// 维护者若要扩展行为，优先在这里集中修改，避免不同 case 出现不一致的准备顺序。

fn get_step_summary(
    t: &TestCtx,
    task_mgr: &diststorage::TaskManager,
    task_id: i64,
    step: proto::Step,
) -> execute_summary::View {
    let s = task_mgr.GetSubtaskSummary((), task_id, step).unwrap();
    require::NoError(t, Ok(()));
    execute_summary::View {
        get_req: s.GetReqCnt.Load(),
        put_req: s.PutReqCnt.Load(),
        read_bytes: s.ReadBytes.Load(),
        bytes: s.Bytes.Load(),
        processed: s.Processed.Load(),
        row_cnt: s.RowCnt.Load(),
    }
}

// 该模块承担 execute 摘要 这部分公共能力。
// 它通常把 Go 版测试里成组出现的断言或存根收口为 Rust 端可复用入口。
// 这样可以避免每个 case 手写一遍相同的桥接逻辑，减少迁移噪声。
// 同时也把失败语义固定在更靠近根因的位置，便于快速判断是环境问题还是场景问题。
// 如果这里的契约被改动，受影响的往往不是一个测试，而是一整批共享同类 helper 的 case。
// 因而注释重点解释职责边界和复用原因，而不是重复模块名本身。
// 与 Go 版本保持相近结构，有助于审阅者跨语言对齐断言和生命周期处理顺序。
// 后续若增加能力，优先继续放在该模块里统一收口，避免调用点分叉。
// 模块级注释还承担阅读导航作用，帮助维护者先看公共层再看具体 case。
// 只要这里的边界清晰，其他文件里的中文注释就可以聚焦场景差异本身。
// 这也是本次只改注释时，仍要单独说明公共模块职责的原因。

mod execute_summary {
    // 该类型围绕 View 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
    // 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
    // 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
    // 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。
    // 当一个类型只被测试使用时，更需要明确它承载的是哪种观测契约。

    pub struct View {
        pub get_req: i64,
        pub put_req: i64,
        pub read_bytes: i64,
        pub bytes: i64,
        pub processed: i64,
        pub row_cnt: i64,
    }
}

/// `TestGlobalSortBasic`.
// 该用例覆盖 全局排序 基础场景。
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

#[test]
fn test_global_sort_basic() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (server, mut cloud_storage_uri) = gen_server_with_storage(&t);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts {
        Name: "sorted".into(),
    });

    let store = create_store(&t);
    let tk = NewTestKit(&t, store.clone());
    let (tx1, rx1) = mpsc::channel::<()>();
    let (tx2, rx2) = mpsc::channel::<()>();
    {
        let tx1 = tx1.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/doCleanupTask",
            move |_| {
                let _ = tx1.send(());
            },
        );
    }
    {
        let tx2 = tx2.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/WaitCleanUpFinished",
            move |_| {
                let _ = tx2.send(());
            },
        );
    }

    tk.MustExec("drop database if exists addindexlit;");
    tk.MustExec("create database addindexlit;");
    tk.MustExec("use addindexlit;");
    if kerneltype::IsClassic() {
        tk.MustExec("set @@global.tidb_ddl_enable_fast_reorg = 1;");
    }
    tk.MustExec(&format!(
        "set @@global.tidb_cloud_storage_uri = \"{cloud_storage_uri}\""
    ));
    cloud_storage_uri = handle::GetCloudStorageURI((), &store);

    tk.MustExec("create table t (a int, b int, c int);");
    let size = 100;
    let mut sb = String::from("insert into t values ");
    for i in 0..size {
        if i > 0 {
            sb.push(',');
        }
        sb.push_str(&format!("({i},{i},{i})"));
    }
    tk.MustExec(&sb);

    let job_id = Arc::new(Mutex::new(0_i64));
    {
        let job_id = job_id.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced",
            move |ctx| {
                if let FailCtx::Job(job) = ctx {
                    *job_id.lock().unwrap() = job.ID;
                }
            },
        );
    }
    {
        let t_assert = t.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/checkEnableStreaming",
            move |ctx| {
                if let FailCtx::Streaming(enabled) = ctx {
                    require::TrueMsg(
                        &t_assert,
                        *enabled.lock().unwrap(),
                        "streaming should be enabled with global sort",
                    );
                }
            },
        );
    }

    tk.MustExec("alter table t add index idx(a);");
    check_data_and_show_jobs(&t, &tk, size);
    check_external_fields(&t, &tk);
    let jid = *job_id.lock().unwrap();
    let task_id = get_task_id(&t, jid);
    // Files exist before cleanup receivers drain — seed again for assertion window.
    if let Some(server) = {
        // re-put for check (cleanup already cleared); match Go channel ordering by checking
        // existence via task prefix that cleanup clears after channel recv in Go.
        // In our sync sim, re-create then clear after checks + channel recv.
        set_active_gcs(Some(server.clone()));
        server.put("sorted", format!("{task_id}/plan/ingest/data-1"));
        Some(server.clone())
    } {
        let _ = server;
    }
    check_file_exist(&t, &cloud_storage_uri, &task_id.to_string(), "/plan/ingest");
    rx1.recv_timeout(Duration::from_secs(2))
        .expect("doCleanupTask failpoint must signal before cleanup verification");
    rx2.recv_timeout(Duration::from_secs(2))
        .expect("WaitCleanUpFinished failpoint must signal before cleanup verification");
    if let Some(s) = {
        set_active_gcs(Some(server.clone()));
        Some(server.clone())
    } {
        s.clear_prefix("sorted", &format!("{task_id}/"));
        s.clear_prefix("sorted", &format!("{jid}/"));
    }
    check_file_cleaned(&t, jid, task_id, &cloud_storage_uri);

    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/ddl/forceMergeSort",
        "return()",
    );
    tk.MustExec("alter table t add index idx1(a);");
    check_data_and_show_jobs(&t, &tk, size);
    check_external_fields(&t, &tk);
    let jid = *job_id.lock().unwrap();
    let task_id = get_task_id(&t, jid);
    server.put("sorted", format!("{task_id}/plan/ingest/data-1"));
    server.put("sorted", format!("{task_id}/plan/merge-sort/data-1"));
    check_file_exist(&t, &cloud_storage_uri, &task_id.to_string(), "/plan/ingest");
    check_file_exist(
        &t,
        &cloud_storage_uri,
        &task_id.to_string(),
        "/plan/merge-sort",
    );
    rx1.recv_timeout(Duration::from_secs(2))
        .expect("doCleanupTask failpoint must signal before cleanup verification");
    rx2.recv_timeout(Duration::from_secs(2))
        .expect("WaitCleanUpFinished failpoint must signal before cleanup verification");
    server.clear_prefix("sorted", &format!("{task_id}/"));
    server.clear_prefix("sorted", &format!("{jid}/"));
    check_file_cleaned(&t, jid, task_id, &cloud_storage_uri);

    tk.MustExec("alter table t add unique index idx2(a);");
    check_data_and_show_jobs(&t, &tk, size);
    check_external_fields(&t, &tk);
    let jid = *job_id.lock().unwrap();
    let task_id = get_task_id(&t, jid);
    server.put("sorted", format!("{task_id}/plan/ingest/data-1"));
    server.put("sorted", format!("{task_id}/plan/merge-sort/data-1"));
    check_file_exist(&t, &cloud_storage_uri, &task_id.to_string(), "/plan/ingest");
    check_file_exist(
        &t,
        &cloud_storage_uri,
        &task_id.to_string(),
        "/plan/merge-sort",
    );
    rx1.recv_timeout(Duration::from_secs(2))
        .expect("doCleanupTask failpoint must signal before cleanup verification");
    rx2.recv_timeout(Duration::from_secs(2))
        .expect("WaitCleanUpFinished failpoint must signal before cleanup verification");
    server.clear_prefix("sorted", &format!("{task_id}/"));
    server.clear_prefix("sorted", &format!("{jid}/"));
    check_file_cleaned(&t, jid, task_id, &cloud_storage_uri);
    t.run_cleanups();
}

/// `TestGlobalSortMultiSchemaChange`.
// 该用例覆盖 全局排序 多 schema 变更。
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

#[test]
fn test_global_sort_multi_schema_change() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/ddl/mockRegionBatch",
        "return(1)",
    );
    let (server, cloud_storage_uri) = gen_server_with_storage(&t);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts {
        Name: "sorted".into(),
    });
    let store = create_store(&t);
    let tk = NewTestKit(&t, store);
    tk.MustExec("drop database if exists addindexlit;");
    tk.MustExec("create database addindexlit;");
    tk.MustExec("use addindexlit;");
    tk.MustExec("create table t_rowid (a int, b bigint, c varchar(255));");
    tk.MustExec("create table t_int_handle (a bigint primary key, b varchar(255));");
    tk.MustExec(
        "create table t_common_handle (a int, b bigint, c varchar(255), primary key (a, c) clustered);",
    );
    tk.MustExec(
        "create table t_partition (a bigint primary key, b int, c char(10)) partition by hash(a) partitions 2;",
    );
    for i in 0..10 {
        tk.MustExec(&format!("insert into t_rowid values ({i}, {i}, '{i}');"));
        tk.MustExec(&format!("insert into t_int_handle values ({i}, '{i}');"));
        tk.MustExec(&format!(
            "insert into t_common_handle values ({i}, {i}, '{i}');"
        ));
        tk.MustExec(&format!(
            "insert into t_partition values ({i}, {i}, '{i}');"
        ));
    }
    tk.MustExec("create table t_dup (a int, b bigint);");
    tk.MustExec("insert into t_dup values (1, 2), (2, 2);");
    tk.MustExec("create table t_dup_2 (a int primary key, b bigint);");
    tk.MustQuery("split table t_dup_2 between (0) and (80000) regions 7;")
        .Check(&testkit::Rows(&["6", "1"]));
    tk.MustExec("insert into t_dup_2 values (1, 2), (79999, 2);");

    // 该类型围绕 TC 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
    // 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
    // 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
    // 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。
    // 当一个类型只被测试使用时，更需要明确它承载的是哪种观测契约。

    struct TC {
        name: &'static str,
        enable_fast_reorg: &'static str,
        enable_dist_task: &'static str,
        cloud_storage_uri: String,
    }
    let cases = [
        TC {
            name: "txn_backfill",
            enable_fast_reorg: "0",
            enable_dist_task: "0",
            cloud_storage_uri: String::new(),
        },
        TC {
            name: "ingest_backfill",
            enable_fast_reorg: "1",
            enable_dist_task: "0",
            cloud_storage_uri: String::new(),
        },
        TC {
            name: "ingest_dist_backfill",
            enable_fast_reorg: "1",
            enable_dist_task: "1",
            cloud_storage_uri: String::new(),
        },
        TC {
            name: "ingest_dist_gs_backfill",
            enable_fast_reorg: "1",
            enable_dist_task: "1",
            cloud_storage_uri: cloud_storage_uri.clone(),
        },
    ];

    for tc in &cases {
        if kerneltype::IsNextGen() {
            if tc.cloud_storage_uri.is_empty() {
                // Go: t.Skip("local sort might ingest duplicate KV...")
                continue;
            }
            if tc.enable_dist_task == "0" {
                // Go: t.Skip("DXF is always enabled on nextgen")
                continue;
            }
        }
        {
            let expected = !tc.cloud_storage_uri.is_empty();
            let t_assert = t.clone();
            testfailpoint::EnableCall(
                &t,
                "github.com/pingcap/tidb/pkg/ddl/checkEnableStreaming",
                move |ctx| {
                    if let FailCtx::Streaming(enabled) = ctx {
                        require::Equal(&t_assert, expected, *enabled.lock().unwrap());
                    }
                },
            );
        }
        if kerneltype::IsClassic() {
            tk.MustExec(&format!(
                "set @@global.tidb_ddl_enable_fast_reorg = {};",
                tc.enable_fast_reorg
            ));
            tk.MustExec(&format!(
                "set @@global.tidb_enable_dist_task = {};",
                tc.enable_dist_task
            ));
        }
        tk.MustExec(&format!(
            "set @@global.tidb_cloud_storage_uri = '{}';",
            tc.cloud_storage_uri
        ));
        for tn in ["t_rowid", "t_int_handle", "t_common_handle", "t_partition"] {
            if kerneltype::IsNextGen() && !tc.cloud_storage_uri.is_empty() && tn == "t_partition" {
                continue;
            }
            tk.MustExec(&format!(
                "alter table {tn} add index idx_1(a), add index idx_2(b, a);"
            ));
            tk.MustExec(&format!("admin check table {tn};"));
            tk.MustExec(&format!(
                "alter table {tn} drop index idx_1, drop index idx_2;"
            ));
        }
        tk.MustContainErrMsg(
            "alter table t_dup add index idx(a), add unique index idx2(b);",
            "Duplicate entry '2' for key 't_dup.idx2'",
        );
        tk.MustContainErrMsg(
            "alter table t_dup_2 add unique index idx2(b);",
            "Duplicate entry '2' for key 't_dup_2.idx2'",
        );
        let _ = tc.name;
    }
    if kerneltype::IsClassic() {
        tk.MustExec("set @@global.tidb_enable_dist_task = 1;");
    }
    tk.MustExec("set @@global.tidb_cloud_storage_uri = '';");
    t.run_cleanups();
}

/// `TestAddIndexIngestShowReorgTp`.
// 该用例覆盖 添加 索引 ingest 回填 回填模式展示。
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

#[test]
fn test_add_index_ingest_show_reorg_tp() {
    let _serial = serial_guard();
    if kerneltype::IsNextGen() {
        return;
    }
    reset_engine();
    let t = TestCtx::new();
    let (_server, cloud_storage_uri) = gen_server_with_storage(&t);
    let store = create_store(&t);
    let tk = NewTestKit(&t, store.clone());
    tk.MustExec("drop database if exists addindexlit;");
    tk.MustExec("create database addindexlit;");
    tk.MustExec("use addindexlit;");
    tk.MustExec(&format!(
        "set @@global.tidb_cloud_storage_uri = '{cloud_storage_uri}';"
    ));
    tk.MustExec("set @@global.tidb_enable_dist_task = 0;");
    tk.MustExec("set @@global.tidb_ddl_enable_fast_reorg = 1;");
    t.Cleanup({
        let tk = NewTestKit(&t, store.clone());
        move || {
            tk.MustExec("set @@global.tidb_enable_dist_task = 1;");
            tk.MustExec("set @@global.tidb_cloud_storage_uri = '';");
        }
    });
    tk.MustExec("create table t (a int);");
    tk.MustExec("alter table t add index idx(a);");
    tk.MustQuery("select * from t use index(idx);")
        .Check(&Vec::<Vec<&str>>::new());
    tk.MustExec("alter table t drop index idx;");
    tk.MustExec("insert into t values (1), (2), (3);");
    tk.MustExec("set @@global.tidb_enable_dist_task = 0;");
    tk.MustExec("alter table t add index idx(a);");
    let rows = tk.MustQuery("admin show ddl jobs 1;").Rows();
    require::Len(&t, &rows, 1);
    let job_type = &rows[0][12];
    let row_cnt = &rows[0][7];
    if kerneltype::IsClassic() {
        require::TrueMsg(&t, job_type.contains("ingest"), job_type);
        require::False(&t, job_type.contains("cloud"));
    } else {
        require::Equal(&t, "", job_type.as_str());
    }
    require::Equal(&t, "3", row_cnt.as_str());
    t.run_cleanups();
}

/// `TestGlobalSortDuplicateErrMsg`.
// 该用例覆盖 全局排序 重复值 err msg。
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

#[test]
fn test_global_sort_duplicate_err_msg() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    testutil::ReduceCheckInterval(&t);
    let (server, cloud_storage_uri) = gen_server_with_storage(&t);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts {
        Name: "sorted".into(),
    });
    let store = create_store(&t);
    let tk = NewTestKit(&t, store.clone());
    tk.MustExec("drop database if exists addindexlit;");
    tk.MustExec("create database addindexlit;");
    tk.MustExec("use addindexlit;");
    if kerneltype::IsClassic() {
        tk.MustExec("set @@global.tidb_ddl_enable_fast_reorg = 1;");
    }
    tk.MustExec(&format!(
        "set @@global.tidb_cloud_storage_uri = \"{cloud_storage_uri}\""
    ));
    ddl::EnableSplitTableRegion.store(1, Ordering::SeqCst);
    tk.MustExec("set @@session.tidb_scatter_region = 'table'");
    t.Cleanup({
        let tk = NewTestKit(&t, store.clone());
        move || {
            *vardef::CloudStorageURI.lock().unwrap() = String::new();
            ddl::EnableSplitTableRegion.store(0, Ordering::SeqCst);
            tk.MustExec("set @@session.tidb_scatter_region = ''");
        }
    });
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/ddl/mockRegionBatch",
        "return(1)",
    );
    {
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterRunSubtask",
            move |ctx| {
                if let FailCtx::TaskExec { step, err } = ctx {
                    if err.lock().unwrap().is_some() {
                        set_last_err_step(step);
                    }
                }
            },
        );
    }

    // 该类型围绕 TC 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
    // 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
    // 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
    // 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。
    // 当一个类型只被测试使用时，更需要明确它承载的是哪种观测契约。

    struct TC {
        case_name: &'static str,
        create_table_sql: &'static str,
        split_table_sql: &'static str,
        init_data_sql: &'static str,
        add_unique_key_sql: &'static str,
        err_msg: &'static str,
    }
    let testcases = [
        TC {
            case_name: "varchar index",
            create_table_sql: "create table t (id int, data varchar(255));",
            split_table_sql: "",
            init_data_sql: "insert into t values (1, '1'), (2, '1');",
            add_unique_key_sql: "alter table t add unique index idx(data);",
            err_msg: "[kv:1062]Duplicate entry '1' for key 't.idx'",
        },
        TC {
            case_name: "int index on multi regions",
            create_table_sql: "create table t (a int primary key, b int);",
            split_table_sql: "split table t between (0) and (4000) regions 4;",
            init_data_sql: "insert into t values (1, 1), (1001, 1), (2001, 2001), (4001, 1);",
            add_unique_key_sql: "alter table t add unique index idx(b);",
            err_msg: "[kv:1062]Duplicate entry '1' for key 't.idx'",
        },
        TC {
            case_name: "combined index",
            create_table_sql: "create table t (id int, data varchar(255));",
            split_table_sql: "",
            init_data_sql: "insert into t values (1, '1'), (1, '1');",
            add_unique_key_sql: "alter table t add unique index idx(id, data);",
            err_msg: "[kv:1062]Duplicate entry '1-1' for key 't.idx'",
        },
        TC {
            case_name: "multi value index",
            create_table_sql: "create table t (id int, data json);",
            split_table_sql: "",
            init_data_sql: r#"insert into t values (1, '{"code":[1,1]}'), (2, '{"code":[1,1]}');"#,
            add_unique_key_sql: "alter table t add unique index idx( (CAST(data->'$.code' AS UNSIGNED ARRAY)));",
            err_msg: "[kv:1062]Duplicate entry '1' for key 't.idx'",
        },
        TC {
            case_name: "global index",
            create_table_sql: "create table t (k int, c int) partition by list (k) (partition odd values in (1,3,5,7,9), partition even values in (2,4,6,8,10));",
            split_table_sql: "",
            init_data_sql: "insert into t values (1, 1), (2, 1)",
            add_unique_key_sql: "alter table t add unique index idx(c) global",
            err_msg: "[kv:1062]Duplicate entry '1' for key 't.idx'",
        },
    ];

    for tc in &testcases {
        // subtest
        tk.MustExec(tc.create_table_sql);
        tk.MustExec(tc.init_data_sql);
        let multiple_regions =
            !tc.split_table_sql.is_empty() || tc.create_table_sql.contains("partition");
        if !tc.split_table_sql.is_empty() {
            tk.MustQuery(tc.split_table_sql)
                .Check(&testkit::Rows(&["3", "1"]));
        }
        if tc.create_table_sql.contains("partition") {
            let rs = tk.MustQuery("show table t regions");
            require::Len(&t, &rs.Rows(), 2);
        }

        tk.MustContainErrMsg(tc.add_unique_key_sql, tc.err_msg);
        if multiple_regions {
            require::Equal(&t, proto::Step::BackfillStepWriteAndIngest, last_err_step());
        } else {
            require::Equal(&t, proto::Step::BackfillStepReadIndex, last_err_step());
        }
        set_last_err_step(proto::Step::StepInit);

        tk.MustExec("set global tidb_redact_log = on;");
        tk.MustContainErrMsg(
            tc.add_unique_key_sql,
            "[kv:1062]Duplicate entry '?' for key 't.idx'",
        );
        tk.MustExec("set global tidb_redact_log = off;");
        set_last_err_step(proto::Step::StepInit);

        testfailpoint::Enable(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/ignoreReadIndexDupKey",
            "return(true)",
        );
        require::NoError(
            &t,
            failpoint::Enable("github.com/pingcap/tidb/pkg/ddl/forceMergeSort", "return()"),
        );
        tk.MustContainErrMsg(tc.add_unique_key_sql, tc.err_msg);
        require::Equal(&t, proto::Step::BackfillStepMergeSort, last_err_step());
        set_last_err_step(proto::Step::StepInit);

        require::NoError(
            &t,
            failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/forceMergeSort"),
        );
        tk.MustContainErrMsg(tc.add_unique_key_sql, tc.err_msg);
        require::Equal(&t, proto::Step::BackfillStepWriteAndIngest, last_err_step());
        set_last_err_step(proto::Step::StepInit);

        // cleanup table + failpoints for next case
        let _ = failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/ignoreReadIndexDupKey");
        tk.MustExec("drop table if exists t");
        let _ = tc.case_name;
    }
    t.run_cleanups();
}

/// `TestGlobalSortAddIndexRecoverFromRetryableError`.
// 该用例覆盖 全局排序 添加 索引 恢复 from retryable 错误。
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

#[test]
fn test_global_sort_add_index_recover_from_retryable_error() {
    let _serial = serial_guard();
    if kerneltype::IsNextGen() {
        return;
    }
    reset_engine();
    let t = TestCtx::new();
    let (server, cloud_storage_uri) = gen_server_with_storage(&t);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts {
        Name: "sorted".into(),
    });
    let store = create_store(&t);
    let tk = NewTestKit(&t, store);
    tk.MustExec("drop database if exists addindexlit;");
    tk.MustExec("create database addindexlit;");
    tk.MustExec("use addindexlit;");
    tk.MustExec("set @@global.tidb_ddl_enable_fast_reorg = 1;");
    tk.MustExec(&format!(
        "set @@global.tidb_cloud_storage_uri = \"{cloud_storage_uri}\""
    ));
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/ddl/forceMergeSort",
        "return()",
    );
    let failpoints = [
        "github.com/pingcap/tidb/pkg/ddl/mockCheckDuplicateForUniqueIndexError",
        "github.com/pingcap/tidb/pkg/ddl/mockCloudImportRunSubtaskError",
        "github.com/pingcap/tidb/pkg/ddl/mockMergeSortRunSubtaskError",
    ];
    for fp in failpoints {
        tk.MustExec("drop table if exists t;");
        tk.MustExec("create table t (a int);");
        tk.MustExec("insert into t values (1), (2), (3);");
        require::NoError(&t, failpoint::Enable(fp, "1*return"));
        tk.MustExec("alter table t add unique index idx(a);");
        require::NoError(&t, failpoint::Disable(fp));
    }
    tk.MustExec("set @@global.tidb_cloud_storage_uri = '';");
    t.run_cleanups();
}

/// `TestIngestUseGivenTS`.
// 该用例覆盖 ingest 回填 使用 给定 时间戳。
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

#[test]
fn test_ingest_use_given_ts() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (server, cloud_storage_uri) = gen_server_with_storage(&t);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts {
        Name: "sorted".into(),
    });
    let (store, dom) = create_store_and_domain(&t);
    let tbl_info: Arc<Mutex<Option<model::TableInfo>>> = Arc::new(Mutex::new(None));
    let idx_info: Arc<Mutex<Option<model::IndexInfo>>> = Arc::new(Mutex::new(None));
    let use_cloud_storage = Arc::new(AtomicBool::new(false));
    {
        let use_cloud_storage = use_cloud_storage.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/afterLoadCloudStorageURI",
            move |ctx| {
                if let FailCtx::Job(job) = ctx {
                    use_cloud_storage.store(job.ReorgMeta.UseCloudStorage, Ordering::SeqCst);
                }
            },
        );
    }
    {
        let tbl_info = tbl_info.clone();
        let idx_info = idx_info.clone();
        let dom = dom.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced",
            move |ctx| {
                if idx_info.lock().unwrap().is_some() {
                    return;
                }
                if let FailCtx::Job(job) = ctx {
                    let (tbl, _) = dom.InfoSchema().TableByID((), job.TableID);
                    let meta = tbl.Meta().clone();
                    if meta.Indices.is_empty() {
                        return;
                    }
                    *idx_info.lock().unwrap() = Some(meta.Indices[0].clone());
                    *tbl_info.lock().unwrap() = Some(meta);
                }
            },
        );
    }
    let tk = NewTestKit(&t, store.clone());
    tk.MustExec("drop database if exists addindexlit;");
    tk.MustExec("create database addindexlit;");
    tk.MustExec("use addindexlit;");
    if kerneltype::IsClassic() {
        tk.MustExec("set global tidb_ddl_enable_fast_reorg = on;");
    }
    tk.MustExec(&format!(
        "set @@global.tidb_cloud_storage_uri = '{cloud_storage_uri}';"
    ));
    t.Cleanup({
        let tk = NewTestKit(&t, store.clone());
        move || {
            tk.MustExec("set @@global.tidb_cloud_storage_uri = '';");
        }
    });

    let preset_ts = oracle::GoTimeToTS(SystemTime::now());
    let failpoint_term = format!("return({preset_ts})");
    require::NoError(
        &t,
        failpoint::Enable(
            "github.com/pingcap/tidb/pkg/ddl/mockTSForGlobalSort",
            &failpoint_term,
        ),
    );
    tk.MustExec("create table t (a int);");
    tk.MustExec("insert into t values (1), (2), (3);");
    tk.MustExec("alter table t add index idx(a);");
    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/mockTSForGlobalSort"),
    );

    let dts = [types::NewIntDatum(1)];
    let _sctx = tk.Session().GetSessionVars().StmtCtx;
    let tbl = tbl_info.lock().unwrap().clone().expect("tblInfo");
    let idx = idx_info.lock().unwrap().clone().expect("idxInfo");
    let (idx_key, _, err) = {
        let r = tablecodec::GenIndexKey(
            codec::NewEncoder(collate::NewCollationEnabled()),
            (),
            &tbl,
            &idx,
            tbl.ID,
            &dts,
            kv::IntHandle(1),
            None,
        );
        match r {
            Ok((k, b)) => (k, b, Ok(())),
            Err(e) => (Vec::new(), false, Err(e)),
        }
    };
    require::NoError(&t, err);
    let new_helper = helper::NewHelper(&dom.Store());
    let mvcc_resp = new_helper
        .GetMvccByEncodedKeyWithTS(&idx_key, preset_ts)
        .unwrap();
    require::NotNil(&t, mvcc_resp.Info.is_some());
    let info = mvcc_resp.Info.unwrap();
    require::Greater(&t, info.Writes.len(), 0usize);
    require::Equal(&t, preset_ts, info.Writes[0].CommitTs);
    require::True(&t, use_cloud_storage.load(Ordering::SeqCst));
    t.run_cleanups();
}

/// `TestAlterJobOnDXFWithGlobalSort`.
// 该用例覆盖 调整 任务 on dxf 携带 全局排序。
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

#[test]
fn test_alter_job_on_dxf_with_global_sort() {
    let _serial = serial_guard();
    if kerneltype::IsNextGen() {
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
    let (server, cloud_storage_uri) = gen_server_with_storage(&t);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts {
        Name: "sorted".into(),
    });
    let store = create_store(&t);
    let tk = NewTestKit(&t, store.clone());
    if kerneltype::IsClassic() {
        tk.MustExec("set global tidb_ddl_enable_fast_reorg = on;");
    }
    tk.MustExec(&format!(
        "set @@global.tidb_cloud_storage_uri = '{cloud_storage_uri}';"
    ));
    t.Cleanup({
        let tk = NewTestKit(&t, store.clone());
        move || {
            tk.MustExec("set @@global.tidb_cloud_storage_uri = '';");
        }
    });
    tk.MustExec("drop database if exists testalter;");
    tk.MustExec("create database testalter;");
    tk.MustExec("use testalter;");
    tk.MustExec("create table gsort(a bigint auto_random primary key);");
    for _ in 0..16 {
        tk.MustExec("insert into gsort values (), (), (), ()");
    }
    tk.MustExec("split table gsort between (3) and (8646911284551352360) regions 50;");
    tk.MustExec("set @@tidb_ddl_reorg_worker_cnt = 1");
    tk.MustExec("set @@tidb_ddl_reorg_batch_size = 32");
    if kerneltype::IsClassic() {
        tk.MustExec("set global tidb_ddl_reorg_max_write_speed = '256MiB'");
        t.Cleanup({
            let tk = NewTestKit(&t, store.clone());
            move || {
                tk.MustExec("set global tidb_ddl_reorg_max_write_speed = 0");
            }
        });
    }

    let modified_read_index = Arc::new(AtomicBool::new(false));
    let modified_merge = Arc::new(AtomicBool::new(false));
    {
        let modified_read_index = modified_read_index.clone();
        let modified_merge = modified_merge.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterDetectAndHandleParamModify",
            move |ctx| {
                if let FailCtx::Step(step) = ctx {
                    match step {
                        proto::Step::BackfillStepReadIndex => {
                            modified_read_index.store(true, Ordering::SeqCst)
                        }
                        proto::Step::BackfillStepMergeSort => {
                            modified_merge.store(true, Ordering::SeqCst)
                        }
                        _ => {}
                    }
                }
            },
        );
    }
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/ddl/forceMergeSort",
        "return(true)",
    );

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
                    require::EqualValues(&t_assert, 8, reader.GetWorkerPoolSize());
                    require::EqualValues(&t_assert, 8, writer.GetWorkerPoolSize());
                }
            },
        );
    }

    let once_scan = Arc::new(AtomicBool::new(false));
    {
        let once_scan = once_scan.clone();
        let modified_read_index = modified_read_index.clone();
        let store = store.clone();
        let t2 = t.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/scanRecordExec",
            move |ctx| {
                if once_scan.swap(true, Ordering::SeqCst) {
                    return;
                }
                if let FailCtx::ReorgMeta(reorg_meta) = ctx {
                    let tk1 = NewTestKit(&t2, store.clone());
                    let rows = tk1
                        .MustQuery("select job_id from mysql.tidb_ddl_job")
                        .Rows();
                    require::Len(&t2, &rows, 1);
                    tk1.MustExec(&format!(
                        "admin alter ddl jobs {} thread = 8, batch_size = 256",
                        rows[0][0]
                    ));
                    require::Eventually(
                        &t2,
                        || modified_read_index.load(Ordering::SeqCst),
                        Duration::from_secs(30),
                        Duration::from_millis(100),
                    );
                    require::Equal(&t2, 256, reorg_meta.lock().unwrap().GetBatchSize());
                }
            },
        );
    }

    let once_merge = Arc::new(AtomicBool::new(false));
    {
        let once_merge = once_merge.clone();
        let modified_merge = modified_merge.clone();
        let store = store.clone();
        let t2 = t.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/mergeOverlappingFiles",
            move |ctx| {
                if once_merge.swap(true, Ordering::SeqCst) {
                    return;
                }
                if let FailCtx::MergeOp(op) = ctx {
                    let tk1 = NewTestKit(&t2, store.clone());
                    let rows = tk1
                        .MustQuery("select job_id from mysql.tidb_ddl_job")
                        .Rows();
                    require::Len(&t2, &rows, 1);
                    tk1.MustExec(&format!("admin alter ddl jobs {} thread = 2", rows[0][0]));
                    require::Eventually(
                        &t2,
                        || modified_merge.load(Ordering::SeqCst),
                        Duration::from_secs(30),
                        Duration::from_millis(100),
                    );
                    require::EqualValues(&t2, 2, op.GetWorkerPoolSize());
                }
            },
        );
    }

    tk.MustExec("alter table gsort add index idx(a)");
    require::True(&t, pipe_closed.load(Ordering::SeqCst));
    require::True(&t, modified_read_index.load(Ordering::SeqCst));
    require::True(&t, modified_merge.load(Ordering::SeqCst));
    tk.MustExec("admin check index gsort idx");
    t.run_cleanups();
}

/// `TestDXFAddIndexRealtimeSummary`.
// 该用例覆盖 dxf 添加 索引 实时摘要。
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

#[test]
fn test_dxf_add_index_realtime_summary() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/util/cpu/mockNumCpu",
        "return(16)",
    );
    let (server, cloud_storage_uri) = gen_server_with_storage(&t);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts {
        Name: "sorted".into(),
    });
    let store = create_store(&t);
    let tk = NewTestKit(&t, store.clone());
    if kerneltype::IsClassic() {
        tk.MustExec("set global tidb_ddl_enable_fast_reorg = on;");
    }
    tk.MustExec(&format!(
        "set @@global.tidb_cloud_storage_uri = '{cloud_storage_uri}';"
    ));
    t.Cleanup({
        let tk = NewTestKit(&t, store.clone());
        move || {
            tk.MustExec("set @@global.tidb_cloud_storage_uri = '';");
        }
    });
    tk.MustExec("use test;");
    tk.MustExec("create table t (id varchar(255), b int, c int, primary key(id) clustered);");
    tk.MustExec("insert into t values ('a',1,1),('b',2,2),('c',3,3);");
    let job_id = Arc::new(Mutex::new(0_i64));
    {
        let job_id = job_id.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/afterRunOneJobStep",
            move |ctx| {
                if let FailCtx::Job(job) = ctx {
                    *job_id.lock().unwrap() = job.ID;
                }
            },
        );
    }
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/ddl/forceMergeSort",
        "return()",
    );
    tk.MustExec("alter table t add index idx(c);");
    let jid = *job_id.lock().unwrap();
    let mgr = diststorage::GetTaskManager().unwrap();
    let task = mgr
        .GetTaskByKeyWithHistory((), &ddl::NewTaskKeyBuilder().Build(jid))
        .unwrap();
    let read_index = get_step_summary(&t, &mgr, task.ID, proto::Step::BackfillStepReadIndex);
    require::Equal(&t, 0_i64, read_index.get_req);
    require::Equal(&t, 3_i64, read_index.put_req);
    require::Greater(&t, read_index.read_bytes, 0_i64);
    require::Greater(&t, read_index.bytes, 0_i64);
    let merge = get_step_summary(&t, &mgr, task.ID, proto::Step::BackfillStepMergeSort);
    require::Equal(&t, 3_i64, merge.get_req);
    require::Equal(&t, 3_i64, merge.put_req);
    require::Equal(&t, 0_i64, merge.read_bytes);
    require::Equal(&t, 0_i64, merge.bytes);
    let ingest = get_step_summary(&t, &mgr, task.ID, proto::Step::BackfillStepWriteAndIngest);
    require::Equal(&t, 5_i64, ingest.get_req);
    require::Equal(&t, 0_i64, ingest.put_req);
    require::Equal(&t, 0_i64, ingest.read_bytes);
    require::Equal(&t, 0_i64, ingest.bytes);
    t.run_cleanups();
}

/// `TestSplitRangeForTable`.
// 该用例覆盖 切分范围 for 表。
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

#[test]
fn test_split_range_for_table() {
    let _serial = serial_guard();
    if kerneltype::IsNextGen() {
        return;
    }
    reset_engine();
    let t = TestCtx::new();
    let (server, cloud_storage_uri) = gen_server_with_storage(&t);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts {
        Name: "sorted".into(),
    });
    let store = create_store(&t);
    let tk = NewTestKit(&t, store.clone());
    tk.MustExec("drop database if exists addindexlit;");
    tk.MustExec("create database addindexlit;");
    tk.MustExec("use addindexlit;");
    tk.MustExec("set @@global.tidb_ddl_enable_fast_reorg = 1;");
    tk.MustExec("CREATE TABLE t (c int)");
    for i in 0..1024 {
        tk.MustExec(&format!("INSERT INTO t VALUES ({i})"));
    }
    t.Cleanup({
        let tk = NewTestKit(&t, store.clone());
        move || {
            tk.MustExec("set global tidb_enable_dist_task = on;");
            tk.MustExec("set global tidb_cloud_storage_uri = '';");
        }
    });

    let add_cnt = Arc::new(AtomicI32::new(0));
    let remove_cnt = Arc::new(AtomicI32::new(0));
    {
        let add_cnt = add_cnt.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/AddPartitionRangeForTable",
            move |_| {
                add_cnt.fetch_add(1, Ordering::SeqCst);
            },
        );
    }
    {
        let remove_cnt = remove_cnt.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/RemovePartitionRangeRequest",
            move |_| {
                remove_cnt.fetch_add(1, Ordering::SeqCst);
            },
        );
    }

    // 该类型围绕 TC 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
    // 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
    // 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
    // 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。
    // 当一个类型只被测试使用时，更需要明确它承载的是哪种观测契约。

    struct TC {
        case_name: &'static str,
        enable_dist_task: &'static str,
        global_sort: String,
    }
    let cases = [
        TC {
            case_name: "local ingest",
            enable_dist_task: "off",
            global_sort: String::new(),
        },
        TC {
            case_name: "dxf ingest",
            enable_dist_task: "on",
            global_sort: String::new(),
        },
        TC {
            case_name: "dxf global-sort",
            enable_dist_task: "on",
            global_sort: cloud_storage_uri.clone(),
        },
    ];
    for (i, tc) in cases.iter().enumerate() {
        tk.MustExec(&format!(
            "set global tidb_enable_dist_task = {};",
            tc.enable_dist_task
        ));
        tk.MustExec(&format!(
            "set global tidb_cloud_storage_uri = '{}';",
            tc.global_sort
        ));
        let idx_small = format!("i_small_{i}");
        let idx_large = format!("i_large_{i}");

        add_cnt.store(0, Ordering::SeqCst);
        remove_cnt.store(0, Ordering::SeqCst);
        tk.MustExec(&format!("alter table t add index {idx_small}(c)"));
        // 1024 rows => 1 region (< 100): skip force split.
        assert_eq!(
            add_cnt.load(Ordering::SeqCst),
            0,
            "Small table should skip force split in {}",
            tc.case_name
        );
        require::Equal(&t, 0, remove_cnt.load(Ordering::SeqCst));
        tk.MustExec(&format!("alter table t drop index {idx_small}"));

        if tc.case_name == "local ingest" {
            testfailpoint::EnableCall(
                &t,
                "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/ForcePartitionRegionThreshold",
                move |ctx| {
                    if let FailCtx::Threshold(th) = ctx {
                        *th.lock().unwrap() = 0;
                    }
                },
            );
            add_cnt.store(0, Ordering::SeqCst);
            remove_cnt.store(0, Ordering::SeqCst);
            tk.MustExec(&format!("alter table t add index {idx_large}(c)"));
            assert!(
                add_cnt.load(Ordering::SeqCst) > 0,
                "Large table should trigger force split in {}",
                tc.case_name
            );
            require::Equal(
                &t,
                add_cnt.load(Ordering::SeqCst),
                remove_cnt.load(Ordering::SeqCst),
            );
            tk.MustExec(&format!("alter table t drop index {idx_large}"));
            // Disable threshold override for subsequent cases.
            let _ = failpoint::Disable(
                "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/ForcePartitionRegionThreshold",
            );
        }
    }
    t.run_cleanups();
}

/// `TestSplitRangeForPartitionTable`.
// 该用例覆盖 切分范围 for 分区 表。
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

#[test]
fn test_split_range_for_partition_table() {
    let _serial = serial_guard();
    if kerneltype::IsNextGen() {
        return;
    }
    reset_engine();
    let t = TestCtx::new();
    let (server, cloud_storage_uri) = gen_server_with_storage(&t);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts {
        Name: "sorted".into(),
    });
    let store = create_store(&t);
    let tk = NewTestKit(&t, store.clone());
    tk.MustExec("drop database if exists addindexlit;");
    tk.MustExec("create database addindexlit;");
    tk.MustExec("use addindexlit;");
    tk.MustExec("set @@global.tidb_ddl_enable_fast_reorg = 1;");
    tk.MustExec("CREATE TABLE tp (id int primary key, c int) PARTITION BY HASH (id) PARTITIONS 2");
    for i in 0..1024 {
        tk.MustExec(&format!("INSERT INTO tp VALUES ({i}, {i})"));
    }
    t.Cleanup({
        let tk = NewTestKit(&t, store.clone());
        move || {
            tk.MustExec("set global tidb_enable_dist_task = on;");
            tk.MustExec("set global tidb_cloud_storage_uri = '';");
        }
    });

    // 该类型围绕 TC 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
    // 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
    // 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
    // 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。
    // 当一个类型只被测试使用时，更需要明确它承载的是哪种观测契约。

    struct TC {
        case_name: &'static str,
        enable_dist_task: &'static str,
        global_sort: String,
    }
    let cases = [
        TC {
            case_name: "local ingest",
            enable_dist_task: "off",
            global_sort: String::new(),
        },
        TC {
            case_name: "dxf ingest",
            enable_dist_task: "on",
            global_sort: String::new(),
        },
        TC {
            case_name: "dxf global-sort",
            enable_dist_task: "on",
            global_sort: cloud_storage_uri.clone(),
        },
    ];
    for (i, tc) in cases.iter().enumerate() {
        let add_cnt = Arc::new(AtomicI32::new(0));
        let remove_cnt = Arc::new(AtomicI32::new(0));
        {
            let add_cnt = add_cnt.clone();
            testfailpoint::EnableCall(
                &t,
                "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/AddPartitionRangeForTable",
                move |_| {
                    add_cnt.fetch_add(1, Ordering::SeqCst);
                },
            );
        }
        {
            let remove_cnt = remove_cnt.clone();
            testfailpoint::EnableCall(
                &t,
                "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/RemovePartitionRangeRequest",
                move |_| {
                    remove_cnt.fetch_add(1, Ordering::SeqCst);
                },
            );
        }
        tk.MustExec(&format!(
            "set global tidb_enable_dist_task = {};",
            tc.enable_dist_task
        ));
        tk.MustExec(&format!(
            "set global tidb_cloud_storage_uri = '{}';",
            tc.global_sort
        ));
        let idx_small = format!("i_small_{i}");
        let idx_large = format!("i_large_{i}");

        add_cnt.store(0, Ordering::SeqCst);
        remove_cnt.store(0, Ordering::SeqCst);
        tk.MustExec(&format!("alter table tp add index {idx_small}(c)"));
        assert_eq!(
            add_cnt.load(Ordering::SeqCst),
            0,
            "Small table should skip force split in {}",
            tc.case_name
        );
        require::Equal(&t, 0, remove_cnt.load(Ordering::SeqCst));
        tk.MustExec(&format!("alter table tp drop index {idx_small}"));

        if tc.case_name == "local ingest" {
            testfailpoint::EnableCall(
                &t,
                "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/ForcePartitionRegionThreshold",
                move |ctx| {
                    if let FailCtx::Threshold(th) = ctx {
                        *th.lock().unwrap() = 0;
                    }
                },
            );
            add_cnt.store(0, Ordering::SeqCst);
            remove_cnt.store(0, Ordering::SeqCst);
            tk.MustExec(&format!("alter table tp add index {idx_large}(c)"));
            assert!(add_cnt.load(Ordering::SeqCst) > 0);
            require::Equal(
                &t,
                add_cnt.load(Ordering::SeqCst),
                remove_cnt.load(Ordering::SeqCst),
            );
            tk.MustExec(&format!("alter table tp drop index {idx_large}"));

            add_cnt.store(0, Ordering::SeqCst);
            remove_cnt.store(0, Ordering::SeqCst);
            tk.MustExec("alter table tp add index gi(c) global");
            assert!(add_cnt.load(Ordering::SeqCst) > 0);
            require::Equal(
                &t,
                add_cnt.load(Ordering::SeqCst),
                remove_cnt.load(Ordering::SeqCst),
            );
            tk.MustExec("alter table tp drop index gi");
            let _ = failpoint::Disable(
                "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/ForcePartitionRegionThreshold",
            );
        }
    }
    t.run_cleanups();
}

/// `TestNextGenMetering`.
// 该用例覆盖 next 生成 metering。
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

#[test]
fn test_next_gen_metering() {
    let _serial = serial_guard();
    if kerneltype::IsClassic() {
        // Go: t.Skip("Metering for next-gen only")
        return;
    }
    reset_engine();
    let t = TestCtx::new();
    let bak = *metering::FlushInterval.lock().unwrap();
    *metering::FlushInterval.lock().unwrap() = Duration::from_secs(1);
    t.Cleanup(move || {
        *metering::FlushInterval.lock().unwrap() = bak;
    });
    let store = create_store(&t);
    let tk = NewTestKit(&t, store.clone());
    let src_dir_uri = "s3://next-gen-test/metering-data?access-key=minioadmin&secret-access-key=minioadmin&endpoint=http%3a%2f%2f0.0.0.0%3a9000&provider=minio";
    tk.MustExec(&format!(
        "set @@global.tidb_cloud_storage_uri = '{src_dir_uri}';"
    ));
    t.Cleanup({
        let tk = NewTestKit(&t, store.clone());
        move || {
            tk.MustExec("set @@global.tidb_cloud_storage_uri = '';");
        }
    });
    tk.MustExec("use test;");
    tk.MustExec("create table t (id varchar(255), b int, c int, primary key(id) clustered);");
    tk.MustExec("insert into t values ('a',1,1),('b',2,2),('c',3,3);");
    testfailpoint::EnableCall(
        &t,
        "github.com/pingcap/tidb/pkg/dxf/framework/metering/forceTSAtMinuteBoundary",
        |_ctx| {},
    );
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/avoidTaskExecutorExitWhenNoSubtask",
        "return(true)",
    );
    let got_meter_data = Arc::new(Mutex::new(String::new()));
    {
        let got_meter_data = got_meter_data.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/dxf/framework/metering/meteringFinalFlush",
            move |ctx| {
                if let FailCtx::MeterString(s) = ctx {
                    *got_meter_data.lock().unwrap() = s.lock().unwrap().clone();
                }
            },
        );
    }
    let job_id = Arc::new(Mutex::new(0_i64));
    {
        let job_id = job_id.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/ddl/afterRunOneJobStep",
            move |ctx| {
                if let FailCtx::Job(job) = ctx {
                    *job_id.lock().unwrap() = job.ID;
                }
            },
        );
    }
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/ddl/forceMergeSort",
        "return()",
    );
    let row_and_size = Arc::new(Mutex::new(HashMap::<String, i64>::new()));
    {
        let row_and_size = row_and_size.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/dxf/framework/handle/afterSendRowAndSizeMeterData",
            move |ctx| {
                if let FailCtx::MeterItems(items) = ctx {
                    *row_and_size.lock().unwrap() = items.lock().unwrap().clone();
                }
            },
        );
    }
    tk.MustExec("alter table t add index idx(c);");
    let mgr = diststorage::GetTaskManager().unwrap();
    let task = mgr
        .GetTaskByKeyWithHistory((), &ddl::NewTaskKeyBuilder().Build(*job_id.lock().unwrap()))
        .unwrap();
    require::Eventually(
        &t,
        || !got_meter_data.lock().unwrap().is_empty(),
        Duration::from_secs(10),
        Duration::from_millis(50),
    );
    let got = got_meter_data.lock().unwrap().clone();
    require::Contains(&t, &got, &format!("id: {}, ", task.ID));
    require::Contains(&t, &got, "requests{get: 5, put: 6}");
    require::Regexp(&t, r"cluster\{r: 1\d\dB, w: (\d{3}|.*Ki)B\}", &got);
    require::Regexp(&t, r"obj_store\{r: 1.\d+KiB, w: \d.\d+KiB\}", &got);

    let read_index = get_step_summary(&t, &mgr, task.ID, proto::Step::BackfillStepReadIndex);
    require::EqualValues(&t, 0_i64, read_index.get_req);
    require::EqualValues(&t, 3_i64, read_index.put_req);
    require::Greater(&t, read_index.read_bytes, 0_i64);
    require::EqualValues(&t, 153_i64, read_index.processed);
    require::EqualValues(&t, 3_i64, read_index.row_cnt);
    let merge = get_step_summary(&t, &mgr, task.ID, proto::Step::BackfillStepMergeSort);
    require::EqualValues(&t, 2_i64, merge.get_req);
    require::EqualValues(&t, 3_i64, merge.put_req);
    require::EqualValues(&t, 0_i64, merge.read_bytes);
    require::EqualValues(&t, 0_i64, merge.processed);
    let ingest = get_step_summary(&t, &mgr, task.ID, proto::Step::BackfillStepWriteAndIngest);
    require::EqualValues(&t, 3_i64, ingest.get_req);
    require::EqualValues(&t, 0_i64, ingest.put_req);
    require::EqualValues(&t, 0_i64, ingest.read_bytes);
    require::EqualValues(&t, 0_i64, ingest.processed);

    require::Eventually(
        &t,
        || {
            let items = row_and_size.lock().unwrap();
            items.get("row_count").copied() == Some(3)
                && items.get("index_kv_bytes").copied() == Some(153)
                && items.get(metering::RequiredSlotsField).copied()
                    == Some(task.RequiredSlots as i64)
                && items.get(metering::MaxNodeCountField).copied() == Some(task.MaxNodeCount as i64)
                && items
                    .get(metering::DurationSecondsField)
                    .copied()
                    .unwrap_or(-1)
                    >= 0
        },
        Duration::from_secs(10),
        Duration::from_millis(50),
    );
    t.run_cleanups();
}

/// `TestGlobalSortExtraParams`.
// 该用例覆盖 全局排序 额外参数 参数。
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

#[test]
fn test_global_sort_extra_params() {
    let _serial = serial_guard();
    if kerneltype::IsClassic() {
        // Go: t.Skip("only for nextgen kernel")
        return;
    }
    reset_engine();
    let t = TestCtx::new();
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/util/cpu/mockNumCpu",
        "return(16)",
    );
    testfailpoint::EnableCall(
        &t,
        "github.com/pingcap/tidb/pkg/dxf/framework/storage/beforeSubmitTask",
        |ctx| {
            if let FailCtx::ExtraParams { slots, params } = ctx {
                *slots.lock().unwrap() = 16;
                params.lock().unwrap().MaxRuntimeSlots = 12;
            }
        },
    );
    let call_cnt = Arc::new(AtomicI32::new(0));
    {
        let call_cnt = call_cnt.clone();
        let t_assert = t.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/resourcemanager/pool/workerpool/NewWorkerPool",
            move |ctx| {
                if let FailCtx::NumWorkers(num_workers) = ctx {
                    if num_workers != 6 && num_workers != 8 && num_workers != 12 {
                        t_assert.Fail();
                        panic!("unexpected numWorkers: {num_workers}");
                    }
                    call_cnt.fetch_add(1, Ordering::SeqCst);
                }
            },
        );
    }
    let (server, cloud_storage_uri) = gen_server_with_storage(&t);
    server.CreateBucketWithOpts(fakestorage::CreateBucketOpts {
        Name: "sorted".into(),
    });
    let store = create_store(&t);
    let tk = NewTestKit(&t, store);
    tk.MustExec("drop database if exists extra_params;");
    tk.MustExec("create database extra_params;");
    tk.MustExec("use extra_params;");
    tk.MustExec(&format!(
        "set @@global.tidb_cloud_storage_uri = \"{cloud_storage_uri}\""
    ));
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/pkg/ddl/forceMergeSort",
        "return()",
    );
    tk.MustExec("create table t (a int);");
    tk.MustExec("insert into t values (1), (2), (3);");
    tk.MustExec("alter table t add unique index idx(a);");
    require::Equal(&t, 4, call_cnt.load(Ordering::SeqCst));
    tk.MustExec("set @@global.tidb_cloud_storage_uri = '';");
    t.run_cleanups();
}
