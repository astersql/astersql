// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! Task registration ported from `br/pkg/utils/register.go`.
//! 通过 etcd lease 注册 BRIE/Lightning/ImportInto 任务，防止冲突导入。
//! 持续 RegisterTask 维护 keepalive；RegisterTaskOnce 做一次性续约或新建。
//! 客户端抽象为 `EtcdRegisterClient`，便于单测用内存实现替换真实 etcd。
//! etcd key 层级：/tidb/brie/import/<type>/<task>。
//! RegisterTask 适合长任务；Once 适合短生命周期续约。
//! Close 必须等待 wg，否则可能 revoke 与 put 竞态。
//! LeaseNotFound 文案固定，便于 downcast 识别。
//! Grant 响应 error 字段与 RPC 错误是两条失败通道。
//! keepalive 断开不等于任务结束，需尝试重建。
//! time_left_threshold 防止在租约将过期时才动作。
//! always-grant failpoint 把阈值拉满，迫使频繁 regrant。
//! keepalive-stop failpoint 主动 revoke，制造空窗。
//! failed-to-grant/reput 模拟 etcd 抖动，验证重试循环。
//! retry-interval failpoint 把 10s 压到毫秒级加速测试。
//! need_reput_kv 保证新 lease 与 key 重新绑定。
//! put 值为空字符串，任务存在性只看 key+lease。
//! GetImportTasksFrom 跳过 ttl<=0，避免陈旧冲突误报。
//! MessageToUser 供 CLI 提示“已有导入任务”。
//! RegisterTasksList::Empty 是简洁判空。
//! Enable/DisableFailpoint 非线程安全到跨测污染：测后须清理。
//! inject_failpoint 只看 key 存在；value 由 value 版读取。
//! NewTaskRegisterWithTTL 返回 trait 对象，隐藏实现。
//! secondTTL 以秒传给 etcd grant API。
//! curLeaseID 在后台线程与 Close 间共享。
//! cancel child 与父 ctx 分离，Close 只取消注册循环。
//! RegisterTask 克隆 TaskRegisterImpl 给线程，避免 &mut 跨线程。
//! RegisterTaskOnce 已存在 key 时不新建 lease，只续约。
//! keep_alive_once 对应 etcd LeaseKeepAliveOnce RPC。
//! time_to_live 失败且非 LeaseNotFound 则整次列举失败。
//! NO_LEASE 避免误 revoke id=0。
//! defaultTaskRegisterTTL 与 Go 3*time.Minute 一致。
//! RegisterRetryInternal 名称沿用 Go（Internal 拼写）。
//! as_str 保证路径稳定，不随 Debug 格式变化。
//! EtcdRegisterClient 方法签名刻意精简，不含事务。
//! TaskRegister::Close 在 LeaseNotFound 时返回 Ok。
//! keepalive 内层 Timeout 每秒醒来检查 cancel。
//! 外层重建循环在 cancel 时任意失败点可 return。
//! sleep_retry_interval 可被 failpoint 覆盖为短间隔。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::stubs::context::Context;
use astersql_br_pkg_logutil::{Field, log};
use astersql_errors::{Annotate, SharedError, Trace};

/// 为错误附加 Trace，保持与 Go `errors.Trace` 调用点一致。
fn trace_err(err: SharedError) -> SharedError {
    Trace(Some(err)).expect("trace")
}

/// 可注册的任务类型，决定 etcd key 路径中的类型段。
pub enum RegisterTaskType {
    RegisterRestore,
    RegisterLightning,
    RegisterImportInto,
}

impl RegisterTaskType {
    /// 路径片段：restore / lightning / import-into。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RegisterRestore => "restore",
            Self::RegisterLightning => "lightning",
            Self::RegisterImportInto => "import-into",
        }
    }
}

/// etcd 导入任务前缀，与 Go `RegisterImportTaskPrefix` 相同。
pub const RegisterImportTaskPrefix: &str = "/tidb/brie/import";
/// keepalive 重建失败后的默认重试间隔。
pub const RegisterRetryInternal: Duration = Duration::from_secs(10);
/// 未显式指定 TTL 时使用的默认租约（3 分钟）。
const defaultTaskRegisterTTL: Duration = Duration::from_secs(3 * 60);

