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

// 插件 SPI（Service Provider Interface）基础类型与清单定义。
//
// 提供插件共享库后缀、Manifest 符号名、可取消的 `Context`，以及各类
// Manifest（通用 / 认证 / Schema / Daemon）与导出 trait。

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Kind, PluginError};

/// 动态库文件后缀（对应 Go 侧 `.so`）。
pub const LIBRARY_SUFFIX: &str = ".so";
/// 共享库中导出 Manifest 符号的名称。
pub const MANIFEST_SYMBOL: &str = "PluginManifest";

/// `Context` 类型化值键；每种键声明其对应的值类型。
pub trait ContextKey: Send + Sync + 'static {
    type Value: Any + Send + Sync;
}

/// 插件生命周期回调使用的可取消上下文。
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
    /// Corresponds to Go `ctx.Value(plugin.IsRetryingCtxKey)`.
    /// 对应 Go `IsRetryingCtxKey`：标记当前是否处于重试路径。
    is_retrying: Arc<AtomicBool>,
    /// Go `context.WithValue` 的类型化值快照。
    values: Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl Context {
    /// 标记上下文已取消，供 watcher / 回调协作退出。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// 查询上下文是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// 返回带有新值的派生上下文，不修改父上下文。
    ///
    /// 派生上下文与父上下文共享取消和重试状态，值则采用写时复制；同类型键的新值
    /// 会遮蔽旧值，对应 Go `context.WithValue` 的查找语义。
    pub fn with_value<K: ContextKey>(&self, _key: K, value: K::Value) -> Self {
        let mut values = self.values.as_ref().clone();
        values.insert(TypeId::of::<K>(), Arc::new(value));
        Self {
            cancelled: Arc::clone(&self.cancelled),
            is_retrying: Arc::clone(&self.is_retrying),
            values: Arc::new(values),
        }
    }

    /// 按类型化键读取上下文值；未设置时返回 `None`。
    pub fn value<K: ContextKey>(&self, _key: K) -> Option<Arc<K::Value>> {
        self.values
            .get(&TypeId::of::<K>())
            .cloned()?
            .downcast::<K::Value>()
            .ok()
    }

    /// 设置是否处于重试（retrying）状态。
    pub fn set_retrying(&self, retrying: bool) {
        self.is_retrying.store(retrying, Ordering::Release);
    }

    /// 查询是否处于重试状态。
    pub fn is_retrying(&self) -> bool {
        self.is_retrying.load(Ordering::Acquire)
    }
}

/// 生命周期回调类型：validate / on_init / on_shutdown / on_flush 共用签名。
pub type LifecycleCallback =
    Arc<dyn Fn(&Context, &Manifest) -> Result<(), PluginError> + Send + Sync>;

/// 插件清单：名称、版本、依赖版本、生命周期钩子与扩展数据。
#[derive(Clone)]
pub struct Manifest {
    /// 插件逻辑名，须与加载 ID 中的 name 一致。
    pub name: String,
    /// 人类可读描述。
    pub description: String,
    /// 组件名 → 最低要求版本号（如 go 运行时版本）。
    pub require_version: HashMap<String, u16>,
    /// 许可证文本。
    pub license: String,
    /// 构建时间字符串。
    pub build_time: String,
    /// 加载后校验回调。
    pub validate: Option<LifecycleCallback>,
    /// 初始化回调。
    pub on_init: Option<LifecycleCallback>,
    /// 关闭回调。
    pub on_shutdown: Option<LifecycleCallback>,
    /// Flush（刷新配置/状态）回调。
    pub on_flush: Option<LifecycleCallback>,
    /// 类型擦除的扩展载荷（如审计回调集合）。
    pub extension: Option<Arc<dyn Any + Send + Sync>>,
    /// 插件版本号。
    pub version: u16,
    /// 插件种类（审计 / 认证 / Schema / Daemon 等）。
    pub kind: Kind,
}

