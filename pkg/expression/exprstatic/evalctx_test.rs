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

// 静态求值上下文（`EvalContext`）单元测试。
//
// 对照 Go：覆盖默认值、选项注入、语句时间（CurrentTime）缓存与重试、
// 告警共享、可选属性替换、`Apply`/`MakeEvalContextStatic`/`LoadSystemVars` 行为。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use chrono_tz::{America::New_York, Australia::Sydney, Europe::Berlin, UTC};
use exprstatic::contextutil::WarnAppender;
use exprstatic::*;

/// 按位或合并两个 SQL Mode 标志（如严格模式、禁止零日期等）。
fn sql_modes(left: mysql::SQLMode, right: mysql::SQLMode) -> mysql::SQLMode {
    mysql::SQLMode(left.0 | right.0)
}

/// 构造整型 Datum，供预处理参数与用户变量断言使用。
fn int_datum(value: i64) -> types::Datum {
    let mut datum = types::Datum::default();
    datum.SetInt64(value);
    datum
}

/// 测试用用户变量读取器：仅识别名字 `"a"`。
#[derive(Clone)]
struct TestUserVars {
    value: i64,
}

impl exprctx::UserVarsReader for TestUserVars {
    fn GetUserVarVal(&self, name: &str) -> Option<types::Datum> {
        (name == "a").then(|| int_datum(self.value))
    }

    fn GetUserVarType(&self, _name: &str) -> Option<types::FieldType> {
        None
    }

    fn Clone(&self) -> Box<dyn exprctx::UserVarsReader> {
        Box::new(Clone::clone(self))
    }
}

/// 仅携带 key 的占位可选属性 Provider，用于注册/替换断言。
struct KeyProvider(exprctx::OptionalEvalPropKey);

impl exprctx::OptionalEvalPropProvider for KeyProvider {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        &exprctx::OPTIONAL_PROPERTY_DESC_LIST[self.0.0]
    }
}

/// 包装 `KeyProvider` 为 trait 对象。
fn provider(key: exprctx::OptionalEvalPropKey) -> Box<dyn exprctx::OptionalEvalPropProvider> {
    Box::new(KeyProvider(key))
}

/// 断言未配置选项时的默认静态求值上下文字段。
fn checkDefaultStaticEvalCtx(ctx: &EvalContext) {
    let default_mode = mysql::GetSQLMode(&mysql::FormatSQLModeStr(mysql::DefaultSQLMode)).unwrap();
    assert_eq!(ctx.SQLMode(), default_mode);
    assert_eq!(ctx.Location(), UTC);
    assert_eq!(ctx.TypeCtx().Flags(), types::StrictFlags);
    assert_eq!(
        ctx.ErrCtx().LevelMap(),
        [errctx::Level::LevelError; errctx::errGroupCount]
    );
    assert_eq!(ctx.CurrentDB(), "");
    assert_eq!(ctx.GetMaxAllowedPacket(), vardef::DefMaxAllowedPacket);
    assert_eq!(ctx.GetDefaultWeekFormatMode(), vardef::DefDefaultWeekFormat);
    assert_eq!(
        ctx.GetDivPrecisionIncrement(),
        vardef::DefDivPrecisionIncrement as i32
    );
    assert!(ctx.AllParamValues().is_empty());
    assert!(ctx.GetUserVarsReader().GetUserVarVal("missing").is_none());
    assert!(ctx.GetOptionalPropSet().IsEmpty());
    assert!(
        ctx.GetOptionalPropProvider(exprctx::OptPropAdvisoryLock)
            .is_none()
    );
    let now = ctx.CurrentTime().unwrap();
    assert_eq!(now.timezone(), UTC);
    assert!((Utc::now().timestamp() - now.timestamp()).abs() <= 5);
    assert_eq!(ctx.WarningCount(), 0);
}

/// 全选项上下文构造时保留的共享句柄，供后续指针/时间断言。
struct OptionState {
    now_micros: i64,
    warning_handler: Arc<contextutil::StaticWarnHandler>,
    ddl_owner: Arc<AtomicBool>,
}

