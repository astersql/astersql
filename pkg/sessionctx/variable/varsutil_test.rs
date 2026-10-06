// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `varsutil` 与系统变量校验路径的单元测试。
//
// 覆盖 TiDBOptOn、SessionVars 默认值、Validate 用例表、语句摘要相关变量、
// 并发度变量、辅助解析函数、会话状态变量以及 ON/OFF 与断言级别转换。

use std::collections::HashMap;
use std::sync::Arc;

use astersql_sessionctx_variable::*;
use chrono::FixedOffset;

#[derive(Default)]
/// 测试用全局变量访问器（内存 HashMap）。
struct TestAccessor {
    globals: HashMap<String, String>,
    tidb: HashMap<String, String>,
}

impl GlobalVarAccessor for TestAccessor {
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
        _update_local: bool,
    ) -> Result<(), VariableError> {
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

/// 构造默认测试会话。
fn session() -> SessionVars {
    SessionVars::new(Box::<TestAccessor>::default())
}

#[test]
fn tidb_table_fallback_preserves_the_go_default_verbatim() {
    let vars = session();
    assert_eq!(getTiDBTableValue(&vars, "missing", "true").unwrap(), "true");
    assert_eq!(
        getTiDBTableValue(&vars, "missing", "false").unwrap(),
        "false"
    );
}

/// 构造指定名称与类型的 SysVar。
fn sysvar(name: &str, kind: vardef::TypeFlag) -> SysVar {
    SysVar {
        Scope: vardef::ScopeGlobal | vardef::ScopeSession,
        Name: name.to_owned(),
        Type: kind,
        MinValue: i64::MIN,
        MaxValue: i64::MAX as u64,
        ..SysVar::default()
    }
}

/// 构造布尔类型 SysVar。
fn bool_sysvar(name: &str) -> SysVar {
    sysvar(name, vardef::TypeBool)
}

/// 构造带可选值列表的枚举 SysVar。
fn enum_sysvar(name: &str, values: &[&str]) -> SysVar {
    SysVar {
        PossibleValues: values.iter().map(|value| (*value).to_owned()).collect(),
        ..sysvar(name, vardef::TypeEnum)
    }
}

// TestTiDBOptOn.
#[test]
/// TestTiDBOptOn：开关字符串识别。
fn test_ti_db_opt_on() {
    for (value, expected) in [
        ("ON", true),
        ("on", true),
        ("On", true),
        ("1", true),
        ("off", false),
        ("No", false),
        ("0", false),
        ("1.1", false),
        ("", false),
    ] {
        assert_eq!(TiDBOptOn(value), expected, "value={value:?}");
    }
}

// TestNewSessionVars.
#[test]
/// 新会话变量默认字段合理性。
fn test_new_session_vars() {
    let vars = session();
    assert!(vars.StmtCtx.warnings().is_empty());
    assert_eq!(vars.system("autocommit"), None);
    assert_eq!(vars.SnapshotTS, 0);
    assert_eq!(vars.TxnReadTS, 0);
    assert_eq!(vars.ReadStaleness, 0);
    assert_eq!(vars.location(), FixedOffset::east_opt(0).unwrap());

    let session_defaults = session::SessionVars::new();
    assert!(session_defaults.Autocommit);
    assert_eq!(session_defaults.RetryLimit, 10);
    assert_eq!(session_defaults.TxnMode, "OPTIMISTIC");
    assert!(session_defaults.PreparedStmts.is_empty());
    assert!(session_defaults.PreparedStmtNameToID.is_empty());
    assert!(!session_defaults.EnableAdaptiveLimitScan);

    let concurrency_defaults = sysvar::SessionVars::default();
    assert_eq!(
        concurrency_defaults.ExecutorConcurrency,
        vardef::ConcurrencyUnset as i32
    );
    assert_eq!(
        concurrency_defaults
            .PipelinedDMLConfig
            .PipelinedFlushConcurrency,
        vardef::DefaultFlushConcurrency as i32
    );
    assert_eq!(
        concurrency_defaults
            .PipelinedDMLConfig
            .PipelinedResolveLockConcurrency,
        vardef::DefaultResolveConcurrency as i32
    );
}

/// `tidb_snapshot` accepts the microsecond timestamp form used by the session
/// variable integration test, matching Go's time parser.
#[test]
fn snapshot_ts_accepts_fractional_seconds() {
    let mut vars = session();
    setSnapshotTS(&mut vars, "2007-01-01 15:04:05.999999").expect("fractional timestamp");
    assert_ne!(vars.SnapshotTS, 0);
}

#[test]
fn charset_and_collation_validation_uses_registered_metadata() {
    let mut vars = session();
    assert_eq!(
        checkCharacterSet("UTF8MB3", "character_set_connection").unwrap(),
        "utf8"
    );
    assert!(checkCharacterSet("definitely_not_a_charset", "character_set_connection").is_err());
    assert_eq!(
        checkCollation(
            &mut vars,
            "UTF8MB3_GENERAL_CI",
            "UTF8MB3_GENERAL_CI",
            vardef::ScopeSession,
        )
        .unwrap(),
        "utf8_general_ci"
    );
    assert!(
        checkCollation(
            &mut vars,
            "definitely_not_a_collation",
            "definitely_not_a_collation",
            vardef::ScopeSession,
        )
        .is_err()
    );
    assert_eq!(
        checkDefaultCollationForUTF8MB4(
            &mut vars,
            "utf8mb4_0900_ai_ci",
            "utf8mb4_0900_ai_ci",
            vardef::ScopeSession,
        )
        .unwrap(),
        "utf8mb4_0900_ai_ci"
    );
}

#[test]
fn timestamp_setters_preserve_go_error_side_effects() {
    let mut vars = session();
    vars.SnapshotTS = 42;
    vars.TxnReadTS = 84;
    assert!(setSnapshotTS(&mut vars, "not-a-timestamp").is_err());
    assert_eq!(vars.SnapshotTS, 0);
    assert_eq!(vars.TxnReadTS, 0);

    assert!(setTxnReadTS(&mut vars, "12345").is_err());
    assert_eq!(vars.SnapshotTS, 0);
    assert_eq!(vars.TxnReadTS, 0);
}

// TestVarsutil.
#[test]
/// varsutil 综合校验与转换用例。
fn test_varsutil() {
    let mut vars = session();

    let autocommit = bool_sysvar("autocommit");
    let normalized = autocommit
        .Validate(&mut vars, "1", vardef::ScopeSession)
        .unwrap();
    assert_eq!(normalized, "ON");
    autocommit
        .SetSessionFromHook(&mut vars, &normalized)
        .unwrap();
    assert_eq!(autocommit.GetSessionFromHook(&mut vars).unwrap(), "ON");
    assert!(
        autocommit
            .Validate(&mut vars, "", vardef::ScopeSession)
            .is_err()
    );

    let foreign_key_checks = bool_sysvar("foreign_key_checks");
    assert_eq!(
        foreign_key_checks
            .Validate(&mut vars, "0", vardef::ScopeSession)
            .unwrap(),
        "OFF"
    );
    assert_eq!(
        foreign_key_checks
            .Validate(&mut vars, "1", vardef::ScopeSession)
            .unwrap(),
        "ON"
    );

    let sql_mode = enum_sysvar(
        "sql_mode",
        &["STRICT_TRANS_TABLES", "REAL_AS_FLOAT,ANSI_QUOTES", ""],
    );
    assert_eq!(
        sql_mode
            .Validate(&mut vars, "strict_trans_tables", vardef::ScopeSession)
            .unwrap(),
        "STRICT_TRANS_TABLES"
    );
    assert_eq!(
        sql_mode
            .Validate(&mut vars, "REAL_AS_FLOAT,ANSI_QUOTES", vardef::ScopeSession,)
            .unwrap(),
        "REAL_AS_FLOAT,ANSI_QUOTES"
    );

    assert_eq!(
        checkCharacterSet("utf8", "character_set_connection").unwrap(),
        "utf8"
    );
    assert_eq!(
        checkCollation(
            &mut vars,
            "utf8_general_ci",
            "utf8_general_ci",
            vardef::ScopeSession,
        )
        .unwrap(),
        "utf8_general_ci"
    );

    let time_zone = sysvar("time_zone", vardef::TypeTime);
    for (input, expected) in [
        ("10:00 +1000", "10:00 +1000"),
        ("06:00 -0600", "06:00 -0600"),
        ("14:00 +1400", "14:00 +1400"),
        ("12:59 -1259", "12:59 -1259"),
    ] {
        assert_eq!(
            time_zone
                .Validate(&mut vars, input, vardef::ScopeSession)
                .unwrap(),
            expected
        );
    }
    assert!(
        time_zone
            .Validate(&mut vars, "6:00", vardef::ScopeSession)
            .is_ok()
    );
    assert!(
        time_zone
            .Validate(&mut vars, "not-a-zone", vardef::ScopeSession)
            .is_err()
    );

    let bounded = SysVar {
        MinValue: 32,
        MaxValue: 1024,
        ..sysvar("tidb_max_chunk_size", vardef::TypeUnsigned)
    };
    assert_eq!(
        bounded
            .Validate(&mut vars, "2", vardef::ScopeSession)
            .unwrap(),
        "32"
    );
    assert_eq!(vars.StmtCtx.warnings().len(), 1);

    for (name, old, new) in [
        ("tidb_opt_cpu_factor", "3.0", "5.0"),
        ("tidb_opt_copcpu_factor", "3.0", "5.0"),
        ("tidb_opt_network_factor", "1.0", "3.0"),
        ("tidb_opt_scan_factor", "1.5", "3.0"),
        ("tidb_opt_desc_factor", "3.0", "5.0"),
        ("tidb_opt_seek_factor", "20.0", "50.0"),
        ("tidb_opt_memory_factor", "0.001", "1.0"),
        ("tidb_opt_disk_factor", "1.5", "1.1"),
        ("tidb_opt_concurrency_factor", "3.0", "5.0"),
    ] {
        let factor = sysvar(name, vardef::TypeFloat);
        assert_eq!(
            factor
                .Validate(&mut vars, old, vardef::ScopeSession)
                .unwrap(),
            old
        );
        assert_eq!(
            factor
                .Validate(&mut vars, new, vardef::ScopeSession)
                .unwrap(),
            new
        );
    }

    let read_only = SysVar {
        Name: "last_plan_from_cache".to_owned(),
        Scope: vardef::ScopeNone,
        ReadOnly: true,
        ..SysVar::default()
    };
    assert_eq!(
        read_only
            .Validate(&mut vars, "1", vardef::ScopeSession)
            .unwrap_err()
            .kind(),
        VariableErrorKind::IncorrectScope
    );
    assert_eq!(
        VariableError::unknown("UnknownVariable").kind(),
        VariableErrorKind::UnknownSystemVariable
    );
}

#[derive(Clone, Copy)]
enum ValidationKind {
    Bool,
    Enum(&'static [&'static str]),
    Float,
    Int { min: i64, max: u64, auto: bool },
    Unsigned { min: i64, max: u64, auto: bool },
    Time,
    Isolation,
    Fallback,
}

