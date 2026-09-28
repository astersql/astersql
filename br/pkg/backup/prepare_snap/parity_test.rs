// Copyright 2026 AsterSQL.

//! Parity tests for prepare_snap public contracts (Go prepare_test.go scenarios,
//! without unistore / PD — in-memory Env mock only).
//!
//! 本文件对照 Go `prepare_test.go` 的公共契约，但不拉起 unistore / PD。
//! 所有外部依赖都收敛到内存中的 `MockStores` / `MockStore`，以便在 darwin 上编译跑通。
//! 断言关注四类语义：正常 prepare、大请求拆包、重试上限、Finalize 清 lease。
//! 这里验证的是“能否在备份窗口内保持安全”，不是真实集群吞吐。
//! 若把桩行为误写成已实现生产路径，后续对接真实 TiKV 时会出现假阳性通过。
//! 因此注释会明确区分“契约断言”和“仅测试替身能力”。

// 下列标准库导入支撑 mock 的并发队列、条件变量与虚拟时钟。
// Atomic* / mpsc 主要用于连接失败一次与可选延迟场景。
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

// Env/PrepareClient 抽象来自 env；错误辅助来自 errors；被测入口是 prepare::New。
use crate::env::{
    AdaptForGRPCInTest, Context, Env, LimitedBackoff, PrepareClient, Region,
    RetryAndSplitRequestEnv, SplitRequestClient, brpb, errorpb, metapb,
};
use crate::errors::{Error, Result, eof};
use crate::prepare::{New, Preparer};

/// 测试用 Region：只携带 meta 与 leader store，足够驱动 prepare 路由。
/// 刻意不模拟 peer 变更，避免把无关复杂性引入契约测试。
#[derive(Clone)]
struct TestRegion {
    /// Region 元数据，含起止键与 epoch。
    meta: metapb::Region,
    /// 当前 leader 所在 store；prepare 按它建立双向流。
    leader: u64,
}

impl Region for TestRegion {
    /// 返回克隆后的 meta，避免测试并发读写同一份缓冲。
    fn GetMeta(&self) -> metapb::Region {
        self.meta.clone()
    }
    /// leader store id，供 `ConnectToStore` / WaitApply 路由使用。
    fn GetLeaderStoreID(&self) -> u64 {
        self.leader
    }
}

/// 单个 mock store 的可变状态：响应队列、lease、成功 WaitApply 的 region。
/// `now_offset` 用于推进“虚拟时间”，对齐 Go 测试里手动拨钟验证 lease 过期。
struct MockStoreInner {
    /// 待 `Recv` 消费的响应队列，模拟 gRPC 服务端推送。
    queue: VecDeque<brpb::PrepareSnapshotBackupResponse>,
    /// Finish 后置 true，后续 `Recv` 返回 EOF。
    closed: bool,
    /// lease 截止时刻；`None` 表示尚未建立或已清空。
    lease_until: Option<Instant>,
    /// 成功完成 WaitApply 的 region，用于备份窗口覆盖断言。
    success_regions: Vec<metapb::Region>,
    /// 相对 `base` 的时间偏移，模拟时钟前进。
    now_offset: Duration,
    /// 虚拟时钟起点。
    base: Instant,
}

use std::collections::VecDeque;

/// 实现 `PrepareClient` 的内存 store：支持 WaitApply / UpdateLease / Finish。
/// 钩子字段允许测试注入失败、延迟与创建回调，对标 Go mock 的可插拔行为。
struct MockStore {
    /// 受锁保护的队列与 lease 状态。
    inner: Mutex<MockStoreInner>,
    /// `Recv` 在空队列时阻塞等待的条件变量。
    cv: Condvar,
    /// WaitApply 前调用的钩子；返回错误会写入响应的 `Error` 字段。
    on_wait_apply: Mutex<Option<Box<dyn Fn(&metapb::Region) -> Result<()> + Send + Sync>>>,
    /// 可选延迟钩子；当前实现仍立即入队，真正阻塞靠 `on_wait_apply`。
    wait_apply_delay: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// 延迟路径用的计数/条件变量占位，保留与 Go 测试结构对齐的字段。
    delayed_wg: Arc<(Mutex<usize>, Condvar)>,
}