/// 注入全部常用选项，返回上下文与可共享状态。
fn contextWithAllOptions() -> (EvalContext, OptionState) {
    let now_micros = 1_709_000_123_456_789;
    let warning_handler = Arc::new(contextutil::NewStaticWarnHandler(8));
    let ddl_owner = Arc::new(AtomicBool::new(false));
    let mut levels = [errctx::Level::LevelError; errctx::errGroupCount];
    levels[errctx::ErrGroup::ErrGroupDividedByZero as usize] = errctx::Level::LevelWarn;
    let ctx = NewEvalContext(vec![
        WithWarnHandler(warning_handler.clone()),
        WithSQLMode(sql_modes(
            mysql::ModeNoZeroDate,
            mysql::ModeStrictTransTables,
        )),
        WithTypeFlags(types::Flags(7)),
        WithErrLevelMap(levels),
        WithLocation(New_York),
        WithCurrentDB("db1".to_owned()),
        WithCurrentTime(Arc::new(move || {
            Ok(UTC.timestamp_micros(now_micros).unwrap())
        })),
        WithMaxAllowedPacket(12345),
        WithDefaultWeekFormatMode("3".to_owned()),
        WithDivPrecisionIncrement(5),
        WithUserVarsReader(Box::new(TestUserVars { value: 9 })),
        WithOptionalProperty(vec![
            provider(exprctx::OptPropCurrentUser),
            provider(exprctx::OptPropDDLOwnerInfo),
        ]),
    ]);
    (
        ctx,
        OptionState {
            now_micros,
            warning_handler,
            ddl_owner,
        },
    )
}

/// 断言全选项上下文的字段与共享句柄一致。
fn checkOptionsStaticEvalCtx(ctx: &EvalContext, state: &OptionState) {
    assert!(std::ptr::eq(
        ctx.GetWarnHandler(),
        state.warning_handler.as_ref()
    ));
    assert_eq!(
        ctx.SQLMode(),
        sql_modes(mysql::ModeNoZeroDate, mysql::ModeStrictTransTables)
    );
    assert_eq!(ctx.TypeCtx().Flags(), types::Flags(7));
    assert_eq!(ctx.Location(), New_York);
    assert_eq!(ctx.CurrentDB(), "db1");
    assert_eq!(
        ctx.CurrentTime().unwrap().timestamp_micros(),
        state.now_micros
    );
    assert_eq!(ctx.GetMaxAllowedPacket(), 12345);
    assert_eq!(ctx.GetDefaultWeekFormatMode(), "3");
    assert_eq!(ctx.GetDivPrecisionIncrement(), 5);
    assert_eq!(
        ctx.GetUserVarsReader()
            .GetUserVarVal("a")
            .unwrap()
            .GetInt64(),
        9
    );
    assert!(
        ctx.GetOptionalPropSet()
            .Contains(exprctx::OptPropCurrentUser)
    );
    assert!(
        ctx.GetOptionalPropSet()
            .Contains(exprctx::OptPropDDLOwnerInfo)
    );
    assert!(
        ctx.GetOptionalPropProvider(exprctx::OptPropInfoSchema)
            .is_none()
    );
    state.ddl_owner.store(true, Ordering::SeqCst);
    assert!(state.ddl_owner.load(Ordering::SeqCst));
}

/// 默认构造与全选项构造的 CtxID 递增及字段正确性。
#[test]
fn TestNewStaticEvalCtx() {
    let first = NewEvalContext(Vec::new());
    checkDefaultStaticEvalCtx(&first);
    let (second, state) = contextWithAllOptions();
    assert!(second.CtxID() > first.CtxID());
    checkOptionsStaticEvalCtx(&second, &state);
}

/// 语句时间：失败不缓存、成功后缓存；Apply 继承或覆盖时区/回调。
#[test]
fn TestStaticEvalCtxCurrentTime() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_fn = calls.clone();
    let expected = New_York.timestamp_micros(123_456_789).unwrap();
    let ctx = NewEvalContext(vec![WithCurrentTime(Arc::new(move || match calls_for_fn
        .fetch_add(1, Ordering::SeqCst)
    {
        0 => Err(contextutil::errors::NewNoStackError("err0")),
        1 => Err(contextutil::errors::NewNoStackError("err1")),
        2 => Ok(expected),
        _ => Err(contextutil::errors::NewNoStackError(
            "should not reach here",
        )),
    }))]);
    assert_eq!(ctx.CurrentTime().unwrap_err().to_string(), "err0");
    assert_eq!(ctx.CurrentTime().unwrap_err().to_string(), "err1");
    let got = ctx.CurrentTime().unwrap();
    assert_eq!(got.timestamp_micros(), expected.timestamp_micros());
    assert_eq!(got.timezone(), UTC);
    assert_eq!(ctx.CurrentTime().unwrap(), got);
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    let located = NewEvalContext(vec![
        WithLocation(Sydney),
        WithCurrentTime(Arc::new(move || Ok(expected))),
    ]);
    let located_time = located.CurrentTime().unwrap();
    assert_eq!(located_time.timestamp_micros(), expected.timestamp_micros());
    assert_eq!(located_time.timezone(), Sydney);
    assert_eq!(
        located.Apply(Vec::new()).CurrentTime().unwrap(),
        located_time
    );
    let moved = located.Apply(vec![WithLocation(New_York)]);
    assert_eq!(moved.CurrentTime().unwrap().timezone(), New_York);
    assert_eq!(located.CurrentTime().unwrap().timezone(), Sydney);
    let replaced = located.Apply(vec![WithCurrentTime(Arc::new(|| {
        Ok(UTC.timestamp_micros(987_654_321).unwrap())
    }))]);
    assert_eq!(
        replaced.CurrentTime().unwrap().timestamp_micros(),
        987_654_321
    );
    assert_eq!(replaced.CurrentTime().unwrap().timezone(), Sydney);
}

