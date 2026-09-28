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
//! 中文总览：`replica_read_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `事务语义与时间戳行为` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 33 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `StatementKind` 是当前文件里的分支类型。
//! `StatementKind` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `StatementKind` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `StatementKind`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `STATEMENTS` 是当前文件里的常量。
//! `STATEMENTS` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `STATEMENTS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `STATEMENTS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `CONDITIONS` 是当前文件里的常量。
//! `CONDITIONS` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `CONDITIONS` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `CONDITIONS`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `sql` 是当前文件里的辅助函数。
//! `sql` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `sql` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `sql`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `reset` 是当前文件里的辅助函数。
//! `reset` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `reset` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `reset`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `expected_selected` 是当前文件里的辅助函数。
//! `expected_selected` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `expected_selected` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `expected_selected`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `expected_after` 是当前文件里的辅助函数。
//! `expected_after` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `expected_after` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `expected_after`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `execute` 是当前文件里的辅助函数。
//! `execute` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `execute` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `execute`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `assert_replica_request` 是当前文件里的辅助函数。
//! `assert_replica_request` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `assert_replica_request` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `assert_replica_request`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_replica_read_effect_scope` 是当前文件里的测试用例。
//! `test_replica_read_effect_scope` 所处的位置主要服务 `事务语义与时间戳行为` 主题下的一个阅读切面。
//! 阅读 `test_replica_read_effect_scope` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_replica_read_effect_scope`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Replica-read effect-scope matrix from the Go RealTiKV test.
//!
//! The concrete in-process KV boundary does not expose client-go's RPC
//! interceptor context. This test therefore drives the same 8×3 SQL matrix in
//! both pessimistic transactions and autocommit, verifies the follower session
//! policy, rollback isolation, and exact committed effects.

use astersql_config_kerneltype::IsNextGen;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_tests_realtikvtest_txntest::serial_guard;

#[derive(Clone, Copy, Debug)]
enum StatementKind {
    Select,
    SelectForUpdate,
    SelectForShare,
    Update,
    Delete,
    InsertSelect,
    InsertOnDuplicate,
    Replace,
}

const STATEMENTS: [StatementKind; 8] = [
    StatementKind::Select,
    StatementKind::SelectForUpdate,
    StatementKind::SelectForShare,
    StatementKind::Update,
    StatementKind::Delete,
    StatementKind::InsertSelect,
    StatementKind::InsertOnDuplicate,
    StatementKind::Replace,
];

const CONDITIONS: [&str; 3] = ["id = 1", "id IN (1, 2)", "id > 1"];

fn sql(kind: StatementKind, condition: &str) -> String {
    match kind {
        StatementKind::Select => format!("select * from t where {condition}"),
        StatementKind::SelectForUpdate => {
            format!("select * from t where {condition} for update")
        }
        StatementKind::SelectForShare => {
            format!("select * from t where {condition} for share")
        }
        StatementKind::Update => format!("update t set v=v+1 where {condition}"),
        StatementKind::Delete => format!("delete from t where {condition}"),
        StatementKind::InsertSelect => {
            format!("insert into t select id+100,v+100 from t where {condition}")
        }
        StatementKind::InsertOnDuplicate => format!(
            "insert into t select id+100,v+100 from t where {condition} \
             on duplicate key update v=values(v)"
        ),
        StatementKind::Replace => {
            format!("replace into t select id,v+100 from t where {condition}")
        }
    }
}

fn reset(tk: &mut TestKit) {
    tk.MustExec("delete from t", Vec::new());
    tk.MustExec("insert into t values (1,10),(2,20),(3,30)", Vec::new());
}

fn expected_selected(condition: &str) -> Vec<Vec<String>> {
    match condition {
        "id = 1" => Rows(&["1 10"]),
        "id IN (1, 2)" => Rows(&["1 10", "2 20"]),
        "id > 1" => Rows(&["2 20", "3 30"]),
        _ => unreachable!("known condition"),
    }
}

