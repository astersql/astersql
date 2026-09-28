// Copyright 2026 AsterSQL.

// AutoID 分配器与远程服务路径的 Aster 单元测试。
//
// 用内存 `IdStore` 与伪造的 Leader 发现 / RPC 客户端，对齐 Go 侧边界条件：
// 纯数学批大小、有符号/无符号缓存分配、SEQUENCE 循环、临时表内存分配器、
// 服务发现重试与取消退避，以及 Allocators / RuntimeStats 集合行为。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::*;

/// 内存版 AutoID 水位存储：键为 [`AutoIdKey`]，值为当前全局 end。
#[derive(Default)]
struct MemoryStore {
    values: Mutex<HashMap<AutoIdKey, i64>>,
}

/// 持有可变 map 借用的单次事务视图，实现 [`IdTransaction`]。
struct MemoryTransaction<'a> {
    values: &'a mut HashMap<AutoIdKey, i64>,
}

impl IdTransaction for MemoryTransaction<'_> {
    fn get(&self, key: AutoIdKey) -> Result<i64> {
        Ok(*self.values.get(&key).unwrap_or(&0))
    }

    fn put(&mut self, key: AutoIdKey, value: i64) -> Result<()> {
        self.values.insert(key, value);
        Ok(())
    }

    fn inc(&mut self, key: AutoIdKey, step: i64) -> Result<i64> {
        let value = self.values.entry(key).or_default();
        *value = value.wrapping_add(step);
        Ok(*value)
    }

    fn copy_to(&mut self, from: AutoIdKey, to: AutoIdKey) -> Result<()> {
        let value = self.get(from)?;
        self.values.insert(to, value);
        Ok(())
    }
}

impl IdStore for MemoryStore {
    fn run_in_transaction(
        &self,
        operation: &mut dyn FnMut(&mut dyn IdTransaction) -> Result<()>,
    ) -> Result<()> {
        // 锁住整表 map，在闭包内以事务语义访问。
        let mut values = self.values.lock().unwrap();
        operation(&mut MemoryTransaction {
            values: &mut values,
        })
    }
}

/// 构造空的共享内存存储。
fn store() -> Arc<MemoryStore> {
    Arc::new(MemoryStore::default())
}

/// 校验 AUTO_RANDOM / 批大小 / SEQUENCE / 动态 step / ShardIdFormat 与 Go 边界一致。
#[test]
fn pure_autoid_math_matches_go_boundaries() {
    // 系统库 ID 高位标志与规范化参数。
    assert!(is_mem_schema_id(INFORMATION_SCHEMA_DB_ID));
    assert!(!is_mem_schema_id(42));
    assert_eq!(auto_random_shard_bits_normalize(-1, "id").unwrap(), 5);
    assert!(auto_random_shard_bits_normalize(0, "id").is_err());
    assert!(auto_random_shard_bits_normalize(16, "id").is_err());
    assert_eq!(auto_random_range_bits_normalize(-1).unwrap(), 64);
    assert!(auto_random_range_bits_normalize(31).is_err());

    // increment/offset 下的寻值与批大小（有符号/无符号）。
    assert_eq!(seek_to_first_auto_id_signed(6, 4, 1), 9);
    assert_eq!(calc_needed_batch_size(6, 2, 4, 1, false), 7);
    assert_eq!(seek_to_first_auto_id_unsigned(6, 4, 1), 9);
    assert_eq!(calc_needed_batch_size(-2, 2, 2, 1, true) as u64, 3);

    // SEQUENCE 批大小、极值寻值，以及有符号整数可比编码往返。
    assert_eq!(calc_sequence_batch_size(0, 3, 2, 1, 1, 10).unwrap(), 5);
    assert_eq!(calc_sequence_batch_size(10, 3, -2, 10, 0, 10).unwrap(), 6);
    assert!(calc_sequence_batch_size(10, 3, 1, 1, 1, 10).is_err());
    assert_eq!(
        seek_to_first_sequence_value(i64::MIN, 3, 1, i64::MIN, i64::MAX),
        (i64::MIN + 2, true)
    );
    for value in [i64::MIN, -1, 0, 1, i64::MAX] {
        assert_eq!(decode_cmp_uint_to_int(encode_int_to_cmp_uint(value)), value);
    }

    // 按消耗时长调节 step：慢则缩小、极快顶到 MAX_STEP、默认窗口保持。
    assert_eq!(next_step(30_000, Duration::from_secs(20)), 30_000);
    assert_eq!(next_step(30_000, Duration::from_millis(1)), 2_000_000);
    assert_eq!(next_step(60_000, Duration::from_secs(10)), 60_000);

    // AUTO_RANDOM：有符号预留符号位，无符号多 1 bit 增量空间；compose 拼 shard|inc。
    let signed = ShardIdFormat::new(false, 5, 64);
    assert_eq!(signed.incremental_bits, 58);
    assert_eq!(signed.compose(3, 7), (3_i64 << 58) | 7);
    let unsigned = ShardIdFormat::new(true, 5, 64);
    assert_eq!(unsigned.incremental_bits, 59);
}

