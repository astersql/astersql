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

// 列脱敏策略（masking policy）DDL 相关测试。
//
// 脱敏策略定义查询结果如何遮蔽敏感列值。下方大块注释保留尚未接线的
// Go 集成测试草稿；文件末尾是当前可运行的执行器级单元测试：改名后策略
// 仍绑定该列，且不兼容的类型变更会被拒绝。

/*
//

// test_masking_policy_ddl_basic 对应 Go 的 TestMaskingPolicyDDLBasic：覆盖 create/disable/enable/replace/drop。
#[test]
fn test_masking_policy_ddl_basic() {
    let store = testkit::CreateMockStore(mockstore::WithDDLChecker());
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t (id int primary key auto_increment, c char(120))");

    tk.MustExec("create masking policy p on t(c) as c");
    tk.MustQuery("select policy_name, db_name, table_name, column_name, expression, status, masking_type, restrict_on from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("p test t c `c` ENABLED CUSTOM NONE"));

    tk.MustExec("alter table t disable masking policy p");
    tk.MustQuery("select status from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("DISABLED"));

    tk.MustExec("alter table t enable masking policy p");
    tk.MustQuery("select status from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("ENABLED"));

    // CREATE OR REPLACE 会覆盖同名 policy，并把表达式保存为规范化后的 SQL 文本。
    tk.MustExec("create or replace masking policy p on t(c) as concat(c, '_x')");
    tk.MustQuery("select expression like 'CONCAT(%', masking_type from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("1 CUSTOM"));

    tk.MustExec("alter table t drop masking policy p");
    tk.MustQuery("select count(*) from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("0"));
}

// test_masking_policy_case_expression 对应 Go 测试：CASE 表达式会被保存并归一化为大写函数名。
#[test]
fn test_masking_policy_case_expression() {
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t (c char(120))");

    tk.MustExec("create masking policy p_case on t(c) as case when current_user() = 'root' then c else 'xxx' end enable");
    tk.MustQuery("select policy_name, status from mysql.tidb_masking_policy where policy_name = 'p_case'")
        .Check(testkit::Rows("p_case ENABLED"));
    tk.MustQuery("select expression like 'CASE WHEN %' from mysql.tidb_masking_policy where policy_name = 'p_case'")
        .Check(testkit::Rows("1"));
    tk.MustQuery("select expression like '%CURRENT_USER()%' from mysql.tidb_masking_policy where policy_name = 'p_case'")
        .Check(testkit::Rows("1"));
}

// test_masking_policy_if_not_exists 对应 Go 测试：并发 DDL helper 执行 IF NOT EXISTS 后只保留一条策略。
#[test]
fn test_masking_policy_if_not_exists() {
    // Go 用带 schema lease 的 mock store 配合并发 DDL helper，验证 IF NOT EXISTS 的幂等语义。
    let store = testkit::CreateMockStoreWithSchemaLease(200 * time::Millisecond);
    let tk = testkit::NewTestKit(store.clone());
    tk.MustExec("create database test_db_state default charset utf8 default collate utf8_bin");
    tk.MustExec("use test_db_state");
    tk.MustExec("create table t_mask (c char(120))");

    // dbChangeTestParallelExecSQL runs SQL concurrently in Go tests.
    dbChangeTestParallelExecSQL(
        store,
        "create masking policy if not exists p on t_mask(c) as c",
    );

    tk.MustQuery("select count(*) from mysql.tidb_masking_policy where db_name = 'test_db_state' and table_name = 't_mask' and policy_name = 'p'")
        .Check(testkit::Rows("1"));
}

// test_masking_policy_rename_table 对应 Go 测试：同库 rename table 后 sys 表元数据更新，策略仍可删除。
#[test]
fn test_masking_policy_rename_table() {
    let store = testkit::CreateMockStore(mockstore::WithDDLChecker());
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists old_table, new_table");

    // Create table and masking policy
    tk.MustExec("create table old_table(id int primary key, c varchar(100))");
    tk.MustExec("insert into old_table values (1, 'secret')");
    tk.MustExec("create masking policy p on old_table(c) as c enable");

    // Verify policy metadata before rename
    tk.MustQuery("select db_name, table_name from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("test old_table"));

    // Rename the table
    tk.MustExec("rename table old_table to new_table");

    // Verify policy metadata is updated in sys table
    tk.MustQuery("select db_name, table_name from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("test new_table"));

    // Verify we can drop the policy after rename
    tk.MustExec("alter table new_table drop masking policy p");
    tk.MustQuery("select count(*) from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("0"));
    tk.MustQuery("select c from new_table")
        .Check(testkit::Rows("secret"));
}

// test_masking_policy_rename_table_cross_database 对应 Go 测试：跨库 rename table 更新 db_name/table_name。
#[test]
fn test_masking_policy_rename_table_cross_database() {
    let store = testkit::CreateMockStore(mockstore::WithDDLChecker());
    let tk = testkit::NewTestKit(store);
    tk.MustExec("drop database if exists db1");
    tk.MustExec("drop database if exists db2");

    // Create databases and table
    tk.MustExec("create database db1");
    tk.MustExec("create database db2");
    tk.MustExec("create table db1.t(id int primary key, c varchar(100))");
    tk.MustExec("insert into db1.t values (1, 'secret')");
    tk.MustExec("create masking policy p on db1.t(c) as c enable");

    // Verify before rename
    tk.MustQuery("select db_name, table_name from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("db1 t"));

    // Rename the table across databases；Go 期望 policy 元数据同时更新库名和表名。
    tk.MustExec("rename table db1.t to db2.t");

    // Verify policy metadata is updated
    tk.MustQuery("select db_name, table_name from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("db2 t"));

    // Cleanup
    tk.MustExec("drop table db2.t");
    tk.MustExec("drop database db1");
    tk.MustExec("drop database db2");
}

// test_masking_policy_rename_table_no_policy 对应 Go 测试：无 policy 的表改名不产生清理或更新副作用。
#[test]
fn test_masking_policy_rename_table_no_policy() {
    let store = testkit::CreateMockStore(mockstore::WithDDLChecker());
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists old_table, new_table");

    // Create table without masking policy
    tk.MustExec("create table old_table(id int primary key, c varchar(100))");
    tk.MustExec("insert into old_table values (1, 'secret')");

    // Rename the table (no policy to update)
    tk.MustExec("rename table old_table to new_table");

    // Verify no error and table works
    tk.MustQuery("select c from new_table")
        .Check(testkit::Rows("secret"));
    tk.MustQuery("select count(*) from mysql.tidb_masking_policy")
        .Check(testkit::Rows("0"));
}

// test_masking_policy_rename_column 对应 Go 测试：rename column 同步 column_name 和 expression。
#[test]
fn test_masking_policy_rename_column() {
    let store = testkit::CreateMockStore(mockstore::WithDDLChecker());
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t_rename_col");
    tk.MustExec("create table t_rename_col(id int primary key, c varchar(20))");
    tk.MustExec("insert into t_rename_col values (1, 'delta')");
    tk.MustExec("create masking policy p_rename_col on t_rename_col(c) as c enable");

    // Verify policy exists before rename
    tk.MustQuery("select column_name, expression from mysql.tidb_masking_policy where policy_name = 'p_rename_col'")
        .Check(testkit::Rows("c `c`"));

    // Rename column；DDL 路径需要同时改 column_name，并重写表达式中的列引用。
    tk.MustExec("alter table t_rename_col rename column c to c_new");

    // Verify column_name and expression are updated in sys table
    tk.MustQuery("select column_name, expression from mysql.tidb_masking_policy where policy_name = 'p_rename_col'")
        .Check(testkit::Rows("c_new `c_new`"));

    // Verify select still works
    tk.MustQuery("select c_new from t_rename_col")
        .Check(testkit::Rows("delta"));
}

// test_masking_policy_modify_column_reject_unsupported_type 对应 Go 测试：带 policy 的列不能改成 JSON。
#[test]
fn test_masking_policy_modify_column_reject_unsupported_type() {
    let store = testkit::CreateMockStore(mockstore::WithDDLChecker());
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t_mod");
    tk.MustExec("create table t_mod(id int primary key, c varchar(100))");
    tk.MustExec("create masking policy p on t_mod(c) as c enable");

    // MODIFY COLUMN to JSON (unsupported type) should be rejected.
    tk.MustGetErrCode(
        "alter table t_mod modify column c json",
        errno::ErrUnsupportedDDLOperation,
    );

    // CHANGE COLUMN to JSON should also be rejected.
    tk.MustGetErrCode(
        "alter table t_mod change column c c2 json",
        errno::ErrUnsupportedDDLOperation,
    );

    // Verify the policy is still intact.
    tk.MustQuery("select column_name, expression from mysql.tidb_masking_policy where policy_name = 'p'")
        .Check(testkit::Rows("c `c`"));
}

// test_masking_policy_expression_rejects_non_target_column 对应 Go 测试：表达式只能引用目标列。
#[test]
fn test_masking_policy_expression_rejects_non_target_column() {
    let store = testkit::CreateMockStore(mockstore::WithDDLChecker());
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t_expr_dep");
    tk.MustExec("create table t_expr_dep(a varchar(100), b varchar(100))");

    // CREATE MASKING POLICY ... ON t(a) AS b must fail because expression references non-target column b.
    tk.MustGetErrCode(
        "create masking policy p_expr_dep on t_expr_dep(a) as b",
        errno::ErrMaskingPolicyExprInvalidColumn,
    );

    // Expression referencing both target and non-target columns must also fail.
    tk.MustGetErrCode(
        "create masking policy p_expr_dep2 on t_expr_dep(a) as concat(a, b)",
        errno::ErrMaskingPolicyExprInvalidColumn,
    );

    // Expression referencing only the target column must succeed.
    tk.MustExec("create masking policy p_valid on t_expr_dep(a) as a enable");
    tk.MustQuery("select expression from mysql.tidb_masking_policy where policy_name = 'p_valid'")
        .Check(testkit::Rows("`a`"));

    // CREATE OR REPLACE with non-target column reference must also fail.
    tk.MustGetErrCode(
        "create or replace masking policy p_valid on t_expr_dep(a) as b",
        errno::ErrMaskingPolicyExprInvalidColumn,
    );

    // ALTER TABLE ... MODIFY MASKING POLICY with non-target column reference must fail.
    tk.MustGetErrCode(
        "alter table t_expr_dep modify masking policy p_valid set expression = b",
        errno::ErrMaskingPolicyExprInvalidColumn,
    );

    // ALTER TABLE ... MODIFY MASKING POLICY with target column reference must succeed.
    tk.MustExec("alter table t_expr_dep modify masking policy p_valid set expression = concat(a, '_masked')");
}

// test_masking_policy_truncate_keeps_policy 对应 Go 测试：truncate 后 policy 保留但 table_id 变化。
#[test]
fn test_masking_policy_truncate_keeps_policy() {
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t_trunc");
    tk.MustExec("create table t_trunc(id int primary key, c varchar(100))");
    tk.MustExec("insert into t_trunc values (1, 'secret')");
    tk.MustExec("create masking policy p_trunc on t_trunc(c) as c enable");

    // Verify policy exists before truncate
    tk.MustQuery("select count(*) from mysql.tidb_masking_policy where policy_name = 'p_trunc'")
        .Check(testkit::Rows("1"));

    // Capture the table_id before truncate
    // Go 通过 sys 表读取 truncate 前的 table_id，后面用 require.NotEqual 验证 ID 已换新。
    let rs = tk.MustQuery("select table_id from mysql.tidb_masking_policy where policy_name = 'p_trunc'");
    let old_table_id = rs.Rows()[0][0].to_string();

    // Truncate the table
    tk.MustExec("truncate table t_trunc");

    // Verify policy still exists after truncate
    tk.MustQuery("select count(*) from mysql.tidb_masking_policy where policy_name = 'p_trunc'")
        .Check(testkit::Rows("1"));

    // Verify table_id was updated to a new table ID (different from old)
    let rs = tk.MustQuery("select table_id from mysql.tidb_masking_policy where policy_name = 'p_trunc'");
    let new_table_id = rs.Rows()[0][0].to_string();
    assert_ne!(
        old_table_id, new_table_id,
        "table_id should change after TRUNCATE TABLE"
    );

    // Verify we can still operate on the policy after truncate
    tk.MustExec("alter table t_trunc disable masking policy p_trunc");
    tk.MustQuery("select status from mysql.tidb_masking_policy where policy_name = 'p_trunc'")
        .Check(testkit::Rows("DISABLED"));

    tk.MustExec("alter table t_trunc enable masking policy p_trunc");
    tk.MustQuery("select status from mysql.tidb_masking_policy where policy_name = 'p_trunc'")
        .Check(testkit::Rows("ENABLED"));

    // Verify we can drop the policy after truncate
    tk.MustExec("alter table t_trunc drop masking policy p_trunc");
    tk.MustQuery("select count(*) from mysql.tidb_masking_policy where policy_name = 'p_trunc'")
        .Check(testkit::Rows("0"));
}

// test_masking_policy_drop_database_cleanup 对应 Go 测试：drop database 清理该库下所有 masking policy。
#[test]
fn test_masking_policy_drop_database_cleanup() {
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(store);

    // Create a separate database with tables and masking policies
    tk.MustExec("drop database if exists db_mask_cleanup");
    tk.MustExec("create database db_mask_cleanup");
    tk.MustExec("use db_mask_cleanup");
    tk.MustExec("create table t1(c varchar(100))");
    tk.MustExec("create table t2(c varchar(100))");
    tk.MustExec("create masking policy p1 on t1(c) as c enable");
    tk.MustExec("create masking policy p2 on t2(c) as c enable");

    // Verify policies exist
    tk.MustQuery("select count(*) from mysql.tidb_masking_policy where db_name = 'db_mask_cleanup'")
        .Check(testkit::Rows("2"));

    // Drop the database
    tk.MustExec("drop database db_mask_cleanup");

    // Verify all policies for this database are cleaned up
    tk.MustQuery("select count(*) from mysql.tidb_masking_policy where db_name = 'db_mask_cleanup'")
        .Check(testkit::Rows("0"));
}
*/

