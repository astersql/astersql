// Copyright 2026 AsterSQL.

// `tidb_vars` / `SysVar` / `varsutil` 相关的 Aster 单元测试。
//
// 覆盖标量转换、类型校验与裁剪、作用域规则、注册表大小写不敏感、
// 会话/全局钩子别名同步、内存与 Schema 缓存解析，以及 SnapshotTS 与陈旧读互斥。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use astersql_sessionctx_variable::*;
use serial_test::serial;

#[derive(Default)]
/// 内存版 GlobalVarAccessor，记录全局 SET 调用。
struct MockAccessor {
    globals: HashMap<String, String>,
    tidb: HashMap<String, String>,
    global_sets: Vec<(String, String, bool)>,
}

impl GlobalVarAccessor for MockAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        self.globals
            .get(name)
            .cloned()
            .ok_or_else(|| VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &Context,
        name: &str,
        value: &str,
        update_local: bool,
    ) -> Result<(), VariableError> {
        self.global_sets
            .push((name.to_owned(), value.to_owned(), update_local));
        self.globals.insert(name.to_owned(), value.to_owned());
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        self.tidb
            .get(name)
            .cloned()
            .ok_or_else(|| VariableError::unknown(name))
    }

    fn set_tidb_table_value(
        &mut self,
        name: &str,
        value: &str,
        _comment: &str,
    ) -> Result<(), VariableError> {
        self.tidb.insert(name.to_owned(), value.to_owned());
        Ok(())
    }
}

/// 构造带空 MockAccessor 的会话变量。
fn session() -> SessionVars {
    SessionVars::new(Box::<MockAccessor>::default())
}

/// 构造带固定 Min/Max 的测试用 SysVar。
fn sysvar(kind: vardef::TypeFlag) -> SysVar {
    SysVar {
        Scope: vardef::ScopeGlobal | vardef::ScopeSession,
        Name: "aster_test_var".to_owned(),
        Value: "10".to_owned(),
        Type: kind,
        MinValue: 2,
        MaxValue: 7,
        ..SysVar::default()
    }
}

#[test]
/// 标量/开关转换辅助函数与 Go 行为一致。
fn scalar_helpers_match_go_conversions() {
    assert_eq!(BoolToOnOff(true), "ON");
    assert_eq!(BoolToOnOff(false), "OFF");
    assert!(TiDBOptOn("oN"));
    assert!(TiDBOptOn("1"));
    assert!(!TiDBOptOn("1.1"));
    assert_eq!(TiDBOptOnOffWarn("WARN"), WarnInt);
    assert_eq!(OnOffToTrueFalse("oFf"), "false");
    assert_eq!(trueFalseToOnOff("TrUe"), "ON");
    assert_eq!(TidbOptInt("bad", 12), 12);
    assert_eq!(TidbOptInt64("-4", 12), -4);
    assert_eq!(TidbOptUint64("-4", 12), 12);
}

#[test]
/// 各 Type 的 Validate 裁剪与归一化对齐 Go。
fn automatic_validation_matches_go_clamping_and_normalization() {
    let mut vars = session();

    let mut unsigned = sysvar(vardef::TypeUnsigned);
    unsigned.AllowAutoValue = true;
    assert_eq!(
        unsigned
            .Validate(&mut vars, "-301", vardef::ScopeSession)
            .unwrap(),
        "2"
    );
    assert_eq!(
        unsigned
            .Validate(&mut vars, "-1", vardef::ScopeSession)
            .unwrap(),
        "-1"
    );
    assert!(
        unsigned
            .Validate(&mut vars, "-ERR", vardef::ScopeSession)
            .is_err()
    );

    let signed = sysvar(vardef::TypeInt);
    assert_eq!(
        signed
            .Validate(&mut vars, "99", vardef::ScopeSession)
            .unwrap(),
        "7"
    );

    let boolean = SysVar {
        Type: vardef::TypeBool,
        AutoConvertNegativeBool: true,
        ..sysvar(vardef::TypeBool)
    };
    assert_eq!(
        boolean
            .Validate(&mut vars, "-2", vardef::ScopeSession)
            .unwrap(),
        "ON"
    );
    assert!(
        boolean
            .Validate(&mut vars, "1.0", vardef::ScopeSession)
            .is_err()
    );

    let enumeration = SysVar {
        Type: vardef::TypeEnum,
        PossibleValues: vec!["OFF".into(), "ON".into(), "AUTO".into()],
        ..sysvar(vardef::TypeEnum)
    };
    assert_eq!(
        enumeration
            .Validate(&mut vars, "auto", vardef::ScopeSession)
            .unwrap(),
        "AUTO"
    );
    assert_eq!(
        enumeration
            .Validate(&mut vars, "2", vardef::ScopeSession)
            .unwrap(),
        "AUTO"
    );

    let floating = sysvar(vardef::TypeFloat);
    assert_eq!(
        floating
            .Validate(&mut vars, "1.1", vardef::ScopeSession)
            .unwrap(),
        "2"
    );
    assert_eq!(
        floating
            .Validate(&mut vars, "22", vardef::ScopeSession)
            .unwrap(),
        "7"
    );
    assert_eq!(vars.StmtCtx.warnings().len(), 4);
}