impl MockStore {
    /// 构造默认“WaitApply 立即成功”的 mock store。
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(MockStoreInner {
                queue: VecDeque::new(),
                closed: false,
                lease_until: None,
                success_regions: Vec::new(),
                now_offset: Duration::ZERO,
                base: Instant::now(),
            }),
            cv: Condvar::new(),
            on_wait_apply: Mutex::new(Some(Box::new(|_| Ok(())))),
            wait_apply_delay: Mutex::new(None),
            delayed_wg: Arc::new((Mutex::new(0usize), Condvar::new())),
        })
    }

    /// 读取虚拟当前时间：`base + now_offset`。
    fn now(&self) -> Instant {
        let g = self.inner.lock().unwrap();
        g.base + g.now_offset
    }

    /// 拨快虚拟时钟，用于制造 lease 过期场景。
    fn set_now_offset(&self, d: Duration) {
        self.inner.lock().unwrap().now_offset = d;
    }

    /// 入队一条响应并唤醒所有 `Recv` 等待者。
    fn push(&self, resp: brpb::PrepareSnapshotBackupResponse) {
        let mut g = self.inner.lock().unwrap();
        g.queue.push_back(resp);
        self.cv.notify_all();
    }

    /// lease 是否仍覆盖当前虚拟时间。
    fn lease_valid(&self) -> bool {
        let g = self.inner.lock().unwrap();
        match g.lease_until {
            Some(until) => until > g.base + g.now_offset,
            None => false,
        }
    }
}

impl PrepareClient for MockStore {
    /// 按请求类型处理：WaitApply 逐 region 回包；UpdateLease/Finish 维护 lease。
    /// 语义对齐 Go mock：零值 leaseUntil 视为已过期，Finish 清空并关闭流。
    fn Send(&self, req: &brpb::PrepareSnapshotBackupRequest) -> Result<()> {
        match req.Ty {
            brpb::PrepareSnapshotBackupRequestType::WaitApply => {
                // 每个 region 独立生成 WaitApplyDone，错误写入 Error 字段而非中断整批。
                for region in &req.Regions {
                    let mut resp = brpb::PrepareSnapshotBackupResponse {
                        Ty: brpb::PrepareSnapshotBackupEventType::WaitApplyDone,
                        Region: Some(region.clone()),
                        Error: None,
                        LastLeaseIsValid: false,
                    };
                    let on = self.on_wait_apply.lock().unwrap();
                    if let Some(cb) = on.as_ref() {
                        if let Err(err) = cb(region) {
                            resp.Error = Some(errorpb::Error {
                                Message: err.to_string(),
                            });
                        }
                    }
                    drop(on);

                    let delay = self.wait_apply_delay.lock().unwrap().clone();
                    if delay.is_some() {
                        // Delay path: still enqueue immediately for unit parity of response shape;
                        // gating is done via on_wait_apply blocking callbacks in higher-level tests.
                        // 延迟路径仍立即入队，保证响应形状与无延迟一致；
                        // 真正的“卡住”由上层测试的阻塞回调负责。
                        if resp.Error.is_none() {
                            self.inner
                                .lock()
                                .unwrap()
                                .success_regions
                                .push(region.clone());
                        }
                        self.push(resp);
                    } else {
                        // 无延迟：成功则记入 success_regions，供备份窗口覆盖检查。
                        if resp.Error.is_none() {
                            self.inner
                                .lock()
                                .unwrap()
                                .success_regions
                                .push(region.clone());
                        }
                        self.push(resp);
                    }
                }
            }
            brpb::PrepareSnapshotBackupRequestType::UpdateLease => {
                // Go: expired := s.leaseUntil.Before(s.now()); zero time → expired.
                // 对齐 Go：零值或已到期都算 expired，再刷新为 now+LeaseInSeconds。
                let mut g = self.inner.lock().unwrap();
                let now = g.base + g.now_offset;
                // Go: expired := s.leaseUntil.Before(s.now()); zero time → expired.
                let expired = g.lease_until.map(|u| u <= now).unwrap_or(true);
                g.lease_until = Some(now + Duration::from_secs(req.LeaseInSeconds));
                g.queue.push_back(brpb::PrepareSnapshotBackupResponse {
                    Ty: brpb::PrepareSnapshotBackupEventType::UpdateLeaseResult,
                    Region: None,
                    Error: None,
                    LastLeaseIsValid: !expired,
                });
                drop(g);
                self.cv.notify_all();
            }
            brpb::PrepareSnapshotBackupRequestType::Finish => {
                // Finish 回报一次 UpdateLeaseResult，然后关闭流；lease 清空。
                let mut g = self.inner.lock().unwrap();
                let now = g.base + g.now_offset;
                let expired = g.lease_until.map(|u| u <= now).unwrap_or(true);
                g.lease_until = None;
                g.queue.push_back(brpb::PrepareSnapshotBackupResponse {
                    Ty: brpb::PrepareSnapshotBackupEventType::UpdateLeaseResult,
                    Region: None,
                    Error: None,
                    LastLeaseIsValid: !expired,
                });
                g.closed = true;
                drop(g);
                self.cv.notify_all();
            }
            // 未知类型静默忽略，与当前 Go mock 宽松行为一致。
            _ => {}
        }
        Ok(())
    }

