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

//! Go-equivalent tests for `br/pkg/utils/register_test.go`.
//!
//! Etcd boundary is mocked via `EtcdRegisterClient` (no real etcd / grpcio).
//! 用内存 MemEtcd 验证持续注册、一次性续约，以及 grant/reput 失败后的恢复路径。
//! MemEtcd 不模拟租约自动过期计时，TTL 由测试手写。
//! next_lease 从 1 起，避开 NO_LEASE=0。
//! keep_alive 保留 tx 以便将来扩展主动推送。
//! revoke 同步删 KV，贴近 etcd 租约绑定删除语义。
//! get(prefix=true) 用 starts_with，足够覆盖本前缀。
//! test_task_register 校验 key 拼装与非空列表。
//! MessageToUser 调用仅防 panic，不比对文案。
//! test_task_register_once 核心是 TTL 刷新与 lease 稳定。
//! import-into 路径段验证 RegisterImportInto.as_str。
//! failed_grant 先制造空窗，再验证自愈。
//! failed_reput 验证 put 失败不会永久卡死注册。
//! 轮询 5s/50ms 平衡 CI 速度与抖动。
//! 测试结束必须 DisableFailpoint，避免污染其它用例。
//! TTL=3s 较短，配合 always-grant 快速进入重建。
//! retry-interval=200ms 缩短失败自旋。
//! Close 在断言后调用，确保资源回收。
//! Arc<dyn EtcdRegisterClient> 强制走 trait 分发。
//! store.lock 短暂持有后 drop，再改 leases，避免死锁。
//! keep_alive_once 的 max(10) 与衰减到 7 形成对比。
//! 列表空断言确认 revoke 真正删除了可见任务。
//! 恢复后 key 仍应为 restore/test。
//! Enable 多个 failpoint 的顺序不影响语义。
//! Disable 先停 stop 再停失败注入，避免竞态窗口过窄。
//! Instant 截止等待比固定 sleep 更稳。
//! RegisterTask 成功不代表 keepalive 永不失败。
//! 本文件不启动真实网络，适合单测并行。
//! put 覆盖写保证 regrant 后 key 指向新 lease。
//! time_to_live 未知 id 返回 0，与列举过滤配合。
//! grant 的 error 字段恒空，错误改由 Result 表达。
//! 通道首帧避免注册瞬间被判定 Disconnected。
//! AtomicI64 保证并发 grant 的 id 唯一。
//! Mutex<HashMap> 足够单测强度，无需 DashMap。
//! 失败场景使用短 TTL 以加快 always-grant 触发。
//! expect 文案便于定位哪一步失败。
//! 两个失败测共享相似编排，分别注入不同 failpoint。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use astersql_errors::SharedError;

use crate::register::{
    DisableFailpoint, EnableFailpoint, EtcdRegisterClient, GetImportTasksFrom, GetResponse,
    KeyValue, LeaseGrantResponse, LeaseKeepAliveResponse, LeaseTimeToLiveResponse,
    NewTaskRegisterWithTTL, RegisterTask, RegisterTaskType, RegisterTasksList,
};
use crate::stubs::context::Context;

/// 内存 etcd：KV + lease TTL + 可选 keepalive sender，覆盖注册器全部客户端调用。
struct MemEtcd {
    next_lease: AtomicI64,
    store: Mutex<HashMap<String, KeyValue>>,
    /// lease id → 剩余/授予的 ttl 秒。
    leases: Mutex<HashMap<i64, i64>>, // id -> ttl
    /// 活跃 keep_alive 通道，供测试侧主动断开或投递。
    keepalive_tx: Mutex<HashMap<i64, mpsc::Sender<LeaseKeepAliveResponse>>>,
}

impl MemEtcd {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            next_lease: AtomicI64::new(1),
            store: Mutex::new(HashMap::new()),
            leases: Mutex::new(HashMap::new()),
            keepalive_tx: Mutex::new(HashMap::new()),
        })
    }
}

impl EtcdRegisterClient for MemEtcd {
    fn put(
        &self,
        _ctx: &Context,
        key: &str,
        value: &str,
        lease_id: i64,
    ) -> Result<(), SharedError> {
        // 覆盖写：同一 key 绑定新 lease 时替换旧条目。
        self.store.lock().unwrap().insert(
            key.to_string(),
            KeyValue {
                key: key.to_string(),
                value: value.to_string(),
                lease: lease_id,
            },
        );
        Ok(())
    }

    fn grant(&self, _ctx: &Context, ttl_secs: i64) -> Result<LeaseGrantResponse, SharedError> {
        // 单调递增 lease id，写入 TTL 表。
        let id = self.next_lease.fetch_add(1, Ordering::SeqCst);
        self.leases.lock().unwrap().insert(id, ttl_secs);
        Ok(LeaseGrantResponse {
            id,
            ttl: ttl_secs,
            error: String::new(),
        })
    }

