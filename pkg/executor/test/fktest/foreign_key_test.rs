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

//! 外键执行器回归测试，对应 Go 版本的 `foreign_key_test.go`。
//!
//! 测试沿用 Go 版本在模拟存储上使用真实 TestKit 的边界：SQL 由 Rust
//! 会话与执行器执行，断言直接检查结果行或生产错误文本。文件末尾的目录测试
//! 则覆盖 Go DDL 回调所依赖的外键模式状态迁移。

#![allow(non_snake_case)]

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use astersql_executor::foreign_key::{
    Assignment, ColumnInfo as CascadeColumnInfo, Datum, FKCascadeRuntimeStats, FKCheckRuntimeStats,
    GenCascadeDeleteAST, GenCascadeSetNullAST, GenCascadeUpdateAST, TableRefsClause,
    UpdatedValuesCouple, WhereCondition,
};
use astersql_parser::NormalizeDigest;
use astersql_parser_auth::parser::auth::auth::UserIdentity;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_testkit_testfailpoint::enable as enable_failpoint;

const FK_ERROR: &str = "Cannot add or update a child row: a foreign key constraint fails";
const PARENT_ERROR: &str = "Cannot delete or update a parent row: a foreign key constraint fails";

static TXN_SIZE_LIMIT_TEST_LOCK: Mutex<()> = Mutex::new(());

struct TxnSizeLimitGuard(u64);

impl Drop for TxnSizeLimitGuard {
    fn drop(&mut self) {
        astersql_kv::TxnTotalSizeLimit.store(self.0, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy)]
/// 一组复合外键建表场景，用同一套 DML 断言覆盖不同索引形态。
struct ForeignKeyCase {
    /// 准备场景时按顺序执行的会话设置与建表语句。
    ddl: &'static [&'static str],
    /// 主键是否使外键列隐式非空，从而不能覆盖 NULL 跳过检查的分支。
    not_null: bool,
}

/// 覆盖唯一索引、普通索引以及聚簇/非聚簇复合主键的外键布局。
const FOREIGN_KEY_CASES: &[ForeignKeyCase] = &[
    ForeignKeyCase {
        ddl: &[
            "create table t1 (id int, a int, b int, unique index(id), unique index(a, b))",
            "create table t2 (b int, name varchar(10), a int, id int, unique index(id), unique index(a,b), foreign key fk(a,b) references t1(a,b))",
        ],
        not_null: false,
    },
    ForeignKeyCase {
        ddl: &[
            "create table t1 (id int key, a int, b int, unique index(id), unique index(a,b,id))",
            "create table t2 (b int, a int, id int key, name varchar(10), unique index(a,b,id), foreign key fk(a,b) references t1(a,b))",
        ],
        not_null: false,
    },
    ForeignKeyCase {
        ddl: &[
            "create table t1 (id int key, a int, b int, unique index(id), index(a,b))",
            "create table t2 (b int, a int, name varchar(10), id int key, index(a,b), foreign key fk(a,b) references t1(a,b))",
        ],
        not_null: false,
    },
    ForeignKeyCase {
        ddl: &[
            "create table t1 (id int key, a int, b int, unique index(id), index(a,b,id))",
            "create table t2 (name varchar(10), b int, a int, id int key, index(a,b,id), foreign key fk(a,b) references t1(a,b))",
        ],
        not_null: false,
    },
    ForeignKeyCase {
        ddl: &[
            "set @@tidb_enable_clustered_index=0",
            "create table t1 (id int, a int, b int, unique index(id), primary key(a,b))",
            "create table t2 (b int, name varchar(10), a int, id int, unique index(id), primary key(a,b), foreign key fk(a,b) references t1(a,b))",
        ],
        not_null: true,
    },
    ForeignKeyCase {
        ddl: &[
            "set @@tidb_enable_clustered_index=1",
            "create table t1 (id int, a int, b int, unique index(id), primary key(a,b))",
            "create table t2 (b int, a int, name varchar(10), id int, unique index(id), primary key(a,b), foreign key fk(a,b) references t1(a,b))",
        ],
        not_null: true,
    },
    ForeignKeyCase {
        ddl: &[
            "set @@tidb_enable_clustered_index=0",
            "create table t1 (id int, a int, b int, unique index(id), primary key(a,b,id))",
            "create table t2 (b int, a int, id int, name varchar(10), unique index(id), primary key(a,b,id), foreign key fk(a,b) references t1(a,b))",
        ],
        not_null: true,
    },
    ForeignKeyCase {
        ddl: &[
            "set @@tidb_enable_clustered_index=1",
            "create table t1 (id int, a int, b int, unique index(id), primary key(a,b,id))",
            "create table t2 (name varchar(10), b int, a int, id int, unique index(id), primary key(a,b,id), foreign key fk(a,b) references t1(a,b))",
        ],
        not_null: true,
    },
];

const FOREIGN_KEY_LOCK_CASES: &[&[&str]] = &[
    &[
        "create table t1 (id int, name varchar(10), unique index (id))",
        "create table t2 (a int, name varchar(10), unique index (a), foreign key fk(a) references t1(id))",
    ],
    &[
        "create table t1 (id int, name varchar(10), unique index (id,name))",
        "create table t2 (name varchar(10), a int, unique index (a,name), foreign key fk(a) references t1(id))",
    ],
    &[
        "create table t1 (id int, name varchar(10), index (id))",
        "create table t2 (a int, name varchar(10), index (a), foreign key fk(a) references t1(id))",
    ],
    &[
        "create table t1 (id int, name varchar(10), index (id,name))",
        "create table t2 (name varchar(10), a int, index (a,name), foreign key fk(a) references t1(id))",
    ],
    &[
        "set @@tidb_enable_clustered_index=0",
        "create table t1 (id int, name varchar(10), primary key (id))",
        "create table t2 (a int, name varchar(10), primary key (a), foreign key fk(a) references t1(id))",
    ],
    &[
        "set @@tidb_enable_clustered_index=1",
        "create table t1 (id int, name varchar(10), primary key (id))",
        "create table t2 (a int, name varchar(10), primary key (a), foreign key fk(a) references t1(id))",
    ],
    &[
        "set @@tidb_enable_clustered_index=0",
        "create table t1 (id int, name varchar(10), primary key (id,name))",
        "create table t2 (a int, name varchar(10), primary key (a,name), foreign key fk(a) references t1(id))",
    ],
    &[
        "set @@tidb_enable_clustered_index=1",
        "create table t1 (id int, name varchar(10), primary key (id,name))",
        "create table t2 (a int, name varchar(10), primary key (a,name), foreign key fk(a) references t1(id))",
    ],
];

