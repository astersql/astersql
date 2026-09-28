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

//! Go-equivalent tests for `prepare_snap` (`prepare_test.go`).
//!
//! PD/TiKV/unistore/kvproto boundaries are in-memory Env mocks (darwin arm64:
//! no kv/domain/kvproto/grpcio). Call order, errors, lease/retry/concurrency
//! and cleanup match the Go suite.
//!
//! 场景覆盖刻意与 Go 表驱动顺序一致，便于两侧对照失败点。
//! 延迟 WaitApply 用独立线程 + delayed_wg，避免主线程假死。
//! 连接延迟用例证明：未完成 PrepareConnections 前不应进入可备份态。
//! Finalize 压力用例验证事件排空与错误上浮不被大量消息淹没。
//!
//! 本文件对齐 Go `prepare_test.go` 的场景集，但不依赖 unistore / PD / gRPC。
//! 通过内存 MockStores 验证：基础成功、瞬时失败重试、lease 超时、拆包、
//! 连接延迟、钩子时序，以及 Finalize 期间大量消息与注入错误。
//! 断言关注控制流与错误文案契约；不把 mock 能力描述成生产 TiKV 行为。
//! FakeClock 用于可控地拨快时间，制造 lease 过期而不必真睡两分钟。
//! 与 `parity_test.rs` 共享 mock 思路，但本文件覆盖更多 Go 专有时序分支。

// 标准库：并发队列、原子标志、条件变量与线程，支撑时序敏感用例。
// AtomicBool/AtomicUsize 用于“失败一次”与连接计数等轻量同步。
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

// 被测入口为 prepare::New；Env/PrepareClient 抽象来自 env。
use crate::env::{
    AdaptForGRPCInTest, Context, Env, IsTiFlash, LimitedBackoff, PrepareClient, Region,
    RetryAndSplitRequestEnv, SplitRequestClient, WithRetryV2, brpb, errorpb, metapb,
};
use crate::errors::{Error, Result, eof};
use crate::prepare::{New, Preparer};

#[derive(Clone)]
/// 测试 Region：仅 meta + leader，足够路由 WaitApply。
/// 不模拟 peer 变更，避免干扰契约断言。
struct TestRegion {
    /// Region 元数据（起止键、epoch）。
    meta: metapb::Region,
    /// leader 所在 store id。
    leader: u64,
}

impl Region for TestRegion {
    /// 克隆 meta，避免测试线程共享可变缓冲。
    fn GetMeta(&self) -> metapb::Region {
        self.meta.clone()
    }
    /// 返回 leader store，供 Connect/Send 路由。
    fn GetLeaderStoreID(&self) -> u64 {
        self.leader
    }
}

/// 可拨快的虚拟时钟，对齐 Go 测试里手动推进时间的做法。
/// 未挂接 FakeClock 的 store 仍使用真实 Instant::now。
struct FakeClock {
    /// 时钟原点。
    base: Instant,
    /// 相对原点的偏移；`advance` 累加此值。
    offset: Mutex<Duration>,
}

impl FakeClock {
    /// 构造默认成功的 MockStore，并初始化 self_weak。
    fn new() -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            offset: Mutex::new(Duration::ZERO),
        })
    }

    /// 优先读 FakeClock，否则用真实时间。
    fn now(&self) -> Instant {
        self.base + *self.offset.lock().unwrap()
    }

    /// 拨快时钟，用于制造 lease 过期。
    fn advance(&self, d: Duration) {
        *self.offset.lock().unwrap() += d;
    }
}

/// 单个 mock store 的可变状态：响应队列、lease、成功 WaitApply 列表。
struct MockStoreInner {
    /// 待 Recv 的响应队列。
    queue: VecDeque<brpb::PrepareSnapshotBackupResponse>,
    /// Finish 后关闭，后续 Recv 返回 EOF。
    closed: bool,
    /// lease 截止；None 表示未建立或已清空。
    lease_until: Option<Instant>,
    /// 成功 WaitApply 的 region，供备份窗口覆盖检查。
    success_regions: Vec<metapb::Region>,
}

/// 实现 PrepareClient 的内存 store；比 parity_test 多了延迟线程与错误注入。
/// `self_weak` 供延迟线程在 store 释放后安全退出。
struct MockStore {
    /// 队列与 lease 状态。
    inner: Mutex<MockStoreInner>,
    /// Recv 等待新响应。
    cv: Condvar,
    /// WaitApply 钩子；返回错误写入响应 Error。
    on_wait_apply: Mutex<Option<Box<dyn Fn(&metapb::Region) -> Result<()> + Send + Sync>>>,
    /// 若设置，则在独立线程执行 delay 后再入队响应。
    wait_apply_delay: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// 统计尚未完成的延迟 WaitApply，供 wait_delayed 汇合。
    delayed_wg: Arc<(Mutex<usize>, Condvar)>,
    /// Recv 路径可注入的连接级错误通道。
    inject_conn_err: Mutex<Option<Receiver<Error>>>,
    /// 可选虚拟时钟；影响 lease 判定。
    clock: Mutex<Option<Arc<FakeClock>>>,
    /// 可选响应 epoch 覆盖，用于验证 protobuf nil getter 的零值语义。
    response_epoch: Mutex<Option<metapb::RegionEpoch>>,
    /// 指向自身的弱引用，供延迟线程 upgrade。
    self_weak: Mutex<Weak<MockStore>>,
}