    fn keep_alive(
        &self,
        _ctx: &Context,
        lease_id: i64,
    ) -> Result<mpsc::Receiver<LeaseKeepAliveResponse>, SharedError> {
        let (tx, rx) = mpsc::channel();
        // One keepalive tick so the loop has something to receive.
        // 先投递一帧，避免注册刚启动时通道空闲被误判为超时-only。
        let _ = tx.send(LeaseKeepAliveResponse {
            id: lease_id,
            ttl: self
                .leases
                .lock()
                .unwrap()
                .get(&lease_id)
                .copied()
                .unwrap_or(0),
        });
        self.keepalive_tx.lock().unwrap().insert(lease_id, tx);
        Ok(rx)
    }

    fn keep_alive_once(&self, _ctx: &Context, lease_id: i64) -> Result<(), SharedError> {
        let mut leases = self.leases.lock().unwrap();
        if let Some(ttl) = leases.get_mut(&lease_id) {
            // Refresh toward original grant size for "once" semantics in tests.
            // 一次性续约：至少抬到 10s，便于断言 TTL 增大。
            *ttl = (*ttl).max(10);
        }
        Ok(())
    }

    fn get(&self, _ctx: &Context, key: &str, prefix: bool) -> Result<GetResponse, SharedError> {
        let store = self.store.lock().unwrap();
        let kvs = if prefix {
            // 前缀扫描：匹配 GetImportTasksFrom 的列举语义。
            store
                .values()
                .filter(|kv| kv.key.starts_with(key))
                .cloned()
                .collect()
        } else {
            store.get(key).cloned().into_iter().collect()
        };
        Ok(GetResponse { kvs })
    }

    fn revoke(&self, _ctx: &Context, lease_id: i64) -> Result<(), SharedError> {
        // 删除租约及其绑定 KV，并丢弃 keepalive sender。
        self.leases.lock().unwrap().remove(&lease_id);
        let mut store = self.store.lock().unwrap();
        store.retain(|_, kv| kv.lease != lease_id);
        self.keepalive_tx.lock().unwrap().remove(&lease_id);
        Ok(())
    }

    fn time_to_live(
        &self,
        _ctx: &Context,
        lease_id: i64,
    ) -> Result<LeaseTimeToLiveResponse, SharedError> {
        // 未知 lease 返回 ttl=0，列举侧会跳过。
        let ttl = self
            .leases
            .lock()
            .unwrap()
            .get(&lease_id)
            .copied()
            .unwrap_or(0);
        Ok(LeaseTimeToLiveResponse { ttl })
    }
}

#[test]
fn test_register_tasks_list_message_to_user_matches_go() {
    let list = RegisterTasksList {
        Tasks: vec![
            RegisterTask {
                Key: "/tidb/brie/import/restore/first".to_string(),
                LeaseID: 0x12,
                TTL: 30,
            },
            RegisterTask {
                Key: "/tidb/brie/import/lightning/second".to_string(),
                LeaseID: 0x2a,
                TTL: 60,
            },
        ],
    };

    assert_eq!(
        list.MessageToUser(),
        "[ key: /tidb/brie/import/restore/first, lease-id: 12, ttl: 30s ], [ key: /tidb/brie/import/lightning/second, lease-id: 2a, ttl: 60s ], "
    );
}

#[test]
fn test_task_register() {
    // 持续注册后应能列举到 restore/test，Close 正常撤销。
    let client = MemEtcd::new();
    let ctx = Context::new();
    let mut register = NewTaskRegisterWithTTL(
        Arc::clone(&client) as Arc<dyn EtcdRegisterClient>,
        Duration::from_secs(10),
        RegisterTaskType::RegisterRestore,
        "test",
    );
    register.RegisterTask(ctx.clone()).expect("register");

    let list = GetImportTasksFrom(&ctx, Arc::clone(&client) as _).expect("list");
    for task in &list.Tasks {
        let _ = task.MessageToUser();
        assert_eq!(task.Key, "/tidb/brie/import/restore/test");
    }
    assert!(!list.Tasks.is_empty());
    register.Close(&ctx).expect("close");
}

#[test]
fn test_task_register_once() {
    // 首次 Once 建 key；模拟 TTL 衰减后再 Once，lease 不变但 TTL 应刷新变大。
    let client = MemEtcd::new();
    let ctx = Context::new();
    let mut register = NewTaskRegisterWithTTL(
        Arc::clone(&client) as Arc<dyn EtcdRegisterClient>,
        Duration::from_secs(10),
        RegisterTaskType::RegisterImportInto,
        "test",
    );

    register.RegisterTaskOnce(&ctx).expect("once");
    // Simulate TTL decay after grant.
    // 人为把 TTL 降到 7，制造“续约前更小”的基线。
    {
        let store = client.store.lock().unwrap();
        let lease = store.values().next().unwrap().lease;
        drop(store);
        client.leases.lock().unwrap().insert(lease, 7);
    }
    let list = GetImportTasksFrom(&ctx, Arc::clone(&client) as _).expect("list");
    assert_eq!(list.Tasks.len(), 1);
    let curr = list.Tasks[0].clone();
    assert_eq!(curr.Key, "/tidb/brie/import/import-into/test");

    register.RegisterTaskOnce(&ctx).expect("once refresh");
    let list = GetImportTasksFrom(&ctx, Arc::clone(&client) as _).expect("list2");
    assert_eq!(list.Tasks.len(), 1);
    let this = &list.Tasks[0];
    assert_eq!(curr.Key, this.Key);
    assert_eq!(curr.LeaseID, this.LeaseID);
    assert!(
        this.TTL > curr.TTL,
        "ttl refresh {} vs {}",
        this.TTL,
        curr.TTL
    );

    register.Close(&ctx).expect("close");
}

