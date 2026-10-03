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

use crate::*;
#[test]
fn embedding_api_base_matches_openai_whitelist_and_endpoint_normalization() {
    for (input, expected) in [
        ("", ""),
        (
            "https://api.openai.com/a/../v1/%65mbeddings",
            "https://api.openai.com/a/../v1",
        ),
        (
            "https://API.OPENAI.COM:443/v1/embeddings",
            "https://API.OPENAI.COM:443/v1",
        ),
        (
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
        ),
        (
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
        ),
        (
            "  https://api.openai.com/v1/embeddings/ ",
            "https://api.openai.com/v1",
        ),
        (
            "https://custom.openai.azure.com/openai/v1",
            "https://custom.openai.azure.com/openai/v1",
        ),
        (
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        ),
    ] {
        assert_eq!(NormalizeOpenAIEmbeddingAPIBase(input).unwrap(), expected);
    }
    for input in [
        "http://api.openai.com/v1",
        "api.openai.com/v1",
        "https://evil.example/v1",
        "https://api.openai.com.evil/v1",
        "https://api.openai.com/v1?q=1",
        "https://api.openai.com/v1#fragment",
    ] {
        assert!(NormalizeOpenAIEmbeddingAPIBase(input).is_err(), "{input}");
    }
    assert_eq!(mask_embedding_api_key(""), "");
    assert_eq!(mask_embedding_api_key("short"), "******");
    assert_eq!(mask_embedding_api_key("1234567890"), "******7890");
}
#[test]
fn embedding_variables_mask_credentials_and_invalidate_only_changed_values() {
    register_builtin_sysvars();
    let mut vars = SessionVars::new(Box::new(NoopAccessor));
    for name in EMBEDDING_API_KEYS {
        let var = GetSysVar(name).unwrap();
        assert_eq!(var.Scope, vardef::ScopeGlobal);
        assert!(
            var.Validate(&mut vars, "secret", vardef::ScopeSession)
                .is_err()
        );
        let previous = embedding_api_key(name);
        let setter = var.SetGlobal.as_ref().unwrap();
        setter(&Context, &mut vars, "1234567890").unwrap();
        let version = embedding_config_version();
        setter(&Context, &mut vars, "1234567890").unwrap();
        assert_eq!(embedding_config_version(), version);
        assert_eq!(
            var.GetGlobal.as_ref().unwrap()(&Context, &mut vars).unwrap(),
            "******7890"
        );
        setter(&Context, &mut vars, &previous).unwrap();
    }
    let var = GetSysVar(EMBEDDING_API_BASE).unwrap();
    let setter = var.SetGlobal.as_ref().unwrap();
    let previous = embedding_api_key(EMBEDDING_API_BASE);
    setter(&Context, &mut vars, "").unwrap();
    let version = embedding_config_version();
    setter(&Context, &mut vars, DEFAULT_EMBEDDING_API_BASE).unwrap();
    assert_eq!(embedding_config_version(), version);
    assert_eq!(GetOpenAIEmbeddingBaseURL(), DEFAULT_EMBEDDING_API_BASE);
    assert!(setter(&Context, &mut vars, "https://evil.example").is_err());
    setter(&Context, &mut vars, &previous).unwrap();
}

struct NoopAccessor;

impl GlobalVarAccessor for NoopAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &Context,
        _name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), VariableError> {
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), VariableError> {
        Ok(())
    }
}
