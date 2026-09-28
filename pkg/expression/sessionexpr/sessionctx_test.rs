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

// 复用 migration 夹具；下列五个测试与 Go sessionctx_test.go 一一对应。
// Reuse the production task's native fixture. The five tests below remain a
// one-to-one port of sessionctx_test.go; the included migration tests provide
// additional coverage for the same production closure.

// sessionctx 单元测试（对应 Go `sessionctx_test.go`）。
//
// 通过 include 复用 migration 夹具，覆盖 EvalContext 基础字段、当前时间、
// 权限、可选属性与 ExprContext 构建路径。

include!("migration_aster_unit_test.rs");

/// 抽取告警错误消息字符串，便于断言。
fn warning_messages(warnings: &[contextutil::SQLWarn]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| warning.Err.as_ref().unwrap().to_string())
        .collect()
}

/// Go: TestSessionEvalContextBasic.
#[test]
/// Go: TestSessionEvalContextBasic — 可选属性、类型/错误上下文、告警复制与截断。
fn test_session_eval_context_basic() {
    let session = Arc::new(TestSession::new());
    let context = NewEvalContext(Arc::clone(&session));

    assert!(context.GetOptionalPropSet().IsFull());
    for index in 0..exprctx::OptPropsCnt {
        let key = exprctx::OptionalEvalPropKey(index);
        let provider = context.GetOptionalPropProvider(key).unwrap();
        assert_eq!(key, provider.Desc().Key());
    }

    assert_eq!(session.type_context.Flags(), context.TypeCtx().Flags());
    assert_eq!(
        session.error_context.LevelMap(),
        context.ErrCtx().LevelMap()
    );
    assert_eq!(
        mysql::SQLMode(mysql::ModeStrictTransTables.0 | mysql::ModeNoZeroDate.0),
        context.SQLMode()
    );
    assert_eq!(Shanghai, context.Location());
    assert_eq!(context.TypeCtx().Location(), context.Location());
    assert_eq!("db1", context.CurrentDB());
    assert_eq!(123_456, context.GetMaxAllowedPacket());
    assert_eq!("0", context.GetDefaultWeekFormatMode());
    session
        .systems
        .lock()
        .unwrap()
        .insert("default_week_format".to_owned(), "5".to_owned());
    assert_eq!("5", context.GetDefaultWeekFormatMode());
    assert!(std::ptr::eq(
        context.GetUserVarsReader(),
        &session.user_vars
    ));

    assert_eq!(0, context.WarningCount());
    context.AppendWarning(contextutil::errors::New("err1"));
    assert_eq!(1, context.WarningCount());
    context
        .TypeCtx()
        .AppendWarning(contextutil::errors::New("err2"));
    assert_eq!(2, context.WarningCount());
    context
        .ErrCtx()
        .AppendWarning(contextutil::errors::New("err3"));
    assert_eq!(3, context.WarningCount());

    let placeholder = contextutil::SQLWarn {
        Level: contextutil::WarnLevelWarning.to_owned(),
        Err: None,
    };
    for destination in [
        Vec::new(),
        vec![placeholder.clone()],
        vec![placeholder.clone(); 3],
        Vec::with_capacity(3),
    ] {
        let warnings = context.CopyWarnings(destination);
        assert_eq!(3, warnings.len());
        assert!(
            warnings
                .iter()
                .all(|warning| warning.Level == contextutil::WarnLevelWarning)
        );
        assert_eq!(vec!["err1", "err2", "err3"], warning_messages(&warnings));
    }

    let warnings = context.TruncateWarnings(1);
    assert_eq!(vec!["err2", "err3"], warning_messages(&warnings));
    let warnings = context.TruncateWarnings(0);
    assert_eq!(vec!["err1"], warning_messages(&warnings));
}

/// Go: TestSessionEvalContextCurrentTime.
#[test]
/// Go: TestSessionEvalContextCurrentTime — stale TSO / timestamp 变量 / 系统时间缓存。
fn test_session_eval_context_current_time() {
    let session = Arc::new(TestSession::new());
    let context = NewEvalContext(Arc::clone(&session));

    let stale_millis = 123_456_789_u64;
    session
        .stale_tso
        .store(stale_millis << 18, Ordering::SeqCst);
    let first = context.CurrentTime().unwrap();
    assert_eq!(stale_millis as i64, first.timestamp_millis());
    assert_eq!(first, context.CurrentTime().unwrap());

    session.stale_tso.store(0, Ordering::SeqCst);
    *session.timestamp.lock().unwrap() = "7654321.875".to_owned();
    let from_timestamp = context.CurrentTime().unwrap();
    assert_eq!(7_654_321, from_timestamp.timestamp());
    assert_eq!(875_000_000, from_timestamp.nanosecond());
    assert_eq!(from_timestamp, context.CurrentTime().unwrap());

    *session.timestamp.lock().unwrap() = "0".to_owned();
    let from_system_time = context.CurrentTime().unwrap();
    assert!((Utc::now().timestamp() - from_system_time.timestamp()).abs() <= 5);

    // Go's timestamp system variable caches the first wall-clock value in the
    // statement context, so the second call must be identical down to nanos.
    std::thread::sleep(std::time::Duration::from_millis(2));
    assert_eq!(from_system_time, context.CurrentTime().unwrap());
}

