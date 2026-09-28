// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// terror 单元测试，对齐 Go `terror_test.go`。
//
// 验证错误码常量、类别注册、JSON 序列化、相等性、日志、栈追踪，
// 以及 RegisterFinish 冻结后的构造限制。

// 本文件按 terror_test.go 的六组测试顺序验证 Rust 对齐行为。

use crate::parser::terror::{
    Call, ClassAdmin, ClassAutoid, ClassDDL, ClassDomain, ClassEvaluator, ClassExecutor,
    ClassExpression, ClassGlobal, ClassJSON, ClassKV, ClassMeta, ClassMockTikv, ClassOptimizer,
    ClassParser, ClassPerfSchema, ClassPlugin, ClassPrivilege, ClassSchema, ClassServer,
    ClassSession, ClassStructure, ClassTable, ClassTiKV, ClassTypes, ClassUtil, ClassVariable,
    ClassXEval, CodeExecResultIsEmpty, CodeMissConnectionID, CodeResultUndetermined, ErrClass,
    ErrClassToMySQLCodes, ErrCode, ErrorEqual, ErrorNotEqual, GetErrClass, Log, MustNil,
    RegisterErrorClass, RegisterFinish, ToSQLError, newCode2ErrClassMap,
};
use std::process::Command;
use std::sync::{
    Arc, Mutex, Once,
    atomic::{AtomicUsize, Ordering},
};

/// 测试用日志收集器：把记录缓存在内存供断言。
struct TerrorTestLogger {
    records: Mutex<Vec<(log::Level, String)>>,
}

impl log::Log for TerrorTestLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        self.records
            .lock()
            .expect("test log records poisoned")
            .push((record.level(), record.args().to_string()));
    }

    fn flush(&self) {}
}

/// 全局测试 logger 实例。
static TERROR_TEST_LOGGER: TerrorTestLogger = TerrorTestLogger {
    records: Mutex::new(Vec::new()),
};
/// 保证 logger 只安装一次。
static INSTALL_TERROR_TEST_LOGGER: Once = Once::new();

/// 安装测试 logger（幂等）。
fn install_terror_test_logger() {
    INSTALL_TERROR_TEST_LOGGER.call_once(|| {
        log::set_logger(&TERROR_TEST_LOGGER).expect("install terror test logger");
        log::set_max_level(log::LevelFilter::Trace);
    });
}

/// 检查是否已记录指定级别与消息。
fn logged(level: log::Level, message: &str) -> bool {
    TERROR_TEST_LOGGER
        .records
        .lock()
        .expect("test log records poisoned")
        .iter()
        .any(|record| record == &(level, message.to_owned()))
}

// 对应 Go TestErrCode。
/// 校验内置 ErrCode 常量数值。
#[test]
fn test_err_code() {
    assert_eq!(CodeMissConnectionID, ErrCode(1));
    assert_eq!(CodeResultUndetermined, ErrCode(2));
}

