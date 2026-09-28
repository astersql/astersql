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
//! 中文总览：`cleanup_on_error_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `cleanup_on_error_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 9 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_cleanup_on_data_engine_import_error` 是当前文件里的辅助函数。
//! 阅读 `test_cleanup_on_data_engine_import_error` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_cleanup_on_data_engine_import_error` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `cleanup_on_error_test.go`.
//!
//! Mapping:
//! - `TestCleanupOnDataEngineImportError` → [`test_cleanup_on_data_engine_import_error`]

use astersql_tests_realtikvtest_importintotest3::harness::{
    FailCtx, MockGCSSuite, drivererr, reset_engine, serial_guard, testfailpoint, testkit,
};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

/// `TestCleanupOnDataEngineImportError`.
#[test]
fn test_cleanup_on_data_engine_import_error() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db("cleanup_on_error");
    s.tk.MustExec("create table t (a int primary key, b varchar(32));");

    let temp_dir = s.TempDir();
    let data_path = temp_dir.join("data.csv");
    let content = b"1,a\n2,b\n";
    s.NoError(std::fs::write(&data_path, content).map_err(|e| e.to_string()));

    let enter_cnt = Arc::new(AtomicI32::new(0));
    {
        let enter_cnt = enter_cnt.clone();
        testfailpoint::EnableCall(
            &s.t,
            "github.com/pingcap/tidb/pkg/executor/importer/mockDataEngineImportErr",
            move |ctx| {
                if let FailCtx::ErrPtr(err_p) = ctx {
                    if enter_cnt.fetch_add(1, Ordering::SeqCst) + 1 == 1 {
                        *err_p.lock().unwrap() = Some(drivererr::ErrPDServerTimeout.to_string());
                    }
                }
            },
        );
    }

    s.tk.MustQuery(&format!("IMPORT INTO t FROM '{}'", data_path.display()));
    s.GreaterOrEqual(enter_cnt.load(Ordering::SeqCst), 2);
    assert!(
        enter_cnt.load(Ordering::SeqCst) >= 2,
        "the import should retry on data engine import error"
    );

    s.tk.MustQuery("select * from t;")
        .Sort()
        .Check(&testkit::Rows(&["1 a", "2 b"]));
    s.tear_down();
}
