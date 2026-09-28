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

// exprstatic 迁移场景的 Aster 单元测试。
//
// 对照 Go：静态 `EvalContext`/`ExprContext` 的默认值、选项覆盖、
// 当前时间缓存、Apply 语义、可选属性替换，以及 `LoadSystemVars` 大小写不敏感加载。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{TimeZone, Timelike};
use chrono_tz::{Asia::Tokyo, UTC};
use exprstatic::*;

#[test]
/// 选项写入后各字段与 Go 静态求值上下文行为一致，含告警截断。
fn eval_context_defaults_and_options_match_go() {
    let mut first = types::Datum::default();
    first.SetInt64(41);
    let warning_handler = Arc::new(contextutil::NewStaticWarnHandler(0));
    let ctx = NewEvalContext(vec![
        WithWarnHandler(warning_handler.clone()),
        WithSQLMode(mysql::ModeANSIQuotes),
        WithTypeFlags(types::Flags(7)),
        WithLocation(Tokyo),
        WithCurrentDB("test_db".to_owned()),
        WithMaxAllowedPacket(4096),
        WithDefaultWeekFormatMode("5".to_owned()),
        WithDivPrecisionIncrement(12),
        WithParamList(vec![first]),
        WithEnableRedactLog("ON".to_owned()),
    ]);

    assert!(ctx.CtxID() > 0);
    assert_eq!(ctx.SQLMode(), mysql::ModeANSIQuotes);
    assert_eq!(ctx.TypeCtx().Flags(), types::Flags(7));
    assert_eq!(ctx.Location(), Tokyo);
    assert_eq!(ctx.CurrentDB(), "test_db");
    assert_eq!(ctx.GetMaxAllowedPacket(), 4096);
    assert_eq!(ctx.GetDefaultWeekFormatMode(), "5");
    assert_eq!(ctx.GetDivPrecisionIncrement(), 12);
    assert_eq!(ctx.GetParamValue(0).unwrap().GetInt64(), 41);
    assert!(ctx.GetParamValue(1).is_err());
    assert_eq!(ctx.GetTiDBRedactLog(), "ON");

    ctx.AppendWarning(contextutil::errors::NewNoStackError("warning"));
    ctx.AppendNote(contextutil::errors::NewNoStackError("note"));
    assert_eq!(ctx.WarningCount(), 2);
    assert_eq!(ctx.TruncateWarnings(1).len(), 1);
    assert_eq!(ctx.WarningCount(), 1);
}

#[test]
/// 当前时间：失败不缓存，成功后缓存；同一语句内多次调用返回相同时间。
fn current_time_is_cached_after_success_and_retried_after_error() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_fn = calls.clone();
    let ctx = NewEvalContext(vec![
        WithLocation(Tokyo),
        WithCurrentTime(Arc::new(move || {
            let call = calls_for_fn.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                return Err(contextutil::errors::NewNoStackError("first call fails"));
            }
            Ok(UTC.timestamp_opt(1_700_000_000, 123_456_000).unwrap())
        })),
    ]);

    assert!(ctx.CurrentTime().is_err());
    let first = ctx.CurrentTime().unwrap();
    let second = ctx.CurrentTime().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(first, second);
    assert_eq!(first.timezone(), Tokyo);
    assert_eq!(first.nanosecond(), 123_456_000);
}

#[test]
/// Apply 分配新 CtxID，但继承原语句时间；其它选项可独立覆盖。
fn eval_apply_gets_new_id_but_preserves_old_statement_time() {
    let ctx = NewEvalContext(vec![WithCurrentTime(Arc::new(|| {
        Ok(UTC.timestamp_opt(1_234_567_890, 0).unwrap())
    }))]);
    let original_time = ctx.CurrentTime().unwrap();
    let applied = ctx.Apply(vec![WithCurrentDB("next".to_owned())]);

    assert!(applied.CtxID() > ctx.CtxID());
    assert_eq!(applied.CurrentTime().unwrap(), original_time);
    assert_eq!(applied.CurrentDB(), "next");
    assert_eq!(ctx.CurrentDB(), "");
}

/// 测试用可选属性 Provider，仅声明 `OptPropCurrentUser`。
struct TestProvider;

impl exprctx::OptionalEvalPropProvider for TestProvider {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        &exprctx::OPTIONAL_PROPERTY_DESC_LIST[exprctx::OptPropCurrentUser.0]
    }
}

#[test]
/// 可选属性可按 key 注册；Apply 空列表可整体替换掉。
fn optional_properties_are_replaced_and_reported_by_key() {
    let provider: Box<dyn exprctx::OptionalEvalPropProvider> = Box::new(TestProvider);
    let ctx = NewEvalContext(vec![WithOptionalProperty(vec![provider])]);
    assert!(
        ctx.GetOptionalPropSet()
            .Contains(exprctx::OptPropCurrentUser)
    );
    assert!(
        ctx.GetOptionalPropProvider(exprctx::OptPropCurrentUser)
            .is_some()
    );

    let replaced = ctx.Apply(vec![WithOptionalProperty(Vec::new())]);
    assert!(replaced.GetOptionalPropSet().IsEmpty());
    assert!(
        ctx.GetOptionalPropSet()
            .Contains(exprctx::OptPropCurrentUser)
    );
}