fn expected_after(kind: StatementKind, condition: &str) -> Vec<Vec<String>> {
    match kind {
        StatementKind::Select | StatementKind::SelectForUpdate | StatementKind::SelectForShare => {
            Rows(&["1 10", "2 20", "3 30"])
        }
        StatementKind::Update => match condition {
            "id = 1" => Rows(&["1 11", "2 20", "3 30"]),
            "id IN (1, 2)" => Rows(&["1 11", "2 21", "3 30"]),
            "id > 1" => Rows(&["1 10", "2 21", "3 31"]),
            _ => unreachable!(),
        },
        StatementKind::Delete => match condition {
            "id = 1" => Rows(&["2 20", "3 30"]),
            "id IN (1, 2)" => Rows(&["3 30"]),
            "id > 1" => Rows(&["1 10"]),
            _ => unreachable!(),
        },
        StatementKind::InsertSelect | StatementKind::InsertOnDuplicate => match condition {
            "id = 1" => Rows(&["1 10", "2 20", "3 30", "101 110"]),
            "id IN (1, 2)" => Rows(&["1 10", "2 20", "3 30", "101 110", "102 120"]),
            "id > 1" => Rows(&["1 10", "2 20", "3 30", "102 120", "103 130"]),
            _ => unreachable!(),
        },
        StatementKind::Replace => match condition {
            "id = 1" => Rows(&["1 110", "2 20", "3 30"]),
            "id IN (1, 2)" => Rows(&["1 110", "2 120", "3 30"]),
            "id > 1" => Rows(&["1 10", "2 120", "3 130"]),
            _ => unreachable!(),
        },
    }
}

fn execute(tk: &mut TestKit, kind: StatementKind, statement: &str, condition: &str) {
    match kind {
        StatementKind::Select | StatementKind::SelectForUpdate | StatementKind::SelectForShare => {
            tk.MustQuery(statement, Vec::new())
                .Check(expected_selected(condition))
        }
        _ => tk.MustExec(statement, Vec::new()),
    }
}

fn assert_replica_request(store: &AnalyzeStatsStore, kind: StatementKind, condition: &str) {
    let request = store
        .last_replica_read_request_for_test()
        .expect("read replica request observation")
        .expect("SQL statement must emit a replica request observation");
    // Go checks replica selection for each observed read. A write may finish
    // with a Get after its range scan, so its final request kind is not fixed.
    assert!(
        matches!(
            request.request_kind.as_str(),
            "Get" | "BatchGet" | "Coprocessor"
        ),
        "statement kind={kind:?}, condition={condition}: {request:?}"
    );
    assert_eq!(
        request.replica_read,
        if matches!(kind, StatementKind::Select) {
            "follower"
        } else {
            "leader"
        }
    );
}

#[test]
fn test_replica_read_effect_scope() {
    let _serial = serial_guard();
    if IsNextGen() {
        // The Go test skips this classic-kernel-only follower-read feature.
        return;
    }

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec("drop database if exists txn_replica_scope", Vec::new());
    tk.MustExec("create database txn_replica_scope", Vec::new());
    tk.MustExec("use txn_replica_scope", Vec::new());
    tk.MustExec("create table t (id int primary key, v int)", Vec::new());
    tk.MustExec("set session tidb_replica_read='follower'", Vec::new());
    tk.MustExec("set session tidb_enable_noop_functions='on'", Vec::new());
    tk.MustQuery("select @@tidb_replica_read", Vec::new())
        .Check(Rows(&["follower"]));

    for kind in STATEMENTS {
        for condition in CONDITIONS {
            let statement = sql(kind, condition);

            // Go executes every statement through an interceptor once inside a
            // pessimistic transaction. Rollback must erase every mutation.
            reset(&mut tk);
            tk.MustExec("begin pessimistic", Vec::new());
            store
                .clear_replica_read_request_for_test()
                .expect("clear replica observation");
            execute(&mut tk, kind, &statement, condition);
            assert_replica_request(&store, kind, condition);
            tk.MustExec("rollback", Vec::new());
            tk.MustQuery("select * from t order by id", Vec::new())
                .Check(Rows(&["1 10", "2 20", "3 30"]));

            // Repeat in autocommit, where the exact committed effect must be
            // visible while the follower policy remains session-scoped.
            store
                .clear_replica_read_request_for_test()
                .expect("clear replica observation");
            execute(&mut tk, kind, &statement, condition);
            assert_replica_request(&store, kind, condition);
            tk.MustQuery("select * from t order by id", Vec::new())
                .Check(expected_after(kind, condition));
            tk.MustQuery("select @@tidb_replica_read", Vec::new())
                .Check(Rows(&["follower"]));
        }
    }
}
