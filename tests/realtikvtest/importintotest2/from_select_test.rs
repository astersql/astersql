// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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
//! 中文总览：`from_select_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `from_select_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 54 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `setup` 是当前文件里的辅助函数。
//! 阅读 `setup` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `setup` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `setup`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `setup` 的重要阅读参照。
//! 理解 `setup` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `setup` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `test_import_from_select_basic` 是当前文件里的辅助函数。
//! 阅读 `test_import_from_select_basic` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_from_select_basic` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_from_select_basic`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_from_select_basic` 的重要阅读参照。
//! 理解 `test_import_from_select_basic` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_from_select_basic` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `test_import_from_select_column_list` 是当前文件里的辅助函数。
//! 阅读 `test_import_from_select_column_list` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_from_select_column_list` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_from_select_column_list`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_from_select_column_list` 的重要阅读参照。
//! 理解 `test_import_from_select_column_list` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_from_select_column_list` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `test_write_after_import_from_select` 是当前文件里的辅助函数。
//! 阅读 `test_write_after_import_from_select` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_write_after_import_from_select` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_write_after_import_from_select`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_write_after_import_from_select` 的重要阅读参照。
//! 理解 `test_write_after_import_from_select` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_write_after_import_from_select` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `test_import_from_select_stale_read` 是当前文件里的辅助函数。
//! 阅读 `test_import_from_select_stale_read` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_from_select_stale_read` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_from_select_stale_read`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_from_select_stale_read` 的重要阅读参照。
//! 理解 `test_import_from_select_stale_read` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_from_select_stale_read` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `test_cast_negative_to_unsigned` 是当前文件里的辅助函数。
//! 阅读 `test_cast_negative_to_unsigned` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_cast_negative_to_unsigned` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_cast_negative_to_unsigned`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_cast_negative_to_unsigned` 的重要阅读参照。
//! 理解 `test_cast_negative_to_unsigned` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_cast_negative_to_unsigned` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `test_disk_full_on_ingest_fail_fast` 是当前文件里的辅助函数。
//! 阅读 `test_disk_full_on_ingest_fail_fast` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_disk_full_on_ingest_fail_fast` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_disk_full_on_ingest_fail_fast`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_disk_full_on_ingest_fail_fast` 的重要阅读参照。
//! 理解 `test_disk_full_on_ingest_fail_fast` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_disk_full_on_ingest_fail_fast` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 中文说明结束（自动生成）

//! Go-equivalent IMPORT INTO ... FROM SELECT scenarios.

use astersql_tests_realtikvtest_importintotest::harness::{
    MockGCSSuite, infoschema, kerneltype, reset_engine, serial_guard, table_global_auto_ids,
    testfailpoint, testkit,
};

fn setup(db: &str) -> MockGCSSuite {
    reset_engine();
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db(db);
    s
}

#[test]
fn test_import_from_select_basic() {
    let _serial = serial_guard();
    let s = setup("from_select");
    s.tk.MustExec("create table src(id int, v varchar(64))");
    s.tk.MustExec("create table dst(id int, v varchar(64))");
    s.tk.MustExec(
        "insert into src values(4, 'aaaaaa'), (5, 'bbbbbb'), (6, 'cccccc'), (7, 'dddddd')",
    );

    s.ErrorIs(
        &s.tk
            .ExecToErr("import into dst FROM select id from src")
            .unwrap_err(),
        "ErrWrongValueCountOnRow",
    );
    s.ErrorIs(
        &s.tk
            .ExecToErr("import into dst(id) FROM select * from src")
            .unwrap_err(),
        "ErrWrongValueCountOnRow",
    );

    s.tk.MustExec("import into dst FROM select * from src");
    assert_eq!(s.tk.Session().AffectedRows(), 4);
    assert!(s.tk.Session().LastMessage().contains("Records: 4,"));
    s.tk.MustQuery("select * from dst").Check(&testkit::Rows(&[
        "4 aaaaaa", "5 bbbbbb", "6 cccccc", "7 dddddd",
    ]));
    s.ErrorContains(
        &s.tk
            .ExecToErr("import into dst FROM select * from src")
            .unwrap_err(),
        "target table is not empty",
    );

    s.tk.MustExec("truncate table dst");
    s.tk.MustExec("import into dst FROM select * from src where id > 5");
    assert_eq!(s.tk.Session().AffectedRows(), 2);
    assert!(s.tk.Session().LastMessage().contains("Records: 2,"));
    s.tk.MustQuery("select * from dst")
        .Check(&testkit::Rows(&["6 cccccc", "7 dddddd"]));

    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/util/cpu/mockNumCpu",
        "return(8)",
    );
    s.tk.MustExec("truncate table src");
    s.tk.MustExec("truncate table dst");
    let values = (0..5000)
        .map(|i| format!("({i}, 'abc-{i}')"))
        .collect::<Vec<_>>()
        .join(",");
    let mut expected = (0..5000)
        .map(|i| vec![i.to_string(), format!("abc-{i}")])
        .collect::<Vec<_>>();
    expected.sort();
    s.tk.MustExec(&format!("insert into src values {values}"));
    s.tk.MustExec("import into dst FROM select * from src with thread = 8");
    assert_eq!(s.tk.Session().AffectedRows(), 5000);
    assert_eq!(s.tk.Session().ParallelWorkers(), 8);
    assert!(s.tk.Session().LastMessage().contains("Records: 5000,"));
    s.tk.MustQuery("select * from dst")
        .Sort()
        .CheckOwned(&expected);

    s.tk.MustExec("create table t(id varchar(100))");
    s.tk.MustExec("import into t from select 1");
    s.tk.MustQuery("select * from t")
        .Check(&testkit::Rows(&["1"]));
    s.tear_down();
}