// 对应 Go TestTError。Rust 用 ErrorArg slice 代替 Go 可变参数。
/// 校验类别字符串、EqualClass、FastGen 与 ToSQLError 转换。
#[test]
fn test_t_error() {
    assert!(!ClassParser.String().is_empty());
    assert!(!ClassOptimizer.String().is_empty());
    assert!(!ClassKV.String().is_empty());
    assert!(!ClassServer.String().is_empty());

    #[allow(deprecated)]
    let parser_error = ClassParser.New(ErrCode(100), "error 100");
    assert!(!parser_error.to_string().is_empty());
    let parser_error = crate::errors::SharedError::new((*parser_error).clone());
    assert!(ClassParser.EqualClass(Some(&parser_error)));
    assert!(!ClassParser.NotEqualClass(Some(&parser_error)));
    assert!(!ClassOptimizer.EqualClass(Some(&parser_error)));

    #[allow(deprecated)]
    let optimizer_error = ClassOptimizer.New(ErrCode(2), "abc");
    assert!(!ClassOptimizer.EqualClass(Some(&crate::errors::New("abc"))));
    assert!(!ClassOptimizer.EqualClass(None));
    assert!(optimizer_error.Equal(Some(&optimizer_error.GenWithStack("def", &[]))));
    assert!(!optimizer_error.Equal(None));
    assert!(!optimizer_error.Equal(Some(&crate::errors::New("abc"))));

    assert!(optimizer_error.Equal(Some(&optimizer_error.FastGen("def", &[]))));
    assert!(optimizer_error.Equal(Some(&optimizer_error.FastGen("def: %s", &["def".into()],))));

    let duplicate_entry = ErrCode(crate::parser::mysql::errcode::ErrDupEntry as isize);
    #[allow(deprecated)]
    let kv_error = ClassKV.New(duplicate_entry, "key already exist");
    let error = kv_error.FastGen("Duplicate entry '%d' for key 'PRIMARY'", &[1.into()]);
    assert_eq!(
        error.to_string(),
        "[kv:1062]Duplicate entry '1' for key 'PRIMARY'"
    );
    let cause = crate::errors::Cause(Some(&error)).expect("generated error cause");
    let sql_error = ToSQLError(
        cause
            .downcast_ref::<crate::parser::terror::Error>()
            .expect("generated terror error"),
    );
    assert_eq!(sql_error.Message, "Duplicate entry '1' for key 'PRIMARY'");
    assert_eq!(sql_error.Code, duplicate_entry.0 as u16);

    let critical = crate::errors::Trace(Some(
        crate::parser::terror::ErrCritical.GenWithStackByArgs(&["test".into()]),
    ))
    .expect("trace generated critical error");
    assert!(crate::parser::terror::ErrCritical.Equal(Some(&critical)));

    let critical = crate::errors::Trace(Some(crate::errors::SharedError::new(
        (**crate::parser::terror::ErrCritical).clone(),
    )))
    .expect("trace critical template");
    assert!(crate::parser::terror::ErrCritical.Equal(Some(&critical)));
}

// 对应 Go TestJson，并显式锁定 Go 兼容键。
/// 校验 Normalize 错误的 JSON 字段与反序列化后相等性。
#[test]
fn test_json() {
    let previous = crate::errors::Normalize(
        "json test",
        &[crate::errors::MySQLErrorCode(
            CodeExecResultIsEmpty.0 as i32,
        )],
    );
    let encoded = serde_json::to_value(&previous).expect("serialize normalized error");
    let object = encoded.as_object().expect("normalized error JSON object");
    assert_eq!(object.len(), 4);
    assert_eq!(object["class"], 0);
    assert_eq!(object["code"], CodeExecResultIsEmpty.0);
    assert_eq!(object["message"], "json test");
    assert_eq!(object["rfccode"], "");

    let current: crate::errors::Error =
        serde_json::from_value(encoded).expect("deserialize normalized error");
    let current = crate::errors::SharedError::new(current);
    assert!(previous.Equal(Some(&current)));
}

// 对应 Go TestErrorEqual。
/// 校验 Trace 后根因指针与 ErrorEqual 文本比较规则。
#[test]
fn test_error_equal() {
    let first = crate::errors::New("test error");
    let second = crate::errors::Trace(Some(first.clone())).expect("first trace");
    let third = crate::errors::Trace(Some(second.clone())).expect("second trace");

    let second_cause = crate::errors::Cause(Some(&second)).expect("second cause");
    let third_cause = crate::errors::Cause(Some(&third)).expect("third cause");
    assert!(first.ptr_eq(&second_cause));
    assert!(first.ptr_eq(&third_cause));
    assert!(second_cause.ptr_eq(&third_cause));

    let fourth = crate::errors::New("test error");
    let fifth = crate::errors::Errorf("test %s", &["error".into()]);
    assert!(!first.ptr_eq(&fourth));
    assert!(!first.ptr_eq(&fifth));
    assert!(ErrorEqual(Some(&first), Some(&second)));
    assert!(ErrorEqual(Some(&first), Some(&third)));
    assert!(ErrorEqual(Some(&first), Some(&fourth)));
    assert!(ErrorEqual(Some(&first), Some(&fifth)));
    assert!(ErrorEqual(None, None));
    assert!(ErrorNotEqual(Some(&first), None));

    let parser = crate::errors::SharedError::new(*ClassParser.Synthesize(ErrCode(9001), "abc"));
    let kv_same_code = crate::errors::SharedError::new(*ClassKV.Synthesize(ErrCode(9001), "abc"));
    let kv_other_code = crate::errors::SharedError::new(*ClassKV.Synthesize(ErrCode(9002), "abc"));
    assert!(!ErrorEqual(Some(&parser), Some(&kv_same_code)));
    assert!(!ErrorEqual(Some(&kv_same_code), Some(&kv_other_code)));
}

