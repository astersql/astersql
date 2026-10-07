# `br/pkg/gc/safepoint.rs`

## 文件定位

`safepoint.rs` 属于 `astersql-br-pkg-gc` library crate；crate 入口 `br/pkg/gc/lib.rs` 以 `pub mod safepoint` 装载它，并重导出其中的安全点数据结构、四个默认 TTL、上下文、检查函数、ID 生成函数和 Keeper 启动函数。`br/pkg/gc/Cargo.toml` 表明该 crate 的生产依赖只有 `astersql-br-pkg-errors`，PD/global/keyspace 差异则被同 crate 的 `Manager` trait 隔离。

它位于 BR 快照读取与 PD GC 机制之间：调用方先用 `CheckGCSafePoint` 确认历史版本尚可读，再用 `StartServiceSafePointKeeper` 注册并续约服务安全点，阻止 GC 在备份存续期间越过 `BackupTS`。实际的 global service safepoint 或 keyspace barrier 写入由 `manager_global.rs`、`manager_keyspace.rs` 实现，不在本文件中直接发起 RPC。

已接线的 Rust 入口包括：`br/pkg/task/backup.rs:379-399` 在备份开始前构造 `BRServiceSafePoint` 并启动 Keeper；`br/pkg/backup/client.rs:434-466` 分别调用检查、生成 ID，并为非法 TTL 回落到默认值。RustCodeGraph 对重导出后的精确 `callers` 没有返回边，因此这些上游关系由源码引用补证，而不是把图查询空结果解释为“无人调用”。

## 核心职责

本文件承担五类职责：

1. 定义 BR、checkpoint、日志备份启动/暂停场景的默认 TTL，单位均为秒。
2. 用 `BRServiceSafePoint { ID, TTL, BackupTS }` 表示一次 BR 实例对历史版本的保护请求。
3. 用 `MakeSafePointID` 生成 `br-` 前缀、UUID v4 外形的低冲突服务 ID。
4. 用 `CheckGCSafePoint` 执行关键不变量：只有 `ts > current_gc_safepoint` 才可继续。
5. 用 `StartServiceSafePointKeeper` 完成参数检查、启动前预检、同步首次注册，以及后台按 `TTL / 3` 续约和每 5 秒复检。

本文件还提供迁移期基础设施：`Context` 只模拟 Go `context.Context` 的取消信号；`SharedError`、`AnnotatedError` 和 `Trace` 提供可跨线程的错误边界及 cause 链。它们不是完整的 deadline/value/堆栈跟踪实现。

## 主要符号

- `SharedError = Box<dyn Error + Send + Sync + 'static>`：所有公开 fallible API 的动态错误类型，允许错误跨 Keeper 线程边界。
- `AnnotatedError::new/cause`、`Display`、`Error::source`：给底层错误增加上下文，同时保留 `source`；`Trace` 当前原样返回错误，不捕获 Rust backtrace。
- `BR_SERVICE_SAFE_POINT_ID_FORMAT`：固定前缀 `br-`。`PRE_UPDATE_SERVICE_SAFE_POINT_FACTOR = 3` 决定续约频率；`CHECK_GC_SAFE_POINT_GAP_TIME = 5s` 决定 GC 复检频率。
- `DefaultBRGCSafePointTTL = 300`、`DefaultCheckpointGCSafePointTTL = 4320`、`DefaultStreamStartSafePointTTL = 1800`、`DefaultStreamPauseSafePointTTL = 86400`：分别对应普通 BR、checkpoint、stream start、stream pause。
- `Context::Background/WithCancel/Done`：以 `Arc<AtomicBool>` 共享取消状态；`WithCancel` 返回上下文和可在线程间调用的闭包。
- `BRServiceSafePoint`：公开字段 `ID: String`、`TTL: i64`、`BackupTS: u64`。`MarshalLogObject` 生成包含 ID、Go 风格秒级 TTL、由 TSO 物理部分换算的时间和原始 TSO 的字符串。
- `get_time_from_ts`：将 TSO 右移 18 位得到 Unix 毫秒。`GoDuration` 处理小时/分钟/秒及负号，包括 `i64::MIN`，只供日志格式化。
- `MakeSafePointID`：把当前纳秒、进程 ID、全局原子序号混入 xorshift64* 状态，格式化为带 version 4/variant 位的 UUID 外形。
- `CheckGCSafePoint(ctx, mgr, ts)`：通过 `Manager::GetGCSafePoint` 读取当前值；相等或落后时报 `ErrBackupGCSafepointExceeded`。
- `StartServiceSafePointKeeper(ctx, sp, mgr)`：校验并同步注册后，分离一个后台线程负责续约与复检；成功返回不代表后台线程以后不会告警或 panic。