/// 尚未授予租约时的哨兵 lease id。
pub const NO_LEASE: i64 = 0;

/// etcd KV 条目：含绑定的 lease id。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
    pub lease: i64,
}

/// Grant 响应；`error` 非空表示 etcd 侧逻辑失败。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeaseGrantResponse {
    pub id: i64,
    pub ttl: i64,
    pub error: String,
}

/// Get 响应的 KV 列表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GetResponse {
    pub kvs: Vec<KeyValue>,
}

/// KeepAlive 流中的单次续约回执。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeaseKeepAliveResponse {
    pub id: i64,
    pub ttl: i64,
}

/// TimeToLive 查询结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeaseTimeToLiveResponse {
    pub ttl: i64,
}

/// 对应 etcd `requested lease not found`，Close/列举时视为可忽略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseNotFound;

impl std::fmt::Display for LeaseNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("etcdserver: requested lease not found")
    }
}

impl std::error::Error for LeaseNotFound {}

/// etcd 注册所需的最小客户端面；生产接真实 etcd，测试接 MemEtcd。
pub trait EtcdRegisterClient: Send + Sync {
    fn put(&self, ctx: &Context, key: &str, value: &str, lease_id: i64) -> Result<(), SharedError>;
    fn grant(&self, ctx: &Context, ttl_secs: i64) -> Result<LeaseGrantResponse, SharedError>;
    fn keep_alive(
        &self,
        ctx: &Context,
        lease_id: i64,
    ) -> Result<std::sync::mpsc::Receiver<LeaseKeepAliveResponse>, SharedError>;
    fn keep_alive_once(&self, ctx: &Context, lease_id: i64) -> Result<(), SharedError>;
    fn get(&self, ctx: &Context, key: &str, prefix: bool) -> Result<GetResponse, SharedError>;
    fn revoke(&self, ctx: &Context, lease_id: i64) -> Result<(), SharedError>;
    fn time_to_live(
        &self,
        ctx: &Context,
        lease_id: i64,
    ) -> Result<LeaseTimeToLiveResponse, SharedError>;
}

/// 任务注册器对外接口：持续注册、一次性注册与关闭撤销。
pub trait TaskRegister: Send + Sync {
    fn Close(&mut self, ctx: &Context) -> Result<(), SharedError>;
    fn RegisterTask(&mut self, ctx: Context) -> Result<(), SharedError>;
    fn RegisterTaskOnce(&mut self, ctx: &Context) -> Result<(), SharedError>;
}

/// 内部实现：持有 client、TTL、key 与当前 lease，以及 keepalive 协程计数。
struct TaskRegisterImpl {
    client: Arc<dyn EtcdRegisterClient>,
    ttl: Duration,
    secondTTL: i64,
    key: String,
    curLeaseID: Arc<Mutex<i64>>,
    /// 取消 keepalive 循环的 child context。
    cancel: Mutex<Option<Context>>,
    /// 活跃 keepalive 线程数；Close 等待其归零。
    wg: Arc<AtomicI32>,
}

/// 使用自定义 TTL 构造注册器；key=`prefix/type/task_name`。
pub fn NewTaskRegisterWithTTL(
    client: Arc<dyn EtcdRegisterClient>,
    ttl: Duration,
    tp: RegisterTaskType,
    task_name: &str,
) -> Box<dyn TaskRegister> {
    Box::new(TaskRegisterImpl {
        client,
        ttl,
        secondTTL: ttl.as_secs() as i64,
        // Path::join 在 Unix 上产生 `/tidb/brie/import/<type>/<name>`。
        key: Path::new(RegisterImportTaskPrefix)
            .join(tp.as_str())
            .join(task_name)
            .to_string_lossy()
            .into_owned(),
        curLeaseID: Arc::new(Mutex::new(NO_LEASE)),
        cancel: Mutex::new(None),
        wg: Arc::new(AtomicI32::new(0)),
    })
}

/// 使用默认 3 分钟 TTL 构造注册器。
pub fn NewTaskRegister(
    client: Arc<dyn EtcdRegisterClient>,
    tp: RegisterTaskType,
    task_name: &str,
) -> Box<dyn TaskRegister> {
    NewTaskRegisterWithTTL(client, defaultTaskRegisterTTL, tp, task_name)
}

