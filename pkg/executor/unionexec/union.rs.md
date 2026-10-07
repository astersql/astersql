# `pkg/executor/unionexec/union.rs`

## 文件定位

该文件属于独立 workspace crate `astersql-executor-unionexec`；crate 入口 `pkg/executor/unionexec/lib.rs` 公开 `union` 模块，根 `Cargo.toml` 同时把该 crate 列为 workspace member 并以 `facade_executor_unionexec` 命名接入根依赖表。当前 Rust 文件提供一套自包含的 Union 执行模型，而不是现有 TiDB Rust 执行器框架的实现：它定义自己的 `Executor` trait 和字符串二维数组 `Chunk`，并只被同 crate 的 `union_aster_unit_test.rs` 直接使用。仓库搜索没有发现生产 Rust 调用者。

同目录 `union.go` 是实际 Go SQL 执行链中的实现：`pkg/executor/builder.go::buildUnionAll` 根据 `PhysicalUnionAll` 构造它，`buildUnionScanFromReader` 还会递归改写其 children。因此，Rust 文件当前应理解为对 Go 并发与生命周期语义的局部移植/验证载体，不能描述为已接入完整 SQL 主链。

## 核心职责

- `UnionExec` 并发消费多个 child executor，把任意 worker 产生的 `Chunk` 按到达顺序交给调用方；它不排序、不去重，也不保证不同 child 间的输出次序。因此其行为接近 `UNION ALL` 的物理合并，而非 SQL `UNION DISTINCT` 的去重阶段。
- `open` 只重置本轮生命周期状态；线程、任务队列和结果通道延迟到第一次 `next` 时由 `initialize` 建立。
- 任一 child 的 `open`/`next` 错误或 panic 会作为字符串错误经结果通道返回，并设置共享取消位，促使其它 worker 停止。
- `close` 先取消并 join worker，再恢复 worker 暂时取走的 children，最后关闭所有已经被任务调度触达的 child；关闭多个 child 时保留第一个错误但继续关闭其余 child。
- `Drop::drop` 调用 `close`，为调用方遗漏显式关闭提供兜底。

以上职责直接来自 `UnionExec::{open,initialize,next,close}`、`child_call`、`send_result` 和 `Drop` 实现；其中“未接入完整 SQL 主链”由全仓 Rust 引用搜索及本 crate 的依赖声明共同确认。

## 主要符号

- `pub type Chunk = Vec<Vec<String>>`：极简批数据表示，外层是行、内层是字符串列；没有 Go `chunk.Chunk` 的类型信息、容量复用、列交换或行数/列数契约。
- `pub trait Executor: Send`：child 的最小生命周期接口，依次暴露 `open() -> Result<(), String>`、`next() -> Result<Option<Chunk>, String>`、`close() -> Result<(), String>`。`Send` 是 child 被移动到 worker 线程的必要边界。
- `WorkerResult { chunk, error }`：worker 到主线程的内部消息。正常数据只填 `chunk`，失败只填 `error`；通道关闭或两个字段都空在 `next` 中都解释为流结束。
- `Child = Box<dyn Executor>` 与 `ChildSlots = Arc<Mutex<Vec<Option<Child>>>>`：前者擦除具体 child 类型，后者让 worker 临时 `take` 独占 child，并在退出前放回原槽位。
- `send_result(...) -> bool`：以 `SyncSender::try_send` 非阻塞尝试发送；通道满时 `yield_now` 后重试，取消、接收端断开或发送成功时退出。`force=true` 用于错误消息，使设置取消位后仍能尝试上报首个错误。
- `child_call`：用 `catch_unwind(AssertUnwindSafe(...))` 包裹一次 child 调用，把 panic 统一转换成 `Err("union worker panicked")`。
- `UnionExec`：公开主体。`children` 保存非运行态所有权；`receiver`/`joins` 表示已初始化的一轮 worker；`cancel` 是跨线程取消位；`opened` 约束调用顺序；`concurrency` 是 worker 上限；`child_slots` 保存运行态 children；`max_opened_child_id` 记录已被调度触达的最大编号。
- `new` 默认把并发度设为 child 数量；`with_concurrency` 允许测试或调用方覆盖；`concurrency` 返回配置值；`restore_children` 把共享槽位移回 `self.children`。
- `Drop for UnionExec`：忽略 `close` 返回值，保证析构不会因关闭错误再次失败。