impl MockStore {
    /// 构造默认成功的 MockStore，并初始化 self_weak。
    fn new() -> Arc<Self> {
        let store = Arc::new(Self {
            inner: Mutex::new(MockStoreInner {
                queue: VecDeque::new(),
                closed: false,
                lease_until: None,
                success_regions: Vec::new(),
            }),
            cv: Condvar::new(),
            on_wait_apply: Mutex::new(Some(Box::new(|_| Ok(())))),
            wait_apply_delay: Mutex::new(None),
            delayed_wg: Arc::new((Mutex::new(0usize), Condvar::new())),
            inject_conn_err: Mutex::new(None),
            clock: Mutex::new(None),
            response_epoch: Mutex::new(None),
            self_weak: Mutex::new(Weak::new()),
        });
        // 保存弱引用，供延迟线程在强引用消失时放弃 push。
        *store.self_weak.lock().unwrap() = Arc::downgrade(&store);
        store
    }

    /// 优先读 FakeClock，否则用真实时间。
    fn now(&self) -> Instant {
        if let Some(c) = self.clock.lock().unwrap().as_ref() {
            return c.now();
        }
        Instant::now()
    }

    /// 入队响应并唤醒 Recv。
    fn push(&self, resp: brpb::PrepareSnapshotBackupResponse) {
        let mut g = self.inner.lock().unwrap();
        g.queue.push_back(resp);
        self.cv.notify_all();
    }

    /// 阻塞直到所有延迟 WaitApply 完成。
    fn wait_delayed(&self) {
        let (lock, cv) = &*self.delayed_wg;
        let mut g = lock.lock().unwrap();
        while *g > 0 {
            g = cv.wait(g).unwrap();
        }
    }

    /// 延迟任务开始前增加计数。
    fn bump_delayed(&self) {
        *self.delayed_wg.0.lock().unwrap() += 1;
    }

    /// 延迟任务结束时减计数并广播。
    fn done_delayed(&self) {
        let mut g = self.delayed_wg.0.lock().unwrap();
        *g = g.saturating_sub(1);
        self.delayed_wg.1.notify_all();
    }
}

