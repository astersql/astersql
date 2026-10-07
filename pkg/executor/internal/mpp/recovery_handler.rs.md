# `pkg/executor/internal/mpp/recovery_handler.rs`

## 文件定位

本文件属于 `astersql-executor-internal-mpp` crate；crate 入口 `pkg/executor/internal/mpp/lib.rs` 将它声明为私有模块 `recovery_handler`，没有直接向 crate 外重导出其中的类型。它位于 MPP 响应消费链的恢复层：`executor_with_retry.rs` 持有一个 `RecoveryHandler`，在把 TiFlash MPP 结果交给上层之前先进行有限缓冲；若预拉期间出现可恢复错误，则触发拓扑恢复、重建 coordinator，并丢弃旧执行轮次尚未对外可见的结果。

`pkg/executor/internal/mpp/Cargo.toml` 将该目录定义为独立库 crate，`[lib]` 指向 `lib.rs`，并通过 `package.metadata.porting.go-package = "pkg/executor/internal/mpp"` 标明 Go 对照包。本文件直接使用的 crate 依赖是 `astersql-errors`、`astersql-kv`、`astersql-util-memory` 和 `astersql-util-tiflashcompute`。

## 核心职责

1. 用 `MppResultHolder` 以 FIFO 顺序暂存 `Box<dyn kv::ResultSubset>`，并按每个响应的 `MemSize()` 向查询的内存 `Tracker` 记账。
2. 维持“结果一旦对外可见便不可再安全重试”的边界：`PopFrontResp` 弹出首条响应后把 `cannotHold` 设为 `true`。
3. 对 MPP 错误实施有上限的策略选择：`Recovery` 最多消耗三次恢复机会，并按 `handlers` 注册顺序选择第一个匹配策略。
4. 提供默认的 `MemLimitHandlerImpl`：仅当启用 AutoScaler 且错误文本包含 `"Memory limit"` 时，调用全局 TiFlash compute topology fetcher 的 `RecoveryAndGetTopo`。

它不负责重建 MPP coordinator。恢复策略成功后，真正关闭旧 coordinator、分配新 gather ID、重新 `Execute` 的逻辑在 `ExecutorWithRetry::setupMPPCoordinator(true)` 中。

## 主要符号

- `MEM_LIMIT_ERR_PATTERN: &str`：默认策略用于文本匹配的常量，值为 `"Memory limit"`，匹配区分大小写且不是结构化错误分类。
- `MppResponseRef = Box<dyn kv::ResultSubset>`：缓冲元素类型。动态 trait 对象使 holder 可接收任何满足 KV 结果子集契约的响应。
- `RecoveryInfo { MPPErr, NodeCnt }`：一次恢复的输入。`MPPErr` 是可空的共享错误；`NodeCnt` 是失败执行涉及的 compute node 数，由 `ExecutorWithRetry` 从 coordinator 的 `GetNodeCnt()` 保存并传入。
- `RecoveryHandler`：对外操作面，持有结果缓冲、策略列表、最大/当前恢复计数和启用开关。`handlers` 为 `pub(crate)` 仅供 crate 内测试或接线替换；其余状态受本模块封装。
- `NewRecoveryHandler(useAutoScaler, holderCap, enable, parent)`：创建 holder、注册唯一默认策略、把最大恢复次数设为 3、当前次数设为 0。
- `Enabled`、`CanHoldResult`、`HoldResult`、`NumHoldResp`、`PopFrontResp`、`ResetHolder`、`RecoveryCnt`：分别暴露开关、缓冲许可、插入、长度、FIFO 弹出、动态 holder 清理和恢复计数。
- `Recovery`：恢复编排入口；负责前置校验、计数、按序选择和错误返回。
- `HandlerImpl: Send + Sync`：策略扩展接口，分为 `chooseHandlerImpl` 和 `doRecovery` 两阶段。`Send + Sync` 约束允许策略对象安全地随所属执行对象跨线程边界使用，但本文件自身不创建线程。
- `MemLimitHandlerImpl`：默认内存限额策略；`newMemLimitHandlerImpl` 保存 AutoScaler 开关。
- `MppResultHolder`：私有 FIFO 与内存记账对象；`insert`、`reset` 和 `Drop::drop` 管理其动态状态与 tracker 挂接。
- `HolderBytesConsumed`：仅在 `cfg(test)` 下可见，用于直接验证 holder tracker 的记账值，不进入生产 API。