impl TaskRegisterImpl {
    /// 申请租约；响应内 `error` 字段非空也视为失败。
    fn grant(&self, ctx: &Context) -> Result<LeaseGrantResponse, SharedError> {
        let lease = self.client.grant(ctx, self.secondTTL)?;
        if !lease.error.is_empty() {
            return Err(SharedError::new(std::io::Error::other(lease.error)));
        }
        Ok(lease)
    }

    /// keepalive 主循环：消费通道；断开后按剩余 TTL 决定是否重新 grant/put。
    fn keepalive_loop(
        &self,
        ctx: Context,
        mut ch: std::sync::mpsc::Receiver<LeaseKeepAliveResponse>,
    ) {
        // 剩余 TTL 阈值：默认 ttl/4，但不低于 20s，避免过晚重建。
        let min_time_left_threshold = Duration::from_secs(20);
        let mut time_left_threshold = self.ttl / 4;
        if time_left_threshold < min_time_left_threshold {
            time_left_threshold = min_time_left_threshold;
        }
        // failpoint：强制阈值=ttl，使几乎立即走 re-grant 路径（便于测失败注入）。
        inject_failpoint("brie-task-register-always-grant", || {
            time_left_threshold = self.ttl;
        });
        let mut last_update_time = Instant::now();
        loop {
            // 内层：持续读 keepalive；断开则跳出以重建流。
            loop {
                // failpoint：主动 revoke 当前 lease，模拟 keepalive 中断后的空列表窗口。
                inject_failpoint("brie-task-register-keepalive-stop", || {
                    let lease_id = *self.curLeaseID.lock().expect("curLeaseID poisoned");
                    if let Err(err) = self.client.revoke(&ctx, lease_id) {
                        log::Warn(
                            "brie-task-register-keepalive-stop",
                            [Field::string("error", &err.to_string())],
                        );
                    }
                });
                if ctx.is_cancelled() {
                    return;
                }
                match ch.recv_timeout(Duration::from_secs(1)) {
                    Ok(_) => last_update_time = Instant::now(),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                }
            }
            log::Warn("the keepalive channel is closed, try to recreate it", []);
            let mut need_reput_kv = false;
            loop {
                let time_gap = last_update_time.elapsed();
                // 剩余 TTL 不足阈值：重新 grant，并标记需要 put 绑定新 lease。
                if self.ttl.saturating_sub(time_gap) <= time_left_threshold {
                    let mut grant_err = None;
                    inject_failpoint("brie-task-register-failed-to-grant", || {
                        grant_err =
                            Some(SharedError::new(std::io::Error::other("failpoint-error")));
                    });
                    let lease = if let Some(err) = grant_err {
                        log::Warn(
                            "failed to grant lease",
                            [Field::string("error", &err.to_string())],
                        );
                        if ctx.is_cancelled() {
                            return;
                        }
                        self.sleep_retry_interval();
                        continue;
                    } else {
                        match self.grant(&ctx) {
                            Ok(v) => v,
                            Err(err) => {
                                log::Warn(
                                    "failed to grant lease",
                                    [Field::string("error", &err.to_string())],
                                );
                                if ctx.is_cancelled() {
                                    return;
                                }
                                self.sleep_retry_interval();
                                continue;
                            }
                        }
                    };
                    *self.curLeaseID.lock().expect("curLeaseID poisoned") = lease.id;
                    last_update_time = Instant::now();
                    need_reput_kv = true;
                }
                // 新 lease 必须 put 回同一 key，否则任务对 GetImportTasksFrom 不可见。
                if need_reput_kv {
                    let lease_id = *self.curLeaseID.lock().expect("curLeaseID poisoned");
                    let mut reput_err = None;
                    inject_failpoint("brie-task-register-failed-to-reput", || {
                        reput_err =
                            Some(SharedError::new(std::io::Error::other("failpoint-error")));
                    });
                    if reput_err.is_none() {
                        reput_err = self.client.put(&ctx, &self.key, "", lease_id).err();
                    }
                    if let Some(err) = reput_err {
                        log::Warn(
                            "failed to put new kv",
                            [Field::string("error", &err.to_string())],
                        );
                        if ctx.is_cancelled() {
                            return;
                        }
                        self.sleep_retry_interval();
                        continue;
                    }
                    need_reput_kv = false;
                }
                // 重建 keepalive 流；成功则回到外层读循环。
                let lease_id = *self.curLeaseID.lock().expect("curLeaseID poisoned");
                match self.client.keep_alive(&ctx, lease_id) {
                    Ok(new_ch) => {
                        ch = new_ch;
                        break;
                    }
                    Err(err) => {
                        log::Warn(
                            "failed to create new kv",
                            [Field::string("error", &err.to_string())],
                        );
                        if ctx.is_cancelled() {
                            return;
                        }
                        self.sleep_retry_interval();
                    }
                }
            }
        }
    }