/// Send/Recv 语义对齐 Go mock：WaitApply 可延迟，UpdateLease/Finish 维护 lease。
impl PrepareClient for MockStore {
    /// 按请求类型处理；未知类型忽略。
    fn Send(&self, req: &brpb::PrepareSnapshotBackupRequest) -> Result<()> {
        match req.Ty {
            // 逐 region 生成 WaitApplyDone；钩子失败写入 Error。
            brpb::PrepareSnapshotBackupRequestType::WaitApply => {
                for region in &req.Regions {
                    let mut response_region = region.clone();
                    if let Some(epoch) = self.response_epoch.lock().unwrap().clone() {
                        response_region.RegionEpoch = Some(epoch);
                    }
                    let mut resp = brpb::PrepareSnapshotBackupResponse {
                        Ty: brpb::PrepareSnapshotBackupEventType::WaitApplyDone,
                        Region: Some(response_region),
                        Error: None,
                        LastLeaseIsValid: false,
                    };
                    {
                        let on = self.on_wait_apply.lock().unwrap();
                        if let Some(cb) = on.as_ref() {
                            if let Err(err) = cb(region) {
                                resp.Error = Some(errorpb::Error {
                                    Message: err.to_string(),
                                });
                            }
                        }
                    }
                    // 仅成功路径记入 success_regions，失败不计入覆盖。
                    if resp.Error.is_none() {
                        self.inner
                            .lock()
                            .unwrap()
                            .success_regions
                            .push(region.clone());
                    }
                    let delay = self.wait_apply_delay.lock().unwrap().clone();
                    // 延迟路径：另起线程执行 delay，再 push，并维护 delayed_wg。
                    if let Some(delay) = delay {
                        self.bump_delayed();
                        let weak = self.self_weak.lock().unwrap().clone();
                        thread::spawn(move || {
                            delay();
                            if let Some(store) = weak.upgrade() {
                                store.push(resp);
                                store.done_delayed();
                            }
                        });
                        // 无延迟：立即入队。
                    } else {
                        self.push(resp);
                    }
                }
            }
            // 对齐 Go：零值或已到期视为 expired，再刷新 lease。
            brpb::PrepareSnapshotBackupRequestType::UpdateLease => {
                let now = self.now();
                let mut g = self.inner.lock().unwrap();
                // LastLeaseIsValid 反映刷新前状态，供流层判断是否已过期。
                let expired = g.lease_until.map(|u| u < now).unwrap_or(true);
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
            // Finish：回报一次 UpdateLeaseResult，清空 lease 并关闭流。
            brpb::PrepareSnapshotBackupRequestType::Finish => {
                let now = self.now();
                let mut g = self.inner.lock().unwrap();
                let expired = g.lease_until.map(|u| u < now).unwrap_or(true);
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
            // 未知类型静默忽略。
            _ => {}
        }
        Ok(())
    }

    /// 本场景不走收包；误调用即测试设计错误。
    fn Recv(&self) -> Result<brpb::PrepareSnapshotBackupResponse> {
        loop {
            {
                let mut inj = self.inject_conn_err.lock().unwrap();
                // 注入错误优先于正常响应，模拟连接异常。
                if let Some(rx) = inj.as_ref() {
                    match rx.try_recv() {
                        Ok(err) => return Err(err),
                        Err(TryRecvError::Empty) => {}
                        Err(TryRecvError::Disconnected) => {
                            *inj = None;
                        }
                    }
                }
            }

            let mut g = self.inner.lock().unwrap();
            if let Some(resp) = g.queue.pop_front() {
                return Ok(resp);
            }
            if g.closed {
                return Err(eof());
            }
            // 短超时等待，便于循环检查 inject_conn_err。
            let (g2, _) = self.cv.wait_timeout(g, Duration::from_millis(20)).unwrap();
            drop(g2);
        }
    }
}

/// 多 store 拓扑 Env；支持创建/连接钩子与连接延迟。
struct MockStores {
    /// store id → 懒创建 MockStore。
    stores: Mutex<HashMap<u64, Option<Arc<MockStore>>>>,
    /// 当前拓扑 region 列表。
    regions: Mutex<Vec<TestRegion>>,
    /// 首次创建 store 时回调。
    on_create_store: Mutex<Option<Box<dyn Fn(&Arc<MockStore>) + Send + Sync>>>,
    /// 连接前钩子，可模拟连不上。
    on_connect: Mutex<Option<Box<dyn Fn(u64) -> Result<()> + Send + Sync>>>,
    /// 返回 Receiver 时阻塞 Connect 直到被唤醒。
    connect_delay: Mutex<Option<Box<dyn Fn(u64) -> Option<Receiver<()>> + Send + Sync>>>,
}

impl MockStores {
    /// 按 store 与 split key 构造拓扑。
    fn new(store_ids: &[u64], split_keys: &[Vec<u8>]) -> Arc<Self> {
        let mut stores = HashMap::new();
        for &id in store_ids {
            stores.insert(id, None);
        }
        Arc::new(Self {
            stores: Mutex::new(stores),
            regions: Mutex::new(build_regions(store_ids, split_keys)),
            on_create_store: Mutex::new(None),
            on_connect: Mutex::new(None),
            connect_delay: Mutex::new(None),
        })
    }

    /// 备份窗口：lease 有效且成功区间无空洞（对齐 Go AssertSafeForBackup）。
    fn assert_safe_for_backup(&self) {
        let stores = self.stores.lock().unwrap();
        let mut ranges: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for store in stores.values().flatten() {
            let g = store.inner.lock().unwrap();
            let now = store.now();
            // lease 必须覆盖当前（虚拟）时间。
            match g.lease_until {
                Some(until) if until >= now => {}
                Some(until) => panic!("lease has expired: at {until:?}, now is {now:?}"),
                None => panic!("lease has expired: missing lease, now is {now:?}"),
            }
            for r in &g.success_regions {
                ranges.push((r.StartKey.clone(), r.EndKey.clone()));
            }
        }
        // 按 StartKey 排序后检查相邻区间无空洞。
        ranges.sort_by(|a, b| a.0.cmp(&b.0));
        for i in 1..ranges.len() {
            if ranges[i - 1].1 < ranges[i].0 {
                panic!(
                    "hole: {} {}",
                    hex_encode(&ranges[i - 1].1),
                    hex_encode(&ranges[i].0)
                );
            }
        }
    }

    /// Finalize 后不应残留有效 lease。
    fn assert_is_normal_mode(&self) {
        let stores = self.stores.lock().unwrap();
        for (id, store) in stores.iter() {
            if let Some(store) = store {
                let g = store.inner.lock().unwrap();
                let now = store.now();
                if let Some(until) = g.lease_until {
                    if until >= now {
                        panic!(
                            "lease in store {id} doesn't expire, the store may not work as normal"
                        );
                    }
                }
            }
        }
    }
}

/// 将键编码为十六进制，便于空洞 panic 信息阅读。
fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 由 split key 生成首尾相接 region；过滤空 key 避免 ( [],[] ) 伪空洞。
fn build_regions(store_ids: &[u64], split_keys: &[Vec<u8>]) -> Vec<TestRegion> {
    let mut keys = split_keys.to_vec();
    keys.sort();
    keys.dedup();
    // Empty key is already the left bound; keep it out of split keys so we never
    // materialize a (start=[], end=[]) region (would look like a coverage hole).
    // 空 split key 已由左界表示，保留会制造空区间。
    keys.retain(|k| !k.is_empty());
    let mut bounds: Vec<Vec<u8>> = vec![Vec::new()];
    bounds.extend(keys);
    let mut regions = Vec::new();
    for i in 0..bounds.len() {
        let start = bounds[i].clone();
        let end = if i + 1 < bounds.len() {
            bounds[i + 1].clone()
        } else {
            Vec::new()
        };
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
            // leader 在 store 间轮转。
            leader: store_ids[i % store_ids.len()],
        });
    }
    regions
}

/// 生成可排序的伪 split key，快速构造多 region。
fn dummy_regions(size: usize) -> Vec<Vec<u8>> {
    let mut res = Vec::new();
    for i in 0..size {
        let mut s = Vec::new();
        let mut j = i;
        while j > 0 {
            s.push(b'a' + (j % 26) as u8);
            j /= 26;
        }
        res.push(s);
    }
    res.sort();
    res
}