## 执行流程

构造阶段，`NewExecutorWithRetry` 根据 `MppRecoveryConfig` 调用 `NewRecoveryHandler`。后者用父 tracker 的 label 新建子 tracker并 `AttachTo(parent)`，预分配 `holderCap` 大小的 `VecDeque`，同时注册 `MemLimitHandlerImpl`。

正常取数阶段由 `ExecutorWithRetry::nextWithRecovery` 驱动：

1. 若 `Enabled()` 为假，跳过预拉和恢复。
2. 当 `CanHoldResult()` 为真时持续调用 coordinator 的 `Next`。
3. 得到响应时调用 `HoldResult`：先读取 `MemSize()`，再追加到队尾；队列长度达到容量时置 `cannotHold = true`，最后增加 tracker 用量。
4. 流结束则停止预拉。随后 `kv::Response::Next` 若发现 holder 非空，会通过 `PopFrontResp` 从队首返回；否则直接向 coordinator 取数。
5. `PopFrontResp` 在禁用或空队列时返回错误；成功时扣减该响应的内存用量并永久关闭本轮继续 hold 的许可，防止在已有行对外可见后重试造成重复行。

错误恢复阶段：

1. `nextWithRecovery` 把原始错误与当前 `node_count` 组装为 `RecoveryInfo`。
2. `Recovery` 依次检查启用状态、`RecoveryInfo`/`MPPErr` 非空、当前次数未达到 3；这些前置失败不增加计数。
3. 通过前置检查后先增加 `curRecoveryCnt`，再按注册顺序调用 `chooseHandlerImpl`。因此“无策略匹配”和“策略执行失败”同样消耗一次机会。
4. 默认策略要求 AutoScaler 开启且错误文本含 `Memory limit`。匹配后取得全局 topology fetcher，以 `RecoveryTypeMemLimit` 和 `NodeCnt` 调用 `RecoveryAndGetTopo`；返回的新拓扑在这里有意丢弃，因为 fetcher 会保存拓扑，后续重新派发会再次读取。
5. 策略成功后，调用方运行 `setupMPPCoordinator(true)` 重建执行轮次，再调用 `ResetHolder` 丢弃旧轮次缓冲。若恢复策略或重建失败，`executor_with_retry.rs` 对外保留最初的 MPP 错误。

## 数据与状态

`RecoveryHandler` 的状态可分为三类：配置态 `enable`/`maxRecoveryCnt`，跨重建累计态 `curRecoveryCnt`，以及 holder 动态态 `responses`/`cannotHold`/`memTracker`。`ResetHolder` 只清理第三类，不会重新启用 handler，也不会清零恢复计数；所以同一 `ExecutorWithRetry` 的多次重建共享“三次”预算。

`capacity` 表示响应条数而非字节数。内存风险由 tracker 记录而非作为停止条件；达到条数上限时 `cannotHold` 置真。调用约定要求在 `HoldResult` 前检查 `Enabled` 和 `CanHoldResult`，因为 `insert` 自身不拒绝越界调用。容量为 0 时 `CanHoldResult` 永远为假；若绕过约定直接插入，当前实现仍会入队。

`VecDeque` 保证到达顺序和弹出顺序一致。每次入队按 `MemSize()` 增加子 tracker，用 `PopFrontResp` 成功弹出时按同一值扣减。`reset` 清空响应并 `Detach` 子 tracker，从父 tracker 移除其累计占用；它随后不会在本文件内重新 attach。这个生命周期与 Go 的 `mppResultHolder.reset` 一致，扩展时不能假定 reset 后的新记账仍聚合到原父 tracker，除非同时明确调整 Go/Rust 契约和测试。

## 依赖与调用关系

上游主调用边由 RustCodeGraph 与源码共同确认：

