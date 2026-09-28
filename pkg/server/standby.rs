// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Standby（热备）生命周期钩子：延迟监听激活与关闭协调。
//
// Standby 实例可在收到激活信号前暂不绑定 TiDB 监听端口；激活 API
// 可通过 status router 暴露。NoopStandbyController 为默认空实现。

use std::sync::Arc;

use crate::http_status::Router;

/// Capabilities exposed while a standby instance is being activated.
/// Standby 激活阶段所需的服务器能力（初始化监听）。
pub trait StandbyReadyServer: Send + Sync {
    /// 初始化 TiDB MySQL 协议监听端口。
    fn init_tidb_listener(&self) -> Result<(), String>;
}

/// Capabilities used by standby shutdown coordination.
/// Standby 关闭协调所需的服务器能力。
pub trait StandbyShutdownServer: Send + Sync {
    /// Server 是否仍可接受重复激活请求。
    fn health(&self) -> bool {
        true
    }
    /// 查询指定连接是否由 Server 记录为正常关闭。
    fn normal_closed_connection(&self, _keyspace: &str, _connection_id: &str) -> Option<String> {
        None
    }
    /// 关闭自动分配 ID（AutoID）服务。
    fn auto_id_service_close(&self);
    /// 是否已进入强制关闭。
    fn force_shutdown(&self) -> bool;
    /// 是否需要请求 manager 释放资源。
    fn need_request_manager_free(&self) -> bool;
    /// 本实例是否为 AutoID owner。
    fn is_auto_id_owner(&self) -> bool;
    /// 标记强制关闭。
    fn set_force_shutdown(&self);
    /// 标记需要请求 manager 释放。
    fn set_need_request_manager_free(&self);
    /// 阻塞直到活跃连接数为 0。
    fn wait_zero_connections(&self);
    /// Wait until zero connections or `timeout` elapses. Returns true when drained.
    /// 在超时内等待连接排空；排空返回 true。
    fn wait_zero_connections_timeout(&self, timeout: std::time::Duration) -> bool;
}

/// Standby lifecycle hooks. Implementations can delay listener activation and
/// install a small activation API in the status router.
/// Standby 生命周期钩子：可延迟监听激活并在 status router 安装激活 API。
pub trait StandbyController: Send + Sync {
    /// 阻塞直至收到激活信号。
    fn wait_for_activate(&self);
    /// 结束 standby，上报准备结果。
    fn end_standby(&self, result: Result<(), String>);
    /// 可选的激活 HTTP 路由（路径 + Router）。
    ///
    /// Go passes the live `Server` to `Handler`; controllers use its shutdown
    /// state to reject activation after the server becomes unhealthy.
    fn handler(&self, server: Arc<dyn StandbyShutdownServer>) -> Option<(String, Router)>;
    /// 有客户端连接变为活跃时回调。
    fn on_connection_active(&self);
    /// 激活前准备（如初始化监听）。
    fn prepare_for_activation(&self, server: &dyn StandbyReadyServer) -> Result<(), String>;
    /// Server 创建完成后回调。
    fn on_server_created(&self, server: &dyn StandbyReadyServer);
    /// Server 关闭流程中回调。
    fn on_server_shutdown(&self, server: &dyn StandbyShutdownServer);
}

#[derive(Default)]
/// 空操作 StandbyController：激活时直接 init_tidb_listener。
pub struct NoopStandbyController;

impl StandbyController for NoopStandbyController {
    fn wait_for_activate(&self) {}

    fn end_standby(&self, _result: Result<(), String>) {}

    fn handler(&self, _server: Arc<dyn StandbyShutdownServer>) -> Option<(String, Router)> {
        None
    }

    fn on_connection_active(&self) {}

    // 默认路径：立即打开监听，不等待外部激活信号。
    fn prepare_for_activation(&self, server: &dyn StandbyReadyServer) -> Result<(), String> {
        server.init_tidb_listener()
    }

    fn on_server_created(&self, _server: &dyn StandbyReadyServer) {}

    fn on_server_shutdown(&self, _server: &dyn StandbyShutdownServer) {}
}

/// 返回默认 NoopStandbyController 的 Arc。
pub fn noop_standby_controller() -> Arc<dyn StandbyController> {
    Arc::new(NoopStandbyController)
}