/// Env 实现：枚举 store、懒连接、按区间加载 region。
impl Env for MockStores {
    /// 返回全部 store，并按 Id 排序保证确定性。
    fn GetAllLiveStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        let stores = self.stores.lock().unwrap();
        let mut res: Vec<_> = stores
            .keys()
            .map(|id| metapb::Store {
                Id: *id,
                Labels: Vec::new(),
            })
            .collect();
        // 排序消除 HashMap 迭代顺序不确定性。
        res.sort_by_key(|s| s.Id);
        Ok(res)
    }

    /// on_connect → 懒创建 → AdaptForGRPCInTest → 可选延迟阻塞。
    fn ConnectToStore(&self, _ctx: &Context, storeID: u64) -> Result<Arc<dyn PrepareClient>> {
        let client = {
            if let Some(cb) = self.on_connect.lock().unwrap().as_ref() {
                cb(storeID)?;
            }
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
            // 测试适配层模拟 gRPC 边界。
            AdaptForGRPCInTest(Arc::clone(entry.as_ref().unwrap()) as Arc<dyn PrepareClient>)
        };
        let delay_rx = {
            let d = self.connect_delay.lock().unwrap();
            d.as_ref().and_then(|f| f(storeID))
        };
        // 阻塞到外部信号，用于连接延迟用例。
        if let Some(rx) = delay_rx {
            let _ = rx.recv();
        }
        Ok(client)
    }

    /// 半开区间重叠过滤；空 endKey 提升为极大哨兵。
    fn LoadRegionsInKeyRange(
        &self,
        _ctx: &Context,
        startKey: &[u8],
        endKey: &[u8],
    ) -> Result<Vec<Box<dyn Region>>> {
        let mut end = endKey.to_vec();
        // 空 end 视为全范围上界哨兵。
        if end.is_empty() {
            end = vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        }
        let regions = self.regions.lock().unwrap();
        let mut out: Vec<Box<dyn Region>> = Vec::new();
        for r in regions.iter() {
            let rs = r.meta.StartKey.as_slice();
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

/// 在后台线程执行闭包，返回可超时等待的 Receiver。
/// 用于 PrepareConnections 被阻塞时主线程仍能断言中间状态。
fn async_call<T, F>(f: F) -> Receiver<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    // 容量 1：结果就绪前发送方不会无限堆积。
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx
}

/// 按起止键排序后比较 region 集合，忽略原始顺序。
fn elements_match(mut a: Vec<metapb::Region>, mut b: Vec<metapb::Region>) {
    // 拆包可能打乱顺序，比较前统一排序。
    a.sort_by(|x, y| (&x.StartKey, &x.EndKey).cmp(&(&y.StartKey, &y.EndKey)));
    b.sort_by(|x, y| (&x.StartKey, &x.EndKey).cmp(&(&y.StartKey, &y.EndKey)));
    assert_eq!(a, b);
}

/// 正常路径：DriveLoop 成功 → 备份窗口安全 → Finalize 回普通模式。
/// Go TestBasic.
#[test]
fn test_basic() {
    let ms = MockStores::new(&[1, 2, 3], &dummy_regions(100));
    let ctx = Context::background();
    let mut prep = New(ms.clone() as Arc<dyn Env>);
    // 30s lease 覆盖本测驱动循环。
    prep.LeaseDuration = Duration::from_secs(30);
    // Drive → 安全窗口 → Finalize → 普通模式，四步缺一不可。
    prep.DriveLoopAndWaitPrepare(&ctx).expect("prepare");
    ms.assert_safe_for_backup();
    prep.Finalize(&ctx).expect("finalize");
    ms.assert_is_normal_mode();
}

/// Go protobuf getters treat a missing epoch and an explicit zero epoch equally.
#[test]
fn test_missing_and_zero_epoch_match_like_go_getters() {
    let ms = MockStores::new(&[1], &[]);
    ms.regions.lock().unwrap()[0].meta.RegionEpoch = None;
    *ms.on_create_store.lock().unwrap() = Some(Box::new(|store: &Arc<MockStore>| {
        *store.response_epoch.lock().unwrap() = Some(metapb::RegionEpoch::default());
    }));

    let ctx = Context::background();
    let mut prep = New(ms as Arc<dyn Env>);
    prep.PrepareConnections(&ctx).expect("connections");
    prep.AdvanceState(&ctx).expect("send wait apply");
    prep.WaitAndHandleNextEvent(&ctx).expect("handle response");

    assert!(
        prep.wait_apply_finished(),
        "missing and explicit zero epochs must match as in Go protobuf getters"
    );
}

/// WaitApply 持续失败：必须耗尽 RetryLimit 并体现退避耗时。
/// Go TestFailDueToErr.
#[test]
fn test_fail_due_to_err() {
    let ms = MockStores::new(&[1, 2, 3], &dummy_regions(100));
    *ms.on_create_store.lock().unwrap() = Some(Box::new(|store: &Arc<MockStore>| {
        *store.on_wait_apply.lock().unwrap() = Some(Box::new(|_| Err(Error::new("failed meow"))));
    }));
    let ctx = Context::background();
    let mut prep = New(ms.clone() as Arc<dyn Env>);
    // 100ms×3 次退避，总耗时应明显大于 300ms。
    prep.RetryBackoff = Duration::from_millis(100);
    prep.RetryLimit = 3;
    // 30s lease 覆盖本测驱动循环。
    prep.LeaseDuration = Duration::from_secs(30);
    let now = Instant::now();
    // 失败后仍应 Finalize 清理，避免泄漏 lease。
    // 失败后仍应 Finalize 清理，避免泄漏 lease。
    assert!(prep.DriveLoopAndWaitPrepare(&ctx).is_err());
    assert!(now.elapsed() > Duration::from_millis(300));
    prep.Finalize(&ctx).expect("finalize");
    ms.assert_is_normal_mode();
}

/// 每个 store 首次 WaitApply 失败、第二次成功，验证可恢复重试。
/// Go TestError.
#[test]
fn test_error() {
    let ms = MockStores::new(&[1, 2, 3], &dummy_regions(100));
    // 每 store 独立 AtomicBool，避免跨 store 共享“已失败”状态。
    // Go: each store has its own `failed := false` closed over in onCreateStore.
    *ms.on_create_store.lock().unwrap() = Some(Box::new(|store: &Arc<MockStore>| {
        let failed = AtomicBool::new(false);
        *store.on_wait_apply.lock().unwrap() = Some(Box::new(move |_| {
            // 第一次返回失败，后续成功，模拟瞬时 region 错误。
            if !failed.swap(true, Ordering::SeqCst) {
                Err(Error::new("failed"))
            } else {
                Ok(())
            }
        }));
    }));
    let ctx = Context::background();
    let mut prep = New(ms.clone() as Arc<dyn Env>);
    // 零退避加速用例，焦点在“失败一次后成功”。
    prep.RetryBackoff = Duration::ZERO;
    // 30s lease 覆盖本测驱动循环。
    prep.LeaseDuration = Duration::from_secs(30);
    prep.DriveLoopAndWaitPrepare(&ctx).expect("prepare");
    ms.assert_safe_for_backup();
    prep.Finalize(&ctx).expect("finalize");
    ms.assert_is_normal_mode();
}

/// prepare 成功后拨快时钟，Finalize 应因 lease 过期失败。
/// Go TestLeaseTimeout.
#[test]
fn test_lease_timeout() {
    let ms = MockStores::new(&[1, 2, 3], &dummy_regions(100));
    let clock = FakeClock::new();
    *ms.on_create_store.lock().unwrap() = Some(Box::new({
        let clock = Arc::clone(&clock);
        move |store: &Arc<MockStore>| {
            *store.clock.lock().unwrap() = Some(Arc::clone(&clock));
        }
    }));
    let ctx = Context::background();
    let mut prep = New(ms.clone() as Arc<dyn Env>);
    // 30s lease 覆盖本测驱动循环。
    prep.LeaseDuration = Duration::from_secs(30);
    prep.DriveLoopAndWaitPrepare(&ctx).expect("prepare");
    ms.assert_safe_for_backup();
    // 拨快远超 LeaseDuration，迫使 Finalize 观测到过期。
    clock.advance(Duration::from_secs(100 * 60));
    assert!(prep.Finalize(&ctx).is_err());
}

/// 仅 AdvanceState 后拨钟，事件循环应表面 lease expired。
/// Go TestLeaseTimeoutWhileTakingSnapshot.
#[test]
fn test_lease_timeout_while_taking_snapshot() {
    let ms = MockStores::new(&[1, 2, 3], &dummy_regions(100));
    let clock = FakeClock::new();
    *ms.on_create_store.lock().unwrap() = Some(Box::new({
        let clock = Arc::clone(&clock);
        move |store: &Arc<MockStore>| {
            *store.clock.lock().unwrap() = Some(Arc::clone(&clock));
        }
    }));
    let ctx = Context::background();
    let mut prep = New(ms as Arc<dyn Env>);
    // 较短 lease，便于与拨钟/睡眠配合。
    prep.LeaseDuration = Duration::from_secs(4);
    // 不走完整 DriveLoop，以便在 WaitApply 进行中制造超时。
    // Go: AdvanceState alone; it loads holes and dials stores via streamOf.
    prep.AdvanceState(&ctx).expect("AdvanceState");
    // 拨快远超 LeaseDuration，迫使 Finalize 观测到过期。
    clock.advance(Duration::from_secs(100 * 60));
    // 给续约/事件线程一点时间，再进入等待循环。
    thread::sleep(Duration::from_secs(2));
    // 1s 后取消子上下文，防止事件循环永久卡住。
    let (cx, cancel) = Context::with_cancel(&ctx);
    let cancel = Arc::new(cancel);
    let cancel2 = Arc::clone(&cancel);
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(1));
        cancel2.cancel();
    });
    // 循环直到看到 lease expired；取消与父上下文竞态时回退到父 ctx。
    loop {
        match prep.WaitAndHandleNextEvent(&cx) {
            Err(err) => {
                let msg = err.to_string();
                // 文案对齐 Go leaseExpired，防止错误类型被误包装丢掉关键字。
                assert!(
                    msg.contains("the lease has expired"),
                    "expected lease expired, got {msg}"
                );
                break;
            }
            Ok(()) => {
                if cx.is_cancelled() {
                    // 子上下文取消后改用父上下文继续等 lease 事件。
                    // Timeout raced; keep waiting on parent until lease event.
                    match prep.WaitAndHandleNextEvent(&ctx) {
                        Err(err) => {
                            let msg = err.to_string();
                            assert!(
                                msg.contains("the lease has expired"),
                                "expected lease expired, got {msg}"
                            );
                            break;
                        }
                        Ok(()) => continue,
                    }
                }
            }
        }
    }
}

