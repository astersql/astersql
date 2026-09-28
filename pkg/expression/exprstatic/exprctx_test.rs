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
// exprstatic 表达式上下文相关单元测试。
//
// 覆盖默认值、全选项构造、Apply、列 ID 分配器共享/替换、
// `MakeExprContextStatic` 以及 `LoadSystemVars` 对 Expr/Eval 两层的同步。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::Arc;

use chrono::TimeZone;
use chrono_tz::{Asia::Tokyo, UTC};
use exprstatic::*;

/// `expressionContextWithAllOptions` 返回的对照状态。
struct ExprOptionState {
    eval_ctx: Arc<EvalContext>,
    rng: Arc<mathutil::MysqlRng>,
    tracker: Arc<contextutil::plancache::PlanCacheTracker>,
}

/// 构造填满常用选项的 ExprContext 及对照状态。
fn expressionContextWithAllOptions() -> (ExprContext, ExprOptionState) {
    let eval_ctx = Arc::new(NewEvalContext(vec![WithLocation(Tokyo)]));
    let rng: Arc<mathutil::MysqlRng> = Arc::from(mathutil::NewWithSeed(12_345_678));
    let tracker = Arc::new(contextutil::plancache::NewPlanCacheTracker(Arc::new(
        contextutil::NewStaticWarnHandler(0),
    )));
    tracker.EnablePlanCache();
    let ctx = NewExprContext(vec![
        WithEvalCtx(eval_ctx.clone()),
        WithCharset("gbk".to_owned(), "gbk_bin".to_owned()),
        WithDefaultCollationForUTF8MB4("utf8mb4_0900_ai_ci".to_owned()),
        WithBlockEncryptionMode("aes-256-cbc".to_owned()),
        WithSysDateIsNow(true),
        WithNoopFuncsMode(variable::WarnInt),
        WithRng(rng.clone()),
        WithPlanCacheTracker(tracker.clone()),
        WithColumnIDAllocator(Arc::new(exprctx::NewSimplePlanColumnIDAllocator(1024))),
        WithConnectionID(778899),
        WithWindowingUseHighPrecision(false),
        WithGroupConcatMaxLen(2_233_445_566),
    ]);
    (
        ctx,
        ExprOptionState {
            eval_ctx,
            rng,
            tracker,
        },
    )
}

/// 断言默认静态表达式上下文与服务器默认值一致。
fn checkDefaultStaticExprCtx(ctx: &ExprContext) {
    assert_eq!(ctx.GetEvalCtx().Location(), UTC);
    let default_charset = charset::GetCharsetInfo(mysql::DefaultCharset).unwrap();
    assert_eq!(
        ctx.GetCharsetInfo(),
        (default_charset.Name, default_charset.DefaultCollation)
    );
    assert_eq!(
        ctx.GetDefaultCollationForUTF8MB4(),
        mysql::DefaultCollationName
    );
    assert_eq!(ctx.GetBlockEncryptionMode(), vardef::DefBlockEncryptionMode);
    assert_eq!(ctx.GetSysdateIsNow(), vardef::DefSysdateIsNow);
    assert_eq!(
        ctx.GetNoopFuncsMode(),
        variable::TiDBOptOnOffWarn(vardef::DefTiDBEnableNoopFuncs)
    );
    assert!(ctx.IsUseCache());
    assert_eq!(ctx.ConnectionID(), 0);
    assert!(ctx.GetWindowingUseHighPrecision());
    assert_eq!(ctx.GetGroupConcatMaxLen(), vardef::DefGroupConcatMaxLen);
    assert!(!ctx.IsInNullRejectCheck());
    assert!(!ctx.IsConstantPropagateCheck());
    assert!(!ctx.IsReadonlyUserVar("anything"));
}