    /// 失败重试睡眠；failpoint 可缩短间隔加速测试。
    fn sleep_retry_interval(&self) {
        let mut interval = RegisterRetryInternal;
        inject_failpoint_value("brie-task-register-retry-interval", |val| {
            if val > 0 {
                interval = Duration::from_millis(val as u64);
            }
        });
        thread::sleep(interval);
    }
}

impl TaskRegister for TaskRegisterImpl {
    /// 取消 keepalive、等待线程结束，并 revoke 当前租约。
    fn Close(&mut self, ctx: &Context) -> Result<(), SharedError> {
        if let Some(cancel) = self.cancel.lock().expect("cancel lock poisoned").take() {
            cancel.cancel();
        }
        // 自旋等待 keepalive 线程退出（wg 归零）。
        while self.wg.load(Ordering::Acquire) > 0 {
            thread::sleep(Duration::from_millis(10));
        }
        let lease_id = *self.curLeaseID.lock().expect("curLeaseID poisoned");
        if lease_id != NO_LEASE {
            if let Err(err) = self.client.revoke(ctx, lease_id) {
                // 租约已不存在视为成功关闭。
                if err.downcast_ref::<LeaseNotFound>().is_some() {
                    return Ok(());
                }
                log::Warn(
                    "failed to revoke the lease",
                    [
                        Field::string("error", &err.to_string()),
                        Field::int("lease-id", lease_id),
                    ],
                );
                return Err(err);
            }
        }
        Ok(())
    }

    /// 持续注册：grant → put → keep_alive，并启动后台 keepalive_loop。
    fn RegisterTask(&mut self, ctx: Context) -> Result<(), SharedError> {
        let child = ctx.child_token();
        *self.cancel.lock().expect("cancel lock poisoned") = Some(child.clone());
        let lease = self
            .grant(&child)
            .map_err(|err| Annotate(Some(err), "failed grant a lease").unwrap())?;
        *self.curLeaseID.lock().expect("curLeaseID poisoned") = lease.id;
        self.client.put(&child, &self.key, "", lease.id)?;
        let ch = self
            .client
            .keep_alive(&child, lease.id)
            .map_err(trace_err)?;
        self.wg.fetch_add(1, Ordering::Relaxed);
        // 克隆一份状态给后台线程；lease id 以 Mutex 共享更新。
        let this = Arc::new(TaskRegisterImpl {
            client: Arc::clone(&self.client),
            ttl: self.ttl,
            secondTTL: self.secondTTL,
            key: self.key.clone(),
            curLeaseID: Arc::clone(&self.curLeaseID),
            cancel: Mutex::new(None),
            wg: Arc::clone(&self.wg),
        });
        thread::spawn(move || {
            this.keepalive_loop(child, ch);
            this.wg.fetch_sub(1, Ordering::Relaxed);
        });
        Ok(())
    }