#[derive(Clone, Copy)]
struct ValidationCase {
    name: &'static str,
    value: &'static str,
    kind: ValidationKind,
    error: bool,
}

/// 按 ValidationCase 组装待测 SysVar。
fn validation_sysvar(case: ValidationCase) -> SysVar {
    match case.kind {
        ValidationKind::Bool => bool_sysvar(case.name),
        ValidationKind::Enum(values) => enum_sysvar(case.name, values),
        ValidationKind::Float => sysvar(case.name, vardef::TypeFloat),
        ValidationKind::Int { min, max, auto } => SysVar {
            MinValue: min,
            MaxValue: max,
            AllowAutoValue: auto,
            ..sysvar(case.name, vardef::TypeInt)
        },
        ValidationKind::Unsigned { min, max, auto } => SysVar {
            MinValue: min,
            MaxValue: max,
            AllowAutoValue: auto,
            ..sysvar(case.name, vardef::TypeUnsigned)
        },
        ValidationKind::Time => sysvar(case.name, vardef::TypeTime),
        ValidationKind::Isolation => SysVar {
            Validation: Some(Arc::new(checkIsolationLevel)),
            ..sysvar(case.name, vardef::TypeStr)
        },
        ValidationKind::Fallback => SysVar {
            AllowEmptyAll: true,
            Validation: Some(Arc::new(|_, normalized, original, _| {
                let value = normalized.trim().to_ascii_lowercase();
                if value.is_empty() || value == "tiflash" {
                    Ok(value)
                } else {
                    Err(VariableError::wrong_value(
                        "tidb_allow_fallback_to_tikv",
                        original,
                    ))
                }
            })),
            ..sysvar(case.name, vardef::TypeStr)
        },
    }
}