#[test]
/// ExprContext 默认值、Apply、以及共享列 ID/计划缓存状态与 Go 一致。
fn expr_context_defaults_apply_and_shared_state_match_go() {
    let ctx = NewExprContext(Vec::new());
    assert_eq!(ctx.GetCharsetInfo().0, mysql::DefaultCharset);
    assert_eq!(
        ctx.GetDefaultCollationForUTF8MB4(),
        mysql::DefaultCollationName
    );
    assert_eq!(ctx.GetBlockEncryptionMode(), vardef::DefBlockEncryptionMode);
    assert!(!ctx.GetSysdateIsNow());
    assert_eq!(ctx.GetNoopFuncsMode(), variable::OffInt);
    assert!(ctx.IsUseCache());
    assert_eq!(ctx.AllocPlanColumnID(), 1);

    let applied = ctx.Apply(vec![
        WithCharset("gbk".to_owned(), "gbk_bin".to_owned()),
        WithConnectionID(778899),
        WithWindowingUseHighPrecision(false),
        WithGroupConcatMaxLen(2233445566),
    ]);
    assert_eq!(
        applied.GetCharsetInfo(),
        ("gbk".to_owned(), "gbk_bin".to_owned())
    );
    assert_eq!(applied.ConnectionID(), 778899);
    assert!(!applied.GetWindowingUseHighPrecision());
    assert_eq!(applied.GetGroupConcatMaxLen(), 2233445566);
    assert_eq!(applied.AllocPlanColumnID(), 2);
    assert_eq!(ctx.AllocPlanColumnID(), 3);

    // 跳过计划缓存会影响共享 tracker，原上下文一并失效。
    applied.SetSkipPlanCache("reason");
    assert!(!ctx.IsUseCache());
}

#[test]
/// LoadSystemVars 同步更新 Expr/Eval 两层，变量名大小写不敏感；未知变量报错。
fn load_system_vars_updates_both_context_layers_case_insensitively() {
    let ctx = NewExprContext(Vec::new());
    let vars = HashMap::from([
        ("CHARACTER_SET_CONNECTION".to_owned(), "gbk".to_owned()),
        (
            "collation_connection".to_owned(),
            "gbk_chinese_ci".to_owned(),
        ),
        (
            "default_collation_for_utf8mb4".to_owned(),
            "utf8mb4_general_ci".to_owned(),
        ),
        ("block_encryption_mode".to_owned(), "aes-256-cbc".to_owned()),
        ("tidb_sysdate_is_now".to_owned(), "1".to_owned()),
        ("tidb_enable_noop_functions".to_owned(), "WARN".to_owned()),
        ("windowing_use_high_precision".to_owned(), "0".to_owned()),
        ("group_concat_max_len".to_owned(), "123456".to_owned()),
        ("time_zone".to_owned(), "Asia/Tokyo".to_owned()),
        ("sql_mode".to_owned(), "ANSI_QUOTES".to_owned()),
        ("timestamp".to_owned(), "1234567890.123456".to_owned()),
        ("max_allowed_packet".to_owned(), "524288".to_owned()),
        ("tidb_redact_log".to_owned(), "ON".to_owned()),
        ("default_week_format".to_owned(), "5".to_owned()),
        ("div_precision_increment".to_owned(), "12".to_owned()),
    ]);
    let loaded = ctx.LoadSystemVars(&vars).unwrap();

    assert_eq!(
        loaded.GetCharsetInfo(),
        ("gbk".to_owned(), "gbk_chinese_ci".to_owned())
    );
    assert_eq!(loaded.GetDefaultCollationForUTF8MB4(), "utf8mb4_general_ci");
    assert_eq!(loaded.GetBlockEncryptionMode(), "aes-256-cbc");
    assert!(loaded.GetSysdateIsNow());
    assert_eq!(loaded.GetNoopFuncsMode(), variable::WarnInt);
    assert!(!loaded.GetWindowingUseHighPrecision());
    assert_eq!(loaded.GetGroupConcatMaxLen(), 123456);
    assert_eq!(loaded.GetEvalCtx().Location(), Tokyo);
    assert_eq!(loaded.GetEvalCtx().SQLMode(), mysql::ModeANSIQuotes);
    assert_eq!(
        loaded
            .GetEvalCtx()
            .CurrentTime()
            .unwrap()
            .timestamp_micros(),
        1_234_567_890_123_456
    );
    assert_eq!(loaded.GetEvalCtx().GetMaxAllowedPacket(), 524288);
    assert_eq!(loaded.GetEvalCtx().GetTiDBRedactLog(), "ON");
    assert_eq!(loaded.GetEvalCtx().GetDefaultWeekFormatMode(), "5");
    assert_eq!(loaded.GetEvalCtx().GetDivPrecisionIncrement(), 12);

    assert!(
        ctx.LoadSystemVars(&HashMap::from([("unknown".to_owned(), "1".to_owned())]))
            .is_err()
    );
}
