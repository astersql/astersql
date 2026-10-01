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

// OwnerManager 集成测试：嵌入式 etcd 上竞选、强制接管、OpValue CAS、Watch 与分布式锁。
//
// 通过 `go run etcd_helper.go` 拉起进程内 etcd，覆盖与 Go `manager_test.go` 对齐的场景。

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use etcd_client::{Client, ConnectOptions, DeleteOptions};
use owner::{
    AcquireDistributedLock, Context, GetOwnerKeyInfo, GetOwnerOpValue, Listener, Manager,
    NewListenersWrapper, NewMockManager, NewOwnerManager, OpType, OwnerError, SetManagerSessionTTL,
    SetWaitTimeOnForceOwner, WatchOwnerForTest,
};
use serial_test::serial;

/// Go `RunWithRetry` 语义：可重试错误按线性退避后再次执行。
#[tokio::test]
async fn test_distributed_lock_retries_transient_errors() {
    let attempts = AtomicU64::new(0);
    crate::manager::retry_lock_operation(3, Duration::from_millis(1), || async {
        let attempt = attempts.fetch_add(1, Ordering::SeqCst);
        if attempt < 2 { Err("transient") } else { Ok(()) }
    })
    .await
    .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

/// 进程内嵌入式 etcd 的 endpoint 与子进程句柄。
struct EmbeddedEtcd {
    endpoint: String,
    _child: Mutex<Child>,
}

static EMBEDDED_ETCD: OnceLock<EmbeddedEtcd> = OnceLock::new();

/// 懒启动 go etcd_helper，读取 READY 行得到 endpoint。
fn embedded_etcd() -> &'static EmbeddedEtcd {
    EMBEDDED_ETCD.get_or_init(|| {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .ancestors()
            .find(|path| path.join("go.mod").is_file())
            .expect("find repository root containing go.mod");
        let helper = manifest_dir.join("etcd_helper.go");
        let mut child = Command::new("go")
            .arg("run")
            .arg(helper)
            .current_dir(repo_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start embedded-etcd helper");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .expect("read embedded-etcd readiness");
        let endpoint = line
            .trim()
            .strip_prefix("READY ")
            .unwrap_or_else(|| panic!("embedded-etcd startup failed: {line}"))
            .to_owned();
        EmbeddedEtcd {
            endpoint,
            _child: Mutex::new(child),
        }
    })
}

/// 连接嵌入式 etcd。
async fn new_client() -> Client {
    let options = ConnectOptions::new()
        .with_connect_timeout(Duration::from_secs(5))
        .with_timeout(Duration::from_secs(5));
    Client::connect([embedded_etcd().endpoint.clone()], Some(options))
        .await
        .expect("connect to embedded etcd")
}

/// 按前缀清理测试路径下的全部 key。
async fn clear_path(client: &mut Client, path: &str) {
    client
        .delete(path, Some(DeleteOptions::new().with_prefix()))
        .await
        .unwrap();
}

/// 轮询直至 manager.IsOwner() 达到期望值。
async fn wait_owner(manager: &Arc<dyn Manager>, expected: bool) {
    tokio::time::timeout(Duration::from_secs(12), async {
        while manager.IsOwner() != expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("manager {} did not reach owner={expected}", manager.ID()));
}

/// 删除当前 leader 的 Owner key，触发退位。
async fn delete_leader(client: &mut Client, path: &str, manager: &Arc<dyn Manager>) {
    let ctx = Context::new();
    let (owner_key, _) = GetOwnerKeyInfo(&ctx, client.clone(), path, &manager.ID())
        .await
        .unwrap();
    client.delete(owner_key, None).await.unwrap();
}

/// 记录 become/retire 事件的测试 Listener。
#[derive(Default)]
struct RecordingListener {
    value: AtomicBool,
    events: Mutex<Vec<&'static str>>,
}

impl RecordingListener {
    /// 已记录事件数。
    fn event_count(&self) -> usize {
        self.events.lock().unwrap().len()
    }
}

impl Listener for RecordingListener {
    fn OnBecomeOwner(&self) {
        self.value.store(true, Ordering::SeqCst);
        self.events.lock().unwrap().push("become");
    }

    fn OnRetireOwner(&self) {
        self.value.store(false, Ordering::SeqCst);
        self.events.lock().unwrap().push("retire");
    }
}

/// 等待 Listener 累计至少 count 条事件。
async fn wait_events(listener: &RecordingListener, count: usize) {
    tokio::time::timeout(Duration::from_secs(12), async {
        while listener.event_count() < count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("listener did not record {count} events"));
}

/// RAII：临时改写 ManagerSessionTTL，Drop 时恢复。
struct TtlGuard(i32);

impl TtlGuard {
    fn set(ttl: i32) -> Self {
        let previous = owner::ManagerSessionTTL();
        SetManagerSessionTTL(ttl);
        Self(previous)
    }
}

impl Drop for TtlGuard {
    fn drop(&mut self) {
        SetManagerSessionTTL(self.0);
    }
}

/// RAII：临时改写 ForceToBeOwner 等待时间。
struct ForceWaitGuard(Duration);

impl ForceWaitGuard {
    fn set(wait: Duration) -> Self {
        let previous = owner::WaitTimeOnForceOwner();
        SetWaitTimeOnForceOwner(wait);
        Self(previous)
    }
}

impl Drop for ForceWaitGuard {
    fn drop(&mut self) {
        SetWaitTimeOnForceOwner(self.0);
    }
}

/// 强制接管：清除残留 key 后竞选成为 Owner。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_force_to_be_owner() {
    let path = "/task-547/force-owner";
    let stale_key = format!("{path}/a");
    let mut client = new_client().await;
    clear_path(&mut client, path).await;
    client.put(stale_key.as_str(), "a", None).await.unwrap();
    assert_eq!(
        client
            .get(stale_key.as_str(), None)
            .await
            .unwrap()
            .kvs()
            .len(),
        1
    );

    let _wait = ForceWaitGuard::set(Duration::from_millis(1));
    let manager = NewOwnerManager(Context::new(), client.clone(), "ddl", "1", path);
    let listener = Arc::new(RecordingListener::default());
    manager.SetListener(listener.clone()).await;
    manager.ForceToBeOwner(&Context::new()).await.unwrap();
    assert!(
        client
            .get(stale_key.as_str(), None)
            .await
            .unwrap()
            .kvs()
            .is_empty()
    );
    manager.CampaignOwner(&[]).await.unwrap();
    wait_owner(&manager, true).await;
    assert!(listener.value.load(Ordering::SeqCst));
    manager.Close().await;
    clear_path(&mut client, path).await;
}

/// 单节点：CampaignCancel / lease revoke 后重选，以及 Close 后无 Leader。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_single() {
    let _ttl = TtlGuard::set(3);
    let mut client = new_client().await;

    // retry on session closed before election
    // session 被取消后再次竞选应能恢复 Owner。
    let close_path = "/task-547/single-close";
    clear_path(&mut client, close_path).await;
    let close_manager = NewOwnerManager(Context::new(), client.clone(), "ddl", "close", close_path);
    close_manager.CampaignOwner(&[]).await.unwrap();
    wait_owner(&close_manager, true).await;
    let first_epoch=close_manager.OwnerEpoch();
    assert!(first_epoch>0);
    close_manager.CampaignCancel().await;
    wait_owner(&close_manager, false).await;
    assert_eq!(close_manager.OwnerEpoch(),0);
    close_manager.CampaignOwner(&[]).await.unwrap();
    wait_owner(&close_manager, true).await;
    assert!(close_manager.OwnerEpoch()>first_epoch);
    close_manager.Close().await;

    // retry on lease revoked before election
    // 外部 revoke lease 后应退位并重新成为 Owner。
    let revoke_path = "/task-547/single-revoke";
    clear_path(&mut client, revoke_path).await;
    let before: HashSet<i64> = client
        .leases()
        .await
        .unwrap()
        .leases()
        .iter()
        .map(|lease| lease.id())
        .collect();
    let revoke_manager =
        NewOwnerManager(Context::new(), client.clone(), "ddl", "revoke", revoke_path);
    let revoke_listener = Arc::new(RecordingListener::default());
    revoke_manager.SetListener(revoke_listener.clone()).await;
    revoke_manager.CampaignOwner(&[]).await.unwrap();
    wait_owner(&revoke_manager, true).await;
    let revoke_epoch=revoke_manager.OwnerEpoch();
    let lease_id = client
        .leases()
        .await
        .unwrap()
        .leases()
        .iter()
        .map(|lease| lease.id())
        .find(|lease| !before.contains(lease))
        .expect("owner campaign created a lease");
    client.lease_revoke(lease_id).await.unwrap();
    wait_events(&revoke_listener, 3).await;
    wait_owner(&revoke_manager, true).await;
    assert!(revoke_manager.OwnerEpoch()>revoke_epoch);
    revoke_manager.Close().await;

    let path = "/task-547/single";
    clear_path(&mut client, path).await;
    let manager = NewOwnerManager(Context::new(), client.clone(), "ddl", "1", path);
    let listener = Arc::new(RecordingListener::default());
    manager.SetListener(listener.clone()).await;
    manager.CampaignOwner(&[]).await.unwrap();
    wait_owner(&manager, true).await;
    assert!(listener.value.load(Ordering::SeqCst));

    let cancelled = Context::new();
    cancelled.cancel();
    let manager2 = NewOwnerManager(cancelled, client.clone(), "ddl", "2", path);
    assert!(matches!(
        manager2.CampaignOwner(&[]).await,
        Err(OwnerError::Closed)
    ));
    assert!(manager.IsOwner());

    manager.Close().await;
    wait_owner(&manager, false).await;
    assert!(!listener.value.load(Ordering::SeqCst));
    assert!(matches!(
        manager2.GetOwnerID(&Context::new()).await,
        Err(OwnerError::NoLeader)
    ));
    assert!(matches!(
        GetOwnerOpValue(&Context::new(), Some(client.clone()), path).await,
        Err(OwnerError::NoLeader)
    ));
    manager2.Close().await;
    clear_path(&mut client, path).await;
}

/// SetOwnerOpValue / GetOwnerOpValue，以及并发改 revision 触发 CompareFailed。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial]
async fn test_set_and_get_owner_op_value() {
    let path = "/task-547/owner-op";
    let mut client = new_client().await;
    clear_path(&mut client, path).await;
    let manager = NewOwnerManager(Context::new(), client.clone(), "ddl", "1", path);
    let listener = Arc::new(RecordingListener::default());
    manager.SetListener(listener.clone()).await;
    manager.CampaignOwner(&[]).await.unwrap();
    wait_owner(&manager, true).await;

    let ctx = Context::new();
    assert_eq!(manager.GetOwnerID(&ctx).await.unwrap(), manager.ID());
    let mut op = GetOwnerOpValue(&ctx, Some(client.clone()), path)
        .await
        .unwrap();
    assert_eq!(op, OpType::OpNone);
    assert!(!op.IsSyncedUpgradingState());
    manager
        .SetOwnerOpValue(&ctx, OpType::OpSyncUpgradingState)
        .await
        .unwrap();
    op = GetOwnerOpValue(&ctx, Some(client.clone()), path)
        .await
        .unwrap();
    assert_eq!(op, OpType::OpSyncUpgradingState);
    assert!(op.IsSyncedUpgradingState());
    manager
        .SetOwnerOpValue(&ctx, OpType::OpSyncUpgradingState)
        .await
        .unwrap();

    // Go's MockDelOwnerKey changes the leader revision between get and txn.
    // Continuously rewrite the same owner value to reproduce that compare race.
    // 持续改写同一 Owner value，复现 get 与 txn 间的 revision 竞态。
    let (owner_key, _) = GetOwnerKeyInfo(&ctx, client.clone(), path, &manager.ID())
        .await
        .unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let writer_running = running.clone();
    let mut writer_client = client.clone();
    let owner_value = owner::join_owner_values(&[manager.ID().as_bytes(), &[OpType::OpNone as u8]]);
    let writer_key = owner_key.clone();
    let writer = tokio::spawn(async move {
        while writer_running.load(Ordering::Acquire) {
            writer_client
                .put(writer_key.clone(), owner_value.clone(), None)
                .await
                .unwrap();
            tokio::task::yield_now().await;
        }
    });
    let mut compare_failed = false;
    for _ in 0..200 {
        match manager
            .SetOwnerOpValue(&ctx, OpType::OpSyncUpgradingState)
            .await
        {
            Err(OwnerError::CompareFailed) => {
                compare_failed = true;
                break;
            }
            Ok(()) => {}
            Err(error) => panic!("unexpected owner op error: {error}"),
        }
    }
    running.store(false, Ordering::Release);
    writer.await.unwrap();
    assert!(
        compare_failed,
        "concurrent revision update must fail compare"
    );

    // onlyDelOwnerKey: deletion retires this manager, then the campaign loop
    // creates a new leader with Go's zero operation value.
    // 删除 Owner key 后退位，竞选循环重建 leader 且 Op 归零。
    client.delete(owner_key, None).await.unwrap();
    wait_events(&listener, 3).await;
    wait_owner(&manager, true).await;
    op = GetOwnerOpValue(&ctx, Some(client.clone()), path)
        .await
        .unwrap();
    assert_eq!(op, OpType::OpNone);
    manager.Close().await;
    clear_path(&mut client, path).await;
}

/// Mock 路径下先 Get 再 Set OwnerOpValue。
#[tokio::test]
#[serial]
async fn test_get_owner_op_value_before_set() {
    let ctx = Context::new();
    let manager = NewMockManager(ctx.child_token(), "1", None, "/task-547/mock-owner-op");
    manager.CampaignOwner(&[]).await.unwrap();
    wait_owner(&manager, true).await;
    assert_eq!(manager.GetOwnerID(&ctx).await.unwrap(), manager.ID());
    assert_eq!(
        GetOwnerOpValue(&ctx, None, "/task-547/mock-owner-op")
            .await
            .unwrap(),
        OpType::OpNone
    );
    manager
        .SetOwnerOpValue(&ctx, OpType::OpSyncUpgradingState)
        .await
        .unwrap();
    assert_eq!(
        GetOwnerOpValue(&ctx, None, "/task-547/mock-owner-op")
            .await
            .unwrap(),
        OpType::OpSyncUpgradingState
    );
    manager.Close().await;
}

/// 多节点竞选：删除 leader key 后由下一节点接管。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_cluster() {
    let _ttl = TtlGuard::set(3);
    let path = "/task-547/cluster";
    let mut client = new_client().await;
    clear_path(&mut client, path).await;
    let manager1 = NewOwnerManager(Context::new(), client.clone(), "ddl", "1", path);
    manager1.CampaignOwner(&[]).await.unwrap();
    wait_owner(&manager1, true).await;

    let manager2 = NewOwnerManager(Context::new(), client.clone(), "ddl", "2", path);
    manager2.CampaignOwner(&[]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!manager2.IsOwner());

    delete_leader(&mut client, path, &manager1).await;
    wait_owner(&manager1, false).await;
    wait_owner(&manager2, true).await;
    manager1.Close().await;

    let manager3 = NewOwnerManager(Context::new(), client.clone(), "ddl", "3", path);
    manager3.CampaignOwner(&[]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!manager3.IsOwner());
    manager3.Close().await;
    manager2.Close().await;

    let ctx = Context::new();
    assert!(matches!(
        GetOwnerKeyInfo(&ctx, client.clone(), path, "unused").await,
        Err(OwnerError::NoLeader)
    ));
    assert!(matches!(
        GetOwnerOpValue(&ctx, Some(client.clone()), path).await,
        Err(OwnerError::NoLeader)
    ));
    clear_path(&mut client, path).await;
}