#[test]
/// Duration/Time 校验及宽松校验不污染既有警告。
fn duration_time_and_relaxed_validation_preserve_go_behavior() {
    let mut vars = session();
    let duration = SysVar {
        Type: vardef::TypeDuration,
        MinValue: 1_000_000_000,
        MaxValue: 3_600_000_000_000,
        ..sysvar(vardef::TypeDuration)
    };
    assert_eq!(
        duration
            .Validate(&mut vars, "1ms", vardef::ScopeSession)
            .unwrap(),
        "1s"
    );
    assert_eq!(
        duration
            .Validate(&mut vars, "2h10m", vardef::ScopeSession)
            .unwrap(),
        "1h0m0s"
    );
    assert!(
        duration
            .Validate(&mut vars, "1hr", vardef::ScopeSession)
            .is_err()
    );

    let time = SysVar {
        Type: vardef::TypeTime,
        ..sysvar(vardef::TypeTime)
    };
    assert_eq!(
        time.Validate(&mut vars, "3:00 +0000", vardef::ScopeSession)
            .unwrap(),
        "03:00 +0000"
    );

    vars.StmtCtx
        .append_warning(VariableError::wrong_value("before", "x"));
    let before = vars.StmtCtx.warnings().to_vec();
    assert_eq!(
        duration.ValidateWithRelaxedValidation(&mut vars, "bad", vardef::ScopeSession),
        "bad"
    );
    assert_eq!(vars.StmtCtx.warnings(), before.as_slice());
}

#[test]
/// 作用域拒绝、原生类型映射与 SkipInit/SkipSysvarCache。
fn scope_native_value_and_skip_rules_match_go() {
    let mut vars = session();
    let read_only = SysVar {
        Name: "read_only".into(),
        Scope: vardef::ScopeNone,
        ReadOnly: true,
        ..SysVar::default()
    };
    assert_eq!(
        read_only
            .Validate(&mut vars, "x", vardef::ScopeSession)
            .unwrap_err()
            .kind(),
        VariableErrorKind::IncorrectScope
    );

    let internal = SysVar {
        Scope: vardef::ScopeSession,
        InternalSessionVariable: true,
        ..sysvar(vardef::TypeBool)
    };
    assert_eq!(
        internal
            .Validate(&mut vars, "ON", vardef::ScopeSession)
            .unwrap_err()
            .kind(),
        VariableErrorKind::UnknownSystemVariable
    );

    let unsigned = sysvar(vardef::TypeUnsigned);
    assert_eq!(
        unsigned.GetNativeValType("1234"),
        (
            Datum::Uint(1234),
            MYSQL_TYPE_LONGLONG,
            UNSIGNED_FLAG | BINARY_FLAG
        )
    );
    let boolean = sysvar(vardef::TypeBool);
    assert_eq!(
        boolean.GetNativeValType("bogus"),
        (Datum::Int(0), MYSQL_TYPE_LONGLONG, BINARY_FLAG)
    );

    let gc = SysVar {
        Name: vardef::TiDBGCEnable.into(),
        Scope: vardef::ScopeGlobal,
        ..SysVar::default()
    };
    assert!(gc.SkipInit());
    assert!(gc.SkipSysvarCache());
}

#[test]
#[serial]
/// 注册表大小写不敏感、GetSysVars 拷贝语义与依赖排序。
fn registry_is_case_insensitive_copies_values_and_orders_dependencies() {
    for name in ["Depended", "plain"] {
        UnregisterSysVar(name);
    }
    RegisterSysVar(SysVar {
        Name: "Depended".into(),
        Depended: true,
        Value: "old".into(),
        ..SysVar::default()
    });
    RegisterSysVar(SysVar {
        Name: "plain".into(),
        ..SysVar::default()
    });
    assert_eq!(GetSysVar("dEpEnDeD").unwrap().Value, "old");
    SetSysVar("DEPENDED", "new").unwrap();
    assert_eq!(GetSysVar("depended").unwrap().Value, "new");

    let copied = GetSysVars();
    SetSysVar("depended", "newer").unwrap();
    assert_eq!(copied["depended"].Value, "new");

    let names = HashMap::from([
        ("plain".to_owned(), String::new()),
        ("depended".to_owned(), String::new()),
        ("unknown".to_owned(), String::new()),
    ]);
    let ordered = OrderByDependency(&names);
    assert_eq!(ordered.first().unwrap(), "depended");
    UnregisterSysVar("DePeNdEd");
    assert!(GetSysVar("depended").is_none());
}

