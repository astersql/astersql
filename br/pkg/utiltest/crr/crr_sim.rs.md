# `br/pkg/utiltest/crr/crr_sim.rs` 逻辑说明

## 文件定位

`crr_sim.rs` 属于 `astersql-br-pkg-utiltest-crr` library crate。crate 由 [`br/pkg/utiltest/crr/Cargo.toml`](./Cargo.toml) 定义，`lib.rs` 通过 `pub mod crr_sim` 纳入模块并用 `pub use crr_sim::*` 再导出公开符号。它不是 BR 生产环境的跨区域复制实现，而是本地测试夹具：用对象存储抽象和有界事件通道模拟“上游对象产生新版本，复制工人把当前内容搬到下游”的最小链路。

完整测试链中的装配入口是 `harness.rs::NewLocalTestHarnessWithTestContext`：它创建容量为 1024 的事件通道，把发送端交给 `CRRUpstreamStorage`，把接收端交给 `CRRWorker`，再由 `TestHarness::PullMessages` 和 `TestHarness::Replicate` 驱动复制。这里没有自动启动后台线程；“worker”只是需要调用者显式推进的确定性模拟器。

## 核心职责

- `CRRUpstreamStorage` 装饰一个 `Arc<dyn Storage>`。`WriteFile`、`Rename` 和流式 `Create` 返回的 writer 在底层变更成功后发送 `NewVersionCreatedEvent`；读取、删除、遍历、预签名和关闭等操作直接透传。
- `CRRWorker` 从 `Receiver<NewVersionCreatedEvent>` 非阻塞拉取路径到本地 `buffer`，再从上游读取该路径的当前内容并写到下游。
- `ReplicateBuffered` 从 `buffer` 尾部取元素，因此普通复制是“最新收到的事件优先”；`ReplicateBufferedRandom` 先通过调用者注入的随机函数做 Fisher–Yates 洗牌，再复用普通复制。
- `new_event_channel` 固定建立容量为 1024 的同步通道，为上游装饰器和 worker 提供背压边界。

该文件只模拟对象事件与复制顺序，不实现真实 CRR 协议、持久事件队列、去重、版本号、重试策略或独立调度循环。文件头模块说明和 Go 对照 `crr_sim.go` 都明确了这一测试替身定位。

## 主要符号

- `NewVersionCreatedEvent { Path }`：公开事件数据，只记录对象路径，不携带内容或版本；worker 复制时重新读取上游当前值。
- `CRRWorker`：持有 `upstream`、`downstream`、可选接收端 `messages` 和待处理 `buffer`。字段不公开，只能通过构造器和方法驱动。
- `NewCRRWorker`：接收两个共享存储和一个可转换为 `Option<Receiver<_>>` 的参数；允许传 `None` 表达 Go 的 nil channel。
- `CRRWorker::PullMessages`：用 `try_recv` 拉取当前已到达事件。`limit <= 0` 表示尽量清空当前可读消息；空路径被丢弃且不计数；发送端断开后把 `messages` 置为 `None`。
- `CRRWorker::BufferedMessages`：克隆并返回缓冲快照，调用者不能借此修改内部队列。
- `CRRWorker::ReplicateBuffered`：把限制归一化到当前缓冲长度，从尾部逐项调用 `replicateOne`，只在成功后删除该项并增加成功计数。
- `CRRWorker::ReplicateBufferedRandom`：要求非空 `intN`，对整个缓冲原地洗牌，然后委托 `ReplicateBuffered`；注入函数使测试可复现。
- `CRRWorker::replicateOne`：执行 `FileExists → ReadFile → WriteFile`。对象已经不存在，或存在性检查后读取遇到 `is_not_exist`，均作为成功跳过。
- `CRRUpstreamStorage` / `NewCRRUpstreamStorage`：底层存储装饰器及构造器；可选 `events` 用于保留 Go nil channel 边界。
- `emitNewVersionEvent`：检查取消状态并用 `try_send` 发事件；通道满时每 1 ms 重试，通道缺失、取消或断开均返回带对象名的错误。
- `CrrEventWriter`：内部 writer 装饰器。`Write` 透传；`Close` 先提交底层 writer，再发送完成事件。
- `new_event_channel`：返回 `SyncSender`/`Receiver` 对，容量固定为 1024。