    /// 阻塞直到队列有响应或流已关闭（EOF）。
    /// 条件变量等待对应 Go channel 收包语义。
    fn Recv(&self) -> Result<brpb::PrepareSnapshotBackupResponse> {
        let mut g = self.inner.lock().unwrap();
        loop {
            if let Some(resp) = g.queue.pop_front() {
                return Ok(resp);
            }
            if g.closed {
                return Err(eof());
            }
            g = self.cv.wait(g).unwrap();
        }
    }
}

/// 多 store + region 拓扑的 Env 桩，承载完整 DriveLoop 场景。
/// `on_create_store` / `on_connect` / `connect_delay` 用于注入故障与时序。
struct MockStores {
    /// store id → 懒创建的 MockStore；`None` 表示尚未 Connect。
    stores: Mutex<HashMap<u64, Option<Arc<MockStore>>>>,
    /// 当前拓扑下的全部 TestRegion。
    regions: Mutex<Vec<TestRegion>>,
    /// 首次创建 store 时回调，常用于改写 WaitApply 行为。
    on_create_store: Mutex<Option<Box<dyn Fn(&Arc<MockStore>) + Send + Sync>>>,
    /// 连接前钩子，可返回错误模拟连不上。
    on_connect: Mutex<Option<Box<dyn Fn(u64) -> Result<()> + Send + Sync>>>,
    /// 连接延迟：返回 Receiver 时会阻塞到信号到来。
    connect_delay: Mutex<Option<Box<dyn Fn(u64) -> Option<Receiver<()>> + Send + Sync>>>,
}

impl MockStores {
    /// 按 store 列表与 split key 构造均匀分布 leader 的拓扑。
    fn new(store_ids: &[u64], split_keys: &[Vec<u8>]) -> Arc<Self> {
        let mut stores = HashMap::new();
        for &id in store_ids {
            stores.insert(id, None);
        }
        let regions = build_regions(store_ids, split_keys);
        Arc::new(Self {
            stores: Mutex::new(stores),
            regions: Mutex::new(regions),
            on_create_store: Mutex::new(None),
            on_connect: Mutex::new(None),
            connect_delay: Mutex::new(None),
        })
    }