文件没有模块级业务常量、枚举、条件编译项或外部 `impl Executor for UnionExec`；`UnionExec` 通过固有方法提供生命周期 API。

## 执行流程

1. `UnionExec::new` 把每个 child 包成 `Some`，创建未取消的 `AtomicBool`，将 `max_opened_child_id` 置为 `-1`，并保持 `opened=false`。
2. `open` 拒绝重复打开；成功时清除取消位、重置最大触达编号并置 `opened=true`。此时不会调用 child。
3. 第一次 `next` 检查已打开状态后调用 `initialize`。后者把 children 整体移入 `ChildSlots`，创建共享 FIFO child-id 队列和容量为 `max(concurrency, 1)` 的同步结果通道，再启动 `min(concurrency, child_count)` 个线程。发送端原件随后被丢弃，使所有 worker 发送端销毁后接收端能够观察结束。
4. 每个 worker 在未取消时从 FIFO 弹出一个 child id，先更新 `max_opened_child_id`，再从对应槽位 `take` 出 child。child 所有权在一个线程中独占，不会被并发调用。
5. worker 通过 `child_call` 执行 `child.open()`。打开失败时设置取消位并以 `force=true` 发送错误；打开成功则循环调用 `child.next()`：`Some(chunk)` 作为普通结果发送，`None` 表示该 child 耗尽，错误则设置取消并强制上报。
6. worker 无论正常耗尽还是遇到受控错误，都会把 child 放回原槽。若取消位已设置则退出，否则继续领取下一个 child。
7. 调用方的 `next` 阻塞在 `receiver.recv()`：收到 chunk 就原样返回；收到 error 就再次设置取消并返回错误；所有发送端断开则返回 `Ok(None)`。
8. `close` 设置取消，先丢弃 receiver 以解除可能因满通道重试的发送方，再逐一 join 线程；之后 `restore_children` 恢复所有权，并只对 `0..=max_opened_child_id` 范围内存在的 child 调用 `close`。最后置 `opened=false` 并返回第一个关闭错误。
9. 显式 `close` 后可以再次 `open`；同目录测试 `union_can_be_reopened_after_close` 验证两轮都产生同一 chunk 且每轮各有一次 child open/close。

## 数据与状态

- 生命周期不变量是：非运行态的 child 位于 `children`；初始化后 `children` 为空而所有槽位位于 `child_slots`；worker 处理某个 child 时对应槽位暂时为 `None`；join 完成后 `restore_children` 才恢复稳定所有权。
- `receiver.is_some()` 是“本轮已经初始化”的标志。`initialize` 可重复调用但第二次直接成功；`close` 通过 `receiver.take()` 清除该标志，为下一轮 reopen 做准备。
- `opened` 只描述外部生命周期。未 `open` 调用 `next` 返回 `"union executor is not open"`；重复 `open` 返回 `"union executor is already open"`。
- `cancel` 使用 Acquire/Release 顺序在线程间发布停止信号。它不是结果完成标志；完成由所有 `SyncSender` 被销毁后 `recv` 返回断开来表示。
- `child_ids` 的 `VecDeque` 保证 child 被领取时编号递增，但 worker 执行速度不同，结果顺序仍不确定。
- `max_opened_child_id` 在领取任务后、调用 child `open` 前更新，所以即使 `open` 失败，该 child 也属于 `close` 必须触达的范围。同目录 `failed_open_child_is_closed` 明确验证这一点。
- `concurrency=0` 时不会创建 worker，原发送端被丢弃，`next` 直接观察到结束；空 children 同理。当前实现不把这两种配置判为错误。

## 依赖与调用关系

Rust 下游全部来自标准库：`std::thread`/`JoinHandle` 管理 worker，`std::sync::mpsc::sync_channel` 传递结果，`Arc<Mutex<_>>` 共享任务队列、child 槽和最大触达编号，`AtomicBool` 传播取消，`catch_unwind` 隔离 child panic，`VecDeque` 分发 child id。`pkg/executor/unionexec/Cargo.toml` 没有启用这些实现所需的非 Windows 外部依赖；其中 `cfg(windows)` 下声明的若干 AsterSQL crate 当前实现也没有引用。

RustCodeGraph 对文件内关键边的结果为：`with_concurrency -> new`，`next -> initialize`，`initialize -> Executor::open / Executor::next / send_result / child_call`，`close -> child_call / restore_children / Executor::close`，`Drop::drop -> close`。图查询对常见方法名存在大量同名候选，因此应用主链结论另以精确文件引用搜索核对。