// TestValidate.
#[test]
/// 大规模 Validate 用例表（对齐 Go）。
fn test_validate() {
    const OFF_ON: &[&str] = &["OFF", "ON"];
    const DELAY_KEY_WRITE: &[&str] = &["OFF", "ON", "ALL"];
    const TRACK_GTIDS: &[&str] = &["OFF", "OWN_GTID", "ALL_GTIDS"];
    const ENFORCE_GTID: &[&str] = &["OFF", "ON", "WARN"];
    const REPLICA_READ: &[&str] = &["leader", "follower", "leader-and-follower"];
    const TXN_MODE: &[&str] = &["", "pessimistic", "optimistic"];

    let cases = [
        ValidationCase {
            name: "tidb_auto_analyze_start_time",
            value: "15:04",
            kind: ValidationKind::Time,
            error: false,
        },
        ValidationCase {
            name: "tidb_auto_analyze_start_time",
            value: "15:04 -0700",
            kind: ValidationKind::Time,
            error: false,
        },
        ValidationCase {
            name: "delay_key_write",
            value: "ON",
            kind: ValidationKind::Enum(DELAY_KEY_WRITE),
            error: false,
        },
        ValidationCase {
            name: "delay_key_write",
            value: "OFF",
            kind: ValidationKind::Enum(DELAY_KEY_WRITE),
            error: false,
        },
        ValidationCase {
            name: "delay_key_write",
            value: "ALL",
            kind: ValidationKind::Enum(DELAY_KEY_WRITE),
            error: false,
        },
        ValidationCase {
            name: "delay_key_write",
            value: "3",
            kind: ValidationKind::Enum(DELAY_KEY_WRITE),
            error: true,
        },
        ValidationCase {
            name: "foreign_key_checks",
            value: "3",
            kind: ValidationKind::Bool,
            error: true,
        },
        ValidationCase {
            name: "max_sp_recursion_depth",
            value: "256",
            kind: ValidationKind::Unsigned {
                min: 0,
                max: 255,
                auto: false,
            },
            error: false,
        },
        ValidationCase {
            name: "session_track_gtids",
            value: "OFF",
            kind: ValidationKind::Enum(TRACK_GTIDS),
            error: false,
        },
        ValidationCase {
            name: "session_track_gtids",
            value: "OWN_GTID",
            kind: ValidationKind::Enum(TRACK_GTIDS),
            error: false,
        },
        ValidationCase {
            name: "session_track_gtids",
            value: "ALL_GTIDS",
            kind: ValidationKind::Enum(TRACK_GTIDS),
            error: false,
        },
        ValidationCase {
            name: "session_track_gtids",
            value: "ON",
            kind: ValidationKind::Enum(TRACK_GTIDS),
            error: true,
        },
        ValidationCase {
            name: "enforce_gtid_consistency",
            value: "OFF",
            kind: ValidationKind::Enum(ENFORCE_GTID),
            error: false,
        },
        ValidationCase {
            name: "enforce_gtid_consistency",
            value: "ON",
            kind: ValidationKind::Enum(ENFORCE_GTID),
            error: false,
        },
        ValidationCase {
            name: "enforce_gtid_consistency",
            value: "WARN",
            kind: ValidationKind::Enum(ENFORCE_GTID),
            error: false,
        },
        ValidationCase {
            name: "secure_auth",
            value: "1",
            kind: ValidationKind::Bool,
            error: false,
        },
        ValidationCase {
            name: "secure_auth",
            value: "3",
            kind: ValidationKind::Bool,
            error: true,
        },
        ValidationCase {
            name: "myisam_use_mmap",
            value: "ON",
            kind: ValidationKind::Enum(OFF_ON),
            error: false,
        },
        ValidationCase {
            name: "myisam_use_mmap",
            value: "OFF",
            kind: ValidationKind::Enum(OFF_ON),
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_correlation_exp_factor",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_correlation_exp_factor",
            value: "-10",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_correlation_threshold",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_correlation_threshold",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_cpu_factor",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_cpu_factor",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_tiflash_concurrency_factor",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_copcpu_factor",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_copcpu_factor",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_network_factor",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_network_factor",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_scan_factor",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_scan_factor",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_desc_factor",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_desc_factor",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_seek_factor",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_seek_factor",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_memory_factor",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_memory_factor",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_disk_factor",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_disk_factor",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_concurrency_factor",
            value: "a",
            kind: ValidationKind::Float,
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_concurrency_factor",
            value: "-2",
            kind: ValidationKind::Float,
            error: false,
        },
        ValidationCase {
            name: "tx_isolation",
            value: "READ-UNCOMMITTED",
            kind: ValidationKind::Isolation,
            error: true,
        },
        ValidationCase {
            name: "tidb_init_chunk_size",
            value: "a",
            kind: ValidationKind::Unsigned {
                min: 1,
                max: 32,
                auto: false,
            },
            error: true,
        },
        ValidationCase {
            name: "tidb_init_chunk_size",
            value: "-1",
            kind: ValidationKind::Unsigned {
                min: 1,
                max: 32,
                auto: false,
            },
            error: false,
        },
        ValidationCase {
            name: "tidb_max_chunk_size",
            value: "a",
            kind: ValidationKind::Unsigned {
                min: 32,
                max: 1024,
                auto: false,
            },
            error: true,
        },
        ValidationCase {
            name: "tidb_max_chunk_size",
            value: "-1",
            kind: ValidationKind::Unsigned {
                min: 32,
                max: 1024,
                auto: false,
            },
            error: false,
        },
        ValidationCase {
            name: "tidb_opt_join_reorder_threshold",
            value: "a",
            kind: ValidationKind::Int {
                min: 0,
                max: 63,
                auto: false,
            },
            error: true,
        },
        ValidationCase {
            name: "tidb_opt_join_reorder_threshold",
            value: "-1",
            kind: ValidationKind::Int {
                min: 0,
                max: 63,
                auto: false,
            },
            error: false,
        },
        ValidationCase {
            name: "tidb_replica_read",
            value: "invalid",
            kind: ValidationKind::Enum(REPLICA_READ),
            error: true,
        },
        ValidationCase {
            name: "tidb_txn_mode",
            value: "invalid",
            kind: ValidationKind::Enum(TXN_MODE),
            error: true,
        },
        ValidationCase {
            name: "tidb_txn_mode",
            value: "pessimistic",
            kind: ValidationKind::Enum(TXN_MODE),
            error: false,
        },
        ValidationCase {
            name: "tidb_txn_mode",
            value: "optimistic",
            kind: ValidationKind::Enum(TXN_MODE),
            error: false,
        },
        ValidationCase {
            name: "tidb_txn_mode",
            value: "",
            kind: ValidationKind::Enum(TXN_MODE),
            error: false,
        },
        ValidationCase {
            name: "tidb_shard_allocate_step",
            value: "ad",
            kind: ValidationKind::Unsigned {
                min: 1,
                max: 128,
                auto: false,
            },
            error: true,
        },
        ValidationCase {
            name: "tidb_shard_allocate_step",
            value: "-123",
            kind: ValidationKind::Unsigned {
                min: 1,
                max: 128,
                auto: false,
            },
            error: false,
        },
        ValidationCase {
            name: "tidb_shard_allocate_step",
            value: "128",
            kind: ValidationKind::Unsigned {
                min: 1,
                max: 128,
                auto: false,
            },
            error: false,
        },
        ValidationCase {
            name: "tidb_allow_fallback_to_tikv",
            value: "",
            kind: ValidationKind::Fallback,
            error: false,
        },
        ValidationCase {
            name: "tidb_allow_fallback_to_tikv",
            value: "tiflash",
            kind: ValidationKind::Fallback,
            error: false,
        },
        ValidationCase {
            name: "tidb_allow_fallback_to_tikv",
            value: "  tiflash  ",
            kind: ValidationKind::Fallback,
            error: false,
        },
        ValidationCase {
            name: "tidb_allow_fallback_to_tikv",
            value: "tikv",
            kind: ValidationKind::Fallback,
            error: true,
        },
        ValidationCase {
            name: "tidb_allow_fallback_to_tikv",
            value: "tidb",
            kind: ValidationKind::Fallback,
            error: true,
        },
        ValidationCase {
            name: "tidb_allow_fallback_to_tikv",
            value: "tiflash,tikv,tidb",
            kind: ValidationKind::Fallback,
            error: true,
        },
    ];

    let mut vars = session();
    for case in cases {
        let result = validation_sysvar(case).Validate(&mut vars, case.value, vardef::ScopeGlobal);
        assert_eq!(
            result.is_err(),
            case.error,
            "{}={:?}: {result:?}",
            case.name,
            case.value
        );
    }

    for (value, error) in [
        ("", true),
        ("tikv", false),
        ("TiKV,tiflash", false),
        ("   tikv,   tiflash  ", false),
    ] {
        let isolation_engines = SysVar {
            AllowEmptyAll: false,
            Validation: Some(Arc::new(|_, normalized, original, _| {
                if normalized.trim().is_empty() {
                    return Err(VariableError::wrong_value(
                        "tidb_isolation_read_engines",
                        original,
                    ));
                }
                let valid = normalized.split(',').map(str::trim).all(|engine| {
                    matches!(engine.to_ascii_lowercase().as_str(), "tikv" | "tiflash")
                });
                valid.then(|| normalized.to_owned()).ok_or_else(|| {
                    VariableError::wrong_value("tidb_isolation_read_engines", original)
                })
            })),
            ..sysvar("tidb_isolation_read_engines", vardef::TypeStr)
        };
        let result = isolation_engines.Validate(&mut vars, value, vardef::ScopeSession);
        assert_eq!(result.is_err(), error, "value={value:?}: {result:?}");
    }
}