impl Manifest {
    /// 按种类、名称与版本构造空钩子的 Manifest。
    pub fn new(kind: Kind, name: impl Into<String>, version: u16) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            require_version: HashMap::new(),
            license: String::new(),
            build_time: String::new(),
            validate: None,
            on_init: None,
            on_shutdown: None,
            on_flush: None,
            extension: None,
            version,
            kind,
        }
    }
}

impl Default for Manifest {
    fn default() -> Self {
        // Go zero-value Manifest has Kind=0; Audit starts at 1. Use Audit as a
        // stable empty placeholder for declare/export round-trips in tests.
        // Go 零值 Kind=0，而 Audit 从 1 起；测试往返用 Audit 作稳定占位。
        Self::new(Kind::Audit, "", 0)
    }
}

impl Default for AuthenticationManifest {
    fn default() -> Self {
        Self {
            manifest: Manifest::default(),
            authenticate_user: None,
            generate_authentication_string: None,
            validate_authentication_string: None,
            set_salt: None,
        }
    }
}

impl Default for SchemaManifest {
    fn default() -> Self {
        Self {
            manifest: Manifest::default(),
        }
    }
}

impl Default for DaemonManifest {
    fn default() -> Self {
        Self {
            manifest: Manifest::default(),
        }
    }
}

/// 将具体清单类型导出为通用 `Manifest` 的 trait。
pub trait ExportManifest {
    /// 导出底层通用 Manifest（通常克隆内嵌字段）。
    fn export_manifest(&self) -> Manifest;
}

impl ExportManifest for Manifest {
    fn export_manifest(&self) -> Manifest {
        self.clone()
    }
}

/// 对任意实现 `ExportManifest` 的值调用导出。
pub fn export_manifest(manifest: &impl ExportManifest) -> Manifest {
    manifest.export_manifest()
}

/// 认证相关回调的占位函数指针类型。
pub type AuthenticationCallback = Arc<dyn Fn() + Send + Sync>;

/// 认证类插件清单：在通用 Manifest 上附加认证钩子。
#[derive(Clone)]
pub struct AuthenticationManifest {
    pub manifest: Manifest,
    pub authenticate_user: Option<AuthenticationCallback>,
    pub generate_authentication_string: Option<AuthenticationCallback>,
    pub validate_authentication_string: Option<AuthenticationCallback>,
    pub set_salt: Option<AuthenticationCallback>,
}

/// Stored in `Manifest.extension` so authentication manifests retain their
/// callbacks across the same export/declare round-trip as Go's pointer cast.
#[derive(Clone)]
pub struct AuthenticationCallbacks {
    pub authenticate_user: Option<AuthenticationCallback>,
    pub generate_authentication_string: Option<AuthenticationCallback>,
    pub validate_authentication_string: Option<AuthenticationCallback>,
    pub set_salt: Option<AuthenticationCallback>,
}

impl ExportManifest for AuthenticationManifest {
    fn export_manifest(&self) -> Manifest {
        let mut manifest = self.manifest.clone();
        manifest.extension = Some(Arc::new(AuthenticationCallbacks {
            authenticate_user: self.authenticate_user.clone(),
            generate_authentication_string: self.generate_authentication_string.clone(),
            validate_authentication_string: self.validate_authentication_string.clone(),
            set_salt: self.set_salt.clone(),
        }));
        manifest
    }
}

/// Returns authentication callbacks retained by `export_manifest`.
pub fn authentication_callbacks(manifest: &Manifest) -> Option<&AuthenticationCallbacks> {
    manifest
        .extension
        .as_ref()?
        .downcast_ref::<AuthenticationCallbacks>()
}

/// Schema 类插件清单。
#[derive(Clone)]
pub struct SchemaManifest {
    pub manifest: Manifest,
}

impl ExportManifest for SchemaManifest {
    fn export_manifest(&self) -> Manifest {
        self.manifest.clone()
    }
}

/// Daemon（守护进程）类插件清单。
#[derive(Clone)]
pub struct DaemonManifest {
    pub manifest: Manifest,
}

impl ExportManifest for DaemonManifest {
    fn export_manifest(&self) -> Manifest {
        self.manifest.clone()
    }
}
