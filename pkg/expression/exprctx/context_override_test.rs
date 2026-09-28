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

// Rust counterpart of `pkg/expression/exprctx/context_override_test.go`.
//
// Go uses `package exprctx_test` with `exprstatic.NewEvalContext` /
// `NewExprContext`. This unit test builds the same `BuildContext` surface
// locally (matching `migration_aster_unit_test`) and drives the real
// `CtxWithHandleTruncateErrLevel` production API.
//
// 截断错误级别覆盖（CtxWithHandleTruncateErrLevel）的单元测试：
// 本地搭建与 Go exprstatic 等价的 BuildContext，验证类型标志与错误级别映射，
// 以及级别未变化时复用原上下文指针。

#![allow(non_snake_case)]

use std::{ptr, str::FromStr, sync::Arc};

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

/// 测试用 EvalContext：固定当前时间、库名与包大小等会话侧字段。
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
        1
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
    fn IsReadonlyUserVar(&self, _name: &str) -> bool {
        false
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

/// 按类型标志、截断错误级别与时区构造测试上下文。
fn new_expr_context(
    flags: types::Flags,
    truncate_level: errctx::Level,
    loc: Tz,
) -> TestExprContext {
    let type_warnings: Arc<dyn WarnAppender + Send + Sync> = Arc::new(contextutil::ignoreWarn {});
    let type_ctx = types_crate::scalar::NewContext(flags, loc, type_warnings);
    // 默认各组为 Error，再单独覆盖除零与截断级别。
    let mut levels = [errctx::Level::LevelError; errctx::errGroupCount];
    levels[errctx::ErrGroup::ErrGroupDividedByZero as usize] = errctx::Level::LevelError;
    levels[errctx::ErrGroup::ErrGroupTruncate as usize] = truncate_level;
    let err_warnings: errctx_crate::errctx::WarnAppenderRef =
        Arc::new(errctx_crate::warn::ignoreWarn {});
    let err_ctx = errctx_crate::errctx::NewContextWithLevels(levels, err_warnings);
    let now = loc.from_utc_datetime(
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

/// Corresponds to Go `TestCtxWithHandleTruncateErrLevel`.
/// 验证 Warn/Ignore/Error 三种截断级别覆盖后的 Flags 与 LevelMap，
/// 且原上下文不变；级别不变时返回同一指针。
#[test]
fn TestCtxWithHandleTruncateErrLevel() {
    for level in [
        errctx::Level::LevelWarn,
        errctx::Level::LevelIgnore,
        errctx::Level::LevelError,
    ] {
        let mut original_level_map = [errctx::Level::LevelError; errctx::errGroupCount];
        original_level_map[errctx::ErrGroup::ErrGroupTruncate as usize] = level;
        let expected_level_map = original_level_map;

        // LevelError 场景：原 flags 先以 Warn 形式存在，覆盖后应关掉 TruncateAsWarning。
        let mut original_flags = types::DefaultStmtFlags;
        let expected_flags = match level {
            errctx::Level::LevelError => {
                original_flags = original_flags.WithTruncateAsWarning(true);
                original_flags.WithTruncateAsWarning(false)
            }
            errctx::Level::LevelWarn => original_flags.WithTruncateAsWarning(true),
            errctx::Level::LevelIgnore => original_flags.WithIgnoreTruncateErr(true),
        };

        // Go uses FixedZone("tz1", 3600*2); Etc/GMT-2 is UTC+2 (POSIX sign flip).
        // Go 固定时区偏移 +2h；POSIX Etc/GMT-2 符号相反，等价 UTC+2。
        let original_loc = Tz::from_str("Etc/GMT-2").expect("fixed UTC+2 timezone must exist");
        let ctx = new_expr_context(original_flags, level, original_loc);

        let original_type_ctx = ctx.GetEvalCtx().TypeCtx();
        let original_err_ctx = ctx.GetEvalCtx().ErrCtx();

        // Override should take effect.
        // 覆盖应生效：Flags 与截断组级别变为期望值。
        let new_ctx = CtxWithHandleTruncateErrLevel(&ctx, level);
        assert!(new_ctx.WasOverridden());
        let new_eval_ctx = new_ctx.GetEvalCtx();
        let new_type_ctx = new_eval_ctx.TypeCtx();
        let new_err_ctx = new_eval_ctx.ErrCtx();
        assert_eq!(new_type_ctx.Flags(), expected_flags);
        assert_eq!(new_err_ctx.LevelMap(), expected_level_map);

        // Other fields should not change.
        // 时区与连接 ID 等无关字段保持不变。
        assert_eq!(new_type_ctx.Location(), original_loc);
        assert_eq!(new_eval_ctx.Location(), original_loc);
        assert_eq!(new_ctx.ConnectionID(), 1234);

        // Old ctx should not change.
        // 原上下文未被原地修改。
        assert_eq!(
            ctx.GetEvalCtx().TypeCtx().Flags(),
            original_type_ctx.Flags()
        );
        assert_eq!(
            ctx.GetEvalCtx().TypeCtx().Location(),
            original_type_ctx.Location()
        );
        assert_eq!(ctx.GetEvalCtx().ErrCtx().LevelMap(), original_level_map);
        assert_eq!(
            ctx.GetEvalCtx().ErrCtx().LevelMap(),
            original_err_ctx.LevelMap()
        );
        assert_eq!(ctx.GetEvalCtx().Location(), original_loc);
        assert_eq!(ctx.ConnectionID(), 1234);

        // Unchanged truncation level reuses the context (Go `require.Same`).
        // 级别未变时复用原上下文（Go require.Same）。
        let new_ctx2 = CtxWithHandleTruncateErrLevel(&new_ctx, level);
        match new_ctx2 {
            CtxWithTruncateResult::Original(reused) => {
                assert!(ptr::eq(reused, &new_ctx as &dyn BuildContext));
            }
            CtxWithTruncateResult::Overridden(_) => {
                panic!("unchanged truncation level must reuse the context")
            }
        }
    }
}
