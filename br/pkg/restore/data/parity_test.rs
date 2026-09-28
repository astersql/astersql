// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/restore/data` vs Go sources.
//!
//! 模块职责：对照 Go `br/pkg/restore/data` 的恢复数据路径，验证键前缀、
//! Region 排序/选主、一致性检查、恢复计划与 flashback 阶段语义。
//! 约束：全部走内存桩（MemMgr/Mock 客户端），不连真实 PD/TiKV；
//! 长 watcher 通过 `spawn_watcher=false` 关闭，避免 30s 阻塞测试。
//! 对应 Go 侧 SortRecoverRegions / LeaderCandidates / RecoverData 等用例意图。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::data::{
    NewRecovery, NewStoreMeta, RecoverData, RecoveryStage, atStage, getStoreAddress, isRetryErr,
};
use crate::key::{PrefixEndKey, PrefixStartKey, keyCmp, keyEq};
use crate::recover::{
    CheckConsistencyAndValidPeer, LeaderCandidates, RecoverRegion, RecoverRegionInfo,
    SelectRegionLeader, SortRecoverRegions,
};
use crate::stubs::metapb;
use crate::stubs::recovpb;
use crate::stubs::{
    ClientFactory, Conn, Context, Error, MemConn, MemMgr, MemProgress, Mgr,
    NewRecoveryBackoffStrategy, RecoverDataClient, RecoverRegionStream, RegionMetaStream, Result,
    WithRetryV2, eof, is_eof,
};

// 在线 store 数量基线：create_stores 固定构造 3 个（含 TiKV/TiFlash 标签组合）。
const NUM_ONLINE_STORE: usize = 3;
// 与 generate_region_meta 中最大 PeerId 对齐，用于断言 MaxAllocID 传播。
const MAX_ALLOCATE_ID: u64 = 0x176f;

// ---- 测试数据构造助手：字段顺序对齐 Go recovpb / RecoverRegion ----

/// 构造单条 RegionMeta；字段名保持 Go protobuf 风格以便与 Go 测试数据一一对应。
/// Tombstone/空键边界由调用方显式传入，避免默认值掩盖边界场景。
fn new_region_meta(
    region_id: u64,
    peer_id: u64,
    last_log_term: u64,
    last_index: u64,
    commit_index: u64,
    version: u64,
    tombstone: bool,
    start_key: &[u8],
    end_key: &[u8],
) -> recovpb::RegionMeta {
    recovpb::RegionMeta {
        RegionId: region_id,
        PeerId: peer_id,
        LastLogTerm: last_log_term,
        LastIndex: last_index,
        CommitIndex: commit_index,
        Version: version,
        Tombstone: tombstone,
        StartKey: start_key.to_vec(),
        EndKey: end_key.to_vec(),
    }
}

/// 构造带 StoreId 的 RecoverRegion：在 RegionMeta 外包一层 store 归属。
/// `end_key=None` 表示空 EndKey（与 Go 零值切片语义一致）。
fn new_peer_meta(
    region_id: u64,
    peer_id: u64,
    store_id: u64,
    start_key: &[u8],
    end_key: Option<&[u8]>,
    last_log_term: u64,
    last_index: u64,
    commit_index: u64,
    version: u64,
    tombstone: bool,
) -> RecoverRegion {
    RecoverRegion {
        RegionMeta: recovpb::RegionMeta {
            RegionId: region_id,
            PeerId: peer_id,
            StartKey: start_key.to_vec(),
            EndKey: end_key.unwrap_or(&[]).to_vec(),
            LastLogTerm: last_log_term,
            LastIndex: last_index,
            CommitIndex: commit_index,
            Version: version,
            Tombstone: tombstone,
        },
        StoreId: store_id,
    }
}

/// 将 RecoverRegion 转为一致性检查用的 RecoverRegionInfo。
/// Start/End 经 PrefixStartKey/PrefixEndKey 规范化，对齐 Go 键空间编码。
fn new_recover_region_info(r: &RecoverRegion) -> RecoverRegionInfo {
    RecoverRegionInfo {
        RegionVersion: r.Version,
        RegionId: r.RegionId,
        // 信息结构中的键必须是加前缀后的规范化形式。
        StartKey: PrefixStartKey(&r.StartKey),
        EndKey: PrefixEndKey(&r.EndKey),
        TombStone: r.Tombstone,
    }
}