use crate::column::SchemaState;
use crate::executor::{
    ColumnInfo, ColumnKind, Executor, ExecutorError, Ident, MemoryJobBackend, OnExist,
    SessionContext, TableInfo,
};
use crate::masking_policy::{
    MaskingPolicyInfo, MaskingPolicyRestrictOps, MaskingPolicyStatus, MaskingPolicyStore,
    MaskingPolicyType, rewrite_masking_policy_expression_column_name,
};
use std::time::Duration;

fn masking_policy(table_id: i64, column_id: i64, name: &str) -> MaskingPolicyInfo {
    MaskingPolicyInfo {
        id: 0,
        name: name.into(),
        database_name: format!("db_{table_id}"),
        table_name: format!("t_{table_id}"),
        table_id,
        column_name: format!("c_{column_id}"),
        column_id,
        expression: format!("`c_{column_id}`"),
        status: MaskingPolicyStatus::Enable,
        masking_type: MaskingPolicyType::Custom,
        restrict_ops: MaskingPolicyRestrictOps::default(),
        created_at: 1,
        updated_at: 1,
        created_by: "root".into(),
        state: SchemaState::None,
    }
}

#[test]
fn duplicate_policy_names_are_scoped_to_a_table() {
    let mut store = MaskingPolicyStore::default();
    let first_id = store.create(masking_policy(10, 1, "p"), false).unwrap();
    let second_id = store.create(masking_policy(20, 1, "p"), false).unwrap();

    assert_ne!(first_id, second_id);
    assert_eq!(store.by_table(10).len(), 1);
    assert_eq!(store.by_table(20).len(), 1);
}

