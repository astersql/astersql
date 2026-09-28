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

// exprctx 迁移对照单元测试：分配器、上下文包装、截断覆盖、可选属性与参数哨兵。
//
// 本地搭建最小 EvalContext / BuildContext，验证与 Go 委托语义一致的行为，
// 包括并发列 ID 分配与时区断言。

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use contextutil::{SQLWarn, WarnAppender, WarnHandler};

use crate::*;

/// 空用户变量表：任意名字均返回 None。
#[derive(Default)]
struct EmptyUserVars;

impl UserVarsReader for EmptyUserVars {
    fn GetUserVarVal(&self, _name: &str) -> Option<types::Datum> {
        None
    }

    fn GetUserVarType(&self, _name: &str) -> Option<types::FieldType> {
        None
    }

    fn Clone(&self) -> Box<dyn UserVarsReader> {
        Box::new(Self)
    }
}

/// 测试用 EvalContext：固定当前时间与会话侧默认值。
struct TestEvalContext {
    type_ctx: types::Context,
    err_ctx: errctx::Context,
    warnings: contextutil::StaticWarnHandler,
    user_vars: EmptyUserVars,
    now: chrono::DateTime<Tz>,
}

impl WarnAppender for TestEvalContext {
    fn AppendWarning(&self, error: contextutil::errors::SharedError) {
        self.warnings.AppendWarning(error);
    }

    fn AppendNote(&self, error: contextutil::errors::SharedError) {
        self.warnings.AppendNote(error);
    }
}

impl WarnHandler for TestEvalContext {
    fn WarningCount(&self) -> usize {
        self.warnings.WarningCount()
    }

    fn TruncateWarnings(&self, start: isize) -> Vec<SQLWarn> {
        self.warnings.TruncateWarnings(start)
    }

    fn CopyWarnings(&self, destination: Vec<SQLWarn>) -> Vec<SQLWarn> {
        self.warnings.CopyWarnings(destination)
    }
}

impl ParamValues for TestEvalContext {
    fn GetParamValue(&self, _index: usize) -> Result<types::Datum, ParamError> {
        Err(ParamError::IndexExceedsParamCount)
    }
}

impl EvalContext for TestEvalContext {
    fn CtxID(&self) -> u64 {
        88
    }
    fn SQLMode(&self) -> mysql::SQLMode {
        mysql::SQLMode::default()
    }
    fn TypeCtx(&self) -> types::Context {
        self.type_ctx.clone()
    }
    fn ErrCtx(&self) -> errctx::Context {
        self.err_ctx.clone()
    }
    fn Location(&self) -> Tz {
        self.type_ctx.Location()
    }
    fn CurrentTime(&self) -> Result<chrono::DateTime<Tz>, contextutil::errors::SharedError> {
        Ok(self.now)
    }
    fn CurrentDB(&self) -> String {
        "test".to_owned()
    }
    fn GetMaxAllowedPacket(&self) -> u64 {
        64 << 20
    }
    fn GetTiDBRedactLog(&self) -> String {
        "OFF".to_owned()
    }
    fn GetDefaultWeekFormatMode(&self) -> String {
        "0".to_owned()
    }
    fn GetDivPrecisionIncrement(&self) -> i32 {
        4
    }
    fn GetUserVarsReader(&self) -> &dyn UserVarsReader {
        &self.user_vars
    }
    fn GetOptionalPropSet(&self) -> OptionalEvalPropKeySet {
        OptionalEvalPropKeySet::default()
    }
    fn GetOptionalPropProvider(
        &self,
        _key: OptionalEvalPropKey,
    ) -> Option<&dyn OptionalEvalPropProvider> {
        None
    }
}

/// 测试用 ExprContext / BuildContext：包装 EvalContext 与确定性 RNG。
struct TestExprContext {
    eval: TestEvalContext,
    rng: Box<mathutil::MysqlRng>,
}

impl BuildContext for TestExprContext {
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        &self.eval
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        ("utf8mb4".to_owned(), "utf8mb4_bin".to_owned())
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        "utf8mb4_bin".to_owned()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        "aes-128-ecb".to_owned()
    }
    fn GetSysdateIsNow(&self) -> bool {
        false
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        0
    }
    fn Rng(&self) -> &mathutil::MysqlRng {
        &self.rng
    }
    fn IsUseCache(&self) -> bool {
        true
    }
    fn SetSkipPlanCache(&self, _reason: &str) {}
    fn AllocPlanColumnID(&self) -> i64 {
        42
    }
    fn IsInNullRejectCheck(&self) -> bool {
        false
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        false
    }
    fn ConnectionID(&self) -> u64 {
        1234
    }
    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        name == "readonly"
    }
}

