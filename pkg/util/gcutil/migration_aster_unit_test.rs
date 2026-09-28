// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// GC（Garbage Collection，垃圾回收）工具迁移补充单元测试。
//
// 用 mock 会话上下文验证 `tidb_gc_enable` 开关读写、GC safe point
// （安全点：低于该时间戳的数据可被回收）查询/解析，以及快照相对
// safe point 的边界校验（错误码 8055）与 Go 行为一致。

use std::sync::Mutex;

use crate::*;

/// 将字符串消息包装为依赖层 `GoError`，便于断言错误文案。
fn dependency_error(message: &str) -> sessionctx::GoError {
    std::io::Error::other(message).into()
}

/// 模拟全局系统变量访问器：记录读值、写操作序列，并可注入 get/set 错误。
#[derive(Default)]
struct MockGlobalVars {
    value: Mutex<String>,
    sets: Mutex<Vec<(String, String)>>,
    get_error: Mutex<Option<String>>,
    set_error: Mutex<Option<String>>,
}

impl GlobalVarAccessor for MockGlobalVars {
    fn get_global_sys_var(&self, name: &str) -> Result<String, sessionctx::GoError> {
        // 生产路径只关心 tidb_gc_enable；名称不符则测试失败。
        assert_eq!(name, "tidb_gc_enable");
        if let Some(error) = self.get_error.lock().unwrap().take() {
            return Err(dependency_error(&error));
        }
        Ok(self.value.lock().unwrap().clone())
    }

    fn set_global_sys_var(&self, name: &str, value: &str) -> Result<(), sessionctx::GoError> {
        if let Some(error) = self.set_error.lock().unwrap().take() {
            return Err(dependency_error(&error));
        }
        // 记录每次写入，便于校验 DisableGC/EnableGC 的变量名与取值。
        self.sets
            .lock()
            .unwrap()
            .push((name.to_owned(), value.to_owned()));
        *self.value.lock().unwrap() = value.to_owned();
        Ok(())
    }
}

/// 模拟受限 SQL 执行器：记录调用参数，并可返回预设行集或注入错误。
#[derive(Default)]
struct MockRestrictedSql {
    rows: Mutex<Vec<RestrictedRow>>,
    error: Mutex<Option<String>>,
    calls: Mutex<Vec<(String, Vec<String>, String)>>,
}

impl RestrictedSqlExecutor for MockRestrictedSql {
    fn exec_restricted_sql(
        &self,
        ctx: RestrictedSqlContext,
        sql: &str,
        arguments: &[&str],
    ) -> Result<Vec<RestrictedRow>, sessionctx::GoError> {
        // 保留 SQL、参数与 internal source type，用于对齐 Go 查询路径。
        self.calls.lock().unwrap().push((
            sql.to_owned(),
            arguments.iter().map(|value| (*value).to_owned()).collect(),
            ctx.internal_source_type().to_owned(),
        ));
        if let Some(error) = self.error.lock().unwrap().take() {
            return Err(dependency_error(&error));
        }
        Ok(self.rows.lock().unwrap().clone())
    }
}

/// 聚合全局变量与受限 SQL 两路依赖的 mock 会话上下文。
#[derive(Default)]
struct MockContext {
    globals: MockGlobalVars,
    sql: MockRestrictedSql,
}

impl Context for MockContext {
    fn global_vars_accessor(&self) -> &dyn GlobalVarAccessor {
        &self.globals
    }

    fn restricted_sql_executor(&self) -> &dyn RestrictedSqlExecutor {
        &self.sql
    }
}

/// 构造已预置一行 safe point 时间字符串的 mock 上下文。
fn context_with_safe_point(value: &str) -> MockContext {
    let ctx = MockContext::default();
    ctx.sql
        .rows
        .lock()
        .unwrap()
        .push(RestrictedRow::new(vec![value.to_owned()]));
    ctx
}

/// 校验 GC 开关：大小写/数字真值解析，以及 Disable/Enable 写入 OFF/ON。
#[test]
fn gc_switch_reads_and_writes_the_go_global_variable() {
    let ctx = MockContext::default();
    *ctx.globals.value.lock().unwrap() = "oN".to_owned();
    assert!(CheckGCEnable(&ctx).unwrap());

    *ctx.globals.value.lock().unwrap() = "1".to_owned();
    assert!(CheckGCEnable(&ctx).unwrap());

    DisableGC(&ctx).unwrap();
    assert!(!CheckGCEnable(&ctx).unwrap());
    EnableGC(&ctx).unwrap();
    assert!(CheckGCEnable(&ctx).unwrap());

    assert_eq!(
        *ctx.globals.sets.lock().unwrap(),
        vec![
            ("tidb_gc_enable".to_owned(), "OFF".to_owned()),
            ("tidb_gc_enable".to_owned(), "ON".to_owned()),
        ]
    );
}

/// 全局变量访问器错误应原样向上传播。
#[test]
fn gc_switch_propagates_accessor_errors() {
    let ctx = MockContext::default();
    *ctx.globals.get_error.lock().unwrap() = Some("read failed".to_owned());
    assert_eq!(CheckGCEnable(&ctx).unwrap_err().to_string(), "read failed");

    *ctx.globals.set_error.lock().unwrap() = Some("write failed".to_owned());
    assert_eq!(DisableGC(&ctx).unwrap_err().to_string(), "write failed");
}