// 对应 Go TestLog。Rust 用测试 logger 验证 sink 收到真实记录。
/// 校验 Log 会写入 error 级别日志。
#[test]
fn test_log() {
    install_terror_test_logger();
    TERROR_TEST_LOGGER
        .records
        .lock()
        .expect("test log records poisoned")
        .clear();
    let error = crate::errors::Errorf("%s", &["xxx".into()]);
    Log(Some(&error));
    assert!(logged(log::Level::Error, "encountered error: xxx"));
}

/// 辅助：用预定义错误生成带栈的消息。
fn call(predefined: &crate::parser::terror::Error) -> crate::errors::SharedError {
    predefined.GenWithStack("error message:%s", &["abc".into()])
}

/// 辅助：再 Trace 一层，供栈位置测试。
fn example(predefined: &crate::parser::terror::Error) -> crate::errors::SharedError {
    crate::errors::Trace(Some(call(predefined))).expect("trace call error")
}

// 对应 Go TestTraceAndLocation。Rust 校验业务 frame 和 call/example 链，
// 不硬编码 Go runtime 的栈行数。
/// 校验 ErrorStack 包含本文件与 call/example 帧。
#[test]
fn test_trace_and_location() {
    #[allow(deprecated)]
    let predefined = ClassExecutor.New(ErrCode(123), "predefiend error");
    let error = example(&predefined);
    let stack = crate::errors::ErrorStack(Some(&error));

    assert!(error.to_string().contains("error message:abc"));
    assert!(stack.contains("terror_test.rs"), "stack =\n{stack}");
    assert!(
        stack.contains("parser_terror_test::call"),
        "stack =\n{stack}"
    );
    assert!(
        stack.contains("parser_terror_test::example"),
        "stack =\n{stack}"
    );
}

/// 校验 RegisterErrorClass 成功注册且重复编号 panic。
#[test]
fn parser_terror_registers_error_classes() {
    let class = RegisterErrorClass(10_001, "test class");
    assert_eq!(class.String(), "test class");

    let duplicate = std::panic::catch_unwind(|| {
        RegisterErrorClass(10_001, "duplicate test class");
    });
    assert!(duplicate.is_err());
    assert_eq!(class.String(), "test class");
}

/// 校验 27 个内置 ErrClass 编号与描述，以及反向映射 Get/Put。
#[test]
fn terror_registry_matches_go_classes() {
    let expected: [(ErrClass, isize, &str); 27] = [
        (ClassAutoid, 1, "autoid"),
        (ClassDDL, 2, "ddl"),
        (ClassDomain, 3, "domain"),
        (ClassEvaluator, 4, "evaluator"),
        (ClassExecutor, 5, "executor"),
        (ClassExpression, 6, "expression"),
        (ClassAdmin, 7, "admin"),
        (ClassKV, 8, "kv"),
        (ClassMeta, 9, "meta"),
        (ClassOptimizer, 10, "planner"),
        (ClassParser, 11, "parser"),
        (ClassPerfSchema, 12, "perfschema"),
        (ClassPrivilege, 13, "privilege"),
        (ClassSchema, 14, "schema"),
        (ClassServer, 15, "server"),
        (ClassStructure, 16, "structure"),
        (ClassVariable, 17, "variable"),
        (ClassXEval, 18, "xeval"),
        (ClassTable, 19, "table"),
        (ClassTypes, 20, "types"),
        (ClassGlobal, 21, "global"),
        (ClassMockTikv, 22, "mocktikv"),
        (ClassJSON, 23, "json"),
        (ClassTiKV, 24, "tikv"),
        (ClassSession, 25, "session"),
        (ClassPlugin, 26, "plugin"),
        (ClassUtil, 27, "util"),
    ];

    for (class, code, description) in expected {
        assert_eq!(class, ErrClass(code));
        assert_eq!(class.String(), description);
    }
    assert_eq!(ErrClass(9_999).String(), "9999");

    let duplicate = std::panic::catch_unwind(|| RegisterErrorClass(1, "duplicate autoid"));
    assert!(duplicate.is_err());
    for (class, _, description) in expected {
        assert_eq!(class.String(), description);
    }

    let reverse = newCode2ErrClassMap();
    assert_eq!(reverse.Get("parser"), (ErrClass(-1), false));
    reverse.Put("parser", ClassParser);
    assert_eq!(reverse.Get("parser"), (ClassParser, true));
}