fn fk_testkit() -> TestKit {
    let store = create_fk_store();
    let mut tk = new_fk_testkit(store);
    tk.MustExec("set @@global.tidb_enable_foreign_key=1", Vec::new());
    tk.MustExec("set @@foreign_key_checks=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk
}

fn create_fk_store() -> Arc<AnalyzeStatsStore> {
    let _guard = TXN_SIZE_LIMIT_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    CreateMockStoreAndDomain().0
}

fn new_fk_testkit(store: Arc<AnalyzeStatsStore>) -> TestKit {
    let _guard = TXN_SIZE_LIMIT_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    NewTestKit(store)
}

/// 清理矩阵用表，确保每个外键布局都从独立的目录状态开始。
fn reset_tables(tk: &mut TestKit) {
    tk.MustExec("drop table if exists t2", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());
}

/// 重建指定矩阵场景；DDL 顺序中可能包含影响主键形态的会话变量。
fn prepare_case(tk: &mut TestKit, case: ForeignKeyCase) {
    reset_tables(tk);
    for ddl in case.ddl {
        tk.MustExec(ddl, Vec::new());
    }
}

/// 用同一索引矩阵重建带引用动作的场景。
fn prepare_case_with_action(tk: &mut TestKit, case: ForeignKeyCase, action: &str) {
    reset_tables(tk);
    for ddl in case.ddl {
        let ddl = ddl.replace(
            "references t1(a,b)",
            &format!("references t1(a,b) {action}"),
        );
        tk.MustExec(&ddl, Vec::new());
    }
}

/// 创建单列父子表，并把删除/更新动作作为参数注入外键定义。
fn create_simple_fk(tk: &mut TestKit, action: &str) {
    reset_tables(tk);
    tk.MustExec(
        "create table t1 (id int primary key, a int, b int)",
        Vec::new(),
    );
    tk.MustExec(
        &format!(
            "create table t2 (id int primary key, pid int, a int, index(pid), foreign key fk(pid) references t1(id) {action})"
        ),
        Vec::new(),
    );
}

fn assert_fk_error(tk: &mut TestKit, sql: &str) {
    let error = match tk.Exec(sql, Vec::new()) {
        Err(error) => error,
        Ok(result) => panic!("expected foreign-key error for {sql}: {result:?}"),
    };
    assert!(
        error.to_string().contains(FK_ERROR),
        "unexpected error for {sql}: {error}"
    );
}

fn assert_parent_error(tk: &mut TestKit, sql: &str) {
    let error = match tk.Exec(sql, Vec::new()) {
        Err(error) => error,
        Ok(result) => panic!("expected parent-row error for {sql}: {result:?}"),
    };
    assert!(
        error.to_string().contains(PARENT_ERROR),
        "unexpected error for {sql}: {error}"
    );
}

#[test]
fn TestForeignKeyOnInsertChildTable() {
    let mut tk = fk_testkit();
    tk.MustExec("create table t_data (id int, a int, b int)", Vec::new());
    tk.MustExec("insert into t_data values (1,1,1),(2,2,2)", Vec::new());
    for case in FOREIGN_KEY_CASES {
        prepare_case(&mut tk, *case);
        tk.MustExec("insert into t1 (id,a,b) values (1,1,1)", Vec::new());
        tk.MustExec("insert into t2 (id,a,b) values (1,1,1)", Vec::new());
        if !case.not_null {
            // 复合外键任一列为 NULL 时不要求父表存在对应键。
            tk.MustExec(
                "insert into t2 (id,a,b) values (2,NULL,1),(3,1,NULL),(4,NULL,NULL)",
                Vec::new(),
            );
        }
        assert_fk_error(&mut tk, "insert into t2 (id,a,b) values (5,1,0)");
        assert_fk_error(&mut tk, "insert into t2 (id,a,b) values (6,0,1)");
        assert_fk_error(&mut tk, "insert into t2 (id,a,b) values (7,2,2)");
        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec(
            "insert into t2 (id,a,b) select id,a,b from t_data where id=1",
            Vec::new(),
        );
        assert_fk_error(
            &mut tk,
            "insert into t2 (id,a,b) select id,a,b from t_data where id=2",
        );
        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec("begin", Vec::new());
        tk.MustExec("delete from t1 where a=1", Vec::new());
        // 外键检查必须看到当前事务中父行已被删除，而不是读取事务前快照。
        assert_fk_error(&mut tk, "insert into t2 (id,a,b) values (1,1,1)");
        tk.MustExec("insert into t1 (id,a,b) values (2,2,2)", Vec::new());
        tk.MustExec("insert into t2 (id,a,b) values (2,2,2)", Vec::new());
        tk.MustExec("rollback", Vec::new());
        tk.MustQuery("select id,a,b from t1 order by id", Vec::new())
            .Check(Rows(&["1 1 1"]));
        tk.MustQuery("select id,a,b from t2 order by id", Vec::new())
            .Check(Rows(&[]));
    }

    // Go case 10: a NOT NULL foreign-key column uses its default and must
    // still be checked against the parent table.
    reset_tables(&mut tk);
    tk.MustExec("set @@tidb_enable_clustered_index=0", Vec::new());
    tk.MustExec("create table t1 (id int,a int,primary key(id))", Vec::new());
    tk.MustExec(
        "create table t2 (id int key,a int not null default 0,index(a),foreign key fk(a) references t1(id))",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1,1)", Vec::new());
    tk.MustExec("insert into t2 values (1,1)", Vec::new());
    assert_fk_error(&mut tk, "insert into t2 (id) values (10)");
    assert_fk_error(&mut tk, "insert into t2 values (3,2)");

    // Go case 11: a nullable foreign-key column without a default resolves to
    // NULL and therefore skips the referenced-row lookup.
    tk.MustExec("drop table t2", Vec::new());
    tk.MustExec(
        "create table t2 (id int key,a int,index(a),foreign key fk(a) references t1(id))",
        Vec::new(),
    );
    tk.MustExec("insert into t2 values (1,1)", Vec::new());
    tk.MustExec("insert into t2 (id) values (10)", Vec::new());
    assert_fk_error(&mut tk, "insert into t2 values (3,2)");
}

#[test]
fn TestForeignKeyOnInsertDuplicateUpdateChildTable() {
    let mut tk = fk_testkit();
    for case in FOREIGN_KEY_CASES {
        prepare_case(&mut tk, *case);
        tk.MustExec(
            "insert into t1 (id,a,b) values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,11,21,'a')",
            Vec::new(),
        );
        for sql in [
            "insert into t2 (id,a,b,name) values (1,12,22,'b') on duplicate key update a=100",
            "insert into t2 (id,a,b,name) values (1,13,23,'c') on duplicate key update a=a+10",
            "insert into t2 (id,a,b,name) values (1,14,24,'d') on duplicate key update a=a+100",
            "insert into t2 (id,a,b,name) values (1,14,24,'d') on duplicate key update a=12,b=23",
        ] {
            assert_fk_error(&mut tk, sql);
        }
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,14,26,'b') on duplicate key update a=12,b=22,name='x'",
            Vec::new(),
        );
        tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
            .Check(Rows(&["1 12 22 x"]));
        if !case.not_null {
            tk.MustExec(
                "insert into t2 (id,a,b,name) values (1,14,26,'b') on duplicate key update a=null,b=22,name='y'",
                Vec::new(),
            );
            tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
                .Check(Rows(&["1 <nil> 22 y"]));
            tk.MustExec(
                "insert into t2 (id,a,b,name) values (1,15,26,'b') on duplicate key update b=null,name='z'",
                Vec::new(),
            );
            tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
                .Check(Rows(&["1 <nil> <nil> z"]));
        }
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,15,26,'b') on duplicate key update a=13,b=23,name='c'",
            Vec::new(),
        );
        tk.MustQuery("select id,a,b,name from t2", Vec::new())
            .Check(Rows(&["1 13 23 c"]));

        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec("delete from t1", Vec::new());
        tk.MustExec(
            "insert into t1 (id,a,b) values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (2,11,21,'a')",
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (2,14,26,'b') on duplicate key update a=12,b=22,name='x'",
            Vec::new(),
        );
        tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
            .Check(Rows(&["2 12 22 x"]));
        tk.MustExec("rollback", Vec::new());
        tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
            .Check(Rows(&["2 11 21 a"]));

        tk.MustExec("begin", Vec::new());
        tk.MustExec("delete from t1 where id=3", Vec::new());
        assert_fk_error(
            &mut tk,
            "insert into t2 (id,a,b,name) values (2,13,23,'y') on duplicate key update a=13,b=23,name='y'",
        );
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (2,14,24,'z') on duplicate key update a=14,b=24,name='z'",
            Vec::new(),
        );
        tk.MustExec("insert into t1 (id,a,b) values (5,15,25)", Vec::new());
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (2,15,25,'o') on duplicate key update a=15,b=25,name='o'",
            Vec::new(),
        );
        tk.MustExec("delete from t1 where id=1", Vec::new());
        assert_fk_error(
            &mut tk,
            "insert into t2 (id,a,b,name) values (2,11,21,'y') on duplicate key update a=11,b=21,name='p'",
        );
        tk.MustExec("commit", Vec::new());
        tk.MustQuery("select id,a,b,name from t2", Vec::new())
            .Check(Rows(&["2 15 25 o"]));
    }

    reset_tables(&mut tk);
    tk.MustExec("set @@tidb_enable_clustered_index=0", Vec::new());
    tk.MustExec(
        "create table t1 (id int,a int,b int,primary key(id))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t2 (b int,a int,id int,name varchar(10),primary key(a),foreign key fk(a) references t1(id))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 (id,a,b) values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t2 (id,a,b,name) values (11,1,21,'a')",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t2 (id,a) values (11,1) on duplicate key update a=2,name='b'",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t2 (id,a,b) values (11,2,22) on duplicate key update a=3,name='c'",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t2 (id,a,name) values (11,3,'b') on duplicate key update b=b+10,name='d'",
        Vec::new(),
    );
    tk.MustQuery("select id,a,b,name from t2", Vec::new())
        .Check(Rows(&["11 3 31 d"]));
    tk.MustExec(
        "insert into t2 (id,a,name) values (11,3,'b') on duplicate key update id=1,name='f'",
        Vec::new(),
    );
    assert_fk_error(
        &mut tk,
        "insert into t2 (id,a,name) values (1,3,'b') on duplicate key update a=10",
    );
    tk.MustQuery("select id,a,b,name from t2", Vec::new())
        .Check(Rows(&["1 3 31 f"]));
}

#[test]
fn TestForeignKeyCheckAndLock() {
    let store = create_fk_store();
    let mut tk = new_fk_testkit(store.clone());
    let mut tk2 = new_fk_testkit(store.clone());
    for session in [&mut tk, &mut tk2] {
        session.MustExec("set @@global.tidb_enable_foreign_key=1", Vec::new());
        session.MustExec("set @@foreign_key_checks=1", Vec::new());
        session.MustExec("use test", Vec::new());
        session.MustExec("set @@tidb_pessimistic_txn_fair_locking=0", Vec::new());
    }

    for ddls in FOREIGN_KEY_LOCK_CASES {
        reset_tables(&mut tk);
        for ddl in *ddls {
            tk.MustExec(ddl, Vec::new());
        }

        // An optimistic child insert reads and protects its parent key. A
        // concurrent parent delete must therefore turn commit into a write
        // conflict, with neither child row nor stale parent surviving.
        tk.MustExec("insert into t1 (id,name) values (1,'a')", Vec::new());
        tk.MustExec("begin optimistic", Vec::new());
        tk.MustExec("insert into t2 (a,name) values (1,'a')", Vec::new());
        tk2.MustExec("delete from t1 where id=1", Vec::new());
        tk.MustContainErrMsg("commit", "Write conflict");
        tk.MustQuery("select id,name from t1 order by name", Vec::new())
            .Check(Rows(&[]));
        tk.MustQuery("select a,name from t2 order by name", Vec::new())
            .Check(Rows(&[]));

        tk.MustExec("insert into t1 (id,name) values (1,'a')", Vec::new());
        tk.MustExec("begin optimistic", Vec::new());
        tk.MustExec("insert into t2 (a,name) values (1,'a')", Vec::new());
        tk2.MustExec("update t1 set id=2 where id=1", Vec::new());
        tk.MustContainErrMsg("commit", "Write conflict");
        tk.MustQuery("select id,name from t1 order by name", Vec::new())
            .Check(Rows(&["2 a"]));
        tk.MustQuery("select a,name from t2 order by name", Vec::new())
            .Check(Rows(&[]));

        tk.MustExec("delete from t1", Vec::new());
        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec(
            "insert into t1 (id,name) values (1,'a'),(2,'b')",
            Vec::new(),
        );
        tk.MustExec("insert into t2 (a,name) values (1,'a')", Vec::new());
        tk.MustExec("begin optimistic", Vec::new());
        tk.MustExec("update t2 set a=2 where a=1", Vec::new());
        tk2.MustExec("delete from t1 where id=2", Vec::new());
        tk.MustContainErrMsg("commit", "Write conflict");
        tk.MustQuery("select id,name from t1 order by name", Vec::new())
            .Check(Rows(&["1 a"]));
        tk.MustQuery("select a,name from t2 order by name", Vec::new())
            .Check(Rows(&["1 a"]));

        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec("begin pessimistic", Vec::new());
        tk.MustExec("insert into t2 (a,name) values (1,'a')", Vec::new());
        let peer_store = store.clone();
        let worker = thread::spawn(move || {
            let mut peer = new_fk_testkit(peer_store);
            peer.MustExec("set @@foreign_key_checks=1", Vec::new());
            peer.MustExec("use test", Vec::new());
            peer.MustExec("begin pessimistic", Vec::new());
            let error = peer.ExecToErr("update t1 set id=2 where id=1").to_string();
            peer.MustExec("commit", Vec::new());
            error
        });
        thread::sleep(Duration::from_millis(50));
        tk.MustExec("commit", Vec::new());
        assert!(
            worker
                .join()
                .expect("parent-update lock waiter")
                .contains(PARENT_ERROR),
            "parent update must observe the committed child"
        );

        tk.MustExec("insert into t1 (id,name) values (2,'b')", Vec::new());
        tk.MustExec("begin pessimistic", Vec::new());
        tk.MustExec("update t2 set a=2 where a=1", Vec::new());
        let peer_store = store.clone();
        let worker = thread::spawn(move || {
            let mut peer = new_fk_testkit(peer_store);
            peer.MustExec("set @@foreign_key_checks=1", Vec::new());
            peer.MustExec("use test", Vec::new());
            peer.MustExec("begin pessimistic", Vec::new());
            let error = peer.ExecToErr("update t1 set id=3 where id=2").to_string();
            peer.MustExec("commit", Vec::new());
            error
        });
        thread::sleep(Duration::from_millis(50));
        tk.MustExec("commit", Vec::new());
        assert!(
            worker
                .join()
                .expect("updated-child lock waiter")
                .contains(PARENT_ERROR),
            "parent update must observe the updated child"
        );

        tk.MustExec("begin pessimistic", Vec::new());
        tk.MustExec("insert into t2 (a,name) values (1,'a')", Vec::new());
        for delete_sql in ["delete from t1 where id=1", "delete from t1 where id<5"] {
            let peer_store = store.clone();
            let sql = delete_sql.to_owned();
            let worker = thread::spawn(move || {
                let mut peer = new_fk_testkit(peer_store);
                peer.MustExec("set @@foreign_key_checks=1", Vec::new());
                peer.MustExec("use test", Vec::new());
                peer.MustExec("begin pessimistic", Vec::new());
                let error = peer.ExecToErr(&sql).to_string();
                peer.MustExec("commit", Vec::new());
                error
            });
            thread::sleep(Duration::from_millis(50));
            tk.MustExec("commit", Vec::new());
            assert!(
                worker
                    .join()
                    .expect("parent-delete lock waiter")
                    .contains(PARENT_ERROR),
                "parent delete must observe the committed child"
            );
            if delete_sql.contains('<') {
                break;
            }
            tk.MustExec("delete from t2", Vec::new());
            tk.MustExec("begin pessimistic", Vec::new());
            tk.MustExec("insert into t2 (a,name) values (1,'a')", Vec::new());
        }
        tk.MustQuery("select id,name from t1 order by name", Vec::new())
            .Check(Rows(&["1 a", "2 b"]));
        tk.MustQuery("select a,name from t2 order by a", Vec::new())
            .Check(Rows(&["1 a"]));
    }
}