- `NewExecutorWithRetry -> NewRecoveryHandler -> newMPPResultHolder`。
- `ExecutorWithRetry::nextWithRecovery -> Enabled / CanHoldResult / HoldResult / Recovery / ResetHolder`。
- `kv::Response for ExecutorWithRetry::Next -> NumHoldResp -> PopFrontResp`。
- `kv::Response for ExecutorWithRetry::Close -> ResetHolder`；`ExecutorWithRetry::Drop` 又调用幂等 `Close`。

下游依赖为：

- `kv::ResultSubset::MemSize`：响应内存记账的唯一大小来源。
- `astersql_util_memory::tracker::{NewTracker, Tracker}`：建立父子 tracker、消费/返还字节与解除挂接。
- `tiflashcompute::GetGlobalTopoFetcher`：读取进程级 `RwLock<Option<Arc<dyn TopoFetcher>>>` 保存的全局实现。
- `TopoFetcher::RecoveryAndGetTopo(RecoveryTypeMemLimit, NodeCnt)`：真实 AWS 实现校验恢复类型和节点数、调用 AutoScaler HTTP 接口并按时间戳更新拓扑缓存；Mock/Test 实现当前返回 `RecoveryAndGetTopo not implemented`。
- `astersql_errors::New`：把本地校验失败或 topology fetcher 错误统一成 `SharedError`。

## 错误处理与边界

- 禁用恢复时 `Recovery` 返回 `mpp err recovery is not enabled`；禁用状态优先于参数校验。
- `info` 为 `None` 或 `MPPErr` 为 `None` 时返回 `RecoveryInfo is nil or mppErr is nil`。
- 第四次恢复调用在计数仍为 3 时返回 `exceeds max recovery cnt: cur: 3, max: 3`，不再调用策略。
- 通过前置检查后，即使没有匹配策略，也先增加计数，再返回 `no handler to recovery this type of mpp err`。
- `PopFrontResp` 对“禁用”和“空队列”使用同一种错误格式，并包含当前 enable 与 size；它不会修改 holder 状态。
- 默认策略只做字符串包含判断，可能受上游错误文案或大小写变化影响。`useAutoScaler = false` 时，即使是内存限额文本也不匹配。
- 全局 fetcher 未初始化时返回显式错误；已初始化的 fetcher 错误会按字符串重新包装。测试 fetcher 的未实现错误会透传为 `RecoveryAndGetTopo not implemented`。
- `NodeCnt` 在本文件不校验；AWS fetcher 会拒绝 MemLimit 且节点数为 0，负数则会被格式化进请求参数。正常主链在 coordinator setup 时已要求节点数大于 0。

## 并发与资源生命周期

本文件没有启动任务、线程或 channel。`RecoveryHandler` 的可变操作要求 `&mut self`，因此 holder 队列和恢复计数由 Rust 独占借用串行修改；策略 trait 要求 `Send + Sync`。全局 topology fetcher 的读取与其内部拓扑缓存并发保护由 `pkg/util/tiflashcompute/topo_fetcher.rs` 的 `RwLock` 承担。

`newMPPResultHolder` 使用父 tracker 的裸指针挂接子 tracker，因此 `ExecutorWithRetry<'parent>` 通过 `PhantomData<&'parent mut Tracker>` 保证包装器不比父 tracker 活得更久。holder 的 `reset` 会 detach；即使调用方未显式 reset，`MppResultHolder::drop` 也会再次执行 `Detach`，避免在父 tracker 中留下悬挂记账。`ExecutorWithRetry::Close` 在关闭 coordinator 前先 `ResetHolder`，而其 `Drop` 会兜底调用 `Close`。

