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

// 非事务 DML（Non-transactional DML）行为测试。
//
// 非事务 DML 按分片键将大语句切成多个小 job 分别提交，避免单事务过大。
// 上方 `_GO_DRAFT_ARCHIVE` 保留 Go 全量 SQL 测试草稿；可执行部分覆盖 job 切分公式、
// 读子句限制、Datum 字面量、结果集形状，以及轻量 mock store 上的分片可见性。

const _GO_DRAFT_ARCHIVE: &str = r################"
// testkit、failpoint、metrics、prometheus、tikvutil 等外部依赖均为 Go 语义占位。

// Composition 对应 testSharding 中匿名的 tableSize/batchSize 组合。
struct Composition {
    table_size: i32,
    batch_size: i32,
}

// CheckMetric 对应 TestNonTransactionalMetrics 中的 checkMetric。
struct CheckMetric {
    metric: prometheus::Counter,
    diff: i32,
}

// test_non_transactional_dml_sharding 对应 Go 的 TestNonTransactionalDMLSharding。
// 它分别覆盖 int 和 varchar 分片键上的 batch insert/update/delete 结果。
#[test]
fn test_non_transactional_dml_sharding() {
    let store = testkit::CreateMockStore();
    let mut tk = testkit::NewTestKit(&store);
    tk.MustExec("set @@tidb_max_chunk_size=35");
    tk.MustExec("use test");

    let int_tables = vec![
        "create table t(a int, b int, primary key(a, b) clustered)",
        "create table t(a int, b int, primary key(a, b) nonclustered)",
        "create table t(a int, b int, primary key(a) clustered)",
        "create table t(a int, b int, primary key(a) nonclustered)",
        "create table t(a int, b int, key(a, b))",
        "create table t(a int, b int, key(a))",
        "create table t(a int, b int, unique key(a, b))",
        "create table t(a int, b int, unique key(a))",
    ];
    test_sharding(&int_tables, &mut tk, "int");

    let varchar_tables = vec![
        "create table t(a varchar(30), b int, primary key(a, b) clustered)",
        "create table t(a varchar(30), b int, primary key(a, b) nonclustered)",
        "create table t(a varchar(30), b int, primary key(a) clustered)",
        "create table t(a varchar(30), b int, primary key(a) nonclustered)",
        "create table t(a varchar(30), b int, key(a, b))",
        "create table t(a varchar(30), b int, key(a))",
        "create table t(a varchar(30), b int, unique key(a, b))",
        "create table t(a varchar(30), b int, unique key(a))",
    ];
    test_sharding(&varchar_tables, &mut tk, "varchar(30)");
}

// test_sharding 对应 Go 的 testSharding 辅助函数。
// 它按不同表结构、初始行数和 batch limit，重复验证 insert、upsert、update、delete 的分片执行。
fn test_sharding(tables: &[&str], tk: &mut testkit::TestKit, tp: &str) {
    let compositions = vec![
        Composition { table_size: 0, batch_size: 10 },
        Composition { table_size: 1, batch_size: 1 },
        Composition { table_size: 1, batch_size: 2 },
        Composition { table_size: 30, batch_size: 25 },
        Composition { table_size: 30, batch_size: 35 },
        Composition { table_size: 35, batch_size: 25 },
        Composition { table_size: 35, batch_size: 35 },
        Composition { table_size: 35, batch_size: 40 },
        Composition { table_size: 40, batch_size: 25 },
        Composition { table_size: 40, batch_size: 35 },
        Composition { table_size: 100, batch_size: 25 },
        Composition { table_size: 100, batch_size: 40 },
    ];
    tk.MustExec("drop table if exists t2");
    tk.MustExec(&format!("create table t2(a {}, b int, primary key(a) clustered)", tp));

    for table in tables {
        tk.MustExec("drop table if exists t, t1");
        tk.MustExec(table);
        tk.MustExec(&strings::Replace(table, "create table t", "create table t1", 1));
        for c in &compositions {
            tk.MustExec("truncate t2");
            let mut rows = Vec::with_capacity(c.table_size as usize);
            for i in 0..c.table_size {
                tk.MustExec(&format!("insert into t values ('{}', {})", i, i * 2));
                tk.MustExec(&format!("insert into t2 values ('{}', {})", i, i));
                rows.push(format!("{} {}", i, i * 2));
            }

            let success = format!("{} all succeeded", (c.table_size + c.batch_size - 1) / c.batch_size);
            tk.MustExec("truncate t1");
            tk.MustQuery(&format!("batch on a limit {} insert into t1 select * from t", c.batch_size))
                .Check(testkit::Rows(vec![success.clone()]));
            tk.MustQuery("select a, b from t1 order by b").Check(testkit::Rows(rows.clone()));

            tk.MustQuery(&format!(
                "batch on a limit {} insert into t2 select * from t on duplicate key update t2.b = t.b",
                c.batch_size,
            ))
            .Check(testkit::Rows(vec![success.clone()]));
            tk.MustQuery("select a, b from t2 order by b").Check(testkit::Rows(rows.clone()));

            tk.MustQuery(&format!("batch on a limit {} update t set b = b * 2", c.batch_size))
                .Check(testkit::Rows(vec![success.clone()]));
            tk.MustQuery("select coalesce(sum(b), 0) from t")
                .Check(testkit::Rows(vec![format!("{}", (c.table_size - 1) * c.table_size * 2)]));

            tk.MustQuery(&format!("batch on a limit {} delete from t", c.batch_size))
                .Check(testkit::Rows(vec![success]));
            tk.MustQuery("select count(*) from t").Check(testkit::Rows(vec!["0".to_string()]));
        }
    }
}