#[test]
fn TestForeignKeyOnInsertOnDuplicateParentTableCheck() {
    let mut tk = fk_testkit();
    for case in FOREIGN_KEY_CASES {
        prepare_case(&mut tk, *case);
        if !case.not_null {
            tk.MustExec(
                "insert into t1 (id,a,b) values (1,11,21),(2,12,22),(3,13,23),(4,14,24),(5,15,null),(6,null,26),(7,null,null)",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t2 (id,a,b,name) values (1,11,21,'a'),(5,15,null,'e'),(6,null,26,'f'),(7,null,null,'g')",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t1 (id,a) values (2,12) on duplicate key update a=a+100,b=b+200",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t1 (id,a) values (3,13),(2,12) on duplicate key update a=a+1000,b=b+2000",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t1 (id) values (5),(6),(7) on duplicate key update a=a+10000,b=b+20000",
                Vec::new(),
            );
            assert_parent_error(
                &mut tk,
                "insert into t1 (id,a) values (1,11) on duplicate key update a=a+10,b=b+20",
            );
            tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
                .Check(Rows(&[
                    "1 11 21 a",
                    "5 15 <nil> e",
                    "6 <nil> 26 f",
                    "7 <nil> <nil> g",
                ]));
        } else {
            tk.MustExec(
                "insert into t1 (id,a,b) values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t2 (id,a,b,name) values (1,11,21,'a')",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t1 values (2,12,22) on duplicate key update a=a+100,b=b+200",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t1 values (3,13,23),(2,12,22) on duplicate key update a=a+1000,b=b+2000",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t1 values (1,11,21) on duplicate key update id=11",
                Vec::new(),
            );
            assert_parent_error(
                &mut tk,
                "insert into t1 values (11,11,21) on duplicate key update a=a+10,b=b+20",
            );
            tk.MustQuery("select id,a,b,name from t2", Vec::new())
                .Check(Rows(&["1 11 21 a"]));
        }
    }

    reset_tables(&mut tk);
    tk.MustExec(
        "create table t1 (id int,a int,b int,primary key(id))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t2 (b int,a int,id int,name varchar(10),primary key(a),foreign key fk(a) references t1(id))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
        Vec::new(),
    );
    tk.MustExec("insert into t2 values (21,1,11,'a')", Vec::new());
    tk.MustExec(
        "insert into t1 values (2,0,0),(3,0,0) on duplicate key update id=id+100",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,0,0) on duplicate key update a=a+100",
        Vec::new(),
    );
    assert_parent_error(
        &mut tk,
        "insert into t1 values (1,0,0) on duplicate key update id=100+id",
    );
    tk.MustQuery("select id,a,b from t1 order by id", Vec::new())
        .Check(Rows(&["1 111 21", "4 14 24", "102 12 22", "103 13 23"]));
}

#[test]
fn TestForeignKeyConcurrentInsertChildTable() {
    let store = create_fk_store();
    let mut setup = new_fk_testkit(store.clone());
    setup.MustExec("set @@global.tidb_enable_foreign_key=1", Vec::new());
    setup.MustExec("set @@foreign_key_checks=1", Vec::new());
    setup.MustExec("use test", Vec::new());
    setup.MustExec("create table t1 (id int,a int,primary key(id))", Vec::new());
    setup.MustExec(
        "create table t2 (id int,a int,index(a),foreign key fk(a) references t1(id))",
        Vec::new(),
    );
    setup.MustExec(
        "insert into t1 values (1,11),(2,12),(3,13),(4,14)",
        Vec::new(),
    );
    // 与 Go 一致，让多个会话并发写入相同的非唯一 id 值，外键锁只按父键协调。
    let mut workers = Vec::new();
    for _ in 0..10 {
        let store = Arc::clone(&store);
        workers.push(thread::spawn(move || {
            let mut tk = new_fk_testkit(store);
            tk.MustExec("set @@global.tidb_enable_foreign_key=1", Vec::new());
            tk.MustExec("set @@foreign_key_checks=1", Vec::new());
            tk.MustExec("use test", Vec::new());
            for row in 0..20 {
                tk.MustExec(
                    &format!("insert into t2 values ({row}, {})", row % 4 + 1),
                    Vec::new(),
                );
            }
        }));
    }
    for worker in workers {
        worker.join().expect("foreign-key insert worker");
    }
    setup
        .MustQuery("select count(*) from t2", Vec::new())
        .Check(Rows(&["200"]));
}

