// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::collections::BTreeMap;

use astersql_sessionctx_variable::{
    Context, GetSysVars, GlobalVarAccessor, SessionVars, VariableError, VariableErrorKind, vardef,
};

struct SnapshotAccessor(BTreeMap<String, String>);

impl SnapshotAccessor {
    fn unsupported() -> VariableError {
        VariableError::new(
            VariableErrorKind::InvalidValue,
            "global-variable diagnostic accessor is read-only",
        )
    }
}

impl GlobalVarAccessor for SnapshotAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        self.0
            .get(&name.to_ascii_lowercase())
            .cloned()
            .or_else(|| {
                GetSysVars()
                    .get(&name.to_ascii_lowercase())
                    .map(|variable| variable.Value.clone())
            })
            .ok_or_else(|| VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _: &Context,
        _: &str,
        _: &str,
        _: bool,
    ) -> Result<(), VariableError> {
        Err(Self::unsupported())
    }

    fn get_tidb_table_value(&self, _: &str) -> Result<String, VariableError> {
        Err(Self::unsupported())
    }

    fn set_tidb_table_value(&mut self, _: &str, _: &str, _: &str) -> Result<(), VariableError> {
        Err(Self::unsupported())
    }
}

/// Build the diagnostic global-variable snapshot. Values come from the
/// runtime cache when present and otherwise use the registered default.
pub fn global_variables(
    overrides: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();
    let mut session_vars = SessionVars::new(Box::new(SnapshotAccessor(overrides.clone())));
    for (_, variable) in GetSysVars() {
        if variable.Scope == vardef::ScopeSession
            || variable.IsNoop && !vardef::EnableNoopVariables.Load()
        {
            continue;
        }
        let mut value = variable
            .GetGlobalFromHook(&Context, &mut session_vars)
            .map_err(|error| format!("read {}: {error}", variable.Name))?;
        if variable
            .Name
            .eq_ignore_ascii_case(vardef::TiDBCloudStorageURI)
            && let Some(configured) = overrides.get(&variable.Name.to_ascii_lowercase())
        {
            value = configured.clone();
            value = astersql_parser_ast::misc::redact_url(&value);
        } else if variable.IsSensitive && !value.is_empty() {
            value = vardef::MaskPwd.to_owned();
        }
        values.insert(variable.Name, value);
    }
    Ok(values)
}
