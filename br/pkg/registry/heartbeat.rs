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

//! Heartbeat updates and background HeartbeatManager (from `heartbeat.go`).
//!
//! 本文件对应 Go `heartbeat.go`：为 restore 任务周期性刷新 `last_heartbeat_time`。
//! 心跳只更新状态供用户观测，不会清理卡住任务；清理由 registry 其他路径负责。
//! Rust 用独立线程 + mpsc 停止信号模拟 Go 的 ticker/context 取消语义。
//! SQL 模板与库表名来自 registration 常量，保证与注册表 schema 一致。
//! 与 Go 差异：Go 挂在 Registry 上用 ticker；Rust 将 Session 以 Arc<Mutex> 注入工作线程。

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::registration::{RestoreRegistryDBName, RestoreRegistryTableName};
use crate::stubs::{Context, Error, Result, Session, SqlValue};

// UPDATE 模板：占位符 `{}` 填库/表名，`%?` 由 Session 绑定时间戳与 restore_id。
pub const UpdateHeartbeatSQLTemplate: &str = "
		UPDATE {}.{}
		SET last_heartbeat_time = FROM_UNIXTIME(%?)
		WHERE id = %?";

// Go 默认心跳间隔 60s；过短会放大元数据写压力。
const defaultHeartbeatIntervalSeconds: u64 = 60;

// 当前 Unix 秒；时钟回拨或异常时退回 0，避免 panic。
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// 仅替换 `{}` 片段；`%?` 留给 ExecuteInternal 参数绑定，避免手工拼接注入。
fn render_sql(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut parts = template.split("{}");
    let Some(first) = parts.next() else {
        return template.to_string();
    };
    out.push_str(first);
    for (idx, part) in parts.enumerate() {
        if let Some(arg) = args.get(idx) {
            out.push_str(arg);
        }
        out.push_str(part);
    }
    out
}

/// Updates `last_heartbeat_time` for a restore task via the heartbeat session.
/// 单次心跳写入：用当前 Unix 时间更新指定 restore_id 行。
pub fn update_heartbeat(session: &mut dyn Session, _ctx: &Context, restore_id: u64) -> Result<()> {
    let current_time = unix_now();
    let update_sql = render_sql(
        UpdateHeartbeatSQLTemplate,
        &[RestoreRegistryDBName, RestoreRegistryTableName],
    );
    session
        .ExecuteInternal(
            _ctx,
            &update_sql,
            &[SqlValue::I64(current_time), SqlValue::U64(restore_id)],
        )
        .map_err(|err| {
            // 保留底层错误，并标注任务 id，便于排查哪条 restore 心跳失败。
            Error::Annotatef(
                err,
                format!("failed to update heartbeat for task {restore_id}"),
            )
        })?;
    Ok(())
}

/// Handles periodic heartbeat updates for a restore task.
///
/// It only updates the restore task and will not remove stalled tasks; the purpose
/// is to provide insights to users about task status.
/// 后台心跳管理器：持有停止通道与 join handle；Drop 时自动 Stop。
pub struct HeartbeatManager {
    session: Option<Arc<Mutex<Box<dyn Session>>>>,
    ctx: Option<Context>,
    restore_id: u64,
    interval: Duration,
    // 向工作线程发送停止信号；None 表示尚未 Start 或已 Stop。
    stop_tx: Option<mpsc::Sender<()>>,
    // 后台线程句柄；Stop/Drop 时 join，避免悬挂线程。
    join: Option<JoinHandle<()>>,
}

impl HeartbeatManager {
    /// Creates a manager; call [`Start`](Self::Start) to spawn the background loop.
    /// 仅构造空壳；真正拉起循环需调用 Start 或 NewHeartbeatManager。
    pub fn new() -> Self {
        Self {
            session: None,
            ctx: None,
            restore_id: 0,
            interval: default_interval(),
            stop_tx: None,
            join: None,
        }
    }
}

impl Default for HeartbeatManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Creates a new heartbeat manager for the given restore task (Go `NewHeartbeatManager`).
/// 对齐 Go 构造函数：只保存任务配置，由调用方显式调用 `Start`。
pub fn NewHeartbeatManager(
    session: Arc<Mutex<Box<dyn Session>>>,
    ctx: Context,
    restore_id: u64,
) -> HeartbeatManager {
    HeartbeatManager {
        session: Some(session),
        ctx: Some(ctx),
        restore_id,
        interval: default_interval(),
        stop_tx: None,
        join: None,
    }
}

// 默认间隔封装为 Duration，供 NewHeartbeatManager 与可测 Start 复用。
fn default_interval() -> Duration {
    Duration::from_secs(defaultHeartbeatIntervalSeconds)
}

impl HeartbeatManager {
    /// Begins the heartbeat background process.
    /// 启动后台循环：先打一枪初始心跳，再按 interval 周期刷新，直到 stop/ctx 取消。
    pub fn Start(&mut self) {
        if self.join.is_some() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(ctx) = self.ctx.clone() else {
            return;
        };
        let restore_id = self.restore_id;
        let interval = self.interval;
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            // Go creates the ticker before the initial heartbeat. Keep the same
            // fixed-rate deadline so a tick that elapses during a slow write is
            // ready as soon as that write returns.
            let mut next_tick = Instant::now() + interval;

            // initial heartbeat
            // 与 Go 一致：Start 后立刻更新一次，避免首个 interval 内状态空白。
            {
                let mut guard = session.lock().unwrap();
                if update_heartbeat(&mut **guard, &ctx, restore_id).is_err() {
                    // 与 Go 一致，单次写入失败不终止后台循环；日志后端由集成层提供。
                }
            }

            loop {
                // Wait for the next fixed-rate tick, but wake early on stop / ctx cancel.
                // 用短超时切片轮询，以便及时响应 stop 与 Context.Done。
                loop {
                    if ctx.Done() {
                        return;
                    }
                    // 剩余等待时间；到期则跳出内层去打心跳。
                    let remaining = next_tick.saturating_duration_since(Instant::now());
                    // 最多睡 50ms，兼顾取消延迟与 CPU 占用。
                    let slice = remaining.min(Duration::from_millis(50));
                    match stop_rx.recv_timeout(slice) {
                        // 显式停止或发送端丢弃，均视为退出。
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                        Err(RecvTimeoutError::Timeout) if remaining.is_zero() => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }

                // Go's ticker channel retains at most one elapsed tick. Advance
                // to the first future boundary before executing it, so slow
                // writes cause one immediate catch-up rather than a burst.
                let now = Instant::now();
                while next_tick <= now {
                    next_tick += interval;
                }
                // 周期心跳失败同样吞掉错误，仅尽力刷新时间戳。
                let mut guard = session.lock().unwrap();
                let _ = update_heartbeat(&mut **guard, &ctx, restore_id);
            }
        });
        self.stop_tx = Some(stop_tx);
        self.join = Some(handle);
    }

    /// Ends the heartbeat background process and waits for the worker to exit.
    /// 发送停止信号并 join，保证资源在调用返回前释放。
    pub fn Stop(&mut self) {
        // take 保证可重入：第二次 Stop 为空操作。
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        // 等待线程退出，确保不再持有 Session 锁。
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for HeartbeatManager {
    fn drop(&mut self) {
        // RAII：忘记显式 Stop 时也要停掉后台线程，防止泄漏。
        self.Stop();
    }
}

#[cfg(test)]
#[path = "heartbeat_test.rs"]
mod heartbeat_test;
