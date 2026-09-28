// Copyright 2026 AsterSQL.

// Normalize 错误生成与脱敏（redaction）路径测试。
//
// 覆盖 `GenWithStack*` / `FastGen*` / `*WithCause` 各生成入口的消息、栈标记，
// 以及 `RedactLogEnabled` 关闭/开启/Marker 模式下参数脱敏，和 `HackedStr` 冻结参数语义。

use std::sync::{Arc, Mutex};

use astersql_errors::{
    Error, ErrorArg, Find, GetStackTracer, HackedStr, HasStack, New, Normalize, RFCCodeText,
    RedactArgs, RedactErrorArg, RedactLogDisable, RedactLogEnable, RedactLogEnabled,
    RedactLogMarker,
};

/// 串行化脱敏相关测试，避免全局 `RedactLogEnabled` 状态互相干扰。
static REDACT_TEST_LOCK: Mutex<()> = Mutex::new(());

/// RAII：退出作用域时将脱敏开关恢复为 Disable。
struct RedactStateGuard;

impl Drop for RedactStateGuard {
    fn drop(&mut self) {
        RedactLogEnabled.Store(RedactLogDisable);
    }
}

/// 可变字节缓冲实现 `HackedStr`，用于验证生成后冻结快照不被后续修改影响。
#[derive(Clone)]
struct MutableHackedString(Arc<Mutex<Vec<u8>>>);

impl MutableHackedString {
    /// 以初始字符串内容构造可变 HackedStr。
    fn new(value: &str) -> Self {
        Self(Arc::new(Mutex::new(value.as_bytes().to_vec())))
    }

    /// 原地替换底层字节，模拟生成后外部数据变更。
    fn replace(&self, value: &str) {
        *self.0.lock().expect("mutable string lock") = value.as_bytes().to_vec();
    }
}

impl HackedStr for MutableHackedString {
    fn FreezeStr(&self) -> String {
        String::from_utf8(self.0.lock().expect("mutable string lock").clone())
            .expect("test data is utf-8")
    }
}

/// 从包装链中取出内层 Normalize `Error` 原型实例。
fn generated_error(error: &astersql_errors::SharedError) -> Error {
    Find(Some(error), |candidate| {
        candidate.downcast_ref::<Error>().is_some()
    })
    .and_then(|candidate| candidate.downcast_ref::<Error>().cloned())
    .expect("generated chain contains normalize Error")
}

/// 对照 Go：带栈/无栈生成、脱敏三态、带 cause 生成，以及 HackedStr 参数冻结。
#[test]
fn all_generation_and_redaction_paths_match_go() {
    let _lock = REDACT_TEST_LOCK.lock().expect("redaction test lock");
    let _reset = RedactStateGuard;
    // 原型：消息模板含两处 `%s`，并通过 RedactArgs 声明参数下标需脱敏。
    let prototype = Normalize(
        "entry %s for key %s",
        &[RFCCodeText("kv:Duplicate"), RedactArgs(&[0, 1])],
    );

    RedactLogEnabled.Store(RedactLogDisable);
    let stackful = prototype.GenWithStackByArgs(&["secret".into(), "PRIMARY".into()]);
    assert_eq!(
        stackful.to_string(),
        "[kv:Duplicate]entry secret for key PRIMARY"
    );
    assert!(HasStack(&stackful));
    let generated = generated_error(&stackful);
    assert!(
        generated
            .Location()
            .0
            .ends_with("normalize_generation_test.rs")
    );
    assert!(generated.Location().1 > 0);

    let custom = prototype.GenWithStack("custom %s", &["visible".into()]);
    assert_eq!(custom.to_string(), "[kv:Duplicate]custom visible");
    assert!(HasStack(&custom));

    let fast_custom = prototype.FastGen("fast %s", &["visible".into()]);
    assert_eq!(fast_custom.to_string(), "[kv:Duplicate]fast visible");
    assert!(!HasStack(&fast_custom));

    let fast_by_args = prototype.FastGenByArgs(&["secret".into(), "PRIMARY".into()]);
    assert_eq!(
        fast_by_args.to_string(),
        "[kv:Duplicate]entry secret for key PRIMARY"
    );
    assert!(!HasStack(&fast_by_args));

    // 开启脱敏：敏感参数替换为 `?`。
    RedactLogEnabled.Store(RedactLogEnable);
    let redacted = prototype.FastGenByArgs(&["secret".into(), "PRIMARY".into()]);
    assert_eq!(redacted.to_string(), "[kv:Duplicate]entry ? for key ?");

    // Marker 模式：用 ‹› 包裹；参数内已有标记需转义加倍。
    RedactLogEnabled.Store(RedactLogMarker);
    let marked = prototype.FastGenByArgs(&["se‹cret›".into(), "PRIMARY".into()]);
    assert_eq!(
        marked.to_string(),
        "[kv:Duplicate]entry ‹se‹‹cret››› for key ‹PRIMARY›"
    );
    let mut direct = vec![34_i32.into(), "plain".into()];
    RedactErrorArg(&mut direct, &[0]);
    assert_eq!(
        astersql_errors::Errorf("%d %s", &direct).to_string(),
        "‹34› plain"
    );

    RedactLogEnabled.Store(RedactLogDisable);
    let cause_prototype = prototype
        .Wrap(Some(New("cause %s")))
        .expect("cause is present");
    let fast_cause = cause_prototype.FastGenWithCause(&["value".into()]);
    assert_eq!(
        fast_cause.to_string(),
        "[kv:Duplicate]cause value: cause %s"
    );
    assert!(!HasStack(&fast_cause));
    let stack_cause = cause_prototype.GenWithStackByCause(&["value".into()]);
    assert_eq!(
        stack_cause.to_string(),
        "[kv:Duplicate]cause value: cause %s"
    );
    assert!(HasStack(&stack_cause));
    assert!(GetStackTracer(&stack_cause).is_some());

    // HackedStr：生成时 FreezeStr 快照；之后改底层缓冲不应改变已生成错误文本。
    let frozen_value = MutableHackedString::new("120120519090607");
    let frozen_arg = || ErrorArg::from_hacked(&frozen_value);
    let assert_frozen = |error: astersql_errors::SharedError, expected: &str| {
        frozen_value.replace("1 1:1:1.0000027");
        assert_eq!(generated_error(&error).GetMsg(), expected);
        frozen_value.replace("120120519090607");
    };
    assert_frozen(
        prototype.GenWithStack("frozen %s", &[frozen_arg()]),
        "frozen 120120519090607",
    );
    assert_frozen(
        prototype.GenWithStackByArgs(&[frozen_arg(), "key".into()]),
        "entry 120120519090607 for key key",
    );
    assert_frozen(
        prototype.FastGen("frozen %s", &[frozen_arg()]),
        "frozen 120120519090607",
    );
    assert_frozen(
        prototype.FastGenByArgs(&[frozen_arg(), "key".into()]),
        "entry 120120519090607 for key key",
    );
    assert_frozen(
        cause_prototype.FastGenWithCause(&[frozen_arg()]),
        "cause 120120519090607",
    );
    assert_frozen(
        cause_prototype.GenWithStackByCause(&[frozen_arg()]),
        "cause 120120519090607",
    );
}
