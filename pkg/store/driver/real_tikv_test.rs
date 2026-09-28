// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 真实 TiKV 集群上的 Rust 客户端端到端测试。
//
// 该测试验证驱动打开真实集群后可获取 TSO、执行 MVCC 提交与回滚、发送标准 DAG 请求，
// 并能在测试自有的 TiKV 进程重启后恢复连接；最后还检查错误 PD 地址会快速失败。
// 测试默认忽略，需显式提供真实 PD 地址并由本地集群脚本管理 TiKV 生命周期。

use std::any::Any;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use astersql_kv as kv;
use protobuf::{Message, RepeatedField};

use crate::{TiKVDriver, TikvStore};

fn open_real_store(pd: &str) -> TikvStore {
    TiKVDriver::default()
        .Open(&format!("tikv://{pd}?disableGC=true"))
        .expect("real PD/TiKV store must open")
}

fn begin_pessimistic(store: &TikvStore) -> Box<dyn kv::Transaction> {
    let mut transaction = kv::Storage::Begin(store, &[]).expect("transaction must begin");
    transaction.SetOption(kv::Pessimistic, Some(Box::new(true)));
    assert!(transaction.IsPessimistic());
    transaction
}

fn lock_ctx(wait_timeout_ms: i64, shared: bool) -> kv::LockCtx {
    kv::LockCtx {
        WaitTimeoutMs: wait_timeout_ms,
        Shared: shared,
        ..kv::LockCtx::default()
    }
}

fn dag_request() -> kv::Request {
    kv::Request {
        Tp: kv::ReqTypeDAG,
        StartTs: 0,
        Data: Vec::new(),
        KeyRanges: None,
        PartitionIDAndRanges: Vec::new(),
        Concurrency: 1,
        CoprRequestRateLimit: None,
        IsolationLevel: kv::IsoLevel::SI,
        Priority: kv::PriorityHigh,
        MemTracker: None,
        KeepOrder: true,
        Desc: false,
        NotFillCache: false,
        ReplicaRead: kv::ReplicaReadType::ReplicaReadLeader,
        StoreType: kv::StoreType::TiKV,
        Cacheable: false,
        SchemaVar: 0,
        BatchCop: false,
        TaskID: 5,
        TiDBServerID: 0,
        TxnScope: kv::GlobalTxnScope.to_owned(),
        ReadReplicaScope: String::new(),
        IsStaleness: false,
        ClosestReplicaReadAdjuster: None,
        MatchStoreLabels: Vec::new(),
        ResourceGroupTagger: None,
        Paging: kv::Paging::default(),
        RequestSource: kv::util::RequestSource::default(),
        StoreBatchSize: 0,
        ResourceGroupName: "default".to_owned(),
        LimitSize: 0,
        StoreBusyThreshold: Duration::ZERO,
        TiKVClientReadTimeout: 5_000,
        MaxExecutionTime: 5_000,
        MaxKeysRead: 0,
        MaxKeysReadCounter: None,
        RunawayChecker: None,
        ResourceControlInterceptor: None,
        ConnID: 5,
        ConnAlias: "real-tikv-e2e".to_owned(),
    }
}

fn send_option() -> kv::ClientSendOption {
    kv::ClientSendOption {
        SessionMemTracker: None,
        EnabledRateLimitAction: false,
        EventCb: None,
        EnableCollectExecutionInfo: false,
        TiFlashReplicaRead: kv::tiflash::ReplicaRead::default(),
        AppendWarning: None,
        TryCopLiteWorker: None,
    }
}

fn encode_int(value: i64) -> [u8; 8] {
    // 翻转符号位后按大端编码，使有符号整数的字节序与数值顺序一致。
    ((value as u64) ^ (1_u64 << 63)).to_be_bytes()
}

fn table_record_key(table_id: i64, handle: i64) -> Vec<u8> {
    // TiDB 行键布局为 `t{table_id}_r{handle}`，两个整数均使用可排序编码。
    let mut key = Vec::with_capacity(19);
    key.push(b't');
    key.extend_from_slice(&encode_int(table_id));
    key.extend_from_slice(b"_r");
    key.extend_from_slice(&encode_int(handle));
    key
}

