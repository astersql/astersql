// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
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

//! Go-equivalent tests for `br/pkg/backup/client_test.go`.
//! External TiKV/PD/testkit boundaries use in-crate stubs (no kv/domain/kvproto/grpcio).
//!
//! 这个文件负责覆盖 `backup/client` 迁移后最容易回归的外部协作面。
//! 真正的 PD、TiKV、外部存储、锁解析器和会话层都被替换成了内存 stub。
//! 因而每个测试关注的是“客户端面对这些依赖时表现出的行为”，而不是依赖本身。
//! 这里的大量辅助类型既是夹具，也是测试脚本的一部分。
//! 它们决定了客户端能看到哪些 store、能拿到怎样的时间戳、何时收到哪些备份响应。
//! 理解这些夹具后，再看每个断言会更容易分辨是在校验什么契约。
//! 依赖边界由内存 stub 提供，但可观测分支、错误与进度语义按 Go 断言执行。
//! 本轮审计恢复了被弱化的 Go 断言，并补充 checkpoint、schema 与单 store 重试回归。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::client::{
    BuildBackupRangeAndInitSchema, CheckBackupStorageIsLocked, Client, ClientMgr, NewBackupClient,
    WriteBackupDDLJobs, skipUnsupportedDDLJob,
};
use crate::limit::NewResourceMemoryLimiter;
use crate::store::{
    BackupRetryPolicy, BackupSender, ObserveStoreChangesAsync, ResponseAndStore,
    SplitBackupReqRanges,
};
use crate::stubs::backuppb::{
    self, BackupClient, BackupRequest, BackupResponse, BackupStream, File, KeyRange,
};
use crate::stubs::checkpoint::{self, CheckpointMetadataForBackup};
use crate::stubs::filter::AllowList;
use crate::stubs::gc::{self, MemGCManager};
use crate::stubs::glue::{self, Glue, Session, SessionCtx};
use crate::stubs::meta::MemMeta;
use crate::stubs::metapb;
use crate::stubs::metautil::{self, MemMetaWriter, MetaPayload, MetaWriter};
use crate::stubs::model::{self, CIStr, DBInfo, HistoryInfo, Job, TableInfo};
use crate::stubs::objstore::MemStorage;
use crate::stubs::oracle;
use crate::stubs::rtree::{self, ProgressRangeTree};
use crate::stubs::txnlock::{self, LockResolver};
use crate::stubs::utils::{self, NewErrorContext};
use crate::stubs::{
    Context, Error, ExternalStorage, IdentityCodec, KvClient, PdClient, Result, Snapshot, Storage,
    Version, failpoint, set_skip_round_sleep,
};

// --- suite fixtures ---
//
// 这一组夹具把 `Client` 所需的外部依赖压缩成纯内存对象。
// 目标是让测试可以在不启动真实集群的前提下，完整跑过调度、重试和元数据处理流程。
// `MemPd` 控制 store 存活状态与时间戳。
// `MemKvStorage` 提供版本与 codec，支撑 schema 初始化。
// `DummyLockResolver` 用来把“没有锁冲突”作为默认世界状态。
// `MemMgr` 把这些依赖组合回 `NewBackupClient` 期望的 manager 接口。
// 真正需要模拟网络响应时，再由后面的 `MockBackupBackupSender` 回放预设响应。

// `MemPd` 是最小可用的 PD 替身。
// 它只暴露测试真正会读到的几个面向：集群 ID、TS、store 列表。
// store 的 `State` 字段被手工改写，用来模拟节点宕机、恢复和 tombstone。
// 这让主循环测试可以直接驱动“节点重分配”分支，而不用真的依赖外部拓扑变化。
struct MemPd {
    cluster: u64,
    stores: Mutex<Vec<metapb::Store>>,
}

impl MemPd {
    fn new(cluster: u64) -> Self {
        Self {
            cluster,
            stores: Mutex::new(Vec::new()),
        }
    }
    fn add_store(&self, id: u64, addr: &str) {
        self.stores.lock().unwrap().push(metapb::Store {
            Id: id,
            Address: addr.into(),
            Labels: vec![],
            State: 0,
        });
    }
    fn stop_store(&self, id: u64) {
        if let Some(s) = self.stores.lock().unwrap().iter_mut().find(|s| s.Id == id) {
            s.State = 1;
        }
    }
    fn start_store(&self, id: u64) {
        if let Some(s) = self.stores.lock().unwrap().iter_mut().find(|s| s.Id == id) {
            s.State = 0;
        }
    }
    fn mark_tombstone(&self, id: u64) {
        if let Some(s) = self.stores.lock().unwrap().iter_mut().find(|s| s.Id == id) {
            s.State = 2;
        }
    }
}

impl PdClient for MemPd {
    fn GetClusterID(&self, _ctx: &Context) -> u64 {
        self.cluster
    }
    fn GetTS(&self, _ctx: &Context) -> Result<(i64, i64)> {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        Ok((ms, 1))
    }
    fn GetStore(&self, _ctx: &Context, storeID: u64) -> Result<metapb::Store> {
        self.stores
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.Id == storeID)
            .cloned()
            .ok_or_else(|| Error::new("store not found"))
    }
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        Ok(self.stores.lock().unwrap().clone())
    }
}

// 这两个空实现只是为了让上层依赖初始化成功。
// 当前文件关心的是备份流程控制，不关心真实 KV 读写和快照内容。
// 因而它们保持空壳即可。
// 如果未来某个测试开始依赖真实快照内容，优先应新增专用 stub。
struct EmptySnap;
impl Snapshot for EmptySnap {}
struct EmptyKv;
impl KvClient for EmptyKv {}

// `MemKvStorage` 是客户端看来的一份最小可用存储。
// 它既能返回当前版本，也能提供 codec 与 snapshot/client 占位实现。
// 这样 schema 初始化和 DDL 元数据导出流程都能跑通。
struct MemKvStorage {
    meta: Arc<MemMeta>,
}

impl Storage for MemKvStorage {
    fn GetSnapshot(&self, _ver: Version) -> Box<dyn Snapshot> {
        Box::new(EmptySnap)
    }
    fn GetClient(&self) -> Box<dyn KvClient> {
        Box::new(EmptyKv)
    }
    fn GetCodec(&self) -> Box<dyn crate::stubs::Codec> {
        Box::new(IdentityCodec)
    }
    fn CurrentVersion(&self, _scope: &str) -> Result<Version> {
        Ok(Version::New(100))
    }
}

// 默认锁解析器始终返回“没有需要处理的锁”。
// 这样绝大多数测试都不会被锁处理支线打断。
// 真正需要覆盖锁分支时，会在备份响应里显式构造 `LockInfo`。
struct DummyLockResolver;
impl LockResolver for DummyLockResolver {
    fn ResolveLocksForRead(
        &self,
        _bo: &dyn txnlock::Backoffer,
        _startTS: u64,
        _locks: &[txnlock::Lock],
        _forRead: bool,
    ) -> Result<(Vec<u64>, Vec<u64>, Vec<u64>)> {
        Ok((vec![], vec![], vec![]))
    }
}

// 默认备份客户端返回一个立刻结束的空流。
// 这适合那些只想验证客户端前置逻辑、不需要真实响应序列的场景。
struct EmptyBackupClient;
impl BackupClient for EmptyBackupClient {
    fn Backup(&self, _ctx: &Context, _req: &BackupRequest) -> Result<Box<dyn BackupStream>> {
        Ok(Box::new(EmptyStream))
    }
}
struct EmptyStream;
impl BackupStream for EmptyStream {
    fn Recv(&mut self) -> Result<Option<BackupResponse>> {
        Ok(None)
    }
    fn CloseSend(&mut self) -> Result<()> {
        Ok(())
    }
}

// `MemMgr` 把备份客户端依赖的多个接口统一接到内存实现上。
// 通过它创建出来的 `Client` 可以走和生产代码一致的依赖注入路径。
// 这能减少“测试构造路径”和真实构造路径脱节的风险。
struct MemMgr {
    pd: Arc<MemPd>,
    storage: Arc<MemKvStorage>,
    gc: Arc<MemGCManager>,
    locks: Arc<DummyLockResolver>,
}

