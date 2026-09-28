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
//
// 文件前半为贴近 Go 的机械翻译草稿（块注释内），后半为可编译的 `SysVarCache`。

// use std::collections::HashMap;
// use std::sync::{Mutex, RwLock};
//
// Error 是 Go error 的占位类型；具体错误栈和 GenWithStackByArgs 语义在后续接线时补齐。
// pub type Error = String;
//
// pub struct SessionContext;
//
// Domain 这里只列出本文件访问到的字段，保持与 Go receiver 方法的读写关系。
// pub struct Domain {
//     pub sysVarCache: sysVarCache,
//     pub sysSessionPool: SysSessionPool,
//     pub infoCache: InfoCache,
// }
//
// pub struct SysSessionPool;
// pub struct InfoCache;
// pub struct SysVar {
//     pub Name: String,
//     pub Value: String,
//     pub IsInitedFromConfig: bool,
//     pub SetGlobal: Option<fn(SessionVars, String) -> Result<(), Error>>,
// }
// pub struct SessionVars;
//
// The sysvar cache replaces the GlobalVariableCache.
// It is an improvement because it operates similar to privilege cache:
// - it caches for 30s instead of 2s
// - the cache is invalidated on update
// - an etcd notification is sent to other tidb servers.
//
// sysVarCache represents the cache of system variables broken up into session and global scope.
// sysVarCache 对应 Go 结构体：global/session 由 RWMutex 保护，rebuildLock 串行化 rebuild。
// pub struct sysVarCache {
// Go 使用嵌入 syncutil.RWMutex 保护 global 和 session map；这里把两份 map 放进 RwLock。
//     pub global: RwLock<HashMap<String, String>>,
//     pub session: RwLock<HashMap<String, String>>,
// rebuildLock protects concurrent rebuild.
//     pub rebuildLock: Mutex<()>,
// }
//
// impl Domain {
// rebuildSysVarCacheIfNeeded 对应 Go 的同名方法：缓存为空时触发一次重建。
//     pub fn rebuildSysVarCacheIfNeeded(&self) -> Result<(), Error> {
//         let cacheNeedsRebuild = {
//             let session = self.sysVarCache.session.read().map_err(|_| "session lock poisoned".to_string())?;
//             let global = self.sysVarCache.global.read().map_err(|_| "global lock poisoned".to_string())?;
//             session.is_empty() || global.is_empty()
//         };
//
//         if cacheNeedsRebuild {
// Go 这里记录 warn，并在重建失败时记录 error；直接返回错误给调用者。
//             log_warn("sysvar cache is empty, triggering rebuild");
//             if let Err(err) = self.rebuildSysVarCache(None) {
//                 log_error("rebuilding sysvar cache failed", &err);
//                 return Err(err);
//             }
//         }
//         Ok(())
//     }
//
// GetSessionCache gets a copy of the session sysvar cache.
// The intention is to copy it directly to the systems[] map
// on creating a new session.
// GetSessionCache 返回 session 缓存深拷贝，对应 Go 的 maps.Clone。
//     pub fn GetSessionCache(&self) -> Result<HashMap<String, String>, Error> {
//         self.rebuildSysVarCacheIfNeeded()?;
//         let session = self.sysVarCache.session.read().map_err(|_| "session lock poisoned".to_string())?;
// Go 将副本直接赋给新 session 的 systems[]，因此不能返回内部 map 引用。
//         Ok(session.clone())
//     }
//
// GetGlobalVar gets an individual global var from the sysvar cache.
// GetGlobalVar 对应 Go 方法：先保证缓存已构建，再从 global map 查询单个变量。
//     pub fn GetGlobalVar(&self, name: &str) -> Result<String, Error> {
//         self.rebuildSysVarCacheIfNeeded()?;
//         let global = self.sysVarCache.global.read().map_err(|_| "global lock poisoned".to_string())?;
//
//         if let Some(val) = global.get(name) {
//             return Ok(val.clone());
//         }
// Go 这里返回 variable.ErrUnknownSystemVar.GenWithStackByArgs(name)。
//         log_warn(&format!("could not find key in global cache: {}", name));
//         Err(format!("unknown system variable: {}", name))
//     }
//
// fetchTableValues 对应 Go 的 (*Domain).fetchTableValues。
// Reads mysql.global_variables via RestrictedSQLExecutor.
//     pub fn fetchTableValues(&self, sctx: &SessionContext) -> Result<HashMap<String, String>, Error> {
//         let _ = sctx;
//         let mut tableContents = HashMap::new();
// Go 使用 kv.WithInternalSourceType(context.Background(), kv.InternalTxnSysVar) 标记内部事务来源。
//         let rows = exec_restricted_sql(
//             "SELECT variable_name, variable_value FROM mysql.global_variables",
//         )?;
//         for row in rows {
//             let name = row.get_string(0);
//             let val = row.get_string(1);
//             tableContents.insert(name, val);
//         }
//         Ok(tableContents)
//     }
//
// overrideSysVarWithConfig 对应 Go 的同名方法：Starter 模式下用 config 覆盖 MaxAllowedPacket。
//     pub fn overrideSysVarWithConfig(&self, tableContent: &mut HashMap<String, String>) {
//         if tableContent.contains_key(vardef_MaxAllowedPacket()) {
//             tableContent.insert(
//                 vardef_MaxAllowedPacket().to_string(),
//                 config_GetMaxAllowedPacket().to_string(),
//             );
//         }
//     }
//
// rebuildSysVarCache rebuilds the sysvar cache both globally and for session vars.
// It needs to be called when sysvars are added or removed.
// rebuildSysVarCache 对应 Go 的完整重建流程：取 session、串行 fetch、按 SysVar 定义生成两份新缓存。
//     pub fn rebuildSysVarCache(&self, ctx: Option<SessionContext>) -> Result<(), Error> {
//         let mut newSessionCache = HashMap::new();
//         let mut newGlobalCache = HashMap::new();
//
// Go 在 ctx 为 nil 时从 sysSessionPool.Get() 借出 session，并用 defer Put(res) 归还。
//         let borrowed_ctx;
//         let ctx_ref = if let Some(ref ctx) = ctx {
//             ctx
//         } else {
//             borrowed_ctx = self.sysSessionPool.get()?;
//             &borrowed_ctx
//         };
//
// Only one rebuild can be in progress at a time, this prevents a lost update race
// where an earlier fetchTableValues() finishes last.
//         let _rebuild_guard = self
//             .sysVarCache
//             .rebuildLock
//             .lock()
//             .map_err(|_| "rebuild lock poisoned".to_string())?;
//
//         let mut tableContents = self.fetchTableValues(ctx_ref)?;
//
//         if deploymode_IsStarter() {
//             self.overrideSysVarWithConfig(&mut tableContents);
//         }
//
//         for sv in variable_GetSysVars() {
//             let mut sVal = sv.Value.clone();
// NOTE: instance variable use values stored in this instance
//             if tableContents.contains_key(&sv.Name) && !sv.IsInitedFromConfig {
//                 sVal = tableContents.get(&sv.Name).cloned().unwrap_or_default();
//             }
//
// session cache stores non-skippable variables, which essentially means session scope.
// for historical purposes there are some globals, but these should eventually be removed.
//             if !sv.SkipInit() {
//                 newSessionCache.insert(sv.Name.clone(), sVal.clone());
//             }
//
//             if sv.HasGlobalScope() {
//                 newGlobalCache.insert(sv.Name.clone(), sVal.clone());
//
// Call the SetGlobal func for this sysvar if it exists.
// SET GLOBAL only calls the SetGlobal func on the calling instances.
// This ensures it is run on all tidb servers.
// This does not apply to INSTANCE scoped vars (HasGlobalScope() is false)
//                 if let Some(set_global) = sv.SetGlobal {
//                     if !sv.SkipSysvarCache() {
// Go 先用 relaxed validation 校验全局变量值，再调用 SetGlobal。
//                         let validated = sv.ValidateWithRelaxedValidation(ctx_ref.get_session_vars(), sVal, vardef_ScopeGlobal());
//                         if let Err(err) = set_global(ctx_ref.get_session_vars(), validated) {
//                             log_error(&format!("load global variable {} error", sv.Name), &err);
//                         }
//                     }
//                 }
//             }
//         }
//
//         log_debug("rebuilding sysvar cache");
//
//         {
//             let mut session = self.sysVarCache.session.write().map_err(|_| "session lock poisoned".to_string())?;
//             *session = newSessionCache;
//         }
//         {
//             let mut global = self.sysVarCache.global.write().map_err(|_| "global lock poisoned".to_string())?;
//             *global = newGlobalCache;
//         }
//         self.infoCache.ReSize(vardef_SchemaVersionCacheLimit_Load() as i32);
//         Ok(())
//     }
// }
//
// impl SysSessionPool {
// get 对应 Go 的 sysSessionPool.Get；资源归还由 Rust drop 注释性表达，不连接真实 session pool。
//     pub fn get(&self) -> Result<SessionContext, Error> {
//         Ok(SessionContext)
//     }
// }
//
// impl InfoCache {
//     pub fn ReSize(&self, _limit: i32) {}
// }
//
// impl SessionContext {
//     pub fn get_session_vars(&self) -> SessionVars {
//         SessionVars
//     }
// }
//
// impl SysVar {
//     pub fn SkipInit(&self) -> bool {
//         false
//     }
//
//     pub fn HasGlobalScope(&self) -> bool {
//         true
//     }
//
//     pub fn SkipSysvarCache(&self) -> bool {
//         false
//     }
//
//     pub fn ValidateWithRelaxedValidation(
//         &self,
//         _vars: SessionVars,
//         value: String,
//         _scope: i32,
//     ) -> String {
//         value
//     }
// }
//
// pub struct RestrictedRow {
//     values: Vec<String>,
// }
//
// impl RestrictedRow {
//     pub fn get_string(&self, idx: usize) -> String {
//         self.values.get(idx).cloned().unwrap_or_default()
//     }
// }
//
// fn exec_restricted_sql(_sql: &str) -> Result<Vec<RestrictedRow>, Error> {
// Restricted SQL 是外部依赖；不访问 mysql.global_variables，返回空结果保留控制流。
//     Ok(Vec::new())
// }
//
// fn variable_GetSysVars() -> Vec<SysVar> {
//     Vec::new()
// }
//
// fn vardef_MaxAllowedPacket() -> &'static str {
//     "max_allowed_packet"
// }
//
// fn vardef_ScopeGlobal() -> i32 {
//     1
// }
//
// fn vardef_SchemaVersionCacheLimit_Load() -> i64 {
//     0
// }
//
// fn config_GetMaxAllowedPacket() -> u64 {
//     0
// }
//
// fn deploymode_IsStarter() -> bool {
//     false
// }
//
// fn log_warn(message: &str) {
//     let _ = message;
// }
//
// fn log_error(message: &str, err: &Error) {
//     let _ = (message, err);
// }
//
// fn log_debug(message: &str) {
//     let _ = message;
// }
// */
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