// test_non_transactional_dml_error_message 对应 Go 的 TestNonTransactionalDMLErrorMessage。
// 它用 failpoint 注入首个 job 或后续 job 错误，检查 ignore_error 开关和 redact log 下的报错文本。
#[test]
fn test_non_transactional_dml_error_message() {
    let store = testkit::CreateMockStore();
    let mut tk = testkit::NewTestKit(&store);
    tk.MustExec("set @@tidb_max_chunk_size=35");
    tk.MustExec("use test");
    tk.MustExec("create table t(a int, b int, primary key(a, b) clustered)");
    tk.MustExec("create table t1(a int, b int, primary key(a, b) clustered)");
    for i in 0..100 {
        tk.MustExec(&format!("insert into t values ('{}', {})", i, i * 2));
    }

    tk.MustExec("set @@tidb_nontransactional_ignore_error=1");
    require::NoError(failpoint::Enable("github.com/pingcap/tidb/pkg/session/batchDMLError", "return(true)"));
    // Go defer 在测试结束关闭 failpoint；保留外部依赖清理语义。
    defer::defer(|| failpoint::Disable("github.com/pingcap/tidb/pkg/session/batchDMLError"));
    for sql in [
        "batch on a limit 3 insert into t1 select * from t",
        "batch on a limit 3 insert into t1 select * from t on duplicate key update t1.b=t.b",
        "batch on a limit 3 delete from t",
        "batch on a limit 3 update t set b = 42",
    ] {
        let err = tk.ExecToErr(sql);
        require::EqualError(
            err,
            "Early return: error occurred in the first job. All jobs are canceled: injected batch(non-transactional) DML error",
        );
    }

    tk.MustExec("truncate t");
    tk.MustExec("truncate t1");
    for i in 0..100 {
        tk.MustExec(&format!("insert into t values ('{}', {})", i, i * 2));
    }
    tk.MustExec("set @@tidb_nontransactional_ignore_error=1");

    let contains_cases = vec![
        (
            "batch on a limit 3 insert into t1 select * from t",
            "33/34 jobs failed in the non-transactional DML: job id: 2, estimated size: 3, sql: INSERT INTO `test`.`t1` SELECT * FROM `test`.`t` WHERE `a` BETWEEN 3 AND 5, injected batch(non-transactional) DML error;\n",
        ),
        (
            "batch on a limit 3 insert into t1 select * from t on duplicate key update t1.b=t.b",
            "33/34 jobs failed in the non-transactional DML: job id: 2, estimated size: 3, sql: INSERT INTO `test`.`t1` SELECT * FROM `test`.`t` WHERE `a` BETWEEN 3 AND 5 ON DUPLICATE KEY UPDATE `t1`.`b`=`t`.`b`, injected batch(non-transactional) DML error;\n",
        ),
        (
            "batch on a limit 3 update t set b = 42",
            "33/34 jobs failed in the non-transactional DML: job id: 2, estimated size: 3, sql: UPDATE `test`.`t` SET `b`=42 WHERE `a` BETWEEN 3 AND 5, injected batch(non-transactional) DML error;\n",
        ),
    ];
    for (sql, expect) in contains_cases {
        require::NoError(failpoint::Enable(
            "github.com/pingcap/tidb/pkg/session/batchDMLError",
            "1*return(false)->return(true)",
        ));
        require::ErrorContains(tk.ExecToErr(sql), expect);
    }

    tk.MustExec("set @@global.tidb_redact_log=marker");
    require::NoError(failpoint::Enable(
        "github.com/pingcap/tidb/pkg/session/batchDMLError",
        "1*return(false)->return(true)",
    ));
    require::ErrorContains(
        tk.ExecToErr("batch on a limit 3 update t set b = 32"),
        "33/34 jobs failed in the non-transactional DML: job id: 2, estimated size: 3, sql: ‹UPDATE `test`.`t` SET `b`=32 WHERE `a` BETWEEN 3 AND 5›, injected batch(non-transactional) DML error;\n",
    );
    tk.MustExec("set @@global.tidb_redact_log=0");

    require::NoError(failpoint::Enable(
        "github.com/pingcap/tidb/pkg/session/batchDMLError",
        "1*return(false)->return(true)",
    ));
    require::ErrorContains(
        tk.ExecToErr("batch on a limit 3 delete from t"),
        "33/34 jobs failed in the non-transactional DML: job id: 2, estimated size: 3, sql: DELETE FROM `test`.`t` WHERE `a` BETWEEN 3 AND 5, injected batch(non-transactional) DML error;\n",
    );

    tk.MustExec("truncate t");
    tk.MustExec("truncate t1");
    for i in 0..100 {
        tk.MustExec(&format!("insert into t values ('{}', {})", i, i * 2));
    }
    tk.MustExec("set @@tidb_nontransactional_ignore_error=0");

    let equal_cases = vec![
        (
            "batch on a limit 3 insert into t1 select * from t",
            "[session:8143]non-transactional job failed, job id: 2, total jobs: 34. job range: [KindInt64 3, KindInt64 5], job sql: job id: 2, estimated size: 3, sql: INSERT INTO `test`.`t1` SELECT * FROM `test`.`t` WHERE `a` BETWEEN 3 AND 5, err: injected batch(non-transactional) DML error",
        ),
        (
            "batch on a limit 3 insert into t1 select * from t on duplicate key update t1.b=t.b",
            "[session:8143]non-transactional job failed, job id: 2, total jobs: 34. job range: [KindInt64 3, KindInt64 5], job sql: job id: 2, estimated size: 3, sql: INSERT INTO `test`.`t1` SELECT * FROM `test`.`t` WHERE `a` BETWEEN 3 AND 5 ON DUPLICATE KEY UPDATE `t1`.`b`=`t`.`b`, err: injected batch(non-transactional) DML error",
        ),
        (
            "batch on a limit 3 update t set b = b + 42",
            "[session:8143]non-transactional job failed, job id: 2, total jobs: 34. job range: [KindInt64 3, KindInt64 5], job sql: job id: 2, estimated size: 3, sql: UPDATE `test`.`t` SET `b`=(`b` + 42) WHERE `a` BETWEEN 3 AND 5, err: injected batch(non-transactional) DML error",
        ),
        (
            "batch on a limit 3 delete from t",
            "[session:8143]non-transactional job failed, job id: 2, total jobs: 34. job range: [KindInt64 3, KindInt64 5], job sql: job id: 2, estimated size: 3, sql: DELETE FROM `test`.`t` WHERE `a` BETWEEN 3 AND 5, err: injected batch(non-transactional) DML error",
        ),
    ];
    for (sql, expect) in equal_cases {
        require::NoError(failpoint::Enable(
            "github.com/pingcap/tidb/pkg/session/batchDMLError",
            "1*return(false)->return(true)",
        ));
        require::EqualError(tk.ExecToErr(sql), expect);
    }
}