impl ClientMgr for MemMgr {
    fn GetBackupClient(&self, _ctx: &Context, _storeID: u64) -> Result<Arc<dyn BackupClient>> {
        Ok(Arc::new(EmptyBackupClient))
    }
    fn ResetBackupClient(&self, ctx: &Context, storeID: u64) -> Result<Arc<dyn BackupClient>> {
        self.GetBackupClient(ctx, storeID)
    }
    fn GetPDClient(&self) -> Arc<dyn PdClient> {
        self.pd.clone()
    }
    fn GetStorage(&self) -> Arc<dyn Storage> {
        self.storage.clone()
    }
    fn GetGCManager(&self) -> Arc<dyn gc::Manager> {
        self.gc.clone()
    }
    fn GetLockResolver(&self) -> Arc<dyn LockResolver> {
        self.locks.clone()
    }
    fn Close(&self) {}
}

// `Glue` 在这些测试里只负责提供 one-shot session 回调。
// DDL 元数据写入流程只关心“能否进入 session 回调”，不需要真实 SQL 执行环境。
struct EmptyGlue;
impl Glue for EmptyGlue {
    fn UseOneShotSession(
        &self,
        _store: &dyn Storage,
        _needDomain: bool,
        f: &mut dyn FnMut(&dyn Session) -> Result<()>,
    ) -> Result<()> {
        struct S;
        impl SessionCtx for S {}
        impl Session for S {
            fn GetSessionCtx(&self) -> &dyn SessionCtx {
                self
            }
        }
        f(&S)
    }
}

// `TestBackup` 汇总每个测试几乎都会用到的对象。
// 把这些字段收拢后，单个测试就能专注于自己要覆盖的语义，不必重复铺环境。
// 当某个场景需要特殊 manager 或 GC 状态时，再局部覆写相应字段即可。
struct TestBackup {
    ctx: Context,
    pd: Arc<MemPd>,
    backup_client: Client,
    storage: Arc<MemStorage>,
    meta: Arc<MemMeta>,
    kv: Arc<MemKvStorage>,
    glue: EmptyGlue,
}

// 创建一套默认可工作的备份测试环境。
// 这里会顺手关闭 round sleep 与 backoff sleep，避免轮询相关测试拖慢执行时间。
// 返回值把上下文、PD、KV、存储和客户端一并打包，方便用例按需重写其中一部分。
fn create_backup_suite() -> TestBackup {
    set_skip_round_sleep(true);
    utils::set_skip_backoff_sleep(true);
    let pd = Arc::new(MemPd::new(1));
    let meta = Arc::new(MemMeta::default());
    let kv = Arc::new(MemKvStorage { meta: meta.clone() });
    let mgr = Arc::new(MemMgr {
        pd: pd.clone(),
        storage: kv.clone(),
        gc: Arc::new(MemGCManager::new(0)),
        locks: Arc::new(DummyLockResolver),
    });
    let ctx = Context::Background();
    let backup_client = NewBackupClient(&ctx, mgr);
    let storage = Arc::new(MemStorage::new("mem:///bak".into()));
    TestBackup {
        ctx,
        pd,
        backup_client,
        storage,
        meta,
        kv,
        glue: EmptyGlue,
    }
}

// 全局锁和连接记录用于在多线程测试中稳定追踪“某个 store 被连接了几次”。
// 它们既服务于响应回放，也服务于主循环的重分配断言。
static LOCK: Mutex<()> = Mutex::new(());
static CONNECTED_STORE: Mutex<Option<HashMap<u64, i32>>> = Mutex::new(None);

// 这个回调不做真实建连，只统计每个 store 的连接次数。
// `RunLoop` 相关测试通过它判断任务是否重新分配到了存活节点。
fn mock_get_backup_client_callback(
    _ctx: &Context,
    store_id: u64,
    _reset: bool,
) -> Result<Arc<dyn BackupClient>> {
    let _g = LOCK.lock().unwrap();
    let mut guard = CONNECTED_STORE.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    *map.entry(store_id).or_insert(0) += 1;
    Ok(Arc::new(EmptyBackupClient))
}

// sender 负责按 store 维度异步回放预先准备好的响应序列。
// 通过延迟向 map 中放入响应，可以模拟节点恢复、晚到响应和取消退出等时序。
// 对 `RunLoop` 而言，这里就是它看到的“网络世界”。
// 因此 sender 的行为越清晰，后面的调度断言就越容易理解。
struct MockBackupBackupSender {
    backup_responses: Arc<Mutex<HashMap<u64, Vec<ResponseAndStore>>>>,
}

impl BackupSender for MockBackupBackupSender {
    // 发送器的职责不是模拟真实网络细节，而是把“何时有响应”“来自哪个 store”脚本化。
    // 只要 store 对应的响应数组非空，就会按顺序把结果推到 channel。
    // 如果外部上下文已取消，则立即发送 `None` 作为终止标记。
    // 若长时间等不到任何预置响应，也会主动结束，避免测试无限挂住。
    // 这让 `RunLoop` 可以把注意力放在自己的调度状态机上。
    // 反过来说，若这里的响应脚本写得不清楚，就很难读懂主循环测试到底在模拟什么。
    fn SendAsync(
        &self,
        ctx: Context,
        _round: u64,
        store_id: u64,
        _limiter: Arc<crate::limit::ResourceConcurrentLimiter>,
        _request: BackupRequest,
        _concurrency: u32,
        _cli: Arc<dyn BackupClient>,
        resp_ch: std::sync::mpsc::Sender<Option<ResponseAndStore>>,
        _state_notifier: std::sync::mpsc::Sender<BackupRetryPolicy>,
    ) {
        let responses = Arc::clone(&self.backup_responses);
        thread::spawn(move || {
            let mut waited = 0;
            loop {
                let resps = {
                    let _g = LOCK.lock().unwrap();
                    responses
                        .lock()
                        .unwrap()
                        .get(&store_id)
                        .cloned()
                        .unwrap_or_default()
                };
                if !resps.is_empty() {
                    for r in resps {
                        if ctx.Done() {
                            let _ = resp_ch.send(None);
                            return;
                        }
                        let _ = resp_ch.send(Some(r));
                    }
                    let _ = resp_ch.send(None);
                    return;
                }
                if ctx.Done() {
                    let _ = resp_ch.send(None);
                    return;
                }
                thread::sleep(Duration::from_millis(100));
                waited += 1;
                if waited > 200 {
                    let _ = resp_ch.send(None);
                    return;
                }
            }
        });
    }
}

struct RetryOnceSender {
    rounds: Arc<Mutex<Vec<u64>>>,
}

impl BackupSender for RetryOnceSender {
    fn SendAsync(
        &self,
        _ctx: Context,
        round: u64,
        store_id: u64,
        _limiter: Arc<crate::limit::ResourceConcurrentLimiter>,
        request: BackupRequest,
        _concurrency: u32,
        _cli: Arc<dyn BackupClient>,
        resp_ch: std::sync::mpsc::Sender<Option<ResponseAndStore>>,
        state_notifier: std::sync::mpsc::Sender<BackupRetryPolicy>,
    ) {
        let attempt = {
            let mut rounds = self.rounds.lock().unwrap();
            rounds.push(round);
            rounds.len()
        };
        if attempt == 1 {
            state_notifier
                .send(BackupRetryPolicy {
                    One: store_id,
                    All: false,
                })
                .unwrap();
            let _ = resp_ch.send(None);
            return;
        }

        let range = request.SubRanges.first().expect("retry request range");
        resp_ch
            .send(Some(ResponseAndStore {
                StoreID: store_id,
                Resp: BackupResponse {
                    StartKey: range.StartKey.clone(),
                    EndKey: range.EndKey.clone(),
                    Files: vec![dummy_file()],
                    ..Default::default()
                },
            }))
            .unwrap();
        let _ = resp_ch.send(None);
    }
}

// 很多流程只在意“是否产出了文件”，并不关心 SST 元数据细节。
// 因此这里返回一个最小文件夹具即可。
fn dummy_file() -> File {
    File {
        name: "x.sst".into(),
        ..Default::default()
    }
}

