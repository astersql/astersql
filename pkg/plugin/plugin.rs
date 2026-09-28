// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 插件运行时：加载、校验、初始化、Flush 监视与全局查询。
//
// 对应 Go `pkg/plugin/plugin.go`。插件（plugin）以动态库或静态注册方式载入，
// 经版本校验与生命周期钩子进入 Ready；可选通过 etcd（键值存储）监视禁用标志并 Flush。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, OnceLock, RwLock};
use std::thread;
use std::time::Duration;

use crate::{
    Context, Id, Kind, LIBRARY_SUFFIX, MANIFEST_SYMBOL, Manifest, PluginError, PluginErrorKind,
    State,
};

/// 静态插件工厂：按名称产出 Manifest。
pub type ManifestFactory = Arc<dyn Fn() -> Manifest + Send + Sync>;
/// 测试加载钩子：替代真实 `.so` 打开，按目录与插件 ID 返回 Manifest。
pub type TestLoadHook = Arc<dyn Fn(&str, &Id) -> Result<Manifest, PluginError> + Send + Sync>;

/// 从共享库路径加载 Manifest 符号的抽象。
pub trait PluginLoader: Send + Sync {
    /// 在 `library_path` 上查找 `symbol` 并解析为 Manifest。
    fn load_manifest(&self, library_path: &Path, symbol: &str) -> Result<Manifest, PluginError>;
}

/// 插件 Flush 所用的键值客户端（通常对接 PD/etcd）。
pub trait KeyValueClient: Send + Sync {
    /// 读取路径上的值；`None` 表示键不存在。
    fn get(&self, path: &str) -> Result<Option<String>, PluginError>;
    /// 写入路径上的值。
    fn put(&self, path: &str, value: &str) -> Result<(), PluginError>;
    /// 监视路径变更；返回一批 watch 事件结果。
    fn watch(&self, path: &str) -> Result<Vec<Result<(), PluginError>>, PluginError>;
}

/// 插件加载/初始化配置。
#[derive(Clone, Default)]
pub struct Config {
    /// 待加载插件 ID 列表（形如 `name-version`）。
    pub plugins: Vec<String>,
    /// 动态库所在目录。
    pub plugin_dir: String,
    /// 失败时跳过并继续，而非整体失败。
    pub skip_when_fail: bool,
    /// 环境组件版本表，供 `require_version` 校验。
    pub environment_versions: HashMap<String, u16>,
    /// 可选 etcd 客户端，用于禁用标志与 Flush。
    pub etcd: Option<Arc<dyn KeyValueClient>>,
    /// 可选动态库加载器。
    pub loader: Option<Arc<dyn PluginLoader>>,
}

/// 监视 etcd 上插件禁用标志并触发 OnFlush 的 watcher。
#[derive(Clone)]
pub struct FlushWatcher {
    context: Context,
    path: String,
    etcd: Arc<dyn KeyValueClient>,
    manifest: Manifest,
    disabled: Arc<AtomicU32>,
    cancelled: Arc<AtomicBool>,
}