#[test]
fn TestForeignKeyOnUpdateChildTable() {
    let mut tk = fk_testkit();
    for case in FOREIGN_KEY_CASES {
        prepare_case(&mut tk, *case);
        tk.MustExec(
            "insert into t1 (id,a,b) values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,11,21,'a')",
            Vec::new(),
        );
        for sql in [
            "update t2 set a=100,b=200 where id=1",
            "update t2 set a=a+10,b=b+20 where a=11",
            "update t2 set a=a+100,b=b+200",
            "update t2 set a=12,b=23 where id=1",
        ] {
            assert_fk_error(&mut tk, sql);
        }
        tk.MustExec("update t2 set a=12,b=22 where id=1", Vec::new());
        tk.MustQuery("select id,a,b,name from t2", Vec::new())
            .Check(Rows(&["1 12 22 a"]));
        if !case.not_null {
            tk.MustExec("update t2 set a=null,b=22 where a=12", Vec::new());
            tk.MustExec("update t2 set b=null where b=22", Vec::new());
            tk.MustQuery("select id,a,b,name from t2", Vec::new())
                .Check(Rows(&["1 <nil> <nil> a"]));
        }
        tk.MustExec("update t2 set a=13,b=23 where id=1", Vec::new());
        tk.MustQuery("select id,a,b,name from t2", Vec::new())
            .Check(Rows(&["1 13 23 a"]));

        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec("delete from t1", Vec::new());
        tk.MustExec(
            "insert into t1 (id,a,b) values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,11,21,'a')",
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
        tk.MustExec("update t2 set a=12,b=22 where id=1", Vec::new());
        tk.MustExec("rollback", Vec::new());
        tk.MustExec("begin", Vec::new());
        tk.MustExec("delete from t1 where id=2", Vec::new());
        assert_fk_error(&mut tk, "update t2 set a=12,b=22 where id=1");
        tk.MustExec("update t2 set a=13,b=23 where id=1", Vec::new());
        tk.MustExec("insert into t1 (id,a,b) values (5,15,25)", Vec::new());
        tk.MustExec("update t2 set a=15,b=25 where id=1", Vec::new());
        tk.MustExec("delete from t1 where id=1", Vec::new());
        assert_fk_error(&mut tk, "update t2 set a=11,b=21 where id=1");
        tk.MustExec("commit", Vec::new());
        tk.MustQuery("select id,a,b,name from t2", Vec::new())
            .Check(Rows(&["1 15 25 a"]));
    }

    reset_tables(&mut tk);
    tk.MustExec("set @@tidb_enable_clustered_index=0", Vec::new());
    tk.MustExec(
        "create table t1 (id int,a int,b int,primary key(id))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t2 (b int,a int,id int,name varchar(10),primary key(a),foreign key fk(a) references t1(id))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
        Vec::new(),
    );
    tk.MustExec("insert into t2 values (21,1,11,'a')", Vec::new());
    tk.MustExec("update t2 set a=2 where id=11", Vec::new());
    tk.MustExec("update t2 set a=3 where id=11", Vec::new());
    tk.MustExec("update t2 set b=b+1 where id=11", Vec::new());
    tk.MustExec("update t2 set id=1 where id=11", Vec::new());
    assert_fk_error(&mut tk, "update t2 set a=10 where id=1");
    tk.MustQuery("select id,a,b,name from t2", Vec::new())
        .Check(Rows(&["1 3 22 a"]));
}

#[test]
fn TestForeignKeyOnUpdateParentTableCheck() {
    let mut tk = fk_testkit();
    for case in FOREIGN_KEY_CASES {
        prepare_case(&mut tk, *case);
        if !case.not_null {
            tk.MustExec(
                "insert into t1 (id,a,b) values (1,11,21),(2,12,22),(3,13,23),(4,14,24),(5,15,null),(6,null,26),(7,null,null)",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t2 (id,a,b,name) values (1,11,21,'a'),(5,15,null,'e'),(6,null,26,'f'),(7,null,null,'g')",
                Vec::new(),
            );
            tk.MustExec("update t1 set a=a+100,b=b+200 where id=2", Vec::new());
            tk.MustExec(
                "update t1 set a=a+1000,b=b+2000 where a=13 or b=222",
                Vec::new(),
            );
            tk.MustExec(
                "update t1 set a=a+10000,b=b+20000 where id=5 or a is null or b is null",
                Vec::new(),
            );
            tk.MustQuery("select id,a,b from t1 order by id", Vec::new())
                .Check(Rows(&[
                    "1 11 21",
                    "2 1112 2222",
                    "3 1013 2023",
                    "4 14 24",
                    "5 10015 <nil>",
                    "6 <nil> 20026",
                    "7 <nil> <nil>",
                ]));
        } else {
            tk.MustExec(
                "insert into t1 (id,a,b) values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t2 (id,a,b,name) values (1,11,21,'a')",
                Vec::new(),
            );
            tk.MustExec("update t1 set a=a+100,b=b+200 where id=2", Vec::new());
            tk.MustExec(
                "update t1 set a=a+1000,b=b+2000 where a=13 or b=222",
                Vec::new(),
            );
            tk.MustQuery("select id,a,b from t1 order by id", Vec::new())
                .Check(Rows(&["1 11 21", "2 1112 2222", "3 1013 2023", "4 14 24"]));
        }
        assert_parent_error(
            &mut tk,
            "update t1 set a=a+10,b=b+20 where id=1 or a=1112 or b=24",
        );
        tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
            .Check(if case.not_null {
                Rows(&["1 11 21 a"])
            } else {
                Rows(&[
                    "1 11 21 a",
                    "5 15 <nil> e",
                    "6 <nil> 26 f",
                    "7 <nil> <nil> g",
                ])
            });
    }

    reset_tables(&mut tk);
    tk.MustExec(
        "create table t1 (id int,a int,b int,primary key(id))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t2 (b int,a int,id int,name varchar(10),primary key(a),foreign key fk(a) references t1(id))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
        Vec::new(),
    );
    tk.MustExec("insert into t2 values (21,1,11,'a')", Vec::new());
    tk.MustExec("update t1 set id=id+100 where id=2 or a=13", Vec::new());
    assert_parent_error(&mut tk, "update t1 set id=id+10 where id=1 or b=24");
    tk.MustQuery("select id,a,b from t1 order by id", Vec::new())
        .Check(Rows(&["1 11 21", "4 14 24", "102 12 22", "103 13 23"]));
}

#[test]
fn TestForeignKeyOnDeleteParentTableCheck() {
    let mut tk = fk_testkit();
    for case in FOREIGN_KEY_CASES {
        prepare_case(&mut tk, *case);
        if !case.not_null {
            tk.MustExec(
                "insert into t1 (id,a,b) values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,null),(6,null,6),(7,null,null)",
                Vec::new(),
            );
            tk.MustExec(
                "insert into t2 (id,a,b) values (1,1,1),(5,5,null),(6,null,6),(7,null,null)",
                Vec::new(),
            );
            tk.MustExec("delete from t1 where id=2", Vec::new());
            tk.MustExec("delete from t1 where a=3 or b=4", Vec::new());
            tk.MustExec(
                "delete from t1 where a=5 or b=6 or a is null or b is null",
                Vec::new(),
            );
        } else {
            tk.MustExec(
                "insert into t1 (id,a,b) values (1,1,1),(2,2,2),(3,3,3),(4,4,4)",
                Vec::new(),
            );
            tk.MustExec("insert into t2 (id,a,b) values (1,1,1)", Vec::new());
            tk.MustExec("delete from t1 where id=2", Vec::new());
            tk.MustExec("delete from t1 where a=3 or b=4", Vec::new());
        }
        assert_parent_error(&mut tk, "delete from t1 where id=1");
        tk.MustQuery("select id,a,b from t1 order by id", Vec::new())
            .Check(Rows(&["1 1 1"]));

        for mode in ["pessimistic", "optimistic"] {
            tk.MustExec("delete from t2", Vec::new());
            tk.MustExec("delete from t1", Vec::new());
            tk.MustExec(&format!("begin {mode}"), Vec::new());
            tk.MustExec("insert into t1 values (1,1,1),(2,2,2)", Vec::new());
            tk.MustExec("insert into t2 (id,a,b) values (1,1,1)", Vec::new());
            assert_parent_error(&mut tk, "delete from t1 where id=1");
            tk.MustExec("delete from t1 where id=2", Vec::new());
            tk.MustExec("delete from t2 where id=1", Vec::new());
            tk.MustExec("delete from t1 where id=1", Vec::new());
            tk.MustExec("commit", Vec::new());
            tk.MustQuery("select * from t1", Vec::new())
                .Check(Rows(&[]));
            tk.MustQuery("select * from t2", Vec::new())
                .Check(Rows(&[]));
        }
    }
}

#[test]
fn TestForeignKeyOnDeleteCascade() {
    let mut tk = fk_testkit();
    for case in FOREIGN_KEY_CASES {
        prepare_case_with_action(&mut tk, *case, "on delete cascade");
        tk.MustExec(
            "insert into t1 (id,a,b) values (1,1,1),(2,2,2),(3,3,3),(4,4,4)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,1,1,'a'),(2,2,2,'b'),(3,3,3,'c'),(4,4,4,'d')",
            Vec::new(),
        );
        tk.MustExec("delete from t1 where id=1 or a=2", Vec::new());
        tk.MustQuery("select id,a,b from t2 order by id", Vec::new())
            .Check(Rows(&["3 3 3", "4 4 4"]));

        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec("delete from t1", Vec::new());
        tk.MustExec("begin", Vec::new());
        tk.MustExec(
            "insert into t1 values (1,1,1),(2,2,2),(3,3,3),(4,4,4)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,1,1,'a'),(2,2,2,'b'),(3,3,3,'c'),(4,4,4,'d')",
            Vec::new(),
        );
        tk.MustExec("delete from t1 where id=1 or a=2", Vec::new());
        tk.MustQuery("select id,a,b from t2 order by id", Vec::new())
            .Check(Rows(&["3 3 3", "4 4 4"]));
        tk.MustExec("rollback", Vec::new());
        tk.MustQuery("select * from t1", Vec::new())
            .Check(Rows(&[]));
        tk.MustQuery("select * from t2", Vec::new())
            .Check(Rows(&[]));
    }

    reset_tables(&mut tk);
    tk.MustExec("drop table if exists t3", Vec::new());
    tk.MustExec("create table t1 (id int primary key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int primary key,foreign key(id) references t1(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "create table t3 (id int primary key,foreign key(id) references t2(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1)", Vec::new());
    tk.MustExec("insert into t2 values (1)", Vec::new());
    tk.MustExec("insert into t3 values (1)", Vec::new());
    tk.MustExec("delete from t1", Vec::new());
    tk.MustQuery("select * from t2", Vec::new())
        .Check(Rows(&[]));
    tk.MustQuery("select * from t3", Vec::new())
        .Check(Rows(&[]));

    tk.MustExec("drop table t3", Vec::new());
    reset_tables(&mut tk);
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int key,pid int,index safe_pid(pid) where pid is not null,foreign key fk_pid(pid) references t1(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1),(2)", Vec::new());
    tk.MustExec("insert into t2 values (1,1),(2,2),(3,null)", Vec::new());
    tk.MustExec("delete from t1 where id=1", Vec::new());
    tk.MustQuery("select id,pid from t2 order by id", Vec::new())
        .Check(Rows(&["2 2", "3 <nil>"]));
}

#[test]
fn TestForeignKeyOnDeleteCascade2() {
    let mut tk = fk_testkit();
    tk.MustExec(
        "create table t1 (id int key,name varchar(10),leader int,index(leader),foreign key(leader) references t1(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,'boss',null),(10,'l1_a',1),(11,'l1_b',1),(12,'l1_c',1)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (100,'l2_a1',10),(101,'l2_a2',10),(102,'l2_a3',10),(110,'l2_b1',11),(111,'l2_b2',11),(112,'l2_b3',11),(120,'l2_c1',12),(121,'l2_c2',12),(122,'l2_c3',12),(1000,'l3_a1',100)",
        Vec::new(),
    );
    tk.MustExec("delete from t1 where id=11", Vec::new());
    tk.MustQuery("select id from t1 order by id", Vec::new())
        .Check(Rows(&[
            "1", "10", "12", "100", "101", "102", "120", "121", "122", "1000",
        ]));
    tk.MustExec("delete from t1 where id=1", Vec::new());
    tk.MustQuery("select * from t1", Vec::new())
        .Check(Rows(&[]));

    tk.MustExec(
        "insert into t1 values (1,'boss',null),(10,'l1_a',1),(11,'l1_b',1),(12,'l1_c',1)",
        Vec::new(),
    );
    tk.MustExec("explain analyze delete from t1 where id=1", Vec::new());
    tk.MustQuery("select * from t1", Vec::new())
        .Check(Rows(&[]));

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id varchar(10) key,name varchar(10),leader varchar(10),index(leader),foreign key(leader) references t1(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values ('1','boss',null),('10','l1_a','1'),('11','l1_b','1'),('100','l2_a1','10'),('110','l2_b1','11'),('1000','l3_a1','100')",
        Vec::new(),
    );
    tk.MustExec("delete from t1 where id='11'", Vec::new());
    tk.MustQuery("select id from t1 order by id", Vec::new())
        .Check(Rows(&["1", "10", "100", "1000"]));
    tk.MustExec("delete from t1 where id='1'", Vec::new());
    tk.MustQuery("select * from t1", Vec::new())
        .Check(Rows(&[]));

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id int primary key,pid int,index(pid),foreign key(pid) references t1(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (0,0),(1,0),(2,1),(3,2),(4,3),(5,4),(6,5),(7,6),(8,7),(9,8),(10,9),(11,10),(12,11),(13,12),(14,13),(15,14)",
        Vec::new(),
    );
    tk.MustContainErrMsg("delete from t1 where id=0", "cascade depth exceeded");
    tk.MustExec("delete from t1 where id=15", Vec::new());
    tk.MustExec("delete from t1 where id=0", Vec::new());
    tk.MustQuery("select * from t1", Vec::new())
        .Check(Rows(&[]));

    // A downstream RESTRICT edge must abort the entire cascade, including
    // the parent delete and the intermediate child delete.
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int key,foreign key(id) references t1(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "create table t3 (id int key,foreign key(id) references t2(id))",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1)", Vec::new());
    tk.MustExec("insert into t2 values (1)", Vec::new());
    tk.MustExec("insert into t3 values (1)", Vec::new());
    assert_parent_error(&mut tk, "delete from t1 where id=1");
    for table in ["t1", "t2", "t3"] {
        tk.MustQuery(&format!("select * from {table}"), Vec::new())
            .Check(Rows(&["1"]));
    }

    tk.MustExec("drop table t3", Vec::new());
    tk.MustExec("drop table t2", Vec::new());
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec("create table t2 (id int key)", Vec::new());
    tk.MustExec(
        "create table t3 (id1 int,id2 int,foreign key fk_id1(id1) references t1(id) on delete cascade,foreign key fk_id2(id2) references t2(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1),(2),(3)", Vec::new());
    tk.MustExec("insert into t2 values (1),(2),(3)", Vec::new());
    tk.MustExec(
        "insert into t3 values (1,1),(1,2),(1,3),(2,1),(2,2)",
        Vec::new(),
    );
    tk.MustExec("delete from t1 where id=1", Vec::new());
    tk.MustQuery("select * from t3 order by id1,id2", Vec::new())
        .Check(Rows(&["2 1", "2 2"]));
    tk.MustExec(
        "create table t4 (id3 int key,foreign key fk_id3(id3) references t3(id2))",
        Vec::new(),
    );
    tk.MustExec("insert into t4 values (2)", Vec::new());
    assert_parent_error(&mut tk, "delete from t1 where id=2");
    assert_parent_error(&mut tk, "delete from t2 where id=2");
    tk.MustExec("delete from t2 where id=1", Vec::new());
    tk.MustQuery("select * from t3", Vec::new())
        .Check(Rows(&["2 2"]));

    tk.MustExec("drop table t4", Vec::new());
    tk.MustExec("drop table t3", Vec::new());
    tk.MustExec("drop table t2", Vec::new());
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id int key,name varchar(10),pid int,index(pid),foreign key fk(pid) references t1(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,'boss',null),(2,'a',1),(3,'b',1),(4,'c',2)",
        Vec::new(),
    );
    let mut tk2 = NewTestKit(tk.Store());
    tk2.MustExec("set @@foreign_key_checks=1", Vec::new());
    tk2.MustExec("use test", Vec::new());
    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustExec("insert into t1 values (5,'d',3)", Vec::new());
    tk2.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("insert into t1 values (6,'e',4)", Vec::new());
    tk2.MustExec("delete from t1 where id=2", Vec::new());
    tk2.MustExec("commit", Vec::new());
    tk.MustExec("delete from t1 where id=1", Vec::new());
    tk.MustExec("commit", Vec::new());
    tk.MustQuery("select * from t1", Vec::new())
        .Check(Rows(&[]));
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t1 (c0 int,index(c0))", Vec::new());
    for column in 1..20 {
        tk.MustExec(
            &format!("alter table t1 add column c{column} int"),
            Vec::new(),
        );
        tk.MustExec(
            &format!("alter table t1 add index idx_{column}(c{column})"),
            Vec::new(),
        );
        tk.MustExec(
            &format!(
                "alter table t1 add constraint fk_{column} foreign key(c{column}) references t1(c{}) on delete cascade",
                column - 1
            ),
            Vec::new(),
        );
    }
    for value in 0..20 {
        let values = std::iter::repeat_n(value.to_string(), 20)
            .collect::<Vec<_>>()
            .join(",");
        tk.MustExec(&format!("insert into t1 values ({values})"), Vec::new());
    }
    tk.MustExec("delete from t1 where c0 in (0,1,2,3,4)", Vec::new());
    tk.MustQuery("select count(*) from t1", Vec::new())
        .Check(Rows(&["15"]));

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id int auto_increment primary key,b int)",
        Vec::new(),
    );
    tk.MustExec(
        "create table t2 (id int,b int,foreign key fk(id) references t1(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 (b) values (1),(1),(1),(1),(1),(1),(1),(1)",
        Vec::new(),
    );
    for _ in 0..12 {
        tk.MustExec("insert into t1 (b) select b from t1", Vec::new());
    }
    tk.MustQuery("select count(*) from t1", Vec::new())
        .Check(Rows(&["32768"]));
    tk.MustExec("insert into t2 select * from t1", Vec::new());
    tk.MustExec("delete from t1", Vec::new());
    tk.MustQuery("select count(*) from t1", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("select count(*) from t2", Vec::new())
        .Check(Rows(&["0"]));
}

#[test]
fn TestForeignKeyGenerateCascadeAST() {
    let values = vec![
        vec![Datum::Int64(1), Datum::String("a".to_owned())],
        vec![Datum::Int64(2), Datum::String("b".to_owned())],
    ];
    let columns = vec![
        CascadeColumnInfo {
            name: "a".to_owned(),
            offset: 0,
        },
        CascadeColumnInfo {
            name: "name".to_owned(),
            offset: 1,
        },
    ];
    for index in ["", "idx"] {
        let table_refs = TableRefsClause {
            schema: "test".to_owned(),
            table: "t2".to_owned(),
            use_index: (!index.is_empty()).then(|| index.to_owned()),
        };
        let condition = WhereCondition::MultiColumnIn {
            columns: vec!["a".to_owned(), "name".to_owned()],
            rows: values.clone(),
        };
        let delete = GenCascadeDeleteAST("test", "t2", index, &columns, values.clone())
            .expect("generate cascade DELETE");
        assert_eq!(delete.table_refs, table_refs);
        assert_eq!(delete.where_condition, condition);

        let set_null = GenCascadeSetNullAST("test", "t2", index, &columns, values.clone())
            .expect("generate cascade SET NULL");
        assert_eq!(set_null.table_refs, table_refs);
        assert_eq!(set_null.where_condition, condition);
        assert_eq!(
            set_null.assignments,
            vec![
                Assignment {
                    column: "a".to_owned(),
                    value: Datum::Null,
                },
                Assignment {
                    column: "name".to_owned(),
                    value: Datum::Null,
                },
            ]
        );

        let couple = UpdatedValuesCouple {
            NewValues: vec![Datum::Int64(10), Datum::String("aa".to_owned())],
            OldValuesList: values.clone(),
        };
        let update = GenCascadeUpdateAST("test", "t2", index, &columns, &couple)
            .expect("generate cascade UPDATE");
        assert_eq!(update.table_refs, table_refs);
        assert_eq!(update.where_condition, condition);
        assert_eq!(
            update.assignments,
            vec![
                Assignment {
                    column: "a".to_owned(),
                    value: Datum::Int64(10),
                },
                Assignment {
                    column: "name".to_owned(),
                    value: Datum::String("aa".to_owned()),
                },
            ]
        );
    }

    let column = [CascadeColumnInfo {
        name: "a".to_owned(),
        offset: 0,
    }];
    for index in ["", "idx"] {
        let delete = GenCascadeDeleteAST(
            "test",
            "t2",
            index,
            &column,
            vec![vec![Datum::Int64(1)], vec![Datum::Int64(2)]],
        )
        .expect("generate single-column cascade DELETE");
        assert_eq!(
            delete.where_condition,
            WhereCondition::SingleColumnIn {
                column: "a".to_owned(),
                values: vec![Datum::Int64(1), Datum::Int64(2)],
            }
        );
        assert_eq!(
            delete.table_refs.use_index.as_deref(),
            (!index.is_empty()).then_some(index)
        );
    }
}

#[test]
fn TestForeignKeyOnDeleteSetNull() {
    let mut tk = fk_testkit();
    for case in FOREIGN_KEY_CASES.iter().take(4) {
        prepare_case_with_action(&mut tk, *case, "on delete set null");
        tk.MustExec(
            "insert into t1 values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,null),(6,null,6),(7,null,null)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,1,1,'a'),(2,2,2,'b'),(3,3,3,'c'),(4,4,4,'d'),(5,5,null,'e'),(6,null,6,'f'),(7,null,null,'g')",
            Vec::new(),
        );
        tk.MustExec("delete from t1 where id=1 or a=2", Vec::new());
        tk.MustExec("delete from t1 where a in (2,3,4)", Vec::new());
        tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
            .Check(Rows(&[
                "1 <nil> <nil> a",
                "2 <nil> <nil> b",
                "3 <nil> <nil> c",
                "4 <nil> <nil> d",
                "5 5 <nil> e",
                "6 <nil> 6 f",
                "7 <nil> <nil> g",
            ]));

        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec("delete from t1", Vec::new());
        tk.MustExec("begin", Vec::new());
        tk.MustExec("insert into t1 values (1,1,1),(2,2,2)", Vec::new());
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,1,1,'a'),(2,2,2,'b')",
            Vec::new(),
        );
        tk.MustExec("delete from t1 where id=1", Vec::new());
        tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
            .Check(Rows(&["1 <nil> <nil> a", "2 2 2 b"]));
        assert_fk_error(&mut tk, "insert into t2 (id,a,b,name) values (11,1,1,'c')");
        tk.MustExec("insert into t1 values (1,1,1)", Vec::new());
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (11,1,1,'c')",
            Vec::new(),
        );
        tk.MustExec("delete from t1", Vec::new());
        tk.MustExec("commit", Vec::new());
        tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
            .Check(Rows(&[
                "1 <nil> <nil> a",
                "2 <nil> <nil> b",
                "11 <nil> <nil> c",
            ]));
    }

    reset_tables(&mut tk);
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int key,pid int,marker int,index unsafe_pid(pid) where marker is not null,foreign key fk_pid(pid) references t1(id) on delete set null)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1),(2)", Vec::new());
    tk.MustExec(
        "insert into t2 values (1,1,null),(2,2,1),(3,null,null)",
        Vec::new(),
    );
    tk.MustExec("delete from t1 where id=1", Vec::new());
    tk.MustQuery("select id,pid,marker from t2 order by id", Vec::new())
        .Check(Rows(&["1 <nil> <nil>", "2 2 1", "3 <nil> <nil>"]));
}