/// GetGCSafePoint：SQL/参数/internal source 固定，时间解析后左移 18 位成 TSO。
#[test]
fn get_gc_safe_point_uses_the_exact_sql_internal_source_and_go_time_conversion() {
    let ctx = context_with_safe_point("20181218-19:53:37 +0800 CST");
    // TSO（Timestamp Oracle）物理毫秒左移 18 位逻辑部分，与 Go 一致。
    let expected_millis = 1_545_134_017_000_u64;
    assert_eq!(GetGCSafePoint(&ctx).unwrap(), expected_millis << 18);

    assert_eq!(
        *ctx.sql.calls.lock().unwrap(),
        vec![(
            selectVariableValueSQL.to_owned(),
            vec!["tikv_gc_safe_point".to_owned()],
            "gc".to_owned(),
        )]
    );
}

/// 接受毫秒小数格式；空结果或多行均报 “can not get 'tikv_gc_safe_point'”。
#[test]
fn get_gc_safe_point_accepts_fractional_format_and_rejects_bad_row_shapes() {
    let ctx = context_with_safe_point("20181218-11:53:37.123 +0000");
    assert_eq!(GetGCSafePoint(&ctx).unwrap(), 1_545_134_017_123_u64 << 18);

    let missing = MockContext::default();
    assert_eq!(
        GetGCSafePoint(&missing).unwrap_err().to_string(),
        "can not get 'tikv_gc_safe_point'"
    );

    // 多行结果同样视为无法唯一取得 safe point。
    let duplicate = context_with_safe_point("20181218-11:53:37 +0000");
    duplicate
        .sql
        .rows
        .lock()
        .unwrap()
        .push(RestrictedRow::new(vec![
            "20181218-11:53:38 +0000".to_owned(),
        ]));
    assert_eq!(
        GetGCSafePoint(&duplicate).unwrap_err().to_string(),
        "can not get 'tikv_gc_safe_point'"
    );
}

/// GoTimeToTS 对 epoch 前时间先将纳秒除为毫秒（向零截断），再按 i64 左移并转 u64。
#[test]
fn get_gc_safe_point_matches_go_for_pre_epoch_milliseconds() {
    let one_millisecond_before_epoch = context_with_safe_point("19691231-23:59:59.999 +0000");
    assert_eq!(
        GetGCSafePoint(&one_millisecond_before_epoch).unwrap(),
        ((-1_i64) << 18) as u64
    );

    let half_millisecond_before_epoch = context_with_safe_point("19691231-23:59:59.9995 +0000");
    assert_eq!(GetGCSafePoint(&half_millisecond_before_epoch).unwrap(), 0);
}

/// SQL 执行失败与时间串解析失败分别传播对应错误文案。
#[test]
fn get_gc_safe_point_propagates_sql_and_parse_errors() {
    let sql_error = MockContext::default();
    *sql_error.sql.error.lock().unwrap() = Some("restricted SQL failed".to_owned());
    assert_eq!(
        GetGCSafePoint(&sql_error).unwrap_err().to_string(),
        "restricted SQL failed"
    );

    let invalid = context_with_safe_point("not-a-gc-time");
    assert_eq!(
        GetGCSafePoint(&invalid).unwrap_err().to_string(),
        "string \"not-a-gc-time\" doesn't has a prefix that matches format \"20060102-15:04:05.000 -0700\""
    );
}

/// 与 Go client 时间表对齐：合法时区/后缀变体通过，非法串报 InvalidSafePointTime。
#[test]
fn gc_time_compatibility_matches_the_client_go_table() {
    let valid_values = [
        "20181218-19:53:37 +0800 CST",
        "20181218-19:53:37 +0800 MST",
        "20181218-19:53:37 +0800 FOO",
        "20181218-19:53:37 +0800 +08",
        "20181218-19:53:37 +0800",
        "20181218-19:53:37 +0800 ",
        "20181218-11:53:37 +0000",
        "20181218-11:53:37.000 +0000",
        "20181218-19:53:37.000 +0800 +08",
    ];
    for value in valid_values {
        let ctx = context_with_safe_point(value);
        assert_eq!(GetGCSafePoint(&ctx).unwrap(), 1_545_134_017_000_u64 << 18);
    }

    let invalid_values = [
        "",
        " ",
        "foo",
        "20181218-11:53:37",
        "20181218-19:53:37 +0800CST",
        "20181218-19:53:37 +0800 FOO BAR",
        "20181218-19:53:37 +0800FOOOOOOO BAR",
        "20181218-19:53:37 ",
    ];
    for value in invalid_values {
        let ctx = context_with_safe_point(value);
        assert!(matches!(
            GetGCSafePoint(&ctx),
            Err(GcUtilError::InvalidSafePointTime { .. })
        ));
    }
}

/// 快照不得早于 GC safe point；越界返回 MySQL 错误码 8055。
#[test]
fn snapshot_validation_matches_go_boundary_and_error_code() {
    let safe_point = 1_545_134_017_000_u64 << 18;
    ValidateSnapshotWithGCSafePoint(safe_point, safe_point).unwrap();
    ValidateSnapshotWithGCSafePoint(safe_point + 1, safe_point).unwrap();

    let error = ValidateSnapshotWithGCSafePoint(safe_point - 1, safe_point).unwrap_err();
    assert_eq!(error.code(), Some(8055));
    assert_eq!(
        error.to_string(),
        "snapshot is older than GC safe point 2018-12-18 11:53:37 +0000 UTC"
    );

    // 经上下文读取 safe point 的包装路径同样返回 8055。
    let ctx = context_with_safe_point("20181218-11:53:37 +0000");
    assert_eq!(
        ValidateSnapshot(&ctx, safe_point - 1).unwrap_err().code(),
        Some(8055)
    );
}
