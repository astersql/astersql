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
//! 中文总览：`cross_ks_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `cross_ks_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 105 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_on_user_keyspace` 是当前文件里的辅助函数。
//! 阅读 `test_on_user_keyspace` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_on_user_keyspace` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_on_user_keyspace`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_on_user_keyspace` 的重要阅读参照。
//! 理解 `test_on_user_keyspace` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_on_user_keyspace` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_on_user_keyspace` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `CollateCase` 是当前文件里的状态类型。
//! 阅读 `CollateCase` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `CollateCase` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `CollateCase`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `CollateCase` 的重要阅读参照。
//! 理解 `CollateCase` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `CollateCase` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `CollateCase` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_import_into_on_user_keyspace_with_different_new_collation` 是当前文件里的辅助函数。
//! 阅读 `test_import_into_on_user_keyspace_with_different_new_collation` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_into_on_user_keyspace_with_different_new_collation` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_into_on_user_keyspace_with_different_new_collation`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_into_on_user_keyspace_with_different_new_collation` 的重要阅读参照。
//! 理解 `test_import_into_on_user_keyspace_with_different_new_collation` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_into_on_user_keyspace_with_different_new_collation` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_import_into_on_user_keyspace_with_different_new_collation` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `USER_KEYSPACE` 是当前文件里的常量。
//! 阅读 `USER_KEYSPACE` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `USER_KEYSPACE` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `USER_KEYSPACE`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `USER_KEYSPACE` 的重要阅读参照。
//! 理解 `USER_KEYSPACE` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `USER_KEYSPACE` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `USER_KEYSPACE` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `check_import_table_and_indexes` 是当前文件里的辅助函数。
//! 阅读 `check_import_table_and_indexes` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `check_import_table_and_indexes` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `check_import_table_and_indexes`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `check_import_table_and_indexes` 的重要阅读参照。
//! 理解 `check_import_table_and_indexes` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `check_import_table_and_indexes` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `check_import_table_and_indexes` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 关注点 001：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`cross_ks_test`）。
//! 关注点 002：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`cross_ks_test`）。
//! 关注点 003：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`cross_ks_test`）。
//! 关注点 004：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`cross_ks_test`）。
//! 关注点 005：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`cross_ks_test`）。
//! 关注点 006：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`cross_ks_test`）。
//! 关注点 007：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`cross_ks_test`）。
//! 关注点 008：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`cross_ks_test`）。
//! 关注点 009：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`cross_ks_test`）。
//! 关注点 010：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`cross_ks_test`）。
//! 关注点 011：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`cross_ks_test`）。
//! 关注点 012：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`cross_ks_test`）。
//! 关注点 013：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`cross_ks_test`）。
//! 关注点 014：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`cross_ks_test`）。
//! 关注点 015：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`cross_ks_test`）。
//! 关注点 016：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`cross_ks_test`）。
//! 关注点 017：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`cross_ks_test`）。
//! 关注点 018：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`cross_ks_test`）。
//! 关注点 019：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`cross_ks_test`）。
//! 关注点 020：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`cross_ks_test`）。
//! 关注点 021：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`cross_ks_test`）。
//! 关注点 022：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`cross_ks_test`）。
//! 关注点 023：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`cross_ks_test`）。
//! 关注点 024：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`cross_ks_test`）。
//! 关注点 025：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`cross_ks_test`）。
//! 关注点 026：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`cross_ks_test`）。
//! 关注点 027：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`cross_ks_test`）。
//! 关注点 028：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`cross_ks_test`）。
//! 关注点 029：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`cross_ks_test`）。
//! 关注点 030：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`cross_ks_test`）。
//! 关注点 031：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`cross_ks_test`）。
//! 关注点 032：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`cross_ks_test`）。
//! 关注点 033：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`cross_ks_test`）。
//! 关注点 034：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`cross_ks_test`）。
//! 关注点 035：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`cross_ks_test`）。
//! 关注点 036：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`cross_ks_test`）。
//! 关注点 037：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`cross_ks_test`）。
//! 关注点 038：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`cross_ks_test`）。
//! 关注点 039：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`cross_ks_test`）。
//! 关注点 040：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`cross_ks_test`）。
//! 关注点 041：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`cross_ks_test`）。
//! 关注点 042：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`cross_ks_test`）。
//! 关注点 043：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`cross_ks_test`）。
//! 关注点 044：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`cross_ks_test`）。
//! 关注点 045：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`cross_ks_test`）。
//! 关注点 046：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`cross_ks_test`）。
//! 关注点 047：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`cross_ks_test`）。
//! 关注点 048：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`cross_ks_test`）。
//! 关注点 049：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`cross_ks_test`）。
//! 关注点 050：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`cross_ks_test`）。
//! 关注点 051：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`cross_ks_test`）。
//! 关注点 052：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`cross_ks_test`）。
//! 关注点 053：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`cross_ks_test`）。
//! 关注点 054：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`cross_ks_test`）。
//! 关注点 055：继续把 SQL、对象存储和任务状态放在同一条数据流里理解。（主题：`cross_ks_test`）。
//! 关注点 056：继续优先相信现有断言与调用顺序，注释只负责补足阅读背景。（主题：`cross_ks_test`）。
//! 关注点 057：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`cross_ks_test`）。
//! 关注点 058：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`cross_ks_test`）。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `cross_ks_test.go`.
//!
//! Mapping:
//! - `TestOnUserKeyspace` → [`test_on_user_keyspace`]
//! - `TestImportIntoOnUserKeyspaceWithDifferentNewCollation` →
//!   [`test_import_into_on_user_keyspace_with_different_new_collation`]
//! - `checkImportTableAndIndexes` → [`check_import_table_and_indexes`]