fn table_scan_dag(table_id: i64) -> Vec<u8> {
    // 构造最小可执行的 TableScan DAG，只返回作为主键句柄的第一列。
    let mut handle_column = tipb::ColumnInfo::new();
    handle_column.set_column_id(1);
    handle_column.set_tp(8);
    handle_column.set_pk_handle(true);

    let mut scan = tipb::TableScan::new();
    scan.set_table_id(table_id);
    scan.set_columns(RepeatedField::from_vec(vec![handle_column]));

    let mut executor = tipb::Executor::new();
    executor.set_tp(tipb::ExecType::TypeTableScan);
    executor.set_tbl_scan(scan);

    let mut dag = tipb::DagRequest::new();
    dag.set_executors(RepeatedField::from_vec(vec![executor]));
    dag.set_output_offsets(vec![0]);
    dag.write_to_bytes().expect("DAG protobuf must serialize")
}

fn run_standard_dag(
    store: &TikvStore,
    ctx: &kv::Context,
    table_id: i64,
    handle: i64,
) -> (usize, usize) {
    // 将扫描范围限制为预先写入的单行，同时遍历流式响应以验证数据包确实可读。
    let record_key = table_record_key(table_id, handle);
    let mut request = dag_request();
    request.StartTs = kv::Storage::CurrentVersion(store, kv::GlobalTxnScope)
        .expect("DAG start_ts must be allocated")
        .Ver;
    request.Data = table_scan_dag(table_id);
    request.KeyRanges = Some(kv::NewNonPartitionedKeyRanges(vec![kv::KeyRange {
        StartKey: kv::Key(record_key),
        EndKey: kv::Key(table_record_key(table_id, handle + 1)),
    }]));

    let mut response = kv::Storage::GetClient(store)
        .Send(ctx, &request, &() as &dyn Any, &send_option())
        .expect("configured client response");
    let mut packets = 0;
    let mut bytes = 0;
    while let Some(subset) = response.Next(ctx).expect("standard DAG must stream") {
        packets += 1;
        bytes += subset.GetData().len();
    }
    response.Close().expect("DAG stream must close");
    assert!(packets > 0, "standard DAG must return a response packet");
    assert!(bytes > 0, "standard DAG response must contain data");
    (packets, bytes)
}

fn source_cluster_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("scripts/run-local-tikv-source.sh")
}

fn restart_owned_tikv() {
    // 仅重启由测试脚本管理的 TiKV，PD 保持运行以便复用同一个客户端验证重连。
    let output = Command::new(source_cluster_script())
        .arg("restart-tikv")
        .output()
        .expect("source-cluster restart script must execute");
    assert!(
        output.status.success(),
        "source-cluster restart failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!(
        "source_tikv_restart={}",
        String::from_utf8_lossy(&output.stdout).trim()
    );
}

fn wait_for_reconnect(store: &TikvStore) -> u64 {
    // 重启期间连接失败属于预期现象；持续申请 TSO，成功即表示客户端已恢复。
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last_error = None;
    while Instant::now() < deadline {
        match kv::Storage::CurrentVersion(store, kv::GlobalTxnScope) {
            Ok(version) => return version.Ver,
            Err(error) => last_error = Some(error.to_string()),
        }
        thread::sleep(Duration::from_millis(500));
    }
    panic!("client-rust did not reconnect after TiKV restart: {last_error:?}");
}