    /// 备份窗口安全断言：每个已连接 store 有有效 lease，且 WaitApply 覆盖无空洞。
    /// 区间按 StartKey 排序后要求相邻 end==next.start，对齐 Go `assertSafeForBackup`。
    fn assert_safe_for_backup(&self) {
        let stores = self.stores.lock().unwrap();
        let mut ranges: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for store in stores.values().flatten() {
            let g = store.inner.lock().unwrap();
            let now = g.base + g.now_offset;
            if let Some(until) = g.lease_until {
                assert!(until > now, "lease expired during backup window");
            } else {
                panic!("missing lease");
            }
            for r in &g.success_regions {
                ranges.push((r.StartKey.clone(), r.EndKey.clone()));
            }
        }
        ranges.sort_by(|a, b| a.0.cmp(&b.0));
        for i in 1..ranges.len() {
            assert!(
                ranges[i - 1].1 >= ranges[i].0,
                "hole between {:?} and {:?}",
                ranges[i - 1].1,
                ranges[i].0
            );
        }
    }

    /// 正常模式断言：Finalize 之后不应再残留有效 lease。
    /// 对应 Go 侧“恢复可写 / 退出准备态”的检查。
    fn assert_normal_mode(&self) {
        let stores = self.stores.lock().unwrap();
        for (id, store) in stores.iter() {
            if let Some(store) = store {
                let g = store.inner.lock().unwrap();
                let now = g.base + g.now_offset;
                if let Some(until) = g.lease_until {
                    assert!(
                        until <= now,
                        "lease in store {id} still active in normal mode"
                    );
                }
            }
        }
    }
}

/// 由 split key 生成首尾相接的 Region 列表，leader 在 store 间轮转。
/// 空 EndKey 表示 +inf，与 TiKV meta 约定一致。
fn build_regions(store_ids: &[u64], split_keys: &[Vec<u8>]) -> Vec<TestRegion> {
    let mut keys = split_keys.to_vec();
    keys.sort();
    keys.dedup();
    let mut bounds: Vec<Vec<u8>> = vec![Vec::new()];
    bounds.extend(keys);
    // end key empty means +inf
    // 最后一个区间 EndKey 留空表示正无穷。
    let mut regions = Vec::new();
    for i in 0..bounds.len() {
        let start = bounds[i].clone();
        let end = if i + 1 < bounds.len() {
            bounds[i + 1].clone()
        } else {
            Vec::new()
        };
        // leader 轮转分配，保证多 store 场景都会被 Connect。
        let leader = store_ids[i % store_ids.len()];
        regions.push(TestRegion {
            meta: metapb::Region {
                Id: (i as u64) + 1,
                StartKey: start,
                EndKey: end,
                RegionEpoch: Some(metapb::RegionEpoch {
                    ConfVer: 1,
                    Version: 1,
                }),
            },
            leader,
        });
    }
    regions
}

/// 生成近似 `size` 个可排序的 split key，供快速构造多 region 拓扑。
/// 编码方式故意简单：只为得到稳定、去重后的键空间切分点。
fn dummy_regions(size: usize) -> Vec<Vec<u8>> {
    let mut res = Vec::new();
    for i in 0..size {
        let mut s = Vec::new();
        let mut j = i;
        if j == 0 {
            // empty start handled by bounds; use "a" style keys from 0..
            // i==0 时走下方空串补 `a`，避免与全局空 StartKey 冲突。
        }
        while j > 0 {
            s.push(b'a' + (j % 26) as u8);
            j /= 26;
        }
        if s.is_empty() {
            s.push(b'a');
        }
        res.push(s);
    }
    res.sort();
    res.dedup();
    res
}

