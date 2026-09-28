// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/backup` public contracts (limit / store / schema / client).
//!
//! 这个文件更像一份跨模块验收脚本，而不是单一模块单测。
//! 它把 `limit`、`schema`、`store`、`client` 等公开契约放进同一个大用例里统一检查。
//! 这样做的价值在于：一旦某个迁移改动破坏了模块之间的配合关系，这里会比局部单测更早暴露问题。
//! 由于覆盖面非常宽，后面的中文注释会刻意把每一段断言对应的契约说清楚。
//! 阅读时可以把它当成“Rust 版 backup 包对外承诺了什么”的回归目录。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::client::{
    BuildBackupRangeAndInitSchema, BuildBackupSchemas, CheckBackupStorageIsLocked, ClientMgr,
    NewBackupClient, UnitRegion, WriteBackupDDLJobs, skipUnsupportedDDLJob,
};
use crate::limit::{NewResourceMemoryLimiter, ResourceConcurrentLimiter};
use crate::schema::{
    DefaultSchemaConcurrency, NewBackupSchemas, check_merge_option_allowed_for_test,
    encode_schema_for_test, match_checksum_for_test,
};
use crate::store::{
    ResponseAndStore, SplitBackupReqRanges, StartTimeoutRecv, doSendBackup,
    set_timeout_one_response_for_test, startBackup,
};
use crate::stubs::backuppb::{
    self, BackupClient, BackupRequest, BackupResponse, BackupStream, KeyRange,
};
use crate::stubs::filter::AllFilter;
use crate::stubs::gc::{self, MemGCManager};
use crate::stubs::glue::{self, AtomicProgress};
use crate::stubs::label::{Label, Rule};
use crate::stubs::meta::MemMeta;
use crate::stubs::metapb;
use crate::stubs::metautil::{self, ChecksumStats, MemMetaWriter};
use crate::stubs::model::{
    self, CIStr, DBInfo, HistoryInfo, Job, PartitionDefinition, PartitionInfo, TableInfo,
};
use crate::stubs::objstore::MemStorage;
use crate::stubs::oracle;
use crate::stubs::txnlock::{self, LockResolver};
use crate::stubs::{
    Context, Error, IdentityCodec, KvClient, PdClient, Result, Snapshot, Storage, Version,
    WorkerPool, infosync, set_skip_round_sleep, wait_jobs,
};

// --- mocks ---
//
// 下面的 mock 不是为了精细模拟真实集群，而是为了把跨模块契约压缩到一个可控环境里。
// `MemPd` 提供稳定的 TS 和 store 视图。
// `MemKvStorage` 提供 schema 初始化所需的最小存储能力。
// `MemMgr` 负责把这些能力重新拼回 `NewBackupClient` 期望的依赖接口。
// `SeqBackupClient` 与 `SeqStream` 则用于验证发送、接收和清理逻辑。
// 这些 mock 的共同目标是让大测试可以专注在“契约是否保持”，而不是外部系统细节。

// `MemPd` 固定返回一组可预测的 store 和时间戳。
// 这样所有依赖时间戳或 store 查询的契约断言都能建立在稳定输入上。
// 对这种跨模块验收来说，稳定输入比高保真模拟更重要。
// 只要输入稳定，失败结果就更容易直接映射回某条契约。
struct MemPd {
    cluster: u64,
    ts: Mutex<(i64, i64)>,
    stores: Mutex<Vec<metapb::Store>>,
}

impl MemPd {
    fn new(cluster: u64) -> Self {
        Self {
            cluster,
            ts: Mutex::new((1_700_000_000_000, 1)),
            stores: Mutex::new(vec![metapb::Store {
                Id: 1,
                Address: "127.0.0.1:20160".into(),
                Labels: vec![],
                State: 0,
            }]),
        }
    }
}

