# `pkg/dxf/operator/wrapper.rs`

## 文件定位

本文件位于 `astersql-dxf-operator` crate 的常用算子封装层，由 [`lib.rs`](lib.rs) 以公开模块 `wrapper` 导出。它把 [`compose.rs`](compose.rs) 的通道接线接口和 [`operator.rs`](operator.rs) 的异步变换算子组合成三类便捷组件：有限输入源 `SimpleDataSource<T>`、变换包装器 `SimpleOperator<T, R>`、末端汇聚器 `SimpleSink<R>`。这些组件可作为 `Box<dyn Operator>` 放入 [`pipeline.rs`](pipeline.rs) 的 `AsyncPipeline`。

当前仓库搜索到的 Rust 直接使用者是独立测试 [`pipeline_test.rs`](pipeline_test.rs) 与 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)；未发现生产 Rust 模块调用这三个构造函数。因此它目前是公开、可复用的 DXF 基础门面，也是 Go 实现迁移语义的测试载体，而不能据此宣称已经接入某条生产任务主链。

## 核心职责

- `SimpleDataSource<T>` 保存一批有限输入；`Open` 后在线程中逐条发送，结束或出错时关闭输出通道，`Close` 等待线程并返回其结果。
- `SimpleSink<R>` 在线程中持续接收上游数据并调用用户提供的 `drainer`；通道正常关闭时成功结束，Context 取消时返回 Context 中记录的算子错误。
- `SimpleOperator<T, R>` 是 `AsyncOperator<T, R>` 的薄包装，统一提供 `simpleOperator(...)` 名称，并透传生命周期、上下游通道和 worker 数量调节能力。
- `context_error` 与 `join_result` 集中处理取消原因提取、重复关闭和线程 panic 到 `workerpool::Error` 的转换。

三类算子不负责创建整条管道或连接通道：调用方必须先用 [`Compose`](compose.rs) 接线，再由 `AsyncPipeline::Execute`/`Close` 管理生命周期。

## 主要符号

- `context_error(&workerpool::Context) -> Error`：优先返回 `Context::OperatorErr()`；没有已登记错误时返回 `"context canceled"`，供 Source/Sink 的取消和通道异常路径复用。
- `join_result(&mut Option<JoinHandle<Result<(), Error>>>)`：用 `take` 保证每个句柄最多 join 一次；无线程时成功，线程 panic 时返回 `"operator thread panicked"`，线程函数的普通错误原样传播。
- `SimpleDataSource<T>` / `NewSimpleDataSource`：公开输入源及构造函数。`T` 必须实现 `TaskMayPanic + Send + 'static`，以便后续送入 workerpool 并跨线程移动。字段 `inputs` 和 `target` 均为 `Option`，会在第一次 `Open` 时转移所有权。
- `SimpleSink<R>` / `newSimpleSink`：公开类型和公开构造函数，持有 `Arc<dyn Fn(R) + Send + Sync>`；`R: Send + 'static`。名称虽为公开 API，仍沿用 Go 风格的小写函数名并由文件级 `non_snake_case` 许可。
- `SimpleOperator<T, R>` / `newSimpleOperator`：内部只保存 `AsyncOperator<T, R>`。构造函数以固定名称 `"simple"` 调用 `NewAsyncOperatorWithTransform`，并接收初始并发度。
- `Operator` 实现：Source/Sink 自行创建线程；Transform 将 `Open`、`Close` 委托给内部 workerpool 算子。三者的 `String` 分别输出带输入类型名的数据源、固定的 `simpleSink`、以及包装后的异步算子描述。
- `WithSource`/`WithSink` 实现：保存或透传 `SimpleDataChannel`，使 [`Compose`](compose.rs) 能用同一个容量为 0 的通道连接相邻算子。
- `TunableOperator for SimpleOperator`：`TuneWorkerPoolSize` 与 `GetWorkerPoolSize` 完整委托给内部 `AsyncOperator`；`Operator::as_tunable_operator(_mut)` 返回自身以支持运行期类型访问。

## 执行流程

典型流程由测试中的 `Source → Transform → Sink` 展示：调用三个构造函数，依次 `Compose(&mut source, &mut transform)`、`Compose(&mut transform, &mut sink)`，再将它们装入 `NewAsyncPipeline`。

1. `Compose` 创建容量为 0 的 `SimpleDataChannel`，把克隆句柄分别注入上游 `SetSink` 和下游 `SetSource`。容量为 0 意味着每次发送必须等待接收方，天然产生背压。
2. `AsyncPipeline::Execute` 按列表顺序调用 `Open`。Source 的 `Open` 取走 `inputs` 和输出通道并启动发送线程；Transform 的 `Open` 启动 workerpool；Sink 的 `Open` 取走输入通道并启动接收线程。
3. Source 发送线程先创建取消监控线程，然后遍历输入。每项发送前检查取消；发送失败也按 Context 错误结束。循环结束后无论成功失败都调用 `target.Finish()`，标记监控结束、唤醒并 join 监控线程。
4. `AsyncOperator` 的 worker 从输入通道取任务，执行 `transform: Fn(T) -> R`，再向输出通道发送结果；这一真实处理循环位于 [`operator.rs`](operator.rs)，本文件不复制它。
5. Sink 的接收线程同样创建取消监控线程，然后循环检查 Context、阻塞接收并同步调用 `drainer`。正常关闭且未取消返回成功；取消导致 `Finish` 打断接收并返回错误。
6. `AsyncPipeline::Close` 按列表顺序调用每个算子的 `Close`。Source/Sink join 自己的线程，Transform 释放 workerpool；管道即使遇到错误仍继续关闭后续算子，并返回第一个关闭错误。

