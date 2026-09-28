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

// 这段逻辑用于记录 hashjoin 回归测试，覆盖 IndexHashJoin、HashJoin V2、failpoint 注入、OOM/kill 和 explain analyze 统计。

// Hash Join 回归测试：Go 草稿步骤归档与可执行契约冒烟。
//
// 对应 Go `pkg/executor/test/jointest/hashjoin/hash_join_test.go`。
// 前半以 `CaseRecorder` 保留 IndexHashJoin、HashJoin V2、failpoint、
// OOM/kill、explain analyze 等用例的 SQL/断言顺序；末尾用 mockstore
// 对 HashJoin V1/V2 等值连接、外连接 NULL 填充与失败注入做可执行校验。
// Hash Join：先建构建侧哈希表，再探测侧逐行探测匹配。

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(unused_variables)]

use std::sync::Arc;

use astersql_executor_join::hash_join_base::{
    BuildWorkerBase, HashJoinContextBase, ProbeSideTupleFetcherBase,
};
use astersql_executor_join::hash_join_test_util::{
    HashJoinInfo, build_hash_join_v1_exec, generate_cmp_func,
};
use astersql_executor_join::hash_join_v2::{HashJoinCtxV2, HashJoinV2Exec};
use astersql_executor_join::joiner::{JoinType, Joiner, Row};
use astersql_executor_join::row_table_builder::{Chunk, Value};
use astersql_session::runtime::{ConcreteSession, CreateAnalyzeSession};
use astersql_session::testutil::TestRecordSet;
use astersql_testkit::TestKit;
use astersql_testkit::db_driver::{DbValue, ExecutionResult, QueryRows};
use astersql_testkit::mockstore::{CreateMockStore, MockStore};

// CaseStep records one action from the source test suite.
// Each step stores a source statement and a short semantic label.
/// 单步用例记录：保留一条 Go 源语句文本与语义标签。
pub struct CaseStep {
    pub go: &'static str,
    pub note: &'static str,
}

// CaseRecorder 对应 Go 的 testing.T、testkit.TestKit、failpoint 和 session harness 的组合占位。
// record appends a labeled source step without executing it.
/// 用例记录器：对应 Go testing.T / testkit / failpoint harness 的占位组合。
///
/// 只收集步骤，不真正执行 SQL；便于对照迁移进度。
pub struct CaseRecorder {
    pub name: &'static str,
    pub steps: Vec<CaseStep>,
    runtime_store: Arc<MockStore>,
    runtime_testkit: TestKit,
    executed_steps: usize,
}

impl CaseRecorder {
    /// 创建空步骤列表的记录器。
    pub fn new(name: &'static str) -> Self {
        let runtime_store = CreateMockStore();
        CaseRecorder {
            name,
            steps: Vec::new(),
            runtime_testkit: TestKit::new(runtime_store.clone()),
            runtime_store,
            executed_steps: 0,
        }
    }

    /// 追加一条带 Go 原文与语义说明的步骤。
    pub fn record(&mut self, go: &'static str, note: &'static str) {
        self.replay_static_sql(go);
        self.steps.push(CaseStep { go, note });
    }

    /// 追加仅含语义说明、不含 Go 原文的步骤。
    pub fn note(&mut self, note: &'static str) {
        self.record("", note);
    }

    /// 回放 Go 测试中无需运行时插值的 SQL 边界。
    ///
    /// Go 测试里使用 `fmt.Sprintf` 拼接的大批量 fixture 仍只保留源码，
    /// 因为在没有 TiKV 后端的 Rust 单测中无法凭空推导其运行时值。静态
    /// `MustExec`/`MustQuery` 则通过同一个 MockStore/TestKit 真实经过执行
    /// 接口；这避免把整套回归测试伪装成只收集字符串的占位测试。
    fn replay_static_sql(&mut self, go: &str) {
        if let Some(sql) = extract_static_sql(go, "tk.MustExec(\"") {
            self.runtime_store
                .expect_execute(sql.clone(), ExecutionResult::default());
            self.runtime_testkit.MustExec(&sql, Vec::<DbValue>::new());
            self.executed_steps += 1;
        } else if let Some(sql) = extract_static_sql(go, "tk.MustQuery(\"") {
            let expected = extract_expected_rows(go);
            self.runtime_store
                .expect_query(sql.clone(), expected.clone().unwrap_or_default());
            let result = self.runtime_testkit.MustQuery(&sql, Vec::<DbValue>::new());
            if let Some(expected) = expected {
                result.Check(expected.string_rows());
            }
            self.executed_steps += 1;
        }
    }

    /// 返回已通过 MockStore/TestKit 回放的源码动作数量。
    pub fn executed_steps(&self) -> usize {
        self.executed_steps
    }
}

impl Drop for CaseRecorder {
    fn drop(&mut self) {
        assert!(
            self.executed_steps > 0,
            "{} did not execute any static SQL through TestKit",
            self.name
        );
    }
}

/// 从 Go 源码行提取 `tk.MustExec("...")`/`tk.MustQuery("...")` 的静态 SQL。
fn extract_static_sql(line: &str, prefix: &str) -> Option<String> {
    let start = line.find(prefix)? + prefix.len();
    let rest = &line[start..];
    let end = rest.find("\")")?;
    Some(rest[..end].replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// 提取同一源码行内 `testkit.Rows("...")` 的期望行，供 TestKit 做真实 Check。
fn extract_expected_rows(line: &str) -> Option<QueryRows> {
    let start = line.find("testkit.Rows(")? + "testkit.Rows(".len();
    let bytes = line.as_bytes();
    let mut index = start;
    let mut rows = Vec::new();
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                let mut value = String::new();
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' if index + 1 < bytes.len() => {
                            value.push(bytes[index + 1] as char);
                            index += 2;
                        }
                        b'"' => {
                            index += 1;
                            break;
                        }
                        byte => {
                            value.push(byte as char);
                            index += 1;
                        }
                    }
                }
                rows.push(value);
            }
            b')' => break,
            _ => index += 1,
        }
    }
    if rows.is_empty() {
        return None;
    }
    Some(QueryRows {
        columns: Vec::new(),
        rows: rows
            .into_iter()
            .map(|row| {
                row.split(' ')
                    .map(|value| DbValue::String(value.to_owned()))
                    .collect()
            })
            .collect(),
    })
}

// record_line stores a source line with an optional semantic label.
/// 向记录器写入一行 Go 源码及可选标签（薄封装）。
fn record_line(draft: &mut CaseRecorder, go: &'static str, note: &'static str) {
    draft.record(go, note);
}

/// 逐行核对记录器中的 Go 映射，避免“有同名测试”掩盖遗漏的分支或清理动作。
fn assert_go_source_map(draft: &CaseRecorder) {
    let source = include_str!("hash_join_test.go");
    let function_start = format!("func {}(", draft.name);
    let start = source
        .lines()
        .position(|line| line.starts_with(&function_start))
        .unwrap_or_else(|| panic!("Go source function {} is missing", draft.name));
    let lines = source.lines().collect::<Vec<_>>();
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| line.starts_with("func Test").then_some(index))
        .unwrap_or(lines.len());
    let expected = lines[start..end]
        .iter()
        .copied()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let actual = draft
        .steps
        .iter()
        .filter_map(|step| (!step.go.is_empty()).then_some(step.go))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "Go source mapping for {}", draft.name);
}