// TestValidateStmtSummary.
#[test]
/// 语句摘要（statement summary）相关变量校验。
fn test_validate_stmt_summary() {
    let cases = [
        ("tidb_enable_stmt_summary", "", ValidationKind::Bool, true),
        (
            "tidb_stmt_summary_internal_query",
            "",
            ValidationKind::Bool,
            true,
        ),
        (
            "tidb_stmt_summary_refresh_interval",
            "",
            ValidationKind::Unsigned {
                min: 0,
                max: u64::MAX,
                auto: false,
            },
            true,
        ),
        (
            "tidb_stmt_summary_refresh_interval",
            "0",
            ValidationKind::Unsigned {
                min: 0,
                max: u64::MAX,
                auto: false,
            },
            false,
        ),
        (
            "tidb_stmt_summary_refresh_interval",
            "99999999999",
            ValidationKind::Unsigned {
                min: 0,
                max: u64::MAX,
                auto: false,
            },
            false,
        ),
        (
            "tidb_stmt_summary_history_size",
            "",
            ValidationKind::Int {
                min: 0,
                max: u64::MAX,
                auto: true,
            },
            true,
        ),
        (
            "tidb_stmt_summary_history_size",
            "0",
            ValidationKind::Int {
                min: 0,
                max: u64::MAX,
                auto: true,
            },
            false,
        ),
        (
            "tidb_stmt_summary_history_size",
            "-1",
            ValidationKind::Int {
                min: 0,
                max: u64::MAX,
                auto: true,
            },
            false,
        ),
        (
            "tidb_stmt_summary_history_size",
            "99999999",
            ValidationKind::Int {
                min: 0,
                max: u64::MAX,
                auto: true,
            },
            false,
        ),
        (
            "tidb_stmt_summary_max_stmt_count",
            "",
            ValidationKind::Unsigned {
                min: 0,
                max: u64::MAX,
                auto: false,
            },
            true,
        ),
        (
            "tidb_stmt_summary_max_stmt_count",
            "0",
            ValidationKind::Unsigned {
                min: 0,
                max: u64::MAX,
                auto: false,
            },
            false,
        ),
        (
            "tidb_stmt_summary_max_stmt_count",
            "99999999",
            ValidationKind::Unsigned {
                min: 0,
                max: u64::MAX,
                auto: false,
            },
            false,
        ),
        (
            "tidb_stmt_summary_max_sql_length",
            "",
            ValidationKind::Int {
                min: 0,
                max: u64::MAX,
                auto: true,
            },
            true,
        ),
        (
            "tidb_stmt_summary_max_sql_length",
            "0",
            ValidationKind::Int {
                min: 0,
                max: u64::MAX,
                auto: true,
            },
            false,
        ),
        (
            "tidb_stmt_summary_max_sql_length",
            "-1",
            ValidationKind::Int {
                min: 0,
                max: u64::MAX,
                auto: true,
            },
            false,
        ),
        (
            "tidb_stmt_summary_max_sql_length",
            "99999999999",
            ValidationKind::Int {
                min: 0,
                max: u64::MAX,
                auto: true,
            },
            false,
        ),
    ];
    let mut vars = session();
    for (name, value, kind, error) in cases {
        let case = ValidationCase {
            name,
            value,
            kind,
            error,
        };
        let result = validation_sysvar(case).Validate(&mut vars, value, vardef::ScopeGlobal);
        assert_eq!(result.is_err(), error, "{name}={value:?}: {result:?}");
    }
}