/// 断言全选项上下文与对照状态（含共享指针）一致。
fn checkOptionsStaticExprCtx(ctx: &ExprContext, state: &ExprOptionState) {
    assert!(std::ptr::eq(ctx.GetEvalCtx(), state.eval_ctx.as_ref()));
    assert_eq!(
        ctx.GetCharsetInfo(),
        ("gbk".to_owned(), "gbk_bin".to_owned())
    );
    assert_eq!(ctx.GetDefaultCollationForUTF8MB4(), "utf8mb4_0900_ai_ci");
    assert_eq!(ctx.GetBlockEncryptionMode(), "aes-256-cbc");
    assert!(ctx.GetSysdateIsNow());
    assert_eq!(ctx.GetNoopFuncsMode(), variable::WarnInt);
    assert!(std::ptr::eq(ctx.Rng(), state.rng.as_ref()));
    assert!(std::ptr::eq(
        ctx.GetPlanCacheTracker(),
        state.tracker.as_ref()
    ));
    assert_eq!(ctx.AllocPlanColumnID(), 1025);
    assert_eq!(ctx.ConnectionID(), 778899);
    assert!(!ctx.GetWindowingUseHighPrecision());
    assert_eq!(ctx.GetGroupConcatMaxLen(), 2_233_445_566);
}

#[test]
/// 默认与全选项构造的字段校验。
fn TestNewStaticExprCtx() {
    let default = NewExprContext(Vec::new());
    checkDefaultStaticExprCtx(&default);
    assert_eq!(default.AllocPlanColumnID(), 1);
    let (configured, state) = expressionContextWithAllOptions();
    checkOptionsStaticExprCtx(&configured, &state);
}

#[test]
/// 未显式传入 tracker 时，默认 tracker 必须绑定选项提供的 EvalCtx。
fn TestDefaultPlanCacheTrackerUsesConfiguredEvalCtx() {
    let warnings = Arc::new(contextutil::NewStaticWarnHandler(0));
    let eval_ctx = Arc::new(NewEvalContext(vec![WithWarnHandler(warnings.clone())]));
    let ctx = NewExprContext(vec![WithEvalCtx(eval_ctx)]);

    ctx.GetPlanCacheTracker()
        .SetCacheType(contextutil::plancache::PlanCacheType::SessionPrepared);
    ctx.SetSkipPlanCache("configured eval context");

    assert_eq!(ctx.GetEvalCtx().WarningCount(), 1);
}

#[test]
/// Apply 覆盖选项且共享 EvalCtx/列 ID/计划缓存 tracker。
fn TestStaticExprCtxApplyOptions() {
    let original = NewExprContext(Vec::new());
    let original_eval = original.GetEvalCtx() as *const EvalContext;
    let updated = original.Apply(vec![
        WithCharset("gbk".to_owned(), "gbk_bin".to_owned()),
        WithConnectionID(778899),
        WithWindowingUseHighPrecision(false),
    ]);
    assert_eq!(original.GetCharsetInfo().0, mysql::DefaultCharset);
    assert_eq!(original.ConnectionID(), 0);
    assert!(original.GetWindowingUseHighPrecision());
    assert_eq!(
        updated.GetCharsetInfo(),
        ("gbk".to_owned(), "gbk_bin".to_owned())
    );
    assert_eq!(updated.ConnectionID(), 778899);
    assert!(!updated.GetWindowingUseHighPrecision());
    assert_eq!(updated.GetEvalCtx() as *const EvalContext, original_eval);
    assert_eq!(updated.AllocPlanColumnID(), 1);
    assert_eq!(original.AllocPlanColumnID(), 2);
    updated.SetSkipPlanCache("reason");
    assert!(!updated.IsUseCache());
    assert!(!original.IsUseCache());

    let copied = updated.Apply(Vec::new());
    assert_eq!(copied.GetCharsetInfo(), updated.GetCharsetInfo());
    assert_eq!(copied.ConnectionID(), updated.ConnectionID());
    assert!(!copied.IsUseCache());
}