文件没有条件编译项、模块级常量或自定义 trait；它实现的是 `stubs.rs::Storage` 和 `stubs.rs::Writer`。

## 执行流程

1. `harness.rs::NewLocalTestHarnessWithTestContext` 调用 `new_event_channel`，用同一个上游底层存储分别构造 `CRRWorker` 和 `CRRUpstreamStorage`。
2. 测试或 `FlushSim` 经装饰后的上游执行 `WriteFile`、`Rename`，或完成 `Create` writer 的 `Close`。底层操作先成功，随后 `emitNewVersionEvent` 把新路径发送到通道。
3. 调用者执行 `PullMessages(limit)`。该方法不等待未来事件，只把调用时已经可读的有效路径追加到 `buffer`。
4. 调用者执行 `ReplicateBuffered(ctx, limit)`，或先洗牌的 `ReplicateBufferedRandom`。普通路径从尾到头处理，因此 `[a, b]` 在限制为 1 时先复制 `b`。
5. `replicateOne` 确认上游对象仍存在，读取其当前字节，再用同名路径写入下游。成功后才从 `buffer` 移除该事件；失败项和更旧项都保留以便调用者重试。

RustCodeGraph 的静态边确认 `ReplicateBuffered → replicateOne`、`ReplicateBufferedRandom → ReplicateBuffered`；直接引用检索确认装配调用位于 `harness.rs`，核心独立断言位于 `parity_test.rs`。Go 集成测试还通过 `TestHarness` 将相同流程接入 checkpoint 计算与恢复可读性检查。

## 数据与状态

`NewVersionCreatedEvent` 的唯一状态是 `Path: String`。同一路径可以出现多次，代码不去重；因为内容不随事件保存，较旧事件执行时也会读取路径的最新可见内容。这正适合测试“新版本通知”，但不能表达真实对象版本之间的历史差异。

`CRRWorker::buffer` 是到达顺序的 `Vec`。`PullMessages` 向尾部追加，`ReplicateBuffered` 也从尾部弹出，所以缓冲行为是 LIFO。`limit <= 0` 和大于缓冲长度的限制都等价于处理全部现有元素；正限制只处理对应数量。复制错误发生前已成功的尾部元素已被移除，当前失败元素仍在缓冲；调用方收到的是错误而不是部分成功数量，因此若需要观察部分进度，应在错误后重新查看 `BufferedMessages`。

`messages: Option<Receiver<_>>` 区分有效、nil 和已断开状态。断开只会在队列中已有消息被耗尽并由 `try_recv` 返回 `Disconnected` 时被记录；之后拉取恒为 0。`CRRUpstreamStorage::events` 也为 `Option`，但 nil 是写后报错的边界，不会阻止底层对象先被修改。

随机路径对整个缓冲原地洗牌，即使 `limit` 只复制一部分，剩余项顺序也会改变。`intN(i + 1)` 必须遵守随机源契约并返回 `0..i+1`；超界值会在 `Vec::swap` 处 panic，函数本身不校验。

## 依赖与调用关系

本文件的直接 Rust 依赖只有标准库的 `Arc`、`std::sync::mpsc`、线程休眠和 `Duration`，以及本 crate `stubs.rs` 中的 `Context`、`Error`、`Result`、`Storage`、`Reader`、`Writer` 和选项类型。`Cargo.toml` 没有为本模块声明 feature；该 crate 是 `kind = "library"` 的 Go 移植包。清单中的其他 BR、`rand`、`serde` 依赖由 crate 的相邻模块共享，本文件没有直接引用它们。

上游调用边：

- `harness.rs::NewLocalTestHarnessWithTestContext` 调用 `new_event_channel`、`NewCRRWorker` 和 `NewCRRUpstreamStorage`；`TestHarness::PullMessages`、`TestHarness::Replicate` 分别转调 worker 方法。
- `parity_test.rs::go_parity_end_to_end`（测试主体所在区域）直接构造装饰器与 worker，覆盖写入通知、普通/随机复制和边界错误。
- Go 侧 `checkpoint_calculator_test.go`、`integration_test.go`、`randomized_integration_test.go`、`service_test.go` 通过 Go `TestHarness` 使用相同公开表面，展示该模拟器服务于 CRR checkpoint 与恢复完整性场景，而不是独立业务入口。