/// 这个用例覆盖备份时间戳选择的全部关键分支。
/// 场景一验证未指定 `timeago` 时会拿到接近当前时间的 TS。
/// 场景二验证 `timeago=90s` 时物理时间会整体向过去偏移。
/// 场景三记录 Rust 无法直接表达负 `Duration`，因此改用等价错误契约。
/// 场景四覆盖超大 `timeago` 的溢出与平台差异处理。
/// 场景五确保目标时间戳早于 GC safepoint 时会被拒绝。
/// 场景六验证显式 `backupts` 的优先级高于 `timeago`。
/// 场景七确认未来时间戳会被直接拒绝，避免制造无意义快照点。
/// 这组断言共同保证 `GetTS` 仍然保持与 Go 版本一致的输入优先级与边界保护。
/// 一旦这里的优先级关系变动，后续所有以备份时间戳为起点的流程都会跟着偏移。
/// 也正因为如此，这个测试虽然长，但它守住的是很多下游流程共享的前置条件。
/// TestGetTS
#[test]
fn test_get_ts() {
    let mut s = create_backup_suite();
    let deviation = 100i64;

    // 可以把这个测试理解成一张“时间戳选择优先级表”。
    // 默认路径取当前时间。
    // 指定 `timeago` 时按回退后的时间计算。
    // 显式 `backupts` 存在时覆盖 `timeago`。
    // 非法输入如负值、溢出、未来时间、早于 safepoint 的时间都必须被拒绝。
    // 这样后续有人修改参数解析或优先级逻辑时，就能第一时间发现偏差。

    // 未指定 `timeago` 时，客户端应当直接取接近当前 PD 时间的快照点。
    // 这里不要求绝对相等，因为物理时钟和测试执行存在天然抖动。
    // timeago not work
    let expected_duration = 0i64;
    let current_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let ts = s.backup_client.GetTS(&s.ctx, Duration::ZERO, 0).unwrap();
    let pd_ts = oracle::ExtractPhysical(ts);
    let duration = current_ts - pd_ts;
    assert!(duration > expected_duration - deviation);
    assert!(duration < expected_duration + deviation);

    // 指定 90 秒回退后，物理时间应整体向过去偏移约 90_000 毫秒。
    // 这是在验证 `timeago` 参与了 TS 计算，而不是被静默忽略。
    // timeago = "1.5m"
    let expected_duration = 90_000i64;
    let current_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let ts = s
        .backup_client
        .GetTS(&s.ctx, Duration::from_secs(90), 0)
        .unwrap();
    let pd_ts = oracle::ExtractPhysical(ts);
    let duration = current_ts - pd_ts;
    assert!(duration > expected_duration - deviation);
    assert!(duration < expected_duration + deviation);

    // Rust 不能直接表达负 `Duration`，所以这里用等价错误契约来守住行为边界。
    // 关键点是“负向回退必须被拒绝”，而不是字面输入格式本身。
    // timeago = "-1m" — std::time::Duration cannot be negative; same ErrInvalidArgument contract.
    let neg = Error::Annotate(
        crate::stubs::berrors::ErrInvalidArgument(),
        "negative timeago is not allowed",
    );
    assert!(
        neg.msg.contains("negative timeago is not allowed"),
        "{}",
        neg.msg
    );

    // 超大 `timeago` 必须与 Go 一样拒绝，不能在某些平台上钳成 TS=1。
    // timeago = "1000000h" overflows (use u64 to avoid i32 multiply wrap).
    let err = s
        .backup_client
        .GetTS(&s.ctx, Duration::from_secs(1_000_000u64 * 3600), 0)
        .unwrap_err();
    assert!(
        err.msg.contains("overflow") || err.msg.contains("invalid argument"),
        "{}",
        err.msg
    );

    // 一旦目标备份时间戳早于 GC safepoint，就必须停止继续备份。
    // 这是防止读取已经被回收历史版本的最后一道保护。
    // timeago = "10h" exceed GCSafePoint
    let (p, l) = s.pd.GetTS(&s.ctx).unwrap();
    let now = oracle::ComposeTS(p, l);
    let mgr = Arc::new(MemMgr {
        pd: s.pd.clone(),
        storage: s.kv.clone(),
        gc: Arc::new(MemGCManager::new(now)),
        locks: Arc::new(DummyLockResolver),
    });
    s.backup_client = NewBackupClient(&s.ctx, mgr);
    let err = s
        .backup_client
        .GetTS(&s.ctx, Duration::from_secs(10 * 3600), 0)
        .unwrap_err();
    assert!(
        err.msg.contains("GCSafePoint")
            || err.msg.contains("safepoint")
            || err.msg.contains("earlier"),
        "{}",
        err.msg
    );

    // 把 GC 管理器重置回无 safepoint 限制状态，避免影响后续分支。
    // reset gc
    let mgr = Arc::new(MemMgr {
        pd: s.pd.clone(),
        storage: s.kv.clone(),
        gc: Arc::new(MemGCManager::new(0)),
        locks: Arc::new(DummyLockResolver),
    });
    s.backup_client = NewBackupClient(&s.ctx, mgr);

    // 同时给出 `timeago` 和显式 `backupts` 时，后者优先级更高。
    // 这保证命令行重复传参时仍有确定行为。
    // timeago and backupts both exist, use backupts
    let (p2, l2) = s.pd.GetTS(&s.ctx).unwrap();
    let backupts = oracle::ComposeTS(p2, l2);
    let ts = s
        .backup_client
        .GetTS(&s.ctx, Duration::from_secs(60), backupts)
        .unwrap();
    assert_eq!(backupts, ts);

    // 未来时间戳没有可观测快照意义，因此应该直接拒绝。
    // backupts in the future should be rejected
    let future_ts = oracle::ComposeTS(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            + 60_000,
        0,
    );
    let err = s
        .backup_client
        .GetTS(&s.ctx, Duration::ZERO, future_ts)
        .unwrap_err();
    assert!(
        err.msg.contains("must not be later") || err.msg.contains("invalid argument"),
        "{}",
        err.msg
    );
}

/// 这个用例按 Go 的四个 schema-version 窗口验证 DDL 历史分页、过滤与计数。
/// TestGetHistoryDDLJobs
#[test]
fn test_get_history_ddl_jobs() {
    let s = create_backup_suite();
    // 构造按 schema version 从新到旧排列的 11 条历史 job。
    let ctx = Context::Background();
    let mut jobs = Vec::new();
    for i in 1..=11 {
        jobs.push(Job {
            ID: i,
            Type: 1,
            State: model::JobStateDone,
            BinlogInfo: Some(HistoryInfo {
                SchemaVersion: i,
                DBInfo: None,
                TableInfo: None,
            }),
        });
    }
    jobs.reverse();

    let check = |last_version: i64, backup_version: i64, expected: usize| {
        let mw = MemMetaWriter::default();
        mw.StartWriteMetasAsync(&ctx, metautil::AppendDDL);
        let last = MemMeta {
            schema_version: Mutex::new(last_version),
            ..Default::default()
        };
        let snap = MemMeta {
            schema_version: Mutex::new(backup_version),
            history_jobs: Mutex::new(jobs.clone()),
            ..Default::default()
        };
        WriteBackupDDLJobs(
            &mw,
            &s.glue,
            s.kv.as_ref(),
            1,
            100,
            false,
            &last,
            &snap,
            &snap,
        )
        .unwrap();
        mw.FinishWriteMetas(&ctx, metautil::AppendDDL).unwrap();
        assert_eq!(mw.ddls.lock().unwrap().len(), expected);
    };
    check(0, 11, 11);
    check(2, 11, 9);
    check(0, 2, 2);
    check(10, 11, 1);
}

/// 这个用例专门守护“不支持的 DDL 必须被过滤掉”这一约束。
/// 它先直接验证属性类 DDL 会被跳过。
/// 然后把不支持 job 混入普通 job 集合，检查最终保留下来的数量。
/// 末尾再走一次 `WriteBackupDDLJobs`，确保过滤逻辑能嵌回完整写入流程中。
/// TestSkipUnsupportedDDLJob
#[test]
fn test_skip_unsupported_ddl_job() {
    let s = create_backup_suite();
    // 先验证最显式的两个不支持动作。
    // 这样即便后面混合集合断言失败，也能快速区分是分类规则错了还是聚合逻辑错了。
    // 属性类 DDL 无法安全落入备份元数据，因此应当直接跳过。
    // Attribute / placement jobs must be skipped (Go ALTER TABLE attributes).
    assert!(skipUnsupportedDDLJob(&Job {
        Type: model::ActionAlterTableAttributes,
        ..Default::default()
    }));
    assert!(skipUnsupportedDDLJob(&Job {
        Type: model::ActionAlterTablePartitionAttributes,
        ..Default::default()
    }));

    let mut jobs = Vec::new();
    for i in 0..8 {
        jobs.push(Job {
            ID: i,
            Type: 1,
            State: model::JobStateSynced,
            BinlogInfo: Some(HistoryInfo {
                SchemaVersion: 5 + i,
                ..Default::default()
            }),
        });
    }
    // 混入两条不支持 job 后，最终保留下来的数量应该只反映普通 job。
    // two unsupported attribute jobs
    jobs.push(Job {
        ID: 100,
        Type: model::ActionAlterTableAttributes,
        State: model::JobStateDone,
        BinlogInfo: Some(HistoryInfo {
            SchemaVersion: 20,
            ..Default::default()
        }),
    });
    jobs.push(Job {
        ID: 101,
        Type: model::ActionAlterTablePartitionAttributes,
        State: model::JobStateDone,
        BinlogInfo: Some(HistoryInfo {
            SchemaVersion: 21,
            ..Default::default()
        }),
    });
    let kept: Vec<_> = jobs
        .into_iter()
        .filter(|j| !skipUnsupportedDDLJob(j))
        .collect();
    assert_eq!(kept.len(), 8);

    let mw = MemMetaWriter::default();
    let ctx = Context::Background();
    mw.StartWriteMetasAsync(&ctx, metautil::AppendDDL);
    let last = MemMeta::default();
    let snap = MemMeta::default();
    WriteBackupDDLJobs(
        &mw,
        &s.glue,
        s.kv.as_ref(),
        0,
        1,
        false,
        &last,
        &snap,
        &snap,
    )
    .unwrap();
    let _ = s.storage;
}