impl FlushWatcher {
    /// 按插件名构造默认 Context 的 FlushWatcher。
    pub fn new(
        plugin_name: &str,
        etcd: Arc<dyn KeyValueClient>,
        manifest: Manifest,
        disabled: Arc<AtomicU32>,
    ) -> Self {
        Self {
            context: Context::default(),
            path: format!("/tidb/plugins/{plugin_name}"),
            etcd,
            manifest,
            disabled,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Test helper mirroring Go `flushWatcher` construction with an explicit context.
    /// 测试辅助：用显式 Context 构造，对齐 Go flushWatcher。
    pub fn with_context(
        context: Context,
        plugin_name: &str,
        etcd: Arc<dyn KeyValueClient>,
        manifest: Manifest,
        disabled: Arc<AtomicU32>,
    ) -> Self {
        Self {
            context,
            path: format!("/tidb/plugins/{plugin_name}"),
            etcd,
            manifest,
            disabled,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 从 etcd 刷新禁用标志，并调用 Manifest 的 OnFlush（若存在）。
    pub fn refresh_plugin_state(&self) -> Result<(), PluginError> {
        let disabled = self.get_plugin_disabled_flag()?;
        self.disabled.store(u32::from(disabled), Ordering::Release);
        if let Some(on_flush) = &self.manifest.on_flush {
            on_flush(&self.context, &self.manifest)?;
        }
        Ok(())
    }

    /// 执行一轮 etcd watch：有事件则刷新状态；取消时返回 `true`。
    pub fn watch_loop_once(&self) -> Result<bool, PluginError> {
        if self.cancelled.load(Ordering::Acquire) || self.context.is_cancelled() {
            return Ok(true);
        }
        for event in self.etcd.watch(&self.path)? {
            if self.cancelled.load(Ordering::Acquire) || self.context.is_cancelled() {
                return Ok(true);
            }
            if event.is_ok() {
                // Flush failures are intentionally ignored so later watch events still run.
                // Flush 失败故意忽略，以便后续 watch 事件仍能继续处理。
                let _ = self.refresh_plugin_state();
            }
        }
        Ok(false)
    }

    /// 持续重建已关闭的 watch，直到 watcher 被取消。
    pub fn watch_loop(&self) {
        const REWATCH_INTERVAL: Duration = Duration::from_secs(5);
        while !self.cancelled.load(Ordering::Acquire) && !self.context.is_cancelled() {
            match self.watch_loop_once() {
                Ok(true) => return,
                Ok(false) | Err(_) => thread::sleep(REWATCH_INTERVAL),
            }
        }
    }

    /// Corresponds to Go `flushWatcher.watchLoopWithChan`.
    ///
    /// Returns `true` when the context is cancelled, `false` when the channel closes.
    /// 对应 Go `watchLoopWithChan`：取消返回 true，通道关闭返回 false。
    pub fn watch_loop_with_chan(&self, rx: &Receiver<()>) -> bool {
        loop {
            if self.cancelled.load(Ordering::Acquire) || self.context.is_cancelled() {
                return true;
            }
            match rx.recv_timeout(Duration::from_millis(1)) {
                Ok(()) => {
                    let _ = self.refresh_plugin_state();
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    /// 读取 etcd 路径：值为 `"1"` 表示禁用。
    pub fn get_plugin_disabled_flag(&self) -> Result<bool, PluginError> {
        Ok(self.etcd.get(&self.path)?.is_some_and(|value| value == "1"))
    }

    /// 取消监视并取消内部 Context。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.context.cancel();
    }

    /// 返回 etcd 监视路径（`/tidb/plugins/{name}`）。
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// 已加载插件实例：清单、路径、禁用标志、状态与可选 watcher。
#[derive(Clone)]
pub struct Plugin {
    pub manifest: Manifest,
    pub path: String,
    pub disabled: Arc<AtomicU32>,
    pub state: State,
    pub watcher: Option<FlushWatcher>,
}

impl Default for Plugin {
    fn default() -> Self {
        Self::new(Manifest::default(), String::new())
    }
}

impl Plugin {
    /// 以 Uninitialized 状态构造插件实例。
    fn new(manifest: Manifest, path: String) -> Self {
        Self {
            manifest,
            path,
            disabled: Arc::new(AtomicU32::new(0)),
            state: State::Uninitialized,
            watcher: None,
        }
    }

    /// 返回插件逻辑名。
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// 返回插件种类。
    pub fn kind(&self) -> Kind {
        self.manifest.kind
    }

    /// 返回插件版本号。
    pub fn version(&self) -> u16 {
        self.manifest.version
    }

    /// 格式化为 `{State}-enable|disable` 状态串。
    pub fn state_value(&self) -> String {
        let flag = if self.disabled.load(Ordering::Acquire) == 1 {
            "disable"
        } else {
            "enable"
        };
        format!("{}-{flag}", self.state)
    }

    /// 设置内存中的禁用标志（1=禁用）。
    pub fn disable_flag(&self, disabled: bool) {
        self.disabled.store(u32::from(disabled), Ordering::Release);
    }

    /// 校验 require_version 与可选 validate 回调。
    fn validate(
        &self,
        context: &Context,
        versions: &HashMap<String, u16>,
    ) -> Result<(), PluginError> {
        for (component, required) in &self.manifest.require_version {
            let actual = versions.get(component).copied().unwrap_or_default();
            if actual < *required {
                return Err(PluginError::new(
                    PluginErrorKind::RequiredVersionCheckFailed,
                    format!(
                        "plugin {} requires {component} version {required}, got {actual}",
                        self.manifest.name
                    ),
                ));
            }
        }
        if let Some(validate) = &self.manifest.validate {
            validate(context, &self.manifest)?;
        }
        Ok(())
    }
}

/// Corresponds to Go `plugins`.
/// 全局插件集合：按种类索引、版本表与正在关闭的插件列表。
#[derive(Clone, Default)]
pub struct Plugins {
    pub by_kind: HashMap<Kind, Vec<Plugin>>,
    pub versions: HashMap<String, u16>,
    pub dying_plugins: Vec<Plugin>,
}

impl Plugins {
    /// 登记插件版本并按 Kind 追加到集合。
    fn add(&mut self, plugin: Plugin) {
        self.versions
            .insert(plugin.name().to_owned(), plugin.version());
        self.by_kind.entry(plugin.kind()).or_default().push(plugin);
    }

    /// Corresponds to Go `plugins.clone`.
    /// 深拷贝集合（独立于后续原地修改）。
    pub fn clone_plugins(&self) -> Self {
        self.clone()
    }
}

/// 全局已加载插件集合（OnceLock + RwLock）。
fn global_plugins() -> &'static RwLock<Option<Plugins>> {
    static GLOBAL: OnceLock<RwLock<Option<Plugins>>> = OnceLock::new();
    GLOBAL.get_or_init(|| RwLock::new(None))
}

/// 静态注册的 Manifest 工厂表。
fn static_plugins() -> &'static RwLock<HashMap<String, ManifestFactory>> {
    static STATIC: OnceLock<RwLock<HashMap<String, ManifestFactory>>> = OnceLock::new();
    STATIC.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 测试加载钩子槽位。
fn test_hook() -> &'static RwLock<Option<TestLoadHook>> {
    static HOOK: OnceLock<RwLock<Option<TestLoadHook>>> = OnceLock::new();
    HOOK.get_or_init(|| RwLock::new(None))
}

/// 按名称注册静态插件工厂；重复名称报 DuplicatePlugin。
pub fn register_static_plugin(name: &str, factory: ManifestFactory) -> Result<(), PluginError> {
    let mut registry = static_plugins()
        .write()
        .map_err(|_| PluginError::backend("static registry lock poisoned"))?;
    if registry.contains_key(name) {
        return Err(PluginError::new(
            PluginErrorKind::DuplicatePlugin,
            format!("plugin with name '{name}' has already been registered to static plugins"),
        ));
    }
    registry.insert(name.to_owned(), factory);
    Ok(())
}

/// 按名称查询静态插件工厂。
pub fn get_static_plugin(name: &str) -> Option<ManifestFactory> {
    static_plugins().read().ok()?.get(name).cloned()
}

/// 清空静态插件注册表。
pub fn clear_static_plugins() {
    if let Ok(mut registry) = static_plugins().write() {
        registry.clear();
    }
}

/// 设置或清除测试加载钩子。
pub fn set_test_hook(hook: Option<TestLoadHook>) {
    if let Ok(mut current) = test_hook().write() {
        *current = hook;
    }
}

/// 按配置加载全部插件，校验后写入全局集合。
pub fn load(context: &Context, config: &Config) -> Result<(), PluginError> {
    let mut collection = Plugins {
        by_kind: HashMap::new(),
        versions: config.environment_versions.clone(),
        dying_plugins: Vec::new(),
    };
    // 逐个解析 ID 并 load_one；重复名或失败受 skip_when_fail 控制。
    for plugin_id in &config.plugins {
        let id = Id(plugin_id.clone());
        let (name, _) = id.decode()?;
        if collection.versions.contains_key(&name) {
            if config.skip_when_fail {
                continue;
            }
            return Err(PluginError::new(
                PluginErrorKind::DuplicatePlugin,
                format!("duplicate plugin: {plugin_id}"),
            ));
        }
        match load_one(config, &id) {
            Ok(plugin) => collection.add(plugin),
            Err(_) if config.skip_when_fail => continue,
            Err(error) => return Err(error),
        }
    }

    // 版本与 validate 回调校验；失败时可标为 Disable。
    let versions = collection.versions.clone();
    for plugins in collection.by_kind.values_mut() {
        for plugin in plugins {
            if let Err(error) = plugin.validate(context, &versions) {
                if config.skip_when_fail {
                    plugin.state = State::Disable;
                } else {
                    return Err(error);
                }
            }
        }
    }
    *global_plugins()
        .write()
        .map_err(|_| PluginError::backend("plugin global lock poisoned"))? = Some(collection);
    Ok(())
}

/// Corresponds to Go `loadOne`.
/// 加载单个插件：静态工厂 > 测试钩子 > PluginLoader > 报 Open 失败。
pub fn load_one(config: &Config, plugin_id: &Id) -> Result<Plugin, PluginError> {
    let (name, version_text) = plugin_id.decode()?;
    let static_factory = get_static_plugin(&name);
    let from_static = static_factory.is_some();
    let path = Path::new(&config.plugin_dir)
        .join(format!("{}{}", plugin_id.0, LIBRARY_SUFFIX))
        .to_string_lossy()
        .into_owned();
    let manifest = if let Some(factory) = static_factory {
        factory()
    } else if let Some(hook) = test_hook().read().ok().and_then(|hook| hook.clone()) {
        hook(&config.plugin_dir, plugin_id)?
    } else if let Some(loader) = &config.loader {
        loader.load_manifest(Path::new(&path), MANIFEST_SYMBOL)?
    } else {
        // Mirror Go `plugin.Open` failure when no .so / loader is available.
        // 无 .so / loader 时模拟 Go plugin.Open 失败。
        return Err(PluginError::new(
            PluginErrorKind::InvalidPluginManifest,
            format!("plugin.Open(\"{path}\"): realpath failed"),
        ));
    };
    if manifest.name != name {
        return Err(PluginError::new(
            PluginErrorKind::InvalidPluginName,
            format!("plugin {} exported name {}", plugin_id.0, manifest.name),
        ));
    }
    // 静态插件不校验版本文本；动态加载须与 ID 中版本一致。
    if !from_static && manifest.version.to_string() != version_text {
        return Err(PluginError::new(
            PluginErrorKind::InvalidPluginVersion,
            format!(
                "plugin {} exported version {}",
                plugin_id.0, manifest.version
            ),
        ));
    }
    Ok(Plugin::new(manifest, path))
}

/// Convenience wrapper matching Go `loadOne(dir, id)` call shape.
/// 便捷封装：仅指定目录与 ID 字符串调用 load_one。
pub fn load_one_from_dir(plugin_dir: &str, plugin_id: &str) -> Result<Plugin, PluginError> {
    let config = Config {
        plugin_dir: plugin_dir.to_owned(),
        ..Config::default()
    };
    load_one(&config, &Id(plugin_id.to_owned()))
}

/// 对已加载插件执行 OnInit，并按需挂载 FlushWatcher 后标为 Ready。
pub fn init(context: &Context, config: &Config) -> Result<(), PluginError> {
    let mut global = global_plugins()
        .write()
        .map_err(|_| PluginError::backend("plugin global lock poisoned"))?;
    let Some(collection) = global.as_mut() else {
        return Ok(());
    };
    for plugins in collection.by_kind.values_mut() {
        for plugin in plugins {
            let manifest = plugin.manifest.clone();
            if let Some(on_init) = &manifest.on_init {
                if let Err(error) = on_init(context, &manifest) {
                    if config.skip_when_fail {
                        plugin.state = State::Disable;
                        continue;
                    }
                    return Err(error);
                }
            }
            // 有 OnFlush 且配置了 etcd 时创建 watcher 并立即刷新一次状态。
            if manifest.on_flush.is_some()
                && let Some(etcd) = &config.etcd
            {
                let plugin_name = manifest.name.clone();
                let watcher = FlushWatcher::new(
                    &plugin_name,
                    Arc::clone(etcd),
                    manifest,
                    Arc::clone(&plugin.disabled),
                );
                plugin.watcher = Some(watcher.clone());
                if let Err(error) = watcher.refresh_plugin_state() {
                    if config.skip_when_fail {
                        plugin.state = State::Disable;
                        thread::spawn(move || watcher.watch_loop());
                        continue;
                    }
                    return Err(error);
                }
                thread::spawn(move || watcher.watch_loop());
            }
            plugin.state = State::Ready;
        }
    }
    Ok(())
}

/// 取出全局集合，取消 watcher，调用 OnShutdown，并记入 dying_plugins。
pub fn shutdown(context: &Context) {
    let collection = global_plugins()
        .write()
        .ok()
        .and_then(|mut global| global.take());
    let Some(mut collection) = collection else {
        return;
    };
    for plugins in collection.by_kind.values_mut() {
        for plugin in plugins {
            plugin.state = State::Dying;
            if let Some(watcher) = &plugin.watcher {
                watcher.cancel();
            }
            if let Some(on_shutdown) = &plugin.manifest.on_shutdown {
                // Shutdown errors are logged-and-continued in Go; callers must never lose later cleanup.
                // Go 侧关闭错误仅记录并继续，避免阻断后续清理。
                let _ = on_shutdown(context, &plugin.manifest);
            }
            collection.dying_plugins.push(plugin.clone());
        }
    }
}

/// 按种类与名称查找已加载插件。
pub fn get(kind: Kind, name: &str) -> Option<Plugin> {
    global_plugins()
        .read()
        .ok()?
        .as_ref()?
        .by_kind
        .get(&kind)?
        .iter()
        .find(|plugin| plugin.name() == name)
        .cloned()
}

/// 对指定种类下 Ready 且未禁用的插件逐个回调。
pub fn foreach_plugin(
    kind: Kind,
    mut callback: impl FnMut(&Plugin) -> Result<(), PluginError>,
) -> Result<(), PluginError> {
    let global = global_plugins()
        .read()
        .map_err(|_| PluginError::backend("plugin global lock poisoned"))?;
    let Some(plugins) = global
        .as_ref()
        .and_then(|collection| collection.by_kind.get(&kind))
    else {
        return Ok(());
    };
    for plugin in plugins {
        if plugin.state == State::Ready && plugin.disabled.load(Ordering::Acquire) != 1 {
            callback(plugin)?;
        }
    }
    Ok(())
}

/// 判断该种类下是否存在至少一个 Ready 且未禁用的插件。
pub fn is_enabled(kind: Kind) -> bool {
    global_plugins()
        .read()
        .ok()
        .and_then(|global| {
            global.as_ref()?.by_kind.get(&kind).map(|plugins| {
                plugins.iter().any(|plugin| {
                    plugin.state == State::Ready && plugin.disabled.load(Ordering::Acquire) != 1
                })
            })
        })
        .unwrap_or(false)
}

/// 返回全局按种类分组的插件表快照。
pub fn get_all() -> Option<HashMap<Kind, Vec<Plugin>>> {
    global_plugins()
        .read()
        .ok()?
        .as_ref()
        .map(|collection| collection.by_kind.clone())
}

/// 跨种类按名称查找插件。
pub fn get_by_name(name: &str) -> Option<Plugin> {
    get_all()?
        .into_values()
        .flatten()
        .find(|plugin| plugin.name() == name)
}

/// 确认插件存在、Ready 且挂有 FlushWatcher。
fn supports_flush(plugin: Option<&Plugin>, name: &str) -> Result<(), PluginError> {
    let Some(plugin) = plugin else {
        return Err(PluginError::new(
            PluginErrorKind::PluginNotFound,
            format!("plugin '{name}' not found"),
        ));
    };
    if plugin.state != State::Ready {
        return Err(PluginError::new(
            PluginErrorKind::PluginNotReady,
            format!("plugin '{name}' is not ready"),
        ));
    }
    if plugin.watcher.is_none() {
        return Err(PluginError::new(
            PluginErrorKind::FlushUnsupported,
            format!("plugin {name} does not support flush, or PD is not available"),
        ));
    }
    Ok(())
}

/// 向 etcd 写入当前禁用标志以触发 Flush 通知。
pub fn notify_flush(plugin_name: &str) -> Result<(), PluginError> {
    let plugin = get_by_name(plugin_name);
    supports_flush(plugin.as_ref(), plugin_name)?;
    let plugin = plugin.expect("supports_flush checked existence");
    let watcher = plugin
        .watcher
        .as_ref()
        .expect("supports_flush checked watcher");
    watcher.etcd.put(
        watcher.path(),
        &plugin.disabled.load(Ordering::Acquire).to_string(),
    )
}

/// 更新内存禁用标志并写入 etcd，触发跨节点 Flush。
pub fn change_disable_flag_and_flush(plugin_name: &str, disabled: bool) -> Result<(), PluginError> {
    let plugin = get_by_name(plugin_name);
    supports_flush(plugin.as_ref(), plugin_name)?;
    let plugin = plugin.expect("supports_flush checked existence");
    plugin.disable_flag(disabled);
    let watcher = plugin
        .watcher
        .as_ref()
        .expect("supports_flush checked watcher");
    watcher
        .etcd
        .put(watcher.path(), &u32::from(disabled).to_string())
}
