// Copyright 2026 AsterSQL.

// 公开 API 面与 Go `go doc -all` 的编译期/运行期对等性清单。
//
// 通过触及各导出类型、方法与模块函数，确保 Rust 映射未遗漏源包表面；
// 详细源测试矩阵见下方英文注释中的文件对照表。

use std::error::Error as StdError;
use std::fmt;

use astersql_errors::*;

/// 测试用多原因错误组，仅用于断言 [`ErrorGroup`] 可实现性。
#[derive(Debug)]
struct ApiGroup(Vec<SharedError>);

impl fmt::Display for ApiGroup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("api group")
    }
}

impl StdError for ApiGroup {}

impl ErrorGroup for ApiGroup {
    fn Errors(&self) -> Vec<SharedError> {
        self.0.clone()
    }
}

/// 测试用 [`HackedStr`]：冻结为固定字符串 `"frozen"`。
struct ApiHackedStr;

impl HackedStr for ApiHackedStr {
    fn FreezeStr(&self) -> String {
        "frozen".to_owned()
    }
}

/// 编译期约束：类型实现 [`StackTracer`]。
fn assert_stack_tracer<T: StackTracer>(_: &T) {}
/// 编译期约束：类型实现 [`StackTraceCarrier`]。
fn assert_stack_carrier<T: StackTraceCarrier>(_: &T) {}
/// 编译期约束：类型实现 [`ErrorGroup`]。
fn assert_error_group<T: ErrorGroup>(_: &T) {}
/// 编译期约束：类型实现 [`HackedStr`]。
fn assert_hacked_str<T: HackedStr>(_: &T) {}