/// 将告警列表展平为 (级别, 消息) 便于断言。
fn warning_messages(warnings: &[contextutil::SQLWarn]) -> Vec<(String, String)> {
    warnings
        .iter()
        .map(|warning| {
            (
                warning.Level.clone(),
                warning.Err.as_ref().unwrap().to_string(),
            )
        })
        .collect()
}

/// 告警经上下文/TypeCtx/ErrCtx 汇入同一处理器；Apply 可共享或替换。
#[test]
fn TestStaticEvalCtxWarnings() {
    let empty = NewEvalContext(Vec::new());
    assert_eq!(empty.WarningCount(), 0);
    let handler = Arc::new(contextutil::NewStaticWarnHandler(8));
    let ctx = NewEvalContext(vec![WithWarnHandler(handler.clone())]);
    handler.AppendWarning(contextutil::errors::NewNoStackError("warn0"));
    ctx.AppendWarning(contextutil::errors::NewNoStackError("warn1"));
    ctx.AppendNote(contextutil::errors::NewNoStackError("note1"));
    ctx.TypeCtx()
        .AppendWarning(contextutil::errors::NewNoStackError("warn2"));
    ctx.ErrCtx()
        .AppendWarning(contextutil::errors::NewNoStackError("warn3"));
    assert_eq!(ctx.WarningCount(), 5);
    assert_eq!(
        warning_messages(&ctx.CopyWarnings(Vec::new())),
        vec![
            ("Warning".to_owned(), "warn0".to_owned()),
            ("Warning".to_owned(), "warn1".to_owned()),
            ("Note".to_owned(), "note1".to_owned()),
            ("Warning".to_owned(), "warn2".to_owned()),
            ("Warning".to_owned(), "warn3".to_owned()),
        ]
    );
    assert_eq!(
        warning_messages(&ctx.TruncateWarnings(2)),
        vec![
            ("Note".to_owned(), "note1".to_owned()),
            ("Warning".to_owned(), "warn2".to_owned()),
            ("Warning".to_owned(), "warn3".to_owned()),
        ]
    );
    assert_eq!(ctx.WarningCount(), 2);

    let shared = ctx.Apply(Vec::new());
    assert!(std::ptr::eq(ctx.GetWarnHandler(), shared.GetWarnHandler()));
    let replacement = Arc::new(contextutil::NewStaticWarnHandler(16));
    let separate = ctx.Apply(vec![WithWarnHandler(replacement)]);
    separate
        .TypeCtx()
        .AppendWarning(contextutil::errors::NewNoStackError("new"));
    ctx.ErrCtx()
        .AppendWarning(contextutil::errors::NewNoStackError("old"));
    assert_eq!(separate.WarningCount(), 1);
    assert_eq!(ctx.WarningCount(), 3);
}

/// 可选属性按 Apply 整体替换，不与旧集合合并。
#[test]
fn TestStaticEvalContextOptionalProps() {
    let ctx = NewEvalContext(Vec::new());
    let current_user = ctx.Apply(vec![WithOptionalProperty(vec![provider(
        exprctx::OptPropCurrentUser,
    )])]);
    assert!(ctx.GetOptionalPropSet().IsEmpty());
    assert!(
        current_user
            .GetOptionalPropSet()
            .Contains(exprctx::OptPropCurrentUser)
    );
    let replaced = current_user.Apply(vec![WithOptionalProperty(vec![
        provider(exprctx::OptPropDDLOwnerInfo),
        provider(exprctx::OptPropInfoSchema),
    ])]);
    assert!(
        replaced
            .GetOptionalPropSet()
            .Contains(exprctx::OptPropDDLOwnerInfo)
    );
    assert!(
        replaced
            .GetOptionalPropSet()
            .Contains(exprctx::OptPropInfoSchema)
    );
    assert!(
        !replaced
            .GetOptionalPropSet()
            .Contains(exprctx::OptPropCurrentUser)
    );
    assert!(
        current_user
            .GetOptionalPropSet()
            .Contains(exprctx::OptPropCurrentUser)
    );
}