/// 首次 Connect 失败由 RetryAndSplitRequestEnv 消化后应整体成功。
/// Go TestRetryEnv.
#[test]
fn test_retry_env() {
    let tms = MockStores::new(&[1, 2, 3], &dummy_regions(100));
    // 只失败一次，验证有限次退避策略足够。
    let fail_once = Arc::new(AtomicBool::new(true));
    *tms.on_connect.lock().unwrap() = Some(Box::new({
        let fail_once = Arc::clone(&fail_once);
        move |_| {
            if fail_once.swap(false, Ordering::SeqCst) {
                Err(Error::new("nya?"))
            } else {
                Ok(())
            }
        }
    }));
    let ms = Arc::new(RetryAndSplitRequestEnv {
        Env: tms as Arc<dyn Env>,
        GetBackoffStrategy: Some(Box::new(|| {
            // remaining=2 足够覆盖“失败一次”；delay=0 加速测试。
            Box::new(LimitedBackoff {
                remaining: 2,
                delay: Duration::ZERO,
            })
        })),
    });
    let mut prep = New(ms as Arc<dyn Env>);
    // 30s lease 覆盖本测驱动循环。
    prep.LeaseDuration = Duration::from_secs(30);
    let ctx = Context::background();
    // 包装 Env 后完整 prepare/finalize 应成功。
    prep.DriveLoopAndWaitPrepare(&ctx).expect("prepare");
    prep.Finalize(&ctx).expect("finalize");
}