#[test]
fn renaming_a_column_does_not_rewrite_string_literals() {
    let rewritten = rewrite_masking_policy_expression_column_name(
        "concat(`secret`, 'secret')",
        "secret",
        "masked",
    )
    .unwrap();

    assert_eq!(rewritten, "concat(`masked`, 'secret')");
}

/// 跨库 rename 后列仍带脱敏策略引用，且改为不兼容类型时返回 Dependency 错误。
#[test]
fn masking_policy_survives_rename_and_blocks_incompatible_type_change() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    // 准备源库与目标库，创建带 masking_policy 引用的列。
    ddl.create_schema(&mut session, "one", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_schema(&mut session, "two", &[], None, OnExist::Error)
        .unwrap();
    let mut protected = ColumnInfo::integer("secret");
    protected.masking_policy = Some(42);
    ddl.create_table(
        &mut session,
        "one",
        TableInfo::new("t", vec![protected]),
        OnExist::Error,
    )
    .unwrap();
    // 跨库改名后策略依赖应随表迁移保留。
    ddl.rename_tables(
        &mut session,
        &[(Ident::new("one", "t"), Ident::new("two", "renamed"))],
    )
    .unwrap();
    // 改为 String 与既有策略不兼容，期望 Dependency("masking policy")。
    let mut replacement = ColumnInfo::integer("secret");
    replacement.kind = ColumnKind::String;
    assert!(matches!(
        ddl.modify_column(&mut session, &Ident::new("two", "renamed"), "secret", replacement),
        Err(ExecutorError::Dependency(message)) if message == "masking policy"
    ));
}