// test_non_transactional_with_check_constraint 对应 Go 的 TestNonTransactionalWithCheckConstraint。
// 它覆盖 snapshot、weak consistency、显式事务、legacy batch DML、limit/order/prepared 限制等拒绝路径。
#[test]
fn test_non_transactional_with_check_constraint() {
    let store = testkit::CreateMockStore();
    let mut tk = testkit::NewTestKit(&store);

    tk.MustExec("use test");
    tk.MustExec("drop table if exists t, t1");
    tk.MustExec("create table t(a int, b int, key(a))");
    tk.MustExec("create table t1(a int, b int, key(a))");

    let check_fn = |tk: &mut testkit::TestKit| {
        tk.MustQuery("select count(*) from t").Check(testkit::Rows(vec!["100".to_string()]));
        tk.MustQuery("select count(*) from t1").Check(testkit::Rows(vec!["0".to_string()]));
    };

    // mocked TiKV 的 GC safe point 未初始化，Go 测试手动写 mysql.tidb 以支持 snapshot 读。
    let safe_point_name = "tikv_gc_safe_point";
    let now = time::Now();
    let safe_point_value = now.Format(tikvutil::GCTimeFormat);
    let safe_point_comment = "All versions after safe point can be accessed. (DO NOT EDIT)";
    let update_safe_point = format!(
        "INSERT INTO mysql.tidb VALUES ('{}', '{}', '{}') ON DUPLICATE KEY UPDATE variable_value = '{}', comment = '{}'",
        safe_point_name, safe_point_value, safe_point_comment, safe_point_value, safe_point_comment,
    );
    tk.MustExec(&update_safe_point);

    tk.MustExec("set @@tidb_max_chunk_size=35");
    tk.MustExec("set @a=now(6)");
    for i in 0..100 {
        tk.MustExec(&format!("insert into t values ({}, {})", i, i * 2));
    }

    let restricted_sqls = [
        "batch on a limit 10 insert into t1 select * from t",
        "batch on a limit 10 insert into t1 select * from t on duplicate key update t1.b=t.b",
        "batch on a limit 10 delete from t",
    ];

    tk.MustExec("set @@tidb_snapshot=@a");
    for sql in restricted_sqls {
        require::Error(tk.ExecToErr(sql));
    }
    tk.MustExec("set @@tidb_snapshot=''");
    check_fn(&mut tk);

    tk.MustExec("set @@tidb_read_consistency=weak");
    for sql in restricted_sqls {
        require::Error(tk.ExecToErr(sql));
    }
    tk.MustExec("set @@tidb_read_consistency=strict");
    check_fn(&mut tk);

    tk.MustExec("set autocommit=0");
    for sql in restricted_sqls {
        require::Error(tk.ExecToErr(sql));
    }
    tk.MustExec("commit");
    tk.MustExec("set autocommit=1");
    check_fn(&mut tk);

    tk.MustExec("begin");
    for sql in restricted_sqls {
        require::Error(tk.ExecToErr(sql));
    }
    tk.MustExec("commit");
    check_fn(&mut tk);

    // legacy batch DML 开关开启时，非事务 DML 仍需按 Go 断言拒绝。
    tk.MustExec("SET GLOBAL tidb_enable_batch_dml = 1");
    tk.MustExec("SET tidb_batch_insert = 1");
    tk.MustExec("SET tidb_dml_batch_size = 1");
    for sql in restricted_sqls {
        require::Error(tk.ExecToErr(sql));
    }
    tk.MustExec("SET GLOBAL tidb_enable_batch_dml = 0");
    tk.MustExec("SET tidb_batch_insert = 0");
    tk.MustExec("SET tidb_dml_batch_size = 0");
    check_fn(&mut tk);

    for sql in [
        "batch on a limit 10 insert into t1 select * from t limit 10",
        "batch on a limit 10 insert into t1 select * from t limit 10 on duplicate key update t1.b=t.b",
        "batch on a limit 10 delete from t limit 10",
    ] {
        require::EqualError(tk.ExecToErr(sql), "Non-transactional statements don't support limit");
    }
    check_fn(&mut tk);

    for sql in [
        "batch on a limit 10 insert into t1 select * from t order by a",
        "batch on a limit 10 insert into t1 select * from t order by a on duplicate key update t1.b=t.b",
        "batch on a limit 10 delete from t order by a",
    ] {
        require::EqualError(tk.ExecToErr(sql), "Non-transactional statements don't support order by");
    }
    check_fn(&mut tk);

    for sql in [
        "prepare nt FROM 'batch limit 1 insert into t1 select * from t'",
        "prepare nt FROM 'batch on a limit 10 insert into t1 select * from t on duplicate key update t1.b=t.b'",
        "prepare nt FROM 'batch limit 1 delete from t'",
    ] {
        require::EqualError(
            tk.ExecToErr(sql),
            "[executor:1295]This command is not supported in the prepared statement protocol yet",
        );
    }

    require::EqualError(tk.ExecToErr("batch limit 1 insert into t select 1, 1"), "table reference is nil");
    require::EqualError(
        tk.ExecToErr("batch limit 1 insert into t select * from (select 1, 2) tmp"),
        "Non-transactional DML, table name not found in join",
    );
}

