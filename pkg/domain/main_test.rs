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

// Domain 包级 smoke 测试入口。
//
// 校验默认 `DomainConfig` 中与后台任务相关的有限上限（慢查询 Top-N、
// 近期窗口、dump 文件 GC lease）均已合理初始化，避免无限资源占用。

/// 断言默认配置使用有限的后台任务上限，而非零值或空 lease。
#[test]
fn canonical_domain_test_runtime_uses_finite_background_limits() {
    let config = crate::domain::DomainConfig::default();
    assert!(config.slow_query_top_n > 0);
    assert!(config.slow_query_recent >= config.slow_query_top_n);
    assert!(!config.dump_file_gc_lease.is_zero());
}

#[test]
/// SetGlobal 失败只记录错误，不应阻止 sysvar cache 原子替换。
fn canonical_sysvar_cache_keeps_values_when_set_global_callback_fails() {
    use crate::sysvar_cache::{SysVarCache, SysVarDefinition, SysVarSource};
    use std::collections::BTreeMap;

    struct Source;
    impl SysVarSource for Source {
        fn table_values(&self) -> Result<BTreeMap<String, String>, String> {
            Ok(BTreeMap::from([("example".into(), "from-table".into())]))
        }
    }

    let cache = SysVarCache::default();
    let definitions = [SysVarDefinition {
        name: "example".into(),
        default_value: "default".into(),
        skip_session_init: false,
        global_scope: true,
        initialized_from_config: false,
    }];
    let result = cache.rebuild(&Source, &definitions, &BTreeMap::new(), |_name, _value| {
        Err("callback failed".into())
    });
    assert!(result.is_ok());
    assert_eq!(cache.session_cache().unwrap()["example"], "from-table");
    assert_eq!(cache.global_var("example").unwrap(), "from-table");
}