#[test]
#[serial]
/// 会话/全局 SET 钩子同步别名且不递归死循环。
fn session_and_global_hooks_update_aliases_without_recursing() {
    for name in ["primary", "alias"] {
        UnregisterSysVar(name);
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let alias_calls = Arc::clone(&calls);
    RegisterSysVar(SysVar {
        Name: "alias".into(),
        SetSession: Some(Arc::new(move |_s, value| {
            alias_calls.lock().unwrap().push(value.to_owned());
            Ok(())
        })),
        ..SysVar::default()
    });
    let primary = SysVar {
        Name: "primary".into(),
        Aliases: vec!["alias".into()],
        ..SysVar::default()
    };
    let mut vars = session();
    primary.SetSessionFromHook(&mut vars, "ON").unwrap();
    assert_eq!(vars.system("primary"), Some("ON"));
    assert_eq!(vars.system("alias"), Some("ON"));
    assert_eq!(&*calls.lock().unwrap(), &["ON"]);

    primary
        .SetGlobalFromHook(&Context::default(), &mut vars, "OFF", false)
        .unwrap();
    assert_eq!(vars.global("alias"), Some("OFF".to_owned()));
}

#[test]
/// 内存限额与 Schema 缓存大小解析归一化。
fn memory_and_schema_size_parsers_match_go_normalization() {
    let mut vars = session();
    vars.memory_total = 10 * 1024 * 1024 * 1024;
    assert_eq!(parsePercentage("10%"), (10, "10%".into()));
    assert_eq!(parsePercentage("0%"), (0, String::new()));
    assert_eq!(parseByteSize("2GiB"), (2_u64 << 30, "2GiB".into()));
    assert_eq!(parseByteSize("1mb"), (0, String::new()));
    assert_eq!(
        parseMemoryLimit(&mut vars, "5%", "5%").unwrap(),
        (512_u64 << 20, "5%".into())
    );
    assert_eq!(
        parseSchemaCacheSize(&mut vars, "1MB", "1MB").unwrap(),
        (64_u64 << 20, "64MB".into())
    );
    assert_eq!(
        parseSchemaCacheSize(&mut vars, "18446744073709551615", "huge").unwrap(),
        (i64::MAX as u64, i64::MAX.to_string())
    );
}

#[test]
/// analyze-skip 类型与表达式索引函数列表完整性。
fn analyze_skip_types_and_expression_functions_are_complete() {
    assert_eq!(
        ValidAnalyzeSkipColumnTypes(" JSON, Text ,blob ").unwrap(),
        "json,text,blob"
    );
    assert!(ValidAnalyzeSkipColumnTypes("json,int").is_err());
    let parsed = ParseAnalyzeSkipColumnTypes("json,JSON, text");
    assert!(parsed.contains("json"));
    assert!(!parsed.contains(" text"));

    let names = collectAllowFuncName4ExpressionIndex();
    assert!(names.contains("json_schema_valid"));
    assert!(names.contains("tidb_shard"));
    assert!(names.split(", ").is_sorted());
}

#[test]
/// SnapshotTS / TxnReadTS / ReadStaleness 互斥约束。
fn snapshot_read_ts_and_staleness_keep_mutual_exclusion() {
    let mut vars = session();
    setSnapshotTS(&mut vars, "12345").unwrap();
    assert_eq!(vars.SnapshotTS, 12345);
    assert_eq!(vars.TxnReadTS, 0);
    assert!(setReadStaleness(&mut vars, "-10").is_err());

    setSnapshotTS(&mut vars, "").unwrap();
    setReadStaleness(&mut vars, "-10").unwrap();
    assert_eq!(vars.ReadStaleness, -10_000_000_000);
    assert!(setSnapshotTS(&mut vars, "12345").is_err());

    setReadStaleness(&mut vars, "0").unwrap();
    setTxnReadTS(&mut vars, "2024-01-02 03:04:05").unwrap();
    assert_eq!(vars.SnapshotTS, 0);
    assert!(vars.TxnReadTS > 0);
}

#[test]
/// 未注入钩子时安全；注入后可调用。
fn optional_instance_hooks_are_safe_and_injectable() {
    clear_instance_hooks_for_test();
    assert!(switchDDL(true).is_ok());
    assert!(switchStats(false).is_ok());

    let calls = Arc::new(Mutex::new(Vec::new()));
    let ddl_calls = Arc::clone(&calls);
    set_enable_ddl_hook(Some(Arc::new(move || {
        ddl_calls.lock().unwrap().push("ddl");
        Ok(())
    })));
    let stats_calls = Arc::clone(&calls);
    set_disable_stats_owner_hook(Some(Arc::new(move || {
        stats_calls.lock().unwrap().push("stats");
        Ok(())
    })));
    switchDDL(true).unwrap();
    switchStats(false).unwrap();
    assert_eq!(&*calls.lock().unwrap(), &["ddl", "stats"]);
}