#[test]
fn TestForeignKeyOnDeleteSetNull2() {
    let mut tk = fk_testkit();
    tk.MustExec(
        "create table t1 (id int key,name varchar(10),leader int,index(leader),foreign key(leader) references t1(id) on delete set null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,'boss',null),(10,'l1_a',1),(11,'l1_b',1),(12,'l1_c',1),(100,'l2_a1',10),(110,'l2_b1',11),(1000,'l3_a1',100)",
        Vec::new(),
    );
    tk.MustExec("delete from t1 where id=11", Vec::new());
    tk.MustQuery("select id,name,leader from t1 order by id", Vec::new())
        .Check(Rows(&[
            "1 boss <nil>",
            "10 l1_a 1",
            "12 l1_c 1",
            "100 l2_a1 10",
            "110 l2_b1 <nil>",
            "1000 l3_a1 100",
        ]));
    tk.MustExec("delete from t1 where id=1", Vec::new());
    tk.MustQuery("select id,name,leader from t1 order by id", Vec::new())
        .Check(Rows(&[
            "10 l1_a <nil>",
            "12 l1_c <nil>",
            "100 l2_a1 10",
            "110 l2_b1 <nil>",
            "1000 l3_a1 100",
        ]));

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id varchar(10) key,name varchar(10),leader varchar(10),index(leader),foreign key(leader) references t1(id) on delete set null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values ('1','boss',null),('10','l1_a','1'),('11','l1_b','1'),('100','l2_a1','10'),('110','l2_b1','11')",
        Vec::new(),
    );
    tk.MustExec("delete from t1 where id='11'", Vec::new());
    tk.MustQuery("select id,leader from t1 order by id", Vec::new())
        .Check(Rows(&["1 <nil>", "10 1", "100 10", "110 <nil>"]));

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id int primary key,pid int,index(pid),foreign key(pid) references t1(id) on delete set null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (0,0),(1,0),(2,1),(3,2),(4,3),(5,4),(6,5),(7,6),(8,7),(9,8),(10,9),(11,10),(12,11),(13,12),(14,13),(15,14)",
        Vec::new(),
    );
    tk.MustExec("delete from t1 where id=0", Vec::new());
    tk.MustQuery("select id,pid from t1 order by id", Vec::new())
        .Check(Rows(&[
            "1 <nil>", "2 1", "3 2", "4 3", "5 4", "6 5", "7 6", "8 7", "9 8", "10 9", "11 10",
            "12 11", "13 12", "14 13", "15 14",
        ]));

    // SET NULL changes the child key. A grandchild RESTRICT edge therefore
    // rejects the statement and all three tables remain unchanged.
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int,foreign key(id) references t1(id) on delete set null)",
        Vec::new(),
    );
    tk.MustExec(
        "create table t3 (id int,foreign key(id) references t2(id))",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1)", Vec::new());
    tk.MustExec("insert into t2 values (1)", Vec::new());
    tk.MustExec("insert into t3 values (1)", Vec::new());
    assert_parent_error(&mut tk, "delete from t1 where id=1");
    for table in ["t1", "t2", "t3"] {
        tk.MustQuery(&format!("select * from {table}"), Vec::new())
            .Check(Rows(&["1"]));
    }

    // When one row matches both CASCADE and SET NULL, deletion wins; the
    // SET NULL mutation must not recreate the deleted row.
    tk.MustExec("drop table t3", Vec::new());
    tk.MustExec("drop table t2", Vec::new());
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id int key,pid int,ppid int,index(pid),index(ppid),foreign key fk_pid(pid) references t1(id) on delete cascade,foreign key fk_ppid(ppid) references t1(id) on delete set null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,null,null),(2,1,1),(3,1,1),(4,2,1)",
        Vec::new(),
    );
    tk.MustExec("delete from t1 where id=1", Vec::new());
    tk.MustQuery("select * from t1", Vec::new())
        .Check(Rows(&[]));

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec("create table t2 (id int key)", Vec::new());
    tk.MustExec(
        "create table t3 (id1 int,id2 int,foreign key fk_id1(id1) references t1(id) on delete set null,foreign key fk_id2(id2) references t2(id) on delete set null)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1),(2),(3)", Vec::new());
    tk.MustExec("insert into t2 values (1),(2),(3)", Vec::new());
    tk.MustExec(
        "insert into t3 values (1,1),(1,2),(1,3),(2,1),(2,2)",
        Vec::new(),
    );
    tk.MustExec("delete from t1 where id=1", Vec::new());
    tk.MustQuery("select * from t3 order by id1,id2", Vec::new())
        .Check(Rows(&["<nil> 1", "<nil> 2", "<nil> 3", "2 1", "2 2"]));
    tk.MustExec(
        "create table t4 (id3 int key,foreign key fk_id3(id3) references t3(id2))",
        Vec::new(),
    );
    tk.MustExec("insert into t4 values (2)", Vec::new());
    tk.MustExec("delete from t1 where id=2", Vec::new());
    assert_parent_error(&mut tk, "delete from t2 where id=2");
    tk.MustQuery("select * from t3 order by id1,id2", Vec::new())
        .Check(Rows(&[
            "<nil> 1", "<nil> 1", "<nil> 2", "<nil> 2", "<nil> 3",
        ]));

    tk.MustExec("drop table t4", Vec::new());
    tk.MustExec("drop table t3", Vec::new());
    tk.MustExec("drop table t2", Vec::new());
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id int key,name varchar(10),pid int,index(pid),foreign key fk(pid) references t1(id) on delete set null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,'boss',null),(2,'a',1),(3,'b',1),(4,'c',2)",
        Vec::new(),
    );
    let mut tk2 = NewTestKit(tk.Store());
    tk2.MustExec("set @@foreign_key_checks=1", Vec::new());
    tk2.MustExec("use test", Vec::new());
    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustExec("insert into t1 values (5,'d',3)", Vec::new());
    tk2.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("insert into t1 values (6,'e',4)", Vec::new());
    tk2.MustExec("delete from t1 where id=2", Vec::new());
    tk2.MustExec("commit", Vec::new());
    tk.MustExec("delete from t1 where id=1", Vec::new());
    tk.MustExec("commit", Vec::new());
    tk.MustQuery("select * from t1 order by id", Vec::new())
        .Check(Rows(&["3 b <nil>", "4 c <nil>", "5 d 3", "6 e 4"]));
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id int auto_increment primary key,b int)",
        Vec::new(),
    );
    tk.MustExec(
        "create table t2 (id int,b int,foreign key fk(id) references t1(id) on delete set null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 (b) values (1),(1),(1),(1),(1),(1),(1),(1)",
        Vec::new(),
    );
    for _ in 0..12 {
        tk.MustExec("insert into t1 (b) select b from t1", Vec::new());
    }
    tk.MustQuery("select count(*) from t1", Vec::new())
        .Check(Rows(&["32768"]));
    tk.MustExec("insert into t2 select * from t1", Vec::new());
    tk.MustExec("delete from t1", Vec::new());
    tk.MustQuery("select count(*) from t1", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("select count(*) from t2 where id is null", Vec::new())
        .Check(Rows(&["32768"]));
}