/// 仅计数 Send 的桩客户端，用于拆包边界测试。
struct CounterClient {
    /// 累计 Send 次数。
    send: Mutex<usize>,
    /// 收集到的全部 region，校验无丢失。
    regions: Mutex<Vec<metapb::Region>>,
}

impl PrepareClient for CounterClient {
    /// 按请求类型处理；未知类型忽略。
    fn Send(&self, req: &brpb::PrepareSnapshotBackupRequest) -> Result<()> {
        *self.send.lock().unwrap() += 1;
        self.regions
            .lock()
            .unwrap()
            .extend(req.Regions.iter().cloned());
        Ok(())
    }
    /// 本场景不走收包；误调用即测试设计错误。
    fn Recv(&self) -> Result<brpb::PrepareSnapshotBackupResponse> {
        panic!("not implemented");
    }
}

/// 验证 SplitRequestClient 在不同 region 体积下的拆包次数与完整性。
/// Go TestSplitEnv.
#[test]
fn test_split_env() {
    let counter = Arc::new(CounterClient {
        send: Mutex::new(0),
        regions: Mutex::new(Vec::new()),
    });
    let cc = SplitRequestClient {
        PrepareClient: counter.clone() as Arc<dyn PrepareClient>,
        // 1024 为缩小版阈值，便于在单测中触发拆包。
        MaxRequestSize: 1024,
    };
    // 构造 n 个 region，键长约 each，用于控制单请求体积。
    let make_regions = |n: usize, each: usize| -> Vec<metapb::Region> {
        (0..n)
            .map(|i| {
                let mut start = vec![0u8; each.saturating_sub(1)];
                start.push(i as u8);
                let mut end = vec![0u8; each.saturating_sub(1)];
                end.push((i + 1) as u8);
                metapb::Region {
                    // Id 置 0：拆包测试只关心键体积与数量，不依赖 id。
                    Id: 0,
                    StartKey: start,
                    EndKey: end,
                    RegionEpoch: None,
                }
            })
            .collect()
    };

    // 100×128B：应拆成多次 Send，且 region 全集守恒。
    let huge = brpb::PrepareSnapshotBackupRequest {
        Ty: brpb::PrepareSnapshotBackupRequestType::WaitApply,
        Regions: make_regions(100, 128),
        LeaseInSeconds: 0,
    };
    cc.Send(&huge).unwrap();
    // ≥20 次是体积约束下的松下限，重点是 region 不丢。
    assert!(*counter.send.lock().unwrap() >= 20);
    elements_match(counter.regions.lock().unwrap().clone(), huge.Regions);

    *counter.send.lock().unwrap() = 0;
    counter.regions.lock().unwrap().clear();
    // 10×2048B：每 region 超阈值，期望恰好 10 次 Send。
    let really_huge = brpb::PrepareSnapshotBackupRequest {
        Ty: brpb::PrepareSnapshotBackupRequestType::WaitApply,
        Regions: make_regions(10, 2048),
        LeaseInSeconds: 0,
    };
    cc.Send(&really_huge).unwrap();
    assert_eq!(*counter.send.lock().unwrap(), 10);
    elements_match(counter.regions.lock().unwrap().clone(), really_huge.Regions);

    *counter.send.lock().unwrap() = 0;
    counter.regions.lock().unwrap().clear();
    // 10×10B：合并为单次 Send。
    let tiny = brpb::PrepareSnapshotBackupRequest {
        Ty: brpb::PrepareSnapshotBackupRequestType::WaitApply,
        Regions: make_regions(10, 10),
        LeaseInSeconds: 0,
    };
    cc.Send(&tiny).unwrap();
    assert_eq!(*counter.send.lock().unwrap(), 1);
    elements_match(counter.regions.lock().unwrap().clone(), tiny.Regions);
}