impl Env for MockStores {
    /// 返回拓扑中全部 store（无 Labels），供 prepare 枚举连接目标。
    fn GetAllLiveStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        let stores = self.stores.lock().unwrap();
        Ok(stores
            .keys()
            .map(|id| metapb::Store {
                Id: *id,
                Labels: Vec::new(),
            })
            .collect())
    }

    /// 懒创建 MockStore，经 `AdaptForGRPCInTest` 包装后返回。
    /// 顺序：on_connect → 创建/回调 → 可选 connect_delay 阻塞 → 返回 client。
    fn ConnectToStore(&self, _ctx: &Context, storeID: u64) -> Result<Arc<dyn PrepareClient>> {
        if let Some(cb) = self.on_connect.lock().unwrap().as_ref() {
            cb(storeID)?;
        }
        // 先取出延迟 channel，避免长时间持有 stores 锁。
        let delay_rx = {
            let d = self.connect_delay.lock().unwrap();
            d.as_ref().and_then(|f| f(storeID))
        };

        let client = {
            let mut stores = self.stores.lock().unwrap();
            let entry = stores
                .get_mut(&storeID)
                .ok_or_else(|| Error::new(format!("unknown store {storeID}")))?;
            if entry.is_none() {
                let ms = MockStore::new();
                if let Some(cb) = self.on_create_store.lock().unwrap().as_ref() {
                    cb(&ms);
                }
                *entry = Some(Arc::clone(&ms));
            }
            // 测试适配层模拟 gRPC 边界，与生产包装路径形状一致。
            AdaptForGRPCInTest(Arc::clone(entry.as_ref().unwrap()) as Arc<dyn PrepareClient>)
        };

        if let Some(rx) = delay_rx {
            // 阻塞到外部信号，用于并发时序类用例。
            let _ = rx.recv();
        }
        Ok(client)
    }

    /// 按半开区间 [startKey, endKey) 过滤重叠 region。
    /// 空 endKey 提升为极大哨兵，模拟 TiKV 的 +inf 语义。
    fn LoadRegionsInKeyRange(
        &self,
        _ctx: &Context,
        startKey: &[u8],
        endKey: &[u8],
    ) -> Result<Vec<Box<dyn Region>>> {
        let mut end = endKey.to_vec();
        if end.is_empty() {
            // 空 end 视为全范围查询上界。
            end = vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        }
        let regions = self.regions.lock().unwrap();
        let mut out: Vec<Box<dyn Region>> = Vec::new();
        for r in regions.iter() {
            // overlap with [start, end)
            // 区间重叠：region.start < query.end && region.end > query.start。
            let rs = r.meta.StartKey.as_slice();
            let re = if r.meta.EndKey.is_empty() {
                end.as_slice()
            } else {
                r.meta.EndKey.as_slice()
            };
            // region overlaps if rs < end && (re > start || re empty as +inf already handled)
            let region_end = if r.meta.EndKey.is_empty() {
                &[0xffu8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff][..]
            } else {
                r.meta.EndKey.as_slice()
            };
            if rs < end.as_slice() && region_end > startKey {
                out.push(Box::new(r.clone()));
            }
        }
        Ok(out)
    }
}

/// 总入口：串联四条与 Go prepare_test 对齐的契约子场景。
/// 拆成子函数便于失败时定位，也避免单测超时把多场景绑死。
#[test]
fn go_rust_public_contract_matches() {
    contract_normal_basic_prepare();
    contract_boundary_split_requests();
    contract_error_retry_limit();
    contract_resource_finalize_clears_lease();
}

/// Go `AssertSafeForBackup` permits overlapping coverage and rejects only holes.
#[test]
fn overlapping_regions_are_safe_like_go() {
    let stores = MockStores::new(&[1], &[]);
    let store = MockStore::new();
    {
        let mut inner = store.inner.lock().unwrap();
        inner.lease_until = Some(inner.base + Duration::from_secs(30));
        inner.success_regions = vec![
            metapb::Region {
                Id: 1,
                StartKey: b"a".to_vec(),
                EndKey: b"d".to_vec(),
                RegionEpoch: None,
            },
            metapb::Region {
                Id: 2,
                StartKey: b"c".to_vec(),
                EndKey: b"e".to_vec(),
                RegionEpoch: None,
            },
        ];
    }
    *stores.stores.lock().unwrap().get_mut(&1).unwrap() = Some(store);

    stores.assert_safe_for_backup();
}

/// 正常路径：DriveLoop 成功 → 备份窗口安全 → Finalize 后回到普通模式。
/// 对应 Go 基础 prepare 成功用例的核心断言。
fn contract_normal_basic_prepare() {
    let keys = dummy_regions(20);
    let ms = MockStores::new(&[1, 2, 3], &keys);
    let ctx = Context::background();
    let mut prep = New(ms.clone() as Arc<dyn Env>);
    // 30s lease 足够覆盖本测驱动循环，避免误触发续约失败。
    prep.LeaseDuration = Duration::from_secs(30);
    prep.DriveLoopAndWaitPrepare(&ctx)
        .expect("drive prepare should succeed");
    assert!(prep.wait_apply_finished());
    ms.assert_safe_for_backup();
    prep.Finalize(&ctx).expect("finalize");
    ms.assert_normal_mode();
}

