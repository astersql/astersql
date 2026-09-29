// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Plan Replayer 配置加载迁移补充单元测试。
//
// 对齐 Go：忽略名单、校验失败、Set 钩子拒绝、未知变量的跳过与日志；
// 以及 TOML/IO 错误时不产生部分写入。

use std::io;
use std::sync::Arc;

use serial_test::serial;
use util_config::{ConfigLoadError, LoadConfigForPlanReplayerLoad, logutil, variable};

/// 空的全局变量访问器：读未知、写空操作，满足 `SessionVars` 构造依赖。
#[derive(Default)]
struct EmptyGlobalAccessor;

impl variable::GlobalVarAccessor for EmptyGlobalAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, variable::VariableError> {
        Err(variable::VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &variable::Context,
        _name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), variable::VariableError> {
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, variable::VariableError> {
        Err(variable::VariableError::unknown(name))
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), variable::VariableError> {
        Ok(())
    }
}

/// 构造仅会话作用域的测试用 `SysVar`。
fn session_var(name: &str, value_type: variable::vardef::TypeFlag) -> variable::SysVar {
    variable::SysVar {
        Scope: variable::vardef::ScopeSession,
        Name: name.to_owned(),
        Type: value_type,
        ..variable::SysVar::default()
    }
}

/// 覆盖忽略、未知、校验失败、Set 失败四类跳过路径，并校验 clamp 与日志文案。
#[test]
#[serial]
fn plan_replayer_load_matches_go_skip_validate_set_and_ignore_behavior() {
    // 注册带边界、枚举、Set 钩子的三类变量。
    variable::clear_sys_vars_for_test();

    let mut bounded = session_var("bounded", variable::vardef::TypeUnsigned);
    bounded.MaxValue = 10;
    variable::RegisterSysVar(bounded);

    let mut bad_enum = session_var("bad_enum", variable::vardef::TypeEnum);
    bad_enum.PossibleValues = vec!["ON".to_owned(), "OFF".to_owned()];
    variable::RegisterSysVar(bad_enum);

    let mut set_fails = session_var("set_fails", variable::vardef::TypeStr);
    set_fails.SetSession = Some(Arc::new(|_, _| {
        Err(variable::VariableError::new(
            variable::VariableErrorKind::InvalidValue,
            "set hook rejected value",
        ))
    }));
    variable::RegisterSysVar(set_fails);

    let logger = logutil::log::BgLogger();
    let original_log_count = logger.entries().len();
    let mut vars = variable::SessionVars::new(Box::new(EmptyGlobalAccessor));
    let input = br#"
bounded = "99"
bad_enum = "invalid"
set_fails = "value"
unknown_name = "value"
innodb_lock_wait_timeout = "88"
"#;

    // 加载后：bounded 被 clamp 到 MaxValue；其余失败项进入 unloaded。
    let mut unloaded = LoadConfigForPlanReplayerLoad(&mut vars, &input[..]).unwrap();
    unloaded.sort();

    assert_eq!(unloaded, ["bad_enum", "set_fails", "unknown_name"]);
    assert_eq!(vars.system("bounded"), Some("10"));
    assert_eq!(vars.system("bad_enum"), None);
    assert_eq!(vars.system("set_fails"), None);
    assert_eq!(vars.system("unknown_name"), None);
    assert_eq!(vars.system("innodb_lock_wait_timeout"), None);

    // 校验后台日志包含 ignore/skip 各类消息。
    let entries = logger.entries();
    let messages: Vec<_> = entries[original_log_count..]
        .iter()
        .map(|entry| entry.message.as_str())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("ignore set variable innodb_lock_wait_timeout:88"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("skip set variable unknown_name:value"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("skip variable bad_enum:invalid"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("skip set variable set_fails:value"))
    );

    variable::clear_sys_vars_for_test();
}

#[test]
#[serial]
fn go_merge_11_ignores_read_timestamp_overrides() {
    variable::clear_sys_vars_for_test();
    for name in [
        "tidb_low_resolution_tso",
        "tidb_snapshot",
        "tidb_read_staleness",
    ] {
        variable::RegisterSysVar(session_var(name, variable::vardef::TypeStr));
    }
    let mut vars = variable::SessionVars::new(Box::new(EmptyGlobalAccessor));
    let input = b"tidb_low_resolution_tso = \"ON\"\ntidb_snapshot = \"123\"\ntidb_read_staleness = \"-1\"\n";
    assert!(
        LoadConfigForPlanReplayerLoad(&mut vars, &input[..])
            .unwrap()
            .is_empty()
    );
    for name in [
        "tidb_low_resolution_tso",
        "tidb_snapshot",
        "tidb_read_staleness",
    ] {
        assert_eq!(vars.system(name), None, "{name} must not be loaded");
    }
    variable::clear_sys_vars_for_test();
}

/// TOML 畸形或 Reader 失败时应整体中止，且不留下部分变量更新。
#[test]
#[serial]
fn malformed_toml_and_reader_errors_abort_without_partial_updates() {
    variable::clear_sys_vars_for_test();
    variable::RegisterSysVar(session_var("known", variable::vardef::TypeStr));
    let mut vars = variable::SessionVars::new(Box::new(EmptyGlobalAccessor));

    let malformed = LoadConfigForPlanReplayerLoad(&mut vars, b"known = [".as_slice());
    assert!(matches!(malformed, Err(ConfigLoadError::Toml(_))));
    assert_eq!(vars.system("known"), None);

    // 故意失败的 Reader，用于触发 ConfigLoadError::Io。
    struct BrokenReader;
    impl io::Read for BrokenReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::Other, "reader failed"))
        }
    }

    let broken = LoadConfigForPlanReplayerLoad(&mut vars, BrokenReader);
    let broken = broken.unwrap_err();
    assert!(matches!(broken, ConfigLoadError::Io(_)));
    assert_eq!(broken.to_string(), "reader failed");
    assert_eq!(vars.system("known"), None);

    variable::clear_sys_vars_for_test();
}