impl ExprContext for TestExprContext {
    fn GetWindowingUseHighPrecision(&self) -> bool {
        true
    }
    fn GetGroupConcatMaxLen(&self) -> u64 {
        1024
    }
}

/// 按类型标志与截断错误级别构造上海时区测试上下文。
fn new_test_context(flags: types::Flags, truncate_level: errctx::Level) -> TestExprContext {
    let type_warnings: Arc<dyn WarnAppender + Send + Sync> = Arc::new(contextutil::ignoreWarn {});
    let type_ctx = types_crate::scalar::NewContext(flags, chrono_tz::Asia::Shanghai, type_warnings);
    let mut levels = [errctx::Level::LevelError; errctx::errGroupCount];
    levels[errctx::ErrGroup::ErrGroupDividedByZero as usize] = errctx::Level::LevelError;
    levels[errctx::ErrGroup::ErrGroupTruncate as usize] = truncate_level;
    let err_warnings: errctx_crate::errctx::WarnAppenderRef =
        Arc::new(errctx_crate::warn::ignoreWarn {});
    let err_ctx = errctx_crate::errctx::NewContextWithLevels(levels, err_warnings);
    let now = chrono_tz::Asia::Shanghai.from_utc_datetime(
        &Utc.with_ymd_and_hms(2026, 7, 15, 1, 2, 3)
            .unwrap()
            .naive_utc(),
    );
    TestExprContext {
        eval: TestEvalContext {
            type_ctx,
            err_ctx,
            warnings: contextutil::NewStaticWarnHandler(0),
            user_vars: EmptyUserVars,
            now,
        },
        rng: mathutil::NewWithSeed(1),
    }
}

/// 列 ID 分配器递增，以及空拒绝/常量传播包装器只翻转对应标志。
#[test]
fn allocator_and_context_wrappers_match_go_delegation() {
    let allocator = NewSimplePlanColumnIDAllocator(40);
    assert_eq!(allocator.GetLastPlanColumnID(), 40);
    assert_eq!(allocator.AllocPlanColumnID(), 41);
    assert_eq!(allocator.AllocPlanColumnID(), 42);

    let context = new_test_context(types::DefaultStmtFlags, errctx::Level::LevelError);
    let null_reject = WithNullRejectCheck(&context);
    assert!(null_reject.IsInNullRejectCheck());
    assert!(!null_reject.IsConstantPropagateCheck());
    assert_eq!(null_reject.ConnectionID(), 1234);
    assert_eq!(null_reject.GetGroupConcatMaxLen(), 1024);

    let constant = WithConstantPropagateCheck(&context);
    assert!(constant.IsConstantPropagateCheck());
    assert!(!constant.IsInNullRejectCheck());
    assert_eq!(constant.ConnectionID(), 1234);
}

/// 多线程并发 AllocPlanColumnID 应得到无重复的 1..=N 全集。
#[test]
fn allocator_is_atomic_under_concurrent_go_style_use() {
    let allocator = Arc::new(NewSimplePlanColumnIDAllocator(0));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let allocator = Arc::clone(&allocator);
        workers.push(std::thread::spawn(move || {
            (0..500)
                .map(|_| allocator.AllocPlanColumnID())
                .collect::<Vec<_>>()
        }));
    }

    let mut allocated = workers
        .into_iter()
        .flat_map(|worker| worker.join().expect("allocator worker must finish"))
        .collect::<Vec<_>>();
    allocated.sort_unstable();
    assert_eq!(allocated, (1..=4000).collect::<Vec<_>>());
    assert_eq!(allocator.GetLastPlanColumnID(), 4000);
}