#[test]
#[ignore = "requires REAL_TIKV_PD and scripts/run-local-tikv-source.sh start"]
fn real_tikv_client_rust_end_to_end() {
    let pd = std::env::var("REAL_TIKV_PD").expect("REAL_TIKV_PD must name the real PD endpoint");
    let mut driver = TiKVDriver::default();
    let mut store = driver
        .Open(&format!("tikv://{pd}?disableGC=true"))
        .expect("real PD/TiKV store must open");
    let ctx = kv::Context::todo();

    let tso1 = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
        .expect("first real TSO must succeed")
        .Ver;
    let tso2 = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
        .expect("second real TSO must succeed")
        .Ver;
    assert!(tso2 > tso1, "PD timestamps must be strictly increasing");
    println!(
        "cluster_id={} tso1={tso1} tso2={tso2}",
        store.GetClusterID()
    );

    let prefix = format!("astersql/client-rust/e2e/{tso2}/").into_bytes();
    let committed_key = [prefix.as_slice(), b"committed"].concat();
    let rolled_back_key = [prefix.as_slice(), b"rolled-back"].concat();
    let old_snapshot = kv::Storage::GetSnapshot(
        &store,
        kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
            .expect("old snapshot timestamp must be allocated"),
    );

    // 先创建快照再提交，借此验证旧快照隔离；新快照则必须看到已提交值。
    let mut committed = kv::Storage::Begin(&store, &[]).expect("transaction must begin");
    kv::Mutator::Set(
        committed.as_mut(),
        kv::Key(committed_key.clone()),
        b"committed-value".to_vec(),
    )
    .expect("transaction put must succeed");
    committed.Commit(&ctx).expect("transaction must commit");
    let old_snapshot_error = kv::Getter::Get(
        old_snapshot.as_ref(),
        &ctx,
        kv::Key(committed_key.clone()),
        &[],
    )
    .expect_err("snapshot before commit must not see the new value");
    assert!(
        kv::ErrNotExist.Equal(Some(&old_snapshot_error)),
        "snapshot before commit must report ErrNotExist, got {old_snapshot_error}"
    );
    let new_snapshot = kv::Storage::GetSnapshot(
        &store,
        kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
            .expect("new snapshot timestamp must be allocated"),
    );
    let committed_value = kv::Getter::Get(new_snapshot.as_ref(), &ctx, kv::Key(committed_key), &[])
        .expect("snapshot after commit must see the value");
    assert_eq!(committed_value.Value, b"committed-value");

    // 回滚路径使用独立键，确保未提交写入不会泄漏到后续快照。
    let mut rolled_back = kv::Storage::Begin(&store, &[]).expect("rollback transaction must begin");
    kv::Mutator::Set(
        rolled_back.as_mut(),
        kv::Key(rolled_back_key.clone()),
        b"must-disappear".to_vec(),
    )
    .expect("rollback transaction put must succeed");
    rolled_back
        .Rollback()
        .expect("transaction rollback must succeed");
    let rollback_snapshot = kv::Storage::GetSnapshot(
        &store,
        kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
            .expect("rollback snapshot timestamp must be allocated"),
    );
    let rollback_error = kv::Getter::Get(
        rollback_snapshot.as_ref(),
        &ctx,
        kv::Key(rolled_back_key),
        &[],
    )
    .expect_err("rolled-back value must remain absent");
    assert!(
        kv::ErrNotExist.Equal(Some(&rollback_error)),
        "rolled-back value must report ErrNotExist, got {rollback_error}"
    );
    println!(
        "mvcc commit={} rollback_missing=true old_snapshot_missing=true",
        String::from_utf8_lossy(&committed_value.Value)
    );

    let table_id = 9_000_000_i64 + i64::try_from(tso2 % 1_000_000).unwrap();
    let handle = 42_i64;
    // 直接写入一条合法行键，为后续真实 coprocessor TableScan 准备最小数据集。
    let mut table_write = kv::Storage::Begin(&store, &[]).expect("DAG seed transaction must begin");
    kv::Mutator::Set(
        table_write.as_mut(),
        kv::Key(table_record_key(table_id, handle)),
        vec![0x80],
    )
    .expect("DAG seed row must be written");
    table_write.Commit(&ctx).expect("DAG seed row must commit");
    let (packets, bytes) = run_standard_dag(&store, &ctx, table_id, handle);
    println!("standard_dag packets={packets} bytes={bytes} table_id={table_id}");

    // 重启前后复用同一个 store，并再次执行 DAG，覆盖连接恢复而非重新建连。
    restart_owned_tikv();
    let reconnect_tso = wait_for_reconnect(&store);
    let (reconnect_packets, reconnect_bytes) = run_standard_dag(&store, &ctx, table_id, handle);
    println!(
        "reconnect tso={reconnect_tso} dag_packets={reconnect_packets} dag_bytes={reconnect_bytes}"
    );
    kv::Storage::Close(&mut store).expect("real TiKV store must close");

    // 无监听服务的本地端口应在驱动的连接超时上限内返回错误，避免初始化长期挂起。
    let started = Instant::now();
    let wrong_pd = TiKVDriver::default().Open("tikv://127.0.0.1:1?disableGC=true");
    let elapsed = started.elapsed();
    assert!(wrong_pd.is_err(), "incorrect PD endpoint must fail");
    assert!(
        elapsed < Duration::from_secs(10),
        "incorrect PD must fail within ten seconds, took {elapsed:?}"
    );
    println!(
        "wrong_pd fail_fast_ms={} error={}",
        elapsed.as_millis(),
        wrong_pd.unwrap_err()
    );
}