#[test]
/// 列 ID 分配器在 Apply 间共享，可被选项整体替换。
fn TestExprCtxColumnIDAllocator() {
    let ctx = NewExprContext(Vec::new());
    assert_eq!(ctx.AllocPlanColumnID(), 1);
    let shared = ctx.Apply(Vec::new());
    assert_eq!(shared.AllocPlanColumnID(), 2);
    assert_eq!(ctx.AllocPlanColumnID(), 3);
    let replaced = ctx.Apply(vec![WithColumnIDAllocator(Arc::new(
        exprctx::NewSimplePlanColumnIDAllocator(1024),
    ))]);
    assert_eq!(replaced.AllocPlanColumnID(), 1025);
    assert_eq!(ctx.AllocPlanColumnID(), 4);
    let fresh = NewExprContext(vec![WithColumnIDAllocator(Arc::new(
        exprctx::NewSimplePlanColumnIDAllocator(2048),
    ))]);
    assert_eq!(fresh.AllocPlanColumnID(), 2049);
}

#[test]
/// 物化静态快照：EvalCtx 独立，RNG/tracker 可复用，列 ID 延续。
fn TestMakeExprContextStatic() {
    let eval_ctx = Arc::new(NewEvalContext(vec![
        WithLocation(Tokyo),
        WithCurrentTime(Arc::new(|| {
            Ok(UTC.timestamp_opt(1_700_000_000, 0).unwrap())
        })),
    ]));
    let rng: Arc<mathutil::MysqlRng> = Arc::from(mathutil::NewWithSeed(12_345_678));
    let tracker = Arc::new(contextutil::plancache::NewPlanCacheTracker(Arc::new(
        contextutil::NewStaticWarnHandler(0),
    )));
    tracker.EnablePlanCache();
    let obj = NewExprContext(vec![
        WithEvalCtx(eval_ctx),
        WithCharset("gbk".to_owned(), "gbk_bin".to_owned()),
        WithDefaultCollationForUTF8MB4("utf8mb4_0900_ai_ci".to_owned()),
        WithBlockEncryptionMode("aes-256-cbc".to_owned()),
        WithSysDateIsNow(true),
        WithNoopFuncsMode(variable::WarnInt),
        WithRng(rng),
        WithPlanCacheTracker(tracker),
        WithColumnIDAllocator(Arc::new(exprctx::NewSimplePlanColumnIDAllocator(10))),
        WithConnectionID(1),
        WithWindowingUseHighPrecision(false),
        WithGroupConcatMaxLen(123456),
    ]);
    assert_eq!(obj.AllocPlanColumnID(), 11);
    let static_obj = MakeExprContextStatic(&obj);
    assert!(!std::ptr::eq(obj.GetEvalCtx(), static_obj.GetEvalCtx()));
    assert_eq!(static_obj.GetCharsetInfo(), obj.GetCharsetInfo());
    assert_eq!(
        static_obj.GetDefaultCollationForUTF8MB4(),
        obj.GetDefaultCollationForUTF8MB4()
    );
    assert_eq!(
        static_obj.GetBlockEncryptionMode(),
        obj.GetBlockEncryptionMode()
    );
    assert_eq!(static_obj.GetSysdateIsNow(), obj.GetSysdateIsNow());
    assert_eq!(static_obj.GetNoopFuncsMode(), obj.GetNoopFuncsMode());
    assert_eq!(static_obj.ConnectionID(), obj.ConnectionID());
    assert_eq!(
        static_obj.GetWindowingUseHighPrecision(),
        obj.GetWindowingUseHighPrecision()
    );
    assert_eq!(
        static_obj.GetGroupConcatMaxLen(),
        obj.GetGroupConcatMaxLen()
    );
    assert_eq!(static_obj.AllocPlanColumnID(), 12);
    assert!(std::ptr::eq(obj.Rng(), static_obj.Rng()));
    assert!(std::ptr::eq(
        obj.GetPlanCacheTracker(),
        static_obj.GetPlanCacheTracker()
    ));
}