上游方面，Rust 只有 `pkg/executor/unionexec/union_aster_unit_test.rs` 构造并调用 `UnionExec`；没有生产 Rust caller。Go 主链则是 `pkg/executor/builder.go::buildUnionAll -> unionexec.UnionExec`，其 child 来自 `PhysicalUnionAll::Children()`，并发度来自 session 的 `UnionConcurrency()`；`buildUnionScanFromReader` 与 `pkg/executor/join/hash_join_v1.go::aggExecutorTreeInputEmpty` 也会按具体类型识别 Go `UnionExec`。

## 错误处理与边界

- child 返回的 `String` 错误不包装上下文，原样传到调用方；panic 被折叠为固定字符串，panic payload 和栈不会保留。相比 Go 的 `errors.Trace` 与日志记录，Rust 可诊断性更弱。
- 错误 worker 先写 `cancel=true`，再用 `force=true` 发送错误，避免普通取消检查吞掉本 worker 的错误；但多个 worker 竞态失败时可能各自强制发送，调用方通常只看到先收到的一个。
- `send_result` 在通道满时采用 `try_send + yield_now` 的忙重试，没有阻塞等待或退避。`close` 先丢弃 receiver，使发送方得到 `Disconnected` 后退出，避免 join 永久等待满通道。
- `Mutex::lock` 均以 `expect` 处理 poisoned mutex；这类 panic 没有统一在整个 worker 闭包外层恢复。`child_call` 只覆盖 child 的 `open`、`next`、`close` 本身。
- `close` 忽略线程 `join` 的 panic 结果，但仍继续恢复并关闭 children；如果 worker 在 child 放回槽位前因非 child panic 退出，对应槽位可能保持 `None`，该 child 随线程栈析构而丢失，无法再执行显式 `close`。
- 调用方在未耗尽结果前调用 `close` 会取消剩余工作，已排队但尚未被 `next` 读取的 chunk 会随 receiver 丢弃。这是关闭语义，不是数据完整性保证。
- 关闭 child 时会尝试所有“编号不大于最大触达编号且槽位仍存在”的 child，并只返回首个错误；`close_returns_first_error_and_closes_all_reached_children` 覆盖该不变量。

## 并发与资源生命周期

并发上限为 `min(concurrency, child_count)`。worker 共享 child-id 队列，但每个 child 在被 `take` 后仅归一个线程所有；因此不要求 child 为 `Sync`，只要求 `Send`。结果通道具有有界背压，容量使用配置并发度的至少 1；生产者超过容量后让出时间片重试。

取消路径依赖三项配合：`cancel` 阻止领取/继续读取，`receiver.take()` 断开发送目标，`joins.drain(..).join()` 等待线程完成。只有 join 后 `close` 才读取最大触达编号、恢复槽位并调用 child close，避免与 worker 同时访问 child。`Drop` 复用同一路径，因此正常析构不会遗留可 join 的 worker。

与 Go 的资源池不同，Rust 每次 `next` 从 child 接收一个拥有所有权的 `Chunk`，不把空 chunk 归还到 per-worker resource pool，也不进行 `SwapColumns`；所以当前 Rust 版本没有 Go 版的 chunk 复用协议和列数一致性检查。它也没有 context/finished channel/failpoint，取消只能由内部原子位和接收端断开表达。

## 与 Go 版本的对应关系

共同点：两版都延迟到首次 `Next/next` 启动并发拉取；都限制同时处理的 child 数；都把 child 打开/拉取错误传到主线程；都在停止后等待 worker；都记录已触达 child 范围，关闭所有相关 child 并保留第一个关闭错误；panic 都转成可返回错误。

关键差异如下：

- Go `UnionExec` 嵌入 `exec.BaseExecutor` 并实现统一 `exec.Executor`，Rust 定义独立 trait，尚未与 `pkg/executor/internal/exec` 的 Rust API 接线。
- Go 构建器的入口是 `PhysicalUnionAll`，Rust 没有 planner/builder 入口，也没有 SQL schema、session context、runtime stats 或 executor id。
- Go 每个 worker 有容量 1 的 `resourcePool` 并复用真实列式 `chunk.Chunk`；Rust 传递拥有所有权的 `Vec<Vec<String>>`，没有复用和列类型校验。
- Go 通过 `finished` channel、`stopFetchData`、wait group 和 result-pool closure 协调；Rust 通过 `AtomicBool`、receiver disconnect、`JoinHandle` 和 sender drop 协调。
- Go worker panic 会记录 recover 值与栈；Rust 固定返回 `"union worker panicked"`。Go 还有 `pauseUnionExecResultPuller`、`issue21441` failpoint，Rust 没有等价注入点。
- Go 文件按配置并发数创建资源池，并将其裁剪到 child 数；Rust 保留原始 `concurrency()` 返回值，仅在启动 worker 时取 `min`，通道容量仍使用原配置（至少 1）。

