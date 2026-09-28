// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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
// 中文总览：类型 `EngineReset` 负责 EngineReset。
// 中文总览：函数 `reset_engine` 负责 复位 engine。
// 中文总览：函数 `drop` 负责 收尾删除。
// 中文总览：函数 `with_fast_add_index_failpoints` 负责 携带 fast 添加 索引 failpoints。

//! PiTR add-index coverage ported from `pitr_test.go`.

use astersql_testkit_testfailpoint::{enable, enable_call, eval_bool, inject};
use astersql_tests_realtikvtest::stubs::{self as realtikv_stubs, TestCtx};
use astersql_tests_realtikvtest_addindextest::serial_guard;
use astersql_tests_realtikvtest_testutils as testutils;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use testutils::stubs::{clear_local_events, take_local_events};

const FORCE_SPLIT_REGION: &str = "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/forceSplitRegion";
const ADJUST_RETRY_BACKOFF: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/adjustRegionJobRetryBackoff";

// 该类型围绕 EngineReset 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

struct EngineReset;

// 该辅助函数负责 复位 engine。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn reset_engine() {
    realtikv_stubs::reset_test_globals();
    testutils::stubs::reset_test_globals();
    package_harness::configure();
}

impl Drop for EngineReset {
    // 该辅助函数负责 收尾删除。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    fn drop(&mut self) {
        reset_engine();
    }
}

// 该辅助函数负责 携带 fast 添加 索引 failpoints。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn with_fast_add_index_failpoints(run: impl FnOnce()) {
    let force_split = enable(FORCE_SPLIT_REGION, "return(true)");
    assert!(
        eval_bool(FORCE_SPLIT_REGION),
        "forceSplitRegion failpoint must be active during the PiTR workload"
    );

    let callback_hits = Arc::new(AtomicUsize::new(0));
    let callback_hits_inner = Arc::clone(&callback_hits);
    let retry_backoff = enable_call(ADJUST_RETRY_BACKOFF, move || {
        callback_hits_inner.fetch_add(1, Ordering::SeqCst);
    });
    inject(ADJUST_RETRY_BACKOFF);
    assert_eq!(callback_hits.load(Ordering::SeqCst), 1);

    run();

    drop(retry_backoff);
    drop(force_split);
    assert!(
        !eval_bool(FORCE_SPLIT_REGION),
        "forceSplitRegion failpoint leaked past the PiTR test"
    );
    inject(ADJUST_RETRY_BACKOFF);
    assert_eq!(
        callback_hits.load(Ordering::SeqCst),
        1,
        "retry-backoff callback leaked past the PiTR test"
    );
}

// 该辅助函数负责 断言 pitr fixture evidence。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn assert_pitr_fixture_evidence(t: &TestCtx) {
    assert!(!t.Failed(), "PiTR fixture marked the test failed");
    let events = take_local_events();
    assert!(
        events
            .iter()
            .any(|event| event.contains("tk.MustExec:create database addindex")),
        "addindex database fixture was not created"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("tk.MustExec:insert into addindex.t")),
        "fixture rows were not inserted"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("tk.Exec:alter table addindex.t")),
        "PiTR add-index SQL was not executed"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("tk.Exec:admin check")),
        "PiTR index/table verification was not executed"
    );
}

// 该辅助函数负责 run pitr case。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn run_pitr_case(workload: impl FnOnce(&testutils::SuiteContext)) {
    let _lock = serial_guard();
    reset_engine();
    let _reset = EngineReset;
    clear_local_events();
    let t = TestCtx::new();
    with_fast_add_index_failpoints(|| {
        let ctx = testutils::InitCompCtx(&t);
        let comp = ctx
            .CompCtx
            .as_ref()
            .expect("InitCompCtx must attach a compatibility context");
        {
            let mut state = comp.write().expect("PiTR context lock poisoned");
            assert!(!state.IsPiTR, "InitCompCtx must start outside PiTR mode");
            state.IsPiTR = true;
        }
        workload(&ctx);
        assert!(
            comp.read().expect("PiTR context lock poisoned").IsPiTR,
            "the workload lost its PiTR boundary"
        );
    });
    assert_pitr_fixture_evidence(&t);
    clear_local_events();
}

// 该用例覆盖 pitr 创建 普通 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

fn test_pitr_create_non_unique_index() {
    let col_ids = vec![vec![1, 4, 7], vec![2, 5, 8], vec![3, 6, 9]];
    run_pitr_case(|ctx| {
        testutils::TestOneColFrame(ctx, &col_ids, testutils::AddIndexNonUnique);
    });
}

// 该用例覆盖 pitr 创建 唯一 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

fn test_pitr_create_unique_index() {
    let col_ids = vec![vec![1, 6], vec![11], vec![19]];
    run_pitr_case(|ctx| {
        testutils::TestOneColFrame(ctx, &col_ids, testutils::AddIndexUnique);
    });
}

// 该用例覆盖 pitr 创建 主键。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

fn test_pitr_create_primary_key() {
    run_pitr_case(|ctx| testutils::TestOneIndexFrame(ctx, 0, testutils::AddIndexPK));
}

// 该用例覆盖 pitr 创建 生成列 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

fn test_pitr_create_gen_col_index() {
    run_pitr_case(|ctx| testutils::TestOneIndexFrame(ctx, 29, testutils::AddIndexGenCol));
}

// 该用例覆盖 pitr 创建 多列 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

fn test_pitr_create_multi_cols_index() {
    let coli_ids = vec![vec![1], vec![8], vec![11]];
    let colj_ids = vec![vec![16], vec![23], vec![27]];
    run_pitr_case(|ctx| {
        testutils::TestTwoColsFrame(ctx, &coli_ids, &colj_ids, testutils::AddIndexMultiCols);
    });
}

#[path = "package_harness.rs"]
mod package_harness;

fn main() -> std::process::ExitCode {
    package_harness::run(&[
        (
            "test_pitr_create_non_unique_index",
            test_pitr_create_non_unique_index,
        ),
        (
            "test_pitr_create_unique_index",
            test_pitr_create_unique_index,
        ),
        ("test_pitr_create_primary_key", test_pitr_create_primary_key),
        (
            "test_pitr_create_gen_col_index",
            test_pitr_create_gen_col_index,
        ),
        (
            "test_pitr_create_multi_cols_index",
            test_pitr_create_multi_cols_index,
        ),
    ])
}