#[test]
/// LoadSystemVars 同步 Expr 层与 Eval 层；字符集/校对可单独覆盖。
fn TestExprCtxLoadSystemVars() {
    let defaults = NewExprContext(Vec::new());
    let loaded = defaults
        .LoadSystemVars(&HashMap::from([
            ("character_set_connection".to_owned(), "gbk".to_owned()),
            (
                "collation_connection".to_owned(),
                "gbk_chinese_ci".to_owned(),
            ),
            (
                "default_collation_for_utf8mb4".to_owned(),
                "utf8mb4_general_ci".to_owned(),
            ),
            ("TIDB_SYSDATE_IS_NOW".to_owned(), "1".to_owned()),
            ("tidb_enable_noop_functions".to_owned(), "warn".to_owned()),
            ("block_encryption_mode".to_owned(), "aes-256-cbc".to_owned()),
            ("group_concat_max_len".to_owned(), "123456".to_owned()),
            ("windowing_use_high_precision".to_owned(), "0".to_owned()),
        ]))
        .unwrap();
    assert_eq!(
        loaded.GetCharsetInfo(),
        ("gbk".to_owned(), "gbk_chinese_ci".to_owned())
    );
    assert_eq!(loaded.GetDefaultCollationForUTF8MB4(), "utf8mb4_general_ci");
    assert!(loaded.GetSysdateIsNow());
    assert_eq!(loaded.GetNoopFuncsMode(), variable::WarnInt);
    assert_eq!(loaded.GetBlockEncryptionMode(), "aes-256-cbc");
    assert_eq!(loaded.GetGroupConcatMaxLen(), 123456);
    assert!(!loaded.GetWindowingUseHighPrecision());
    assert!(std::ptr::eq(defaults.Rng(), loaded.Rng()));
    assert!(std::ptr::eq(
        defaults.GetPlanCacheTracker(),
        loaded.GetPlanCacheTracker()
    ));

    let charset_only = defaults
        .LoadSystemVars(&HashMap::from([(
            "character_set_connection".to_owned(),
            "ascii".to_owned(),
        )]))
        .unwrap();
    assert_eq!(
        charset_only.GetCharsetInfo(),
        ("ascii".to_owned(), "ascii_bin".to_owned())
    );
    let collation_only = defaults
        .LoadSystemVars(&HashMap::from([(
            "collation_connection".to_owned(),
            "latin1_bin".to_owned(),
        )]))
        .unwrap();
    assert_eq!(
        collation_only.GetCharsetInfo(),
        ("latin1".to_owned(), "latin1_bin".to_owned())
    );
    let eval_loaded = defaults
        .LoadSystemVars(&HashMap::from([
            ("div_precision_increment".to_owned(), "9".to_owned()),
            ("time_zone".to_owned(), "Asia/Tokyo".to_owned()),
        ]))
        .unwrap();
    assert_eq!(eval_loaded.GetEvalCtx().GetDivPrecisionIncrement(), 9);
    assert_eq!(eval_loaded.GetEvalCtx().Location(), Tokyo);
}

#[test]
/// Expr 层数值/枚举变量遵循 Go SysVar 的裁剪、大小写和枚举索引规则。
fn TestExprSystemVarNormalizationMatchesGo() {
    let loaded = NewExprContext(Vec::new())
        .LoadSystemVars(&HashMap::from([
            ("group_concat_max_len".to_owned(), "1".to_owned()),
            ("block_encryption_mode".to_owned(), "AES-128-CBC".to_owned()),
            ("tidb_enable_noop_functions".to_owned(), "2".to_owned()),
        ]))
        .unwrap();

    assert_eq!(loaded.GetGroupConcatMaxLen(), 4);
    assert_eq!(loaded.GetBlockEncryptionMode(), "aes-128-cbc");
    assert_eq!(loaded.GetNoopFuncsMode(), variable::WarnInt);
    assert!(
        NewExprContext(Vec::new())
            .LoadSystemVars(&HashMap::from([(
                "block_encryption_mode".to_owned(),
                "aes-512-ecb".to_owned(),
            )]))
            .is_err()
    );
}