## 执行流程

典型备份链路如下：

1. `BackupClient::GetSafePointID` 调用 `MakeSafePointID`，或从 checkpoint 元数据复用既有 ID；备份任务构造 `BRServiceSafePoint`。
2. `StartServiceSafePointKeeper` 先拒绝空 ID 或 `TTL <= 0`，此时不会读写 Manager。
3. 它调用 `CheckGCSafePoint(sp.BackupTS)`。若当前 GC safepoint 大于等于备份 TSO，立即返回带 `ErrBackupGCSafepointExceeded` cause 的错误；若读取 Manager 失败，则仅告警并继续。
4. 它同步调用一次 `Manager::SetServiceSafePoint`。这一步先于线程创建，避免“函数已返回但保护尚未建立”的空窗；失败会原样传播且不会创建 Keeper 线程。
5. 首次写入成功后，计算 `update_gap_time = TTL / 3`，克隆上下文、Manager 与 safe point，启动分离线程。
6. 后台循环先看 `Context::Done`；到续约时刻时重新调用 `SetServiceSafePoint`，失败只告警，仍等待下一周期；到 5 秒复检时刻时调用 `CheckGCSafePoint`，失败则在线程内 panic，并附上 `MarshalLogObject` 信息。
7. 每轮休眠到两个 deadline 中较早者，但单次最多 10ms，以轮询方式降低取消延迟。调用方取消上下文后线程自行退出。

单独调用 `CheckGCSafePoint` 的链路更短：例如 `br/pkg/backup/client.rs:434-436` 在确定 `backupTS` 后读取 Manager；只有严格大于 safepoint 才返回 `Ok(())`。

## 数据与状态

`BRServiceSafePoint` 是 Keeper 捕获的不可变值；后台每次续约都克隆它。`TTL` 的单位是秒，`BackupTS` 是 TSO 而非 Unix 时间。Manager 实现把它转换为实际保护边界：`parity_test.rs` 证明 global 与 keyspace 路径最终使用 `BackupTS - 1`，并按作用域隔离服务点/barrier；该减一语义位于 Manager 实现而非本文件。

进程内可变状态有两处：`Context.cancelled` 是共享 `AtomicBool`，全部读写使用 `SeqCst`；`MakeSafePointID` 的静态 `COUNTER: AtomicU64` 也使用 `SeqCst`，与时间和 PID 一起降低同进程并发碰撞。Keeper 自己只维护 `next_update`、`next_check` 两个单线程 `Instant` deadline。

`MarshalLogObject` 不修改服务点。它从 TSO 物理位构造 `SystemTime`，对 `duration_since(UNIX_EPOCH)` 的异常回落为零，并用 `GoDuration` 保留负 TTL 的日志表现；这不意味着负 TTL 可用于启动 Keeper，启动校验仍会拒绝。

## 依赖与调用关系

上游：

- `br/pkg/gc/lib.rs` 是 canonical 导出面。
- `br/pkg/task/backup.rs` 在正式备份启动路径上创建可取消上下文、注册 Keeper，并由 `BackupGCGuard` 持有清理状态。
- `br/pkg/backup/client.rs` 使用 `CheckGCSafePoint`、`MakeSafePointID` 和 `DefaultBRGCSafePointTTL`。
- `br/pkg/gc/safepoint_test.rs` 与 `br/pkg/gc/parity_test.rs` 是直接 Rust 测试证据；其他本地 `stubs.rs` 中的同名函数是独立轻量边界，不能视作本实现的调用者。

下游：