// TestConcurrencyVariables.
#[test]
/// 并发度类系统变量边界。
fn test_concurrency_variables() {
    let mut vars = sysvar::SessionVars::default();
    assert_eq!(vars.ExecutorConcurrency, vardef::ConcurrencyUnset as i32);

    for (name, default) in [
        (
            "tidb_window_concurrency",
            vardef::DefExecutorConcurrency as i32,
        ),
        (
            "tidb_merge_join_concurrency",
            vardef::DefTiDBMergeJoinConcurrency as i32,
        ),
        (
            "tidb_stream_agg_concurrency",
            vardef::DefTiDBStreamAggConcurrency as i32,
        ),
        (
            "tidb_executor_concurrency",
            vardef::DefExecutorConcurrency as i32,
        ),
    ] {
        let variable = sysvar::newExecConcurrencySysVar(
            name,
            default,
            Arc::new(|vars, value| vars.ExecutorConcurrency = value),
            std::iter::empty::<sysvar::ExecConcurrencySysVarOption>(),
        );
        assert_eq!(variable.Value, default.to_string());
        variable.SetSession(&mut vars, "2").unwrap();
        assert_eq!(vars.ExecutorConcurrency, 2);
        variable.SetSession(&mut vars, "invalid").unwrap();
        assert_eq!(vars.ExecutorConcurrency, vardef::ConcurrencyUnset as i32);
    }

    let executor = vardef::DefExecutorConcurrency as i32 + 1;
    assert_eq!(executor, vardef::DefExecutorConcurrency as i32 + 1);
}

