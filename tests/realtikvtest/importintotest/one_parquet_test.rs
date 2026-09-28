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
//! 中文总览：`one_parquet_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `one_parquet_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 13 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `PARQUET_CONTENT` 是当前文件里的常量。
//! 阅读 `PARQUET_CONTENT` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `PARQUET_CONTENT` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `PARQUET_CONTENT`，应从这里理解职责边界。
//! 符号 `test_detached_load_parquet` 是当前文件里的辅助函数。
//! 阅读 `test_detached_load_parquet` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_detached_load_parquet` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_detached_load_parquet`，应从这里理解职责边界。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `one_parquet_test.go`.

use astersql_tests_realtikvtest_importintotest::harness::{
    MockGCSSuite, fakestorage, gcs_endpoint, max_wait_time, proto, reset_engine, serial_guard,
    testkit,
};
use std::time::Duration;

const PARQUET_CONTENT: &[u8] = include_bytes!("test.parquet");

/// `TestDetachedLoadParquet`.
#[test]
fn test_detached_load_parquet() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.tk.MustExec("DROP DATABASE IF EXISTS load_csv;");
    s.tk.MustExec("CREATE DATABASE load_csv;");
    s.tk.MustExec("USE load_csv;");
    s.tk.MustExec(
        "CREATE TABLE t (id INT, val1 INT, val2 VARCHAR(20), \
         d1 DECIMAL(10, 0), d2 DECIMAL(10, 2), d3 DECIMAL(8, 8),\
         d4 DECIMAL(20, 0), d5 DECIMAL(36, 0), d6 DECIMAL(28, 8));",
    );

    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "test-load-parquet".into(),
            Name: "p.parquet".into(),
        },
        Content: PARQUET_CONTENT.to_vec(),
    });
    let temp_dir = s.TempDir();
    let path = temp_dir.join("test.parquet");
    s.NoError(std::fs::write(&path, PARQUET_CONTENT).map_err(|e| e.to_string()));

    let expected = testkit::Rows(&[
        "1 1 0 123 1.23 0.00000001 1234567890 123 1.23000000",
        "2 123456 0 123456 9999.99 0.12345678 99999999999999999999 999999999999999999999999999999999999 99999999999999999999.99999999",
        "3 123456 0 -123456 -9999.99 -0.12340000 -99999999999999999999 -999999999999999999999999999999999999 -99999999999999999999.99999999",
        "4 1 0 123 1.23 0.00000001 1234567890 123 1.23000000",
        "5 123456 0 123456 9999.99 0.12345678 12345678901234567890 123456789012345678901234567890123456 99999999999999999999.99999999",
        "6 123456 0 -123456 -9999.99 -0.12340000 -12345678901234567890 -123456789012345678901234567890123456 -99999999999999999999.99999999",
    ]);

    for option in ["FORMAT 'parquet'", ""] {
        s.tk.MustQuery(&format!(
            "IMPORT INTO t FROM '{}' {};",
            path.display(),
            option
        ));
        s.tk.MustQuery("SELECT * FROM t;").Check(&expected);
        s.tk.MustExec("TRUNCATE TABLE t;");
    }

    s.tk.MustExec("TRUNCATE TABLE t;");
    let sql = format!(
        "IMPORT INTO t FROM 'gs://test-load-parquet/p.parquet?endpoint={}' FORMAT 'parquet' WITH detached;",
        gcs_endpoint()
    );
    let rows = s.tk.MustQuery(&sql).Rows();
    s.Len(&rows, 1);
    let job_id: i64 = rows[0][0].parse().expect("job id");
    s.Eventually(
        || s.get_task_by_job_id((), job_id).State == proto::TaskStateSucceed,
        max_wait_time(),
        Duration::from_secs(1),
    );
    s.tk.MustQuery("SELECT * FROM t;").Check(&expected);
    s.tk.MustExec("TRUNCATE TABLE t;");
    s.tear_down();
}