#[test]
fn test_task_register_failed_grant() {
    // grant 失败 + keepalive-stop：任务短暂消失；关闭 stop/grant 失败点后应重新出现。
    let client = MemEtcd::new();
    let ctx = Context::new();
    let mut register = NewTaskRegisterWithTTL(
        Arc::clone(&client) as Arc<dyn EtcdRegisterClient>,
        Duration::from_secs(3),
        RegisterTaskType::RegisterRestore,
        "test",
    );

    // always-grant 迫使阈值=ttl；keepalive-stop 主动 revoke；缩短重试间隔。
    EnableFailpoint("brie-task-register-failed-to-grant", 0);
    EnableFailpoint("brie-task-register-always-grant", 0);
    EnableFailpoint("brie-task-register-keepalive-stop", 0);
    EnableFailpoint("brie-task-register-retry-interval", 200);

    register.RegisterTask(ctx.clone()).expect("register");

    // 等待 revoke 生效，列表应变空。
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        let list = GetImportTasksFrom(&ctx, Arc::clone(&client) as _).expect("list");
        if list.Tasks.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let list = GetImportTasksFrom(&ctx, Arc::clone(&client) as _).expect("list");
    assert!(
        list.Tasks.is_empty(),
        "expected empty after keepalive-stop revoke"
    );

    // 放开 stop/grant 失败，允许重建成功。
    DisableFailpoint("brie-task-register-keepalive-stop");
    DisableFailpoint("brie-task-register-failed-to-grant");

    let start = Instant::now();
    let mut list = None;
    while start.elapsed() < Duration::from_secs(5) {
        let cur = GetImportTasksFrom(&ctx, Arc::clone(&client) as _).expect("list");
        if !cur.Tasks.is_empty() {
            list = Some(cur);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let list = list.expect("task should reappear");
    for task in &list.Tasks {
        assert_eq!(task.Key, "/tidb/brie/import/restore/test");
    }

    DisableFailpoint("brie-task-register-always-grant");
    DisableFailpoint("brie-task-register-retry-interval");
    register.Close(&ctx).expect("close");
    assert!(
        !ctx.is_cancelled(),
        "Close must only cancel the child context"
    );
    assert!(
        GetImportTasksFrom(&ctx, Arc::clone(&client) as _)
            .unwrap()
            .Tasks
            .is_empty(),
        "Close must revoke the lease created by the keepalive loop"
    );
}

#[test]
fn test_task_register_failed_reput() {
    // reput 失败路径与 grant 失败类似：先空列表，禁用失败点后任务应恢复。
    let client = MemEtcd::new();
    let ctx = Context::new();
    let mut register = NewTaskRegisterWithTTL(
        Arc::clone(&client) as Arc<dyn EtcdRegisterClient>,
        Duration::from_secs(3),
        RegisterTaskType::RegisterRestore,
        "test",
    );

    EnableFailpoint("brie-task-register-failed-to-reput", 0);
    EnableFailpoint("brie-task-register-always-grant", 0);
    EnableFailpoint("brie-task-register-keepalive-stop", 0);
    EnableFailpoint("brie-task-register-retry-interval", 200);

    register.RegisterTask(ctx.clone()).expect("register");

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        let list = GetImportTasksFrom(&ctx, Arc::clone(&client) as _).expect("list");
        if list.Tasks.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        GetImportTasksFrom(&ctx, Arc::clone(&client) as _)
            .unwrap()
            .Tasks
            .is_empty()
    );

    DisableFailpoint("brie-task-register-keepalive-stop");
    DisableFailpoint("brie-task-register-failed-to-reput");

    // 恢复后应重新 put 成功并再次可见。
    let start = Instant::now();
    let mut list = None;
    while start.elapsed() < Duration::from_secs(5) {
        let cur = GetImportTasksFrom(&ctx, Arc::clone(&client) as _).expect("list");
        if !cur.Tasks.is_empty() {
            list = Some(cur);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let list = list.expect("task should reappear");
    for task in &list.Tasks {
        assert_eq!(task.Key, "/tidb/brie/import/restore/test");
    }

    DisableFailpoint("brie-task-register-always-grant");
    DisableFailpoint("brie-task-register-retry-interval");
    register.Close(&ctx).expect("close");
    assert!(
        !ctx.is_cancelled(),
        "Close must only cancel the child context"
    );
    assert!(
        GetImportTasksFrom(&ctx, Arc::clone(&client) as _)
            .unwrap()
            .Tasks
            .is_empty(),
        "Close must revoke the lease created by the keepalive loop"
    );
}