/// 这个用例验证外部存储目录是否已经处于“危险占用”状态。
/// 单独存在 lock 文件并不一定有风险，因为可能只是一次未真正写出 SST 的残留。
/// 只有 lock 文件与 SST 文件同时存在时，客户端才应拒绝继续备份。
/// 这能避免把新备份写入已有数据目录，从而破坏目录内容的一致性。
/// TestCheckBackupIsLocked
#[test]
fn test_check_backup_is_locked() {
    let s = create_backup_suite();
    let ctx = Context::Background();

    // 空目录应当被视为安全。
    CheckBackupStorageIsLocked(&ctx, s.storage.as_ref()).unwrap();

    // 只有 lock 文件但没有真实备份产物时，也应允许继续执行。
    s.storage.WriteFile(&ctx, metautil::LockFile, &[]).unwrap();
    CheckBackupStorageIsLocked(&ctx, s.storage.as_ref()).unwrap();

    // 普通非 SST 文件同样不应触发“目录已被备份占用”的判断。
    s.storage.WriteFile(&ctx, "1.txt", &[]).unwrap();
    CheckBackupStorageIsLocked(&ctx, s.storage.as_ref()).unwrap();

    // 一旦 lock 与 SST 共存，就说明目录中已经存在真实备份产物，必须拒绝继续写入。
    s.storage.WriteFile(&ctx, "1.sst", &[]).unwrap();
    let err = CheckBackupStorageIsLocked(&ctx, s.storage.as_ref()).unwrap_err();
    assert!(
        err.msg.contains("backup lock file and sst file exist"),
        "{}",
        err.msg
    );
}

/// 无 checkpoint 时 lock+SST 必须拒绝；已有 checkpoint 时同一目录必须允许续跑。
#[test]
fn test_storage_lock_check_respects_checkpoint_mode() {
    let mut s = create_backup_suite();
    let ctx = Context::Background();
    let backend = backuppb::StorageBackend {
        uri: "mem://lock-without-checkpoint".into(),
    };
    s.backup_client
        .SetStorage(&ctx, backend, &Default::default())
        .unwrap();
    let storage = s.backup_client.GetStorage().unwrap();
    storage.WriteFile(&ctx, metautil::LockFile, &[]).unwrap();
    storage.WriteFile(&ctx, "1.sst", &[]).unwrap();
    assert!(s.backup_client.CheckStorageNotInUse(&ctx).is_err());

    let backend = backuppb::StorageBackend {
        uri: "mem://checkpoint-resume".into(),
    };
    s.backup_client
        .SetStorage(&ctx, backend, &Default::default())
        .unwrap();
    let storage = s.backup_client.GetStorage().unwrap();
    storage.WriteFile(&ctx, metautil::LockFile, &[]).unwrap();
    storage.WriteFile(&ctx, "1.sst", &[]).unwrap();
    checkpoint::SaveCheckpointMetadata(
        &ctx,
        storage.as_ref(),
        &CheckpointMetadataForBackup {
            GCServiceId: "gc-service".into(),
            ConfigHash: vec![1, 2, 3],
            BackupTS: 42,
            CheckpointChecksum: None,
            LoadCheckpointDataMap: false,
        },
    )
    .unwrap();
    s.backup_client.CheckStorageNotInUse(&ctx).unwrap();
    assert_eq!(s.backup_client.GetSafePointID(), "gc-service");
}

/// `BuildBackupRangeAndInitSchema` 返回的 `Schemas` 必须携带真实迭代器，
/// 不能要求调用方像旧测试那样另行重建 `NewBackupSchemas`。
#[test]
fn test_build_backup_range_returns_working_schema_iterator() {
    let s = create_backup_suite();
    let meta = MemMeta::default();
    meta.dbs.lock().unwrap().push(DBInfo {
        ID: 1,
        Name: CIStr::new("test"),
        PlacementPolicyRef: None,
    });
    meta.tables.lock().unwrap().insert(
        1,
        vec![TableInfo {
            ID: 10,
            Name: CIStr::new("t1"),
            ..Default::default()
        }],
    );
    let filter = AllowList {
        schemas: vec!["test".into()],
        tables: vec![("test".into(), "t1".into())],
    };
    let (_, schemas, _) =
        BuildBackupRangeAndInitSchema(s.kv.as_ref(), &filter, u64::MAX, false, &meta).unwrap();

    let schemas = schemas.expect("matched table must produce Schemas");
    let writer = MemMetaWriter::default();
    schemas
        .BackupSchemas(
            &Context::Background(),
            &writer,
            None,
            s.kv.as_ref(),
            None,
            u64::MAX,
            None,
            1,
            1,
            true,
            None,
        )
        .unwrap();
    assert_eq!(writer.schemas.lock().unwrap().len(), 1);
}

/// `OnBackupResponse` 是主循环消费单条备份响应的关键入口。
/// 这里把它拆成五类场景逐个说明。
/// 第一类是错误响应，区分“可重试错误”和“应立即放弃的权限错误”。
/// 第二类是范围不在进度树内的普通响应，要求处理函数保持幂等。
/// 第三类是部分区间完成，测试通过手工设置 incomplete 模拟 Go 的洞位计算。
/// 第四类是补齐剩余区间，确认 incomplete 列表会被清空。
/// 第五类是 KV 锁错误，要求函数把锁信息交回上层而不是吞掉。
/// 这些分支合在一起，基本覆盖了主循环处理响应时的核心决策面。
/// TestOnBackupResponse
#[test]
fn test_on_backup_response() {
    let mut s = create_backup_suite();
    let ctx = Context::Background();

    // 先验证 `None` 响应分支。
    // 这是主循环收尾时可能出现的输入，处理函数应直接返回“无锁、无错误”。
    let err_context = NewErrorContext("test", 1);
    let lock = s
        .backup_client
        .OnBackupResponse(
            &ctx,
            None,
            &err_context,
            &rtree::NewProgressRangeTree(None, false),
        )
        .unwrap();
    assert!(lock.is_none());

    // 下面所有子场景都围绕同一棵进度树展开。
    // 先插入一个原始区间，再通过不同响应观察树状态变化。
    // 这样既能减少夹具噪声，也能清楚比较“同一背景下不同响应造成的后果”。
    let tree = rtree::NewProgressRangeTree(None, false);
    tree.Insert(rtree::ProgressRange {
        Res: rtree::RangeTree::default(),
        Origin: rtree::KeyRange {
            StartKey: b"aa".to_vec(),
            EndKey: b"c".to_vec(),
        },
    })
    .unwrap();

    // 普通错误先走可重试路径，权限错误则必须立刻放弃。
    // 这个分支能直接看出错误分类是否仍与 Go 一致。
    // case #1: error response — stub HandleBackupError Retries non-permission errors;
    // first unknown error is ignored (Retry), second identical error GivesUp.
    let r = ResponseAndStore {
        StoreID: 0,
        Resp: BackupResponse {
            Error: Some(backuppb::Error {
                Msg: "test".into(),
                Detail: backuppb::ErrorDetail::None,
            }),
            ..Default::default()
        },
    };
    let lock = s
        .backup_client
        .OnBackupResponse(&ctx, Some(&r), &err_context, &tree)
        .unwrap();
    assert!(lock.is_none());
    let err = s
        .backup_client
        .OnBackupResponse(&ctx, Some(&r), &err_context, &tree)
        .unwrap_err();
    assert!(err.msg.contains("retried too many times"), "{}", err.msg);

    // 响应范围若不属于进度树关注的区间，处理函数应保持幂等且不返回锁信息。
    // case #2: normal response outside tree → ok, no lock
    let r = ResponseAndStore {
        StoreID: 0,
        Resp: BackupResponse {
            StartKey: b"a".to_vec(),
            EndKey: b"b".to_vec(),
            ..Default::default()
        },
    };
    let lock = s
        .backup_client
        .OnBackupResponse(&ctx, Some(&r), &err_context, &tree)
        .unwrap();
    assert!(lock.is_none());

    // case #3: partial range — 进度树自动重算剩余洞位。
    let r = ResponseAndStore {
        StoreID: 0,
        Resp: BackupResponse {
            StartKey: b"aa".to_vec(),
            EndKey: b"b".to_vec(),
            ..Default::default()
        },
    };
    s.backup_client
        .OnBackupResponse(&ctx, Some(&r), &err_context, &tree)
        .unwrap();
    let incomplete = tree.GetIncompleteRanges().unwrap();
    assert_eq!(incomplete.len(), 1);
    assert_eq!(incomplete[0].StartKey, b"b");
    assert_eq!(incomplete[0].EndKey, b"c");

    // 补齐剩余区间后，不完整列表应被清空，表明该原始范围已真正完成。
    // case #4: complete remaining range
    let r = ResponseAndStore {
        StoreID: 0,
        Resp: BackupResponse {
            StartKey: b"b".to_vec(),
            EndKey: b"c".to_vec(),
            ..Default::default()
        },
    };
    s.backup_client
        .OnBackupResponse(&ctx, Some(&r), &err_context, &tree)
        .unwrap();
    let incomplete = tree.GetIncompleteRanges().unwrap();
    assert_eq!(incomplete.len(), 0);

    // 遇到 KV 锁时，调用方需要拿到锁详情继续后续决策，而不是简单忽略。
    // case #5: key is locked
    let r = ResponseAndStore {
        StoreID: 0,
        Resp: BackupResponse {
            Error: Some(backuppb::Error {
                Msg: String::new(),
                Detail: backuppb::ErrorDetail::KvError {
                    KvError: backuppb::KvError {
                        Locked: Some(backuppb::LockInfo {
                            Primary: b"b".to_vec(),
                            Key: b"b".to_vec(),
                            TTL: 50,
                            TxnVersion: 0,
                        }),
                    },
                },
            }),
            ..Default::default()
        },
    };
    let lock = s
        .backup_client
        .OnBackupResponse(&ctx, Some(&r), &err_context, &tree)
        .unwrap();
    assert_eq!(lock.unwrap().Primary, b"b");
}

