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

// 已加载扩展集合（Extensions）及其对外查询接口。
//
// 持有按序排列的 Manifest 列表，提供引导、访问检查函数收集、
// 会话扩展工厂与全局鉴权插件映射等能力。

use crate::auth::AuthPlugin;
use crate::manifest::{AccessCheckFunc, BootstrapContext, Manifest};
use crate::session::{SessionExtensions, newSessionExtensions};
use crate::util::ExtensionError;
use std::collections::HashMap;
use std::sync::Arc;

/// 一组已注册扩展的 Manifest 容器。
pub struct Extensions {
    /// 按注册/排序后的 Manifest 列表。
    pub(crate) manifests: Vec<Arc<Manifest>>,
}

impl Extensions {
    /// 由 Manifest 列表构造扩展集合。
    pub fn from_manifests(manifests: Vec<Arc<Manifest>>) -> Self {
        Self { manifests }
    }

    /// 返回当前全部 Manifest 的克隆列表。
    pub fn Manifests(&self) -> Vec<Arc<Manifest>> {
        self.manifests.clone()
    }

    /// 按 Manifest 顺序依次调用各自的 bootstrap 钩子。
    pub fn Bootstrap(&self, context: &mut dyn BootstrapContext) -> Result<(), ExtensionError> {
        for manifest in &self.manifests {
            if let Some(bootstrap) = &manifest.bootstrap {
                bootstrap(context)?;
            }
        }
        Ok(())
    }

    /// 收集各 Manifest 声明的访问检查（access check）回调。
    pub fn GetAccessCheckFuncs(&self) -> Vec<AccessCheckFunc> {
        self.manifests
            .iter()
            .filter_map(|manifest| manifest.accessCheckFunc.clone())
            .collect()
    }

    /// 基于本扩展集合创建会话级扩展句柄。
    pub fn NewSessionExtensions(&self) -> SessionExtensions {
        newSessionExtensions(self)
    }

    /// 汇总全部 Manifest 的鉴权插件，同名后者覆盖前者。
    pub fn GetAuthPlugins(&self) -> HashMap<String, Arc<AuthPlugin>> {
        let mut plugins = HashMap::new();
        for manifest in &self.manifests {
            if let Some(manifest_plugins) = &manifest.authPlugins {
                for plugin in manifest_plugins {
                    plugins.insert(plugin.Name.clone(), Arc::clone(plugin));
                }
            }
        }
        plugins
    }
}

/// 若扩展为空则返回空 Manifest 列表，否则返回其 Manifests。
pub fn manifests_or_empty(extensions: Option<&Extensions>) -> Vec<Arc<Manifest>> {
    extensions.map_or_else(Vec::new, Extensions::Manifests)
}