- `crate::manager::Manager` 提供 `GetGCSafePoint` 和 `SetServiceSafePoint`；trait 要求 `Send + Sync`，因此可放入 `Arc<dyn Manager>` 并移交后台线程。
- `astersql_br_pkg_errors::{ErrBackupGCSafepointExceeded, ErrInvalidArgument}` 提供可识别的根错误，`AnnotatedError` 增加现场数值。
- 标准库提供原子状态、墙钟/单调时钟、线程和休眠。本 crate 的 Cargo 生产依赖没有 uuid、Tokio、PD 或 gRPC，因此 ID 生成和 Keeper 调度均为本地同步实现。

RustCodeGraph 显示 `safepoint.rs` 被 crate 内 Manager/测试等文件引用，并确认 `StartServiceSafePointKeeper -> CheckGCSafePoint` 的内部调用边；对该 Rust 符号执行精确 `callers/callees` 未产出边，上述跨 crate 调用点由 `rg` 和对应源码节点核验。

## 错误处理与边界

- 空 `ID`、零 TTL、负 TTL：`StartServiceSafePointKeeper` 返回包裹 `ErrInvalidArgument` 的 `AnnotatedError`，不会调用 Manager。
- `BackupTS <= GCSafePoint`：返回包裹 `ErrBackupGCSafepointExceeded` 的错误；等于也失败，这是防止读取刚好已不可用版本的严格边界。
- `GetGCSafePoint` 失败：`CheckGCSafePoint` 打印 warning 后返回成功。这是与 Go 对齐的可用性选择，但意味着短暂读取失败时启动流程可能继续；真正的首次写入仍必须成功。
- 首次 `SetServiceSafePoint` 失败：同步传播底层 error/source，不启动后台线程。
- 后续续约失败：只告警、不退出；若持续失败，TTL 可能过期并导致备份失去保护。
- 周期复检发现 GC 已越过：后台线程 panic。Rust 默认只终止该线程，而 Go `log.Panic` 通常会让未恢复的 goroutine panic 终止进程；因此当前端口在“全进程中止”效果上并不完全等价，调用方也拿不到 join handle 来观察该 panic。
- `MakeSafePointID` 生成 UUID v4 **外形**，但随机源是时间/PID/计数器混合的非密码学 PRNG，不等同于 Go `uuid.New()` 的系统随机保证。现有测试只验证格式和有限样本的顺序/并发唯一性。
- `Context` 不传播父子关系、deadline、取消原因或 values；它仅满足此文件的停止信号需求。

## 并发与资源生命周期

`StartServiceSafePointKeeper` 接受 `Arc<dyn Manager>`，线程闭包取得其所有权副本；即使启动函数返回，Manager 与 `BRServiceSafePoint` 仍保持存活。`Context` 的原子标志允许取消闭包和 Keeper 并发读写且无 mutex 中毒风险。

线程是 detached 的：函数不返回 `JoinHandle`，取消也不等待线程已经结束。Keeper 最多每 10ms 检查一次取消，因此正常情况下会很快退出，但调用方若必须确认资源回收，当前 API 没有同步确认机制。`safepoint_test.rs` 通过取消后等待 100ms 并检查调用次数不再增长来验证实际退出效果。

续约 deadline 使用 `Instant`，不受系统墙钟回拨影响。每次触发后以“当前时刻 + 周期”重置，而非追补错过的 tick；若一次 Manager 调用阻塞很久，不会并发堆叠补发。两个事件同轮到期时先续约、再复检。固定 10ms 轮询会带来每个活跃 Keeper 的周期唤醒成本，扩展到大量实例时应评估线程数和唤醒开销。

调用方负责生命周期终止。本文件不会在取消时自动调用 `DeleteServiceSafePoint`；删除/保留策略由更上层 guard 根据成功、失败和 checkpoint 语义决定。

## 与 Go 版本的对应关系

Rust 直接对照 `br/pkg/gc/safepoint.go`：四个 TTL、`BRServiceSafePoint` 字段、严格 `ts > safepoint` 判定、首次同步 Set、`TTL/3` 续约、5 秒复检、续约失败只告警及取消退出均保持一致。`br/pkg/gc/safepoint_test.go` 和 Rust 独立测试用相同的 global/keyspace 参数化场景覆盖空 ID、非正 TTL、落后/相等 TSO、首次写失败、周期刷新、取消与读错误忽略。