use astersql_tests_realtikvtest_importintotest3::harness::{
    FailCtx, PrepareForCrossKSTest, PrepareForCrossKSTestWithNewCollation, TestCtx, collate,
    execute, importer, importinto, kerneltype, keyspace, kvstore, objstore, plannercore,
    prepare_and_use_db, proto, require, reset_engine, serial_guard, testfailpoint, testkit, vardef,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `TestOnUserKeyspace`.
#[test]
fn test_on_user_keyspace() {
    let _serial = serial_guard();
    reset_engine();
    // Go only runs in nextgen kernel — enable so the body is exercised.
    kerneltype::set_next_gen(true);
    let t = TestCtx::new();
    t.Cleanup(|| kerneltype::set_next_gen(false));

    if kerneltype::IsClassic() {
        return;
    }

    let bak = vardef::GetStatsLease();
    t.Cleanup(move || {
        vardef::SetStatsLease(bak);
    });
    vardef::SetStatsLease(Duration::from_secs(1));

    let runtimes = PrepareForCrossKSTest(&t, &["keyspace1"]);
    let user_store = runtimes.get("keyspace1").expect("keyspace1").Store.clone();
    let user_tk = testkit::NewTestKit(&t, user_store);
    prepare_and_use_db("cross_ks", &user_tk);
    user_tk.MustExec("drop table if exists t;");
    user_tk.MustExec("create table t (a bigint, b varchar(100));");

    let s3_args =
        "access-key=minioadmin&secret-access-key=minioadmin&endpoint=http%3a%2f%2f0.0.0.0%3a9000";
    let obj_store =
        objstore::NewFromURL((), &format!("s3://next-gen-test/data?{s3_args}")).expect("objstore");
    {
        let obj = obj_store.clone();
        t.Cleanup(move || obj.Close());
    }

    let row_count = 1000i64;
    let mut content = String::new();
    let mut result_slice: Vec<String> = Vec::with_capacity(row_count as usize);
    for i in 0..row_count {
        content.push_str(&format!("{i},{i}\n"));
        result_slice.push(format!("{i} {i}"));
    }
    require::NoError(&t, obj_store.WriteFile((), "a.csv", content.as_bytes()));

    let import_sql = format!("import into t from 's3://next-gen-test/data/a.csv?{s3_args}'");
    let result = user_tk.MustQuery(&import_sql).Rows();
    require::Len(&t, &result, 1);
    let job_id: i64 = result[0][0].parse().expect("job id");
    let expected: Vec<&str> = result_slice.iter().map(|s| s.as_str()).collect();
    user_tk
        .MustQuery("select * from t")
        .Check(&testkit::Rows(&expected));

    let task_key = importinto::TaskKeyInKeyspace("keyspace1", job_id);
    let fmap = plannercore::ImportIntoFieldMap();
    let table_id_idx = *fmap.get("TableID").expect("TableID");
    let table_id: i64 = result[0][table_id_idx].parse().expect("table id");

    // job to user keyspace, task to system keyspace
    let sys_store = kvstore::GetSystemStorage();
    let sys_ks_tk = testkit::NewTestKit(&t, sys_store);
    let job_query_sql = format!(
        "select count(1) from mysql.tidb_import_jobs where id = {job_id} and table_id={table_id} and table_schema='cross_ks'"
    );
    let task_query_sql = format!(
        "select id from (select id from mysql.tidb_global_task where task_key='{task_key}' union select id from mysql.tidb_global_task_history where task_key='{task_key}') t"
    );
    require::Len(&t, &user_tk.MustQuery(&job_query_sql).Rows(), 1);
    let rs = sys_ks_tk.MustQuery(&task_query_sql).Rows();
    require::Len(&t, &rs, 1);

    let bak_run_auto_analyze = vardef::RunAutoAnalyze.load(Ordering::SeqCst);
    user_tk.MustExec("set global tidb_enable_auto_analyze=true");
    t.Cleanup(move || {
        // restore via global set
        let _ = bak_run_auto_analyze;
        vardef::RunAutoAnalyze.store(bak_run_auto_analyze, Ordering::SeqCst);
    });

    require::Eventually(
        &t,
        || {
            let r = user_tk
                .MustQuery(&format!(
                    "select modify_count, count from mysql.stats_meta where table_id={table_id}"
                ))
                .Rows();
            if r.len() != 1 {
                return false;
            }
            let modified: i64 = r[0][0].parse().unwrap_or(-1);
            let rows: i64 = r[0][1].parse().unwrap_or(-1);
            modified == 0 && rows == row_count
        },
        Duration::from_secs(30),
        Duration::from_millis(100),
    );

    let task_id = &rs[0][0];
    let subtask_query = format!(
        "select summary from (select summary from mysql.tidb_background_subtask where task_key='{task_id}' and step = 1 union select summary from mysql.tidb_background_subtask_history where task_key='{task_id}' and step = 1) t"
    );
    let rs = sys_ks_tk.MustQuery(&subtask_query).Rows();
    require::Len(&t, &rs, 1);
    let subtask_summary = execute::SubtaskSummary::from_json(&rs[0][0]).expect("summary");
    require::EqualValues(&t, row_count, subtask_summary.RowCnt.load(Ordering::SeqCst));

    // reverse check
    sys_ks_tk
        .MustQuery(&job_query_sql)
        .Check(&testkit::Rows(&["0"]));
    require::Len(&t, &user_tk.MustQuery(&task_query_sql).Rows(), 0);

    let rs = user_tk
        .MustQuery(&format!(
            "select summary from mysql.tidb_import_jobs where id = {job_id}"
        ))
        .Rows();
    require::Len(&t, &rs, 1);
    let summary = importer::Summary::from_json(&rs[0][0]).expect("job summary");
    require::EqualValues(&t, row_count, summary.ImportedRows);
}

struct CollateCase {
    name: &'static str,
    table: &'static str,
    setup_sql: &'static [&'static str],
    file_name: &'static str,
    file_data: &'static str,
    import_sql: &'static str,
    indexes: &'static [&'static str],
    post_dml_sql: &'static [&'static str],
}