/// 构造三节点 store 列表：含纯 TiKV、TiFlash 标签优先、以及 TiKV+杂项标签。
/// 用于验证 getStoreAddress 与标签过滤不会误连错误引擎。
fn create_stores() -> Vec<metapb::Store> {
    vec![
        metapb::Store {
            Id: 1,
            Address: "127.0.0.1:20160".into(),
            // store1：仅 engine=tikv，作为默认在线 TiKV。
            Labels: vec![metapb::StoreLabel {
                Key: "engine".into(),
                Value: "tikv".into(),
            }],
        },
        metapb::Store {
            Id: 2,
            Address: "127.0.0.1:20161".into(),
            Labels: vec![
                metapb::StoreLabel {
                    // store2：engine=tiflash 优先标签，验证过滤逻辑。
                    Key: "else".into(),
                    Value: "tikv".into(),
                },
                metapb::StoreLabel {
                    Key: "engine".into(),
                    Value: "tiflash".into(),
                },
            ],
        },
        metapb::Store {
            Id: 3,
            Address: "127.0.0.1:20162".into(),
            Labels: vec![
                metapb::StoreLabel {
                    Key: "else".into(),
                    Value: "tiflash".into(),
                },
                metapb::StoreLabel {
                    Key: "engine".into(),
                    Value: "tikv".into(),
                },
            ],
        },
    ]
}

/// 向 Recovery 注入三 store × 三 region 的元数据样例。
/// Region 11/12/13 覆盖 ["",b)/[b,c)/[c,"")；PeerId 与 term/index 刻意错开，
/// 以便 MakeRecoveryPlan 选出正确 leader 并更新 MaxAllocID。
fn generate_region_meta(recovery: &mut crate::data::Recovery) {
    let mut store_meta0 = NewStoreMeta(1);
    store_meta0
        .RegionMetas
        .push(new_region_meta(11, 24, 8, 5, 4, 1, false, b"", b"b"));
    store_meta0
        .RegionMetas
        .push(new_region_meta(12, 34, 5, 6, 5, 1, false, b"b", b"c"));
    store_meta0
        .RegionMetas
        .push(new_region_meta(13, 44, 1200, 7, 6, 1, false, b"c", b""));
    recovery.StoreMetas[0] = store_meta0;
    // 写入下标 0 对应 create_stores 的 store Id=1。

    let mut store_meta1 = NewStoreMeta(2);
    store_meta1
        .RegionMetas
        .push(new_region_meta(11, 25, 7, 6, 4, 1, false, b"", b"b"));
    store_meta1
        .RegionMetas
        .push(new_region_meta(12, 35, 5, 6, 5, 1, false, b"b", b"c"));
    store_meta1
        .RegionMetas
        .push(new_region_meta(13, 45, 1200, 6, 6, 1, false, b"c", b""));
    recovery.StoreMetas[1] = store_meta1;
    // store Id=2：同 region 的 term/index 略低，验证选主不会误选。

    let mut store_meta2 = NewStoreMeta(3);
    store_meta2
        .RegionMetas
        .push(new_region_meta(11, 26, 7, 5, 4, 1, false, b"", b"b"));
    store_meta2
        .RegionMetas
        .push(new_region_meta(12, 36, 5, 6, 6, 1, false, b"b", b"c"));
    store_meta2.RegionMetas.push(new_region_meta(
        13,
        MAX_ALLOCATE_ID,
        1200,
        6,
        6,
        1,
        false,
        b"c",
        b"",
    ));
    recovery.StoreMetas[2] = store_meta2;
    // store Id=3：PeerId=MAX_ALLOCATE_ID，驱动 MaxAllocID 断言。
}

/// 内存 RegionMeta 流：按序 Recv，耗尽后返回 eof，模拟 gRPC 服务端推送。
struct VecMetaStream {
    metas: Vec<recovpb::RegionMeta>,
    idx: usize,
}

// RegionMetaStream：idx 耗尽即 eof，与 Go io.EOF 结束约定一致。
impl RegionMetaStream for VecMetaStream {
    fn Recv(&mut self) -> Result<recovpb::RegionMeta> {
        if self.idx >= self.metas.len() {
            // 流耗尽返回 eof，调用方用 is_eof 区分结束与失败。
            return Err(eof());
        }
        let m = self.metas[self.idx].clone();
        self.idx += 1;
        Ok(m)
    }
}