实现形态的差异包括：

- Go 使用 `context.Context`、两个 `time.Ticker` 和 goroutine；Rust 使用最小 `Context`、`Instant` deadline、10ms 轮询和 OS thread。
- Go `MarshalLogObject` 写入 zap object encoder 并返回 error；Rust 返回扁平字符串。字段信息和 TTL 文案被测试，但输出结构与 `time.Time.String()` 文案不是逐字相同 API。
- Go 使用 `google/uuid.New()`；Rust 为避免 Cargo 外部依赖而自行生成 UUID v4 外形，随机质量不同。
- Go `errors.Trace` 可附加栈语义；Rust `Trace` 是 no-op。两端都保留 cause 文本/类型意图。
- Go goroutine 中未恢复的 panic 具有进程级失败效果；Rust detached thread panic 默认不会自动终止整个进程，这是需要显式记录的迁移差异。
- Rust 测试还覆盖负 TTL 的日志格式，并总是运行约 1 秒的周期刷新用例；Go 在 short 模式跳过后者。

## 扩展指南

- 新增安全点字段时，应同时更新 `BRServiceSafePoint`、`MarshalLogObject`、global/keyspace Manager 转换逻辑、`safepoint_test.rs` 和 Go 对照；若字段影响公开 crate API，还需检查 `lib.rs` 重导出与所有结构体字面量。
- 修改合法性规则或 GC 边界时，优先改 `CheckGCSafePoint`/`StartServiceSafePointKeeper` 的同步阶段，并增加“失败前未写 Manager”的断言；必须维持 `ts == safepoint` 失败的不变量，除非上游 GC 协议明确变化。
- 修改续约策略时，关注 `TTL/3`、首次同步 Set、阻塞调用后的 deadline 重置、取消响应和错误降级策略。测试应留在独立的 `br/pkg/gc/safepoint_test.rs`，不要嵌回生产文件。
- 若要让周期复检真正中止任务/进程，应设计可观测的错误通道、join handle 或受控 abort 策略，不能假设 detached thread panic 与 Go 完全等价。
- 若提高 ID 安全性，应在独立上游依赖仓库按仓库规则移植并发布带 tag 的依赖，或使用已批准的系统随机接口；同时保留 `br-` 与 RFC 4122 格式兼容。
- 若替换轮询为条件变量、channel 或异步定时器，应保持 Manager 的 `Send + Sync` 约束、取消后不再续约，以及不会并发堆积多个 Set 的性质，并评估大量 Keeper 的线程/唤醒性能。

## 验证依据

本说明读取并核对了以下直接证据：

- 生产源码：`br/pkg/gc/safepoint.rs`（RustCodeGraph file node，完整 337 行）、`br/pkg/gc/manager.rs`、`br/pkg/gc/lib.rs`。
- crate 边界：`br/pkg/gc/Cargo.toml`，确认 library 路径、Go 包映射及唯一生产依赖。
- Go 对照：`br/pkg/gc/safepoint.go`、`br/pkg/gc/safepoint_test.go`。
- Rust 测试：`br/pkg/gc/safepoint_test.rs`；补充公开契约证据 `br/pkg/gc/parity_test.rs:288-504`。
- 实际上游：`br/pkg/task/backup.rs:379-399`、`br/pkg/backup/client.rs:434-466`。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/gc` 找到目标及同目录实现/测试；`explore` 确认 `StartServiceSafePointKeeper -> CheckGCSafePoint`；对 `MakeSafePointID`、`CheckGCSafePoint`、`StartServiceSafePointKeeper` 做了精确 `query`，并对目标 ID 执行 `callers/callees`（无输出，故以上游源码引用补证）。

人工复核结论：本文件存在于备份历史版本读取与 GC 之间，先同步建立保护、后后台续约；安全扩展必须同时维持严格 TSO 边界、首次写入无空窗、取消生命周期与 global/keyspace Manager 抽象。任务为纯文档分析，按计划不运行 Cargo。