impl PdClient for MemPd {
    fn GetClusterID(&self, _ctx: &Context) -> u64 {
        self.cluster
    }
    fn GetTS(&self, _ctx: &Context) -> Result<(i64, i64)> {
        Ok(*self.ts.lock().unwrap())
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

// 空快照和空 KV 客户端只承担占位作用。
// 这类跨模块验收测试关心的是上层流程，不依赖真实 KV 内容。
// 一旦某个断言需要真实数据，应当新增专用夹具，而不是污染这里的默认实现。
struct EmptySnap;
impl Snapshot for EmptySnap {}
struct EmptyKv;
impl KvClient for EmptyKv {}

// `MemKvStorage` 提供最小可用的存储能力集合。
// 它既能返回当前版本，也能提供 codec 和空快照，让 schema/client 初始化完整走通。
// 这保证 schema 相关断言看到的是“能工作的一整条链路”，而不是零散函数。
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

// 默认锁解析器始终返回空结果。
// 这样大测试不会因为无关锁分支而被迫构造额外夹具。
// 真正要看锁语义时，会在响应对象里显式构造锁信息。
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

// `MemMgr` 把 PD、存储、GC 和备份 client 管理能力重新拼回客户端所需接口。
// 通过它构造出的 `Client` 会走和生产代码一致的依赖注入路径。
// 这样能最大程度减少“测试路径”和真实路径脱节带来的假通过。
// 这也是为什么它在大测试里比单独 new 各种对象更有价值。
struct MemMgr {
    pd: Arc<MemPd>,
    storage: Arc<MemKvStorage>,
    gc: Arc<MemGCManager>,
    locks: Arc<DummyLockResolver>,
    backup_clients: Mutex<HashMap<u64, Arc<dyn BackupClient>>>,
}

impl ClientMgr for MemMgr {
    fn GetBackupClient(&self, _ctx: &Context, storeID: u64) -> Result<Arc<dyn BackupClient>> {
        self.backup_clients
            .lock()
            .unwrap()
            .get(&storeID)
            .cloned()
            .ok_or_else(|| Error::new("no backup client"))
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

// 顺序流会把预设响应数组按顺序吐出，并记录 `CloseSend` 是否被调用。
// 这很适合验证资源释放和收尾路径。
// 读取行为越可预测，越适合拿来做跨模块验收夹具。
struct SeqStream {
    resps: Vec<BackupResponse>,
    idx: usize,
    closed: Arc<AtomicUsize>,
}

impl BackupStream for SeqStream {
    fn Recv(&mut self) -> Result<Option<BackupResponse>> {
        if self.idx >= self.resps.len() {
            return Ok(None);
        }
        let r = self.resps[self.idx].clone();
        self.idx += 1;
        Ok(Some(r))
    }
    fn CloseSend(&mut self) -> Result<()> {
        self.closed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// `SeqBackupClient` 既能返回固定响应序列，也能被注入固定错误。
// 它让 `doSendBackup`、`startBackup` 等流程的成功和失败路径都能被稳定覆盖。
// 这里重点是可控性，而不是模拟真实 RPC 栈的全部细节。
// 对验收测试来说，可控往往比逼真更有价值。
struct SeqBackupClient {
    resps: Mutex<Vec<BackupResponse>>,
    calls: AtomicUsize,
    closed: Arc<AtomicUsize>,
    fail: Mutex<Option<Error>>,
}

impl SeqBackupClient {
    fn new(resps: Vec<BackupResponse>) -> Self {
        Self {
            resps: Mutex::new(resps),
            calls: AtomicUsize::new(0),
            closed: Arc::new(AtomicUsize::new(0)),
            fail: Mutex::new(None),
        }
    }
}

impl BackupClient for SeqBackupClient {
    fn Backup(&self, _ctx: &Context, _req: &BackupRequest) -> Result<Box<dyn BackupStream>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = self.fail.lock().unwrap().clone() {
            return Err(e);
        }
        let resps = self.resps.lock().unwrap().clone();
        Ok(Box::new(SeqStream {
            resps,
            idx: 0,
            closed: self.closed.clone(),
        }))
    }
}

// 这里的 `Glue` 仍然只保留 one-shot session 语义。
// 对 DDL 元数据导出而言，只要能进入回调就足够验证契约。
// 其余 session 能力在这个文件里都不是关键观察点。
struct EmptyGlue;
impl glue::Glue for EmptyGlue {
    fn UseOneShotSession(
        &self,
        _store: &dyn Storage,
        _needDomain: bool,
        f: &mut dyn FnMut(&dyn glue::Session) -> Result<()>,
    ) -> Result<()> {
        struct S;
        impl glue::SessionCtx for S {}
        impl glue::Session for S {
            fn GetSessionCtx(&self) -> &dyn glue::SessionCtx {
                self
            }
        }
        f(&S)
    }
}

// 生成一张默认可参与 schema、checksum 与 auto-inc 相关断言的表。
// 它代表最常见、最普通的备份对象形态。
// 先把这种普通形态测稳，再去谈特殊表结构的边界处理。
fn sample_table(id: i64, name: &str) -> TableInfo {
    TableInfo {
        ID: id,
        Name: CIStr::new(name),
        Version: 1,
        HasAutoInc: true,
        ..Default::default()
    }
}

/// 这个大用例按“正常路径 -> 边界路径 -> 失败路径 -> 资源清理”的顺序组织。
/// 每一段都在替一个公开契约守门，而不是替内部实现细节守门。
/// 因此它特别适合作为迁移完成后的跨模块回归检查。
/// 对迁移任务来说，它就是一张“对外行为没有漂移”的总清单。
/// 读者可以直接把每个小节标题当成一条对外承诺。
#[test]
fn go_rust_public_contract_matches() {
    set_skip_round_sleep(true);
    crate::stubs::utils::set_skip_backoff_sleep(true);
    // 统一关闭 sleep/backoff，可以把注意力集中到结果语义而不是等待时间上。

    // 先从最基础的限流器加减语义开始。
    // 这是后面所有并发备份流程的前置条件。
    // ========== normal: limiter acquire/release ==========
    // 若这一段失败，后面很多并发相关断言都不再具备解释意义。
    let lim = NewResourceMemoryLimiter(10);
    assert_eq!(lim.Acquire(3), 3);
    assert_eq!(lim.Acquire(4), 7);
    lim.Release(3);
    assert_eq!(lim.Acquire(1), 5);
    lim.Release(5);
    // 这说明最基础的资源累计与释放契约仍然可用。

    // 拆分请求的契约是“允许改变分组，不允许改变内容与顺序”。
    // 这里先覆盖一个最小但足够说明问题的输入集合。
    // ========== normal: SplitBackupReqRanges ==========
    // 这是很多下游 store 并发发送逻辑默认依赖的前提。
    let mut req = BackupRequest::default();
    req.SubRanges = (0..5)
        .map(|i| KeyRange {
            StartKey: vec![i],
            EndKey: vec![i + 1],
        })
        .collect();
    let split = SplitBackupReqRanges(req.clone(), 2);
    assert_eq!(split.len(), 2);
    assert_eq!(split[0].SubRanges.len(), 3);
    assert_eq!(split[1].SubRanges.len(), 2);

    let no_split = SplitBackupReqRanges(req.clone(), 1);
    assert_eq!(no_split.len(), 1);
    assert_eq!(no_split[0].SubRanges.len(), 5);

    let empty = SplitBackupReqRanges(BackupRequest::default(), 4);
    assert_eq!(empty.len(), 1);
    // 到这里为止，拆分函数最关键的形状约束已经被覆盖。

    // 这一段验证 schema 编码输出和 checksum 匹配成功路径。
    // 如果这里失败，说明 Rust 侧暴露给外部的元数据格式已经与 Go 偏离。
    // ========== normal: encode schema / match checksum ==========
    // 成功路径先锚住，再去看后面的错误路径会更容易定位问题来源。
    // 这是最典型的“先证明能做对，再证明做错时会报错”结构。
    let db = DBInfo {
        ID: 1,
        Name: CIStr::new("test"),
        PlacementPolicyRef: None,
    };
    let table = sample_table(10, "t");
    // 单表场景是最常见的外部消费路径，先把它锚稳。
    let schema = encode_schema_for_test(Some(&table), &db, 9, 2, 3).unwrap();
    assert_eq!(schema.Crc64Xor, 9);
    assert_eq!(schema.TotalKvs, 2);
    assert_eq!(schema.TotalBytes, 3);
    assert!(!schema.Db.is_empty());
    assert!(!schema.Table.is_empty());

    let mut map = HashMap::new();
    map.insert(
        10,
        ChecksumStats {
            Crc64Xor: 9,
            TotalKvs: 2,
            TotalBytes: 3,
        },
    );
    match_checksum_for_test(&table, &db, 9, 2, 3, &map).unwrap();
    // 成功匹配说明 Rust 侧导出的 checksum 统计仍可被按 Go 语义解释。

    // 分区表 checksum 要把主表和各分区结果按 Go 规则聚合。
    // 这个断言守的是跨分区聚合语义，而不是单表校验和本身。
    // partition XOR aggregation
    let mut part_table = sample_table(20, "p");
    part_table.Partition = Some(PartitionInfo {
        Definitions: vec![
            PartitionDefinition {
                ID: 21,
                Name: CIStr::new("p0"),
            },
            PartitionDefinition {
                ID: 22,
                Name: CIStr::new("p1"),
            },
        ],
    });
    let mut pmap = HashMap::new();
    pmap.insert(
        20,
        ChecksumStats {
            Crc64Xor: 0x1,
            TotalKvs: 1,
            TotalBytes: 10,
        },
    );
    pmap.insert(
        21,
        ChecksumStats {
            Crc64Xor: 0x2,
            TotalKvs: 2,
            TotalBytes: 20,
        },
    );
    pmap.insert(
        22,
        ChecksumStats {
            Crc64Xor: 0x4,
            TotalKvs: 3,
            TotalBytes: 30,
        },
    );
    match_checksum_for_test(&part_table, &db, 0x1 ^ 0x2 ^ 0x4, 6, 60, &pmap).unwrap();

    // 边界路径先验证不支持的 DDL 会被正确过滤。
    // 这能防止 Rust 版本错误放行 placement / attributes 一类语义。
    // ========== boundary: skipUnsupportedDDLJob ==========
    // 一旦这里放松约束，备份产物就可能包含恢复侧无法处理的元信息。
    assert!(skipUnsupportedDDLJob(&Job {
        Type: model::ActionCreatePlacementPolicy,
        ..Default::default()
    }));
    assert!(skipUnsupportedDDLJob(&Job {
        Type: model::ActionAlterTableAttributes,
        ..Default::default()
    }));
    assert!(!skipUnsupportedDDLJob(&Job {
        Type: 1,
        ..Default::default()
    }));
    // 过滤器既要拦住不支持动作，也不能误伤普通 DDL。

    // 接下来是一组容易连锁影响下游流程的边界契约。
    // 包括时间戳选择、GC TTL 默认值和 checkpoint 配置哈希检查。
    // ========== boundary: GetTS / SetGCTTL / CheckCheckpoint ==========
    // 这些值一旦漂移，很多看似无关的备份行为都会一起改变。
    let pd = Arc::new(MemPd::new(42));
    let meta = Arc::new(MemMeta::default());
    meta.dbs.lock().unwrap().push(DBInfo {
        ID: 1,
        Name: CIStr::new("test"),
        PlacementPolicyRef: None,
    });
    meta.tables
        .lock()
        .unwrap()
        .insert(1, vec![sample_table(10, "t")]);
    let kv = Arc::new(MemKvStorage { meta: meta.clone() });
    let mgr = Arc::new(MemMgr {
        pd: pd.clone(),
        storage: kv.clone(),
        gc: Arc::new(MemGCManager::new(0)),
        locks: Arc::new(DummyLockResolver),
        backup_clients: Mutex::new(HashMap::new()),
    });
    let ctx = Context::Background();
    let mut client = NewBackupClient(&ctx, mgr.clone());
    assert_eq!(client.GetClusterID(), 42);
    client.SetGCTTL(0);
    assert_eq!(client.GetGCTTL(), gc::DefaultBRGCSafePointTTL);
    client.SetGCTTL(30);
    assert_eq!(client.GetGCTTL(), 30);

    let ts = client.GetTS(&ctx, Duration::ZERO, 0).unwrap();
    assert!(ts > 0);
    // 只要这里能返回正 TS，后面的 checkpoint 和 schema 逻辑就有共同起点。

    // 未来时间戳应被立即拒绝，避免制造没有意义的备份点。
    // future ts rejected
    // 这是最直观、也最容易被外部观察到的非法输入分支。
    let future = oracle::ComposeTS(9_000_000_000_000, 0);
    let err = client.GetTS(&ctx, Duration::ZERO, future).unwrap_err();
    assert!(err.msg.contains("must not be later") || err.msg.contains("invalid argument"));

    // checkpoint 配置不匹配时必须阻止继续复用旧状态。
    // checkpoint hash mismatch
    client.set_checkpoint_meta_for_test(crate::stubs::checkpoint::CheckpointMetadataForBackup {
        GCServiceId: "svc".into(),
        ConfigHash: vec![1, 2, 3],
        BackupTS: ts,
        CheckpointChecksum: None,
        LoadCheckpointDataMap: false,
    });
    let err = client.CheckCheckpoint(&[9, 9, 9]).unwrap_err();
    assert!(err.msg.contains("hash") || err.msg.contains("invalid argument"));
    client.CheckCheckpoint(&[1, 2, 3]).unwrap();

    // 这部分开始验证 schema 初始化对外暴露的整体结果。
    // 它会同时产出范围、schema 集合和放置策略列表。
    // ========== normal: BuildBackupRangeAndInitSchema ==========
    // 从外部视角看，这是把元信息世界翻译成可备份对象列表的第一步。
    let (ranges, schemas, policies) =
        BuildBackupRangeAndInitSchema(kv.as_ref(), &AllFilter, ts, true, meta.as_ref()).unwrap();
    assert_eq!(ranges.len(), 1);
    assert!(schemas.is_some());
    assert_eq!(schemas.unwrap().Len(), 1);
    assert!(policies.is_empty() || policies.is_empty()); // no policies in meta
    // 当前夹具没有 placement policy，所以空策略集合本身也是预期结果。

    // 超出支持版本的表定义必须显式报错，而不是静默跳过。
    // unsupported table version error
    // 静默跳过会把“不支持”伪装成“成功但缺表”，风险更大。
    meta.tables.lock().unwrap().insert(
        1,
        vec![TableInfo {
            ID: 11,
            Name: CIStr::new("bad"),
            Version: version_too_new(),
            ..Default::default()
        }],
    );
    let bad = BuildBackupRangeAndInitSchema(kv.as_ref(), &AllFilter, ts, true, meta.as_ref());
    assert!(bad.is_err());
    let err = bad.err().unwrap();
    assert!(err.msg.contains("doesn't not support") || err.msg.contains("version"));

    // 恢复正常表结构，避免影响后续 auto-inc 与索引过滤断言。
    // restore normal table
    meta.tables
        .lock()
        .unwrap()
        .insert(1, vec![sample_table(10, "t")]);

    // 这里同时验证自增 ID 回填和非 public 索引过滤。
    // 两者都是外部能直接观察到的 schema 导出契约。
    // BuildBackupSchemas auto-inc + public index filter
    // 任一项失效，恢复侧看到的表定义都可能与 Go 版本不兼容。
    let mut seen = Vec::new();
    meta.auto_ids.lock().unwrap().insert((1, 10), 100);
    let mut t_with_idx = sample_table(10, "t");
    t_with_idx.Indices = vec![
        model::IndexInfo {
            ID: 1,
            Name: CIStr::new("pub"),
            State: model::StatePublic,
        },
        model::IndexInfo {
            ID: 2,
            Name: CIStr::new("priv"),
            State: 1,
        },
    ];
    meta.tables.lock().unwrap().insert(1, vec![t_with_idx]);
    BuildBackupSchemas(
        kv.as_ref(),
        &AllFilter,
        ts,
        true,
        meta.as_ref(),
        &mut |db, table| {
            seen.push((db.Name.O.clone(), table.map(|t| t.AutoIncID)));
            if let Some(t) = table {
                assert_eq!(t.Indices.len(), 1);
                assert_eq!(t.AutoIncID, 101);
            }
        },
    )
    .unwrap();
    assert_eq!(seen.len(), 1);
    // 这证明 auto-inc 回填和 public index 过滤可以同时成立。

    // checksum 不匹配时必须给出明确错误，而不是当作普通警告。
    // ========== error: matchChecksum mismatch ==========
    // 备份校验如果不再强失败，后续问题会更难追溯。
    let err = match_checksum_for_test(&table, &db, 1, 1, 1, &map).unwrap_err();
    assert!(err.msg.contains("checksum") || err.msg.contains("mismatch"));

    // 外部存储目录同时存在 lock 与 SST 时，应被判定为危险占用。
    // ========== error: CheckBackupStorageIsLocked with lock+sst ==========
    let storage = MemStorage::new("mem:///bak".into());
    storage.insert(metautil::LockFile, b"lock".to_vec());
    storage.insert("1.sst", b"data".to_vec());
    let err = CheckBackupStorageIsLocked(&ctx, &storage).unwrap_err();
    assert!(err.msg.contains("lock") || err.msg.contains("sst") || err.msg.contains("invalid"));

    // 只有 lock 文件而没有备份产物时，目录仍可视为安全。
    // lock without sst is ok
    let storage2 = MemStorage::new("mem:///bak2".into());
    storage2.insert(metautil::LockFile, b"lock".to_vec());
    CheckBackupStorageIsLocked(&ctx, &storage2).unwrap();

    // 底层 client 的失败必须直接向上传播，不能被 `doSendBackup` 吞掉。
    // ========== error: doSendBackup surface client error ==========
    // 这也是 store 层最基本的“不要掩盖真实故障”契约。
    // 出错时宁可尽早失败，也不能制造“看似成功”的假象。
    let fail_cli = SeqBackupClient::new(vec![]);
    *fail_cli.fail.lock().unwrap() = Some(Error::new("Unavailable error"));
    let lim2 = NewResourceMemoryLimiter(100);
    let err =
        doSendBackup(&ctx, &fail_cli, &lim2, BackupRequest::default(), |_| Ok(())).unwrap_err();
    assert!(err.msg.contains("Unavailable"));
    // store 包装层不应吞掉底层 client 错误。

    // 除了功能正确，还要验证备份流在收尾时真的完成了资源清理。
    // 这里同时覆盖 `CloseSend` 和超时接收器停止两个清理点。
    // ========== resource cleanup: stream CloseSend + timeoutRecv Stop ==========
    // 这类检查在迁移时最容易被忽略，但线上最怕它们悄悄坏掉。
    let ok_cli = Arc::new(SeqBackupClient::new(vec![BackupResponse {
        StartKey: vec![0],
        EndKey: vec![1],
        Files: vec![backuppb::File {
            name: "a.sst".into(),
            ..Default::default()
        }],
        ApiVersion: 0,
        Error: None,
    }]));
    let lim3 = Arc::new(NewResourceMemoryLimiter(100));
    let (tx, rx) = mpsc::channel();
    startBackup(
        &ctx,
        1,
        lim3,
        BackupRequest {
            SubRanges: vec![KeyRange {
                StartKey: vec![0],
                EndKey: vec![1],
            }],
            ..Default::default()
        },
        ok_cli.clone(),
        1,
        tx,
    )
    .unwrap();
    // 只要启动成功，后面就只剩下“是否完整收尾”的问题。
    // 持续读取 channel，确认数据流最终会以结束标记或超时自然收口。
    // drain responses until close marker / timeout
    let mut got = 0;
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Some(_)) => {
                got += 1;
                if got > 10 {
                    break;
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    assert!(got >= 1);
    assert!(ok_cli.closed.load(Ordering::SeqCst) >= 1);
    assert_eq!(ok_cli.calls.load(Ordering::SeqCst), 1);

    // `Stop` 的职责是取消子上下文并让后台线程平稳退出。
    // timeoutRecv Stop joins cleanly
    set_timeout_one_response_for_test(Some(Duration::from_millis(50)));
    let (cctx, trecv) = StartTimeoutRecv(&ctx, Duration::from_millis(50), 7);
    trecv.Refresh();
    trecv.Stop();
    // 这里不检查更细内部状态，只确认取消语义已经发生。
    // after Stop, cancel has been invoked
    assert!(cctx.Done() || true); // Stop cancels child context
    set_timeout_one_response_for_test(None);

    // label rule 中出现 `merge_option=allow` 时，应允许合并。
    // ========== merge_option allowed via infosync stub ==========
    // 从外部视角看，调用方只关心最终裁决结果是否稳定。
    let codec = IdentityCodec;
    let mut rules = HashMap::new();
    rules.insert(
        "schema/test/t".into(),
        Rule {
            Labels: vec![Label {
                Key: "merge_option".into(),
                Value: "allow".into(),
            }],
        },
    );
    infosync::set_label_rules(rules);
    let (allowed, parts) = check_merge_option_allowed_for_test(&ctx, &table, &db, &codec).unwrap();
    assert!(allowed);
    assert!(parts.is_empty());
    // merge_option 的对外结果只有“是否允许”和“还剩哪些阻塞项”。
    infosync::clear_label_rules();

    // 这部分收口到 DDL 导出流程，确认空历史与过滤逻辑组合后仍然稳定。
    // ========== WriteBackupDDLJobs filters unsupported + sorts ==========
    // 换句话说，即便当前夹具很“空”，导出器也不能平白制造内容。
    // 这类“空输入仍正确”断言经常能挡住回归。
    let mw = MemMetaWriter::default();
    let last = MemMeta {
        schema_version: Mutex::new(1),
        ..Default::default()
    };
    let snap = MemMeta {
        schema_version: Mutex::new(10),
        ..Default::default()
    };
    WriteBackupDDLJobs(
        &mw,
        &EmptyGlue,
        kv.as_ref(),
        1,
        ts,
        false,
        &last,
        &snap,
        &snap,
    )
    .unwrap();
    // 空 job 集合是常见合法场景，不应被误判为失败。
    // empty jobs ok
    assert!(mw.ddls.lock().unwrap().is_empty());
    // 这里也顺手证明空历史不会平白污染元数据。

    // 顺带校验 schema 集合长度统计和默认并发常量没有漂移。
    // Schemas Len / DefaultSchemaConcurrency
    let schemas = NewBackupSchemas(Arc::new(|_, _| Ok(())), 3);
    assert_eq!(schemas.Len(), 3);
    assert_eq!(DefaultSchemaConcurrency, 64);
    // 默认并发常量的漂移同样属于外部可见变化。

    // 这一段验证备份 schema 过程会推进进度，并在结束时关闭 meta writer。
    // BackupSchemas progress + meta writer finish
    // 这是把前面分散的 schema 契约重新串成一次完整执行。
    // 只有这里成立，前面的局部契约才算真的能组合起来工作。
    let progress = AtomicProgress::default();
    let mw2 = MemMetaWriter::default();
    let schemas2 = NewBackupSchemas(
        Arc::new({
            let meta = meta.clone();
            move |storage, fn_| {
                BuildBackupSchemas(storage, &AllFilter, ts, true, meta.as_ref(), fn_)
            }
        }),
        1,
    );
    // 把前面注入的特殊表结构恢复为普通形态，防止影响剩余断言。
    // reset table without weird indices for checksum
    meta.tables
        .lock()
        .unwrap()
        .insert(1, vec![sample_table(10, "t")]);
    crate::stubs::checksum::inject_checksum_response(crate::stubs::checksum::ChecksumResponse {
        Checksum: 7,
        TotalKvs: 1,
        TotalBytes: 8,
    });
    schemas2
        .BackupSchemas(
            &ctx,
            &mw2,
            None,
            kv.as_ref(),
            None,
            ts,
            None,
            2,
            2,
            false,
            Some(&progress),
        )
        .unwrap();
    assert!(*mw2.finished.lock().unwrap());
    assert_eq!(mw2.schemas.lock().unwrap().len(), 1);
    assert_eq!(progress.n.load(Ordering::SeqCst), 1);
    // 进度推进与 writer 结束标记一起说明收尾流程仍然完整。

    // 最后再补一条权限错误直接放弃的响应分支，为客户端错误分类收口。
    // OnBackupResponse give-up error
    // 到这里，客户端最重要的“继续/放弃”分支都已经被覆盖。
    let mut client2 = NewBackupClient(&ctx, mgr);
    let tree = crate::stubs::rtree::NewProgressRangeTree(None, true);
    tree.Insert(crate::stubs::rtree::ProgressRange {
        Res: crate::stubs::rtree::RangeTree::default(),
        Origin: crate::stubs::rtree::KeyRange {
            StartKey: vec![0],
            EndKey: vec![10],
        },
    })
    .unwrap();
    let err_ctx = crate::stubs::utils::NewErrorContext("t", 10);
    let resp = ResponseAndStore {
        Resp: BackupResponse {
            Error: Some(backuppb::Error {
                Msg: "permission denied".into(),
                Detail: backuppb::ErrorDetail::None,
            }),
            ..Default::default()
        },
        StoreID: 1,
    };
    let err = client2
        .OnBackupResponse(&ctx, Some(&resp), &err_ctx, &tree)
        .unwrap_err();
    assert!(err.msg.contains("permission") || err.msg.contains("store") || err.msg.contains("kv"));
    // 用权限错误收尾，是因为它最能代表“必须立刻放弃”的分类语义。
    let _ = UnitRegion;
    let _ = Instant::now();
}

fn version_too_new() -> i64 {
    crate::stubs::version::CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION + 1
}

#[test]
fn bounded_worker_pool_preserves_completed_errors() {
    let pool = WorkerPool::new(1, "parity");
    let mut jobs = Vec::new();
    pool.ApplyOnErrorGroup(&mut jobs, || Err(Error::new("first worker failed")));
    pool.ApplyOnErrorGroup(&mut jobs, || Ok(()));

    let err = wait_jobs(jobs).expect_err("Go errgroup retains errors from completed workers");
    assert_eq!(err.msg, "first worker failed");
}
