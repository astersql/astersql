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
//! 中文总览：`detach_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `detach_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 14 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `DetachedCase` 是当前文件里的状态类型。
//! 阅读 `DetachedCase` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `DetachedCase` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `DETACHED_CASES` 是当前文件里的常量。
//! 阅读 `DETACHED_CASES` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `DETACHED_CASES` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `test_same_behaviour_detached_or_not` 是当前文件里的辅助函数。
//! 阅读 `test_same_behaviour_detached_or_not` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_same_behaviour_detached_or_not` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `detach_test.go`.
//!
//! Mapping:
//! - `TestSameBehaviourDetachedOrNot` → [`test_same_behaviour_detached_or_not`]

use astersql_tests_realtikvtest_importintotest::harness::{
    MockGCSSuite, fakestorage, gcs_endpoint, max_wait_time, proto, reset_engine, serial_guard,
};
use std::time::Duration;

struct DetachedCase {
    table_cols: &'static str,
    physical_mode_data: &'static str,
}

const DETACHED_CASES: &[DetachedCase] = &[
    DetachedCase {
        table_cols: "(dt DATETIME, ts TIMESTAMP);",
        physical_mode_data: "2019-01-01 00:00:00,2019-01-01 00:00:00",
    },
    DetachedCase {
        table_cols: "(c INT NOT NULL, c2 TINYINT);",
        physical_mode_data: "1,100",
    },
];

/// `TestSameBehaviourDetachedOrNot`.
#[test]
fn test_same_behaviour_detached_or_not() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.tk.MustExec("SET SESSION TIME_ZONE = '+08:00';");
    for ca in DETACHED_CASES {
        s.tk.MustExec("DROP DATABASE IF EXISTS test_detached;");
        s.tk.MustExec("CREATE DATABASE test_detached;");
        s.tk.MustExec(&format!("CREATE TABLE test_detached.t1 {}", ca.table_cols));
        s.tk.MustExec(&format!("CREATE TABLE test_detached.t2 {}", ca.table_cols));

        s.server.CreateObject(fakestorage::Object {
            ObjectAttrs: fakestorage::ObjectAttrs {
                BucketName: "test-detached".into(),
                Name: "1.txt".into(),
            },
            Content: ca.physical_mode_data.as_bytes().to_vec(),
        });
        s.tk.MustQuery(&format!(
            "IMPORT INTO test_detached.t1 FROM 'gs://test-detached/1.txt?endpoint={}' WITH thread=1;",
            gcs_endpoint()
        ));
        let rows = s
            .tk
            .MustQuery(&format!(
                "IMPORT INTO test_detached.t2 FROM 'gs://test-detached/1.txt?endpoint={}' WITH DETACHED, thread=1;",
                gcs_endpoint()
            ))
            .Rows();
        s.Len(&rows, 1);
        let job_id: i64 = rows[0][0].parse().expect("job id");
        s.Eventually(
            || {
                let task = s.get_task_by_job_id((), job_id);
                task.State == proto::TaskStateSucceed
            },
            max_wait_time(),
            Duration::from_secs(1),
        );

        let r1 =
            s.tk.MustQuery("SELECT * FROM test_detached.t1")
                .Sort()
                .Rows();
        s.tk.MustQuery("SELECT * FROM test_detached.t2")
            .Sort()
            .CheckOwned(&r1);
    }
    s.tear_down();
}
