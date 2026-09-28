// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 空闲连接监视器：在 Standby/Starter 部署下检测实例长时间无业务流量后请求退出。
//
// 与 Manager（连接池管理器）配合：超过 `max_idle` 且无事务、无交互式连接时，
// 写入正常重启信息并发送退出信号，便于资源回收。

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use astersql_parser_mysql::r#const::ServerStatusInTrans;
use astersql_server::server::Server;

use crate::standby::{ExitSignal, LoadKeyspaceController};

/// MySQL 客户端能力位：交互式连接（CLIENT_INTERACTIVE）。
const CLIENT_INTERACTIVE: u32 = 1 << 10;

#[derive(Clone, Debug)]
/// 空闲监视配置：最大空闲时长、轮询间隔与 starter/零后端开关。
pub struct IdleWatcherConfig {
    /// 允许的最大空闲秒数；为 0 时不启动监视线程。
    pub max_idle: Duration,
    /// 轮询检查间隔。
    pub check_interval: Duration,
    /// 是否处于 starter（按需启动）模式。
    pub starter_mode: bool,
    /// 是否启用零后端（无后端进程）场景下的 Terminate 退出路径。
    pub zero_backend_enabled: bool,
}

/// 默认：不启用空闲退出，每 10 秒检查一次。
impl Default for IdleWatcherConfig {
    fn default() -> Self {
        Self {
            max_idle: Duration::ZERO,
            check_interval: Duration::from_secs(10),
            starter_mode: false,
            zero_backend_enabled: false,
        }
    }
}

/// Start the same idle decision loop used by the Go standby controller. A
/// zero max-idle value disables the watcher and does not create a thread.
/// 启动与 Go standby 控制器相同的空闲判定循环；`max_idle` 为 0 时禁用且不建线程。

pub fn start_idle_watcher(
    controller: LoadKeyspaceController,
    server: Arc<Server>,
    config: IdleWatcherConfig,
) -> Option<JoinHandle<()>> {
    // 未配置空闲阈值则直接返回，避免无意义后台线程。
    if config.max_idle.is_zero() {
        return None;
    }
    // 启动瞬间记一次活跃时间，避免刚启动就被判定为空闲。
    controller.on_connection_active_now();
    Some(
        thread::Builder::new()
            .name("astersql-standby-idle-watcher".into())
            .spawn(move || idle_loop(controller, server, config))
            .expect("idle watcher thread must start"),
    )
}

/// 周期性检查连接/进程/事务与交互式客户端，超时则请求退出。
fn idle_loop(controller: LoadKeyspaceController, server: Arc<Server>, config: IdleWatcherConfig) {
    while !server.force_shutdown() {
        thread::sleep(config.check_interval);
        // 距上次活跃的秒数；未超阈值则继续等待。
        let idle_seconds = unix_seconds().saturating_sub(controller.last_active());
        if idle_seconds <= config.max_idle.as_secs() as i64 {
            continue;
        }

        // 汇总连接数、非 Sleep 进程、进行中事务与交互式客户端。
        let connection_count = server.connection_count();
        let processes = server.user_process_list();
        let process_count = processes
            .values()
            .filter(|process| process.command != "Sleep")
            .count();
        let in_transaction_count = processes
            .values()
            .filter(|process| process.state & ServerStatusInTrans != 0)
            .count();
        let interactive_count = server
            .client_capability_list()
            .values()
            .filter(|capability| **capability & CLIENT_INTERACTIVE != 0)
            .count();

        // 无连接或无可运行进程，且无进行中事务，视为“业务空闲”。
        let idle_without_transaction =
            (connection_count == 0 || process_count == 0) && in_transaction_count == 0;
        // Starter + 零后端：写重启日志并请求 Manager 回收，随后 Terminate。
        if config.starter_mode && config.zero_backend_enabled && idle_without_transaction {
            let _ = controller.save_normal_restart_info("connection idle for too long");
            server.set_need_request_manager_free();
            controller.request_exit(ExitSignal::Terminate);
            break;
        }
        // 普通路径：业务空闲且无交互式客户端时，以 Interrupt 请求退出。
        if idle_without_transaction && interactive_count == 0 {
            let _ = controller.save_normal_restart_info("connection idle for too long");
            controller.request_exit(ExitSignal::Interrupt);
            break;
        }
    }
}

/// 返回当前 Unix 秒时间戳；时钟异常时回退为 0。
fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}