/// 校验已注册码走真实 MySQL code，未注册 Synthesize 回退 ErrUnknown。
#[test]
fn parser_terror_converts_to_sql_error() {
    install_terror_test_logger();
    TERROR_TEST_LOGGER
        .records
        .lock()
        .expect("test log records poisoned")
        .clear();
    let class = RegisterErrorClass(10_003, "test sql error");
    let registered_code = ErrCode(crate::parser::mysql::errcode::ErrNoDB as isize);
    #[allow(deprecated)]
    let registered = class.New(registered_code, "registered message");
    let registered_sql_error = ToSQLError(&registered);
    assert_eq!(registered_sql_error.Code, registered_code.0 as u16);
    assert_eq!(registered_sql_error.Message, "registered message");

    let unregistered = class.Synthesize(ErrCode(7), "fallback message");
    let fallback_sql_error = ToSQLError(&unregistered);
    assert_eq!(
        fallback_sql_error.Code,
        crate::parser::mysql::errcode::ErrUnknown
    );
    assert_eq!(fallback_sql_error.Message, "fallback message");
    assert!(logged(
        log::Level::Debug,
        "Unknown error code: class=10003, code=7"
    ));

    let unknown_class = ErrClass(10_004).Synthesize(ErrCode(8), "unknown class message");
    assert_eq!(
        ToSQLError(&unknown_class).Code,
        crate::parser::mysql::errcode::ErrUnknown
    );
    assert!(logged(log::Level::Warn, "Unknown error class: 0"));
}

/// Go 包初始化会在任何调用前注册 ErrCritical/ErrResultUndetermined 的 global 错误码。
#[test]
fn global_error_codes_are_registered_during_package_initialization() {
    const GLOBAL_INIT_HELPER_ENV: &str = "TERROR_GLOBAL_INIT_HELPER";

    if std::env::var_os(GLOBAL_INIT_HELPER_ENV).is_some() {
        let synthesized =
            ClassGlobal.Synthesize(CodeExecResultIsEmpty, "critical error from remote");
        assert_eq!(
            ToSQLError(&synthesized).Code,
            CodeExecResultIsEmpty.0 as u16
        );
        return;
    }

    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("parser_terror_test::global_error_codes_are_registered_during_package_initialization")
        .env(GLOBAL_INIT_HELPER_ENV, "1")
        .status()
        .expect("run package-initialization subprocess helper");
    assert!(status.success(), "package initialization helper failed");
}

/// 校验 New/Synthesize/NewStd 注册语义，并在子进程验证 RegisterFinish 后 New 会 panic。
#[test]
fn terror_constructors_register_codes_and_freeze_in_subprocess() {
    const FREEZE_HELPER_ENV: &str = "TERROR_REGISTER_FINISH_HELPER";

    if std::env::var_os(FREEZE_HELPER_ENV).is_some() {
        let frozen_class = RegisterErrorClass(10_014, "frozen constructor");
        RegisterFinish();
        let result = std::panic::catch_unwind(|| {
            #[allow(deprecated)]
            let _ = frozen_class.New(ErrCode(14), "must panic after freeze");
        });
        assert!(
            result.is_err(),
            "constructing after RegisterFinish must panic"
        );
        return;
    }

    let class = RegisterErrorClass(10_013, "constructor test");
    let legacy_code = ErrCode(crate::parser::mysql::errcode::ErrNoDB as isize);
    #[allow(deprecated)]
    let legacy = class.New(legacy_code, "legacy message");
    let _: &crate::errors::Error = &legacy;
    assert_eq!(legacy.Code(), legacy_code.0 as i32);
    assert_eq!(legacy.RFCCode(), "constructor test:1046");
    assert!(
        ErrClassToMySQLCodes
            .read()
            .expect("error-code registry poisoned")[&class]
            .contains_key(&legacy_code)
    );

    let synthesized_code = ErrCode(10_015);
    let synthesized = class.Synthesize(synthesized_code, "external message");
    assert_eq!(synthesized.Code(), synthesized_code.0 as i32);
    assert!(
        !ErrClassToMySQLCodes
            .read()
            .expect("error-code registry poisoned")[&class]
            .contains_key(&synthesized_code)
    );

    let standard = ClassParser.NewStd(legacy_code);
    assert_eq!(standard.GetMsg(), "No database selected");
    assert_eq!(ToSQLError(&standard).Code, legacy_code.0 as u16);
    assert_eq!(
        ToSQLError(&synthesized).Code,
        crate::parser::mysql::errcode::ErrUnknown
    );

    assert_eq!(crate::parser::terror::ErrCritical.Code(), 3);
    assert_eq!(crate::parser::terror::ErrResultUndetermined.Code(), 2);

    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("parser_terror_test::terror_constructors_register_codes_and_freeze_in_subprocess")
        .env(FREEZE_HELPER_ENV, "1")
        .status()
        .expect("run RegisterFinish subprocess helper");
    assert!(status.success(), "freeze subprocess failed with {status}");
}