/// Apply 分配新 CtxID，默认继承告警处理器；原上下文不受后续选项影响。
#[test]
fn TestUpdateStaticEvalContext() {
    let old = NewEvalContext(Vec::new());
    let applied = old.Apply(Vec::new());
    assert!(applied.CtxID() > old.CtxID());
    assert!(std::ptr::eq(old.GetWarnHandler(), applied.GetWarnHandler()));
    checkDefaultStaticEvalCtx(&old);
    checkDefaultStaticEvalCtx(&applied);
    let (configured, state) = contextWithAllOptions();
    checkOptionsStaticEvalCtx(&configured, &state);
    checkDefaultStaticEvalCtx(&old);
}

/// 预处理参数列表在构造后与外部 Vec 解耦；越界返回错误。
#[test]
fn TestParamList() {
    let mut params = vec![int_datum(1), int_datum(2), int_datum(3)];
    let ctx = NewEvalContext(vec![WithParamList(params.clone())]);
    params.clear();
    params.push(int_datum(4));
    for (index, expected) in [1, 2, 3].into_iter().enumerate() {
        assert_eq!(ctx.GetParamValue(index).unwrap().GetInt64(), expected);
    }
    assert!(ctx.GetParamValue(3).is_err());
}

/// `MakeEvalContextStatic` 物化快照：复用告警 Arc，清空可选属性。
#[test]
fn TestMakeEvalContextStatic() {
    let handler = Arc::new(contextutil::NewStaticWarnHandler(16));
    let fixed_time = UTC.timestamp_opt(1_700_000_000, 0).unwrap();
    let obj = NewEvalContext(vec![
        WithWarnHandler(handler),
        WithSQLMode(sql_modes(
            mysql::ModeNoZeroDate,
            mysql::ModeStrictTransTables,
        )),
        WithTypeFlags(types::Flags(9)),
        WithLocation(New_York),
        WithCurrentDB("db1".to_owned()),
        WithCurrentTime(Arc::new(move || Ok(fixed_time))),
        WithMaxAllowedPacket(12345),
        WithDefaultWeekFormatMode("3".to_owned()),
        WithDivPrecisionIncrement(5),
        WithParamList(vec![int_datum(1)]),
        WithUserVarsReader(Box::new(TestUserVars { value: 2 })),
        WithOptionalProperty(vec![provider(exprctx::OptPropDDLOwnerInfo)]),
        WithEnableRedactLog("ON".to_owned()),
    ]);
    obj.AppendWarning(contextutil::errors::NewNoStackError("test warning"));
    let static_obj = MakeEvalContextStatic(&obj);
    assert_eq!(static_obj.SQLMode(), obj.SQLMode());
    assert_eq!(static_obj.TypeCtx().Flags(), obj.TypeCtx().Flags());
    assert_eq!(static_obj.ErrCtx().LevelMap(), obj.ErrCtx().LevelMap());
    assert_eq!(static_obj.Location(), obj.Location());
    assert_eq!(static_obj.CurrentDB(), obj.CurrentDB());
    assert_eq!(
        static_obj.CurrentTime().unwrap(),
        obj.CurrentTime().unwrap()
    );
    assert_eq!(static_obj.GetParamValue(0).unwrap().GetInt64(), 1);
    assert_eq!(
        static_obj
            .GetUserVarsReader()
            .GetUserVarVal("a")
            .unwrap()
            .GetInt64(),
        2
    );
    assert_eq!(static_obj.GetTiDBRedactLog(), "ON");
    assert!(static_obj.GetOptionalPropSet().IsEmpty());
    assert!(std::ptr::eq(
        obj.GetWarnHandler(),
        static_obj.GetWarnHandler()
    ));
    assert_eq!(static_obj.WarningCount(), 1);
}