#[test]
#[ignore = "requires REAL_TIKV_PD and scripts/run-local-tikv-source.sh start"]
fn real_tikv_pessimistic_lock_matrix() {
    let pd = std::env::var("REAL_TIKV_PD").expect("REAL_TIKV_PD must name the real PD endpoint");
    let store = open_real_store(&pd);
    let ctx = kv::Context::todo();
    let tso = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
        .expect("lock test timestamp must be allocated")
        .Ver;
    let key = |scenario: &str| {
        kv::Key(format!("astersql/client-rust/locks/{tso}/{scenario}").into_bytes())
    };

    // Shared locks coexist, but an exclusive upgrade must wait for the other reader.
    let shared_key = key("shared-upgrade");
    let mut shared_one = begin_pessimistic(&store);
    let mut shared_two = begin_pessimistic(&store);
    shared_one
        .LockKeys(&ctx, &mut lock_ctx(1_000, true), &[shared_key.clone()])
        .expect("first shared lock must succeed");
    shared_two
        .LockKeys(&ctx, &mut lock_ctx(1_000, true), &[shared_key.clone()])
        .expect("second shared lock must coexist");
    let upgrade_started = Instant::now();
    let upgrade_error = shared_one
        .LockKeys(&ctx, &mut lock_ctx(-1, false), &[shared_key.clone()])
        .expect_err("exclusive upgrade must conflict with the other shared owner");
    assert!(
        upgrade_started.elapsed() < Duration::from_secs(2),
        "NOWAIT upgrade took {:?}",
        upgrade_started.elapsed()
    );
    shared_two
        .Rollback()
        .expect("second shared owner must roll back");
    shared_one
        .LockKeys(&ctx, &mut lock_ctx(1_000, false), &[shared_key])
        .expect("upgrade must succeed after the other shared owner releases");
    shared_one
        .Rollback()
        .expect("upgraded transaction must roll back");
    println!("shared_lock compatible=true upgrade_nowait={upgrade_error}");

    // Verify both immediate and bounded server-side lock waits against a real owner.
    let wait_key = key("wait-budget");
    let mut owner = begin_pessimistic(&store);
    owner
        .LockKeys(&ctx, &mut lock_ctx(1_000, false), &[wait_key.clone()])
        .expect("exclusive owner must lock");
    let mut nowait = begin_pessimistic(&store);
    let nowait_started = Instant::now();
    let nowait_error = nowait
        .LockKeys(&ctx, &mut lock_ctx(-1, false), &[wait_key.clone()])
        .expect_err("NOWAIT contender must fail");
    assert!(nowait_started.elapsed() < Duration::from_secs(2));
    nowait.Rollback().expect("NOWAIT contender must roll back");
    let mut bounded = begin_pessimistic(&store);
    let bounded_started = Instant::now();
    let bounded_error = bounded
        .LockKeys(&ctx, &mut lock_ctx(300, false), &[wait_key])
        .expect_err("bounded contender must time out");
    let bounded_elapsed = bounded_started.elapsed();
    assert!(
        bounded_elapsed >= Duration::from_millis(200) && bounded_elapsed < Duration::from_secs(5),
        "300ms lock wait took {bounded_elapsed:?}"
    );
    bounded
        .Rollback()
        .expect("bounded contender must roll back");
    owner.Rollback().expect("exclusive owner must roll back");
    println!(
        "wait_budget nowait_ms={} bounded_ms={} nowait_error={} bounded_error={}",
        nowait_started.elapsed().as_millis(),
        bounded_elapsed.as_millis(),
        nowait_error,
        bounded_error
    );

    // Fair locking reports one fresh RPC lock, then one locally derived retry.
    let fair_key = key("fair-details");
    let mut fair = begin_pessimistic(&store);
    fair.StartFairLocking().expect("fair locking must start");
    let mut first_details = lock_ctx(1_000, false);
    fair.LockKeys(&ctx, &mut first_details, &[fair_key.clone()])
        .expect("fresh fair lock must succeed");
    assert_eq!(first_details.AggressiveLockNewCount, 1);
    assert_eq!(first_details.AggressiveLockDerivedCount, 0);
    assert_eq!(first_details.LockedWithConflictCount, 0);
    fair.RetryFairLocking(&ctx)
        .expect("fair locking retry must start");
    let mut retry_details = lock_ctx(1_000, false);
    fair.LockKeys(&ctx, &mut retry_details, &[fair_key])
        .expect("fair retry must derive the prior lock");
    assert_eq!(retry_details.AggressiveLockNewCount, 0);
    assert_eq!(retry_details.AggressiveLockDerivedCount, 1);
    assert_eq!(retry_details.LockedWithConflictCount, 0);
    fair.DoneFairLocking(&ctx)
        .expect("fair locking must finish");
    fair.Rollback().expect("fair transaction must roll back");
    println!(
        "fair_details first={}/{}/{} retry={}/{}/{}",
        first_details.AggressiveLockNewCount,
        first_details.AggressiveLockDerivedCount,
        first_details.LockedWithConflictCount,
        retry_details.AggressiveLockNewCount,
        retry_details.AggressiveLockDerivedCount,
        retry_details.LockedWithConflictCount
    );

    // Each worker owns its transaction. Once TiKV aborts one edge, that worker
    // rolls back immediately so the surviving transaction can acquire both keys.
    let deadlock_a = key("deadlock-a");
    let deadlock_b = key("deadlock-b");
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = [
        (deadlock_a.clone(), deadlock_b.clone()),
        (deadlock_b, deadlock_a),
    ]
    .into_iter()
    .map(|(first, second)| {
        let pd = pd.clone();
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let store = open_real_store(&pd);
            let ctx = kv::Context::todo();
            let mut transaction = begin_pessimistic(&store);
            transaction
                .LockKeys(&ctx, &mut lock_ctx(1_000, false), &[first])
                .expect("deadlock first edge must lock");
            barrier.wait();
            let result = transaction.LockKeys(&ctx, &mut lock_ctx(10_000, false), &[second]);
            let message = result.as_ref().err().map(ToString::to_string);
            transaction
                .Rollback()
                .expect("deadlock participant must remain rollback-capable");
            (result.is_ok(), message)
        })
    })
    .collect();
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("deadlock worker must not panic"))
        .collect();
    assert_eq!(
        outcomes.iter().filter(|(success, _)| *success).count(),
        1,
        "exactly one deadlock participant must survive: {outcomes:?}"
    );
    let victim_error = outcomes
        .iter()
        .find_map(|(success, error)| (!success).then_some(error.as_deref()).flatten())
        .expect("one deadlock victim must return an error");
    assert!(
        victim_error.to_ascii_lowercase().contains("deadlock"),
        "victim must report a deadlock: {victim_error}"
    );
    println!("deadlock outcomes={outcomes:?} victim_rollback=true survivor=true");
}