/// 有符号缓存分配器：alloc / rebase / force_rebase / transfer 与并发唯一性对齐 Go。
#[test]
fn cached_allocator_signed_rebase_transfer_and_concurrency_match_go() {
    let store = store();
    let alloc = Arc::new(DefaultAllocator::with_options(
        store.clone(),
        1,
        10,
        false,
        AllocatorType::RowId,
        &[AllocatorOption::CustomStep(5)],
    ));

    // 首次水位为 1；batch=2 得到 (0,2]，本地 end 推进到自定义 step=5。
    assert_eq!(alloc.next_global_auto_id().unwrap(), 1);
    assert_eq!(
        alloc.alloc(&Context::background(), 2, 1, 1).unwrap(),
        (0, 2)
    );
    assert_eq!(alloc.end(), 5);
    assert_eq!(
        alloc.alloc(&Context::background(), 1, 2, 1).unwrap(),
        (2, 3)
    );
    // rebase 抬高 base；随后 force_rebase 写回全局；更低的 rebase 不回退。
    alloc.rebase(&Context::background(), 10, false).unwrap();
    assert_eq!(
        alloc.alloc(&Context::background(), 1, 1, 1).unwrap(),
        (10, 11)
    );
    assert_eq!(alloc.next_global_auto_id().unwrap(), 16);

    alloc.force_rebase(20).unwrap();
    assert_eq!(alloc.base(), 20);
    alloc.rebase(&Context::background(), 3, true).unwrap();
    assert_eq!(alloc.base(), 20);
    // transfer 把水位拷到新库表键，下一全局 ID 仍为 21。
    alloc.transfer(2, 30).unwrap();
    assert_eq!(alloc.next_global_auto_id().unwrap(), 21);

    // 4 线程各分配 25 个 ID，去重后应为 100。
    let mut joins = Vec::new();
    for _ in 0..4 {
        let alloc = alloc.clone();
        joins.push(thread::spawn(move || {
            (0..25)
                .map(|_| alloc.alloc(&Context::background(), 1, 1, 1).unwrap().1)
                .collect::<Vec<_>>()
        }));
    }
    let mut ids: Vec<_> = joins.into_iter().flat_map(|j| j.join().unwrap()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 100);
}

/// 无符号耗尽报错；带 cache+cycle 的 SEQUENCE 分配与 rebase_seq 对齐 Go。
#[test]
fn cached_allocator_unsigned_and_sequence_match_go() {
    let store = store();
    let unsigned = DefaultAllocator::new(store.clone(), 1, 20, true, AllocatorType::AutoIncrement);
    unsigned.set_step_for_test(4);
    // 推到 u64::MAX 附近后下一次分配应失败。
    unsigned.force_rebase((u64::MAX - 2) as i64).unwrap();
    assert_eq!(unsigned.next_global_auto_id().unwrap() as u64, u64::MAX - 1);
    assert_eq!(
        unsigned.alloc(&Context::background(), 1, 1, 1).unwrap().1 as u64,
        u64::MAX - 1
    );
    assert!(matches!(
        unsigned.alloc(&Context::background(), 1, 1, 1),
        Err(AutoIdError::AutoIncrementReadFailed(_))
    ));

    // SEQUENCE：increment=2，cache=2，cycle=true；第三批 round 变为 1。
    let sequence = DefaultAllocator::new_sequence(
        store,
        1,
        21,
        SequenceInfo {
            increment: 2,
            start: 1,
            min_value: 1,
            max_value: 5,
            cache: true,
            cache_value: 2,
            cycle: true,
        },
    );
    assert_eq!(sequence.alloc_seq_cache().unwrap(), (0, 3, 0));
    assert_eq!(sequence.alloc_seq_cache().unwrap(), (3, 5, 0));
    assert_eq!(sequence.alloc_seq_cache().unwrap(), (0, 3, 1));
    assert_eq!(sequence.rebase_seq(4).unwrap(), (4, false));
    // 已满足目标水位时返回 (0, true)。
    assert_eq!(sequence.rebase_seq(3).unwrap(), (0, true));
}