下游调用边：

- `ReplicateBuffered` 调用私有 `replicateOne`；后者调用上游 `FileExists`、`ReadFile` 和下游 `WriteFile`。
- `ReplicateBufferedRandom` 调用注入随机函数后调用 `ReplicateBuffered`。
- `Storage::WriteFile`、`Storage::Rename` 与 `CrrEventWriter::Close` 调用 `emitNewVersionEvent`；其他 `Storage` 方法只透传底层实现。

## 错误处理与边界

- `replicateOne` 为存在性检查、上游读取和下游写入错误分别增加 `check/read/write ...` 上下文。不存在不是错误：初次检查为 false，或检查与读取之间发生删除并返回 `is_not_exist`，都会成功跳过并让事件出队。
- 复制项只有在 `replicateOne` 成功后才从缓冲删除。因此下游写失败不会丢事件；但上游不存在被定义为已消费，之后对象若重新出现，需要新的事件才能复制。
- `ReplicateBufferedRandom(None)` 返回 `random replicate buffered: nil intN`。随机函数返回超范围索引会 panic，属于调用契约而非可恢复错误。
- 事件通道为 `None`、上下文已取消或接收端断开时，`emitNewVersionEvent` 返回错误；满通道不立即失败，而是轮询取消并重试。
- `WriteFile`、`Rename` 和 writer `Close` 都先完成底层变更再发送事件。若事件发送失败，调用者会看到失败，但对象写入/重命名/提交已经发生，没有回滚。这一“副作用成功、通知失败”状态是扩展错误处理时必须保留或显式重新定义的兼容边界。
- `PullMessages` 是非阻塞快照式拉取；返回 0 只表示此刻没有有效可读事件、通道为 nil/断开，不能证明未来不会再有事件。
- 删除操作不发事件，读取和元数据操作也不发事件。新增会产生版本的存储方法时，不能依赖默认透传自动通知。

## 并发与资源生命周期

`Storage: Send + Sync` 且由 `Arc` 共享，所以底层存储可跨线程使用；`Writer: Send` 允许 writer 被移动。但 `CRRWorker` 的变更方法要求 `&mut self`，本文件不在内部加锁或生成线程，调用者负责串行驱动或在外部同步。`Receiver` 由单个 worker 独占；发送端可以克隆，所有发送端都释放后 receiver 才进入断开状态。

事件通道容量 1024。上游操作遇满通道时以 1 ms 休眠循环形成背压，并在每轮检查 `Context`；这不是高吞吐生产队列。若调用方不拉取且不取消，产生第 1025 个未消费事件的操作可能无限等待。Go 实现用阻塞 `select` 同时等待发送或 `ctx.Done()`，Rust 用 `try_send + sleep` 近似这一语义。

`CrrEventWriter` 在 `Close` 前不通知，确保下游不会因半成品写入收到事件；若底层 `Close` 失败也不会发事件。类型没有 `Drop` 补偿，丢弃未关闭 writer 不会通知。`CRRUpstreamStorage::Close` 只转发到底层存储，不主动关闭事件通道；通道生命周期由 sender 所有权和 clone 数量决定。

## 与 Go 版本的对应关系

Rust 文件逐项对应同目录 `crr_sim.go`：事件、worker 的四个公开方法、`replicateOne`、上游存储装饰器、事件发送函数和 writer 装饰器的顺序与主要分支一致。以下是实现语言带来的显式差异：

- Go 用 nil channel/函数；Rust 用 `Option<Receiver<_>>`、`Option<SyncSender<_>>` 和 `Option<&mut dyn FnMut>` 表达相同公共边界。`parity_test.rs` 验证 nil receiver 返回 0、nil 随机函数报错、nil sender 在底层写入后报错。
- Go channel 由 harness 以 `make(chan ..., 1024)` 创建；Rust 将容量封装在 `new_event_channel`。Go `select` 发送与取消，Rust 使用带 1 ms 休眠的非阻塞重试。
- Go worker 构造器返回指针；Rust 返回拥有值，并通过 `Arc<dyn Storage>` 共享存储。Go slice 快照通过 copy，Rust 通过 `Vec::clone`，都不暴露内部缓冲。
- Go 用 `errors.Is(err, os.ErrNotExist)`；Rust 依赖本地 `Error::is_not_exist` 标志。两者都容忍存在性检查后的竞态删除。
- Rust crate 使用本地 `stubs.rs::Storage` 子集，而 Go 实现真实实现 `storeapi.Storage`。因此该 Rust 类型是移植测试表面，不应当作完整对象存储适配器使用。