/// 边界：`SplitRequestClient` 按 MaxRequestSize 拆分 WaitApply。
/// 大 region 元数据时一 region 一包；小 region 可合并；总数必须守恒。
fn contract_boundary_split_requests() {
    /// 只计数 Send，不实现 Recv——本场景只验证拆包发送侧。
    struct CounterClient {
        /// 累计 Send 次数。
        send: Mutex<usize>,
        /// 收集到的全部 region，用于校验未丢失。
        regions: Mutex<Vec<metapb::Region>>,
    }
    impl PrepareClient for CounterClient {
        fn Send(&self, req: &brpb::PrepareSnapshotBackupRequest) -> Result<()> {
            *self.send.lock().unwrap() += 1;
            self.regions
                .lock()
                .unwrap()
                .extend(req.Regions.iter().cloned());
            Ok(())
        }
        fn Recv(&self) -> Result<brpb::PrepareSnapshotBackupResponse> {
            // 本契约不走收包路径；误调用即测试设计错误。
            panic!("not implemented")
        }
    }

    let counter = Arc::new(CounterClient {
        send: Mutex::new(0),
        regions: Mutex::new(Vec::new()),
    });
    // MaxRequestSize=1024 对齐生产拆包量级的缩小版阈值。
    let cc = SplitRequestClient {
        PrepareClient: counter.clone() as Arc<dyn PrepareClient>,
        MaxRequestSize: 1024,
    };

    // 构造 n 个 region，每个键长度约 `each`，用于控制单请求体积。
    let make_regions = |n: usize, each: usize| -> Vec<metapb::Region> {
        (0..n)
            .map(|i| {
                let mut start = vec![0u8; each.saturating_sub(1)];
                start.push(i as u8);
                let mut end = vec![0u8; each.saturating_sub(1)];
                end.push((i + 1) as u8);
                metapb::Region {
                    Id: i as u64 + 1,
                    StartKey: start,
                    EndKey: end,
                    RegionEpoch: None,
                }
            })
            .collect()
    };

    // 100×128B：体积超过阈值，应拆成多次 Send（≥20）。
    let huge = brpb::PrepareSnapshotBackupRequest {
        Ty: brpb::PrepareSnapshotBackupRequestType::WaitApply,
        Regions: make_regions(100, 128),
        LeaseInSeconds: 0,
    };
    cc.Send(&huge).unwrap();
    assert!(*counter.send.lock().unwrap() >= 20);
    assert_eq!(counter.regions.lock().unwrap().len(), 100);

    // 10×2048B：单 region 即超阈值，期望恰好 10 次 Send。
    *counter.send.lock().unwrap() = 0;
    counter.regions.lock().unwrap().clear();
    let really_huge = brpb::PrepareSnapshotBackupRequest {
        Ty: brpb::PrepareSnapshotBackupRequestType::WaitApply,
        Regions: make_regions(10, 2048),
        LeaseInSeconds: 0,
    };
    cc.Send(&really_huge).unwrap();
    assert_eq!(*counter.send.lock().unwrap(), 10);

    // 10×10B：整体远小于阈值，应合并为单次 Send。
    *counter.send.lock().unwrap() = 0;
    counter.regions.lock().unwrap().clear();
    let tiny = brpb::PrepareSnapshotBackupRequest {
        Ty: brpb::PrepareSnapshotBackupRequestType::WaitApply,
        Regions: make_regions(10, 10),
        LeaseInSeconds: 0,
    };
    cc.Send(&tiny).unwrap();
    assert_eq!(*counter.send.lock().unwrap(), 1);
}