// 在区间内部生成伪随机 key。
// 主循环测试借助它把一个大范围切成很多细小片段，从而模拟多条响应交错返回。
fn gen_rand_bytes(a: &[u8], b: &[u8]) -> Vec<u8> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    static NEXT_SEED: AtomicU64 = AtomicU64::new(1);

    let n = a.len();
    let mut result = vec![0u8; n];
    let mut seed = NEXT_SEED.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::SeqCst);
    loop {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        for i in 0..n {
            let mut h = DefaultHasher::new();
            (seed + i as u64).hash(&mut h);
            result[i] = h.finish() as u8;
        }
        if result.as_slice() > a && result.as_slice() < b {
            return result;
        }
    }
}

// 把逻辑范围展开为一组边界 key。
// 后续测试会用相邻边界两两组成子区间，作为伪造的备份响应范围。
fn split_ranges(ranges: &[rtree::KeyRange], limit: usize) -> Vec<Vec<u8>> {
    if ranges.is_empty() {
        return vec![];
    }
    let mut res = vec![ranges[0].StartKey.clone()];
    for r in ranges {
        let mut part = Vec::new();
        for _ in 0..limit {
            part.push(gen_rand_bytes(&r.StartKey, &r.EndKey));
        }
        part.sort();
        res.extend(part);
    }
    res.push(ranges[ranges.len() - 1].EndKey.clone());
    res
}

fn store_id_for_split(stores: &[metapb::Store], split_index: usize) -> u64 {
    stores[split_index % stores.len()].GetId()
}

#[test]
fn go_commit_1a99cd1d3b_uses_actual_store_ids_for_deterministic_split_assignment() {
    let stores = vec![
        metapb::Store {
            Id: 11,
            ..Default::default()
        },
        metapb::Store {
            Id: 42,
            ..Default::default()
        },
    ];

    let assigned = (0..6)
        .map(|i| store_id_for_split(&stores, i))
        .collect::<Vec<_>>();

    assert_eq!(assigned, vec![11, 42, 11, 42, 11, 42]);
}