#[test]
#[ignore = "requires REAL_TIKV_PD and scripts/run-local-tikv-source.sh start"]
fn real_tikv_locked_with_conflict_details_and_repeated_retry() {
    let pd = std::env::var("REAL_TIKV_PD").expect("REAL_TIKV_PD must name the real PD endpoint");
    let store = open_real_store(&pd);
    let ctx = kv::Context::todo();
    let tso = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
        .expect("conflict test timestamp must be allocated")
        .Ver;
    let existing_key =
        kv::Key(format!("astersql/client-rust/locked-with-conflict/{tso}/existing").into_bytes());
    let missing_key =
        kv::Key(format!("astersql/client-rust/locked-with-conflict/{tso}/missing").into_bytes());

    let mut seed = kv::Storage::Begin(&store, &[]).expect("seed transaction must begin");
    seed.Set(existing_key.clone(), b"old".to_vec())
        .expect("existing key must be seeded");
    seed.Commit(&ctx).expect("seed transaction must commit");

    // Begin the waiter before the owner, then let its ForceLock RPC wait on the
    // owner's real pessimistic lock. The owner's later commit is newer than the
    // request's for_update_ts, so TiKV must return LockedWithConflict.
    let waiter_ready = Arc::new(Barrier::new(2));
    let owner_locked = Arc::new(Barrier::new(2));
    let (attempting_tx, attempting_rx) = mpsc::channel();
    let waiter = {
        let pd = pd.clone();
        let existing_key = existing_key.clone();
        let waiter_ready = Arc::clone(&waiter_ready);
        let owner_locked = Arc::clone(&owner_locked);
        thread::spawn(move || {
            let store = open_real_store(&pd);
            let ctx = kv::Context::todo();
            let mut transaction = begin_pessimistic(&store);
            transaction
                .StartFairLocking()
                .expect("fair locking must start");
            waiter_ready.wait();
            owner_locked.wait();
            attempting_tx
                .send(())
                .expect("owner must observe lock attempt");
            let started = Instant::now();
            let mut first = lock_ctx(5_000, false);
            transaction
                .LockKeys(&ctx, &mut first, &[existing_key.clone()])
                .expect("waiting fair lock must succeed after owner commit");
            let waited = started.elapsed();

            transaction
                .RetryFairLocking(&ctx)
                .expect("fair locking retry must start");
            let mut retry = lock_ctx(5_000, false);
            transaction
                .LockKeys(&ctx, &mut retry, &[existing_key])
                .expect("fair retry must derive the acquired lock");
            transaction
                .DoneFairLocking(&ctx)
                .expect("fair locking must finish");
            transaction
                .Rollback()
                .expect("waiter must release its pessimistic lock");
            (first, retry, waited)
        })
    };

    waiter_ready.wait();
    let mut owner = begin_pessimistic(&store);
    owner
        .LockKeys(&ctx, &mut lock_ctx(1_000, false), &[existing_key.clone()])
        .expect("owner must acquire the existing key");
    owner
        .Set(existing_key.clone(), b"new".to_vec())
        .expect("owner must buffer the new value");
    owner_locked.wait();
    attempting_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("waiter must attempt its real lock RPC");
    thread::sleep(Duration::from_millis(300));
    owner.Commit(&ctx).expect("owner must commit the new value");

    let (first, retry, waited) = waiter.join().expect("waiter must not panic");
    assert!(
        waited >= Duration::from_millis(200),
        "waiter must reach TiKV before owner commit, waited {waited:?}"
    );
    assert_eq!(first.AggressiveLockNewCount, 1);
    assert_eq!(first.AggressiveLockDerivedCount, 0);
    assert_eq!(first.LockedWithConflictCount, 1);
    assert_eq!(retry.AggressiveLockNewCount, 0);
    assert_eq!(retry.AggressiveLockDerivedCount, 1);
    assert_eq!(retry.LockedWithConflictCount, 0);

    // A never-written key still produces a normal new fair lock, not a false
    // LockedWithConflict classification, and is explicitly rolled back.
    let mut missing = begin_pessimistic(&store);
    missing
        .StartFairLocking()
        .expect("missing-key fair locking must start");
    let mut missing_first = lock_ctx(1_000, false);
    missing
        .LockKeys(&ctx, &mut missing_first, &[missing_key.clone()])
        .expect("missing key must be lockable");
    missing
        .RetryFairLocking(&ctx)
        .expect("missing-key retry must start");
    let mut missing_retry = lock_ctx(1_000, false);
    missing
        .LockKeys(&ctx, &mut missing_retry, &[missing_key])
        .expect("missing-key retry must derive the lock");
    missing
        .DoneFairLocking(&ctx)
        .expect("missing-key fair locking must finish");
    missing
        .Rollback()
        .expect("missing-key transaction must release its lock");
    assert_eq!(missing_first.AggressiveLockNewCount, 1);
    assert_eq!(missing_first.AggressiveLockDerivedCount, 0);
    assert_eq!(missing_first.LockedWithConflictCount, 0);
    assert_eq!(missing_retry.AggressiveLockNewCount, 0);
    assert_eq!(missing_retry.AggressiveLockDerivedCount, 1);
    assert_eq!(missing_retry.LockedWithConflictCount, 0);

    println!(
        "locked_with_conflict response=LockedWithConflict existing_first={}/{}/{} existing_retry={}/{}/{} waited_ms={} missing_first={}/{}/{} missing_retry={}/{}/{}",
        first.AggressiveLockNewCount,
        first.AggressiveLockDerivedCount,
        first.LockedWithConflictCount,
        retry.AggressiveLockNewCount,
        retry.AggressiveLockDerivedCount,
        retry.LockedWithConflictCount,
        waited.as_millis(),
        missing_first.AggressiveLockNewCount,
        missing_first.AggressiveLockDerivedCount,
        missing_first.LockedWithConflictCount,
        missing_retry.AggressiveLockNewCount,
        missing_retry.AggressiveLockDerivedCount,
        missing_retry.LockedWithConflictCount,
    );
}
