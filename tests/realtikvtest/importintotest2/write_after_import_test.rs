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
//! 中文总览：`write_after_import_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `write_after_import_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 30 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `Case` 是当前文件里的状态类型。
//! 阅读 `Case` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `Case` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `Case`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `Case` 的重要阅读参照。
//! 理解 `Case` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `Case` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `Case` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `CASES` 是当前文件里的常量。
//! 阅读 `CASES` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `CASES` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `CASES`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `CASES` 的重要阅读参照。
//! 理解 `CASES` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `CASES` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `CASES` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_write_after_import_from_file` 是当前文件里的辅助函数。
//! 阅读 `test_write_after_import_from_file` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_write_after_import_from_file` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_write_after_import_from_file`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_write_after_import_from_file` 的重要阅读参照。
//! 理解 `test_write_after_import_from_file` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_write_after_import_from_file` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_write_after_import_from_file` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 中文说明结束（自动生成）

//! Go-equivalent table-driven write-after-file-import coverage.

use astersql_tests_realtikvtest_importintotest::harness::{
    MockGCSSuite, fakestorage, gcs_endpoint, reset_engine, serial_guard, table_global_auto_ids,
    testkit,
};

struct Case {
    ddl: &'static str,
    insert: &'static str,
    file_inserted: &'static str,
    file_auto_ids: &'static [i64],
    auto_id_cache_1: bool,
}

const CASES: &[Case] = &[
    Case {
        ddl: "CREATE TABLE t (id int AUTO_INCREMENT PRIMARY KEY CLUSTERED, v varchar(64))",
        insert: "insert into t(v) values(1)",
        file_inserted: "8 1",
        file_auto_ids: &[8],
        auto_id_cache_1: false,
    },
    Case {
        ddl: "CREATE TABLE t (id int AUTO_INCREMENT PRIMARY KEY CLUSTERED, v varchar(64)) AUTO_ID_CACHE 1",
        insert: "insert into t(v) values(1)",
        file_inserted: "8 1",
        file_auto_ids: &[8, 1],
        auto_id_cache_1: true,
    },
    Case {
        ddl: "CREATE TABLE t (id int AUTO_INCREMENT PRIMARY KEY NONCLUSTERED, v varchar(64))",
        insert: "insert into t(v) values(1)",
        file_inserted: "12 1",
        file_auto_ids: &[12],
        auto_id_cache_1: false,
    },
    Case {
        ddl: "CREATE TABLE t (id int AUTO_INCREMENT PRIMARY KEY NONCLUSTERED, v varchar(64)) AUTO_ID_CACHE 1",
        insert: "insert into t(v) values(1)",
        file_inserted: "12 1",
        file_auto_ids: &[8, 12],
        auto_id_cache_1: true,
    },
    Case {
        ddl: "CREATE TABLE t (id int PRIMARY KEY CLUSTERED, v varchar(64))",
        insert: "insert into t values(1,1)",
        file_inserted: "1 1",
        file_auto_ids: &[],
        auto_id_cache_1: false,
    },
    Case {
        ddl: "CREATE TABLE t (id int PRIMARY KEY CLUSTERED, v varchar(64)) AUTO_ID_CACHE 1",
        insert: "insert into t values(1,1)",
        file_inserted: "1 1",
        file_auto_ids: &[],
        auto_id_cache_1: true,
    },
    Case {
        ddl: "CREATE TABLE t (id int, v varchar(64))",
        insert: "insert into t values(1,1)",
        file_inserted: "1 1",
        file_auto_ids: &[12],
        auto_id_cache_1: false,
    },
    Case {
        ddl: "CREATE TABLE t (id int, v varchar(64)) AUTO_ID_CACHE 1",
        insert: "insert into t values(1,1)",
        file_inserted: "1 1",
        file_auto_ids: &[12],
        auto_id_cache_1: true,
    },
    Case {
        ddl: "CREATE TABLE t (id int PRIMARY KEY NONCLUSTERED, v varchar(64))",
        insert: "insert into t values(1,1)",
        file_inserted: "1 1",
        file_auto_ids: &[12],
        auto_id_cache_1: false,
    },
    Case {
        ddl: "CREATE TABLE t (id int PRIMARY KEY NONCLUSTERED, v varchar(64)) AUTO_ID_CACHE 1",
        insert: "insert into t values(1,1)",
        file_inserted: "1 1",
        file_auto_ids: &[12],
        auto_id_cache_1: true,
    },
    Case {
        ddl: "CREATE TABLE t (id bigint PRIMARY KEY auto_random, v varchar(64))",
        insert: "insert into t(v) values(1)",
        file_inserted: "8 1",
        file_auto_ids: &[8],
        auto_id_cache_1: true,
    },
];

#[test]
fn test_write_after_import_from_file() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "write_after_import".into(),
            Name: "1.csv".into(),
        },
        Content: b"4,aaaaaa\n5,bbbbbb\n".to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "write_after_import".into(),
            Name: "2.csv".into(),
        },
        Content: b"6,cccccc\n7,dddddd\n".to_vec(),
    });
    let import_sql = format!(
        "import into t FROM 'gs://write_after_import/*.csv?endpoint={}'",
        gcs_endpoint()
    );
    s.prepare_and_use_db("write_after_import");

    let mut executed = 0;
    let mut go_skipped = 0;
    for case in CASES {
        if case.auto_id_cache_1 {
            go_skipped += 1;
            continue;
        }
        executed += 1;
        s.tk.MustExec("drop table if exists t");
        s.tk.MustExec(case.ddl);
        s.tk.MustQuery(&import_sql);
        s.tk.MustQuery("select * from t").Check(&testkit::Rows(&[
            "4 aaaaaa", "5 bbbbbb", "6 cccccc", "7 dddddd",
        ]));
        assert_eq!(
            table_global_auto_ids(&s.store, "write_after_import", "t"),
            case.file_auto_ids
        );
        s.tk.MustExec(case.insert);
        let mut expected = [
            "4 aaaaaa",
            "5 bbbbbb",
            "6 cccccc",
            "7 dddddd",
            case.file_inserted,
        ];
        expected.sort();
        s.tk.MustQuery("select * from t")
            .Sort()
            .Check(&testkit::Rows(&expected));
    }
    // The same six AUTO_ID_CACHE=1 cases are explicitly skipped by the Go suite.
    assert_eq!(executed, 5);
    assert_eq!(go_skipped, 6);
    s.tk.MustExec("drop table if exists t");
    s.tear_down();
}
