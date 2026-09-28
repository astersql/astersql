// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Service safe point helpers ported from `br/pkg/gc/safepoint.go`.
//!
//! BR 服务安全点辅助：ID 生成、相对 GC 的预检、以及后台 TTL 续约 Keeper。
//! 依赖 `Manager` 抽象，同时服务 global / keyspace 实现；错误类型对齐 Go juju Annotate/Trace。
//! Keeper 以 TTL/3 为更新间隔、固定 5s 再检 GC；ctx 取消后退出，对齐 Go ticker+select。
//! 默认 TTL 常量供备份/检查点/日志备份启停路径选用，数值与 Go 字面量对齐。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use astersql_br_pkg_errors::{ErrBackupGCSafepointExceeded, ErrInvalidArgument};

use crate::manager::Manager;

/// The error type used by this crate's public API, mirroring Go's `error`.
///
/// 可跨线程传递的装箱错误；本 crate 公开 API 统一返回该别名。
pub type SharedError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// AnnotatedError mirrors juju `errors.Annotatef`: message plus wrapped cause.
///
/// 外层补充上下文消息，内层保留原始 cause，供 Display 与 source 链使用。
#[derive(Debug)]
pub struct AnnotatedError {
    message: String,
    cause: SharedError,
}

impl AnnotatedError {
    /// 包装 cause 并附加 annotate 消息。
    pub fn new(cause: SharedError, message: String) -> Self {
        Self { message, cause }
    }

    /// 取被包装的原始错误。
    pub fn cause(&self) -> &SharedError {
        &self.cause
    }
}

impl std::fmt::Display for AnnotatedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 格式 `消息: cause`，与 Go Annotatef 可读性接近。
        write!(f, "{}: {}", self.message, self.cause)
    }
}

impl std::error::Error for AnnotatedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

/// Trace mirrors `errors.Trace`: keeps the cause unchanged (stack capture is a
/// no-op in the Rust port).
///
/// Rust 端口不抓栈，仅透传错误以保持调用点语义。
pub fn Trace(err: SharedError) -> SharedError {
    err
}

/// 服务安全点 ID 前缀，与 Go `br-%s` 格式一致。
const BR_SERVICE_SAFE_POINT_ID_FORMAT: &str = "br-";
/// 更新间隔 = TTL / 该因子，保证 TTL 到期前多次续约。
const PRE_UPDATE_SERVICE_SAFE_POINT_FACTOR: u32 = 3;
/// Keeper 周期性 CheckGCSafePoint 的间隔（秒级，与 Go 常量一致）。
const CHECK_GC_SAFE_POINT_GAP_TIME: Duration = Duration::from_secs(5);

/// DefaultBRGCSafePointTTL means PD keep safePoint limit at least 5min.
/// 默认 BR GC 安全点 TTL：至少 5 分钟。
pub const DefaultBRGCSafePointTTL: i64 = 5 * 60;
/// DefaultCheckpointGCSafePointTTL means PD keep safePoint limit at least 72 minutes.
/// 检查点场景默认 TTL：72 分钟。
pub const DefaultCheckpointGCSafePointTTL: i64 = 72 * 60;
/// DefaultStreamStartSafePointTTL specifies keeping the server safepoint 30 mins when start task.
/// 日志备份启动时默认保持 30 分钟。
pub const DefaultStreamStartSafePointTTL: i64 = 1800;
/// DefaultStreamPauseSafePointTTL specifies Keeping the server safePoint at list 24h when pause task.
/// 日志备份暂停时默认保持 24 小时。
pub const DefaultStreamPauseSafePointTTL: i64 = 24 * 3600;

/// Context is a minimal cancellation context mirroring Go `context.Context`
/// as used by this package (only `Done` semantics are needed).
///
/// 仅实现取消标志；本包不需要 deadline/value 传递。
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
}

impl Context {
    /// Background mirrors `context.Background()`.
    ///
    /// 永不取消的根上下文。
    pub fn Background() -> Self {
        Self::default()
    }

    /// WithCancel mirrors `context.WithCancel`; call the returned closure to cancel.
    ///
    /// 返回可克隆的 Context 与 cancel 闭包；多线程共享同一 AtomicBool。
    pub fn WithCancel() -> (Self, impl Fn() + Send + Sync + 'static) {
        let ctx = Self::default();
        let flag = ctx.cancelled.clone();
        (ctx, move || flag.store(true, Ordering::SeqCst))
    }