#[test]
fn TestForeignKeyOnUpdateCascade() {
    let mut tk = fk_testkit();
    for case in FOREIGN_KEY_CASES {
        prepare_case_with_action(&mut tk, *case, "on update cascade");
        tk.MustExec(
            "insert into t1 (id,a,b) values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,11,21,'a'),(2,12,22,'b'),(3,13,23,'c'),(4,14,24,'d')",
            Vec::new(),
        );
        tk.MustExec(
            "update t1 set a=a+100,b=b+200 where id in (1,2)",
            Vec::new(),
        );
        tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
            .Check(Rows(&[
                "1 111 221 a",
                "2 112 222 b",
                "3 13 23 c",
                "4 14 24 d",
            ]));
        if !case.not_null {
            tk.MustExec("update t1 set a=null,b=300 where id=3", Vec::new());
            tk.MustQuery("select id,a,b,name from t2 where id=3", Vec::new())
                .Check(Rows(&["3 <nil> 300 c"]));
        }

        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec("delete from t1", Vec::new());
        tk.MustExec("begin", Vec::new());
        tk.MustExec(
            "insert into t1 values (1,1,1),(2,2,2),(3,3,3),(4,4,4)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 (id,a,b,name) values (1,1,1,'a'),(2,2,2,'b'),(3,3,3,'c'),(4,4,4,'d')",
            Vec::new(),
        );
        tk.MustExec(
            "update t1 set a=a+100,b=b+200 where id in (1,2)",
            Vec::new(),
        );
        tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
            .Check(Rows(&["1 101 201 a", "2 102 202 b", "3 3 3 c", "4 4 4 d"]));
        tk.MustExec("rollback", Vec::new());
        tk.MustQuery("select * from t1", Vec::new())
            .Check(Rows(&[]));
        tk.MustQuery("select * from t2", Vec::new())
            .Check(Rows(&[]));
    }

    reset_tables(&mut tk);
    tk.MustExec("set @@tidb_enable_clustered_index=0", Vec::new());
    tk.MustExec(
        "create table t1 (id int primary key,a int,b int)",
        Vec::new(),
    );
    tk.MustExec(
        "create table t2 (b int,a int primary key,id int,name varchar(10),foreign key fk(a) references t1(id) on update cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,11,21),(2,12,22),(3,13,23),(4,14,24)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t2 values (21,1,11,'a'),(22,2,12,'b'),(23,3,13,'c'),(24,4,14,'d')",
        Vec::new(),
    );
    tk.MustExec("update t1 set id=id+100 where id in (1,2,3)", Vec::new());
    tk.MustQuery("select id,a,b from t1 order by id", Vec::new())
        .Check(Rows(&["4 14 24", "101 11 21", "102 12 22", "103 13 23"]));
    tk.MustQuery("select id,a,b,name from t2 order by id", Vec::new())
        .Check(Rows(&[
            "11 101 21 a",
            "12 102 22 b",
            "13 103 23 c",
            "14 4 24 d",
        ]));

    reset_tables(&mut tk);
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int key,pid int,marker int,index unsafe_pid(pid) where marker is not null,foreign key fk_pid(pid) references t1(id) on update cascade)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1),(2)", Vec::new());
    tk.MustExec(
        "insert into t2 values (1,1,null),(2,2,1),(3,null,null)",
        Vec::new(),
    );
    tk.MustExec("update t1 set id=10 where id=1", Vec::new());
    tk.MustQuery("select id,pid,marker from t2 order by id", Vec::new())
        .Check(Rows(&["1 10 <nil>", "2 2 1", "3 <nil> <nil>"]));
}

#[test]
fn TestForeignKeyOnUpdateCascade2() {
    let mut tk = fk_testkit();
    tk.MustExec("create table t1 (id int key,a int,index(a))", Vec::new());
    tk.MustExec(
        "create table t2 (id int key,pid int,foreign key(pid) references t1(a) on update cascade)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1,1),(2,1)", Vec::new());
    tk.MustExec("insert into t2 values (1,1),(2,1)", Vec::new());
    tk.MustExec("update t1 set a=id+1", Vec::new());
    tk.MustQuery("select id,a from t1 order by id", Vec::new())
        .Check(Rows(&["1 2", "2 3"]));
    tk.MustQuery("select id,pid from t2 order by id", Vec::new())
        .Check(Rows(&["1 2", "2 2"]));

    tk.MustExec("drop table t2", Vec::new());
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id int key,name varchar(10),leader int,index(leader),foreign key(leader) references t1(id) on update cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,'boss',null),(10,'l1_a',1),(11,'l1_b',1),(12,'l1_c',1),(100,'l2_a1',10),(110,'l2_b1',11),(1000,'l3_a1',100)",
        Vec::new(),
    );
    tk.MustExec("update t1 set id=id+10000 where id=11", Vec::new());
    tk.MustExec("update t1 set id=0 where id=1", Vec::new());
    tk.MustQuery("select id,name,leader from t1 order by id", Vec::new())
        .Check(Rows(&[
            "0 boss <nil>",
            "10 l1_a 0",
            "12 l1_c 0",
            "100 l2_a1 10",
            "110 l2_b1 10011",
            "1000 l3_a1 100",
            "10011 l1_b 0",
        ]));
    tk.MustExec("explain analyze update t1 set id=1 where id=10", Vec::new());
    tk.MustQuery("select id,leader from t1 where id in (1,100)", Vec::new())
        .Check(Rows(&["1 0", "100 1"]));

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec(
        "create table t1 (id varchar(100) key,name varchar(10),leader varchar(100),index(leader),foreign key(leader) references t1(id) on update cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values ('1','boss',null),('10','l1_a','1'),('11','l1_b','1'),('100','l2_a1','10'),('110','l2_b1','11')",
        Vec::new(),
    );
    tk.MustExec("update t1 set id=id+10000 where id='11'", Vec::new());
    tk.MustExec("update t1 set id='0' where id='1'", Vec::new());
    tk.MustQuery("select id,leader from t1 order by name", Vec::new())
        .Check(Rows(&["0 <nil>", "10 0", "10011 0", "100 10", "110 10011"]));

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t0 (id int,unique index(id))", Vec::new());
    tk.MustExec("insert into t0 values (1)", Vec::new());
    for index in 1..17 {
        tk.MustExec(
            &format!(
                "create table t{index} (id int,unique index(id),foreign key(id) references t{}(id) on update cascade)",
                index - 1
            ),
            Vec::new(),
        );
        tk.MustExec(&format!("insert into t{index} values (1)"), Vec::new());
    }
    tk.MustContainErrMsg("update t0 set id=10 where id=1", "cascade depth exceeded");
    tk.MustQuery("select id from t0", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("drop table t16", Vec::new());
    tk.MustExec("update t0 set id=10 where id=1", Vec::new());
    tk.MustQuery("select id from t15", Vec::new())
        .Check(Rows(&["10"]));

    for table in (0..=15).rev() {
        tk.MustExec(&format!("drop table t{table}"), Vec::new());
    }
    tk.MustExec(
        "create table t1 (id int auto_increment primary key,b int,index(b))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t2 (id int,b int,foreign key fk(b) references t1(b) on update cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 (b) values (1),(2),(3),(4),(5),(6),(7),(8)",
        Vec::new(),
    );
    for _ in 0..12 {
        tk.MustExec("insert into t1 (b) select id from t1", Vec::new());
    }
    tk.MustQuery("select count(*) from t1", Vec::new())
        .Check(Rows(&["32768"]));
    tk.MustExec("insert into t2 select * from t1", Vec::new());
    tk.MustExec("update t1 set b=2", Vec::new());
    tk.MustQuery(
        "select count(*) from t1 join t2 on t1.id=t2.id where t1.b=t2.b",
        Vec::new(),
    )
    .Check(Rows(&["32768"]));
}

