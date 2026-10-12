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

// 系统变量（sysvar）缓存：替代旧的 GlobalVariableCache。
//
// 对齐 privilege cache 思路：默认约 30s 有效、更新时失效，并通过 etcd 通知
// 其他 TiDB 节点重建。缓存拆成 global / session 两份 map；rebuild 时用
// `rebuild_lock` 串行化，避免并发 fetch 导致丢更新。

use std::collections::BTreeMap;
use std::sync::{Mutex, RwLock};

#[derive(Clone, Debug, PartialEq, Eq)]
/// 单个系统变量的定义元数据（名称、默认值、作用域与初始化来源）。
pub struct SysVarDefinition {
    /// 变量名（如 `max_allowed_packet`）。
    pub name: String,
    /// 默认值字符串。
    pub default_value: String,
    /// 为 true 时不写入 session 缓存（对齐 Go SysVar.SkipInit）。
    pub skip_session_init: bool,
    /// 是否具有 GLOBAL 作用域。
    pub global_scope: bool,
    /// 是否已从配置初始化（此时优先用默认值而非表中值）。
    pub initialized_from_config: bool,
}

/// 从持久化来源（如 `mysql.global_variables`）读取变量名→值映射。
pub trait SysVarSource {
    /// 返回表中全部全局变量快照。
    fn table_values(&self) -> Result<BTreeMap<String, String>, String>;
}

#[derive(Default)]
/// 系统变量缓存本体：global/session 由 RwLock 保护，rebuild_lock 串行重建。
pub struct SysVarCache {
    /// GLOBAL 作用域变量名→值。
    global: RwLock<BTreeMap<String, String>>,
    /// SESSION 初始化用变量名→值（新 session 深拷贝）。
    session: RwLock<BTreeMap<String, String>>,
    /// 重建互斥锁，防止并发 rebuild 的 lost-update。
    rebuild_lock: Mutex<()>,
}

impl SysVarCache {
    /// 任一份缓存为空则视为需要重建。
    pub fn is_empty(&self) -> bool {
        self.global
            .read()
            .expect("sysvar cache poisoned")
            .is_empty()
            || self
                .session
                .read()
                .expect("sysvar cache poisoned")
                .is_empty()
    }

    /// 完整重建 session/global 缓存：读表、应用配置覆盖、按定义填充并回调 SetGlobal。
    pub fn rebuild<S: SysVarSource>(
        &self,
        source: &S,
        definitions: &[SysVarDefinition],
        config_overrides: &BTreeMap<String, String>,
        set_global: impl FnMut(&str, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        self.rebuild_with_policy(
            source,
            definitions,
            config_overrides,
            |_name, value| value.to_owned(),
            |_name| true,
            set_global,
        )
    }

    /// 按 Go `rebuildSysVarCache` 的完整回调策略重建缓存。
    ///
    /// `validate_global` 对应 `ValidateWithRelaxedValidation`；`should_set_global`
    /// 同时表达 `SetGlobal != nil` 与 `!SkipSysvarCache()`。缓存保存校验前的值，
    /// 只有传给 SetGlobal 的值经过校验，且回调失败只记录而不终止重建。
    pub fn rebuild_with_policy<S: SysVarSource>(
        &self,
        source: &S,
        definitions: &[SysVarDefinition],
        config_overrides: &BTreeMap<String, String>,
        mut validate_global: impl FnMut(&str, &str) -> String,
        mut should_set_global: impl FnMut(&str) -> bool,
        mut set_global: impl FnMut(&str, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        // 同一时刻只允许一个 rebuild，避免较早的 fetch 后写覆盖较新结果。
        let _guard = self
            .rebuild_lock
            .lock()
            .expect("sysvar rebuild lock poisoned");
        // 先取表内容，再用 config_overrides 覆盖已存在的键（如 Starter 的 MaxAllowedPacket）。
        let mut table_values = source.table_values()?;
        for (name, value) in config_overrides {
            if table_values.contains_key(name) {
                table_values.insert(name.clone(), value.clone());
            }
        }
        let mut session = BTreeMap::new();
        let mut global = BTreeMap::new();
        // 按 SysVar 定义生成两份新 map：session 存可初始化项，global 存全局作用域并触发 SetGlobal。
        for definition in definitions {
            // 配置已初始化则用默认值；否则优先表值，缺失时回退默认值。
            let value = if definition.initialized_from_config {
                definition.default_value.clone()
            } else {
                table_values
                    .get(&definition.name)
                    .cloned()
                    .unwrap_or_else(|| definition.default_value.clone())
            };
            if !definition.skip_session_init {
                session.insert(definition.name.clone(), value.clone());
            }
            if definition.global_scope {
                global.insert(definition.name.clone(), value.clone());
                // Go logs SetGlobal failures and continues rebuilding.  A
                // single runtime callback must not leave the cache empty or
                // publish only a partially rebuilt snapshot.
                if should_set_global(&definition.name) {
                    let validated = validate_global(&definition.name, &value);
                    let _ = set_global(&definition.name, &validated);
                }
            }
        }
        *self.session.write().expect("sysvar cache poisoned") = session;
        *self.global.write().expect("sysvar cache poisoned") = global;
        Ok(())
    }

    /// 返回 session 缓存深拷贝；空缓存返回错误（调用方应先 rebuild）。
    pub fn session_cache(&self) -> Result<BTreeMap<String, String>, String> {
        let cache = self
            .session
            .read()
            .map_err(|_| "sysvar cache poisoned".to_string())?;
        if cache.is_empty() {
            Err("sysvar cache is empty".to_string())
        } else {
            Ok(cache.clone())
        }
    }

    /// 按名查询单个全局变量；不存在则返回 unknown system variable。
    pub fn global_var(&self, name: &str) -> Result<String, String> {
        self.global
            .read()
            .map_err(|_| "sysvar cache poisoned".to_string())?
            .get(name)
            .cloned()
            .ok_or_else(|| format!("unknown system variable: {name}"))
    }
}