/// Compile-time inventory for `go doc -all .` at source commit 306e305bcf41.
///
/// The source surface is 38 module functions, 26 exported concrete methods,
/// and 12 exported types. Go's variadics map to `&[ErrorArg]`, nil maps to
/// `Option`, and error ownership maps to `SharedError`. Go formatter methods
/// map to `Display`/`Debug`; JSON methods map to serde. `StackTraceAware`'s
/// runtime marker maps to `HasStack` plus the explicit `StackTraceCarrier`
/// trait used for Rust cause-chain discovery.
///
/// Source test-intent matrix (the Rust files consolidate table-driven cases):
///
/// - `errors_test.go` (22): core, wrap, group/join, and std-interop tests.
/// - `format_test.go` (6): format test.
/// - `stack_test.go` (10): stack and adaptor tests.
/// - `normalize_test.go` (11): normalize identity and generation tests.
/// - `join_test.go` (3): group/join test.
/// - `example_test.go` (12): example test, one Rust test per example.
/// - `bench_test.go` (3): the three Criterion functions in `benches/errors.rs`.
/// - `terror_test.go` (8 suite cases): normalize tests plus parser terror tests.
///
/// 在固定 Go 源提交上盘点全部公开符号，运行时逐一触及以防映射遗漏。
#[test]
fn errors_api_parity_test() {
    // Constants, global state, and all 12 source types.
    // 常量、全局脱敏开关与全部 12 个源类型的构造触达
    let _: ErrCode = 1;
    let _: ErrCodeText = "code".to_owned();
    let _: ErrorID = "id".to_owned();
    let _: RFCErrorCode = "component:class:code".to_owned();
    let _: NormalizeOption = MySQLErrorCode(1);
    let _: Frame = Frame::from_instruction_pointer(0);
    let _: StackTrace = StackTrace::default();
    let _: &str = RedactLogEnable;
    let _: &str = RedactLogDisable;
    let _: &str = RedactLogMarker;
    let original_redact_mode = RedactLogEnabled.Load();
    RedactLogEnabled.Store(RedactLogDisable);

    let hacked = ApiHackedStr;
    assert_hacked_str(&hacked);
    assert_eq!(hacked.FreezeStr(), "frozen");
    let group = ApiGroup(vec![New("child")]);
    assert_error_group(&group);
    assert_eq!(group.Errors().len(), 1);
    assert_eq!(group.Errors()[0].to_string(), "child");

    let stack = NewStack(0);
    assert_stack_tracer(&stack);
    assert_stack_carrier(&stack);
    let tracer = GetStackTracer(&stack).expect("NewStack exposes StackTracer");
    let trace = tracer.StackTrace();
    let _ = tracer.Empty();
    let _ = trace.len();
    let _ = trace.is_empty();
    let frame = Frame::from_instruction_pointer(0);
    let _ = frame.instruction_pointer();
    let _ = frame.file();
    let _ = frame.line();
    let _ = frame.function();
    let _ = format!("{frame} {frame:#?} {trace} {trace:#?}");

    // All normalize options and Error methods. Display/Debug stand in for
    // Go Error/Format; serde_json stands in for MarshalJSON/UnmarshalJSON.
    // 规范化选项与 Error 方法；Display/Debug 与 serde 对应 Go 格式化/JSON
    let prototype: Error = Normalize(
        "item %s",
        &[
            RFCCodeText("api:Item"),
            MySQLErrorCode(42),
            RedactArgs(&[0]),
        ],
    );
    let _ = prototype.Args();
    let _ = prototype.Cause();
    let _ = prototype.Code();
    let _ = prototype.Equal(None);
    let _ = prototype.NotEqual(None);
    let _ = prototype.FastGen("fast %s", &["value".into()]);
    let _ = prototype.FastGenByArgs(&["value".into()]);
    let _ = prototype.FastGenWithCause(&["value".into()]);
    let _ = prototype.GenWithStack("stack %s", &["value".into()]);
    let _ = prototype.GenWithStackByArgs(&["value".into()]);
    let _ = prototype.GenWithStackByCause(&["value".into()]);
    let _ = prototype.GetMsg();
    let _ = prototype.GetSelfMsg();
    let _ = prototype.ID();
    let _ = prototype.Is(&prototype);
    let _ = prototype.Location();
    let _ = prototype.MessageTemplate();
    let _ = prototype.RFCCode();
    let _ = prototype.Unwrap();
    let _ = prototype.Wrap(Some(New("cause")));
    let json = serde_json::to_string(&prototype).expect("Error serializes");
    let decoded: Error = serde_json::from_str(&json).expect("Error deserializes");
    let _ = format!("{decoded} {decoded:#?}");

    // All remaining module-level functions from go doc.
    // 其余模块级函数：包装、查找、分组、Join 与 Juju 适配
    let base = New("base");
    let formatted = Errorf("value %d", &[1.into()]);
    let _ = WithMessage(Some(base.clone()), "message");
    let _ = WithStack(Some(base.clone()));
    let _ = AddStack(Some(base.clone()));
    let _ = Wrap(Some(base.clone()), "wrap");
    let _ = Wrapf(Some(base.clone()), "wrap %s", &["value".into()]);
    let _ = Annotate(Some(base.clone()), "annotate");
    let _ = Annotatef(Some(base.clone()), "annotate %s", &["value".into()]);
    let _ = Trace(Some(base.clone()));
    let _ = SuspendStack(Some(base.clone()));
    let _ = Cause(Some(&base));
    let _ = Unwrap(Some(&base));
    let _ = HasStack(&base);
    let _ = Find(Some(&base), |_| true);
    let _ = GetErrStackMsg(Some(&base));
    let _ = ErrorStack(Some(&base));
    let _ = ErrorEqual(Some(&base), Some(&formatted));
    let _ = ErrorNotEqual(Some(&base), Some(&formatted));
    let _ = Errors(&base);
    let _ = WalkDeep(Some(&base), |_| false);
    let _ = Join(&[Some(base.clone()), None]);

    let mut redactable = vec![ErrorArg::from_hacked(&hacked)];
    RedactErrorArg(&mut redactable, &[0]);
    let no_stack = NewNoStackError("no stack");
    let _ = NewNoStackErrorf("no %s", &["stack".into()]);
    let not_found = NotFoundf("item %s", &["x".into()]);
    let already_exists = AlreadyExistsf("item %s", &["x".into()]);
    let _ = IsNotFound(&not_found);
    let _ = IsAlreadyExists(&already_exists);
    let _ = BadRequestf("item %s", &["x".into()]);
    let _ = NotSupportedf("item %s", &["x".into()]);
    let _ = NotValidf("item %s", &["x".into()]);
    let _ = no_stack;

    RedactLogEnabled.Store(original_redact_mode);
}