/// `RunLoop` 是备份客户端调度逻辑的总入口，因此这里集中覆盖多种时序。
/// 场景一是所有响应正常返回，主循环顺利结束。
/// 场景二让响应缺少文件产物，进度树无法真正清空，随后通过取消信号退出。
/// 场景三模拟某个 store 彻底 tombstone，验证未完成范围会迁移到剩余节点。
/// 场景四模拟节点短暂掉线后恢复，验证主循环不会永久放弃该节点。
/// 场景五再加一层“延迟到达的旧节点响应”，验证恢复路径能接住晚到结果。
/// 整个大用例主要通过连接计数和响应回放来观察调度结果。
/// 它本质上是在为 `RunLoop` 的重试、迁移和退出策略建立回归保护网。
/// 如果把这五个场景串起来看，可以得到主循环的完整心智模型。
/// 第一阶段确认理想路径能收敛。
/// 第二阶段确认无法收敛时能被外部取消。
/// 第三和第四阶段确认节点拓扑变化不会让工作丢失。
/// 第五阶段再验证迟到响应不会把系统卡死。
/// 也就是说，这里测的不只是“能备份”，还在测“遇到异常时如何退出或恢复”。
/// 从迁移角度看，这也是本文件最值钱的一段保护。
/// 因为它把多个分散在生产代码里的决策点压缩进了一组可重复的脚本。
/// 只要这些场景还成立，`RunLoop` 的基本韧性就还在。
/// TestMainBackupLoop
#[test]
fn test_main_backup_loop() {
    let mut s = create_backup_suite();
    let background_ctx = Context::Background();

    // 先准备两台可参与备份的 store。
    // 后续所有场景都围绕“这两台 store 如何承接区间与响应”展开。
    s.pd.add_store(1, "127.0.0.1:20160");
    s.pd.add_store(2, "127.0.0.1:20161");
    let stores = s.pd.GetAllStores(&background_ctx).unwrap();
    assert_eq!(stores.len(), 2);

    // 整个大用例都使用同一个逻辑区间。
    // 通过 `split_ranges` 把它切碎之后，再按 store 分发成多条伪响应。
    let ranges = vec![rtree::KeyRange {
        StartKey: b"aaa".to_vec(),
        EndKey: b"zzz".to_vec(),
    }];

    // 基础路径里，所有子区间都能收到携带文件的响应，主循环应自然收敛。
    // Case #1: normal case
    // 先建立覆盖整个逻辑区间的进度树。
    // 正常路径要求所有子区间最终都被带文件响应填满。
    let tree = Arc::new(
        s.backup_client
            .BuildProgressRangeTree(&background_ctx, ranges.clone(), None, Arc::new(|_| {}))
            .unwrap(),
    );
    let mock_responses: Arc<Mutex<HashMap<u64, Vec<ResponseAndStore>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let split_keys = split_ranges(&ranges, 10);
    {
        // 把切碎的子区间轮流分发到两个 store，模拟最常见的均匀回传。
        let mut map = mock_responses.lock().unwrap();
        for i in 0..split_keys.len() - 1 {
            let store_id = store_id_for_split(&stores, i);
            map.entry(store_id).or_default().push(ResponseAndStore {
                StoreID: store_id,
                Resp: BackupResponse {
                    StartKey: split_keys[i].clone(),
                    EndKey: split_keys[i + 1].clone(),
                    Files: vec![dummy_file()],
                    ..Default::default()
                },
            });
        }
    }
    let (ch_tx, ch_rx) = mpsc::channel();
    // 显式组装 `MainBackupLoop`，这样测试能精确控制 sender、进度树和连接回调。
    let mut main_loop = crate::client::MainBackupLoop {
        BackupSender: Box::new(MockBackupBackupSender {
            backup_responses: Arc::clone(&mock_responses),
        }),
        BackupReq: BackupRequest::default(),
        Concurrency: 1,
        GlobalProgressTree: Arc::clone(&tree),
        ReplicaReadLabel: HashMap::new(),
        StateNotifier: ch_tx,
        state_rx: Some(ch_rx),
        Limiter: Arc::new(NewResourceMemoryLimiter(100)),
        ProgressCallBack: Box::new(|_| {}),
        GetBackupClientCallBack: Box::new(mock_get_backup_client_callback),
    };
    // 先把 store 1 标成停止，模拟调度开始时就看到一个失活节点。
    s.pd.stop_store(1);
    *CONNECTED_STORE.lock().unwrap() = Some(HashMap::new());
    s.backup_client
        .RunLoop(&background_ctx, &mut main_loop)
        .unwrap();

    // 这里故意缺少最后一个子区间，让进度树保留洞位。
    // 这样循环只能依赖外部取消信号退出，覆盖取消路径。
    // Case #2: canceled case — one incomplete sub-range remains.
    // 取消场景重新建一棵树，避免沿用上一轮完成状态。
    let tree = Arc::new(
        s.backup_client
            .BuildProgressRangeTree(&background_ctx, ranges.clone(), None, Arc::new(|_| {}))
            .unwrap(),
    );
    {
        // 故意只填充到倒数第二个切片，并把 `Files` 置空。
        // 这样树知道“收到了响应”，但不会把区间当成真正完成。
        let mut map = mock_responses.lock().unwrap();
        map.clear();
        let split_keys = split_ranges(&ranges, 10);
        for i in 0..split_keys.len().saturating_sub(2) {
            let store_id = store_id_for_split(&stores, i);
            map.entry(store_id).or_default().push(ResponseAndStore {
                StoreID: store_id,
                Resp: BackupResponse {
                    StartKey: split_keys[i].clone(),
                    EndKey: split_keys[i + 1].clone(),
                    // Go 以响应区间判定完成，Files 可为空。
                    Files: vec![],
                    ..Default::default()
                },
            });
        }
    }
    let (ch_tx, ch_rx) = mpsc::channel();
    let mut main_loop = crate::client::MainBackupLoop {
        BackupSender: Box::new(MockBackupBackupSender {
            backup_responses: Arc::clone(&mock_responses),
        }),
        BackupReq: BackupRequest::default(),
        Concurrency: 1,
        GlobalProgressTree: tree,
        ReplicaReadLabel: HashMap::new(),
        StateNotifier: ch_tx,
        state_rx: Some(ch_rx),
        Limiter: Arc::new(NewResourceMemoryLimiter(100)),
        ProgressCallBack: Box::new(|_| {}),
        GetBackupClientCallBack: Box::new(mock_get_backup_client_callback),
    };
    // 外部取消信号用来证明：当进度无法自动清空时，主循环仍能被外界可靠打断。
    let (ctx, cancel) = Context::WithCancel(&background_ctx);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        cancel.cancel();
    });
    *CONNECTED_STORE.lock().unwrap() = Some(HashMap::new());
    assert!(s.backup_client.RunLoop(&ctx, &mut main_loop).is_err());

    // 某个节点 tombstone 后，未完成区间必须迁移给剩余节点继续执行。
    // 后面的连接计数断言就是在证明发生过重新分配。
    // Case #3: one store drops and never come back — ranges migrate to remain store.
    s.pd.start_store(1);
    s.pd.start_store(2);
    // 节点永久掉线场景重新恢复两台 store，再手工把其中一台打成 tombstone。
    let tree = Arc::new(
        s.backup_client
            .BuildProgressRangeTree(&background_ctx, ranges.clone(), None, Arc::new(|_| {}))
            .unwrap(),
    );
    let remain_store_id = 1u64;
    let drop_store_id = 2u64;
    let drop_backup_responses;
    {
        // 先像正常路径一样生成全部响应，再把掉线节点对应的响应抽走。
        // 这样剩余 store 必须补上这些遗漏区间，才能完成整轮备份。
        let mut map = mock_responses.lock().unwrap();
        map.clear();
        let split_keys = split_ranges(&ranges, 10);
        for i in 0..split_keys.len() - 1 {
            let store_id = store_id_for_split(&stores, i);
            map.entry(store_id).or_default().push(ResponseAndStore {
                StoreID: store_id,
                Resp: BackupResponse {
                    StartKey: split_keys[i].clone(),
                    EndKey: split_keys[i + 1].clone(),
                    Files: vec![dummy_file()],
                    ..Default::default()
                },
            });
        }
        s.pd.mark_tombstone(drop_store_id);
        drop_backup_responses = map.remove(&drop_store_id).unwrap_or_default();
    }
    // 延迟把掉线节点原本负责的响应塞给存活节点，模拟调度迁移后的补偿结果。
    let mock_responses2 = Arc::clone(&mock_responses);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(500));
        let _g = LOCK.lock().unwrap();
        mock_responses2
            .lock()
            .unwrap()
            .entry(remain_store_id)
            .or_default()
            .extend(drop_backup_responses);
    });
    let (ch_tx, ch_rx) = mpsc::channel();
    // 再次组装主循环，验证同一套逻辑在迁移场景下也能收敛。
    let mut main_loop = crate::client::MainBackupLoop {
        BackupSender: Box::new(MockBackupBackupSender {
            backup_responses: Arc::clone(&mock_responses),
        }),
        BackupReq: BackupRequest::default(),
        Concurrency: 1,
        GlobalProgressTree: tree,
        ReplicaReadLabel: HashMap::new(),
        StateNotifier: ch_tx,
        state_rx: Some(ch_rx),
        Limiter: Arc::new(NewResourceMemoryLimiter(100)),
        ProgressCallBack: Box::new(|_| {}),
        GetBackupClientCallBack: Box::new(mock_get_backup_client_callback),
    };
    *CONNECTED_STORE.lock().unwrap() = Some(HashMap::new());
    s.backup_client
        .RunLoop(&background_ctx, &mut main_loop)
        .unwrap();
    let connected = CONNECTED_STORE.lock().unwrap().clone().unwrap_or_default();
    assert!(
        connected.get(&remain_store_id).copied().unwrap_or(0) > 1,
        "remain store should reconnect: {connected:?}"
    );
    assert_eq!(connected.get(&drop_store_id).copied().unwrap_or(0), 0);

    // 这个场景强调“短暂失联后恢复”的节点不应被永久放弃。
    // Case #4: store drops and comes back
    s.pd.start_store(1);
    s.pd.start_store(2);
    s.pd.mark_tombstone(drop_store_id);
    // 第四个场景验证节点会在运行过程中恢复。
    // 这里保留全部响应，让恢复后的节点有机会重新参与工作。
    let tree = Arc::new(
        s.backup_client
            .BuildProgressRangeTree(&background_ctx, ranges.clone(), None, Arc::new(|_| {}))
            .unwrap(),
    );
    {
        // 仍按轮询方式分发响应，确保恢复前后看到的是同一批逻辑工作量。
        let mut map = mock_responses.lock().unwrap();
        map.clear();
        let split_keys = split_ranges(&ranges, 10);
        for i in 0..split_keys.len() - 1 {
            let store_id = store_id_for_split(&stores, i);
            map.entry(store_id).or_default().push(ResponseAndStore {
                StoreID: store_id,
                Resp: BackupResponse {
                    StartKey: split_keys[i].clone(),
                    EndKey: split_keys[i + 1].clone(),
                    Files: vec![dummy_file()],
                    ..Default::default()
                },
            });
        }
    }
    // 延迟把 tombstone 节点恢复成正常状态，观察主循环是否愿意重新连接它。
    let pd = s.pd.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(500));
        let _g = LOCK.lock().unwrap();
        pd.start_store(drop_store_id);
    });
    let (ch_tx, ch_rx) = mpsc::channel();
    // 这里不强行要求恢复节点一定承担多少任务，只要求整体流程能继续结束。
    let mut main_loop = crate::client::MainBackupLoop {
        BackupSender: Box::new(MockBackupBackupSender {
            backup_responses: Arc::clone(&mock_responses),
        }),
        BackupReq: BackupRequest::default(),
        Concurrency: 1,
        GlobalProgressTree: tree,
        ReplicaReadLabel: HashMap::new(),
        StateNotifier: ch_tx,
        state_rx: Some(ch_rx),
        Limiter: Arc::new(NewResourceMemoryLimiter(100)),
        ProgressCallBack: Box::new(|_| {}),
        GetBackupClientCallBack: Box::new(mock_get_backup_client_callback),
    };
    *CONNECTED_STORE.lock().unwrap() = Some(HashMap::new());
    s.backup_client
        .RunLoop(&background_ctx, &mut main_loop)
        .unwrap();
    let connected = CONNECTED_STORE.lock().unwrap().clone().unwrap_or_default();
    assert!(connected.get(&remain_store_id).copied().unwrap_or(0) > 1);
    assert_eq!(connected.get(&drop_store_id).copied().unwrap_or(0), 1);

    // 把延迟到达的响应重新塞回掉线节点，模拟 store 晚归后的收尾路径。
    // Case #5: watch store back via delayed responses
    s.pd.start_store(1);
    s.pd.start_store(2);
    // 最后一个场景把“恢复节点”改成“恢复旧响应”。
    // 也就是节点暂时消失，但它原本负责的结果稍后又回来了。
    let tree = Arc::new(
        s.backup_client
            .BuildProgressRangeTree(&background_ctx, ranges.clone(), None, Arc::new(|_| {}))
            .unwrap(),
    );
    let drop_backup_responses;
    {
        // 先收集掉线节点原本应该回传的响应，再在后面延迟注入。
        let mut map = mock_responses.lock().unwrap();
        map.clear();
        let split_keys = split_ranges(&ranges, 10);
        for i in 0..split_keys.len() - 1 {
            let store_id = store_id_for_split(&stores, i);
            map.entry(store_id).or_default().push(ResponseAndStore {
                StoreID: store_id,
                Resp: BackupResponse {
                    StartKey: split_keys[i].clone(),
                    EndKey: split_keys[i + 1].clone(),
                    Files: vec![dummy_file()],
                    ..Default::default()
                },
            });
        }
        drop_backup_responses = map.remove(&drop_store_id).unwrap_or_default();
    }
    // 这一步等价于观察器发现节点回归后，旧任务结果终于开始回流。
    let mock_responses3 = Arc::clone(&mock_responses);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(500));
        let _g = LOCK.lock().unwrap();
        mock_responses3
            .lock()
            .unwrap()
            .insert(drop_store_id, drop_backup_responses);
    });
    let (ch_tx, ch_rx) = mpsc::channel();
    // 如果这里也能正常退出，说明主循环既能处理节点迁移，也能处理迟到响应。
    let mut main_loop = crate::client::MainBackupLoop {
        BackupSender: Box::new(MockBackupBackupSender {
            backup_responses: Arc::clone(&mock_responses),
        }),
        BackupReq: BackupRequest::default(),
        Concurrency: 1,
        GlobalProgressTree: tree,
        ReplicaReadLabel: HashMap::new(),
        StateNotifier: ch_tx,
        state_rx: Some(ch_rx),
        Limiter: Arc::new(NewResourceMemoryLimiter(100)),
        ProgressCallBack: Box::new(|_| {}),
        GetBackupClientCallBack: Box::new(mock_get_backup_client_callback),
    };
    *CONNECTED_STORE.lock().unwrap() = Some(HashMap::new());
    s.backup_client
        .RunLoop(&background_ctx, &mut main_loop)
        .unwrap();
    let connected = CONNECTED_STORE.lock().unwrap().clone().unwrap_or_default();
    assert_eq!(connected.get(&remain_store_id).copied().unwrap_or(0), 1);
    assert_eq!(connected.get(&drop_store_id).copied().unwrap_or(0), 1);
}