#[test]
fn TestDMLExplainAnalyzeFKInfo() {
    let mut tk = fk_testkit();
    tk.MustExec("drop table if exists t1,t2,t3", Vec::new());
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec("create table t2 (id int key)", Vec::new());
    tk.MustExec(
        "create table t3 (id int key, id1 int, id2 int, constraint fk_id1 foreign key (id1) references t1 (id) on delete cascade, constraint fk_id2 foreign key (id2) references t2 (id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1),(2)", Vec::new());
    tk.MustExec("insert into t2 values (1)", Vec::new());

    // Go asserts the executor exposes the complete insert/FK-check runtime-stat
    // hierarchy, rather than merely returning a non-empty EXPLAIN row.
    let explain = tk
        .MustQuery(
            "explain analyze insert ignore into t3 values (1,1,1),(2,1,1),(3,2,1),(4,1,1),(5,2,1),(6,2,1)",
            Vec::new(),
        )
        .String();
    for field in ["prepare:", "check_insert:", "fk_check:"] {
        assert!(
            explain.contains(field),
            "missing {field} in foreign-key INSERT runtime stats: {explain}"
        );
    }

    let null_explain = tk
        .MustQuery(
            "explain analyze insert ignore into t3 values (7,null,null),(8,null,null)",
            Vec::new(),
        )
        .String();
    for field in ["prepare:", "check_insert:", "fk_check:"] {
        assert!(
            null_explain.contains(field),
            "missing {field} in NULL foreign-key INSERT runtime stats: {null_explain}"
        );
    }
}

#[test]
fn TestForeignKeyOnInsertOnDuplicateUpdate() {
    let mut tk = fk_testkit();
    tk.MustExec("create table t1 (id int key,name varchar(10))", Vec::new());
    tk.MustExec(
        "create table t2 (id int key,pid int,foreign key fk(pid) references t1(id) on update cascade on delete cascade)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1,'a'),(2,'b')", Vec::new());
    tk.MustExec(
        "insert into t2 values (1,1),(2,2),(3,1),(4,2),(5,null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,'aa') on duplicate key update name='aa'",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,'aaa') on duplicate key update id=10",
        Vec::new(),
    );
    tk.MustQuery("select * from t1 order by id", Vec::new())
        .Check(Rows(&["2 b", "10 aa"]));
    tk.MustQuery("select * from t2 order by id", Vec::new())
        .Check(Rows(&["1 10", "2 2", "3 10", "4 2", "5 <nil>"]));

    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t1 values (3,'c')", Vec::new());
    tk.MustExec("insert into t2 values (6,3)", Vec::new());
    tk.MustExec(
        "insert into t1 values (2,'bb'),(3,'cc') on duplicate key update id=id*10",
        Vec::new(),
    );
    tk.MustExec("commit", Vec::new());
    tk.MustQuery("select * from t1 order by id", Vec::new())
        .Check(Rows(&["10 aa", "20 b", "30 c"]));
    tk.MustQuery("select * from t2 order by id", Vec::new())
        .Check(Rows(&["1 10", "2 20", "3 10", "4 20", "5 <nil>", "6 30"]));
    tk.MustExec("delete from t1", Vec::new());
    tk.MustQuery("select * from t2", Vec::new())
        .Check(Rows(&["5 <nil>"]));

    tk.MustExec("drop table t2", Vec::new());
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int key,foreign key(id) references t1(id) on update cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "create table t3 (id int key,foreign key(id) references t2(id))",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t1 values (1)", Vec::new());
    tk.MustExec("insert into t2 values (1)", Vec::new());
    tk.MustExec("insert into t3 values (1)", Vec::new());
    assert_parent_error(
        &mut tk,
        "insert into t1 values (1) on duplicate key update id=2",
    );
    tk.MustExec("commit", Vec::new());
    tk.MustQuery("select * from t1", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("select * from t2", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("select * from t3", Vec::new())
        .Check(Rows(&["1"]));
}

#[test]
fn TestExplainAnalyzeDMLWithFKInfo() {
    let mut tk = fk_testkit();
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int key, foreign key fk(id) references t1(id) on update cascade on delete cascade)",
        Vec::new(),
    );
    tk.MustExec("create table t3 (id int, unique index idx(id))", Vec::new());
    tk.MustExec(
        "create table t4 (id int,index idx_id(id),foreign key fk(id) references t3(id))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t5 (id int key,id2 int,id3 int,unique index idx2(id2),index idx3(id3))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t6 (id int,id2 int,id3 int,index idx_id(id),index idx_id2(id2),foreign key fk_1(id) references t5(id) on update cascade on delete set null,foreign key fk_2(id2) references t5(id2) on update cascade,foreign key fk_3(id3) references t5(id3) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "create table t7 (id int primary key,pid int,index(pid),foreign key fk_1(pid) references t7(id) on delete cascade)",
        Vec::new(),
    );

    tk.MustExec("insert into t1 values (1),(2),(3),(4),(5)", Vec::new());
    let insert_pk = tk
        .MustQuery(
            "explain analyze insert into t2 values (1),(2),(3)",
            Vec::new(),
        )
        .String();
    for expected in [
        "Insert_",
        "Foreign_Key_Check_",
        "table:t1",
        "foreign_key:fk",
        "check_exist",
    ] {
        assert!(
            insert_pk.contains(expected),
            "missing {expected}: {insert_pk}"
        );
    }
    let update_child = tk
        .MustQuery("explain analyze update t2 set id=5 where id=3", Vec::new())
        .String();
    for expected in ["Update_", "Foreign_Key_Check_", "table:t1", "check_exist"] {
        assert!(
            update_child.contains(expected),
            "missing {expected}: {update_child}"
        );
    }
    let delete_parent = tk
        .MustQuery("explain analyze delete from t1 where id=4", Vec::new())
        .String();
    for expected in [
        "Delete_",
        "Foreign_Key_Cascade_",
        "table:t2",
        "on_delete:CASCADE",
    ] {
        assert!(
            delete_parent.contains(expected),
            "missing {expected}: {delete_parent}"
        );
    }

    tk.MustExec("insert into t3 values (1),(2),(3),(4),(5)", Vec::new());
    let insert_index = tk
        .MustQuery(
            "explain analyze insert into t4 values (1),(2),(3)",
            Vec::new(),
        )
        .String();
    for expected in [
        "Foreign_Key_Check_",
        "table:t3, index:idx",
        "foreign_key:fk",
    ] {
        assert!(
            insert_index.contains(expected),
            "missing {expected}: {insert_index}"
        );
    }

    tk.MustExec(
        "insert into t5 values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5)",
        Vec::new(),
    );
    let insert_multi = tk
        .MustQuery("explain analyze insert into t6 values (1,1,1)", Vec::new())
        .String();
    for expected in [
        "foreign_key:fk_1",
        "foreign_key:fk_2",
        "foreign_key:fk_3",
        "index:idx2",
        "index:idx3",
    ] {
        assert!(
            insert_multi.contains(expected),
            "missing {expected}: {insert_multi}"
        );
    }
    tk.MustExec("delete from t6", Vec::new());
    tk.MustExec("insert into t6 values (4,null,4)", Vec::new());
    let delete_multi = tk
        .MustQuery("explain analyze delete from t5 where id=4", Vec::new())
        .String();
    for expected in [
        "foreign_key:fk_1",
        "on_delete:SET NULL",
        "foreign_key:fk_3",
        "on_delete:CASCADE",
    ] {
        assert!(
            delete_multi.contains(expected),
            "missing {expected}: {delete_multi}"
        );
    }
    tk.MustExec("insert into t6 values (3,3,null)", Vec::new());
    let update_multi = tk
        .MustQuery(
            "explain analyze update t5 set id=30,id2=30 where id=3",
            Vec::new(),
        )
        .String();
    for expected in ["foreign_key:fk_1", "on_update:CASCADE", "foreign_key:fk_2"] {
        assert!(
            update_multi.contains(expected),
            "missing {expected}: {update_multi}"
        );
    }

    tk.MustExec("insert into t7 values (0,0),(1,0),(2,1),(3,2)", Vec::new());
    let self_cascade = tk
        .MustQuery("explain analyze delete from t7 where id=0", Vec::new())
        .String();
    for expected in [
        "Foreign_Key_Cascade_",
        "table:t7",
        "foreign_key:fk_1",
        "on_delete:CASCADE",
    ] {
        assert!(
            self_cascade.contains(expected),
            "missing {expected}: {self_cascade}"
        );
    }
}

#[test]
fn TestForeignKeyRuntimeStats() {
    let mut check = FKCheckRuntimeStats {
        Total: Duration::from_secs(3),
        Check: Duration::from_secs(2),
        Lock: Duration::from_secs(1),
        Keys: 10,
    };
    assert_eq!(
        check.String(),
        "total:3s, check:2s, lock:1s, foreign_keys:10"
    );
    check.Merge(&check.Clone());
    assert_eq!(
        check.String(),
        "total:6s, check:4s, lock:2s, foreign_keys:20"
    );

    let mut cascade = FKCascadeRuntimeStats {
        Total: Duration::from_secs(1),
        Keys: 10,
    };
    assert_eq!(cascade.String(), "total:1s, foreign_keys:10");
    cascade.Merge(&cascade.Clone());
    assert_eq!(cascade.String(), "total:2s, foreign_keys:20");
}

