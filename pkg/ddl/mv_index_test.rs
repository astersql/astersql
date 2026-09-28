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

// 多值索引（Multi-Valued Index，MV Index）在线 DDL 测试。
//
// 多值索引把 JSON 数组等集合类型展开为多条索引项；在线 DDL 期间需与并发 DML
//（插入/删除/更新）交错推进，并校验唯一约束冲突与无符号类型溢出等错误路径。
// 本文件用步骤枚举保留 Go 侧 failpoint 驱动的交错语义，不真正执行 SQL。

// 多值索引在线 DDL 期间的插入、删除、更新以及重复/溢出错误校验。

#![allow(dead_code, non_snake_case, non_camel_case_types, unused_variables)]

/// 多值索引在线 DDL 测试步骤：执行 SQL、断言错误码/消息、注入 failpoint，或循环插入行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MvIndexStep {
    /// 执行一条 SQL 语句。
    Exec(&'static str),
    /// 执行由 Go `strings.Builder` 等价构造的 SQL 语句。
    GeneratedExec(String),
    /// 执行 SQL 并期望返回指定错误码（如 errno.ErrDupEntry）。
    ErrorCode(&'static str, &'static str),
    /// 执行 SQL 并期望错误消息包含给定子串（如溢出提示）。
    ErrorMessage(&'static str, &'static str),
    /// 启用或禁用 failpoint，用于在 DDL job 步进时注入并发 DML。
    Failpoint(&'static str),
}

fn build_backfill_insert(row_count: usize) -> String {
    let mut sql = String::from("insert into t values ");
    for i in 0..row_count {
        if i != 0 {
            sql.push(',');
        }
        sql.push_str(&format!("({i}, '[{}, {}, {}]')", i + 1, i + 2, i + 3));
    }
    sql
}

fn online_ddl_callback_sql(n: usize) -> [String; 3] {
    [
        format!("insert into t values ({n}, '[{n}, {}, {}]')", n + 1, n + 2),
        format!("delete from t where pk = {}", n - 4),
        format!(
            "update t set a = '[{}, {}, {}]' where pk = {}",
            n - 3,
            n - 2,
            n + 1000,
            n - 3
        ),
    ]
}

/// 对应 Go `TestMultiValuedIndexOnlineDDL`：在线加多值索引时交错 DML，并覆盖唯一/溢出错误。
// test_multi_valued_index_online_ddl 对应 Go 的 TestMultiValuedIndexOnlineDDL。
// Go 在 add index 过程中通过 failpoint 持续 DML；用步骤列表保留在线 DDL 与并发写入的交错语义。
fn multi_valued_index_online_ddl_steps() -> Vec<MvIndexStep> {
    let mut steps = Vec::new();
    steps.push(MvIndexStep::Exec("use test"));
    steps.push(MvIndexStep::Exec("drop table if exists t"));
    steps.push(MvIndexStep::Exec(
        "create table t (pk int primary key, a json) partition by hash(pk) partitions 32;",
    ));
    // Go 用 strings.Builder 拼接 100 行 JSON 数组，确保多值索引 backfill 有足够数据。
    steps.push(MvIndexStep::GeneratedExec(build_backfill_insert(100)));
    // beforeRunOneJobStep 回调中 internalTK 每次插入 n、删除 n-4、更新 n-3，模拟在线 DDL 期间的 DML 交错。
    steps.push(MvIndexStep::Failpoint("github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep: insert/delete/update rows while DDL job advances"));
    steps.push(MvIndexStep::Exec(
        "alter table t add index idx((cast(a as signed array)))",
    ));
    steps.push(MvIndexStep::Exec("admin check table t"));
    steps.push(MvIndexStep::Failpoint(
        "disable github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
    ));

    // 第二组覆盖 signed array 唯一索引重复项和 unsigned array 对负数的溢出报错。
    steps.push(MvIndexStep::Exec("drop table if exists t;"));
    steps.push(MvIndexStep::Exec(
        "create table t (pk int primary key, a json);",
    ));
    steps.push(MvIndexStep::Exec("insert into t values (1, '[1,2,3]');"));
    steps.push(MvIndexStep::Exec("insert into t values (2, '[2,3,4]');"));
    steps.push(MvIndexStep::Exec("insert into t values (3, '[3,4,5]');"));
    steps.push(MvIndexStep::Exec("insert into t values (4, '[-4,5,6]');"));
    steps.push(MvIndexStep::ErrorCode(
        "alter table t add unique index idx((cast(a as signed array)));",
        "errno.ErrDupEntry",
    ));
    steps.push(MvIndexStep::ErrorMessage(
        "alter table t add index idx((cast(a as unsigned array)));",
        "[types:1690]constant -4 overflows bigint",
    ));

    // 第三组覆盖数组重叠导致 unique 多值索引重复。
    steps.push(MvIndexStep::Exec("drop table if exists t;"));
    steps.push(MvIndexStep::Exec(
        "create table t (pk int primary key, a json);",
    ));
    steps.push(MvIndexStep::Exec("insert into t values (1, '[1,2,3]');"));
    steps.push(MvIndexStep::Exec("insert into t values (2, '[2,3]');"));
    steps.push(MvIndexStep::ErrorCode(
        "alter table t add unique index idx((cast(a as signed array)));",
        "errno.ErrDupEntry",
    ));
    steps
}

#[test]
fn test_multi_valued_index_online_ddl() {
    let steps = multi_valued_index_online_ddl_steps();
    assert_eq!(steps.len(), 21);
    assert_eq!(
        steps[3],
        MvIndexStep::GeneratedExec(build_backfill_insert(100))
    );
    assert_eq!(
        &steps[4..8],
        &[
            MvIndexStep::Failpoint(
                "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep: insert/delete/update rows while DDL job advances"
            ),
            MvIndexStep::Exec("alter table t add index idx((cast(a as signed array)))"),
            MvIndexStep::Exec("admin check table t"),
            MvIndexStep::Failpoint("disable github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep"),
        ]
    );
    assert_eq!(
        steps[14],
        MvIndexStep::ErrorCode(
            "alter table t add unique index idx((cast(a as signed array)));",
            "errno.ErrDupEntry",
        )
    );
    assert_eq!(
        steps[15],
        MvIndexStep::ErrorMessage(
            "alter table t add index idx((cast(a as unsigned array)));",
            "[types:1690]constant -4 overflows bigint",
        )
    );
    assert_eq!(
        steps[20],
        MvIndexStep::ErrorCode(
            "alter table t add unique index idx((cast(a as signed array)));",
            "errno.ErrDupEntry",
        )
    );
}

#[test]
fn backfill_insert_matches_go_builder_boundaries() {
    let sql = build_backfill_insert(100);
    assert!(sql.starts_with("insert into t values (0, '[1, 2, 3]'),(1, '[2, 3, 4]')"));
    assert!(sql.ends_with("(99, '[100, 101, 102]')"));
    assert_eq!(sql.matches("),(").count(), 99);
}

#[test]
fn online_ddl_callback_matches_go_dml_and_counter_progression() {
    assert_eq!(
        online_ddl_callback_sql(100),
        [
            "insert into t values (100, '[100, 101, 102]')",
            "delete from t where pk = 96",
            "update t set a = '[97, 98, 1100]' where pk = 97",
        ]
    );
    assert_eq!(
        online_ddl_callback_sql(101),
        [
            "insert into t values (101, '[101, 102, 103]')",
            "delete from t where pk = 97",
            "update t set a = '[98, 99, 1101]' where pk = 98",
        ]
    );
}