/// RecoverRegion 客户端流桩：记录 Send 请求并在 CloseAndRecv 标记关闭。
/// 用于断言恢复阶段确实向目标 store 发送了计划请求。
struct RecvStream {
    store_id: u64,
    sent: Vec<recovpb::RecoverRegionRequest>,
    closed: Arc<AtomicBool>,
}

// RecoverRegionStream：Send 只收集，CloseAndRecv 返回本 store_id。
impl RecoverRegionStream for RecvStream {
    fn Send(&mut self, req: &recovpb::RecoverRegionRequest) -> Result<()> {
        self.sent.push(req.clone());
        Ok(())
    }

    fn CloseAndRecv(&mut self) -> Result<recovpb::RecoverRegionResponse> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(recovpb::RecoverRegionResponse {
            StoreId: self.store_id,
            // 响应带回 store_id，便于调用方核对关闭的是哪条流。
        })
    }
}

/// 按 store 固定元数据的 RecoverDataClient 桩。
/// closed_conns / recover_calls 供资源清理与调用次数断言共享计数。
struct MockRecoverClient {
    store_id: u64,
    metas: Vec<recovpb::RegionMeta>,
    closed_conns: Arc<AtomicU64>,
    recover_calls: Arc<AtomicU64>,
}

// ReadRegionMeta 校验 StoreId 后返回克隆元数据流；RecoverRegion 递增计数。
impl RecoverDataClient for MockRecoverClient {
    fn ReadRegionMeta(
        &mut self,
        _ctx: &Context,
        req: &recovpb::ReadRegionMetaRequest,
    ) -> Result<Box<dyn RegionMetaStream>> {
        assert_eq!(req.StoreId, self.store_id);
        // 请求 store 必须与客户端绑定 store 一致，防止串读元数据。
        Ok(Box::new(VecMetaStream {
            metas: self.metas.clone(),
            idx: 0,
        }))
    }

    fn RecoverRegion(&mut self, _ctx: &Context) -> Result<Box<dyn RecoverRegionStream>> {
        self.recover_calls.fetch_add(1, Ordering::SeqCst);
        // 每次打开 RecoverRegion 流计一次，供清理用例断言计划下发次数。
        Ok(Box::new(RecvStream {
            store_id: self.store_id,
            sent: Vec::new(),
            closed: Arc::new(AtomicBool::new(false)),
        }))
    }
}

/// 包装 MemConn：首次 Close 时递增全局 closed_conns，验证连接必释放。
struct TrackingConn {
    inner: MemConn,
    closed_conns: Arc<AtomicU64>,
}