/// 第二个 Connect 阻塞期间，已建连 store 不应提前持有有效 lease。
/// Go TestConnectionDelay.
#[test]
fn test_connection_delay() {
    let ms = MockStores::new(&[1, 2, 3], &dummy_regions(100));
    // 统计 Connect 次数；第 2 次返回 delay channel。
    let called = Arc::new(AtomicUsize::new(0));
    let (delay_tx, delay_rx) = mpsc::channel::<()>();
    let delay_rx = Arc::new(Mutex::new(Some(delay_rx)));
    let (blocked_tx, blocked_rx) = mpsc::sync_channel::<()>(64);
    *ms.connect_delay.lock().unwrap() = Some(Box::new({
        let called = Arc::clone(&called);
        let delay_rx = Arc::clone(&delay_rx);
        let blocked_tx = blocked_tx.clone();
        move |_i| {
            let n = called.fetch_add(1, Ordering::SeqCst) + 1;
            // 仅阻塞第二次连接，模拟慢拨号。
            if n == 2 {
                let _ = blocked_tx.send(());
                delay_rx.lock().unwrap().take()
            } else {
                None
            }
        }
    }));
    let ctx = Context::background();
    let ms2 = Arc::clone(&ms);
    // 后台跑 PrepareConnections，主线程检查中间态。
    let result = async_call(move || {
        let mut prep = New(ms2 as Arc<dyn Env>);
        prep.LeaseDuration = Duration::from_secs(30);
        prep.PrepareConnections(&ctx)
    });
    // 确认已进入阻塞点后再断言 lease 状态。
    blocked_rx.recv().expect("blocked");
    {
        let stores = ms.stores.lock().unwrap();
        let mut non_nil = 0;
        for (_id, store) in stores.iter() {
            if let Some(store) = store {
                let g = store.inner.lock().unwrap();
                let now = Instant::now();
                // 阻塞期间已创建的 store 必须仍无有效 lease。
                assert!(g.lease_until.map(|u| u < now).unwrap_or(true));
                non_nil += 1;
            }
        }
        // 至少两个 store 已创建，说明并行拨号未因单点阻塞全部停住。
        assert!(non_nil >= 2);
    }
    // 放行阻塞连接，期望 PrepareConnections 最终成功。
    delay_tx.send(()).unwrap();
    result
        .recv_timeout(Duration::from_secs(10))
        .expect("join")
        .expect("PrepareConnections");
}

/// AfterConnectionsEstablished 必须在 WaitApply 门闩打开前触发。
/// Go TestHooks.
#[test]
fn test_hooks() {
    let ms = MockStores::new(&[1, 2, 3], &dummy_regions(100));
    // gate=true 时 WaitApply 钩子阻塞，便于观察建连钩子时序。
    let gate = Arc::new((Mutex::new(true), Condvar::new()));
    *ms.on_create_store.lock().unwrap() = Some(Box::new({
        let gate = Arc::clone(&gate);
        move |store: &Arc<MockStore>| {
            let gate = Arc::clone(&gate);
            *store.on_wait_apply.lock().unwrap() = Some(Box::new(move |_| {
                let (lock, cv) = &*gate;
                let mut g = lock.lock().unwrap();
                while *g {
                    g = cv.wait(g).unwrap();
                }
                Ok(())
            }));
        }
    }));

    let established = Arc::new(AtomicBool::new(false));
    let ms_clone = Arc::clone(&ms);
    let flag = Arc::clone(&established);
    let (drive_tx, drive_rx) = mpsc::sync_channel::<Result<()>>(1);
    let (fin_tx, fin_rx) = mpsc::sync_channel::<Result<()>>(1);
    let (safe_tx, safe_rx) = mpsc::sync_channel::<()>(1);
    thread::spawn(move || {
        let mut adv = New(ms_clone as Arc<dyn Env>);
        adv.LeaseDuration = Duration::from_secs(30);
        // 建连完成后置位，主线程轮询此标志。
        adv.AfterConnectionsEstablished = Some(Box::new(move || {
            flag.store(true, Ordering::SeqCst);
        }));
        let drive = adv.DriveLoopAndWaitPrepare(&Context::background());
        let _ = drive_tx.send(drive);
        // 主线程先 assert_safe，再发信号允许 Finalize。
        // Go: AssertSafeForBackup before Finalize — wait for main to check.
        let _ = safe_rx.recv();
        let fin = adv.Finalize(&Context::background());
        let _ = fin_tx.send(fin);
    });

    let deadline = Instant::now() + Duration::from_secs(1);
    // 1s 内必须看到钩子，否则视为钩子未接线。
    while !established.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "hook not fired within 1s");
        thread::sleep(Duration::from_millis(50));
    }
    {
        let (lock, cv) = &*gate;
        // 打开门闩，让 WaitApply 完成。
        *lock.lock().unwrap() = false;
        cv.notify_all();
    }
    // Drive 完成后先确认备份窗口，再允许后台 Finalize。
    drive_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("drive join")
        .expect("DriveLoopAndWaitPrepare");
    ms.assert_safe_for_backup();
    let _ = safe_tx.send(());
    fin_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("finalize join")
        .expect("Finalize");
    ms.assert_is_normal_mode();
}