/// `LoadSystemVars` 解析时区/SQL Mode/timestamp 等并 Apply；默认 timestamp 取墙钟。
#[test]
fn TestEvalCtxLoadSystemVars() {
    let defaults = NewEvalContext(Vec::new());
    let loaded = defaults
        .LoadSystemVars(&HashMap::from([
            ("time_zone".to_owned(), "Europe/Berlin".to_owned()),
            (
                "sql_mode".to_owned(),
                "ALLOW_INVALID_DATES,ONLY_FULL_GROUP_BY".to_owned(),
            ),
            ("timestamp".to_owned(), "1234567890.123456".to_owned()),
            ("MAX_ALLOWED_PACKET".to_owned(), "524288".to_owned()),
            ("TIDB_REDACT_LOG".to_owned(), "ON".to_owned()),
            ("default_week_format".to_owned(), "5".to_owned()),
            ("div_precision_increment".to_owned(), "12".to_owned()),
        ]))
        .unwrap();
    assert!(loaded.CtxID() > defaults.CtxID());
    assert_eq!(loaded.Location(), Berlin);
    assert_eq!(
        loaded.SQLMode(),
        sql_modes(mysql::ModeAllowInvalidDates, mysql::ModeOnlyFullGroupBy)
    );
    assert_eq!(
        loaded.CurrentTime().unwrap().timestamp_micros(),
        1_234_567_890_123_456
    );
    assert_eq!(loaded.CurrentTime().unwrap().timezone(), Berlin);
    assert_eq!(loaded.GetMaxAllowedPacket(), 524288);
    assert_eq!(loaded.GetTiDBRedactLog(), "ON");
    assert_eq!(loaded.GetDefaultWeekFormatMode(), "5");
    assert_eq!(loaded.GetDivPrecisionIncrement(), 12);
    assert_eq!(loaded.CurrentDB(), defaults.CurrentDB());
    assert!(std::ptr::eq(
        loaded.GetWarnHandler(),
        defaults.GetWarnHandler()
    ));

    let before = Utc::now();
    let current = defaults
        .LoadSystemVars(&HashMap::from([(
            "timestamp".to_owned(),
            vardef::DefTimestamp.to_owned(),
        )]))
        .unwrap()
        .CurrentTime()
        .unwrap();
    let after = Utc::now();
    assert!(current.with_timezone(&Utc) >= before - Duration::from_secs(5));
    assert!(current.with_timezone(&Utc) <= after + Duration::from_secs(5));
}

#[test]
/// `timestamp=0` 应在首次 CurrentTime 调用时取墙钟，而不是在加载变量时提前固定。
fn TestDefaultTimestampIsEvaluatedLazily() {
    let loaded = NewEvalContext(Vec::new())
        .LoadSystemVars(&HashMap::from([(
            "timestamp".to_owned(),
            vardef::DefTimestamp.to_owned(),
        )]))
        .unwrap();

    std::thread::sleep(Duration::from_millis(50));
    let just_before_current_time = Utc::now();
    let current = loaded.CurrentTime().unwrap().with_timezone(&Utc);

    assert!(
        current >= just_before_current_time - Duration::from_millis(5),
        "timestamp=0 was evaluated before CurrentTime: current={current}, before={just_before_current_time}"
    );
}

#[test]
/// 已注册但与 EvalCtx 字段无关的会话变量应被校验后忽略。
fn TestLoadSystemVarsAcceptsUnrelatedRegisteredVariables() {
    let defaults = NewEvalContext(Vec::new());
    let loaded = defaults
        .LoadSystemVars(&HashMap::from([(
            "max_execution_time".to_owned(),
            "1000".to_owned(),
        )]))
        .unwrap();

    assert!(loaded.CtxID() > defaults.CtxID());
    assert_eq!(loaded.SQLMode(), defaults.SQLMode());
    assert_eq!(loaded.GetMaxAllowedPacket(), defaults.GetMaxAllowedPacket());
    assert!(
        defaults
            .LoadSystemVars(&HashMap::from([(
                "max_execution_time".to_owned(),
                "invalid".to_owned(),
            )]))
            .is_err()
    );
}

#[test]
/// 数值系统变量按 Go SysVar 规则裁剪，redact hook 保留原始值。
fn TestEvalSystemVarNormalizationMatchesGo() {
    let loaded = NewEvalContext(Vec::new())
        .LoadSystemVars(&HashMap::from([
            ("max_allowed_packet".to_owned(), "2049".to_owned()),
            ("default_week_format".to_owned(), "9".to_owned()),
            ("div_precision_increment".to_owned(), "31".to_owned()),
            ("tidb_redact_log".to_owned(), "custom".to_owned()),
        ]))
        .unwrap();

    assert_eq!(loaded.GetMaxAllowedPacket(), 2048);
    assert_eq!(loaded.GetDefaultWeekFormatMode(), "7");
    assert_eq!(loaded.GetDivPrecisionIncrement(), 30);
    assert_eq!(loaded.GetTiDBRedactLog(), "custom");
}