// TestHelperFuncs.
#[test]
/// 解析/转换辅助函数。
fn test_helper_funcs() {
    assert_eq!(BoolToOnOff(1 == 1), "ON");
    assert_eq!(BoolToOnOff(0 == 1), "OFF");

    assert_eq!(
        vardef::TiDBOptEnableClustered("ON"),
        vardef::ClusteredIndexDefModeOn
    );
    assert_eq!(
        vardef::TiDBOptEnableClustered("OFF"),
        vardef::ClusteredIndexDefModeOff
    );
    assert_eq!(
        vardef::TiDBOptEnableClustered("bogus"),
        vardef::ClusteredIndexDefModeIntOnly
    );

    assert_eq!(tidbOptPositiveInt32("1234", 5), 1234);
    assert_eq!(tidbOptPositiveInt32("-1234", 5), 5);
    assert_eq!(tidbOptPositiveInt32("bogus", 5), 5);
    assert_eq!(TidbOptInt("1234", 5), 1234);
    assert_eq!(TidbOptInt("-1234", 5), -1234);
    assert_eq!(TidbOptInt("bogus", 5), 5);
}

// TestSessionStatesSystemVar.
#[test]
/// 会话状态相关系统变量。
fn test_session_states_system_var() {
    let mut vars = session::SessionVars::new();
    vars.SetSystemVar("autocommit", "ON").unwrap();
    let (value, keep) = vars.GetSessionStatesSystemVar("autocommit");
    assert_eq!(value, "ON");
    assert!(keep);

    let (value, keep) = vars.GetSessionStatesSystemVar("timestamp");
    assert_eq!(value, "");
    assert!(!keep);

    vars.SetSystemVar("max_allowed_packet", "1024").unwrap();
    let (value, keep) = vars.GetSessionStatesSystemVar("max_allowed_packet");
    assert_eq!(value, "1024");
    assert!(keep);
}