/// Finalize 前注入连接错误，并在大量延迟消息下验证错误上浮。
/// Go TestManyMessagesWhenFinalizing.
#[test]
fn test_many_messages_when_finalizing() {
    let ms = MockStores::new(&[1, 2, 3], &dummy_regions(256));
    // gate=true 时 WaitApply 钩子阻塞，便于观察建连钩子时序。
    let gate = Arc::new((Mutex::new(true), Condvar::new()));
    // 仅第一个创建的 store 挂上 inject channel。
    let (inject_tx, inject_rx) = mpsc::channel::<Error>();
    let inject_rx = Arc::new(Mutex::new(Some(inject_rx)));
    *ms.on_create_store.lock().unwrap() = Some(Box::new({
        let gate = Arc::clone(&gate);
        let inject_rx = Arc::clone(&inject_rx);
        move |store: &Arc<MockStore>| {
            let gate = Arc::clone(&gate);
            // 延迟回调阻塞，堆积未完成 WaitApply。
            *store.wait_apply_delay.lock().unwrap() = Some(Arc::new(move || {
                let (lock, cv) = &*gate;
                let mut g = lock.lock().unwrap();
                while *g {
                    g = cv.wait(g).unwrap();
                }
            }));
            let mut guard = inject_rx.lock().unwrap();
            if let Some(rx) = guard.take() {
                *store.inject_conn_err.lock().unwrap() = Some(rx);
            }
        }
    }));

    let ms_clone = Arc::clone(&ms);
    let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(1);
    let (drive_tx, drive_rx) = mpsc::sync_channel::<Result<()>>(1);
    let (fin_tx, fin_rx) = mpsc::sync_channel::<Result<()>>(1);
    thread::spawn(move || {
        let mut prep: Preparer = New(ms_clone as Arc<dyn Env>);
        prep.LeaseDuration = Duration::from_secs(30);
        // 先建连，再注入错误，避免连不上导致场景退化。
        prep.PrepareConnections(&Context::background())
            .expect("PrepareConnections");
        let _ = ready_tx.send(());
        // Drive 与 Finalize 结果分别回传，主线程按阶段断言。
        let drive = prep.DriveLoopAndWaitPrepare(&Context::background());
        let _ = drive_tx.send(drive);
        let fin = prep.Finalize(&Context::background());
        let _ = fin_tx.send(fin);
    });
    // 建连完成后再注入，避免错误落在 PrepareConnections。
    ready_rx.recv().unwrap();
    // 短暂等待，确保后台已进入 DriveLoop 收包循环。
    thread::sleep(Duration::from_millis(100));
    // 注入后 DriveLoop 应失败；再放行延迟并断言 Finalize 也失败。
    inject_tx.send(Error::new("whoa!")).unwrap();
    let drive_err = drive_rx.recv_timeout(Duration::from_secs(60)).unwrap();
    assert!(drive_err.is_err(), "DriveLoop should fail: {drive_err:?}");
    {
        let (lock, cv) = &*gate;
        // 打开门闩，让 WaitApply 完成。
        *lock.lock().unwrap() = false;
        cv.notify_all();
    }
    {
        let stores = ms.stores.lock().unwrap();
        for store in stores.values().flatten() {
            store.wait_delayed();
        }
    }
    // 关闭流在错误注入后应失败，对齐 Go “Closing the stream should be error”。
    let fin = fin_rx.recv_timeout(Duration::from_secs(60)).unwrap();
    assert!(fin.is_err(), "Closing the stream should be error");
}

/// Go `context.WithCancel` only cancels the derived context, while cancellation
/// of a parent must remain observable by descendants created from it.
#[test]
fn test_context_cancel_propagation_matches_go() {
    let parent = Context::background();
    let (child, cancel_child) = Context::with_cancel(&parent);

    cancel_child.cancel();
    assert!(child.is_cancelled());
    assert!(
        !parent.is_cancelled(),
        "cancelling a child must not cancel its parent"
    );

    let (parent, cancel_parent) = Context::with_cancel(&Context::background());
    let (child, _cancel_child) = Context::with_cancel(&parent);
    cancel_parent.cancel();
    assert!(parent.is_cancelled());
    assert!(
        child.is_cancelled(),
        "parent cancellation must propagate to its child"
    );
}

/// `engine.IsTiFlash` filters both classic TiFlash and NextGen compute nodes.
#[test]
fn test_tiflash_compute_store_is_filtered() {
    let store = metapb::Store {
        Id: 42,
        Labels: vec![metapb::StoreLabel {
            Key: "engine".into(),
            Value: "tiflash_compute".into(),
        }],
    };
    assert!(IsTiFlash(&store));
}

/// Go `utils.WithRetryV2` returns a multi-error containing every failed attempt.
#[test]
fn test_with_retry_v2_preserves_all_errors() {
    let mut attempt = 0;
    let result: Result<()> = WithRetryV2(
        &Context::background(),
        Box::new(LimitedBackoff {
            remaining: 2,
            delay: Duration::ZERO,
        }),
        |_| {
            attempt += 1;
            Err(Error::new(format!("failure-{attempt}")))
        },
    );
    let message = result.expect_err("all attempts should fail").to_string();
    assert!(
        message.contains("failure-1"),
        "missing first error: {message}"
    );
    assert!(
        message.contains("failure-2"),
        "missing second error: {message}"
    );
}