## 数据与状态

`SimpleDataSource` 的 `inputs`、`target` 和 `handle` 表示尚未消费的输入、尚未绑定/取走的输出通道和正在运行的线程。第一次成功进入 `Open` 后，前两者会被 `take` 置空；因此实例是一次性数据源，不支持关闭后用原数据重新启动。若缺少 Sink，代码会先取走 `inputs`，随后返回错误；再次接线并 `Open` 时输入已经为空，这是现有实现的重要状态边界。

`SimpleSink` 的 `source` 同样在首次 `Open` 时被取走，`drainer` 则放在 `Arc` 中克隆给接收线程。`SimpleOperator` 不复制状态，worker 数量、Context、输入接收端和结果发送端都由内部 `AsyncOperator`/`WorkerPool` 持有。

Source 与 Sink 每次运行还各自创建一个 `Arc<AtomicBool>`。主工作线程在退出前以 `SeqCst` 写入 `true`；监控线程轮询该标志，并在 Context 取消时关闭对应通道。该原子值只协调监控线程的终止，不代表算子的公开运行状态。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：本 crate 直接依赖本地 `astersql-resourcemanager-pool-workerpool`；本文件经 `crate::workerpool` 使用它的 `Context`、`Error`、`TaskMayPanic`。`crossbeam-channel` 是 crate 依赖，但本文件通过 workerpool `Channel` 与 [`compose.rs`](compose.rs) 的 `SimpleDataChannel` 间接使用通道能力。

下游关系如下：

- `newSimpleOperator` → `NewAsyncOperatorWithTransform` → `WorkerPool::NewWorkerPool` / `NewAsyncOperator`；RustCodeGraph 明确记录了这条调用边。
- Source/Sink → `SimpleDataChannel::Channel` 的 `send`/`recv` 与 `Finish`；`Finish` 最终调用底层 channel 的 `close`。
- `SimpleOperator` → 内部 `AsyncOperator` 的 `Open`、`Close`、`SetSource`、`SetSink`、`TuneWorkerPoolSize` 和 `GetWorkerPoolSize`。
- 测试调用方 [`pipeline_test.rs`](pipeline_test.rs) 组装五段词频流水线；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 组装三段数字变换流水线并验证取消错误传播。

RustCodeGraph 对构造函数的精确 `callers/callees` 命令未输出完整边，因此调用者范围又用仓库级 `rg` 复核；没有发现上述两个测试之外的 Rust 调用者。

## 错误处理与边界

- Source 未接 Sink、Sink 未接 Source 时，`Open` 分别返回 `"simple data source has no sink"`、`"simple sink has no source"`；必须在 Execute 前完成 Compose。
- Source/Sink 的 `handle` 已存在时再次 `Open` 会返回 already-open 错误。`Close` 会取走 handle；第二次 `Close` 是幂等成功，但这不等于实例可安全重新 Open，因为输入和通道已被消费。
- 取消时优先传播 `Context::OperatorErr`，因此下游 transform 通过 `Context::OnError` 登记的具体错误能在 Source/Sink 的 `Close` 中出现；没有具体错误才退化为 `context canceled`。
- Source 的底层 `send` 返回 false 表示通道已经关闭，也走 Context 错误路径；若此时 Context 未保存具体错误，最终信息仍是通用取消错误，不能区分正常的接收端提前关闭。
- 工作线程 panic 会由 `join_result` 转为普通 `Error`；监控线程 panic 的 join 结果被忽略。`drainer` 自身无 `Result` 返回值，其 panic 只能通过工作线程 panic 路径在 `Close` 暴露。
- `newSimpleOperator` 的 transform 也不返回 `Result`；业务失败需按 workerpool 约定调用共享 Context 的 `OnError`，或通过任务 panic 恢复契约处理，不能直接从闭包返回错误。

## 并发与资源生命周期

Source 和 Sink 的 `Open` 都立即返回，仅把实际工作交给一个主线程；主线程内部再启动一个取消监控线程。监控以 1 ms `park_timeout` 轮询 Context，正常结束时主线程设置原子标志并 `unpark`，随后 join 监控线程，避免正常路径遗留后台线程。

取消监控关闭的是克隆的共享通道：Source 侧关闭输出以解除可能阻塞的无缓冲 `send`，Sink 侧关闭输入以解除可能阻塞的 `recv`。`Finish` 可由正常工作路径和取消监控重复调用，依赖底层通道关闭操作可重复执行。Source 无论发送成功还是失败都会关闭输出；Sink 不负责关闭上游，只在取消时主动关闭输入。