    /// 一次性注册：key 不存在则 grant+put；已存在则 keep_alive_once 续约。
    fn RegisterTaskOnce(&mut self, ctx: &Context) -> Result<(), SharedError> {
        let resp = self.client.get(ctx, &self.key, false).map_err(trace_err)?;
        if resp.kvs.is_empty() {
            let lease = self
                .grant(ctx)
                .map_err(|err| Annotate(Some(err), "failed grant a lease").unwrap())?;
            *self.curLeaseID.lock().expect("curLeaseID poisoned") = lease.id;
            self.client
                .put(ctx, &self.key, "", lease.id)
                .map_err(trace_err)?;
        } else {
            let lease_id = resp.kvs[0].lease;
            *self.curLeaseID.lock().expect("curLeaseID poisoned") = lease_id;
            self.client
                .keep_alive_once(ctx, lease_id)
                .map_err(trace_err)?;
        }
        Ok(())
    }
}

/// 列举给用户看的单条任务信息。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegisterTask {
    pub Key: String,
    pub LeaseID: i64,
    pub TTL: i64,
}

impl RegisterTask {
    /// 人类可读摘要：lease-id 以十六进制展示。
    pub fn MessageToUser(&self) -> String {
        format!(
            "[ key: {}, lease-id: {:x}, ttl: {}s ]",
            self.Key, self.LeaseID, self.TTL
        )
    }
}

/// 当前有效导入任务列表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegisterTasksList {
    pub Tasks: Vec<RegisterTask>,
}

impl RegisterTasksList {
    /// 按 Go 实现逐项追加 MessageToUser 和尾随 `, `。
    pub fn MessageToUser(&self) -> String {
        let mut message = String::new();
        for task in &self.Tasks {
            message.push_str(&task.MessageToUser());
            message.push_str(", ");
        }
        message
    }

    pub fn Empty(&self) -> bool {
        self.Tasks.is_empty()
    }
}

/// 前缀扫描导入任务，过滤 TTL<=0 或 LeaseNotFound 的条目。
pub fn GetImportTasksFrom(
    ctx: &Context,
    client: Arc<dyn EtcdRegisterClient>,
) -> Result<RegisterTasksList, SharedError> {
    let resp = client
        .get(ctx, RegisterImportTaskPrefix, true)
        .map_err(trace_err)?;
    let mut list = RegisterTasksList {
        Tasks: Vec::with_capacity(resp.kvs.len()),
    };
    for kv in resp.kvs {
        let lease_resp = match client.time_to_live(ctx, kv.lease) {
            Ok(v) => v,
            // 租约已消失：跳过该 KV，不中断整次列举。
            Err(err) if err.downcast_ref::<LeaseNotFound>().is_some() => continue,
            Err(err) => {
                return Err(Annotate(
                    Some(err),
                    format!("failed to get time-to-live of lease: {:x}", kv.lease),
                )
                .unwrap());
            }
        };
        // TTL 已耗尽的条目对冲突检测无意义，直接忽略。
        if lease_resp.ttl <= 0 {
            continue;
        }
        list.Tasks.push(RegisterTask {
            Key: kv.key,
            LeaseID: kv.lease,
            TTL: lease_resp.ttl,
        });
    }
    Ok(list)
}

/// 进程内 failpoint 表；仅测试路径使用。
static FAILPOINTS: OnceLock<Mutex<HashMap<String, i32>>> = OnceLock::new();

fn failpoints() -> &'static Mutex<HashMap<String, i32>> {
    FAILPOINTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Enable a test failpoint. Value `0` means "enabled without payload".
/// 启用命名 failpoint；值为 0 表示仅开关、无载荷。
pub fn EnableFailpoint(name: &str, value: i32) {
    failpoints()
        .lock()
        .expect("failpoints poisoned")
        .insert(name.to_string(), value);
}

/// Disable a test failpoint.
/// 关闭命名 failpoint。
pub fn DisableFailpoint(name: &str) {
    failpoints()
        .lock()
        .expect("failpoints poisoned")
        .remove(name);
}

/// 若 failpoint 已启用则执行闭包（无载荷）。
fn inject_failpoint(name: &str, f: impl FnOnce()) {
    if failpoints()
        .lock()
        .expect("failpoints poisoned")
        .contains_key(name)
    {
        f();
    }
}

/// 若 failpoint 已启用则把其整型载荷传给闭包。
fn inject_failpoint_value(name: &str, f: impl FnOnce(i32)) {
    if let Some(val) = failpoints()
        .lock()
        .expect("failpoints poisoned")
        .get(name)
        .copied()
    {
        f(val);
    }
}