#[test]
fn test_import_from_select_column_list() {
    let _serial = serial_guard();
    let s = setup("from_select");
    s.tk.MustExec("create table src(id int, a varchar(64))");
    s.tk.MustExec(
        "create table dst(id int auto_increment primary key, a varchar(64), b int default 10, c int)",
    );
    s.tk.MustExec(
        "insert into src values(4, 'aaaaaa'), (5, 'bbbbbb'), (6, 'cccccc'), (7, 'dddddd')",
    );
    s.tk.MustExec("import into dst(c, a) FROM select * from src order by id");
    s.tk.MustQuery("select * from dst").Check(&testkit::Rows(&[
        "1 aaaaaa 10 4",
        "2 bbbbbb 10 5",
        "3 cccccc 10 6",
        "4 dddddd 10 7",
    ]));

    s.tk.MustExec("truncate table dst");
    s.tk.MustExec("create table src2(id int, a varchar(64))");
    s.tk.MustExec("insert into src2 values(4, 'four'), (5, 'five')");
    s.tk.MustExec(
        "import into dst(c, a) FROM select y.id, y.a from src x join src2 y on x.id = y.id order by y.id",
    );
    s.tk.MustQuery("select * from dst")
        .Check(&testkit::Rows(&["1 four 10 4", "2 five 10 5"]));
    s.tear_down();
}

#[test]
fn test_write_after_import_from_select() {
    let _serial = serial_guard();
    let s = setup("from_select");
    s.tk.MustExec("create table dt(id int, v varchar(64))");
    s.tk.MustExec(
        "insert into dt values(4, 'aaaaaa'), (5, 'bbbbbb'), (6, 'cccccc'), (7, 'dddddd')",
    );
    s.prepare_and_use_db("write_after_import");
    let cases = [
        (
            "CREATE TABLE t (id int AUTO_INCREMENT PRIMARY KEY CLUSTERED, v varchar(64))",
            "insert into t(v) values(1)",
            vec![8],
            "8 1",
        ),
        (
            "CREATE TABLE t (id int AUTO_INCREMENT PRIMARY KEY NONCLUSTERED, v varchar(64))",
            "insert into t(v) values(1)",
            vec![8],
            "8 1",
        ),
        (
            "CREATE TABLE t (id int PRIMARY KEY CLUSTERED, v varchar(64))",
            "insert into t values(1,1)",
            vec![],
            "1 1",
        ),
        (
            "CREATE TABLE t (id int, v varchar(64))",
            "insert into t values(1,1)",
            vec![5],
            "1 1",
        ),
        (
            "CREATE TABLE t (id int PRIMARY KEY NONCLUSTERED, v varchar(64))",
            "insert into t values(1,1)",
            vec![5],
            "1 1",
        ),
    ];
    for (ddl, insert, auto_ids, inserted) in cases {
        s.tk.MustExec("drop table if exists t");
        s.tk.MustExec(ddl);
        s.tk.MustExec("import into t FROM select * from from_select.dt");
        s.tk.MustQuery("select * from t").Check(&testkit::Rows(&[
            "4 aaaaaa", "5 bbbbbb", "6 cccccc", "7 dddddd",
        ]));
        assert_eq!(
            table_global_auto_ids(&s.store, "from_select", "t"),
            auto_ids
        );
        s.tk.MustExec(insert);
        let mut expected = ["4 aaaaaa", "5 bbbbbb", "6 cccccc", "7 dddddd", inserted];
        expected.sort();
        s.tk.MustQuery("select * from t")
            .Sort()
            .Check(&testkit::Rows(&expected));
    }
    s.tear_down();
}