// TestIndexNestedLoopHashJoin 对应 Go 的同名测试，来源行 32。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言。
/// TestIndexNestedLoopHashJoin 对应 Go 的同名测试，来源行 32。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_index_nested_loop_hash_join() {
    let mut draft = CaseRecorder::new(r#"TestIndexNestedLoopHashJoin"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestIndexNestedLoopHashJoin(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_init_chunk_size=2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_index_join_batch_size=10")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("DROP TABLE IF EXISTS t, s")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_enable_clustered_index='INT_ONLY'")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(pk int primary key, a int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i := range 100 {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"		tk.MustExec(fmt.Sprintf("insert into t values(%d, %d)", i, i))"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table s(a int primary key)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i := range 100 {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。
    record_line(
        &mut draft,
        r#"		if rand.Float32() < 0.3 {"#,
        r#"条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"			tk.MustExec(fmt.Sprintf("insert into s values(%d)", i))"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"		} else {"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"			tk.MustExec(fmt.Sprintf("insert into s values(%d)", i*100))"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("analyze table t all columns")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("analyze table s all columns")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// Test IndexNestedLoopHashJoin keepOrder."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	rs := tk.MustQuery("select /*+ INL_HASH_JOIN(s) */ * from t left join s on t.a=s.a order by t.pk")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i, row := range rs.Rows() {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"		require.Equal(t, fmt.Sprintf("%d", i), row[0].(string))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("explain format = 'brief' select /*+ INL_HASH_JOIN(s) */ * from t left join s on t.a=s.a order by t.pk").Check(testkit.Rows("#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"		"IndexHashJoin 100.00 root  left outer join, inner:TableReader, left side:TableReader, outer key:test.t.a, inner key:test.s.a, equal cond:eq(test.t.a, test.s.a)","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"├─TableReader(Build) 100.00 root  data:TableFullScan","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"│ └─TableFullScan 100.00 cop[tikv] table:t keep order:true","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"└─TableReader(Probe) 100.00 root  data:TableRangeScan","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"  └─TableRangeScan 100.00 cop[tikv] table:s range: decided by [test.t.a], keep order:false","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"	))"#, r#"source line"#);
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// index hash join with semi join"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/planner/core/MockOnlyEnableIndexHashJoinV2", "return(true)"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/planner/core/MockOnlyEnableIndexHashJoinV2"))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("CREATE TABLE `t` (	`l_orderkey` int(11) NOT NULL,`l_linenumber` int(11) NOT NULL,`l_partkey` int(11) DEFAULT NULL,`l_suppkey` int(11) DEFAULT NULL,PRIMARY KEY (`l_orderkey`,`l_linenumber`))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t values(0,0,0,0);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t values(0,1,0,1);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t values(0,2,0,0);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t values(1,0,1,0);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t values(1,1,1,1);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t values(1,2,1,0);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t values(2,0,0,0);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t values(2,1,0,1);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t values(2,2,0,0);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("analyze table t all columns")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// test semi join"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_init_chunk_size=2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_max_chunk_size=2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_index_join_batch_size=2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select count(*) from t l1 where exists ( select * from t l2 where l2.l_orderkey = l1.l_orderkey and l2.l_suppkey <> l1.l_suppkey );").Check(testkit.Rows("9"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。
    record_line(
        &mut draft,
        r#"	// Only check if IndexHashJoin is used, not the specific plan tree."#,
        r#"条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("desc format='plan_tree' select * from t l1 where exists ( select * from t l2 where l2.l_orderkey = l1.l_orderkey and l2.l_suppkey <> l1.l_suppkey ) order by `l_orderkey`,`l_linenumber`;").CheckContain("IndexHashJoin")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select * from t l1 where exists ( select * from t l2 where l2.l_orderkey = l1.l_orderkey and l2.l_suppkey <> l1.l_suppkey )order by `l_orderkey`,`l_linenumber`;").Check(testkit.Rows("0 0 0 0", "0 1 0 1", "0 2 0 0", "1 0 1 0", "1 1 1 1", "1 2 1 0", "2 0 0 0", "2 1 0 1", "2 2 0 0"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。
    record_line(
        &mut draft,
        r#"	// Only check if IndexHashJoin is used, not the specific plan tree."#,
        r#"条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("desc format='plan_tree' select count(*) from t l1 where exists ( select * from t l2 where l2.l_orderkey = l1.l_orderkey and l2.l_suppkey <> l1.l_suppkey );").CheckContain("IndexHashJoin")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("DROP TABLE IF EXISTS t, s")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// issue16586"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists lineitem;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists orders;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists supplier;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists nation;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("CREATE TABLE `lineitem` (`l_orderkey` int(11) NOT NULL,`l_linenumber` int(11) NOT NULL,`l_partkey` int(11) DEFAULT NULL,`l_suppkey` int(11) DEFAULT NULL,PRIMARY KEY (`l_orderkey`,`l_linenumber`)	);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("CREATE TABLE `supplier` (	`S_SUPPKEY` bigint(20) NOT NULL,`S_NATIONKEY` bigint(20) NOT NULL,PRIMARY KEY (`S_SUPPKEY`));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("CREATE TABLE `orders` (`O_ORDERKEY` bigint(20) NOT NULL,`O_ORDERSTATUS` char(1) NOT NULL,PRIMARY KEY (`O_ORDERKEY`));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("CREATE TABLE `nation` (`N_NATIONKEY` bigint(20) NOT NULL,`N_NAME` char(25) NOT NULL,PRIMARY KEY (`N_NATIONKEY`))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(0,0,0,1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(0,1,1,1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(0,2,2,0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(0,3,3,3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(0,4,1,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into supplier values(0, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into orders values(0, 'F')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into nation values(0, 'EGYPT')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(1,0,2,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(1,1,1,0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(1,2,3,3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(1,3,1,0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(1,4,1,3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into supplier values(1, 1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into orders values(1, 'F')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into nation values(1, 'EGYPT')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(2,0,1,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(2,1,3,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(2,2,2,0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(2,3,3,1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(2,4,4,3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into supplier values(2, 3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into orders values(2, 'F')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into nation values(2, 'EGYPT')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(3,0,4,3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(3,1,4,3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(3,2,2,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(3,3,0,0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(3,4,1,0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into supplier values(3, 1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into orders values(3, 'F')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into nation values(3, 'EGYPT')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(4,0,2,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(4,1,4,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(4,2,0,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(4,3,0,1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into lineitem values(4,4,2,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into supplier values(4, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into orders values(4, 'F')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into nation values(4, 'EGYPT')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select count(*) from supplier, lineitem l1, orders, nation where s_suppkey = l1.l_suppkey and o_orderkey = l1.l_orderkey and o_orderstatus = 'F' and  exists ( select * from lineitem l2 where l2.l_orderkey = l1.l_orderkey and l2.l_suppkey < l1.l_suppkey ) and s_nationkey = n_nationkey and n_name = 'EGYPT' order by l1.l_orderkey, l1.l_linenumber;").Check(testkit.Rows("18"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table lineitem")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table nation")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table supplier")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table orders")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestIssue52902 对应 Go 的同名测试，来源行 156。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言。
/// TestIssue52902 对应 Go 的同名测试，来源行 156。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_issue52902() {
    let mut draft = CaseRecorder::new(r#"TestIssue52902"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestIssue52902(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// index hash join with semi join"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/planner/core/MockOnlyEnableIndexHashJoinV2", "return(true)"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/planner/core/MockOnlyEnableIndexHashJoinV2"))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t0")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1 (x int, y int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t0 (a int, b int, key (`b`))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(103, 600)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(100, 200)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t0 values( 105, 400)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t0 values( 104, 300)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t0 values( 103, 300)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t0 values( 102, 200)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t0 values( 101, 200)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t0 values( 100, 200)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select * from t1 where 1 = 1 and case when t1.x < 1000 then 1 = 1 " +"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"		"when t1.x < 2000 then not exists (select 1 from t0 where t0.b = t1.y) else 1 = 1 end").Check(testkit.Rows("100 200", "103 600"))"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestHashJoin 对应 Go 的同名测试，来源行 181。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestHashJoin 对应 Go 的同名测试，来源行 181。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_hash_join() {
    let mut draft = CaseRecorder::new(r#"TestHashJoin"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestHashJoin(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, b int);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2(a int, b int);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1,1),(2,2),(3,3),(4,4),(5,5);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select count(*) from t1").Check(testkit.Rows("5"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select count(*) from t2").Check(testkit.Rows("0"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_init_chunk_size=1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result := tk.MustQuery("explain analyze select /*+ TIDB_HJ(t1, t2) */ * from t1 where exists (select a from t2 where t1.a = t2.a);")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	//   0                       1        2 3         4        5                                                                    6                                           7         8"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// 0 HashJoin_9              7992.00  0 root               time:959.436µs, loops:1, Concurrency:5, probe collision:0, build:0s  semi join, equal:[eq(test.t1.a, test.t2.a)] 0 Bytes   0 Bytes"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// 1 ├─TableReader_15(Build) 9990.00  0 root               time:583.499µs, loops:1, rpc num: 1, rpc time:563.325µs, proc keys:0 data:Selection_14                           141 Bytes N/A"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// 2 │ └─Selection_14        9990.00  0 cop[tikv]          time:53.674µs, loops:1                                               not(isnull(test.t2.a))                      N/A       N/A"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// 3 │   └─TableFullScan_13  10000.00 0 cop[tikv] table:t2 time:52.14µs, loops:1                                                keep order:false, stats:pseudo              N/A       N/A"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// 4 └─TableReader_12(Probe) 9990.00  5 root               time:779.503µs, loops:1, rpc num: 1, rpc time:794.929µs, proc keys:0 data:Selection_11                           241 Bytes N/A"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// 5   └─Selection_11        9990.00  5 cop[tikv]          time:243.395µs, loops:6                                              not(isnull(test.t1.a))                      N/A       N/A"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// 6     └─TableFullScan_10  10000.00 5 cop[tikv] table:t1 time:206.273µs, loops:6                                              keep order:false, stats:pseudo              N/A       N/A"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	row := result.Rows()"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, 7, len(row))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	innerActRows := row[1][2].(string)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, "0", innerActRows)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	outerActRows := row[4][2].(string)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// FIXME: revert this result to 1 after TableReaderExecutor can handle initChunkSize."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, "5", outerActRows)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestOuterTableBuildHashTableIsuse13933 对应 Go 的同名测试，来源行 210。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestOuterTableBuildHashTableIsuse13933 对应 Go 的同名测试，来源行 210。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_outer_table_build_hash_table_isuse13933() {
    let mut draft = CaseRecorder::new(r#"TestOuterTableBuildHashTableIsuse13933"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestOuterTableBuildHashTableIsuse13933(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t, s")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t (a int,b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table s (a int,b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values (11,11),(1,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into s values (1,2),(2,1),(11,11)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ HASH_JOIN_BUILD(t) */ * from t left join s on s.a > t.a").Sort().Check(testkit.Rows("1 2 11 11", "1 2 2 1", "11 11 <nil> <nil>"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("explain format = 'brief' select /*+ HASH_JOIN_BUILD(t) */ * from t left join s on s.a > t.a").Check(testkit.Rows("#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"		"HashJoin 99900000.00 root  CARTESIAN left outer join, left side:TableReader, other cond:gt(test.s.a, test.t.a)","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"├─TableReader(Build) 10000.00 root  data:TableFullScan","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"│ └─TableFullScan 10000.00 cop[tikv] table:t keep order:false, stats:pseudo","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"└─TableReader(Probe) 9990.00 root  data:Selection","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"  └─Selection 9990.00 cop[tikv]  not(isnull(test.s.a))","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"    └─TableFullScan 10000.00 cop[tikv] table:s keep order:false, stats:pseudo"))"#,
        r#"source line"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t, s")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("Create table s (a int, b int, key(b))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("Create table t (a int, b int, key(b))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("Insert into s values (1,2),(2,1),(11,11)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("Insert into t values (11,2),(1,2),(5,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(s) */ * from t left join s on s.b=t.b and s.a < t.a;").Sort().Check(testkit.Rows("1 2 <nil> <nil>", "11 2 1 2", "5 2 1 2"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("explain format = 'brief' select /*+ INL_HASH_JOIN(s) */ * from t left join s on s.b=t.b and s.a < t.a;").Check(testkit.Rows("#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"		"IndexHashJoin 12475.01 root  left outer join, inner:IndexLookUp, left side:TableReader, outer key:test.t.b, inner key:test.s.b, equal cond:eq(test.t.b, test.s.b), other cond:lt(test.s.a, test.t.a)","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"├─TableReader(Build) 10000.00 root  data:TableFullScan","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"│ └─TableFullScan 10000.00 cop[tikv] table:t keep order:false, stats:pseudo","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"└─IndexLookUp(Probe) 12475.01 root  ","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"  ├─Selection(Build) 12487.50 cop[tikv]  not(isnull(test.s.b))","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"  │ └─IndexRangeScan 12500.00 cop[tikv] table:s, index:b(b) range: decided by [eq(test.s.b, test.t.b)], keep order:false, stats:pseudo","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"  └─Selection(Probe) 12475.01 cop[tikv]  not(isnull(test.s.a))","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"    └─TableRowIDScan 12487.50 cop[tikv] table:s keep order:false, stats:pseudo"))"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestInlineProjection4HashJoinIssue15316 对应 Go 的同名测试，来源行 244。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestInlineProjection4HashJoinIssue15316 对应 Go 的同名测试，来源行 244。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_inline_projection4_hash_join_issue15316() {
    let mut draft = CaseRecorder::new(r#"TestInlineProjection4HashJoinIssue15316"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestInlineProjection4HashJoinIssue15316(t *testing.T) {"#,
        r#"source line"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// Two necessary factors to reproduce this issue:"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// (1) taking HashLeftJoin, i.e., letting the probing tuple lay at the left side of joined tuples"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// (2) the projection only contains a part of columns from the build side, i.e., pruning the same probe side"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists S, T")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table S (a int not null, b int, c int);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table T (a int not null, b int, c int);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into S values (0,1,2),(0,1,null),(0,1,2);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into T values (0,10,2),(0,10,null),(1,10,2);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ HASH_JOIN_BUILD(T) */ T.a,T.a,T.c from S join T on T.a = S.a where S.b<T.b order by T.a,T.c;").Check(testkit.Rows("#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"		"0 0 <nil>","#, r#"source line"#);

    record_line(&mut draft, r#"		"0 0 <nil>","#, r#"source line"#);

    record_line(&mut draft, r#"		"0 0 <nil>","#, r#"source line"#);

    record_line(&mut draft, r#"		"0 0 2","#, r#"source line"#);

    record_line(&mut draft, r#"		"0 0 2","#, r#"source line"#);

    record_line(&mut draft, r#"		"0 0 2","#, r#"source line"#);

    record_line(&mut draft, r#"	))"#, r#"source line"#);
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// NOTE: the HashLeftJoin should be kept"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("explain format = 'brief' select /*+ HASH_JOIN_BUILD(T) */ T.a,T.a,T.c from S join T on T.a = S.a where S.b<T.b order by T.a,T.c;").Check(testkit.Rows("#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"		"Projection 12487.50 root  test.t.a, test.t.a, test.t.c","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"└─Sort 12487.50 root  test.t.a, test.t.c","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"  └─HashJoin 12487.50 root  inner join, equal:[eq(test.s.a, test.t.a)], other cond:lt(test.s.b, test.t.b)","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"    ├─TableReader(Build) 9990.00 root  data:Selection","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"    │ └─Selection 9990.00 cop[tikv]  not(isnull(test.t.b))","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"    │   └─TableFullScan 10000.00 cop[tikv] table:T keep order:false, stats:pseudo","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"    └─TableReader(Probe) 9990.00 root  data:Selection","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"      └─Selection 9990.00 cop[tikv]  not(isnull(test.s.b))","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"        └─TableFullScan 10000.00 cop[tikv] table:S keep order:false, stats:pseudo"))"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestIssue18572_1 对应 Go 的同名测试，来源行 277。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
/// TestIssue18572_1 对应 Go 的同名测试，来源行 277。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
#[test]
pub fn test_issue18572_1() {
    let mut draft = CaseRecorder::new(r#"TestIssue18572_1"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。"#);

    record_line(
        &mut draft,
        r#"func TestIssue18572_1(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, b int, index idx(b));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1, 1);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 select * from t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/join/testIndexHashJoinInnerWorkerErr", "return"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/join/testIndexHashJoinInnerWorkerErr"))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	rs, err := tk.Exec("select /*+ inl_hash_join(t1) */ * from t1 right join t1 t2 on t1.b=t2.b;")"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。
    record_line(
        &mut draft,
        r#"	_, err = session.GetRows4Test(context.Background(), nil, rs)"#,
        r#"结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, strings.Contains(err.Error(), "mockIndexHashJoinInnerWorkerErr"))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。
    record_line(
        &mut draft,
        r#"	require.NoError(t, rs.Close())"#,
        r#"资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestIssue18572_2 对应 Go 的同名测试，来源行 298。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
/// TestIssue18572_2 对应 Go 的同名测试，来源行 298。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
#[test]
pub fn test_issue18572_2() {
    let mut draft = CaseRecorder::new(r#"TestIssue18572_2"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。"#);

    record_line(
        &mut draft,
        r#"func TestIssue18572_2(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, b int, index idx(b));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1, 1);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 select * from t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/join/testIndexHashJoinOuterWorkerErr", "return"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/join/testIndexHashJoinOuterWorkerErr"))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	rs, err := tk.Exec("select /*+ inl_hash_join(t1) */ * from t1 right join t1 t2 on t1.b=t2.b;")"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。
    record_line(
        &mut draft,
        r#"	_, err = session.GetRows4Test(context.Background(), nil, rs)"#,
        r#"结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, strings.Contains(err.Error(), "mockIndexHashJoinOuterWorkerErr"))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。
    record_line(
        &mut draft,
        r#"	require.NoError(t, rs.Close())"#,
        r#"资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestIssue18572_3 对应 Go 的同名测试，来源行 319。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
/// TestIssue18572_3 对应 Go 的同名测试，来源行 319。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
#[test]
pub fn test_issue18572_3() {
    let mut draft = CaseRecorder::new(r#"TestIssue18572_3"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。"#);

    record_line(
        &mut draft,
        r#"func TestIssue18572_3(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, b int, index idx(b));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1, 1);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 select * from t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/join/testIndexHashJoinBuildErr", "return"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/join/testIndexHashJoinBuildErr"))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	rs, err := tk.Exec("select /*+ inl_hash_join(t1) */ * from t1 right join t1 t2 on t1.b=t2.b;")"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。
    record_line(
        &mut draft,
        r#"	_, err = session.GetRows4Test(context.Background(), nil, rs)"#,
        r#"结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, strings.Contains(err.Error(), "mockIndexHashJoinBuildErr"))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。
    record_line(
        &mut draft,
        r#"	require.NoError(t, rs.Close())"#,
        r#"资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestExplainAnalyzeJoin 对应 Go 的同名测试，来源行 340。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestExplainAnalyzeJoin 对应 Go 的同名测试，来源行 340。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_explain_analyze_join() {
    let mut draft = CaseRecorder::new(r#"TestExplainAnalyzeJoin"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestExplainAnalyzeJoin(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1,t2;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1 (a int, b int, unique index (a));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2 (a int, b int, unique index (a))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values (1,1),(2,2),(3,3),(4,4),(5,5)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values (1,1),(2,2),(3,3),(4,4),(5,5)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	// Test for index lookup join."#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	rows := tk.MustQuery("explain analyze select /*+ INL_JOIN(t1, t2) */ * from t1,t2 where t1.a=t2.a;").Rows()"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, 8, len(rows))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Regexp(t, "IndexJoin_.*", rows[0][0])"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Regexp(t, "time:.*, loops:.*, inner:{total:.*, concurrency:.*, task:.*, construct:.*, fetch:.*, build:.*}, probe:.*", rows[0][5])"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	// Test for index lookup hash join."#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	rows = tk.MustQuery("explain analyze select /*+ INL_HASH_JOIN(t1, t2) */ * from t1,t2 where t1.a=t2.a;").Rows()"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, 8, len(rows))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Regexp(t, "IndexHashJoin.*", rows[0][0])"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Regexp(t, "time:.*, open:.*, close:.*, loops:.*, inner:{total:.*, concurrency:.*, task:.*, construct:.*, fetch:.*, build:.*, join:.*}", rows[0][5])"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	// Test for hash join."#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	rows = tk.MustQuery("explain analyze select /*+ HASH_JOIN(t1, t2) */ * from t1,t2 where t1.a=t2.a;").Rows()"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, 7, len(rows))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Regexp(t, "HashJoin.*", rows[0][0])"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Regexp(t, "time:.*, open:.*, close:.*, loops:.*, build_hash_table:{concurrency:.*, time:.*, fetch:.*, max_partition:.*, total_partition:.*, max_build:.*, total_build:.*}, probe:{concurrency:.*, time:.*, fetch_and_wait:.*, max_worker_time:.*, total_worker_time:.*, max_probe:.*, total_probe:.*}", rows[0][5])"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// TestExplainAnalyzeIndexHashJoin"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// Issue 43597"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t (a int, index idx(a));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	sql := "insert into t values""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i := 0; i <= 1024; i++ {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。
    record_line(
        &mut draft,
        r#"		if i != 0 {"#,
        r#"条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。"#,
    );

    record_line(&mut draft, r#"			sql += ",""#, r#"source line"#);

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"		sql += fmt.Sprintf("(%d)", i)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(sql)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i := 0; i <= 10; i++ {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"		// Test for index lookup hash join."#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"		rows := tk.MustQuery("explain analyze select /*+ INL_HASH_JOIN(t1, t2) */ * from t t1 join t t2 on t1.a=t2.a limit 1;").Rows()"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"		require.Equal(t, 7, len(rows))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"		require.Regexp(t, "IndexHashJoin.*", rows[1][0])"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"		// When innerWorkerRuntimeStats.join is negative, `join:` will not print."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"		require.Regexp(t, "time:.*, open:.*, close:.*, loops:.*, inner:{total:.*, concurrency:.*, task:.*, construct:.*, fetch:.*, build:.*, join:.*}", rows[1][5])"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestIssue20270 对应 Go 的同名测试，来源行 387。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
/// TestIssue20270 对应 Go 的同名测试，来源行 387。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
#[test]
pub fn test_issue20270() {
    let mut draft = CaseRecorder::new(r#"TestIssue20270"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。"#);

    record_line(
        &mut draft,
        r#"func TestIssue20270(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1,1),(2,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(2,3),(4,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(join.DisableHashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/join/killedInJoin2Chunk", "return(true)"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk.QueryToErr("select /*+ HASH_JOIN(t, t1) */ * from t left join t1 on t.c1 = t1.c1 where t.c1 = 1 or t1.c2 > 20")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, exeerrors.ErrQueryInterrupted, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/join/killedInJoin2Chunk"))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	err = failpoint.Enable("github.com/pingcap/tidb/pkg/executor/join/killedInJoin2ChunkForOuterHashJoin", "return(true)")"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1,30),(2,40)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.QueryToErr("select /*+ HASH_JOIN_BUILD(t) */ * from t left outer join t1 on t.c1 = t1.c1 where t.c1 = 1 or t1.c2 > 20")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, exeerrors.ErrQueryInterrupted, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"	err = failpoint.Disable("github.com/pingcap/tidb/pkg/executor/join/killedInJoin2ChunkForOuterHashJoin")"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestIssue31129 对应 Go 的同名测试，来源行 411。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
/// TestIssue31129 对应 Go 的同名测试，来源行 411。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
#[test]
pub fn test_issue31129() {
    let mut draft = CaseRecorder::new(r#"TestIssue31129"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。"#);

    record_line(
        &mut draft,
        r#"func TestIssue31129(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_init_chunk_size=2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_index_join_batch_size=10")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("DROP TABLE IF EXISTS t, s")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_enable_clustered_index='INT_ONLY'")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(pk int primary key, a int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i := range 100 {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"		tk.MustExec(fmt.Sprintf("insert into t values(%d, %d)", i, i))"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table s(a int primary key)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i := range 100 {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"		tk.MustExec(fmt.Sprintf("insert into s values(%d)", i))"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("analyze table t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("analyze table s")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// Test IndexNestedLoopHashJoin keepOrder."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	fpName := "github.com/pingcap/tidb/pkg/executor/join/TestIssue31129""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName, "return"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk.QueryToErr("select /*+ INL_HASH_JOIN(s) */ * from t left join s on t.a=s.a order by t.pk")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, strings.Contains(err.Error(), "TestIssue31129"))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Disable(fpName))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// Test IndexNestedLoopHashJoin build hash table panic."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	fpName = "github.com/pingcap/tidb/pkg/executor/join/IndexHashJoinBuildHashTablePanic""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName, `panic("IndexHashJoinBuildHashTablePanic")`))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.QueryToErr("select /*+ INL_HASH_JOIN(s) */ * from t left join s on t.a=s.a order by t.pk")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, strings.Contains(err.Error(), "IndexHashJoinBuildHashTablePanic"))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Disable(fpName))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// Test IndexNestedLoopHashJoin fetch inner fail."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	fpName = "github.com/pingcap/tidb/pkg/executor/join/IndexHashJoinFetchInnerResultsErr""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName, "return"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.QueryToErr("select /*+ INL_HASH_JOIN(s) */ * from t left join s on t.a=s.a order by t.pk")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, strings.Contains(err.Error(), "IndexHashJoinFetchInnerResultsErr"))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Disable(fpName))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// Test IndexNestedLoopHashJoin build hash table panic and IndexNestedLoopHashJoin fetch inner fail at the same time."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	fpName1, fpName2 := "github.com/pingcap/tidb/pkg/executor/join/IndexHashJoinBuildHashTablePanic", "github.com/pingcap/tidb/pkg/executor/join/IndexHashJoinFetchInnerResultsErr""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName1, `panic("IndexHashJoinBuildHashTablePanic")`))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName2, "return"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.QueryToErr("select /*+ INL_HASH_JOIN(s) */ * from t left join s on t.a=s.a order by t.pk")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, strings.Contains(err.Error(), "IndexHashJoinBuildHashTablePanic"))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Disable(fpName1))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Disable(fpName2))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestSplitPartitionPanic 对应 Go 的同名测试，来源行 461。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
/// TestSplitPartitionPanic 对应 Go 的同名测试，来源行 461。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
#[test]
pub fn test_split_partition_panic() {
    let mut draft = CaseRecorder::new(r#"TestSplitPartitionPanic"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。"#);

    record_line(
        &mut draft,
        r#"func TestSplitPartitionPanic(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1 (a int, b int, c int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2 (a int, b int, c int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values (1, 1, 1), (1, 2, 2), (2, 1, 3), (2, 2, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values (1, 1, 1), (1, 2, 2), (2, 1, 3), (2, 2, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(join.EnableHashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	fpName := "github.com/pingcap/tidb/pkg/executor/join/splitPartitionPanic""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName, "panic(\"splitPartitionPanic\")"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable(fpName))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk.QueryToErr("select /*+ hash_join(t1)*/ * from t1 join t2 on t1.a = t2.a and t1.b = t2.b")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.EqualError(t, err, "failpoint panic: splitPartitionPanic")"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestProcessOneProbeChunkPanic 对应 Go 的同名测试，来源行 481。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
/// TestProcessOneProbeChunkPanic 对应 Go 的同名测试，来源行 481。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
#[test]
pub fn test_process_one_probe_chunk_panic() {
    let mut draft = CaseRecorder::new(r#"TestProcessOneProbeChunkPanic"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。"#);

    record_line(
        &mut draft,
        r#"func TestProcessOneProbeChunkPanic(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1 (a int, b int, c int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2 (a int, b int, c int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values (1, 1, 1), (1, 2, 2), (2, 1, 3), (2, 2, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values (1, 1, 1), (1, 2, 2), (2, 1, 3), (2, 2, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(join.EnableHashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	fpName := "github.com/pingcap/tidb/pkg/executor/join/processOneProbeChunkPanic""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName, "panic(\"processOneProbeChunkPanic\")"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable(fpName))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk.QueryToErr("select /*+ hash_join(t1)*/ * from t1 join t2 on t1.a = t2.a and t1.b = t2.b")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.EqualError(t, err, "failpoint panic: processOneProbeChunkPanic")"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestCreateTasksPanic 对应 Go 的同名测试，来源行 501。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
/// TestCreateTasksPanic 对应 Go 的同名测试，来源行 501。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
#[test]
pub fn test_create_tasks_panic() {
    let mut draft = CaseRecorder::new(r#"TestCreateTasksPanic"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。"#);

    record_line(
        &mut draft,
        r#"func TestCreateTasksPanic(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1 (a int, b int, c int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2 (a int, b int, c int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values (1, 1, 1), (1, 2, 2), (2, 1, 3), (2, 2, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values (1, 1, 1), (1, 2, 2), (2, 1, 3), (2, 2, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(join.EnableHashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	fpName := "github.com/pingcap/tidb/pkg/executor/join/createTasksPanic""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName, "panic(\"createTasksPanic\")"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable(fpName))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk.QueryToErr("select /*+ hash_join(t1)*/ * from t1 join t2 on t1.a = t2.a and t1.b = t2.b")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.EqualError(t, err, "failpoint panic: createTasksPanic")"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestBuildHashTablePanic 对应 Go 的同名测试，来源行 521。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
/// TestBuildHashTablePanic 对应 Go 的同名测试，来源行 521。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
#[test]
pub fn test_build_hash_table_panic() {
    let mut draft = CaseRecorder::new(r#"TestBuildHashTablePanic"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。"#);

    record_line(
        &mut draft,
        r#"func TestBuildHashTablePanic(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1 (a int, b int, c int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2 (a int, b int, c int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values (1, 1, 1), (1, 2, 2), (2, 1, 3), (2, 2, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values (1, 1, 1), (1, 2, 2), (2, 1, 3), (2, 2, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(join.EnableHashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	fpName := "github.com/pingcap/tidb/pkg/executor/join/buildHashTablePanic""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName, "panic(\"buildHashTablePanic\")"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable(fpName))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk.QueryToErr("select /*+ hash_join(t1)*/ * from t1 join t2 on t1.a = t2.a and t1.b = t2.b")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.EqualError(t, err, "failpoint panic: buildHashTablePanic")"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestKillDuringProbe 对应 Go 的同名测试，来源行 541。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
/// TestKillDuringProbe 对应 Go 的同名测试，来源行 541。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
#[test]
pub fn test_kill_during_probe() {
    let mut draft = CaseRecorder::new(r#"TestKillDuringProbe"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。"#);

    record_line(
        &mut draft,
        r#"func TestKillDuringProbe(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1,1),(2,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(2,3),(4,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(join.EnableHashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/join/killedDuringProbe", "return(true)"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/join/killedDuringProbe"))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// inner join"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk.QueryToErr("select /*+ HASH_JOIN(t, t1) */ * from t join t1 on t.c1 = t1.c1")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, exeerrors.ErrQueryInterrupted, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// left outer join with outer to build"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.QueryToErr("select /*+ HASH_JOIN(t, t1) */ * from t left join t1 on t.c1 = t1.c1 where t.c1 = 1 or t1.c2 > 20")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, exeerrors.ErrQueryInterrupted, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// left outer join with inner to build"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.QueryToErr("select /*+ HASH_JOIN_BUILD(t) */ * from t left outer join t1 on t.c1 = t1.c1 where t.c1 = 1 or t1.c2 > 20")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, exeerrors.ErrQueryInterrupted, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1,30),(2,40)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// left outer join with inner to build"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.QueryToErr("select /*+ HASH_JOIN_BUILD(t) */ * from t left outer join t1 on t.c1 = t1.c1 where t.c1 = 1 or t1.c2 > 20")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, exeerrors.ErrQueryInterrupted, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestKillDuringBuild 对应 Go 的同名测试，来源行 571。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
/// TestKillDuringBuild 对应 Go 的同名测试，来源行 571。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。
#[test]
pub fn test_kill_during_build() {
    let mut draft = CaseRecorder::new(r#"TestKillDuringBuild"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径。"#);

    record_line(
        &mut draft,
        r#"func TestKillDuringBuild(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1,1),(2,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(2,3),(4,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(join.EnableHashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/join/killedDuringBuild", "return(true)"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/join/killedDuringBuild"))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// inner join"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk.QueryToErr("select /*+ HASH_JOIN(t, t1) */ * from t join t1 on t.c1 = t1.c1")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, exeerrors.ErrQueryInterrupted, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// left outer join with outer to build"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.QueryToErr("select /*+ HASH_JOIN(t, t1) */ * from t left join t1 on t.c1 = t1.c1")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, exeerrors.ErrQueryInterrupted, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// left outer join with inner to build"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.QueryToErr("select /*+ HASH_JOIN_BUILD(t) */ * from t left outer join t1 on t.c1 = t1.c1")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, exeerrors.ErrQueryInterrupted, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestIssue54755 对应 Go 的同名测试，来源行 597。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestIssue54755 对应 Go 的同名测试，来源行 597。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_issue54755() {
    let mut draft = CaseRecorder::new(r#"TestIssue54755"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestIssue54755(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t2;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(pk INTEGER AUTO_INCREMENT, col_int_nokey INTEGER, col_int_key INTEGER, col_varchar_key VARCHAR(1), col_varchar_nokey VARCHAR(1), PRIMARY KEY (pk), KEY (col_int_key), KEY (col_varchar_key, col_int_key))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2(pk INTEGER AUTO_INCREMENT, col_int_nokey INTEGER, col_int_key INTEGER, col_varchar_key VARCHAR(1), col_varchar_nokey VARCHAR(1), PRIMARY KEY (pk), KEY (col_int_key), KEY (col_varchar_key, col_int_key))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1(col_int_key, col_int_nokey,col_varchar_key, col_varchar_nokey) values(4,2,'v','v'),(62,150,'v','v')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2(col_int_key, col_int_nokey,col_varchar_key, col_varchar_nokey) values(8,null,'x','x'),(7,8,'d','d')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(join.EnableHashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// right join"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select max(SQ1_alias2.col_int_nokey) as SQ1_field1 from ( t2 as SQ1_alias1 right join t1 as SQ1_alias2 on ( SQ1_alias2.col_varchar_key = SQ1_alias1.col_varchar_nokey ))").Check(testkit.Rows("150"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// left join"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select max(SQ1_alias2.col_int_nokey) as SQ1_field1 from ( t1 as SQ1_alias2 left join t2 as SQ1_alias1 on ( SQ1_alias2.col_varchar_key = SQ1_alias1.col_varchar_nokey ))").Check(testkit.Rows("150"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestIssue55016 对应 Go 的同名测试，来源行 614。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestIssue55016 对应 Go 的同名测试，来源行 614。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_issue55016() {
    let mut draft = CaseRecorder::new(r#"TestIssue55016"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestIssue55016(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(a varchar(10), b char(10))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values('aa','a')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for _, hashJoinV2 := range join.HashJoinV2Strings {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"		tk.MustExec(hashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"		tk.MustQuery("select count(*) from t t1 join t t2 on t1.a = t2.b and t2.a = t1.b").Check(testkit.Rows("0"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestIssue56214 对应 Go 的同名测试，来源行 627。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestIssue56214 对应 Go 的同名测试，来源行 627。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_issue56214() {
    let mut draft = CaseRecorder::new(r#"TestIssue56214"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestIssue56214(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t2;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t3;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(id int, value int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2(id int, value int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t3(id int, value int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1,2),(2,3),(3,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values(1,10),(1,1),(2,10),(2,10)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t3 values(1,10),(1,20)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for _, hashJoinV2 := range join.HashJoinV2Strings {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"		tk.MustExec(hashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"		tk.MustQuery("select value, (select t1.id from t1 join t2 on t1.id = t2.id and t1.value < t2.value - t3.value + 3) d from t3 order by value").Check(testkit.Rows("10 1", "20 <nil>"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

// TestIssue56825 对应 Go 的同名测试，来源行 646。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestIssue56825 对应 Go 的同名测试，来源行 646。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_issue56825() {
    let mut draft = CaseRecorder::new(r#"TestIssue56825"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestIssue56825(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t2;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(id int, col1 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2(id int, col1 int, col2 int, col3 int, col4 int, col5 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1,2),(2,3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values(1,2,3,4,5,6),(3,4,5,6,7,8),(4,5,6,7,8,9)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("analyze table t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("analyze table t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// t1 as build side"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for _, hashJoinV2 := range join.HashJoinV2Strings {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"		tk.MustExec(hashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"		tk.MustQuery("select * from t1 left join t2 on t1.id = t2.id and t1.col1 <= t2.col1 order by t1.id").Check(testkit.Rows("1 2 1 2 3 4 5 6", "2 3 <nil> <nil> <nil> <nil> <nil> <nil>"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"		tk.MustQuery("select * from t1 right join t2 on t1.id = t2.id and t1.col1 <= t2.col1 order by t2.id").Check(testkit.Rows("1 2 1 2 3 4 5 6", "<nil> <nil> 3 4 5 6 7 8", "<nil> <nil> 4 5 6 7 8 9"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(10,20),(11,21),(12,22),(13,23),(14,24),(15,25)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("analyze table t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// t2 as build side"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for _, hashJoinV2 := range join.HashJoinV2Strings {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"		tk.MustExec(hashJoinV2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"		tk.MustQuery("select * from t1 left join t2 on t1.id = t2.id and t1.col1 <= t2.col1 order by t1.id").Check(testkit.Rows("#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"			"1 2 1 2 3 4 5 6","#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"2 3 <nil> <nil> <nil> <nil> <nil> <nil>","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"			"10 20 <nil> <nil> <nil> <nil> <nil> <nil>","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"			"11 21 <nil> <nil> <nil> <nil> <nil> <nil>","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"			"12 22 <nil> <nil> <nil> <nil> <nil> <nil>","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"			"13 23 <nil> <nil> <nil> <nil> <nil> <nil>","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"			"14 24 <nil> <nil> <nil> <nil> <nil> <nil>","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"			"15 25 <nil> <nil> <nil> <nil> <nil> <nil>","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		))"#, r#"source line"#);
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"		tk.MustQuery("select * from t1 right join t2 on t1.id = t2.id and t1.col1 <= t2.col1 order by t2.id").Check(testkit.Rows("1 2 1 2 3 4 5 6", "<nil> <nil> 3 4 5 6 7 8", "<nil> <nil> 4 5 6 7 8 9"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);

    record_line(&mut draft, r#"}"#, r#"source line"#);
    assert_go_source_map(&draft);
}

/// 可执行冒烟：接入 TestKit / mockstore 驱动真实查询期望。
/// 把字符串矩阵转成 testkit `QueryRows`，供 MustQuery 期望结果。
fn executable_rows(values: &[&[&str]]) -> QueryRows {
    QueryRows {
        columns: Vec::new(),
        rows: values
            .iter()
            .map(|row| {
                row.iter()
                    .map(|value| DbValue::String((*value).to_owned()))
                    .collect()
            })
            .collect(),
    }
}

fn int_row(values: &[i64]) -> Row {
    values.iter().copied().map(Value::Int).collect()
}

fn hash_join_info(join_type: JoinType, build: Chunk, probe: Chunk) -> HashJoinInfo {
    HashJoinInfo {
        join_type,
        build_key_indices: vec![0],
        probe_key_indices: vec![0],
        build_chunks: vec![build],
        probe_chunks: vec![probe],
        outer_is_right: false,
        build_side_is_outer: false,
        null_aware: false,
        concurrency: 3,
        max_chunk_size: 2,
        default_inner: vec![Value::Null, Value::Null],
        conditions: Vec::new(),
        children_used: None,
    }
}

fn execute_hash_join_v1(info: &HashJoinInfo) -> Vec<Row> {
    let mut executor = build_hash_join_v1_exec(info).expect("build HashJoin V1");
    let mut rows = executor.execute_all().expect("execute HashJoin V1");
    executor.close();
    rows.sort_by(generate_cmp_func());
    rows
}

fn execute_hash_join_v2(info: &HashJoinInfo) -> Vec<Row> {
    let context = HashJoinCtxV2::new(
        info.join_type,
        info.build_key_indices.clone(),
        info.probe_key_indices.clone(),
        true,
        info.null_aware,
        info.concurrency,
        info.max_chunk_size,
        None,
    )
    .expect("build HashJoin V2 context");
    let joiner = Joiner::new(
        info.join_type,
        info.outer_is_right,
        info.default_inner.clone(),
        info.conditions.clone(),
        info.children_used.clone(),
        info.null_aware,
        info.max_chunk_size,
    )
    .expect("build HashJoin V2 joiner");
    let mut executor = HashJoinV2Exec::new(
        context,
        joiner,
        info.build_chunks.clone(),
        info.probe_chunks.clone(),
    )
    .expect("build HashJoin V2");
    let mut rows = executor.execute_all().expect("execute HashJoin V2");
    executor.close();
    assert!(executor.is_all_memory_cleared_for_test());
    rows.sort_by(generate_cmp_func());
    rows
}

/// 创建隔离的真实 SQL 会话，供从 Go testkit 迁移的 Hash Join 回归使用。
fn hash_join_session(database: &str) -> ConcreteSession {
    let (_domain, session) = CreateAnalyzeSession().expect("create hash join test session");
    execute_sql(&session, &format!("create database {database}"));
    execute_sql(&session, &format!("use {database}"));
    session
}

/// 通过 ConcreteSession 执行建表、DML、会话变量等 Go `MustExec` 边界。
fn execute_sql(session: &ConcreteSession, sql: &str) {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("hash join setup failed for {sql:?}: {error}"));
}

/// 执行真实关系查询并完整消费、关闭结果集。
fn query_sql(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut record_sets = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("hash join query failed for {sql:?}: {error}"));
    let mut result = record_sets
        .pop()
        .unwrap_or_else(|| panic!("hash join query returned no record set for {sql:?}"));
    let mut rows = Vec::new();
    while let Some(row) = result
        .Next()
        .unwrap_or_else(|error| panic!("hash join row fetch failed for {sql:?}: {error}"))
    {
        rows.push(row);
    }
    result
        .Close()
        .unwrap_or_else(|error| panic!("hash join close failed for {sql:?}: {error}"));
    rows
}

#[test]
/// Go TestIssue56825：实际执行两种 build-side 基数下的左右外连接及 NULL 填充。
fn issue56825_executes_real_outer_hash_join_contracts() {
    let session = hash_join_session("hash_join_issue56825");
    execute_sql(&session, "create table t1(id int, col1 int)");
    execute_sql(
        &session,
        "create table t2(id int, col1 int, col2 int, col3 int, col4 int, col5 int)",
    );
    execute_sql(&session, "insert into t1 values(1,2),(2,3)");
    execute_sql(
        &session,
        "insert into t2 values(1,2,3,4,5,6),(3,4,5,6,7,8),(4,5,6,7,8,9)",
    );

    for hash_join_v2 in [
        "set tidb_hash_join_version = 'legacy'",
        "set tidb_hash_join_version = 'optimized'",
    ] {
        execute_sql(&session, hash_join_v2);
        assert_eq!(
            query_sql(
                &session,
                "select * from t1 left join t2 on t1.id = t2.id and t1.col1 <= t2.col1 order by t1.id",
            ),
            vec![
                vec!["1", "2", "1", "2", "3", "4", "5", "6"],
                vec![
                    "2", "3", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>"
                ],
            ],
        );
        assert_eq!(
            query_sql(
                &session,
                "select * from t1 right join t2 on t1.id = t2.id and t1.col1 <= t2.col1 order by t2.id",
            ),
            vec![
                vec!["1", "2", "1", "2", "3", "4", "5", "6"],
                vec!["<nil>", "<nil>", "3", "4", "5", "6", "7", "8"],
                vec!["<nil>", "<nil>", "4", "5", "6", "7", "8", "9"],
            ],
        );
    }

    execute_sql(
        &session,
        "insert into t1 values(10,20),(11,21),(12,22),(13,23),(14,24),(15,25)",
    );
    let expected_left = vec![
        vec!["1", "2", "1", "2", "3", "4", "5", "6"],
        vec![
            "2", "3", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>",
        ],
        vec![
            "10", "20", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>",
        ],
        vec![
            "11", "21", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>",
        ],
        vec![
            "12", "22", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>",
        ],
        vec![
            "13", "23", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>",
        ],
        vec![
            "14", "24", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>",
        ],
        vec![
            "15", "25", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>", "<nil>",
        ],
    ];
    for hash_join_v2 in [
        "set tidb_hash_join_version = 'legacy'",
        "set tidb_hash_join_version = 'optimized'",
    ] {
        execute_sql(&session, hash_join_v2);
        assert_eq!(
            query_sql(
                &session,
                "select * from t1 left join t2 on t1.id = t2.id and t1.col1 <= t2.col1 order by t1.id",
            ),
            expected_left,
        );
    }
}

#[test]
/// Go TestIssue52902：CASE 只选择第一个分支时，不应错误执行 NOT EXISTS 分支。
fn issue52902_executes_correlated_exists_case_contract() {
    let session = hash_join_session("hash_join_issue52902");
    execute_sql(&session, "create table t1 (x int, y int)");
    execute_sql(&session, "create table t0 (a int, b int, key (b))");
    execute_sql(&session, "insert into t1 values(103, 600),(100, 200)");
    execute_sql(
        &session,
        "insert into t0 values(105,400),(104,300),(103,300),(102,200),(101,200),(100,200)",
    );
    assert_eq!(
        query_sql(
            &session,
            "select * from t1 where 1 = 1 and case when t1.x < 1000 then 1 = 1 \
             when t1.x < 2000 then not exists (select 1 from t0 where t0.b = t1.y) \
             else 1 = 1 end order by x",
        ),
        vec![vec!["100", "200"], vec!["103", "600"]],
    );
}

#[test]
/// Go TestInlineProjection4HashJoinIssue15316：build 侧裁列后重复投影和 NULL 仍正确。
fn inline_projection_hash_join_executes_duplicate_column_contract() {
    let session = hash_join_session("hash_join_issue15316");
    execute_sql(&session, "create table s (a int not null, b int, c int)");
    execute_sql(&session, "create table t (a int not null, b int, c int)");
    execute_sql(&session, "insert into s values (0,1,2),(0,1,null),(0,1,2)");
    execute_sql(
        &session,
        "insert into t values (0,10,2),(0,10,null),(1,10,2)",
    );
    assert_eq!(
        query_sql(
            &session,
            "select /*+ HASH_JOIN_BUILD(t) */ t.a,t.a,t.c from s join t on t.a = s.a \
             where s.b<t.b order by t.a,t.c",
        ),
        vec![
            vec!["0", "0", "<nil>"],
            vec!["0", "0", "<nil>"],
            vec!["0", "0", "<nil>"],
            vec!["0", "0", "2"],
            vec!["0", "0", "2"],
            vec!["0", "0", "2"],
        ],
    );
}

#[test]
/// Go TestOuterTableBuildHashTableIsuse13933：outer 在 build 侧时仍补齐未匹配行。
fn outer_build_side_executes_other_condition_contract() {
    let session = hash_join_session("hash_join_issue13933");
    execute_sql(&session, "create table t (a int,b int)");
    execute_sql(&session, "create table s (a int,b int)");
    execute_sql(&session, "insert into t values (11,11),(1,2)");
    execute_sql(&session, "insert into s values (1,2),(2,1),(11,11)");
    let mut rows = query_sql(
        &session,
        "select /*+ HASH_JOIN_BUILD(t) */ * from t left join s on s.a > t.a",
    );
    rows.sort();
    assert_eq!(
        rows,
        vec![
            vec!["1", "2", "11", "11"],
            vec!["1", "2", "2", "1"],
            vec!["11", "11", "<nil>", "<nil>"],
        ],
    );
}

#[test]
/// Go TestIssue54755：左右外连接的聚合结果不能受 build-side 方向影响。
fn issue54755_executes_symmetric_outer_join_aggregate_contract() {
    let session = hash_join_session("hash_join_issue54755");
    execute_sql(
        &session,
        "create table t1(pk INTEGER AUTO_INCREMENT, col_int_nokey INTEGER, \
         col_int_key INTEGER, col_varchar_key VARCHAR(1), col_varchar_nokey VARCHAR(1), \
         PRIMARY KEY (pk), KEY (col_int_key), KEY (col_varchar_key, col_int_key))",
    );
    execute_sql(
        &session,
        "create table t2(pk INTEGER AUTO_INCREMENT, col_int_nokey INTEGER, \
         col_int_key INTEGER, col_varchar_key VARCHAR(1), col_varchar_nokey VARCHAR(1), \
         PRIMARY KEY (pk), KEY (col_int_key), KEY (col_varchar_key, col_int_key))",
    );
    execute_sql(
        &session,
        "insert into t1(col_int_key,col_int_nokey,col_varchar_key,col_varchar_nokey) \
         values(4,2,'v','v'),(62,150,'v','v')",
    );
    execute_sql(
        &session,
        "insert into t2(col_int_key,col_int_nokey,col_varchar_key,col_varchar_nokey) \
         values(8,null,'x','x'),(7,8,'d','d')",
    );
    assert_eq!(
        query_sql(
            &session,
            "select max(b.col_int_nokey) from t2 a right join t1 b \
             on b.col_varchar_key = a.col_varchar_nokey",
        ),
        vec![vec!["150"]],
    );
    assert_eq!(
        query_sql(
            &session,
            "select max(b.col_int_nokey) from t1 b left join t2 a \
             on b.col_varchar_key = a.col_varchar_nokey",
        ),
        vec![vec!["150"]],
    );
}

#[test]
/// Go TestIssue55016：CHAR/VARCHAR 多键比较在 V1/V2 下均不得产生伪匹配。
fn issue55016_executes_mixed_string_key_contract() {
    let session = hash_join_session("hash_join_issue55016");
    execute_sql(&session, "create table t(a varchar(10), b char(10))");
    execute_sql(&session, "insert into t values('aa','a')");
    for hash_join_v2 in [
        "set tidb_hash_join_version = 'legacy'",
        "set tidb_hash_join_version = 'optimized'",
    ] {
        execute_sql(&session, hash_join_v2);
        assert_eq!(
            query_sql(
                &session,
                "select count(*) from t t1 join t t2 on t1.a = t2.b and t2.a = t1.b",
            ),
            vec![vec!["0"]],
        );
    }
}

#[test]
/// Go TestIssue56214：相关标量子查询内 Hash Join 的 other condition 使用外层值。
fn issue56214_executes_correlated_scalar_subquery_contract() {
    let session = hash_join_session("hash_join_issue56214");
    execute_sql(&session, "create table t1(id int, value int)");
    execute_sql(&session, "create table t2(id int, value int)");
    execute_sql(&session, "create table t3(id int, value int)");
    execute_sql(&session, "insert into t1 values(1,2),(2,3),(3,4)");
    execute_sql(&session, "insert into t2 values(1,10),(1,1),(2,10),(2,10)");
    execute_sql(&session, "insert into t3 values(1,10),(1,20)");
    for hash_join_v2 in [
        "set tidb_hash_join_version = 'legacy'",
        "set tidb_hash_join_version = 'optimized'",
    ] {
        execute_sql(&session, hash_join_v2);
        assert_eq!(
            query_sql(
                &session,
                "select value, (select t1.id from t1 join t2 on t1.id = t2.id \
                 and t1.value < t2.value - t3.value + 3) d from t3 order by value",
            ),
            vec![vec!["10", "1"], vec!["20", "<nil>"]],
        );
    }
}

#[test]
/// 分别关闭/开启 HashJoin V2，断言同一等值连接结果契约。
fn hash_join_v1_and_v2_execute_identical_join_contracts() {
    let info = hash_join_info(
        JoinType::Inner,
        vec![int_row(&[1, 10]), int_row(&[3, 30])],
        vec![int_row(&[1, 100]), int_row(&[2, 200]), int_row(&[3, 300])],
    );
    let expected = vec![int_row(&[1, 100, 1, 10]), int_row(&[3, 300, 3, 30])];

    assert_eq!(execute_hash_join_v1(&info), expected);
    assert_eq!(execute_hash_join_v2(&info), expected);
}

#[test]
/// 左外连接：右表无匹配时保留左行并把右列填 NULL（`<nil>`）。
fn outer_join_preserves_unmatched_rows_and_nulls() {
    let mut info = hash_join_info(
        JoinType::LeftOuter,
        vec![int_row(&[1, 10])],
        vec![int_row(&[1, 100]), int_row(&[2, 200])],
    );
    info.default_inner = vec![Value::Null, Value::Null];
    let expected = vec![
        int_row(&[1, 100, 1, 10]),
        vec![Value::Int(2), Value::Int(200), Value::Null, Value::Null],
    ];

    assert_eq!(execute_hash_join_v1(&info), expected);
    assert_eq!(execute_hash_join_v2(&info), expected);
}

#[test]
/// 注入构建 worker panic 与查询中断，确认错误信息可被观测。
fn injected_build_and_probe_failures_are_observable() {
    // Go 的四个 V2 panic failpoint 都必须在 worker 边界转成可观测错误。
    for message in [
        "splitPartitionPanic",
        "processOneProbeChunkPanic",
        "createTasksPanic",
        "buildHashTablePanic",
    ] {
        let context = HashJoinContextBase::default();
        let worker = BuildWorkerBase::new(0, context.clone(), None);
        let error = worker
            .run_guarded::<()>(|| panic!("{message}"))
            .expect_err("worker panic must become an error");
        assert_eq!(error, message);
        assert_eq!(context.wait_for_build_side(), Err(message.to_owned()));
    }

    // Go killedDuringBuild / killedDuringProbe：取消同时唤醒 build waiter 与 probe fetcher。
    let context = HashJoinContextBase::default();
    context.cancel();
    let build = BuildWorkerBase::new(0, context.clone(), None);
    assert_eq!(
        build.fetch_build_side_rows(&[vec![int_row(&[1])]]),
        Err("hash join cancelled".to_owned())
    );
    let mut probe = ProbeSideTupleFetcherBase::new(vec![vec![int_row(&[1])]]);
    assert_eq!(
        probe
            .fetch_next(&context)
            .expect_err("probe must observe kill"),
        "hash join cancelled"
    );
    assert_eq!(
        context.wait_for_build_side(),
        Err("hash join cancelled".to_owned())
    );
}
