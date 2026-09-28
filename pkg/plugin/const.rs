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

// 插件子系统的种类与生命周期状态常量。
//
// `Kind` 区分审计、认证、Schema、守护类插件；`State` 描述插件从初始化到就绪、
// 退出或禁用的状态机（对应 Go `plugin.Kind` / `plugin.State`）。

/// 插件种类：决定加载路径与可用 SPI（服务提供接口）回调集合。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// 审计插件：监听连接与 SQL 通用事件等。
    Audit = 1,
    /// 认证插件：参与用户身份校验与 salt 等。
    Authentication,
    /// Schema 插件：扩展元数据/表结构相关能力。
    Schema,
    /// 守护类插件：长期后台任务。
    Daemon,
}

impl Kind {
    /// 返回与 Go 字符串化一致的种类名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Audit => "Audit",
            Self::Authentication => "Authentication",
            Self::Schema => "Schema",
            Self::Daemon => "Daemon",
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 插件生命周期状态。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// 尚未完成初始化。
    #[default]
    Uninitialized,
    /// 已就绪，可接受调用。
    Ready,
    /// 正在退出/销毁过程中。
    Dying,
    /// 已禁用，不参与分发。
    Disable,
}

impl State {
    /// 返回与 Go 字符串化一致的状态名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Uninitialized => "Uninitialized",
            Self::Ready => "Ready",
            Self::Dying => "Dying",
            Self::Disable => "Disable",
        }
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