/// WatchOwnerForTest：删除 key 前后 Watch 行为。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_watch_owner() {
    let path = "/task-547/watch";
    let mut client = new_client().await;
    clear_path(&mut client, path).await;
    let manager = NewOwnerManager(Context::new(), client.clone(), "ddl", "1", path);
    let listener = Arc::new(RecordingListener::default());
    manager.SetListener(listener).await;
    manager.CampaignOwner(&[]).await.unwrap();
    wait_owner(&manager, true).await;

    let ctx = Context::new();
    let (owner_key, revision) = GetOwnerKeyInfo(&ctx, client.clone(), path, &manager.ID())
        .await
        .unwrap();
    let watch_manager = manager.clone();
    let watch_ctx = ctx.clone();
    let watch_key = owner_key.clone();
    let mut watch = tokio::spawn(async move {
        WatchOwnerForTest(&watch_ctx, watch_manager.as_ref(), watch_key, revision).await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut watch)
            .await
            .is_err()
    );
    client.delete(owner_key.as_str(), None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), watch)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    WatchOwnerForTest(&ctx, manager.as_ref(), owner_key, revision)
        .await
        .unwrap();
    manager.Close().await;
    clear_path(&mut client, path).await;
}

/// Owner key 已删除后再 Watch，应立即返回。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_watch_owner_after_delete_owner_key() {
    let path = "/task-547/watch-after-delete";
    let mut client = new_client().await;
    clear_path(&mut client, path).await;
    let manager = NewOwnerManager(Context::new(), client.clone(), "ddl", "1", path);
    manager.CampaignOwner(&[]).await.unwrap();
    wait_owner(&manager, true).await;
    let ctx = Context::new();
    let (owner_key, revision) = GetOwnerKeyInfo(&ctx, client.clone(), path, &manager.ID())
        .await
        .unwrap();
    client.delete(owner_key.as_str(), None).await.unwrap();
    WatchOwnerForTest(&ctx, manager.as_ref(), owner_key, revision)
        .await
        .unwrap();
    manager.Close().await;
    clear_path(&mut client, path).await;
}