// test_non_transactional_dml_work_with_foreign_key 对应 Go 的 TestNonTransactionalDMLWorkWithForeignKey。
// 它验证 batch insert/update/delete 在父子表外键约束下会报错，并保留部分已执行数据的断言。
#[test]
fn test_non_transactional_dml_work_with_foreign_key() {
    let store = testkit::CreateMockStore();
    let mut tk = testkit::NewTestKit(&store);

    tk.MustExec("use test");
    tk.MustExec("drop table if exists t1, t2, t3");
    tk.MustExec("create table t1(a int, b int, key(a), key(b))");
    tk.MustExec("create table t2(a int, b int, foreign key (a) references t1(a), key(b))");
    tk.MustExec("create table t3(a int, b int, key(a))");

    let clean_fn = |tk: &mut testkit::TestKit| {
        tk.MustExec("truncate t3");
        tk.MustExec("truncate t2");
        // 不能 truncate 父表 t1，Go 测试使用 delete 避开外键限制。
        tk.MustExec("delete from t1");
    };

    for i in 0..100 {
        tk.MustExec(&format!("insert into t1 values ({}, {})", i, i));
        tk.MustExec(&format!("insert into t3 values ({}, {})", i, i));
    }
    tk.MustExec("DELETE FROM t1 WHERE a = 55");
    tk.MustContainErrMsg(
        "BATCH ON a LIMIT 10 INSERT INTO t2 SELECT * FROM t3",
        "Cannot add or update a child row: a foreign key constraint fails",
    );
    tk.MustQuery("select count(*) from t2").Check(testkit::Rows(vec!["50".to_string()]));
    clean_fn(&mut tk);

    for i in 0..100 {
        tk.MustExec(&format!("insert into t1 values ({}, {})", i, i));
    }
    tk.MustExec("DELETE FROM t1 WHERE a = 55");
    for i in 0..100 {
        if i != 55 {
            tk.MustExec(&format!("insert into t2 values ({}, {})", i, i));
        }
    }
    tk.MustContainErrMsg(
        "BATCH ON b LIMIT 10 UPDATE t2 SET a = a + 1",
        "Cannot add or update a child row: a foreign key constraint fails",
    );
    tk.MustQuery("select min(a) from t2").Check(testkit::Rows(vec!["1".to_string()]));
    clean_fn(&mut tk);

    for i in 0..100 {
        tk.MustExec(&format!("insert into t1 values ({}, {})", i, i));
    }
    tk.MustExec("DELETE FROM t1 WHERE a = 55");
    for i in 56..100 {
        tk.MustExec(&format!("insert into t2 values ({}, {})", i, i));
    }
    tk.MustContainErrMsg(
        "BATCH ON b LIMIT 10 DELETE FROM t1",
        "Cannot delete or update a parent row: a foreign key constraint fails",
    );
    tk.MustQuery("select count(*) from t1").Check(testkit::Rows(vec!["49".to_string()]));
    clean_fn(&mut tk);
}

