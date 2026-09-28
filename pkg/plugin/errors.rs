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

// 插件子系统错误类型定义。
//
// 将加载、校验、版本检查、查找与后端失败等场景归类为 `PluginErrorKind`，
// 并通过带消息的 `PluginError` 对外传播（对应 Go 侧插件错误码与文案）。

use std::fmt;

/// 插件错误分类，对应各类校验与运行时失败原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginErrorKind {
    /// 插件 ID（名称-版本）格式非法。
    InvalidPluginId,
    /// 清单（manifest）内容不合法。
    InvalidPluginManifest,
    /// 插件名称非法。
    InvalidPluginName,
    /// 插件版本非法。
    InvalidPluginVersion,
    /// 同名/同标识插件重复注册。
    DuplicatePlugin,
    /// 环境/依赖版本要求检查失败。
    RequiredVersionCheckFailed,
    /// 按名称或签名未找到插件。
    PluginNotFound,
    /// 插件尚未进入 Ready 状态。
    PluginNotReady,
    /// 不支持 Flush 操作。
    FlushUnsupported,
    /// 后端或其它通用失败。
    Backend,
}

/// 携带分类与可读消息的插件错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginError {
    /// 错误种类。
    pub kind: PluginErrorKind,
    /// 面向调用方的错误说明。
    pub message: String,
}

impl PluginError {
    /// 构造指定种类与消息的错误。
    pub fn new(kind: PluginErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// 快捷构造 Backend 类错误。
    pub fn backend(message: impl Into<String>) -> Self {
        Self::new(PluginErrorKind::Backend, message)
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PluginError {}