    /// Done reports whether the context has been cancelled.
    ///
    /// 对应 Go `ctx.Done()` 可读侧：true 表示应停止后台循环。
    pub fn Done(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// BRServiceSafePoint is metadata of service safe point from a BR 'instance'.
///
/// 一次 BR 实例的服务安全点元数据：ID、TTL（秒）与备份时间戳。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BRServiceSafePoint {
    pub ID: String,
    pub TTL: i64,
    pub BackupTS: u64,
}

/// GetTimeFromTS mirrors `oracle.GetTimeFromTS`: extract physical time from a TSO.
///
/// 从 TSO 右移 18 位取物理毫秒，拼成 SystemTime。
fn get_time_from_ts(ts: u64) -> SystemTime {
    const PHYSICAL_SHIFT_BITS: u64 = 18;
    UNIX_EPOCH + Duration::from_millis(ts >> PHYSICAL_SHIFT_BITS)
}

impl BRServiceSafePoint {
    /// MarshalLogObject mirrors zapcore.ObjectMarshaler: renders the fields
    /// used by structured logging in the same order as Go.
    ///
    /// 按 ID → TTL → BackupTime → BackupTS 顺序渲染，供告警/日志复用。
    pub fn MarshalLogObject(&self) -> String {
        let backup_time = get_time_from_ts(self.BackupTS);
        let backup_unix = backup_time
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        format!(
            "ID={} TTL={:?} BackupTime={}.{:03} BackupTS={}",
            self.ID,
            GoDuration(self.TTL),
            backup_unix.as_secs(),
            backup_unix.subsec_millis(),
            self.BackupTS
        )
    }
}

/// Whole-second subset of Go `time.Duration.String()` used by the source API.
///
/// `BRServiceSafePoint.TTL` is expressed in seconds, so the Go result only
/// needs hour, minute, and second components. The sign is preserved, including
/// for `i64::MIN`, instead of being clamped as `std::time::Duration` would
/// require.
struct GoDuration(i64);

impl std::fmt::Debug for GoDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let negative = self.0 < 0;
        let mut seconds = self.0.unsigned_abs();
        let hours = seconds / 3600;
        seconds %= 3600;
        let minutes = seconds / 60;
        seconds %= 60;

        if negative {
            f.write_str("-")?;
        }
        if hours > 0 {
            write!(f, "{hours}h{minutes}m{seconds}s")
        } else if minutes > 0 {
            write!(f, "{minutes}m{seconds}s")
        } else {
            write!(f, "{seconds}s")
        }
    }
}

/// MakeSafePointID makes a unique safe point ID, for reduce name conflict.
///
/// Mirrors Go `fmt.Sprintf("br-%s", uuid.New())` — a random v4-style UUID.
///
/// 生成 `br-` + UUID v4 形态字符串，降低多实例 ID 冲突概率。
pub fn MakeSafePointID() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // 混合时间/进程/序号作 PRNG 种子，无需外部 uuid crate。
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as u64;
    let seq = COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut state = nanos ^ seq.rotate_left(32) ^ (std::process::id() as u64).rotate_left(48);
    let mut next = || {
        // xorshift64* pseudo-random step; uniqueness is what matters here.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let hi = next();
    let lo = next();
    // Format as a RFC 4122 style UUID string with version 4 / variant bits.
    // 强制 version=4、variant=10xx，长度与 Go uuid.New() 字符串一致。
    let bytes_hi = hi.to_be_bytes();
    let bytes_lo = lo.to_be_bytes();
    format!(
        "{}{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:01x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        BR_SERVICE_SAFE_POINT_ID_FORMAT,
        bytes_hi[0],
        bytes_hi[1],
        bytes_hi[2],
        bytes_hi[3],
        bytes_hi[4],
        bytes_hi[5],
        bytes_hi[6] & 0x0f,
        bytes_hi[7],
        (bytes_lo[0] & 0x3f) | 0x80,
        bytes_lo[1],
        bytes_lo[2],
        bytes_lo[3],
        bytes_lo[4],
        bytes_lo[5],
        bytes_lo[6],
        bytes_lo[7]
    )
}