恢复期间，旧轮次响应在重建成功后由 `ResetHolder` 丢弃。只有尚未经过 `PopFrontResp` 对外暴露的响应才允许这样处理；一旦弹出过，`cannotHold` 禁止继续预拉，从状态机上阻止之后遇错再重试导致重复数据。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/executor/internal/mpp/recovery_handler.go`：字段、默认最大次数 3、先递增计数再选策略、`Memory limit` 文本匹配、FIFO holder、按 `MemSize` 记账、弹出后 `cannotHold`、reset 时清空并 detach 等语义均保持一致。Rust 用 `Option<&RecoveryInfo>`/`Option<SharedError>` 表达 Go 的 nil 参数与 nil error，用 `VecDeque` 替代 Go slice 头部切片，用 `Box<dyn ResultSubset>` 替代 `*mppResponse`，并用 `Result` 显式传播错误。

有两点实现层差异需要维护者知晓：Go 的 `GetGlobalTopoFetcher()` 直接返回接口并立即调用，Rust 返回 `Option`，因此额外提供“全局 fetcher 未初始化”的确定性错误；Rust 还增加 `Drop for MppResultHolder` 作为资源兜底。另有 Go 注释称 `ResetHolder` 会重置“recovery cnt”，但 Go 与 Rust 的实际代码都只调用 holder 的 `reset`，均不清零 `curRecoveryCnt`；应以实现及 Rust 测试固定的行为为准。

## 扩展指南

新增恢复类型时，优先实现新的 `HandlerImpl`，在 `NewRecoveryHandler` 中按明确优先级注册；若多个策略可能匹配同一错误，第一个匹配者会短路后续策略。应避免继续扩大脆弱的文本识别，若上游能提供结构化错误类型，应同步修改 Go/Rust 判断和兼容测试。

修改缓冲策略时需要同时检查 `CanHoldResult`、`MppResultHolder::insert`、`PopFrontResp`、`reset` 与 `ExecutorWithRetry::nextWithRecovery`，保持“不对外可见才可丢弃重试”的不变量。若把阈值从响应条数改为行数或字节数，需要确认 `ResultSubset` 能提供可靠指标，并评估内存峰值与预拉延迟。

新增或修改行为应放在独立测试文件，不要把测试嵌入生产文件。直接测试位置是 `recovery_handler_aster_unit_test.rs`；真实 `mppResponse` 的内存契约在 `mpp_response_aster_unit_test.rs`；重建 gather、丢弃旧缓冲和 FIFO 端到端行为在 `executor_with_retry_aster_unit_test.rs`。涉及 topology fetcher 时还应同步其独立测试，覆盖未初始化、恢复类型、节点数、HTTP 失败和缓存更新。兼容风险主要是 Go/Rust 错误文案及恢复计数时机漂移；性能风险主要是过大的 holder 容量、昂贵的 `MemSize` 计算和恢复 HTTP 请求。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录列出 16 个已索引 Go/Rust 文件。`explore` 确认 `NewExecutorWithRetry -> NewRecoveryHandler -> newMPPResultHolder`，并确认 `Recovery -> chooseHandlerImpl/doRecovery` 等局部调用。
- 生产源码：`pkg/executor/internal/mpp/recovery_handler.rs`（全部 25 个索引符号）、`executor_with_retry.rs`（构造、预拉恢复、Next、Close）、`lib.rs`（私有模块边界）。
- crate 配置：`pkg/executor/internal/mpp/Cargo.toml`（crate 名称、lib 入口、Go porting 元数据及直接依赖）。
- Go 对照：`pkg/executor/internal/mpp/recovery_handler.go` 和 `executor_with_retry.go`，核对字段、错误文案、计数顺序、holder 生命周期与调用位置。
- 下游实现：`pkg/util/tiflashcompute/topo_fetcher.rs`，核对全局 fetcher、AWS 恢复参数、缓存并发保护及 Mock/Test 未实现行为。
- 独立 Rust 测试：`recovery_handler_aster_unit_test.rs` 覆盖 FIFO、父 tracker 记账、禁用/空/零容量、策略顺序、失败尝试计数和真实全局测试 fetcher；`mpp_response_aster_unit_test.rs` 覆盖真实响应 `MemSize`；`executor_with_retry_aster_unit_test.rs` 覆盖 FIFO 消费、幂等关闭、重建 gather 与丢弃旧缓冲。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构通过固定 11 章节命令验证，并人工复核只新增本说明文档、未修改 Rust/Go/Cargo/`plan.md`。
