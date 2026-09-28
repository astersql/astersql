// Copyright 2026 AsterSQL.

// `recovery_handler` 的 Aster 单元测试。
//
// 覆盖：FIFO 缓冲与父 Tracker 内存记账、禁用/空队列/零容量错误路径、
// handler 按序选择与失败尝试消耗重试次数、以及真实全局 TiFlash compute
// fetcher 在 Memory limit 场景下的调用。
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_util_memory as memory;
use astersql_util_tiflashcompute as tiflashcompute;

use crate::recovery_handler::{HandlerImpl, NewRecoveryHandler, RecoveryInfo};

/// 仅携带固定 `MemSize` 的假响应，用于验证缓冲与内存追踪。
struct SizedResponse(i64);

impl kv::ResultSubset for SizedResponse {
    fn GetData(&self) -> &[u8] {
        &[]
    }

    fn GetStartKey(&self) -> kv::Key {
        kv::Key::default()
    }

    fn MemSize(&self) -> i64 {
        self.0
    }

    fn RespTime(&self) -> Duration {
        Duration::ZERO
    }
}

#[test]
/// 验证缓冲为 FIFO，且占用计入真实父 Tracker；弹出后不可再 hold，Reset 可恢复。
fn holder_is_fifo_and_accounts_memory_to_the_real_parent_tracker() {
    let mut parent = memory::tracker::NewTracker(42, 0);
    let mut recovery = NewRecoveryHandler(false, 2, true, &mut parent);

    assert!(recovery.Enabled());
    assert!(recovery.CanHoldResult());
    // 容量为 2：放入两条后 CanHoldResult 应变为 false。
    recovery.HoldResult(Box::new(SizedResponse(10)));
    assert!(recovery.CanHoldResult());
    recovery.HoldResult(Box::new(SizedResponse(20)));
    assert!(!recovery.CanHoldResult());
    assert_eq!(recovery.NumHoldResp(), 2);
    assert_eq!(recovery.HolderBytesConsumed(), 30);
    assert_eq!(parent.BytesConsumed(), 30);

    // 弹出队首后内存应扣减，且即使队列未满也禁止继续 hold。
    let first = recovery.PopFrontResp().expect("the first response exists");
    assert_eq!(first.MemSize(), 10);
    assert_eq!(recovery.NumHoldResp(), 1);
    assert_eq!(parent.BytesConsumed(), 20);
    // Returning any response makes recovery unsafe even if the queue is below
    // its original capacity.
    assert!(!recovery.CanHoldResult());

    // Reset 清空缓冲并允许再次 hold，但恢复计数不受影响（本用例未测计数）。
    recovery.ResetHolder();
    assert_eq!(recovery.NumHoldResp(), 0);
    assert_eq!(parent.BytesConsumed(), 0);
    assert!(recovery.CanHoldResult());
}

#[test]
/// 核对禁用恢复、空队列弹出与零容量路径的错误文案与 Go 一致。
fn disabled_empty_and_zero_capacity_paths_match_go_errors() {
    let mut parent = memory::tracker::NewTracker(7, 0);
    let mut disabled = NewRecoveryHandler(false, 1, false, &mut parent);
    assert!(!disabled.Enabled());
    assert!(
        disabled
            .PopFrontResp()
            .err()
            .expect("disabled recovery rejects pop")
            .to_string()
            .contains("enable: false, size: 0")
    );
    assert_eq!(disabled.RecoveryCnt(), 0);
    assert_eq!(
        disabled.Recovery(None).unwrap_err().to_string(),
        "mpp err recovery is not enabled"
    );

    // 容量为 0 时即使启用也不能 hold。
    drop(disabled);
    let zero_capacity = NewRecoveryHandler(false, 0, true, &mut parent);
    assert!(!zero_capacity.CanHoldResult());
}

/// 可配置是否被选中的测试 handler，用于记录 `doRecovery` 收到的节点数。
struct RecordingHandler {
    /// `chooseHandlerImpl` 的固定返回值。
    selected: bool,
    /// 被选中执行恢复时写入的 NodeCnt。
    recovered_node_count: Arc<AtomicI32>,
}

impl HandlerImpl for RecordingHandler {
    fn chooseHandlerImpl(&self, _: &errors::SharedError) -> bool {
        self.selected
    }

    fn doRecovery(&self, info: &RecoveryInfo) -> Result<(), errors::SharedError> {
        self.recovered_node_count
            .store(info.NodeCnt, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
/// 验证按 handlers 顺序选择，且无匹配/失败尝试同样消耗恢复计数直至达上限。
fn recovery_selects_in_order_and_consumes_failed_or_unsupported_attempts() {
    let mut parent = memory::tracker::NewTracker(8, 0);
    let mut recovery = NewRecoveryHandler(false, 1, true, &mut parent);
    let recovered_node_count = Arc::new(AtomicI32::new(0));
    // 第一个不匹配，第二个匹配；应只执行第二个并记 NodeCnt=6。
    recovery.handlers = vec![
        Box::new(RecordingHandler {
            selected: false,
            recovered_node_count: Arc::clone(&recovered_node_count),
        }),
        Box::new(RecordingHandler {
            selected: true,
            recovered_node_count: Arc::clone(&recovered_node_count),
        }),
    ];
    // 错误文本需包含 "Memory limit" 子串以匹配默认 handler。
    let info = RecoveryInfo {
        MPPErr: Some(errors::New("recoverable MPP failure")),
        NodeCnt: 6,
    };
    recovery
        .Recovery(Some(&info))
        .expect("second handler recovers");
    assert_eq!(recovery.RecoveryCnt(), 1);
    assert_eq!(recovered_node_count.load(Ordering::SeqCst), 6);

    // 清空 handlers 后每次调用仍递增计数，直到超过 maxRecoveryCnt。
    recovery.handlers.clear();
    for expected_count in 2..=3 {
        assert_eq!(
            recovery.Recovery(Some(&info)).unwrap_err().to_string(),
            "no handler to recovery this type of mpp err"
        );
        assert_eq!(recovery.RecoveryCnt(), expected_count);
    }
    assert!(
        recovery
            .Recovery(Some(&info))
            .unwrap_err()
            .to_string()
            .contains("exceeds max recovery cnt: cur: 3, max: 3")
    );
    assert_eq!(recovery.RecoveryCnt(), 3);
}

#[test]
/// 启用 AutoScaler 时，Memory limit 错误应走到真实全局拓扑 fetcher。
fn mem_limit_handler_calls_the_real_global_tiflash_compute_fetcher() {
    // 初始化测试用全局拓扑拉取器，以便 MemLimitHandler 能取到 fetcher。
    tiflashcompute::InitGlobalTopoFetcher(
        tiflashcompute::config::TestASStr.to_owned(),
        "test-autoscaler".to_owned(),
        "test-cluster".to_owned(),
        false,
    )
    .expect("test topology fetcher initializes");

    let mut parent = memory::tracker::NewTracker(9, 0);
    let mut recovery = NewRecoveryHandler(true, 1, true, &mut parent);
    let info = RecoveryInfo {
        MPPErr: Some(errors::New("TiFlash Memory limit exceeded")),
        NodeCnt: 3,
    };
    let error = recovery
        .Recovery(Some(&info))
        .expect_err("the test fetcher exposes its real recovery error");
    assert_eq!(error.to_string(), "RecoveryAndGetTopo not implemented");
    assert_eq!(recovery.RecoveryCnt(), 1);
}
