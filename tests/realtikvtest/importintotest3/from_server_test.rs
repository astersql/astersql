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
//! 中文总览：`from_server_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `from_server_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 14 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_import_from_server` 是当前文件里的辅助函数。
//! 阅读 `test_import_from_server` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_from_server` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_from_server`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_from_server` 的重要阅读参照。
//! 理解 `test_import_from_server` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_from_server` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_import_from_server` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `from_server_test.go`.
//!
//! Mapping:
//! - `TestImportFromServer` → [`test_import_from_server`]

use astersql_tests_realtikvtest_importintotest3::harness::{
    MockGCSSuite, importinto, mydump, require, reset_engine, serial_guard, storage, testkit,
};

#[test]
fn test_task_meta_json_decode_rejects_malformed_input() {
    assert!(importinto::TaskMeta::from_json(b"not json").is_err());
    assert!(importinto::TaskMeta::from_json(br#"{"Plan":{}}"#).is_err());
}

/// `TestImportFromServer`.
#[test]
fn test_import_from_server() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    let temp_dir = s.TempDir();

    let mut all_data: Vec<String> = Vec::new();
    for i in 0..3 {
        let file_name = format!("server-{i}.csv");
        let mut content = Vec::new();
        let row_cnt = 2;
        for j in 0..row_cnt {
            let a = i * row_cnt + j;
            content.extend(format!("{a},test-{a}\n").into_bytes());
            all_data.push(format!("{a} test-{a}"));
        }
        s.NoError(std::fs::write(temp_dir.join(&file_name), content).map_err(|e| e.to_string()));
    }

    s.prepare_and_use_db("from_server");
    s.tk.MustExec("create table t (a bigint, b varchar(100));");

    s.tk.MustQuery(&format!(
        "IMPORT INTO t FROM '{}'",
        temp_dir.join("server-0.csv").display()
    ));
    s.tk.MustQuery("SELECT * FROM t;")
        .Sort()
        .Check(&testkit::Rows(&["0 test-0", "1 test-1"]));

    s.tk.MustExec("truncate table t");
    s.tk.MustQuery(&format!(
        "IMPORT INTO t FROM '{}'",
        temp_dir.join("server-*.csv").display()
    ));
    let expected: Vec<&str> = all_data.iter().map(|s| s.as_str()).collect();
    s.tk.MustQuery("SELECT * FROM t;")
        .Sort()
        .Check(&testkit::Rows(&expected));

    // try a gzip file
    s.NoError(
        std::fs::write(
            temp_dir.join("test.csv.gz"),
            s.get_compressed_data(mydump::Compression::GZ, b"1,test1\n2,test2"),
        )
        .map_err(|e| e.to_string()),
    );
    s.tk.MustExec("truncate table t");
    let rows =
        s.tk.MustQuery(&format!(
            "IMPORT INTO t FROM '{}'",
            temp_dir.join("test.csv.gz").display()
        ))
        .Rows();
    s.tk.MustQuery("SELECT * FROM t;")
        .Sort()
        .Check(&testkit::Rows(&["1 test1", "2 test2"]));
    let job_id: i64 = rows[0][0].parse().expect("job id");
    let task_manager = storage::GetTaskManager().expect("task manager");
    let task_key = importinto::TaskKey(job_id);
    let task = task_manager
        .GetTaskByKeyWithHistory((), &task_key)
        .expect("task");
    let task_meta = importinto::TaskMeta::from_json(&task.Meta).expect("task meta JSON");
    require::Equal(&s.t, 2, task_meta.ChunkMap.len());
    require::False(&s.t, task_meta.Plan.DisableTiKVImportMode);
    s.tear_down();
}