/// Go: TestSessionEvalContextPrivilegeCheck.
#[test]
/// Go: TestSessionEvalContextPrivilegeCheck — 权限开关前后的静态与动态校验。
fn test_session_eval_context_privilege_check() {
    let session = Arc::new(TestSession::new());
    let context = NewEvalContext(Arc::clone(&session));

    assert!(context.RequestVerification("test", "tbl1", "col1", mysql::PrivilegeType(1)));
    assert!(context.RequestDynamicVerification("RESTRICTED_TABLES_ADMIN", true));
    assert!(context.RequestDynamicVerification("RESTRICTED_TABLES_ADMIN", false));

    session.privilege_enabled.store(true, Ordering::SeqCst);
    assert!(context.RequestVerification("db1", "t1", "c1", mysql::PrivilegeType(1)));
    assert!(!context.RequestVerification("db2", "t2", "c2", mysql::PrivilegeType(1)));
    assert!(context.RequestDynamicVerification("BACKUP_ADMIN", true));
    assert!(!context.RequestDynamicVerification("BACKUP_ADMIN", false));
}

/// Go: TestSessionEvalContextOptProps.
#[test]
/// Go: TestSessionEvalContextOptProps — CurrentUser、SessionVars、AdvisoryLock、DDLOwner。
fn test_session_eval_context_opt_props() {
    let session = Arc::new(TestSession::new());
    let context = NewEvalContext(Arc::clone(&session));

    let user = expropt::CurrentUserPropReader
        .current_user(&context)
        .unwrap();
    let roles = expropt::CurrentUserPropReader
        .active_roles(&context)
        .unwrap();
    assert_eq!("user1", user.username);
    assert_eq!(
        ["role1", "role2"],
        [roles[0].username.as_str(), roles[1].username.as_str()]
    );

    expropt::intest::EnableAssert.store(false, Ordering::Relaxed);
    let vars = expropt::SessionVarsPropReader
        .get_session_vars(&context)
        .unwrap();
    assert!(std::ptr::eq(Arc::as_ptr(&session.vars), vars));

    let locks = expropt::AdvisoryLockPropReader
        .advisory_lock_ctx(&context)
        .unwrap();
    locks.get_advisory_lock("lock1", 10).unwrap();
    assert_eq!(1, locks.is_used_advisory_lock("lock1"));
    assert!(session.advisory_locks.lock().unwrap().contains("lock1"));

    assert!(!expropt::DDLOwnerPropReader.is_ddl_owner(&context).unwrap());
    session.ddl_owner.store(true, Ordering::SeqCst);
    assert!(expropt::DDLOwnerPropReader.is_ddl_owner(&context).unwrap());

    let checker = expropt::PrivilegeCheckerPropReader
        .get_privilege_checker(&context)
        .unwrap();
    assert!(checker.request_verification("any", "any", "any", mysql::PrivilegeType(1)));
    session.privilege_enabled.store(true, Ordering::SeqCst);
    assert!(checker.request_verification("db1", "t1", "c1", mysql::PrivilegeType(1)));
    assert!(!checker.request_verification("db2", "t2", "c2", mysql::PrivilegeType(1)));
}

/// Go: TestSessionBuildContext.
#[test]
/// Go: TestSessionBuildContext — ExprContext 与 EvalContext 指针同一性及计划缓存。
fn test_session_build_context() {
    let session = Arc::new(TestSession::new());
    let context = NewExprContext(Arc::clone(&session));
    let eval_trait: &dyn exprctx::EvalContext = &context.EvalContext;

    let trait_data = context.GetEvalCtx() as *const dyn exprctx::EvalContext as *const ();
    let concrete_data = eval_trait as *const dyn exprctx::EvalContext as *const ();
    assert_eq!(concrete_data, trait_data);
    assert!(context.EvalContext.GetOptionalPropSet().IsFull());
    assert!(std::ptr::eq(context.EvalContext.Sctx(), session.as_ref()));
    assert_eq!(
        ("gbk".to_owned(), "gbk_chinese_ci".to_owned()),
        context.GetCharsetInfo()
    );
    assert_eq!(
        "utf8mb4_0900_ai_ci",
        context.GetDefaultCollationForUTF8MB4()
    );
    assert!(context.GetSysdateIsNow());
    assert_eq!(2, context.GetNoopFuncsMode());
    assert!(std::ptr::eq(context.Rng(), session.rng.as_ref()));

    session.plan_cache.EnablePlanCache();
    assert!(context.IsUseCache());
    context.SetSkipPlanCache("mockReason");
    assert!(!context.IsUseCache());

    let previous_id = session.plan_column_id.load(Ordering::SeqCst);
    assert_eq!(previous_id + 1, context.AllocPlanColumnID());
    assert_eq!(previous_id + 2, context.AllocPlanColumnID());
    session.alloc_plan_column_id();
    assert_eq!(previous_id + 4, context.AllocPlanColumnID());
    assert!(!context.IsInNullRejectCheck());
    assert_eq!(123, context.ConnectionID());
}