/// 临时表内存分配器：从 TableInfo 初始水位、increment 分配、rebase 单调、无 SEQUENCE。
#[test]
fn in_memory_allocator_matches_temporary_table_go_behavior() {
    let table = TableInfo {
        pk_is_handle: false,
        is_common_handle: false,
        has_auto_increment_column: true,
        auto_increment_unsigned: false,
        auto_increment_id: 100,
        ..TableInfo::default()
    };
    let alloc = new_allocator_from_temp_table_info(&table).unwrap();
    assert_eq!(alloc.next_global_auto_id().unwrap(), 100);
    assert_eq!(
        alloc.alloc(&Context::background(), 1, 1, 1).unwrap(),
        (99, 100)
    );
    assert_eq!(
        alloc.alloc(&Context::background(), 2, 10, 1).unwrap(),
        (100, 111)
    );
    // rebase 只抬高不降低；force_rebase 可强制回写。
    alloc.rebase(&Context::background(), 200, false).unwrap();
    alloc.rebase(&Context::background(), 10, false).unwrap();
    assert_eq!(alloc.base(), 200);
    alloc.force_rebase(7).unwrap();
    assert_eq!(alloc.base(), 7);
    assert!(alloc.alloc_seq_cache().is_err());
}

/// 伪造 etcd Leader 发现：按队列弹出地址，缺省返回固定 leader。
#[derive(Default)]
struct FakeDiscovery {
    leaders: Mutex<VecDeque<Result<Option<String>>>>,
    calls: AtomicUsize,
}

impl LeaderDiscovery for FakeDiscovery {
    fn leader(&self, _ctx: &Context, _path: &str) -> Result<Option<String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.leaders
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(Some("leader:1234".into())))
    }
}

/// 伪造 AutoID RPC 客户端：按队列返回 alloc/rebase 结果并计数调用次数。
#[derive(Default)]
struct FakeClient {
    alloc_results: Mutex<VecDeque<Result<AutoIdResponse>>>,
    rebase_results: Mutex<VecDeque<Result<RebaseResponse>>>,
    alloc_calls: AtomicUsize,
    rebase_calls: AtomicUsize,
}

impl AutoIdClient for FakeClient {
    fn alloc_auto_id(&self, _ctx: &Context, request: AutoIdRequest) -> Result<AutoIdResponse> {
        self.alloc_calls.fetch_add(1, Ordering::SeqCst);
        self.alloc_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(AutoIdResponse {
                min: 0,
                max: request.n as i64,
                errmsg: String::new(),
            }))
    }

    fn rebase(&self, _ctx: &Context, _request: RebaseRequest) -> Result<RebaseResponse> {
        self.rebase_calls.fetch_add(1, Ordering::SeqCst);
        self.rebase_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(RebaseResponse {
                errmsg: String::new(),
            }))
    }
}

/// 伪造连接：仅统计 close 次数，用于验证失败后连接回收。
#[derive(Default)]
struct FakeConnection {
    closes: AtomicUsize,
}

impl ClientConnection for FakeConnection {
    fn close(&self) -> Result<()> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// 伪造连接器：始终返回同一 client/connection 对。
struct FakeConnector {
    client: Arc<FakeClient>,
    connection: Arc<FakeConnection>,
    calls: AtomicUsize,
}

impl AutoIdClientConnector for FakeConnector {
    fn connect(
        &self,
        _address: &str,
    ) -> Result<(Arc<dyn AutoIdClient>, Arc<dyn ClientConnection>)> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok((self.client.clone(), self.connection.clone()))
    }
}

/// 组装 `ClientDiscover` 与各 Fake 组件，供服务端路径测试复用。
fn service_fixture() -> (
    Arc<ClientDiscover>,
    Arc<FakeDiscovery>,
    Arc<FakeClient>,
    Arc<FakeConnection>,
) {
    let discovery = Arc::new(FakeDiscovery::default());
    let client = Arc::new(FakeClient::default());
    let connection = Arc::new(FakeConnection::default());
    let connector = Arc::new(FakeConnector {
        client: client.clone(),
        connection: connection.clone(),
        calls: AtomicUsize::new(0),
    });
    let discover = Arc::new(ClientDiscover::new(discovery.clone(), connector));
    (discover, discovery, client, connection)
}