/// 错误路径：WaitApply 持续失败时必须在 RetryLimit 后退出。
/// 同时校验退避总耗时下限，防止“立刻失败”绕过重试语义。
fn contract_error_retry_limit() {
    let keys = dummy_regions(10);
    let ms = MockStores::new(&[1, 2, 3], &keys);
    // 每个新 store 的 WaitApply 一律失败，触发 Preparer 重试。
    *ms.on_create_store.lock().unwrap() = Some(Box::new(|store: &Arc<MockStore>| {
        *store.on_wait_apply.lock().unwrap() = Some(Box::new(|_| Err(Error::new("failed meow"))));
    }));
    let ctx = Context::background();
    let mut prep = New(ms.clone() as Arc<dyn Env>);
    prep.RetryBackoff = Duration::from_millis(20);
    prep.RetryLimit = 3;
    prep.LeaseDuration = Duration::from_secs(30);
    let started = Instant::now();
    let err = prep
        .DriveLoopAndWaitPrepare(&ctx)
        .expect_err("should fail after retries");
    // 消息对齐 Go retryLimitExceeded 文案，兼容本地包装后的子串。
    assert!(
        err.to_string().contains("the limit of retrying exceeded")
            || err.to_string().contains("retry"),
        "got {err}"
    );
    // 至少经历若干次回退睡眠（3 次 × 20ms 量级的下限放宽到 50ms）。
    assert!(started.elapsed() >= Duration::from_millis(50));
    let _ = prep.Finalize(&ctx);
}

/// 资源清理：Finalize 必须清 lease；并顺带校验错误辅助与 Retry 包装 Env。
/// 后半段把“连接失败一次再成功”交给 `RetryAndSplitRequestEnv`，对齐 Go 重试包装。
fn contract_resource_finalize_clears_lease() {
    let keys = dummy_regions(8);
    let ms = MockStores::new(&[1, 2], &keys);
    let ctx = Context::background();
    let mut prep = New(ms.clone() as Arc<dyn Env>);
    prep.LeaseDuration = Duration::from_secs(30);
    prep.DriveLoopAndWaitPrepare(&ctx).unwrap();
    prep.Finalize(&ctx).unwrap();
    ms.assert_normal_mode();

    // error helpers
    // 错误构造器文案必须与 Go errors.go 保持一致，供上层按字符串分支。
    assert_eq!(
        crate::errors::leaseExpired().message(),
        "the lease has expired"
    );
    assert_eq!(
        crate::errors::unsupported().message(),
        "unsupported operation"
    );
    assert_eq!(
        crate::errors::retryLimitExceeded().message(),
        "the limit of retrying exceeded"
    );
    assert!(crate::errors::convertErr(None).is_none());
    assert_eq!(
        crate::errors::convertErr(Some(&errorpb::Error {
            Message: "x".into()
        }))
        .unwrap()
        .message(),
        "x"
    );

    // Retry env wraps connect failures
    // 首次 Connect 失败、第二次成功：验证 RetryAndSplitRequestEnv 会消化瞬时错误。
    let ms2 = MockStores::new(&[1], &dummy_regions(4));
    let fail_once = AtomicBool::new(true);
    *ms2.on_connect.lock().unwrap() = Some(Box::new(move |_| {
        if fail_once.swap(false, Ordering::SeqCst) {
            Err(Error::new("nya?"))
        } else {
            Ok(())
        }
    }));
    let retry_env = Arc::new(RetryAndSplitRequestEnv {
        Env: ms2.clone() as Arc<dyn Env>,
        GetBackoffStrategy: Some(Box::new(|| {
            Box::new(LimitedBackoff {
                remaining: 2,
                delay: Duration::ZERO,
            })
        })),
    });
    let mut prep2 = New(retry_env as Arc<dyn Env>);
    // 第二次连接应成功；若仍失败说明重试包装未生效。
    prep2.LeaseDuration = Duration::from_secs(30);
    prep2.DriveLoopAndWaitPrepare(&ctx).unwrap();
    // Finalize 再次确认无残留 lease（与首段场景同一约束）。
    prep2.Finalize(&ctx).unwrap();
}