因此，源码前半段被注释掉的 Go 风格伪移植只能作为映射说明；真正可运行行为以第 267 行之后的 Rust 实现为准，不能把注释中的资源池、列数检查或 failpoint 当成当前已支持能力。

## 扩展指南

- 若要接入完整 Rust SQL 主链，首要修改点不是单纯扩展本地 trait，而是让 `UnionExec` 使用 `pkg/executor/internal/exec` 的 Rust executor/chunk/context 契约，并在 Rust planner/executor builder 的 `PhysicalUnionAll` 路径构造它；同时应移除或迁移本文件自定义 `Chunk`/`Executor`，避免两套接口长期并存。
- 若保留当前模型并增加行为，应优先在 `initialize` 集中维护 worker 建立与 child 所有权不变量，在 `send_result` 维护“错误可越过取消位上报、关闭能解除背压”的保证，在 `close` 维护“先 join、后恢复、再关闭全部已触达 child”的顺序。
- 增加顺序保证、去重或限制/取消语义时，必须明确这是否仍是物理 `UNION ALL`；跨 child 的全局有序或 distinct 会引入缓存、比较/哈希、内存限制与性能风险，不应悄悄塞入当前无序汇聚循环。
- 改变错误模型时，应保留 panic 隔离并增加上下文；若扩大 catch 范围，还要验证 child 不会因提前 unwind 而跳过放回槽位和 close。
- Rust 测试应继续放在独立的 `pkg/executor/unionexec/union_aster_unit_test.rs`，不要内嵌到生产文件。至少同步覆盖：未 open/重复 open、零 child/零并发、多个 worker 竞态错误、接收端提前关闭、mutex poison 或线程 panic、close 后 reopen、所有已触达 child 都关闭且只返回首错。
- 若向 Go 语义靠拢，还应对照 `pkg/executor/executor_failpoint_test.go` 的关闭等待与 shutdown panic 场景，以及 `pkg/executor/test/executor/executor_test.go::TestTwiceCloseUnionExec` 的 reopen/错误打开/无泄漏场景。兼容风险集中在错误先后顺序、关闭覆盖范围和 chunk schema；性能风险集中在忙重试、每批分配及通道容量。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/executor/unionexec` 列出 `lib.rs`、`union.rs`、`union.go`、`union_aster_unit_test.rs`。
- RustCodeGraph 源码/符号查询：完整检查 `pkg/executor/unionexec/union.rs` 的 555 行；查询 `UnionExec`、`send_result`、`child_call`，并检查 `new/open/initialize/next/close/drop` 的 callers/callees。可确认文件内边 `next -> initialize`、`initialize -> child_call/send_result/child.open/child.next`、`close -> restore_children/child_call/child.close`、`drop -> close`。
- crate 与接线：读取 `pkg/executor/unionexec/Cargo.toml`、`lib.rs`、`BUILD.bazel` 及根 `Cargo.toml` 的 workspace member/facade 条目；全仓 `rg` 确认 Rust 生产代码没有构造该 `UnionExec`。
- Go 对照：读取 `pkg/executor/unionexec/union.go` 全文；读取 `pkg/executor/builder.go::buildUnionAll`、`buildUnionScanFromReader` 和 `pkg/executor/join/hash_join_v1.go::aggExecutorTreeInputEmpty`，确认 Go 版的实际构建、递归改写与类型识别入口。
- 独立测试：读取 `pkg/executor/unionexec/union_aster_unit_test.rs` 全文，覆盖 worker panic、open 失败仍 close、close 后 reopen、关闭全部已触达 child 并返回首错；另核对 Go `pkg/executor/executor_failpoint_test.go::{TestUnionExecCloseWaitsForWorkers,TestUnionExecCloseReturnsAfterWorkerPanicDuringShutdown}` 与 `pkg/executor/test/executor/executor_test.go::TestTwiceCloseUnionExec`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核本文明确区分“可运行 Rust 实现”“注释中的 Go 映射”和“Go 生产接线”，未把未接线能力写成现状。