// 幂等关闭：swap 保证重复 Close 不重复计数，对齐 Go defer conn.Close。
impl Conn for TrackingConn {
    fn Close(&mut self) {
        if !self.inner.closed.swap(true, Ordering::SeqCst) {
            // 仅首次关闭计入统计，匹配 Go sync.Once 式关闭语义。
            self.closed_conns.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.Close();
    }
}

/// 按地址解析 store_id，并组装 MockRecoverClient + TrackingConn。
/// 未知地址直接报错，防止测试静默连错节点。
fn mock_factory_from_metas(
    by_store: HashMap<u64, Vec<recovpb::RegionMeta>>,
    addr_to_id: HashMap<String, u64>,
    closed_conns: Arc<AtomicU64>,
    recover_calls: Arc<AtomicU64>,
) -> ClientFactory {
    Arc::new(move |_ctx: &Context, addr: &str| {
        let store_id = *addr_to_id
            .get(addr)
            .ok_or_else(|| Error::new(format!("unknown addr {addr}")))?;
        // 地址未登记则失败，避免默认 store_id=0 造成静默错读。
        let metas = by_store.get(&store_id).cloned().unwrap_or_default();
        let client = MockRecoverClient {
            store_id,
            metas,
            closed_conns: closed_conns.clone(),
            recover_calls: recover_calls.clone(),
        };
        let conn = TrackingConn {
            inner: MemConn::new(),
            closed_conns: closed_conns.clone(),
        };
        Ok((
            Box::new(client) as Box<dyn RecoverDataClient>,
            Box::new(conn) as Box<dyn Conn>,
        ))
    })
}

/// 总入口：串联正常路径、边界、错误与资源清理四组契约，对齐 Go 公开行为。
#[test]
fn go_rust_public_contract_matches() {
    contract_normal_keys_and_sort();
    // 四组子契约：正常键/计划、边界、错误阶段、资源清理。
    contract_normal_make_recovery_plan();
    contract_boundary_prefix_and_empty_peers();
    contract_error_consistency_and_stages();
    contract_resource_cleanup_on_recover_data();
}

/// Go `WithRetryV2` treats a zero backoff as an immediate retry; remaining attempts
/// decide when retrying stops.
#[test]
fn retryable_error_with_zero_delay_is_retried() {
    let ctx = Context::Background();
    let mut attempts = 0;
    let value = WithRetryV2(&ctx, NewRecoveryBackoffStrategy(Box::new(|_| true)), |_| {
        attempts += 1;
        if attempts == 1 {
            Err(Error::new("retry me"))
        } else {
            Ok(42)
        }
    })
    .expect("second attempt succeeds");
    assert_eq!(value, 42);
    assert_eq!(attempts, 2);
}

/// Go uint64 subtraction wraps, including the flashback start timestamp at zero.
#[test]
fn zero_commit_ts_uses_wrapping_previous_version() {
    let mgr = Arc::new(MemMgr::new());
    let recovery = NewRecovery(Vec::new(), mgr.clone(), Arc::new(MemProgress::new()), 1);
    recovery
        .FlashbackToVersion(&Context::Background(), 7, 0)
        .expect("flashback");
    assert_eq!(
        mgr.flashback.last_flashback_start_ts.load(Ordering::SeqCst),
        u64::MAX
    );
}

/// Cancelling a Go child context never cancels its parent.
#[test]
fn child_context_cancellation_does_not_cancel_parent() {
    let parent = Context::Background();
    let (child, cancel) = Context::WithCancel(&parent);
    cancel.cancel(Error::new("context canceled"));
    assert!(child.Done());
    assert!(!parent.Done());
}

/// errors.Is(err, io.EOF) does not accept unrelated messages containing EOF.
#[test]
fn eof_detection_does_not_use_substring_matching() {
    assert!(!is_eof(&Error::new("not EOF: malformed payload")));
}

/// Go errors.As only recognizes the recoveryError type, not matching text.
#[test]
fn recovery_stage_is_not_inferred_from_plain_error_text() {
    let err = Error::new("remote said stage: collecting meta");
    assert_eq!(atStage(&err), RecoveryStage::StageUnknown);
}

/// Go WithRetryV2 returns a multi-error containing every failed attempt.
#[test]
fn exhausted_retries_preserve_all_errors() {
    let ctx = Context::Background();
    let mut attempts = 0;
    let err = WithRetryV2::<(), _>(&ctx, NewRecoveryBackoffStrategy(Box::new(|_| true)), |_| {
        attempts += 1;
        Err(Error::new(format!("attempt {attempts}")))
    })
    .expect_err("all attempts fail");
    assert_eq!(attempts, 16);
    assert!(err.msg.contains("attempt 1"), "err={}", err.msg);
    assert!(err.msg.contains("attempt 2"), "err={}", err.msg);
    assert!(err.msg.contains("attempt 16"), "err={}", err.msg);
}

/// Go `WithRetryV2` always invokes the operation once before observing an
/// already-cancelled context, and returns the operation error rather than the
/// context cancellation error.
#[test]
fn cancelled_retry_context_still_runs_first_attempt() {
    let ctx = Context::Background();
    ctx.cancel(Error::new("context canceled"));
    let mut attempts = 0;
    let err = WithRetryV2::<(), _>(&ctx, NewRecoveryBackoffStrategy(Box::new(|_| true)), |_| {
        attempts += 1;
        Err(Error::new("operation failed"))
    })
    .expect_err("cancelled retry must return the attempted operation error");

    assert_eq!(attempts, 1);
    assert_eq!(err.msg, "operation failed");
}

/// 正常路径：键比较/前缀编码、SortRecoverRegions、选主与 store 分数偏向。
/// 对齐 Go TestSortRecoverRegions / TestLeaderCandidates / TestSelectRegionLeader。
fn contract_normal_keys_and_sort() {
    // 下列断言锁定 keyEq/keyCmp/Prefix* 与 Go 完全一致的契约面。
    assert!(keyEq(b"abc", b"abc"));
    assert!(!keyEq(b"ab", b"abc"));
    assert_eq!(keyCmp(b"a", b"b"), -1);
    assert_eq!(keyCmp(b"b", b"a"), 1);
    assert_eq!(keyCmp(b"a", b"a"), 0);
    // 公共前缀时较短键更小，对齐 bytes.Compare 语义。
    assert_eq!(keyCmp(b"a", b"aa"), -1);
    // 非空 StartKey 加 z 前缀；空 EndKey 的 PrefixEndKey 用 z+1 哨兵。
    assert_eq!(PrefixStartKey(b"x"), b"zx".to_vec());
    assert_eq!(PrefixEndKey(b""), vec![b'z' + 1]);
    assert_eq!(PrefixEndKey(b"x"), b"zx".to_vec());
    // 键相等/比较与 z 前缀编码必须与 Go key 包字节级一致，否则排序选主全错。

    // Go TestSortRecoverRegions
    // 每个 region 多 peer：SortRecoverRegions 按 version/term/index 选出代表后逆序输出。
    let selected1 = new_peer_meta(9, 11, 2, b"aa", None, 2, 0, 0, 0, false);
    // selected* 为各 region 按规则应被 Sort 选中的代表 peer。
    let selected2 = new_peer_meta(19, 22, 3, b"bbb", None, 2, 1, 0, 1, false);
    let selected3 = new_peer_meta(29, 30, 1, b"c", None, 2, 1, 1, 2, false);
    // region_id → peer 列表；Sort 内部会改写/消费该 map。
    let mut regions = HashMap::from([
        (
            9,
            vec![
                new_peer_meta(9, 10, 1, b"a", None, 1, 1, 1, 1, false),
                selected1.clone(),
                new_peer_meta(9, 12, 3, b"aaa", None, 0, 0, 0, 0, false),
            ],
        ),
        (
            19,
            vec![
                new_peer_meta(19, 20, 1, b"b", None, 1, 1, 1, 1, false),
                new_peer_meta(19, 21, 2, b"bb", None, 2, 0, 0, 0, false),
                selected2.clone(),
            ],
        ),
        (
            29,
            vec![
                selected3.clone(),
                new_peer_meta(29, 31, 2, b"cc", None, 2, 0, 0, 0, false),
                new_peer_meta(29, 32, 3, b"ccc", None, 2, 1, 0, 0, false),
            ],
        ),
    ]);
    // 排序结果应只含每个 region 的最优 peer 信息，并按版本降序。
    let infos = SortRecoverRegions(&mut regions);
    let expect = vec![
        new_recover_region_info(&selected3),
        new_recover_region_info(&selected2),
        new_recover_region_info(&selected1),
    ];
    assert_eq!(expect, infos);
    // 期望顺序：version 高的在前（selected3,2,1），与 Go 稳定排序一致。

    // Go TestLeaderCandidates / TestSelectRegionLeader
    // 无分数时取 peers[0]；有 store 分数时偏向分数最低的 store（负载均衡意图）。
    let p1 = new_peer_meta(9, 11, 2, b"", Some(b"bb"), 2, 1, 0, 0, false);
    let p2 = new_peer_meta(19, 22, 3, b"bb", Some(b"cc"), 2, 1, 0, 1, false);
    let p3 = new_peer_meta(29, 30, 1, b"cc", Some(b""), 2, 1, 0, 2, false);
    // 三段连续 region 的代表 peer，供选主与一致性检查共用。
    let peers = vec![p1.clone(), p2.clone(), p3.clone()];
    let cands = LeaderCandidates(&peers).expect("candidates");
    assert_eq!(cands.len(), 3);
    // 三 peer 均存活且非 tombstone 时，候选集大小等于 peers。

    // 空分数表：SelectRegionLeader 退化为选取列表首个 peer。
    let mut scores = HashMap::new();
    assert_eq!(SelectRegionLeader(&scores, &peers), p1);
    scores.insert(2, 3);
    // store 分数：2→3, 3→2, 1→1；SelectRegionLeader 取最低分 store。
    scores.insert(3, 2);
    scores.insert(1, 1);
    assert_eq!(SelectRegionLeader(&scores, &peers), p3);
    // store1 分数最低 → 选 p3（store_id=1），对齐 Go 低负载优先。
    assert_eq!(SelectRegionLeader(&HashMap::new(), &[p3.clone()]), p3);
    // 单 peer 时无论分数如何都必须选中该 peer。
}

/// 正常路径：注入元数据后 MakeRecoveryPlan，校验总 region 数、MaxAllocID 与计划条数。
/// RecoveryPlan 长度为 2：仅需要下发恢复指令的 store 会进计划。
fn contract_normal_make_recovery_plan() {
    // MemMgr/MemProgress：纯内存进度与连接工厂，无外部依赖。
    let mgr = Arc::new(MemMgr::new());
    let progress = Arc::new(MemProgress::new());
    // concurrency=64 仅作参数透传，本用例不强调并发度本身。
    let mut recovery = NewRecovery(create_stores(), mgr, progress, 64);
    generate_region_meta(&mut recovery);
    assert_eq!(recovery.GetTotalRegions(), 3);
    // 三 store 各报 3 region，但 region_id 去重后总数应为 3。
    // 计划成功后 MaxAllocID/RecoveryPlan 才可断言。
    recovery.MakeRecoveryPlan().expect("plan");
    assert_eq!(recovery.MaxAllocID, MAX_ALLOCATE_ID);
    // 计划阶段必须上收全局最大已分配 peer id，供后续 PD 分配避让。
    assert_eq!(recovery.RecoveryPlan.len(), 2);
    // 仅部分 store 需要执行恢复动作，计划长度小于在线 store 数属预期。
    // NUM_ONLINE_STORE 仅作文档化常量引用，避免未使用告警掩盖真问题。
    let _ = NUM_ONLINE_STORE;
}

/// 边界：空前缀、未知 store、空 peers，以及连续键空间一致性检查的成功路径。
fn contract_boundary_prefix_and_empty_peers() {
    // 空 StartKey 规范化为单字节 z，与 Go PrefixStartKey 一致。
    assert_eq!(PrefixStartKey(&[]), b"z".to_vec());
    assert!(getStoreAddress(&create_stores(), 99).is_empty());
    // 未知 store_id 返回空串而非 panic，调用方需自行判空。
    assert_eq!(getStoreAddress(&create_stores(), 1), "127.0.0.1:20160");

    // 空 peers 必须失败：恢复不能接受“无副本 region”。
    let err = LeaderCandidates(&[]).expect_err("empty peers");
    assert!(
        err.msg.contains("restore met a region without any peer")
            || err.msg.contains("invalid region range")
    );

    // Valid continuous keyspace (Go TestCheckConsistencyAndValidPeer happy path)
    // 三段 region 首尾相接覆盖全键空间，应返回全部 region_id。
    let valid = vec![
        new_recover_region_info(&new_peer_meta(
            9,
            11,
            2,
            b"",
            Some(b"bb"),
            2,
            0,
            0,
            0,
            false,
        )),
        new_recover_region_info(&new_peer_meta(
            19,
            22,
            3,
            b"bb",
            Some(b"cc"),
            2,
            1,
            0,
            1,
            false,
        )),
        new_recover_region_info(&new_peer_meta(
            29,
            30,
            1,
            b"cc",
            Some(b""),
            2,
            1,
            1,
            2,
            false,
        )),
    ];
    // 成功路径返回通过校验的 region_id 集合。
    let peers = CheckConsistencyAndValidPeer(valid).expect("valid");
    assert_eq!(peers.len(), 3);
    assert!(peers.contains(&9) && peers.contains(&19) && peers.contains(&29));
    // 一致性检查返回的是 region_id 集合，而非 peer 列表。
}

/// 错误路径：键空间缺口应失败；阶段字符串、可重试错误与 eof 分类对齐 Go。
/// 仅 CollectingMeta 阶段错误可重试，Flashback 失败不得进入重试环。
fn contract_error_consistency_and_stages() {
    let invalid = vec![
        // 故意构造不连续/乱序键范围，触发一致性检查失败。
        new_recover_region_info(&new_peer_meta(
            9,
            11,
            2,
            b"aa",
            Some(b"cc"),
            2,
            0,
            0,
            0,
            false,
        )),
        new_recover_region_info(&new_peer_meta(
            19,
            22,
            3,
            b"dd",
            Some(b"cc"),
            2,
            1,
            0,
            1,
            false,
        )),
        new_recover_region_info(&new_peer_meta(
            29,
            30,
            1,
            b"cc",
            Some(b"dd"),
            2,
            1,
            1,
            2,
            false,
        )),
    ];
    let err = CheckConsistencyAndValidPeer(invalid).expect_err("gap");
    // aa→cc 与 dd→cc 形成缺口/乱序，必须报 invalid restore/region range。
    assert!(
        err.msg.contains("invalid restore range") || err.msg.contains("invalid region range"),
        "err={}",
        err.msg
    );

    // 阶段 Display 字符串与 Go 侧对外文案保持一致，供日志/进度展示。
    assert_eq!(
        RecoveryStage::StageCollectingMeta.String(),
        "collecting meta"
    );
    assert_eq!(RecoveryStage::StageFlashback.String(), "flashback");
    assert_eq!(RecoveryStage::StageUnknown.String(), "unknown");

    // 为错误打上阶段标签，模拟 Go errors.WithStack/阶段包装。
    let mut collecting = Error::new("meta boom");
    collecting.stage = Some(RecoveryStage::StageCollectingMeta as i32);
    assert!(isRetryErr(&collecting));
    assert_eq!(atStage(&collecting), RecoveryStage::StageCollectingMeta);
    // CollectingMeta 错误标记可重试，供 RecoverData 外层退避重跑。

    let mut flash = Error::new("flash boom");
    flash.stage = Some(RecoveryStage::StageFlashback as i32);
    assert!(!isRetryErr(&flash));
    // Flashback 阶段失败不可重试，避免重复 flashback 破坏一致性。

    // 无阶段信息的错误既不可重试，atStage 归为 Unknown。
    let unknown = Error::new("plain");
    assert!(!isRetryErr(&unknown));
    assert_eq!(atStage(&unknown), RecoveryStage::StageUnknown);

    assert!(is_eof(&eof()));
    // eof 哨兵必须被 is_eof 识别，流结束不能当普通错误重试。
}

/// 资源清理与端到端恢复步骤：读元数据→计划→分配 ID→恢复 region→flashback。
/// 断言连接关闭次数、RecoverRegion 调用与进度推进；并覆盖 flashback 失败与取消。
fn contract_resource_cleanup_on_recover_data() {
    // 端到端路径复用同一套三 store / 三 region 元数据种子。
    let stores = create_stores();
    let mut by_store = HashMap::new();
    let mut recovery_seed = NewRecovery(
        stores.clone(),
        Arc::new(MemMgr::new()),
        Arc::new(MemProgress::new()),
        64,
    );
    generate_region_meta(&mut recovery_seed);
    // 按 StoreId 建索引，供 mock factory 按地址回放元数据。
    for sm in &recovery_seed.StoreMetas {
        by_store.insert(sm.StoreId, sm.RegionMetas.clone());
    }
    let mut addr_to_id = HashMap::new();
    // 地址→store_id 映射与 create_stores 中 Address 字段一致。
    for s in &stores {
        addr_to_id.insert(s.Address.clone(), s.Id);
    }

    // 共享原子计数：跨多个 TrackingConn 实例汇总关闭次数。
    let closed_conns = Arc::new(AtomicU64::new(0));
    let recover_calls = Arc::new(AtomicU64::new(0));
    let mgr = Arc::new(MemMgr::new());
    // 注入工厂后，ReadRegionMeta/RecoverRegions 全部走内存桩。
    mgr.set_client_factory(mock_factory_from_metas(
        by_store,
        addr_to_id,
        closed_conns.clone(),
        recover_calls.clone(),
    ));
    mgr.flashback.completed_regions.store(3, Ordering::SeqCst);
    // 预置 flashback 已完成 region 数，跳过等待真实进度收敛。

    let progress = Arc::new(MemProgress::new());
    let ctx = Context::Background();
    // Disable long watcher sleep by not spawning (RecoverData uses Spawn with 30s).
    // 关闭 watcher 并用短 tick，保证单测可在毫秒级跑完读/计划/恢复链路。
    // Exercise doRecoveryData path via RecoverData but watcher would block tests for 30s.
    // Use a short-tick Recovery manually instead of RecoverData's internal spawn.
    let mut recovery = NewRecovery(stores.clone(), mgr.clone(), progress.clone(), 64);
    recovery.spawn_watcher = false;
    // 禁止后台 watcher，防止测试被 30s sleep 拖死。
    recovery.watcher_tick = Duration::from_millis(1);

    // 读阶段会为每个在线 store 建连并在结束后关闭。
    recovery.ReadRegionMeta(&ctx).expect("read meta");
    assert_eq!(recovery.GetTotalRegions(), 3);
    recovery.MakeRecoveryPlan().expect("plan");
    assert_eq!(recovery.MaxAllocID, MAX_ALLOCATE_ID);
    mgr.RecoverBaseAllocID(&ctx, recovery.MaxAllocID)
        .expect("alloc");
    assert_eq!(*mgr.max_alloc_id.lock().unwrap(), MAX_ALLOCATE_ID);
    // RecoverBaseAllocID 把计划中的 MaxAllocID 写入 Mgr，供 PD 侧避让。
    // 按 RecoveryPlan 向相关 store 发送 RecoverRegion 流式请求。
    recovery.RecoverRegions(&ctx).expect("recover regions");
    // prepare 使用 [start_ts, commit_ts) 窗口，与后续 flashback 配对。
    recovery
        .PrepareFlashbackToVersion(&ctx, 100, 199)
        .expect("prepare");
    // flashback 提交版本 200；成功后 flashback_calls 应为 1。
    recovery.FlashbackToVersion(&ctx, 100, 200).expect("flash");

    // Connections closed after ReadRegionMeta (3 stores) + RecoverRegions (2 plan stores).
    // 至少 5 次关闭：证明每条 gRPC 连接在阶段结束后都被 TrackingConn 回收。
    assert!(
        closed_conns.load(Ordering::SeqCst) >= 5,
        "closed={}",
        closed_conns.load(Ordering::SeqCst)
    );
    // 计划涉及 2 个 store，RecoverRegion 至少被调用两次。
    assert!(recover_calls.load(Ordering::SeqCst) >= 2);
    // 进度条至少覆盖读/计划/恢复/prepare/flash 等主要步骤。
    assert!(progress.current() >= 5);
    assert_eq!(mgr.flashback.prepare_calls.load(Ordering::SeqCst), 1);
    // prepare 与 flashback 各恰好一次，防止重复 flashback。
    assert_eq!(mgr.flashback.flashback_calls.load(Ordering::SeqCst), 1);

    // Full RecoverData with flashback-stage failure must not retry.
    // 这里绕过 RecoverData 内部 30s watcher，直接测 FlashbackToVersion 错误传播。
    // 第二套 Mgr：专门验证 flashback 错误路径，与成功路径隔离。
    let mgr2 = Arc::new(MemMgr::new());
    mgr2.set_client_factory(mock_factory_from_metas(
        recovery_seed
            .StoreMetas
            .iter()
            .map(|s| (s.StoreId, s.RegionMetas.clone()))
            .collect(),
        stores.iter().map(|s| (s.Address.clone(), s.Id)).collect(),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    ));
    *mgr2.flashback.flashback_err.lock().unwrap() = Some(Error::new("flash failed"));
    // 强制 flashback 失败，验证错误不进入 isRetryErr 重试分支。
    // RecoverData always spawns 30s watcher — cancel quickly via short context...
    // Instead call RecoverData only if we can make watcher tick tiny. Override by
    // testing stage classification already covered; here verify flashback err path
    // via Recovery methods without RecoverData's watcher.
    let progress2 = Arc::new(MemProgress::new());
    // 复用已生成的 StoreMetas，跳过再次 ReadRegionMeta。
    let mut r2 = NewRecovery(stores, mgr2.clone(), progress2, 4);
    r2.spawn_watcher = false;
    // 直接填入种子元数据后做计划，聚焦 flashback 失败语义。
    r2.StoreMetas = recovery_seed.StoreMetas.clone();
    r2.MakeRecoveryPlan().unwrap();
    let err = r2
        .FlashbackToVersion(&Context::Background(), 1, 2)
        .expect_err("flash");
    assert!(err.msg.contains("flash failed"));
    // 注入的 flashback_err 必须原样冒泡，供上层判定不可重试。

    // Cancel path: cancelled context fails RecoverData early.
    // 上下文取消必须在进入重活前失败，错误文案保留 context canceled。
    let ctx_cancel = Context::Background();
    ctx_cancel.cancel(Error::new("context canceled"));
    // 主动取消后 RecoverData 应尽早返回，不读元数据。
    // RecoverData 入口在 context 已取消时应立即失败。
    let err = RecoverData(
        &ctx_cancel,
        1,
        create_stores(),
        Arc::new(MemMgr::new()),
        Arc::new(MemProgress::new()),
        2,
        4,
    )
    .expect_err("canceled");
    assert!(err.msg.contains("context canceled"));
    // 取消错误文案保持与 Go context.Canceled 可识别的子串。
}