/// 单点分配器：Leader 路径、RPC 重试、业务 errmsg 与延迟 close 对齐 Go。
#[test]
fn service_discovery_retry_reset_and_business_errors_match_go() {
    // nullspace 用裸路径；非 0 keyspace 加前缀 `/`。
    assert_eq!(
        get_auto_id_service_leader_etcd_path(NULLSPACE_ID),
        AUTO_ID_LEADER_PATH
    );
    assert_eq!(
        get_auto_id_service_leader_etcd_path(7),
        format!("/{AUTO_ID_LEADER_PATH}")
    );

    let (discover, discovery, client, connection) = service_fixture();
    // 先 None 再两次同一 leader；alloc 先 RPC 失败再成功，再业务错误。
    discovery.leaders.lock().unwrap().extend([
        Ok(None),
        Ok(Some("leader:1234".into())),
        Ok(Some("leader:1234".into())),
    ]);
    client.alloc_results.lock().unwrap().extend([
        Err(AutoIdError::Rpc("rpc error: unavailable".into())),
        Ok(AutoIdResponse {
            min: 10,
            max: 12,
            errmsg: String::new(),
        }),
        Ok(AutoIdResponse {
            min: 0,
            max: 0,
            errmsg: "server rejected".into(),
        }),
    ]);
    let alloc = SinglePointAllocator::new(1, 2, false, NULLSPACE_ID, discover.clone());
    assert_eq!(
        alloc.alloc(&Context::background(), 2, 1, 1).unwrap(),
        (10, 12)
    );
    // 一次失败重试后成功：共 2 次 RPC；discover 版本递增。
    assert_eq!(client.alloc_calls.load(Ordering::SeqCst), 2);
    assert_eq!(discover.version(), 1);
    assert!(alloc.alloc(&Context::background(), 1, 1, 1).is_err());
    // 业务失败后异步关闭连接。
    thread::sleep(Duration::from_millis(230));
    assert_eq!(connection.closes.load(Ordering::SeqCst), 1);
}

/// 已取消上下文应快速返回 Canceled；Backoffer 在等待中被取消也不阻塞。
#[test]
fn canceled_rpc_and_context_aware_backoff_return_quickly() {
    let (discover, _, client, _) = service_fixture();
    discover
        .get_client(&Context::background(), NULLSPACE_ID)
        .unwrap();
    client
        .alloc_results
        .lock()
        .unwrap()
        .push_back(Err(AutoIdError::Rpc("rpc error: canceled".into())));
    let alloc = SinglePointAllocator::new(1, 2, false, NULLSPACE_ID, discover);
    let ctx = Context::background();
    ctx.cancel();
    let started = Instant::now();
    assert!(matches!(
        alloc.alloc(&ctx, 1, 1, 1),
        Err(AutoIdError::Canceled)
    ));
    assert!(started.elapsed() < Duration::from_millis(50));
    assert_eq!(client.alloc_calls.load(Ordering::SeqCst), 1);

    // 退避进行中由另一线程 cancel，应在 50ms 内退出。
    let ctx = Context::background();
    let cancel_ctx = ctx.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(5));
        cancel_ctx.cancel();
    });
    let mut backoff = Backoffer::default();
    backoff.duration = Duration::from_millis(100);
    let started = Instant::now();
    assert!(matches!(
        backoff.backoff(Some(&ctx)),
        Err(AutoIdError::Canceled)
    ));
    assert!(started.elapsed() < Duration::from_millis(50));
}

/// Allocators 在未分离时 AutoIncrement 回退到 RowId；RuntimeStats 记录与 merge。
#[test]
fn allocator_collections_and_runtime_stats_match_go() {
    let row: Arc<dyn Allocator> = Arc::new(InMemoryAllocator::new(false, AllocatorType::RowId));
    let random: Arc<dyn Allocator> =
        Arc::new(InMemoryAllocator::new(false, AllocatorType::AutoRandom));
    let all = Allocators::new(false, vec![row.clone(), random]);
    assert!(Arc::ptr_eq(
        &all.get(AllocatorType::AutoIncrement).unwrap(),
        &row
    ));
    assert_eq!(
        all.filter(|a| a.get_type() == AllocatorType::AutoRandom)
            .len(),
        1
    );

    let mut stats = AllocatorRuntimeStats::default();
    assert_eq!(stats.to_string(), "");
    stats.record_alloc();
    stats.record_rebase();
    stats.snapshot_stats = "snapshot: 1".into();
    assert_eq!(
        stats.to_string(),
        "auto_id_allocator: {alloc_cnt: 1, rebase_cnt: 1, snapshot: 1}"
    );
    let mut clone = stats.clone();
    let mut other = AllocatorRuntimeStats::default();
    other.commit_stats = "commit: 2".into();
    clone.merge(&other);
    assert!(clone.to_string().contains("commit: 2"));
}