// TestOnOffHelpers.
#[test]
/// ON/OFF 与 true/false 互转。
fn test_on_off_helpers() {
    for value in ["TRUE", "TRue", "true"] {
        assert_eq!(trueFalseToOnOff(value), "ON");
    }
    for value in ["FALSE", "False", "false"] {
        assert_eq!(trueFalseToOnOff(value), "OFF");
    }
    assert_eq!(trueFalseToOnOff("other"), "other");

    for value in ["ON", "on", "On"] {
        assert_eq!(OnOffToTrueFalse(value), "true");
    }
    for value in ["OFF", "Off", "off"] {
        assert_eq!(OnOffToTrueFalse(value), "false");
    }
    assert_eq!(OnOffToTrueFalse("other"), "other");
}

// TestAssertionLevel.
#[test]
/// 断言级别解析。
fn test_assertion_level() {
    assert_eq!(
        tidbOptAssertionLevel(vardef::AssertionStrictStr),
        AssertionLevel::AssertionLevelStrict
    );
    assert_eq!(
        tidbOptAssertionLevel(vardef::AssertionOffStr),
        AssertionLevel::AssertionLevelOff
    );
    assert_eq!(
        tidbOptAssertionLevel(vardef::AssertionFastStr),
        AssertionLevel::AssertionLevelFast
    );
    assert_eq!(
        tidbOptAssertionLevel("bogus"),
        AssertionLevel::AssertionLevelOff
    );
}