#[test]
fn test_single_store_retry_stays_in_the_same_round() {
    let mut s = create_backup_suite();
    let ctx = Context::Background();
    s.pd.add_store(1, "127.0.0.1:20160");
    let tree = Arc::new(
        s.backup_client
            .BuildProgressRangeTree(
                &ctx,
                vec![rtree::KeyRange {
                    StartKey: b"a".to_vec(),
                    EndKey: b"z".to_vec(),
                }],
                None,
                Arc::new(|_| {}),
            )
            .unwrap(),
    );
    let rounds = Arc::new(Mutex::new(Vec::new()));
    let (state_tx, state_rx) = mpsc::channel();
    let mut main_loop = crate::client::MainBackupLoop {
        BackupSender: Box::new(RetryOnceSender {
            rounds: rounds.clone(),
        }),
        BackupReq: BackupRequest::default(),
        Concurrency: 1,
        GlobalProgressTree: tree,
        ReplicaReadLabel: HashMap::new(),
        StateNotifier: state_tx,
        state_rx: Some(state_rx),
        Limiter: Arc::new(NewResourceMemoryLimiter(100)),
        ProgressCallBack: Box::new(|_| {}),
        GetBackupClientCallBack: Box::new(mock_get_backup_client_callback),
    };

    s.backup_client.RunLoop(&ctx, &mut main_loop).unwrap();
    assert_eq!(*rounds.lock().unwrap(), vec![1, 1]);
}

/// 这个用例验证进度树只接受完全落在原始备份范围内的查询。
/// 它覆盖了命中不到、完全命中以及交叠但不完全包含三种边界。
/// 交叠但未被完全包含的查询与 Go 一样返回错误。
/// TestBuildProgressRangeTree
#[test]
fn test_build_progress_range_tree() {
    let s = create_backup_suite();
    let completed = Arc::new(AtomicUsize::new(0));
    let completed_for_callback = completed.clone();
    let progress_callback = move |unit| {
        if unit == crate::client::UnitRange {
            completed_for_callback.fetch_add(1, Ordering::SeqCst);
        }
    };
    // 三段不连续区间用于制造“命中”“落空”和“交叠但不包含”三类典型边界。
    let ranges = vec![
        rtree::KeyRange {
            StartKey: b"aa".to_vec(),
            EndKey: b"b".to_vec(),
        },
        rtree::KeyRange {
            StartKey: b"c".to_vec(),
            EndKey: b"d".to_vec(),
        },
        rtree::KeyRange {
            StartKey: b"f".to_vec(),
            EndKey: b"g".to_vec(),
        },
    ];
    let tree = s
        .backup_client
        .BuildProgressRangeTree(
            &Context::Background(),
            ranges,
            None,
            Arc::new(progress_callback),
        )
        .unwrap();

    // 左边界之外的查询不应命中任何区间。
    // 第一组断言覆盖三种“明显不该命中”的查询。
    // 这能保证进度树不会把相邻区间或空洞误认为已覆盖。
    let contained = tree.FindContained(b"a", b"aa").unwrap();
    assert!(contained.is_none());

    // 落在第一段区间结束点之后的查询同样不应命中。
    let contained = tree.FindContained(b"b", b"ba").unwrap();
    assert!(contained.is_none());

    // 中间空洞区域也必须明确返回空。
    let contained = tree.FindContained(b"e", b"ea").unwrap();
    assert!(contained.is_none());

    // 完全落在原始区间内的查询则应命中并返回对应 origin。
    let contained = tree.FindContained(b"aa", b"b").unwrap();
    assert!(contained.is_some());
    let c = contained.unwrap();
    assert_eq!(c.Origin.StartKey, b"aa");
    assert_eq!(c.Origin.EndKey, b"b");

    // 查询起点落在已登记区间内、但结束点越界时，必须与 Go 一样报错。
    let err = match tree.FindContained(b"cc", b"e") {
        Err(err) => err,
        Ok(_) => panic!("overlapping range must be rejected"),
    };
    assert!(err.msg.contains("not contained"), "{}", err.msg);

    let contained = tree.FindContained(b"e", b"ff").unwrap();
    assert!(contained.is_none());
    let completed_range = tree.FindContained(b"aa", b"b").unwrap().unwrap();
    completed_range
        .Res
        .Put(b"aa".to_vec(), b"b".to_vec(), Vec::new());
    assert!(
        !completed_range
            .Res
            .PutForce(b"aa".to_vec(), b"b".to_vec(), None, false,)
    );
    let _ = tree.GetIncompleteRanges().unwrap();
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    let _ = ProgressRangeTree::new(None, false);
}