/// CheckGCSafePoint checks whether the ts is older than GC safepoint using Manager.
/// Note: It ignores errors other than exceed GC safepoint.
///
/// ts 必须严格大于当前 GC 安全点；读安全点失败仅告警并视为通过（与 Go 一致）。
pub fn CheckGCSafePoint(ctx: &Context, mgr: &dyn Manager, ts: u64) -> Result<(), SharedError> {
    let safe_point = match mgr.GetGCSafePoint(ctx) {
        Ok(safe_point) => safe_point,
        Err(err) => {
            // Go: log.Warn("fail to get GC safe point", zap.Error(err)) and swallow.
            // 非「越过安全点」类错误一律吞掉，避免误杀备份。
            eprintln!("[WARN] fail to get GC safe point: {err}");
            return Ok(());
        }
    };
    // ts <= safe_point：备份时间戳已被 GC 越过，包装 ErrBackupGCSafepointExceeded。
    if ts <= safe_point {
        return Err(Box::new(AnnotatedError::new(
            Box::new((*ErrBackupGCSafepointExceeded).clone()),
            format!("GC safepoint {safe_point} exceed TS {ts}"),
        )));
    }
    Ok(())
}

/// StartServiceSafePointKeeper starts a background thread to periodically
/// update the service safe point. It uses the provided Manager to set the
/// safe point. The keeper will run until the context is canceled.
///
/// 启动前校验 ID/TTL，并做 BackupTS 预检；同步首次 Set 成功后才拉起续约线程。
pub fn StartServiceSafePointKeeper(
    ctx: &Context,
    sp: BRServiceSafePoint,
    mgr: Arc<dyn Manager>,
) -> Result<(), SharedError> {
    // 空 ID 或非正 TTL 视为无效参数。
    if sp.ID.is_empty() || sp.TTL <= 0 {
        return Err(Box::new(AnnotatedError::new(
            Box::new((*ErrInvalidArgument).clone()),
            format!("invalid service safe point {sp:?}"),
        )));
    }
    if let Err(err) = CheckGCSafePoint(ctx, mgr.as_ref(), sp.BackupTS) {
        return Err(Trace(err));
    }
    // Set service safe point immediately to cover the gap between starting
    // update goroutine and updating service safe point.
    // 同步首次写入，填补线程启动前的保护空窗。
    if let Err(err) = mgr.SetServiceSafePoint(ctx, sp.clone()) {
        return Err(Trace(err));
    }

    // It would be OK since TTL won't be zero, so gapTime should > 0.
    // TTL 已保证 >0，故更新间隔为正。
    let update_gap_time = Duration::from_secs(sp.TTL as u64) / PRE_UPDATE_SERVICE_SAFE_POINT_FACTOR;
    let keeper_ctx = ctx.clone();
    thread::spawn(move || {
        let mut next_update = Instant::now() + update_gap_time;
        let mut next_check = Instant::now() + CHECK_GC_SAFE_POINT_GAP_TIME;
        loop {
            if keeper_ctx.Done() {
                // Go: log.Debug("service safe point keeper exited")
                // 取消后干净退出，不再续约。
                return;
            }
            let now = Instant::now();
            if now >= next_update {
                next_update = now + update_gap_time;
                // 续约失败只告警，不退出；下一周期再试。
                if let Err(err) = mgr.SetServiceSafePoint(&keeper_ctx, sp.clone()) {
                    eprintln!(
                        "[WARN] failed to update service safe point, backup may fail if gc triggered: {err}"
                    );
                }
            }
            let now = Instant::now();
            if now >= next_check {
                next_check = now + CHECK_GC_SAFE_POINT_GAP_TIME;
                if let Err(err) = CheckGCSafePoint(&keeper_ctx, mgr.as_ref(), sp.BackupTS) {
                    // Go: log.Panic("cannot pass gc safe point check, aborting", ...)
                    // GC 已越过备份 TS：进程级 panic，与 Go log.Panic 对齐。
                    panic!(
                        "cannot pass gc safe point check, aborting: {err}; safePoint: {}",
                        sp.MarshalLogObject()
                    );
                }
            }
            let sleep_until = next_update.min(next_check);
            let now = Instant::now();
            let mut wait = sleep_until.saturating_duration_since(now);
            // Poll cancellation with a small granularity like Go's select.
            // 最长睡 10ms，以便及时响应 cancel。
            if wait > Duration::from_millis(10) {
                wait = Duration::from_millis(10);
            }
            thread::sleep(wait);
        }
    });
    Ok(())
}
