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

//! Go-equivalent tests for `br/pkg/backup/store_test.go`.
//!
//! 覆盖 `store.rs` 超时看门狗与取消语义，场景与 Go `store_test.go` 对齐：
//! 1) 首包超时；2) 非首包超时；3) 父取消后 Stop；4) Stop 取消派生 context。
//! 通过 `MockBackupClient` 注入 `Recv` 阻塞/计数行为，不依赖真实 TiKV。
//! 测试会调用 `set_timeout_one_response_for_test` 缩短默认 1h 超时。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::limit::NewResourceMemoryLimiter;
use crate::store::{StartTimeoutRecv, set_timeout_one_response_for_test, startBackup};
use crate::stubs::backuppb::{BackupClient, BackupRequest, BackupResponse, BackupStream};
use crate::stubs::{Context, Error, Result};

/// MockBackupClient injects Recv behaviour (Go MockBackupClient.recvFunc).
///
/// 将 Go 侧 `recvFunc` 钩子迁为闭包；`Backup` 始终成功返回流，错误仅在 Recv 注入。
struct MockBackupClient {
    /// 每次 Recv 调用的注入函数；可读取 ctx 是否已被超时取消。
    recv_func: Arc<dyn Fn(&Context) -> Result<Option<BackupResponse>> + Send + Sync>,
}

impl BackupClient for MockBackupClient {
    /// 构造绑定同一 `recv_func` 的流；忽略请求内容（超时用例不关心 range）。
    fn Backup(&self, ctx: &Context, _req: &BackupRequest) -> Result<Box<dyn BackupStream>> {
        Ok(Box::new(MockBackupStream {
            ctx: ctx.clone(),
            recv_func: Arc::clone(&self.recv_func),
            closed: false,
        }))
    }
}

/// 内存 Backup 流：CloseSend 后 Recv 返回 None，模拟 gRPC 半关闭。
struct MockBackupStream {
    ctx: Context,
    recv_func: Arc<dyn Fn(&Context) -> Result<Option<BackupResponse>> + Send + Sync>,
    closed: bool,
}

impl BackupStream for MockBackupStream {
    fn Recv(&mut self) -> Result<Option<BackupResponse>> {
        if self.closed {
            return Ok(None);
        }
        // 将派生 ctx 传入闭包，断言超时 cancel 已生效。
        (self.recv_func)(&self.ctx)
    }
    fn CloseSend(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}

/// TestTimeoutRecv: first Recv blocks past timeout; then non-first-packet timeout.
///
/// 对齐 Go `TestTimeoutRecv`：先验证首包超过 800ms 未 Refresh 即失败；
/// 再验证连续 15 包短间隔成功后，第 16 次阻塞触发超时，且计数恰为 15。
#[test]
fn test_timeout_recv() {
    // 生产默认 1h，测试必须覆盖为亚秒级，否则用例无法在合理时间完成。
    set_timeout_one_response_for_test(Some(Duration::from_millis(800)));
    let ctx = Context::Background();

    // Just Timeout Once — 首包 sleep 1s > 800ms，期望 startBackup 返回 Err。
    {
        let err = startBackup(
            &ctx,
            0,
            Arc::new(NewResourceMemoryLimiter(100)),
            BackupRequest::default(),
            Arc::new(MockBackupClient {
                recv_func: Arc::new(|ctx| {
                    thread::sleep(Duration::from_secs(1));
                    // 看门狗应已 cancel 子 ctx；此处把 cancel cause 向上返回。
                    assert!(ctx.Err().is_some(), "timeout should cancel child ctx");
                    Err(ctx.Err().unwrap())
                }),
            }),
            1,
            {
                let (tx, _rx) = mpsc::channel();
                tx
            },
        );
        assert!(err.is_err(), "first-packet timeout must error");
    }

    // Timeout Not At First — 前 15 次及时 Refresh，第 16 次卡死触发超时。
    {
        let count = Arc::new(AtomicUsize::new(0));
        let count2 = Arc::clone(&count);
        let (tx, rx) = mpsc::channel();
        // Drain responses (Go: make(chan, 15)).
        // 后台排空通道，避免 send 阻塞影响超时计时。
        thread::spawn(move || while rx.recv().is_ok() {});
        let err = startBackup(
            &ctx,
            0,
            Arc::new(NewResourceMemoryLimiter(100)),
            BackupRequest::default(),
            Arc::new(MockBackupClient {
                recv_func: Arc::new(move |ctx| {
                    // 进入第 16 次前 ctx 仍应健康。
                    assert!(ctx.Err().is_none());
                    let c = count2.load(Ordering::SeqCst);
                    if c == 15 {
                        thread::sleep(Duration::from_secs(1));
                        assert!(ctx.Err().is_some());
                        return Err(ctx.Err().unwrap());
                    }
                    count2.fetch_add(1, Ordering::SeqCst);
                    // 80ms << 800ms，保证 Refresh 能重置看门狗。
                    thread::sleep(Duration::from_millis(80));
                    Ok(Some(BackupResponse::default()))
                }),
            }),
            1,
            tx,
        );
        assert!(err.is_err());
        // 恰好成功投递 15 包后超时，与 Go 断言一致。
        assert_eq!(count.load(Ordering::SeqCst), 15);
    }

    // 恢复默认，避免污染同进程其他用例。
    set_timeout_one_response_for_test(None);
}

/// TestTimeoutRecvCancel: cancel parent; timeoutRecv worker exits.
///
/// 父 context 取消后调用 Stop，应能 join 刷新循环而不死锁（对齐 Go WaitGroup）。
#[test]
fn test_timeout_recv_cancel() {
    let ctx = Context::Background();
    let (cctx, cancel) = Context::WithCancel(&ctx);

    let (_tctx, trecv) = StartTimeoutRecv(&cctx, Duration::from_secs(3600), 0);
    cancel.cancel();
    // Go waits trecv.wg; Rust Stop joins the refresh loop.
    // 即使父已 Done，Stop 仍应幂等完成清理。
    trecv.Stop();
}

/// TestTimeoutRecvCanceled: Stop cancels derived context.
///
/// 主动 Stop 后派生 `tctx` 必须变为 canceled，供调用方感知结束。
#[test]
fn test_timeout_recv_canceled() {
    let ctx = Context::Background();
    let (cctx, cancel) = Context::WithCancel(&ctx);
    let _keep = cancel; // defer cancel at end of scope
    // 保留 cancel 句柄至作用域结束，避免过早 drop 影响派生链。

    let (tctx, trecv) = StartTimeoutRecv(&cctx, Duration::from_secs(3600), 0);
    trecv.Stop();
    let err = tctx.Err().expect("derived ctx must be canceled");
    assert!(
        err.msg.contains("context canceled") || err.msg.contains("canceled"),
        "got {}",
        err.msg
    );
}

// silence unused import of Mutex in case of future fixtures
// 占位以消除部分构建下 Mutex 未使用警告。
#[allow(dead_code)]
fn _lock_type() -> Mutex<()> {
    Mutex::new(())
}