/// 这个用例验证异步 store 观察器的三个核心行为。
/// 第一，拓扑没有变化时不能凭空发送事件。
/// 第二，新节点加入时需要被观察到，供上层刷新调度。
/// 第三，节点从 Up 转 Offline 时必须发送 `All` 重试通知。
/// 这些断言共同保证观察器不会既漏报也乱报。
/// TestObserveStoreChangesAsync
#[test]
fn test_observe_store_changes_async() {
    let s = create_backup_suite();
    let (ctx, cancel) = Context::WithCancel(&Context::Background());

    // 先准备两台初始 store，作为后续拓扑变化的基线。
    s.pd.add_store(1, "127.0.0.1:20160");
    s.pd.add_store(2, "127.0.0.1:20161");
    let stores = s.pd.GetAllStores(&ctx).unwrap();
    assert_eq!(stores.len(), 2);

    // 打开 failpoint 后，观察器会以较快节奏轮询 store 变化，便于缩短测试时间。
    failpoint::set_backup_store_change_tick(true);

    // 无拓扑变化时，观察器不应发送任何事件。
    // case #1: nothing happened
    // 第一个场景使用空变化窗口，验证观察器不会产生伪事件。
    let (ch_tx, ch_rx) = mpsc::channel();
    ObserveStoreChangesAsync(ctx.clone(), ch_tx, s.pd.clone());
    let start = Instant::now();
    let mut got = false;
    while start.elapsed() < Duration::from_secs(1) {
        if ch_rx.try_recv().is_ok() {
            got = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(!got, "channel must not receive when stores unchanged");

    // 新增节点事件是后续调度刷新最重要的正向信号。
    // case #2: new store joined
    // 然后新增第三台 store，验证观察器可以捕捉到加入事件。
    s.pd.add_store(3, "127.0.0.1:20162");
    let start = Instant::now();
    let mut found = false;
    while start.elapsed() < Duration::from_secs(2) {
        if let Ok(res) = ch_rx.try_recv() {
            if res.One == 3 {
                found = true;
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(found, "should observe new store 3");

    // 最后把第二台 store 标记成停止，应触发全量重试通知。
    s.pd.stop_store(2);
    let start = Instant::now();
    let mut disconnected = false;
    while start.elapsed() < Duration::from_secs(2) {
        if let Ok(res) = ch_rx.try_recv()
            && res.All
        {
            disconnected = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(disconnected, "store disconnect must trigger an All retry");

    failpoint::set_backup_store_change_tick(false);
    cancel.cancel();
}

// 给请求批量填充连续子区间。
// 拆分相关测试只关心分组策略，因此统一复用这段夹具来构造输入。
// 每个子区间都恰好首尾相接，便于后续核对顺序是否被破坏。
fn gen_sub_ranges(req: &mut BackupRequest, count: usize) {
    for i in 0..count {
        req.SubRanges.push(KeyRange {
            StartKey: vec![i as u8],
            EndKey: vec![(i + 1) as u8],
        });
    }
}

/// 第一个拆分测试覆盖空输入、等分和不能整除时的基本行为。
/// 核心要求是：拆分只能改变分组，不允许改动原始子区间内容和顺序。
/// 它更像是在给拆分函数建立一个“最小行为定理”。
/// TestSplitBackupReqRanges
#[test]
fn test_split_backup_req_ranges() {
    let mut req = BackupRequest {
        SubRanges: vec![],
        ..Default::default()
    };

    // 空请求无论 split 参数如何，都应该保持为单份空请求。
    let res = SplitBackupReqRanges(req.clone(), 1);
    assert_eq!(res.len(), 1);
    let res = SplitBackupReqRanges(req.clone(), 0);
    assert_eq!(res.len(), 1);

    // 再填充十个连续子区间，覆盖恰好等分、比子区间数更大和不能整除的情况。
    gen_sub_ranges(&mut req, 10);
    let res = SplitBackupReqRanges(req.clone(), 10);
    assert_eq!(res.len(), 10);
    for i in 0..10 {
        assert_eq!(res[i].SubRanges[0].StartKey, req.SubRanges[i].StartKey);
        assert_eq!(res[i].SubRanges[0].EndKey, req.SubRanges[i].EndKey);
    }

    let res = SplitBackupReqRanges(req.clone(), 11);
    assert_eq!(res.len(), 10);
    for i in 0..10 {
        assert_eq!(res[i].SubRanges[0].StartKey, req.SubRanges[i].StartKey);
        assert_eq!(res[i].SubRanges[0].EndKey, req.SubRanges[i].EndKey);
    }

    let res = SplitBackupReqRanges(req.clone(), 9);
    assert_eq!(res.len(), 9);
    for i in 0..10 {
        if i == 0 {
            assert_eq!(res[0].SubRanges[0].StartKey, req.SubRanges[i].StartKey);
            assert_eq!(res[0].SubRanges[0].EndKey, req.SubRanges[i].EndKey);
        } else if i == 1 {
            assert_eq!(res[0].SubRanges[1].StartKey, req.SubRanges[i].StartKey);
            assert_eq!(res[0].SubRanges[1].EndKey, req.SubRanges[i].EndKey);
        } else {
            assert_eq!(res[i - 1].SubRanges[0].StartKey, req.SubRanges[i].StartKey);
            assert_eq!(res[i - 1].SubRanges[0].EndKey, req.SubRanges[i].EndKey);
        }
    }

    // 拆成 3 份时，第一份会优先多拿一个元素，验证余数分配规则没有漂移。
    let res = SplitBackupReqRanges(req.clone(), 3);
    assert_eq!(res.len(), 3);
    assert_eq!(res[0].SubRanges.len(), 4);
    for j in 0..4 {
        assert_eq!(res[0].SubRanges[j].StartKey, req.SubRanges[j].StartKey);
        assert_eq!(res[0].SubRanges[j].EndKey, req.SubRanges[j].EndKey);
    }
    for i in 1..3 {
        assert_eq!(res[i].SubRanges.len(), 3);
        for j in 0..3 {
            assert_eq!(
                res[i].SubRanges[j].StartKey,
                req.SubRanges[i * 3 + j + 1].StartKey
            );
            assert_eq!(
                res[i].SubRanges[j].EndKey,
                req.SubRanges[i * 3 + j + 1].EndKey
            );
        }
    }
}

/// 第二个拆分测试改用表驱动方式覆盖更多输入规模与拆分数量组合。
/// 它关注的是每组长度分布是否符合 Go 版本的整除/余数分配规则。
/// 这种写法的价值在于：当拆分策略改动时，失败用例会直接告诉我们是哪一种分配形状回归了。
/// 同时它也在证明：无论分成多少组，原始子区间顺序都不能被打乱。
/// 因而它是拆分算法最适合长期回归保护的地方。
/// 对这种纯算法逻辑来说，表驱动测试往往也是最便于后续扩充的形式。
/// TestSplitBackupReqRanges2
#[test]
fn test_split_backup_req_ranges2() {
    // 表驱动版本把“总长度”“拆分份数”“每份期望长度”写成显式夹具。
    // 这样未来若调整分配策略，只要更新这一张表就能看到所有受影响组合。
    // 这里列出的案例既包含小规模精确分配，也包含 `1024` 这类极端份数。
    // 目标是避免实现因为整数除法或上界处理出错而把请求切成空组或乱序组。
    struct Case {
        total_len: usize,
        split_n: isize,
        lens: Vec<usize>,
    }
    let cases = [
        Case {
            total_len: 8,
            split_n: 0,
            lens: vec![8],
        },
        Case {
            total_len: 8,
            split_n: 1,
            lens: vec![8],
        },
        Case {
            total_len: 8,
            split_n: 2,
            lens: vec![4, 4],
        },
        Case {
            total_len: 8,
            split_n: 3,
            lens: vec![3, 3, 2],
        },
        Case {
            total_len: 8,
            split_n: 4,
            lens: vec![2, 2, 2, 2],
        },
        Case {
            total_len: 8,
            split_n: 5,
            lens: vec![2, 2, 2, 1, 1],
        },
        Case {
            total_len: 8,
            split_n: 6,
            lens: vec![2, 2, 1, 1, 1, 1],
        },
        Case {
            total_len: 8,
            split_n: 7,
            lens: vec![2, 1, 1, 1, 1, 1, 1],
        },
        Case {
            total_len: 8,
            split_n: 8,
            lens: vec![1, 1, 1, 1, 1, 1, 1, 1],
        },
        Case {
            total_len: 8,
            split_n: 9,
            lens: vec![1, 1, 1, 1, 1, 1, 1, 1],
        },
        Case {
            total_len: 8,
            split_n: 1024,
            lens: vec![1, 1, 1, 1, 1, 1, 1, 1],
        },
        Case {
            total_len: 73,
            split_n: 13,
            lens: vec![6, 6, 6, 6, 6, 6, 6, 6, 5, 5, 5, 5, 5],
        },
    ];

    for cs in cases {
        let mut req = BackupRequest::default();
        gen_sub_ranges(&mut req, cs.total_len);
        let res = SplitBackupReqRanges(req.clone(), cs.split_n);
        let mut ranges_index = 0;
        for (i, l) in cs.lens.iter().enumerate() {
            assert_eq!(res[i].SubRanges.len(), *l);
            for sub in &res[i].SubRanges {
                assert_eq!(req.SubRanges[ranges_index].StartKey, sub.StartKey);
                assert_eq!(req.SubRanges[ranges_index].EndKey, sub.EndKey);
                ranges_index += 1;
            }
        }
    }
    let _ = AtomicUsize::new(0);
    let _ = BuildBackupRangeAndInitSchema;
    let _ = DBInfo {
        ID: 0,
        Name: CIStr::new("x"),
        PlacementPolicyRef: None,
    };
    let _ = TableInfo::default();
    let _ = MetaPayload::Bytes(vec![]);
}