/// 连续 CampaignOwner / CampaignCancel 不应挂死。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_immediately_cancel() {
    let path = "/task-547/immediately-cancel";
    let mut client = new_client().await;
    clear_path(&mut client, path).await;
    let manager = NewOwnerManager(Context::new(), client.clone(), "ddl", "1", path);
    for _ in 0..10 {
        manager.CampaignOwner(&[]).await.unwrap();
        manager.CampaignCancel().await;
    }
    manager.Close().await;
    clear_path(&mut client, path).await;
}

/// 分布式锁：同客户端互斥、跨客户端互斥、丢弃后 TTL 过期。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial]
async fn test_acquire_distributed_lock() {
    let mut client1 = new_client().await;
    let client2 = new_client().await;

    // acquire distributed lock with same client
    // 同一 client 二次加锁应阻塞直至前一把释放。
    let key = "/task-547/lock/same";
    clear_path(&mut client1, key).await;
    let ctx = Context::new();
    let lock1 = AcquireDistributedLock(&ctx, client1.clone(), key, 10)
        .await
        .unwrap();
    let same_ctx = ctx.clone();
    let same_client = client1.clone();
    let mut blocked =
        tokio::spawn(async move { AcquireDistributedLock(&same_ctx, same_client, key, 10).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut blocked)
            .await
            .is_err()
    );
    lock1.release().await.unwrap();
    let lock2 = tokio::time::timeout(Duration::from_secs(5), blocked)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    lock2.release().await.unwrap();
    let independent1 = AcquireDistributedLock(&ctx, client1.clone(), format!("{key}/1"), 10)
        .await
        .unwrap();
    let independent2 = AcquireDistributedLock(&ctx, client1.clone(), format!("{key}/2"), 10)
        .await
        .unwrap();
    independent1.release().await.unwrap();
    independent2.release().await.unwrap();

    // acquire distributed lock with different clients
    // 不同 client 对同一 key 也应互斥。
    let key = "/task-547/lock/different";
    clear_path(&mut client1, key).await;
    let lock1 = AcquireDistributedLock(&ctx, client1.clone(), key, 10)
        .await
        .unwrap();
    let other_ctx = ctx.clone();
    let other_client = client2.clone();
    let mut blocked =
        tokio::spawn(
            async move { AcquireDistributedLock(&other_ctx, other_client, key, 10).await },
        );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut blocked)
            .await
            .is_err()
    );
    lock1.release().await.unwrap();
    let lock2 = tokio::time::timeout(Duration::from_secs(5), blocked)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    lock2.release().await.unwrap();

    // acquire distributed lock until timeout: dropping the first client's
    // handle stops keepalive and etcd releases the lock after its one-second TTL.
    // 丢弃锁句柄后保活停止，TTL 到期后另一 client 可获取。
    let key = "/task-547/lock/timeout";
    clear_path(&mut client1, key).await;
    let abandoned = AcquireDistributedLock(&ctx, client1.clone(), key, 1)
        .await
        .unwrap();
    drop(abandoned);
    let lock2 = tokio::time::timeout(
        Duration::from_secs(8),
        AcquireDistributedLock(&ctx, client2, key, 10),
    )
    .await
    .expect("abandoned one-second lease should expire")
    .unwrap();
    lock2.release().await.unwrap();
}

/// ListenersWrapper 向多个 Listener 广播事件。
#[test]
#[serial]
fn test_listeners_wrapper() {
    let listener1 = Arc::new(RecordingListener::default());
    let listener2 = Arc::new(RecordingListener::default());
    let wrapper = NewListenersWrapper(vec![listener1.clone(), listener2.clone()]);
    wrapper.OnBecomeOwner();
    assert!(listener1.value.load(Ordering::SeqCst));
    assert!(listener2.value.load(Ordering::SeqCst));
    wrapper.OnRetireOwner();
    assert!(!listener1.value.load(Ordering::SeqCst));
    assert!(!listener2.value.load(Ordering::SeqCst));
    assert_eq!(*listener1.events.lock().unwrap(), vec!["become", "retire"]);
    assert_eq!(*listener2.events.lock().unwrap(), vec!["become", "retire"]);
}