/// 截断级别覆盖后 Flags/Level 正确，原上下文不变；同级别不再包装。
#[test]
fn truncate_override_matches_go_and_reuses_unchanged_context() {
    for (level, expected_warning, expected_ignore) in [
        (errctx::Level::LevelWarn, true, false),
        (errctx::Level::LevelIgnore, false, true),
        (errctx::Level::LevelError, false, false),
    ] {
        let original_flags = if level == errctx::Level::LevelError {
            types::DefaultStmtFlags.WithTruncateAsWarning(true)
        } else {
            types::DefaultStmtFlags
        };
        let original_level = if level == errctx::Level::LevelError {
            errctx::Level::LevelWarn
        } else {
            errctx::Level::LevelError
        };
        let context = new_test_context(original_flags, original_level);
        let original_type_ctx = context.GetEvalCtx().TypeCtx();
        let original_err_ctx = context.GetEvalCtx().ErrCtx();

        let overridden = CtxWithHandleTruncateErrLevel(&context, level);
        assert!(overridden.WasOverridden());
        let eval = overridden.GetEvalCtx();
        assert_eq!(eval.TypeCtx().Flags().TruncateAsWarning(), expected_warning);
        assert_eq!(eval.TypeCtx().Flags().IgnoreTruncateErr(), expected_ignore);
        assert_eq!(
            eval.ErrCtx()
                .LevelForGroup(errctx::ErrGroup::ErrGroupTruncate),
            level
        );
        assert_eq!(eval.Location(), chrono_tz::Asia::Shanghai);
        assert_eq!(overridden.ConnectionID(), 1234);

        assert_eq!(
            context.GetEvalCtx().TypeCtx().Flags(),
            original_type_ctx.Flags()
        );
        assert_eq!(
            context
                .GetEvalCtx()
                .ErrCtx()
                .LevelForGroup(errctx::ErrGroup::ErrGroupTruncate),
            original_err_ctx.LevelForGroup(errctx::ErrGroup::ErrGroupTruncate),
        );

        let unchanged = CtxWithHandleTruncateErrLevel(&overridden, level);
        assert!(!unchanged.WasOverridden());
    }
}

/// 可选属性键位图与非法键/未使用高位行为与 Go 一致。
#[test]
fn optional_property_keys_and_unused_bits_match_go() {
    validateOptionalProperties();
    let empty = OptionalEvalPropKeySet::default();
    assert!(empty.IsEmpty());
    assert!(!empty.IsFull());

    let current = empty.Add(OptPropCurrentUser);
    assert!(current.Contains(OptPropCurrentUser));
    assert!(empty.IsEmpty());

    let mut full = current.Add(OptPropDDLOwnerInfo);
    for key in [
        OptPropSessionVars,
        OptPropInfoSchema,
        OptPropKVStore,
        OptPropSQLExecutor,
        OptPropSequenceOperator,
        OptPropAdvisoryLock,
        OptPropPrivilegeChecker,
    ] {
        full = full.Add(key);
    }
    assert!(full.IsFull());
    assert!(!full.IsEmpty());

    for index in 0..OPT_PROPS_CNT {
        let key = OptionalEvalPropKey(index);
        let singleton = key.AsPropKeySet();
        assert!(singleton.Contains(key));
        assert_eq!(key.Desc().Key(), key);
        assert_eq!(
            key.Desc() as *const _,
            &OPTIONAL_PROPERTY_DESC_LIST[index] as *const _
        );
        assert!(singleton.Remove(key).IsEmpty());
    }

    let unused = OptionalEvalPropKeySet(u64::MAX << OPT_PROPS_CNT);
    assert!(unused.IsEmpty());
    assert!(!unused.Contains(OptPropCurrentUser));
    let invalid = OptionalEvalPropKey(OPT_PROPS_CNT);
    assert_eq!(
        invalid.to_string(),
        format!("UnknownOptionalEvalPropKey({OPT_PROPS_CNT})")
    );
    assert_eq!(unused.Add(invalid), unused);
    assert_eq!(unused.Remove(invalid), unused);
    assert!(!unused.Contains(invalid));
}

/// EmptyParamValues 对任意下标返回 Go 哨兵错误。
#[test]
fn empty_param_values_returns_the_go_sentinel_error() {
    let error = match EMPTY_PARAM_VALUES.GetParamValue(0) {
        Err(error) => error,
        Ok(_) => panic!("empty parameter values must reject every index"),
    };
    assert_eq!(error, ParamError::IndexExceedsParamCount);
    assert_eq!(error.to_string(), ERR_PARAM_INDEX_EXCEED_PARAM_COUNTS);
}

/// 会话与语句时区名称成对提供，供 AssertLocationWithSessionVars 使用。
struct TestSessionLocations {
    session: &'static str,
    statement: &'static str,
}

impl SessionVarsLocation for TestSessionLocations {
    fn SessionLocationName(&self) -> String {
        self.session.to_owned()
    }
    fn StatementLocationName(&self) -> String {
        self.statement.to_owned()
    }
}

/// 会话/语句时区一致时通过；不一致时 panic。
#[test]
fn location_assertion_checks_both_session_and_statement_locations() {
    let matching = TestSessionLocations {
        session: "Asia/Shanghai",
        statement: "Asia/Shanghai",
    };
    AssertLocationWithSessionVars(&chrono_tz::Asia::Shanghai, &matching);

    let mismatched = TestSessionLocations {
        session: "Asia/Shanghai",
        statement: "UTC",
    };
    assert!(
        std::panic::catch_unwind(|| {
            AssertLocationWithSessionVars(&chrono_tz::Asia::Shanghai, &mismatched);
        })
        .is_err()
    );
}