Rust 的 `parity_test.rs` 覆盖主流程和多数公开边界；Go 的 CRR checkpoint 集成测试进一步证明部分复制会导致恢复校验失败、全量复制后 checkpoint 才可安全推进、随机复制可反复清空队列。当前 Rust 独立测试没有直接断言 `Create/Close`、`Rename`、通道满时取消以及复制错误后缓冲保留，这些是对齐扩展时应优先补的独立测试。

## 扩展指南

- 新增会创建可复制版本的 `Storage` 操作时，应在底层操作成功后调用 `emitNewVersionEvent`，并明确通知失败后的副作用是否仍保留；同步扩展 `stubs.rs::Storage`、Go `crr_sim.go` 及独立 Rust 测试，不能只改装饰器一侧。
- 修改复制顺序或 limit 语义时，重点修改 `PullMessages`、`ReplicateBuffered`/`ReplicateBufferedRandom`，并同步 `parity_test.rs` 的 `[a,b]` 最新优先断言，以及 Go checkpoint 集成测试对部分复制和全量复制的预期。
- 若引入去重或真实版本标识，需要扩展 `NewVersionCreatedEvent`，并解决“事件只存路径、执行时读取最新值”的现有语义；这会影响兼容性和内存占用，也可能改变旧事件删除竞态的处理。
- 若要改为后台 worker，应在 `harness.rs` 中定义启动、取消、join 和关闭顺序，避免 sender/receiver 或存储的生命周期悬挂；当前确定性手动推进是大量测试可控制时序的基础。
- 若改变 1024 容量或满通道策略，应补充可取消背压测试并评估忙等/休眠延迟、内存峰值和 Go 行为一致性。不要把这个测试替身直接扩张成生产队列。
- 回归测试应优先放在独立的 `parity_test.rs` 或同目录新增 `*_test.rs` 文件；遵守仓库要求，不把测试内嵌到 `crr_sim.rs`。跨模块语义还应参考 Go 的 `br/pkg/stream/crr/internal/checkpoint/integration_test.go` 与 `randomized_integration_test.go`。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/utiltest/crr` 确认目标与相邻模块已索引；`node --file br/pkg/utiltest/crr/crr_sim.rs` 读取目标 345 行并报告直接使用文件 `harness.rs`、`parity_test.rs`。
- RustCodeGraph 调用查询：`callees ReplicateBuffered` 得到 `replicateOne`；`callees ReplicateBufferedRandom` 得到 `ReplicateBuffered`；`callees replicateOne` 显示存在性检查、读取和写入调用；`callees emitNewVersionEvent` 显示事件构造边。
- crate 与模块证据：`br/pkg/utiltest/crr/Cargo.toml`、`br/pkg/utiltest/crr/lib.rs`。
- Rust 直接调用与测试证据：`br/pkg/utiltest/crr/harness.rs`、`br/pkg/utiltest/crr/parity_test.rs`、`br/pkg/utiltest/crr/harness_test.rs`；其中 `parity_test.rs` 直接验证写入通知、最新优先、随机复制、空路径及 nil 边界。
- Go 对照与场景证据：`br/pkg/utiltest/crr/crr_sim.go`、`br/pkg/utiltest/crr/harness.go`、`br/pkg/stream/crr/internal/checkpoint/checkpoint_calculator_test.go`、`integration_test.go`、`randomized_integration_test.go`、`br/pkg/stream/crr/service/service_test.go`。
- 接口与并发约束证据：`br/pkg/utiltest/crr/stubs.rs` 中 `Context`、`Writer: Send` 和 `Storage: Send + Sync` 的定义。
- 本任务只新增说明文档；未运行 Cargo 或代码测试。最终结构以任务指定命令校验，并通过差异审阅确认没有修改 Rust、Go、Cargo 或只读 `plan.md`。