`drainer` 在唯一的 Sink 工作线程中顺序调用；但上游 `SimpleOperator` 可有多个 worker，因此结果到达顺序不保证与输入顺序相同。[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 因此先排序结果再断言。若闭包捕获共享可变状态，调用方仍需按整个管道的访问方式选择 `Mutex` 等同步手段；类型约束 `Send + Sync` 只保证可跨线程共享，不自动保证业务不变量。

资源回收依赖显式 `Close`：本文件没有 `Drop` 实现。调用方若只 `Open` 而不 `Close`，就不会通过这里 join Source/Sink 工作线程；标准入口应交由 `AsyncPipeline::Close` 管理。

## 与 Go 版本的对应关系

直接对照文件是 [`wrapper.go`](wrapper.go)。两版都包含有限切片/向量数据源、带 drainer 的 Sink、嵌入/包装 `AsyncOperator` 的 Transform，并共享 workerpool Context 以使任一算子报错时整条流水线退出。`String` 的外层格式和 `newSimpleOperator` 固定使用名称 `simple` 也保持一致；[`pipeline_test.go`](pipeline_test.go) 与 Rust [`pipeline_test.rs`](pipeline_test.rs) 都验证同一条词频流水线、拓扑字符串和错误注入分支。

实现机制存在语言适配差异：Go 用 `errgroup.Group` 管理 goroutine，并在 `select` 中同时等待 channel 与 `ctx.Done()`；Rust 的 workerpool Channel API没有在这里提供同构 `select`，所以 Source/Sink 用工作线程加取消监控线程，取消时主动 `Finish` 通道以打断阻塞操作。Go `simpleSink`/`simpleOperator` 是包内类型，Rust 对应类型为 `pub`；Rust 还显式实现 `TunableOperator` 向下访问接口，以替代 Go 的运行期类型断言。

Rust `SimpleDataSource::String` 使用 `std::any::type_name::<T>()`，通常产生完整 Rust 类型路径；Go 使用 `%T`。因此测试应分别断言各语言的本地类型名，不能逐字跨语言复用期望值。

## 扩展指南

- 新增通用同步变换时优先复用 `newSimpleOperator`，把并发、通道绑定和 worker 调节留给 `AsyncOperator`；若需要每 worker 初始化/关闭状态或 transform 可直接返回错误，应扩展 [`operator.rs`](operator.rs) 的 worker 构造路径，而不是在本文件另建线程池。
- 修改 Source/Sink 生命周期时要保持三个不变量：完成时关闭输出、取消能解除阻塞的 send/recv、`Close` 能稳定回收并传播线程结果。尤其要为“缺少接线后重试”“关闭后重开”“接收端提前关闭”和监控线程异常明确设计语义。
- 新增算子包装器时实现 `Operator`，并按数据方向实现 `WithSource`/`WithSink`；需要被 `AsyncPipeline::GetReaderAndWriter` 识别时，还要实现 `TunableOperator` 并覆写 `as_tunable_operator(_mut)`。
- 测试逻辑必须保持在独立 Rust 测试文件，不能内嵌到 `wrapper.rs`。管道行为优先扩展 [`pipeline_test.rs`](pipeline_test.rs)，Rust 特有的取消、panic、重复生命周期和迁移边界优先扩展 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，并与 [`pipeline_test.go`](pipeline_test.go) / [`wrapper.go`](wrapper.go) 的原始意图核对。
- 并发度或通道策略变化可能改变背压、结果顺序、内存占用和取消延迟；公开构造函数或 trait 边界变化会影响 crate 外调用者。扩展前应再次运行 RustCodeGraph/`rg` 查找外部使用点，不能仅依据当前两个测试调用者推断长期兼容范围。

## 验证依据

- RustCodeGraph `status`：索引覆盖 7,032 个 Rust 文件；`node --file pkg/dxf/operator/wrapper.rs` 读取目标文件 327 行并列出 26 个符号。
- RustCodeGraph `query`：确认 `NewSimpleDataSource`、`newSimpleSink`、`newSimpleOperator` 和 `SimpleDataSource` 的定义位置与公开签名。
- RustCodeGraph `node NewAsyncOperatorWithTransform`：确认 `newSimpleOperator` 是其 Rust 调用者，并确认其继续调用 `newAsyncWorkerCtor`、`NewAsyncOperator`。
- RustCodeGraph `node Compose`、`node SimpleDataChannel`、目标文件读取：确认容量为 0 的接线通道、显式 `Finish`、Source/Sink 的线程与取消路径。
- 已读源码与边界文件：[`wrapper.rs`](wrapper.rs)、[`compose.rs`](compose.rs)、[`operator.rs`](operator.rs)、[`pipeline.rs`](pipeline.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。
- 已读 Go 对照与测试：[`wrapper.go`](wrapper.go)、[`pipeline_test.go`](pipeline_test.go)；已读独立 Rust 测试：[`pipeline_test.rs`](pipeline_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。仓库级 `rg` 复核 Rust 构造函数调用点，未发现两个测试模块之外的生产 Rust 调用。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证恰有 11 个固定二级章节，并人工复核每项行为结论均能回指上述源码、图查询或测试。