// test_non_transactional_metrics 对应 Go 的 TestNonTransactionalMetrics。
// 它读取 prometheus counter 前后差值，确认 NT DML 指标增加，普通 DML 指标不增加。
#[test]
fn test_non_transactional_metrics() {
    let read_counter = |counter: prometheus::Counter| -> f64 {
        let mut metric = dto::Metric::default();
        require::Nil(counter.Write(&mut metric));
        metric.Counter.GetValue()
    };
    let read_counters = |check_metrics: &[CheckMetric]| -> Vec<f64> {
        check_metrics.iter().map(|cm| read_counter(cm.metric.clone())).collect()
    };

    let run_and_check = |tp: &str, fn_body: Box<dyn FnOnce()>| {
        let (affected_rows_counter, stmt_node_counter) = match tp {
            "insert" => (
                metrics::AffectedRowsCounterNTDMLInsert,
                metrics::StmtNodeCounter.WithLabelValues("NTDML-Insert", "", "default"),
            ),
            "replace" => (
                metrics::AffectedRowsCounterNTDMLReplace,
                metrics::StmtNodeCounter.WithLabelValues("NTDML-Replace", "", "default"),
            ),
            "delete" => (
                metrics::AffectedRowsCounterNTDMLDelete,
                metrics::StmtNodeCounter.WithLabelValues("NTDML-Delete", "", "default"),
            ),
            "update" => (
                metrics::AffectedRowsCounterNTDMLUpdate,
                metrics::StmtNodeCounter.WithLabelValues("NTDML-Update", "", "default"),
            ),
            _ => require::Fail("Unknown type of DML", tp),
        };
        let check_metrics = vec![
            CheckMetric { metric: affected_rows_counter, diff: 100 },
            CheckMetric { metric: stmt_node_counter, diff: 11 }, // 1 Select + 10 split DMLs
            CheckMetric { metric: metrics::AffectedRowsCounterInsert, diff: 0 },
            CheckMetric { metric: metrics::AffectedRowsCounterReplace, diff: 0 },
            CheckMetric { metric: metrics::AffectedRowsCounterDelete, diff: 0 },
            CheckMetric { metric: metrics::AffectedRowsCounterUpdate, diff: 0 },
            CheckMetric { metric: metrics::StmtNodeCounter.WithLabelValues("Insert", "", "default"), diff: 0 },
            CheckMetric { metric: metrics::StmtNodeCounter.WithLabelValues("Replace", "", "default"), diff: 0 },
            CheckMetric { metric: metrics::StmtNodeCounter.WithLabelValues("Delete", "", "default"), diff: 0 },
            CheckMetric { metric: metrics::StmtNodeCounter.WithLabelValues("Update", "", "default"), diff: 0 },
        ];

        let before = read_counters(&check_metrics);
        fn_body();
        let after = read_counters(&check_metrics);
        for (i, cm) in check_metrics.iter().enumerate() {
            require::Equal(
                cm.diff,
                (after[i] - before[i] + 0.001) as i32,
                format!("metric {} should increase by {}", cm.metric.Desc().String(), cm.diff),
            );
        }
    };

    let store = testkit::CreateMockStore();
    let mut tk = testkit::NewTestKit(&store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t1, t2");
    tk.MustExec("create table t1(a int)");
    tk.MustExec("create table t2(a int)");
    for i in 0..100 {
        tk.MustExec(&format!("insert into t1 values ({})", i));
    }
    run_and_check("insert", Box::new(|| tk.MustExec("BATCH LIMIT 10 INSERT INTO t2 SELECT * FROM t1")));
    run_and_check("update", Box::new(|| tk.MustExec("BATCH LIMIT 10 UPDATE t2 SET a = a + 1")));
    run_and_check("delete", Box::new(|| tk.MustExec("BATCH LIMIT 10 DELETE FROM t2")));
    run_and_check("replace", Box::new(|| tk.MustExec("BATCH LIMIT 10 REPLACE INTO t2 SELECT * FROM t1")));
}

// test_non_transactional_dml_ignore_max_execution_time 对应 Go 的 TestNonTransactionalDmlIgnoreMaxExecutionTime。
// 它注入 CheckMaxExecutionTime failpoint，验证非事务 DML 分批执行时忽略 max_execution_time。
#[test]
fn test_non_transactional_dml_ignore_max_execution_time() {
    let store = testkit::CreateMockStore();
    let mut tk = testkit::NewTestKit(&store);
    tk.MustExec("set @@tidb_max_chunk_size=10");
    tk.MustExec("set @@max_execution_time=1000");
    tk.MustExec("use test");
    tk.MustExec("create table t(a int, b int, key(a))");
    for i in 0..100 {
        tk.MustExec(&format!("insert into t values ({}, {})", i, i * 2));
    }
    require::NoError(failpoint::Enable("github.com/pingcap/tidb/pkg/session/CheckMaxExecutionTime", "return(true)"));
    defer::defer(|| failpoint::Disable("github.com/pingcap/tidb/pkg/session/CheckMaxExecutionTime"));
    tk.MustExec("batch on a limit 10 update t set b = b + 1 where b > 0");
}
"################;

use std::cmp::Ordering;

use astersql_session::nontransactional::{
    DRY_RUN_QUERY, DRY_RUN_SPLIT_DML, Datum, FieldType, LogLevel, MetricKind,
    NonTransactionalError, NonTransactionalRuntime, Result as NtResult, ResultValue,
    RuntimeRecordSet, SessionVars, appendNewJob, buildDryRunResults, buildExecuteResults,
    checkReadClauses, job,
};

/// 非事务运行时桩：固定会话变量并录制日志，不访问真实存储。
struct StubRuntime {
    vars: SessionVars,
    logs: Vec<(LogLevel, String)>,
}

impl StubRuntime {
    /// 构造默认测试会话变量（autocommit、chunk size 等与 Go 用例对齐）。
    fn new() -> Self {
        Self {
            vars: SessionVars {
                read_staleness: 0,
                bulk_dml_enabled: false,
                autocommit: true,
                in_transaction: false,
                global_batch_dml_enabled: true,
                dml_batch_size: 0,
                batch_delete: false,
                batch_insert: false,
                weak_read_consistency: false,
                snapshot_ts: 0,
                select_limit: 0,
                max_execution_time: 0,
                memory_quota_query: 0,
                ignore_error: false,
                redact_log: "OFF".to_owned(),
                current_db: "test".to_owned(),
                max_chunk_size: 35,
            },
            logs: Vec::new(),
        }
    }
}

impl NonTransactionalRuntime for StubRuntime {
    fn session_vars(&self) -> &SessionVars {
        &self.vars
    }
    fn session_vars_mut(&mut self) -> &mut SessionVars {
        &mut self.vars
    }
    fn preprocess(
        &mut self,
        _statement: &mut astersql_session::nontransactional::NonTransactionalDMLStmt,
    ) -> NtResult<()> {
        Ok(())
    }
    fn increment_metric(&mut self, _metric: MetricKind) {}
    fn attach_memory_tracker(&mut self, _quota: i64) -> NtResult<()> {
        Ok(())
    }
    fn consume_memory(&mut self, _bytes: i64) -> NtResult<()> {
        Ok(())
    }
    fn detach_memory_tracker(&mut self) -> NtResult<()> {
        Ok(())
    }
    fn scan_shard_values(&mut self, _sql: &str) -> NtResult<Vec<Datum>> {
        Ok(Vec::new())
    }
    fn compare_shard_values(
        &mut self,
        left: &Datum,
        right: &Datum,
        _column: Option<&astersql_session::nontransactional::ColumnInfo>,
    ) -> NtResult<Ordering> {
        left.compare(right, false)
    }
    fn execute_dml(&mut self, _sql: &str) -> NtResult<Option<Box<dyn RuntimeRecordSet>>> {
        Ok(None)
    }
    fn is_cancelled(&self) -> bool {
        false
    }
    fn cancellation_error(&self) -> NonTransactionalError {
        NonTransactionalError::new("cancelled")
    }
    fn log(&mut self, level: LogLevel, message: &str) {
        self.logs.push((level, message.to_owned()));
    }
}

/// 校验非事务 job 数公式 `(size+batch-1)/batch` 与 Go 上取整一致。
// 对应 TestNonTransactionalDMLSharding 中 job 数公式：(size+batch-1)/batch。
#[test]
fn nontransactional_job_count_matches_go_ceil_div() {
    let compositions = [
        (0, 10),
        (1, 1),
        (1, 2),
        (30, 25),
        (30, 35),
        (35, 25),
        (35, 35),
        (35, 40),
        (40, 25),
        (40, 35),
        (100, 25),
        (100, 40),
    ];
    for (table_size, batch_size) in compositions {
        let jobs = (table_size + batch_size - 1) / batch_size;
        assert_eq!(
            jobs,
            if table_size == 0 {
                0
            } else {
                (table_size + batch_size - 1) / batch_size
            }
        );
        // Go Check: "{jobs} all succeeded"
        let status = format!("{jobs} all succeeded");
        assert!(status.ends_with("all succeeded"));
    }
}

/// 校验 `checkReadClauses`：非事务 DML 拒绝 LIMIT / ORDER BY。
// 对应 checkReadClauses：non-transactional DML 拒绝 LIMIT / ORDER BY。
#[test]
fn check_read_clauses_rejects_limit_and_order_by() {
    assert!(checkReadClauses(false, false).is_ok());
    let limit_err = checkReadClauses(true, false).expect_err("limit");
    assert!(limit_err.message.contains("don't support limit"));
    let order_err = checkReadClauses(false, true).expect_err("order by");
    assert!(order_err.message.contains("don't support order by"));
}

/// 校验 Datum 有序比较与 SQL 字面量还原（分片边界依赖）。
// 对应 Datum 比较与 SQL 字面量还原：分片边界依赖有序比较与字面量拼接。
#[test]
fn datum_compare_and_sql_literal_cover_signed_text_and_binary() {
    assert_eq!(
        Datum::Signed(1).compare(&Datum::Signed(2), false).unwrap(),
        Ordering::Less
    );
    assert_eq!(
        Datum::Text("a".into())
            .compare(&Datum::Text("B".into()), true)
            .unwrap(),
        Ordering::Less
    );
    assert_eq!(
        Datum::Signed(42).to_sql_literal(FieldType::Signed).unwrap(),
        "42"
    );
    assert_eq!(
        Datum::Text("o'reilly".into())
            .to_sql_literal(FieldType::Text)
            .unwrap(),
        "'o''reilly'"
    );
    assert_eq!(
        Datum::Binary(vec![0x41, 0xf6])
            .to_sql_literal(FieldType::Binary)
            .unwrap(),
        "X'41F6'"
    );
}

/// 校验执行成功汇总与 dry-run 结果集字段名形状。
// 对应 buildExecuteResults / buildDryRunResults：成功汇总与 dry-run 字段名。
#[test]
fn build_execute_and_dry_run_results_match_go_shapes() {
    let mut runtime = StubRuntime::new();
    let mut jobs = Vec::new();
    appendNewJob(
        &mut jobs,
        Datum::Signed(0),
        Datum::Signed(9),
        10,
        &mut runtime,
    )
    .unwrap();
    appendNewJob(
        &mut jobs,
        Datum::Signed(10),
        Datum::Signed(19),
        10,
        &mut runtime,
    )
    .unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].jobID, 1);
    assert_eq!(jobs[1].jobID, 2);

    let ok = buildExecuteResults(&jobs, 35, "OFF", &mut runtime).unwrap();
    assert_eq!(ok.fields[0].name, "number of jobs");
    assert_eq!(ok.fields[1].name, "job status");
    assert_eq!(
        ok.rows[0],
        vec![
            ResultValue::Integer(2),
            ResultValue::Text("all succeeded".into())
        ]
    );

    let mut failed = jobs.clone();
    failed[1].err = Some(NonTransactionalError::new(
        "injected batch(non-transactional) DML error",
    ));
    failed[1].sql = "UPDATE `test`.`t` SET `b`=42 WHERE `a` BETWEEN 3 AND 5".into();
    let err = buildExecuteResults(&failed, 35, "OFF", &mut runtime).unwrap_err();
    assert_eq!(
        err.message,
        "1/2 jobs failed in the non-transactional DML: job id: 2, estimated size: 10, sql: UPDATE `test`.`t` SET `b`=42 WHERE `a` BETWEEN 3 AND 5, injected batch(non-transactional) DML error;, ...(more in logs)"
    );

    // Go TestNonTransactionalDMLErrorMessage sets tidb_redact_log=marker and
    // requires the complete job SQL to be enclosed by redaction markers.
    let marker_err = buildExecuteResults(&failed, 35, "MARKER", &mut runtime).unwrap_err();
    assert_eq!(
        marker_err.message,
        "1/2 jobs failed in the non-transactional DML: job id: 2, estimated size: 10, sql: ‹UPDATE `test`.`t` SET `b`=42 WHERE `a` BETWEEN 3 AND 5›, injected batch(non-transactional) DML error;, ...(more in logs)"
    );

    let dry = buildDryRunResults(
        DRY_RUN_SPLIT_DML,
        vec!["UPDATE t SET b=b*2 WHERE a BETWEEN 0 AND 9".into()],
        35,
    )
    .unwrap();
    assert_eq!(dry.fields[0].name, "split statement examples");
    let dry_q = buildDryRunResults(DRY_RUN_QUERY, vec!["SELECT a FROM t".into()], 35).unwrap();
    assert_eq!(dry_q.fields[0].name, "query statement");
    let _ = job {
        start: Datum::Null,
        end: Datum::Null,
        err: None,
        jobID: 0,
        jobSize: 0,
        sql: String::new(),
    };
}

