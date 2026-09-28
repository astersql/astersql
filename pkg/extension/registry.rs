// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 扩展全局注册表（registry）。
//
// 进程内单例收集各扩展的工厂，在 `Setup`/`GetExtensions` 时按名称排序
// 构建 Manifest 列表并组装为 `Extensions`。Setup 完成后禁止再注册；
// `Reset` 用于测试清理全局状态。

use crate::extensions::Extensions;
use crate::manifest::{Option, newManifestWithSetup};
use crate::util::{ClearFunc, ExtensionError, clearFuncBuilder};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

/// 扩展工厂：返回应用于 Manifest 的选项列表。
pub type ExtensionFactory = Arc<dyn Fn() -> Result<Vec<Option>, ExtensionError> + Send + Sync>;

/// 注册表内部可变状态。
#[derive(Default)]
struct registryState {
    factories: HashMap<String, ExtensionFactory>,
    extensionNames: Vec<String>,
    setup: bool,
    extensions: std::option::Option<Arc<Extensions>>,
    close: std::option::Option<ClearFunc>,
}

/// 线程安全的扩展注册表，持有写锁保护的状态。
pub struct registry {
    state: RwLock<registryState>,
}

impl registry {
    /// 创建空注册表。
    fn new() -> Self {
        Self {
            state: RwLock::new(registryState::default()),
        }
    }

    /// 显式触发 Setup：构建全部扩展（若尚未完成）。
    pub fn Setup(&self) -> Result<(), ExtensionError> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::doSetup(&mut state).map(|_| ())
    }

    /// 返回已构建的 Extensions；若尚未 Setup 则先执行 doSetup。
    pub fn Extensions(&self) -> Result<std::option::Option<Arc<Extensions>>, ExtensionError> {
        {
            let state = self
                .state
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.setup {
                return Ok(state.extensions.clone());
            }
        }
        // 未 Setup：升级为写锁并完成构建。
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::doSetup(&mut state)
    }

    /// 在 Setup 之前按名称注册扩展工厂；名称须非空且唯一。
    pub fn RegisterFactory(
        &self,
        name: String,
        factory: ExtensionFactory,
    ) -> Result<(), ExtensionError> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.setup {
            return Err(ExtensionError::new(
                "Cannot register new extension because registry has already been setup",
            ));
        }
        if name.is_empty() {
            return Err(ExtensionError::new("extension name should not be empty"));
        }
        if state.factories.contains_key(&name) {
            return Err(ExtensionError::new(format!(
                "extension with name '{}' already registered",
                name
            )));
        }
        state.factories.insert(name.clone(), factory);
        state.extensionNames.push(name);
        // 按名称排序，保证 Manifest 顺序稳定可测。
        state.extensionNames.sort();
        Ok(())
    }

    /// 实际构建逻辑：按排序后的名称依次 newManifestWithSetup。
    fn doSetup(
        state: &mut registryState,
    ) -> Result<std::option::Option<Arc<Extensions>>, ExtensionError> {
        if state.setup {
            return Ok(state.extensions.clone());
        }
        if state.factories.is_empty() {
            state.extensions = None;
            state.setup = true;
            return Ok(None);
        }

        let mut manifests = Vec::with_capacity(state.factories.len());
        let mut clear_builder = clearFuncBuilder::default();
        for name in &state.extensionNames {
            let factory = Arc::clone(
                state
                    .factories
                    .get(name)
                    .expect("registered extension name must have a factory"),
            );
            match newManifestWithSetup(name.clone(), move || factory()) {
                Ok((manifest, clear)) => {
                    manifests.push(Arc::new(manifest));
                    clear_builder.DoWithCollectClear(move || Ok(Some(clear)))?;
                }
                Err(error) => {
                    // 任一扩展构建失败则回滚已成功注册的资源。
                    clear_builder.Build()();
                    return Err(error);
                }
            }
        }

        let extensions = Arc::new(Extensions::from_manifests(manifests));
        state.extensions = Some(Arc::clone(&extensions));
        state.setup = true;
        state.close = Some(clear_builder.Build());
        Ok(Some(extensions))
    }

    /// 调用关闭清理、清空工厂与状态，供测试复位。
    pub fn Reset(&self) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(close) = state.close.take() {
            close();
        }
        state.factories.clear();
        state.extensionNames.clear();
        state.extensions = None;
        state.setup = false;
    }
}

/// 进程级全局注册表单例。
static globalRegistry: LazyLock<registry> = LazyLock::new(registry::new);

/// 向全局注册表注册扩展工厂。
pub fn RegisterFactory(name: String, factory: ExtensionFactory) -> Result<(), ExtensionError> {
    globalRegistry.RegisterFactory(name, factory)
}

/// 以固定选项列表注册扩展（工厂直接返回这些选项）。
pub fn Register(name: String, options: Vec<Option>) -> Result<(), ExtensionError> {
    RegisterFactory(name, Arc::new(move || Ok(options.clone())))
}

/// 触发全局注册表 Setup。
pub fn Setup() -> Result<(), ExtensionError> {
    globalRegistry.Setup()
}

/// 获取全局已构建的 Extensions（惰性 Setup）。
pub fn GetExtensions() -> Result<std::option::Option<Arc<Extensions>>, ExtensionError> {
    globalRegistry.Extensions()
}

/// 重置全局注册表。
pub fn Reset() {
    globalRegistry.Reset();
}