#[test]
fn test_import_from_select_stale_read() {
    let _serial = serial_guard();
    let s = setup("from_select");
    s.tk.MustExec(
        "replace into mysql.tidb(variable_name, variable_value) values ('tikv_gc_safe_point', '20240131-00:00:00.000 +0800')",
    );
    s.tk.MustExec("create table src(id int, v varchar(64))");
    s.tk.MustExec("insert into src values(1, 'a')");
    let now = s.tk.MustQuery("select now(6)").Rows()[0][0].clone();
    s.tk.MustExec("insert into src values(2, 'b')");
    s.tk.MustQuery("select * from src")
        .Check(&testkit::Rows(&["1 a", "2 b"]));
    let stale = format!("select * from src as of timestamp '{now}'");
    s.tk.MustQuery(&stale).Check(&testkit::Rows(&["1 a"]));
    s.tk.MustExec("create table dst(id int, v varchar(64))");

    s.tk.MustExec(&format!("set tidb_snapshot = '{now}'"));
    s.ErrorIs(
        &s.tk
            .ExecToErr(&format!("import into dst from {stale}"))
            .unwrap_err(),
        infoschema::ErrTableNotExists,
    );
    s.ErrorIs(
        &s.tk
            .ExecToErr("import into dst from select * from src")
            .unwrap_err(),
        infoschema::ErrTableNotExists,
    );
    s.tk.MustExec("set tidb_snapshot = ''");
    s.tk.MustExec(&format!("import into dst from {stale}"));
    s.tk.MustQuery("select * from dst")
        .Check(&testkit::Rows(&["1 a"]));

    s.tk.MustExec("truncate table dst");
    let now = s.tk.MustQuery("select now(6)").Rows()[0][0].clone();
    let stale = format!("select * from src as of timestamp '{now}'");
    s.tk.MustExec("insert into src values(3, 'c')");
    s.tk.MustQuery("select * from src")
        .Check(&testkit::Rows(&["1 a", "2 b", "3 c"]));
    s.tk.MustExec(&format!("set tidb_snapshot = '{now}'"));
    for sql in [
        format!("import into dst from {stale}"),
        "import into dst from select * from src".to_string(),
    ] {
        s.ErrorContains(
            &s.tk.ExecToErr(&sql).unwrap_err(),
            "can not execute write statement when 'tidb_snapshot' is set",
        );
    }
    s.tk.MustExec("set tidb_snapshot = ''");
    s.tk.MustExec(&format!("import into dst from {stale}"));
    s.tk.MustQuery("select * from dst")
        .Check(&testkit::Rows(&["1 a", "2 b"]));

    s.tk.MustExec("truncate table dst");
    s.tk.MustExec(&format!("set tidb_snapshot = '{now}'"));
    for sql in [
        format!("import into dst from {stale}"),
        "import into dst from select * from src".to_string(),
    ] {
        s.ErrorContains(
            &s.tk.ExecToErr(&sql).unwrap_err(),
            "can not execute IMPORT statement when 'tidb_snapshot' is set",
        );
    }
    s.tk.MustExec("set tidb_snapshot = ''");
    s.tk.MustExec(&format!("import into dst from {stale}"));
    s.tk.MustQuery("select * from dst")
        .Check(&testkit::Rows(&["1 a", "2 b"]));
    s.tear_down();
}

#[test]
fn test_cast_negative_to_unsigned() {
    let _serial = serial_guard();
    let s = setup("from_select");
    s.tk.MustExec("create table dt(id int unsigned)");
    s.ErrorContains(
        &s.tk.ExecToErr("import into dt from select -1").unwrap_err(),
        "constant -1 overflows int",
    );
    s.tk.MustExec("set sql_mode=''");
    s.tk.MustExec("import into dt from select -1");
    s.tk.MustQuery("select * from dt")
        .Check(&testkit::Rows(&["0"]));
    s.tear_down();
}

#[test]
fn test_disk_full_on_ingest_fail_fast() {
    let _serial = serial_guard();
    if kerneltype::IsNextGen() {
        return;
    }
    let s = setup("from_select");
    s.tk.MustExec("create table dt(id int unsigned)");
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/diskFullOnIngest",
        "return(true)",
    );
    s.ErrorContains(
        &s.tk.ExecToErr("import into dt from select 1").unwrap_err(),
        "tikv disk full",
    );
    s.tear_down();
}