#[test]
fn TestPrivilegeCheckInForeignKeyCascade() {
    let store = create_fk_store();
    let mut admin = new_fk_testkit(store.clone());
    let mut user = new_fk_testkit(store);
    admin.MustExec("set @@global.tidb_enable_foreign_key=1", Vec::new());
    admin.MustExec("set @@foreign_key_checks=1", Vec::new());
    admin.MustExec("use test", Vec::new());
    user.MustExec("set @@foreign_key_checks=1", Vec::new());
    user.MustExec("use test", Vec::new());
    admin.MustExec("create table t1 (id int key)", Vec::new());
    admin.MustExec(
        "create table t2 (id int key, foreign key fk(id) references t1(id) on delete cascade on update cascade)",
        Vec::new(),
    );
    admin.MustExec("insert into t1 values (1),(2),(3)", Vec::new());

    for (grant, sql, t1_rows, t2_rows) in [
        (
            "grant insert on test.t2 to 'u1'@'%'",
            "insert into t2 values (1),(2),(3)",
            &["1", "2", "3"][..],
            &["1", "2", "3"][..],
        ),
        (
            "grant select,delete on test.t1 to 'u1'@'%'",
            "delete from t1 where id=1",
            &["2", "3"][..],
            &["2", "3"][..],
        ),
        (
            "grant select,update on test.t1 to 'u1'@'%'",
            "update t1 set id=id+10 where id=2",
            &["3", "12"][..],
            &["3", "12"][..],
        ),
    ] {
        admin.MustExec("drop user if exists 'u1'@'%'", Vec::new());
        admin.MustExec("create user 'u1'@'%' identified by ''", Vec::new());
        admin.MustExec(grant, Vec::new());
        user.Session()
            .AuthenticateUserForTest(&UserIdentity {
                username: "u1".to_owned(),
                hostname: "localhost".to_owned(),
                ..Default::default()
            })
            .expect("authenticate foreign-key cascade user");
        user.MustExec(sql, Vec::new());
        admin
            .MustQuery("select * from t1 order by id", Vec::new())
            .Check(Rows(t1_rows));
        admin
            .MustQuery("select * from t2 order by id", Vec::new())
            .Check(Rows(t2_rows));
    }
}

#[test]
fn TestForeignKeyIssue39732() {
    let mut tk = fk_testkit();
    tk.MustExec("set @@global.tidb_enable_stmt_summary=1", Vec::new());
    tk.MustExec(
        "create table t1 (id int key,leader int,index(leader),foreign key(leader) references t1(id) on delete cascade)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values (1,null),(10,1),(11,1),(20,10)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare stmt1 from 'delete from t1 where id = ?'",
        Vec::new(),
    );
    tk.MustExec("set @a=1", Vec::new());
    tk.MustExec("execute stmt1 using @a", Vec::new());
    tk.MustQuery("select * from t1", Vec::new())
        .Check(Rows(&[]));
}

#[test]
fn TestForeignKeyOnReplaceIntoChildTable() {
    let mut tk = fk_testkit();
    tk.MustExec("create table t_data (id int,a int,b int)", Vec::new());
    tk.MustExec("insert into t_data values (1,1,1),(2,2,2)", Vec::new());
    for case in FOREIGN_KEY_CASES {
        prepare_case(&mut tk, *case);
        tk.MustExec("replace into t1 (id,a,b) values (1,1,1)", Vec::new());
        tk.MustExec("replace into t2 (id,a,b) values (1,1,1)", Vec::new());
        assert_parent_error(&mut tk, "replace into t1 (id,a,b) values (1,2,3)");
        if !case.not_null {
            tk.MustExec("replace into t2 (id,a,b) values (2,null,1)", Vec::new());
            tk.MustExec("replace into t2 (id,a,b) values (3,1,null)", Vec::new());
            tk.MustExec("replace into t2 (id,a,b) values (4,null,null)", Vec::new());
        }
        for sql in [
            "replace into t2 (id,a,b) values (5,1,0)",
            "replace into t2 (id,a,b) values (6,0,1)",
            "replace into t2 (id,a,b) values (7,2,2)",
        ] {
            assert_fk_error(&mut tk, sql);
        }
        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec(
            "replace into t2 (id,a,b) select id,a,b from t_data where id=1",
            Vec::new(),
        );
        assert_fk_error(
            &mut tk,
            "replace into t2 (id,a,b) select id,a,b from t_data where id=2",
        );

        tk.MustExec("delete from t2", Vec::new());
        tk.MustExec("begin", Vec::new());
        tk.MustExec("delete from t1 where a=1", Vec::new());
        assert_fk_error(&mut tk, "replace into t2 (id,a,b) values (1,1,1)");
        tk.MustExec("replace into t1 values (2,2,2)", Vec::new());
        tk.MustExec("replace into t2 (id,a,b) values (2,2,2)", Vec::new());
        assert_parent_error(&mut tk, "replace into t1 values (2,2,3)");
        tk.MustExec("rollback", Vec::new());
        tk.MustQuery("select id,a,b from t1", Vec::new())
            .Check(Rows(&["1 1 1"]));
        tk.MustQuery("select * from t2", Vec::new())
            .Check(Rows(&[]));
    }
}

#[test]
fn TestForeignKeyLargeTxnErr() {
    let mut tk = fk_testkit();
    tk.MustExec(
        "create table t1 (id int auto_increment key,pid int,name varchar(200),index(pid))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 (name) values ('abcdefghijklmnopqrstuvwxyz1234567890abcdefghijklmnopqrstuvwxyz1234567890abcdefghijklmnopqrstuvwxyz1234567890abcdefghijklmnopqrstuvwxyz1234567890abcdefghijklmnopqrstuvwxyz1234567890')",
        Vec::new(),
    );
    for _ in 0..8 {
        tk.MustExec("insert into t1 (name) select name from t1", Vec::new());
    }
    tk.MustQuery("select count(*) from t1", Vec::new())
        .Check(Rows(&["256"]));
    tk.MustExec("update t1 set pid=1 where id>1", Vec::new());
    tk.MustExec(
        "alter table t1 add foreign key(pid) references t1(id) on update cascade",
        Vec::new(),
    );
    tk.MustQuery("select sum(id) from t1", Vec::new())
        .Check(Rows(&["32896"]));
    let size_limit_lock = TXN_SIZE_LIMIT_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let original = astersql_kv::TxnTotalSizeLimit.swap(10_240, Ordering::SeqCst);
    let _limit = TxnSizeLimitGuard(original);
    tk.MustContainErrMsg(
        "update t1 set id=id+100000 where id=1",
        "Transaction is too large",
    );
    // Rust's test harness runs package tests concurrently, unlike Go tests
    // without t.Parallel. Restore the process-wide limit immediately after
    // the assertion so unrelated sessions cannot inherit the tiny test value.
    drop(_limit);
    drop(size_limit_lock);
    tk.MustQuery("select sum(id) from t1", Vec::new())
        .Check(Rows(&["32896"]));
    tk.MustExec("set @@foreign_key_checks=0", Vec::new());
    tk.MustExec("update t1 set id=id+100000 where id=1", Vec::new());
    tk.MustQuery(
        "select id,pid from t1 where id<3 or pid is null order by id",
        Vec::new(),
    )
    .Check(Rows(&["2 1", "100001 <nil>"]));
}

#[test]
fn TestForeignKeyAndLockView() {
    let store = create_fk_store();
    let mut tk = new_fk_testkit(store.clone());
    tk.MustExec("set @@global.tidb_enable_foreign_key=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int key,foreign key(id) references t1(id) on delete cascade on update cascade)",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1)", Vec::new());
    tk.MustExec("insert into t2 values (1)", Vec::new());
    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustExec("set @@foreign_key_checks=0", Vec::new());
    tk.MustExec("update t2 set id=2", Vec::new());

    let (ready_tx, ready_rx) = mpsc::sync_channel(0);
    let worker = thread::spawn(move || {
        let mut tk2 = new_fk_testkit(store);
        tk2.MustExec("set @@foreign_key_checks=1", Vec::new());
        tk2.MustExec("use test", Vec::new());
        tk2.MustExec("begin pessimistic", Vec::new());
        ready_tx.send(()).expect("signal lock waiter");
        tk2.MustExec("update t1 set id=2 where id=1", Vec::new());
        tk2.MustExec("commit", Vec::new());
    });
    ready_rx.recv().expect("lock waiter ready");
    thread::sleep(Duration::from_millis(200));
    let (_, digest) = NormalizeDigest("update t1 set id=2 where id=1");
    tk.MustQuery(
        "select current_sql_digest from information_schema.tidb_trx where state='LockWaiting' and db='test'",
        Vec::new(),
    )
    .Check(Rows(&[&digest.String()]));
    let error = tk.ExecToErr("update t1 set id=2");
    assert!(
        error
            .to_string()
            .contains("Deadlock found when trying to get lock"),
        "unexpected deadlock error: {error}"
    );
    worker.join().expect("foreign-key deadlock waiter");
}

#[test]
fn TestLockKeysInDML() {
    assert_insert_holds_parent_fk_lock("insert into t2 values (1)", false);
}

#[test]
fn TestLockKeysInInsertIgnore() {
    assert_insert_holds_parent_fk_lock("insert ignore into t2 values (1)", true);
}

fn assert_insert_holds_parent_fk_lock(insert_sql: &str, disable_fair_locking: bool) {
    let store = create_fk_store();
    let mut tk = new_fk_testkit(store.clone());
    tk.MustExec("set @@global.tidb_enable_foreign_key=1", Vec::new());
    tk.MustExec("set @@foreign_key_checks=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t1 (id int primary key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int primary key,foreign key fk(id) references t1(id))",
        Vec::new(),
    );
    tk.MustExec("insert into t1 values (1)", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec(insert_sql, Vec::new());

    let (ready_tx, ready_rx) = mpsc::sync_channel(0);
    let worker = thread::spawn(move || {
        let mut tk2 = new_fk_testkit(store);
        if disable_fair_locking {
            tk2.MustExec("set tidb_pessimistic_txn_fair_locking='OFF'", Vec::new());
        }
        tk2.MustExec("set @@foreign_key_checks=1", Vec::new());
        tk2.MustExec("use test", Vec::new());
        tk2.MustExec("begin", Vec::new());
        let started = std::time::Instant::now();
        ready_tx.send(()).expect("signal parent update start");
        let error = tk2.ExecToErr("update t1 set id=2 where id=1");
        tk2.MustExec("commit", Vec::new());
        (started.elapsed(), error.to_string())
    });
    let wait = Duration::from_millis(500);
    ready_rx.recv().expect("parent update worker ready");
    thread::sleep(wait);
    tk.MustExec("commit", Vec::new());
    let (elapsed, error) = worker.join().expect("foreign-key lock waiter");
    assert!(elapsed >= wait, "parent update did not wait: {elapsed:?}");
    assert!(
        error.contains(PARENT_ERROR),
        "unexpected lock error: {error}"
    );
    tk.MustQuery("select * from t1", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("select * from t2", Vec::new())
        .Check(Rows(&["1"]));
}

#[test]
fn TestFKBuild() {
    let mut tk = fk_testkit();
    tk.MustExec("create table t1 (id int key)", Vec::new());
    tk.MustExec(
        "create table t2 (id int key, foreign key fk (id) references t1(id) on delete cascade on update cascade)",
        Vec::new(),
    );

    let _guard = enable_failpoint(
        "github.com/pingcap/tidb/pkg/infoschema/issyncer/MockTryLoadDiffError",
        "return(\"renametable\")",
    );
    tk.MustExec("rename table t1 to t3", Vec::new());
    tk.MustExec("insert into test.t3 values (1)", Vec::new());
    tk.MustExec("insert into test.t2 values (1)", Vec::new());
    tk.MustExec("delete from test.t3", Vec::new());
    tk.MustQuery("select * from test.t2", Vec::new())
        .Check(Rows(&[]));
}
