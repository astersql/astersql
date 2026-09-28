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
// 中文总览：函数 `reset_engine` 负责 复位 engine。
// 中文总览：函数 `enable_fast_add_index_failpoints` 负责 启用 fast 添加 索引 failpoints。
// 中文总览：函数 `init_multi_schema_context` 负责 初始化 multi schema context。
// 中文总览：函数 `assert_multi_schema_sql` 负责 断言 multi schema sql。

use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest::stubs::TestCtx;
use astersql_tests_realtikvtest_addindextest::serial_guard;
use astersql_tests_realtikvtest_testutils::{
    AddIndexGenCol, AddIndexMultiCols, AddIndexNonUnique, AddIndexPK, AddIndexUnique, InitCompCtx,
    SuiteContext, TestOneColFrame, TestOneIndexFrame, TestTwoColsFrame,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const FORCE_SPLIT_REGION: &str = "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/forceSplitRegion";
const ADJUST_RETRY_BACKOFF: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/adjustRegionJobRetryBackoff";
static RETRY_BACKOFF_CALLBACKS: AtomicUsize = AtomicUsize::new(0);

// 该辅助函数负责 复位 engine。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn reset_engine() {
    astersql_tests_realtikvtest::stubs::reset_test_globals();
    astersql_tests_realtikvtest_testutils::stubs::reset_test_globals();
    package_harness::configure();
}

// 该辅助函数负责 启用 fast 添加 索引 failpoints。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn enable_fast_add_index_failpoints() -> (testfailpoint::FailGuard, testfailpoint::FailGuard) {
    let force_split = testfailpoint::enable(FORCE_SPLIT_REGION, "return(true)");
    let retry_backoff = testfailpoint::enable_call(ADJUST_RETRY_BACKOFF, || {
        RETRY_BACKOFF_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    });
    let old_hits = RETRY_BACKOFF_CALLBACKS.load(Ordering::SeqCst);
    testfailpoint::inject(ADJUST_RETRY_BACKOFF);
    assert!(testfailpoint::eval_bool(FORCE_SPLIT_REGION));
    assert_eq!(RETRY_BACKOFF_CALLBACKS.load(Ordering::SeqCst), old_hits + 1);
    (force_split, retry_backoff)
}

// 该辅助函数负责 初始化 multi schema context。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn init_multi_schema_context(t: &TestCtx) -> SuiteContext {
    let ctx = InitCompCtx(t);
    let comp = ctx
        .CompCtx
        .as_ref()
        .expect("InitCompCtx must install CompCtx");
    comp.write().unwrap().IsMultiSchemaChange = true;
    assert!(comp.read().unwrap().IsMultiSchemaChange);
    ctx
}

// 该辅助函数负责 断言 multi schema sql。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

fn assert_multi_schema_sql(ctx: &SuiteContext, t: &TestCtx, expected_ddl_count: usize) {
    let multi_schema_ddls = ctx
        .tk
        .execs()
        .into_iter()
        .filter(|sql| sql.contains(", add column"))
        .collect::<Vec<_>>();
    assert_eq!(
        multi_schema_ddls.len(),
        expected_ddl_count,
        "every add-index DDL must carry the extra schema change: {multi_schema_ddls:?}"
    );
    assert!(!t.Failed(), "multi-schema DDL/admin checks failed");
}

// 该用例覆盖 多 schema 变更 创建 普通 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

fn test_multi_schema_change_create_non_unique_index() {
    let _serial = serial_guard();
    reset_engine();
    let _failpoints = enable_fast_add_index_failpoints();
    let t = TestCtx::new();
    let ctx = init_multi_schema_context(&t);
    let col_ids = vec![vec![1, 4, 7], vec![2, 5, 8], vec![3, 6, 9]];

    TestOneColFrame(&ctx, &col_ids, AddIndexNonUnique);

    assert_multi_schema_sql(&ctx, &t, 9);
}

// 该用例覆盖 多 schema 变更 创建 唯一 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

fn test_multi_schema_change_create_unique_index() {
    let _serial = serial_guard();
    reset_engine();
    let _failpoints = enable_fast_add_index_failpoints();
    let t = TestCtx::new();
    let ctx = init_multi_schema_context(&t);
    let col_ids = vec![vec![1, 6, 8], vec![2, 19], vec![11]];

    TestOneColFrame(&ctx, &col_ids, AddIndexUnique);

    assert_multi_schema_sql(&ctx, &t, 6);
}

// 该用例覆盖 多 schema 变更 创建 主键。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

fn test_multi_schema_change_create_primary_key() {
    let _serial = serial_guard();
    reset_engine();
    let _failpoints = enable_fast_add_index_failpoints();
    let t = TestCtx::new();
    let ctx = init_multi_schema_context(&t);

    TestOneIndexFrame(&ctx, 0, AddIndexPK);

    assert_multi_schema_sql(&ctx, &t, 3);
}

// 该用例覆盖 多 schema 变更 创建 生成列 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

fn test_multi_schema_change_create_gen_col_index() {
    let _serial = serial_guard();
    reset_engine();
    let _failpoints = enable_fast_add_index_failpoints();
    let t = TestCtx::new();
    let ctx = init_multi_schema_context(&t);

    TestOneIndexFrame(&ctx, 29, AddIndexGenCol);

    assert_multi_schema_sql(&ctx, &t, 3);
}

// 该用例覆盖 多 schema 变更 多列 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

fn test_multi_schema_change_multi_cols_index() {
    let _serial = serial_guard();
    reset_engine();
    let _failpoints = enable_fast_add_index_failpoints();
    let t = TestCtx::new();
    let ctx = init_multi_schema_context(&t);
    let coli_ids = vec![vec![1], vec![2], vec![3]];
    let colj_ids = vec![vec![16], vec![14], vec![18]];

    TestTwoColsFrame(&ctx, &coli_ids, &colj_ids, AddIndexMultiCols);

    assert_multi_schema_sql(&ctx, &t, 3);
}

#[path = "package_harness.rs"]
mod package_harness;

fn main() -> std::process::ExitCode {
    package_harness::run(&[
        (
            "test_multi_schema_change_create_non_unique_index",
            test_multi_schema_change_create_non_unique_index,
        ),
        (
            "test_multi_schema_change_create_unique_index",
            test_multi_schema_change_create_unique_index,
        ),
        (
            "test_multi_schema_change_create_primary_key",
            test_multi_schema_change_create_primary_key,
        ),
        (
            "test_multi_schema_change_create_gen_col_index",
            test_multi_schema_change_create_gen_col_index,
        ),
        (
            "test_multi_schema_change_multi_cols_index",
            test_multi_schema_change_multi_cols_index,
        ),
    ])
}