/// 校验 EqualClass、ErrorEqual、Log/Call 与 MustNil 清理顺序。
#[test]
fn terror_helpers_match_go_terminal_and_logging_semantics() {
    const MUST_NIL_HELPER_ENV: &str = "TERROR_MUST_NIL_HELPER";

    if std::env::var_os(MUST_NIL_HELPER_ENV).is_some() {
        let cleanup_order = Arc::new(AtomicUsize::new(0));
        let first_order = Arc::clone(&cleanup_order);
        let second_order = Arc::clone(&cleanup_order);
        let mut cleanup: Vec<Box<dyn FnMut()>> = vec![
            Box::new(move || {
                first_order
                    .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                    .expect("first cleanup must run first");
            }),
            Box::new(move || {
                second_order
                    .compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst)
                    .expect("second cleanup must run second");
            }),
        ];
        let terminal_error = crate::errors::New("must nil helper error");
        MustNil(Some(&terminal_error), &mut cleanup);
        unreachable!("MustNil must terminate the process");
    }

    install_terror_test_logger();

    let class = RegisterErrorClass(10_015, "helper test");
    #[allow(deprecated)]
    let normalized = class.New(ErrCode(15), "normalized helper error");
    let normalized = crate::errors::SharedError::new(*normalized);
    let traced = crate::errors::Trace(Some(normalized.clone())).expect("trace normalized error");
    assert!(class.EqualClass(Some(&traced)));
    assert!(!ClassParser.EqualClass(Some(&traced)));
    assert!(ClassParser.NotEqualClass(Some(&traced)));
    assert_eq!(
        GetErrClass(
            normalized
                .downcast_ref::<crate::parser::terror::Error>()
                .expect("normalized terror error"),
        ),
        class
    );

    let unregistered_class = ErrClass(10_016);
    let unregistered = crate::errors::SharedError::new(
        *unregistered_class.Synthesize(ErrCode(16), "unregistered helper error"),
    );
    assert!(!unregistered_class.EqualClass(Some(&unregistered)));

    let plain_left = crate::errors::New("same plain helper error");
    let plain_right = crate::errors::New("same plain helper error");
    let traced_plain = crate::errors::Trace(Some(plain_left.clone())).expect("trace plain error");
    assert!(ErrorEqual(Some(&traced_plain), Some(&plain_right)));
    assert!(!ErrorNotEqual(Some(&traced_plain), Some(&plain_right)));
    assert!(ErrorEqual(None, None));
    assert!(ErrorNotEqual(Some(&plain_left), None));

    Log(Some(&plain_left));
    Log(None);
    Call(|| -> Result<(), crate::errors::SharedError> {
        Err(crate::errors::New("call helper error"))
    });
    assert!(logged(
        log::Level::Error,
        "encountered error: same plain helper error"
    ));
    assert!(logged(
        log::Level::Error,
        "function call errored: call helper error"
    ));

    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("parser_terror_test::terror_helpers_match_go_terminal_and_logging_semantics")
        .env(MUST_NIL_HELPER_ENV, "1")
        .status()
        .expect("run MustNil subprocess helper");
    assert_eq!(
        status.code(),
        Some(1),
        "Go log.Fatal semantics require process exit code 1"
    );
}