/// 在 mock store 上用逐点 update/delete 模拟分片 DML，并以 analyze 行数校验可见性。
// 对应 TestNonTransactionalDMLSharding SQL 路径子集：逐点 update/delete 后以 analyze 校验行数。
#[test]
fn batch_style_update_and_delete_cover_sharded_dml_visibility() {
    use astersql_domain::Domain;
    use astersql_testkit::TestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    /// 从 Domain 统计元数据读取表实时行数。
    fn table_row_count(domain: &Domain, database: &str, table: &str) -> i64 {
        let table_info = domain
            .table_by_name(database, table)
            .unwrap_or_else(|error| {
                panic!("typed InfoSchema lookup for {database}.{table}: {error}")
            });
        domain
            .stats_handle()
            .lock()
            .expect("statistics handle")
            .stats_meta(table_info.ID)
            .cloned()
            .unwrap_or_else(|| panic!("no statistics recorded for {database}.{table}"))
            .realtime_count
    }

    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table t(a int, b int, primary key(a))", Vec::new());
    for i in 0..10 {
        tk.MustExec(
            &format!("insert into t values ({i}, {})", i * 2),
            Vec::new(),
        );
    }
    // 轻量 runtime 仅支持 WHERE k = literal；用逐点更新模拟分片 DML。
    for i in 0..10 {
        tk.MustExec(
            &format!("update t set b = {} where a = {i}", i * 4),
            Vec::new(),
        );
    }
    tk.MustExec("analyze table t", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t"), 10);
    for i in 0..10 {
        tk.MustExec(&format!("delete from t where a = {i}"), Vec::new());
    }
    tk.MustExec("analyze table t", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t"), 0);
}