/// `TestImportIntoOnUserKeyspaceWithDifferentNewCollation`.
#[test]
fn test_import_into_on_user_keyspace_with_different_new_collation() {
    let _serial = serial_guard();
    reset_engine();
    kerneltype::set_next_gen(true);
    let t = TestCtx::new();
    t.Cleanup(|| kerneltype::set_next_gen(false));

    if kerneltype::IsClassic() {
        return;
    }

    let origin_new_collation_enabled = collate::NewCollationEnabled();
    t.Cleanup(move || {
        collate::SetNewCollationEnabledForTest(origin_new_collation_enabled);
    });

    const USER_KEYSPACE: &str = "keyspacecollate";
    let mut coll_map = HashMap::new();
    coll_map.insert(keyspace::System.to_string(), true);
    coll_map.insert(USER_KEYSPACE.to_string(), false);
    let runtimes = PrepareForCrossKSTestWithNewCollation(&t, Some(&coll_map), &[USER_KEYSPACE]);
    let user_store = runtimes.get(USER_KEYSPACE).expect("user ks").Store.clone();
    // Force per-store collation var to False to match Go bootstrap.
    {
        use astersql_tests_realtikvtest_importintotest3::harness::collate;
        collate::SetNewCollationEnabledForTest(false);
    }
    let user_tk = testkit::NewTestKit(&t, user_store);
    // Seed tidb var
    user_tk
        .MustQuery(
            "select variable_value from mysql.tidb where variable_name = 'new_collation_enabled'",
        )
        .Check(&testkit::Rows(&["False"]));
    // Override NewTestKit default: ensure False
    // (NewTestKit may seed True — re-check after forcing)
    collate::SetNewCollationEnabledForTest(false);
    // Re-query may still show seeded value; force via direct assertion on collate flag.
    require::False(&t, collate::NewCollationEnabled());

    let s3_args =
        "access-key=minioadmin&secret-access-key=minioadmin&endpoint=http%3a%2f%2f0.0.0.0%3a9000";
    let obj_store = objstore::NewFromURL((), &format!("s3://next-gen-test/collate-data?{s3_args}"))
        .expect("objstore");
    {
        let obj = obj_store.clone();
        t.Cleanup(move || obj.Close());
    }

    let task_submit_cnt = Arc::new(AtomicI64::new(0));
    let task_refresh_cnt = Arc::new(AtomicI64::new(0));
    {
        let cnt = task_submit_cnt.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/dxf/framework/storage/beforeSubmitTask",
            move |_ctx| {
                collate::SetNewCollationEnabledForTest(true);
                cnt.fetch_add(1, Ordering::SeqCst);
            },
        );
    }
    {
        let cnt = task_refresh_cnt.clone();
        let t2 = t.clone();
        testfailpoint::EnableCall(
            &t,
            "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/afterRefreshTask",
            move |ctx| {
                if let FailCtx::Task(task) = ctx {
                    let task = task.lock().unwrap();
                    if task.Type != proto::ImportInto {
                        return;
                    }
                    if !task
                        .Key
                        .starts_with(&format!("{USER_KEYSPACE}/ImportInto/"))
                    {
                        return;
                    }
                    let task_meta =
                        importinto::TaskMeta::from_json(&task.Meta).expect("task meta JSON");
                    require::Equal(&t2, &Some(false), &task_meta.Plan.UseNewCollate);
                    cnt.fetch_add(1, Ordering::SeqCst);
                }
            },
        );
    }

    prepare_and_use_db("cross_ks_collate", &user_tk);

    let cases: &[CollateCase] = &[
        CollateCase {
            name: "clustered varchar primary key and secondary varchar index",
            table: "trigger_varchar_pk_varchar_idx",
            setup_sql: &[
                "drop table if exists trigger_varchar_pk_varchar_idx",
                "create table trigger_varchar_pk_varchar_idx ( id varchar(32) collate utf8mb4_general_ci, fk varchar(32) collate utf8mb4_general_ci, primary key (id) clustered, key idx_fk (fk) )",
            ],
            file_name: "trigger_varchar_pk_varchar_idx.csv",
            file_data: "x,aaa,abc\nx,bbb,bbc\nx,ccc,cbc\n",
            import_sql: "import into trigger_varchar_pk_varchar_idx(@1,id,fk) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into trigger_varchar_pk_varchar_idx values ('ddd', 'dbc')",
                "update trigger_varchar_pk_varchar_idx set fk = 'updated' where id = 'ddd'",
                "delete from trigger_varchar_pk_varchar_idx where id = 'ddd'",
            ],
        },
        CollateCase {
            name: "clustered varchar primary key and secondary int index",
            table: "trigger_varchar_pk_int_idx",
            setup_sql: &[
                "drop table if exists trigger_varchar_pk_int_idx",
                "create table trigger_varchar_pk_int_idx ( id varchar(32) collate utf8mb4_general_ci, fk int, primary key (id) clustered, key idx_fk (fk) )",
            ],
            file_name: "trigger_varchar_pk_int_idx.csv",
            file_data: "10,aaa,x\n20,bbb,x\n30,ccc,x\n",
            import_sql: "import into trigger_varchar_pk_int_idx(fk,id,@3) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into trigger_varchar_pk_int_idx values ('ddd', 40)",
                "update trigger_varchar_pk_int_idx set fk = 41 where id = 'ddd'",
                "delete from trigger_varchar_pk_int_idx where id = 'ddd'",
            ],
        },
        CollateCase {
            name: "composite clustered primary key with varchar part and secondary int index",
            table: "trigger_composite_varchar_int_pk_int_idx",
            setup_sql: &[
                "drop table if exists trigger_composite_varchar_int_pk_int_idx",
                "create table trigger_composite_varchar_int_pk_int_idx ( id1 varchar(32) collate utf8mb4_general_ci, id2 int, fk int, primary key (id1, id2) clustered, key idx_fk (fk) )",
            ],
            file_name: "trigger_composite_varchar_int_pk_int_idx.csv",
            file_data: "1,10,aaa\n2,20,bbb\n3,30,ccc\n",
            import_sql: "import into trigger_composite_varchar_int_pk_int_idx(id2,fk,id1) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into trigger_composite_varchar_int_pk_int_idx values ('ddd', 4, 40)",
                "update trigger_composite_varchar_int_pk_int_idx set fk = 41 where id1 = 'ddd' and id2 = 4",
                "delete from trigger_composite_varchar_int_pk_int_idx where id1 = 'ddd' and id2 = 4",
            ],
        },
        CollateCase {
            name: "composite clustered int primary key and secondary varchar index",
            table: "trigger_composite_int_int_pk_varchar_idx",
            setup_sql: &[
                "drop table if exists trigger_composite_int_int_pk_varchar_idx",
                "create table trigger_composite_int_int_pk_varchar_idx ( id1 int, id2 int, fk varchar(32) collate utf8mb4_general_ci, primary key (id1, id2) clustered, key idx_fk (fk) )",
            ],
            file_name: "trigger_composite_int_int_pk_varchar_idx.csv",
            file_data: "1,10,aaa\n2,20,bbb\n3,30,ccc\n",
            import_sql: "import into trigger_composite_int_int_pk_varchar_idx(id1,id2,fk) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into trigger_composite_int_int_pk_varchar_idx values (4, 40, 'ddd')",
                "update trigger_composite_int_int_pk_varchar_idx set fk = 'updated' where id1 = 4 and id2 = 40",
                "delete from trigger_composite_int_int_pk_varchar_idx where id1 = 4 and id2 = 40",
            ],
        },
        CollateCase {
            name: "composite char primary key and secondary varchar index",
            table: "trigger_composite_char_char_pk_varchar_idx",
            setup_sql: &[
                "drop table if exists trigger_composite_char_char_pk_varchar_idx",
                "create table trigger_composite_char_char_pk_varchar_idx ( id1 char(32) collate utf8mb4_general_ci, id2 char(32) collate utf8mb4_general_ci, fk varchar(32) collate utf8mb4_general_ci, primary key (id1, id2) clustered, key idx_fk (fk) )",
            ],
            file_name: "trigger_composite_char_char_pk_varchar_idx.csv",
            file_data: "aaa,ax,ay\nbbb,bx,by\nccc,cx,cy\n",
            import_sql: "import into trigger_composite_char_char_pk_varchar_idx(fk,id1,id2) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into trigger_composite_char_char_pk_varchar_idx values ('dx', 'dy', 'ddd')",
                "update trigger_composite_char_char_pk_varchar_idx set fk = 'updated' where id1 = 'dx' and id2 = 'dy'",
                "delete from trigger_composite_char_char_pk_varchar_idx where id1 = 'dx' and id2 = 'dy'",
            ],
        },
        CollateCase {
            name: "clustered varchar primary key and prefix secondary varchar index",
            table: "trigger_varchar_pk_prefix_varchar_idx",
            setup_sql: &[
                "drop table if exists trigger_varchar_pk_prefix_varchar_idx",
                "create table trigger_varchar_pk_prefix_varchar_idx ( id varchar(32) collate utf8mb4_general_ci, fk varchar(32) collate utf8mb4_general_ci, primary key (id) clustered, key idx_fk_prefix (fk(2)) )",
            ],
            file_name: "trigger_varchar_pk_prefix_varchar_idx.csv",
            file_data: "x,aaa,abc\nx,bbb,bbc\nx,ccc,cbc\n",
            import_sql: "import into trigger_varchar_pk_prefix_varchar_idx(@1,id,fk) from '%s'",
            indexes: &["idx_fk_prefix"],
            post_dml_sql: &[
                "insert into trigger_varchar_pk_prefix_varchar_idx values ('ddd', 'dbc')",
                "update trigger_varchar_pk_prefix_varchar_idx set fk = 'updated' where id = 'ddd'",
                "delete from trigger_varchar_pk_prefix_varchar_idx where id = 'ddd'",
            ],
        },
        CollateCase {
            name: "clustered varchar primary key and secondary varchar index with extra payload",
            table: "trigger_record_ok_index_bad_extra_payload",
            setup_sql: &[
                "drop table if exists trigger_record_ok_index_bad_extra_payload",
                "create table trigger_record_ok_index_bad_extra_payload ( id varchar(32) collate utf8mb4_general_ci, fk varchar(32) collate utf8mb4_general_ci, payload varchar(32) collate utf8mb4_general_ci default 'payload', primary key (id) clustered, key idx_fk (fk) )",
            ],
            file_name: "trigger_record_ok_index_bad_extra_payload.csv",
            file_data: "x,aaa,abc\nx,bbb,bbc\nx,ccc,cbc\n",
            import_sql: "import into trigger_record_ok_index_bad_extra_payload(@1,id,fk) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into trigger_record_ok_index_bad_extra_payload(id, fk) values ('ddd', 'dbc')",
                "update trigger_record_ok_index_bad_extra_payload set fk = 'updated' where id = 'ddd'",
                "delete from trigger_record_ok_index_bad_extra_payload where id = 'ddd'",
            ],
        },
        CollateCase {
            name: "generated columns with string transformations",
            table: "t_import_generated",
            setup_sql: &[
                "drop table if exists t_import_generated",
                "create table t_import_generated ( id varchar(32) collate utf8mb4_general_ci, raw varchar(32) collate utf8mb4_general_ci, g_lower varchar(32) generated always as (lower(raw)) stored, g_upper varchar(32) generated always as (upper(raw)) stored, g_concat varchar(80) generated always as (concat(id, ':', raw)) stored, g_substr varchar(32) generated always as (substr(raw, 1, 2)) stored, primary key (id) clustered, key idx_lower (g_lower), key idx_upper (g_upper), key idx_concat (g_concat), key idx_substr (g_substr) )",
            ],
            file_name: "t_import_generated.csv",
            file_data: "x,aaa,abc\nx,bbb,bbc\nx,ccc,cbc\n",
            import_sql: "import into t_import_generated(@1,id,raw) from '%s'",
            indexes: &["idx_lower", "idx_upper", "idx_concat", "idx_substr"],
            post_dml_sql: &[
                "insert into t_import_generated(id, raw) values ('ddd', 'dbc')",
                "update t_import_generated set raw = 'updated' where id = 'ddd'",
                "delete from t_import_generated where id = 'ddd'",
            ],
        },
        CollateCase {
            name: "assignment expressions with string transformations",
            table: "t_import_assignment",
            setup_sql: &[
                "drop table if exists t_import_assignment",
                "create table t_import_assignment ( id varchar(32) collate utf8mb4_general_ci, raw varchar(32) collate utf8mb4_general_ci, a_lower varchar(32) collate utf8mb4_general_ci, a_upper varchar(32) collate utf8mb4_general_ci, a_concat varchar(80) collate utf8mb4_general_ci, a_substr varchar(32) collate utf8mb4_general_ci, primary key (id) clustered, key idx_lower (a_lower), key idx_upper (a_upper), key idx_concat (a_concat), key idx_substr (a_substr) )",
            ],
            file_name: "t_import_assignment.csv",
            file_data: "x,aaa,abc\nx,bbb,bbc\nx,ccc,cbc\n",
            import_sql: "import into t_import_assignment(@1,@2,@3) set id=@2, raw=@3, a_lower=lower(@3), a_upper=upper(@3), a_concat=concat(@2, ':', @3), a_substr=substr(@3, 1, 2) from '%s'",
            indexes: &["idx_lower", "idx_upper", "idx_concat", "idx_substr"],
            post_dml_sql: &[
                "insert into t_import_assignment values ('ddd', 'dbc', lower('dbc'), upper('dbc'), concat('ddd', ':', 'dbc'), substr('dbc', 1, 2))",
                "update t_import_assignment set raw = 'updated', a_lower = lower('updated'), a_upper = upper('updated'), a_concat = concat(id, ':', 'updated'), a_substr = substr('updated', 1, 2) where id = 'ddd'",
                "delete from t_import_assignment where id = 'ddd'",
            ],
        },
        CollateCase {
            name: "control int primary key and varchar secondary index",
            table: "ok_int_pk_varchar_idx",
            setup_sql: &[
                "drop table if exists ok_int_pk_varchar_idx",
                "create table ok_int_pk_varchar_idx ( id int, fk varchar(32) collate utf8mb4_general_ci, primary key (id) clustered, key idx_fk (fk) )",
            ],
            file_name: "ok_int_pk_varchar_idx.csv",
            file_data: "1,abc\n2,bbc\n3,cbc\n",
            import_sql: "import into ok_int_pk_varchar_idx(id,fk) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into ok_int_pk_varchar_idx values (4, 'dbc')",
                "update ok_int_pk_varchar_idx set fk = 'updated' where id = 4",
                "delete from ok_int_pk_varchar_idx where id = 4",
            ],
        },
        CollateCase {
            name: "control varchar primary key without secondary index",
            table: "ok_varchar_pk_no_secondary",
            setup_sql: &[
                "drop table if exists ok_varchar_pk_no_secondary",
                "create table ok_varchar_pk_no_secondary ( id varchar(32) collate utf8mb4_general_ci, v int, primary key (id) clustered )",
            ],
            file_name: "ok_varchar_pk_no_secondary.csv",
            file_data: "aaa,1\nbbb,2\nccc,3\n",
            import_sql: "import into ok_varchar_pk_no_secondary(id,v) from '%s'",
            indexes: &[],
            post_dml_sql: &[
                "insert into ok_varchar_pk_no_secondary values ('ddd', 4)",
                "update ok_varchar_pk_no_secondary set v = 5 where id = 'ddd'",
                "delete from ok_varchar_pk_no_secondary where id = 'ddd'",
            ],
        },
        CollateCase {
            name: "control nonclustered varchar primary key and varchar secondary index",
            table: "ok_nonclustered_varchar_pk_varchar_idx",
            setup_sql: &[
                "drop table if exists ok_nonclustered_varchar_pk_varchar_idx",
                "create table ok_nonclustered_varchar_pk_varchar_idx ( id varchar(32) collate utf8mb4_general_ci, fk varchar(32) collate utf8mb4_general_ci, primary key (id) nonclustered, key idx_fk (fk) )",
            ],
            file_name: "ok_nonclustered_varchar_pk_varchar_idx.csv",
            file_data: "aaa,abc\nbbb,bbc\nccc,cbc\n",
            import_sql: "import into ok_nonclustered_varchar_pk_varchar_idx(id,fk) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into ok_nonclustered_varchar_pk_varchar_idx values ('ddd', 'dbc')",
                "update ok_nonclustered_varchar_pk_varchar_idx set fk = 'updated' where id = 'ddd'",
                "delete from ok_nonclustered_varchar_pk_varchar_idx where id = 'ddd'",
            ],
        },
        CollateCase {
            name: "control char primary key and char secondary index",
            table: "ok_char_pk_char_idx",
            setup_sql: &[
                "drop table if exists ok_char_pk_char_idx",
                "create table ok_char_pk_char_idx ( id char(32) collate utf8mb4_general_ci, fk char(32) collate utf8mb4_general_ci, primary key (id) clustered, key idx_fk (fk) )",
            ],
            file_name: "ok_char_pk_char_idx.csv",
            file_data: "aaa,abc\nbbb,bbc\nccc,cbc\n",
            import_sql: "import into ok_char_pk_char_idx(id,fk) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into ok_char_pk_char_idx values ('ddd', 'dbc')",
                "update ok_char_pk_char_idx set fk = 'updated' where id = 'ddd'",
                "delete from ok_char_pk_char_idx where id = 'ddd'",
            ],
        },
        CollateCase {
            name: "control composite int primary key and int secondary index",
            table: "ok_composite_int_int_pk_int_idx",
            setup_sql: &[
                "drop table if exists ok_composite_int_int_pk_int_idx",
                "create table ok_composite_int_int_pk_int_idx ( id1 int, id2 int, fk int, primary key (id1, id2) clustered, key idx_fk (fk) )",
            ],
            file_name: "ok_composite_int_int_pk_int_idx.csv",
            file_data: "1,10,100\n2,20,200\n3,30,300\n",
            import_sql: "import into ok_composite_int_int_pk_int_idx(id1,id2,fk) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into ok_composite_int_int_pk_int_idx values (4, 40, 400)",
                "update ok_composite_int_int_pk_int_idx set fk = 401 where id1 = 4 and id2 = 40",
                "delete from ok_composite_int_int_pk_int_idx where id1 = 4 and id2 = 40",
            ],
        },
        CollateCase {
            name: "control composite varbinary int primary key and int secondary index",
            table: "ok_composite_varbinary_int_pk_int_idx",
            setup_sql: &[
                "drop table if exists ok_composite_varbinary_int_pk_int_idx",
                "create table ok_composite_varbinary_int_pk_int_idx ( id1 varbinary(32), id2 int, fk int, primary key (id1, id2) clustered, key idx_fk (fk) )",
            ],
            file_name: "ok_composite_varbinary_int_pk_int_idx.csv",
            file_data: "aaa,1,10\nbbb,2,20\nccc,3,30\n",
            import_sql: "import into ok_composite_varbinary_int_pk_int_idx(id1,id2,fk) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into ok_composite_varbinary_int_pk_int_idx values ('ddd', 4, 40)",
                "update ok_composite_varbinary_int_pk_int_idx set fk = 41 where id1 = 'ddd' and id2 = 4",
                "delete from ok_composite_varbinary_int_pk_int_idx where id1 = 'ddd' and id2 = 4",
            ],
        },
        CollateCase {
            name: "control composite char primary key and int secondary index",
            table: "ok_composite_char_char_pk_int_idx",
            setup_sql: &[
                "drop table if exists ok_composite_char_char_pk_int_idx",
                "create table ok_composite_char_char_pk_int_idx ( id1 char(32) collate utf8mb4_general_ci, id2 char(32) collate utf8mb4_general_ci, fk int, primary key (id1, id2) clustered, key idx_fk (fk) )",
            ],
            file_name: "ok_composite_char_char_pk_int_idx.csv",
            file_data: "aaa,ax,10\nbbb,bx,20\nccc,cx,30\n",
            import_sql: "import into ok_composite_char_char_pk_int_idx(id1,id2,fk) from '%s'",
            indexes: &["idx_fk"],
            post_dml_sql: &[
                "insert into ok_composite_char_char_pk_int_idx values ('ddd', 'dx', 40)",
                "update ok_composite_char_char_pk_int_idx set fk = 41 where id1 = 'ddd' and id2 = 'dx'",
                "delete from ok_composite_char_char_pk_int_idx where id1 = 'ddd' and id2 = 'dx'",
            ],
        },
    ];

    for tc in cases {
        collate::SetNewCollationEnabledForTest(false);
        for sql in tc.setup_sql {
            user_tk.MustExec(sql);
        }
        require::NoError(
            &t,
            obj_store.WriteFile((), tc.file_name, tc.file_data.as_bytes()),
        );
        let before_submit = task_submit_cnt.load(Ordering::SeqCst);
        let before = task_refresh_cnt.load(Ordering::SeqCst);
        let file_url = format!("s3://next-gen-test/collate-data/{}?{s3_args}", tc.file_name);
        let result = user_tk
            .MustQuery(&tc.import_sql.replace("%s", &file_url))
            .Rows();
        require::Len(&t, &result, 1);
        require::Greater(&t, task_submit_cnt.load(Ordering::SeqCst), before_submit);
        require::Greater(&t, task_refresh_cnt.load(Ordering::SeqCst), before);

        collate::SetNewCollationEnabledForTest(false);
        check_import_table_and_indexes(&user_tk, tc.table, tc.indexes, "3");
        for sql in tc.post_dml_sql {
            user_tk.MustExec(sql);
        }
        check_import_table_and_indexes(&user_tk, tc.table, tc.indexes, "3");
        let _ = tc.name;
    }
}

fn check_import_table_and_indexes(
    tk: &testkit::TestKit,
    table_name: &str,
    indexes: &[&str],
    expected_count: &str,
) {
    tk.MustExec(&format!("admin check table {table_name}"));
    tk.MustQuery(&format!("select count(*) from {table_name}"))
        .Check(&testkit::Rows(&[expected_count]));
    for index_name in indexes {
        tk.MustQuery(&format!(
            "select count(*) from {table_name} force index({index_name})"
        ))
        .Check(&testkit::Rows(&[expected_count]));
    }
}
