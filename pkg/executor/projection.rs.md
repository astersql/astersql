# `pkg/executor/projection.rs`

源码：[projection.rs](projection.rs)；Go 对照：[projection.go](projection.go)。

## 文件定位

本文件位于 `astersql-executor` crate（`pkg/executor/Cargo.toml`），并由 `pkg/executor/lib.rs` 以 `pub mod projection` 公开。它包含两层能力：

- `ProjectRows` 是已经接入 Rust 运行时的轻量列投影函数。`pkg/executor/physical_plan_runtime.rs::ExecuteLimitRows` 在 Limit 的内联投影路径调用它；`pkg/executor/benchmark_test.rs` 还用它比较 TopN/Limit 的内联投影与外层投影结果。
- `ProjectionExec<B>` 是以 `ProjectionBackend` 抽象宿主能力的完整 Chunk 投影执行器，复刻 Go `ProjectionExec` 的 Open/Next/Close、串行执行和 Fetcher/Worker 并行流水线。当前仓库搜索不到 `ProjectionBackend` 的具体实现，也没有构建器构造这个泛型类型；`pkg/executor/builder.rs::buildProjection` 当前经 `ExecutorBuildDependencies::build_parallel_unary_executor` 接线，而规范物理计划的实际执行还由 `physical_plan_runtime.rs` 和 `typed_projection.rs` 承担。因此该泛型执行器是公开的移植边界，但不能据此宣称已进入当前 SQL 主链。

文件直接使用 `astersql-executor-sortexec::{Row, SortError}`；Cargo 清单将该依赖映射到同目录子 crate `pkg/executor/sortexec`。本文件没有 feature 条件编译项。

## 核心职责

1. `ProjectRows` 对一组 `Row` 按列下标重排/裁剪，保持输入行顺序、列顺序和重复列；任一列越界即返回 `SortError`。
2. `ProjectionBackend` 把基执行器、子节点取数、Chunk 操作、表达式求值、内存统计、运行时统计、trace 与 panic 转换隔离为可注入边界。
3. `ProjectionExec` 在 `numWorkers <= 0` 时串行拉取并求值；在 worker 数大于零且表达式可向量化时惰性启动并行流水线。
4. 并行路径用固定容量通道和预分配 Chunk 池限制在途资源，通过 `parentReqRows` 把父节点本次需求传给后台 Fetcher。
5. Open/执行/Close 全程维护 Chunk 内存差额，Close 负责停止并 join 后台线程、排空对象池并登记并发度。

## 主要符号

- `CHANNEL_POLL_INTERVAL: Duration`：`readProjection` 的 2ms 接收超时，使阻塞读取能周期检查全局结束标志。
- `ProjectRows(rows, columns)`：当前已接线的行级列投影。每个下标通过 `row.0.get(index).cloned()` 读取，因此不改变源行且允许重复选择同一列。
- `projectionInput<C>`：Fetcher/Worker 间的输入资源，保存 Chunk 和固定的 `targetWorker` 编号。
- `projectionOutput<C, E>`：共享输出 Chunk 与容量为 1 的完成通道。自定义 `Clone` 只克隆 `Arc` 和 sender，不复制 Chunk 或 receiver；`signal`、`try_signal`、`wait` 分别服务正常完成、panic 恢复和主线程等待。
- `projectionExecutorContext<M, R, V>` 与 `newProjectionExecutorContext`：在构造时快照语句内存 tracker、运行时统计集合、求值上下文和向量化开关，供 worker 安全共享。
- `ProjectionBackend`：完整执行器所需的生产边界 trait。关联类型均带有与跨线程用途相匹配的 `Send`/`Sync`/`'static` 约束。
- `ProjectionParallelState<B>`：持有 finish 原子标志、全局结果接收端、输入/输出池、逐 worker 接收端以及所有线程句柄；只在 `prepare` 后存在。
- `ProjectionExec<B>`：执行器本体。重要状态包括 `numWorkers`、串行 `childResult`、原子 `parentReqRows`、`memTracker`、惰性启动标志 `prepared` 和可选并行状态 `parallel`。
- `projectionInputFetcher<B>`：唯一调用 `backend.child_next` 的并行 Fetcher；把输入与输出资源配对，并按 `targetWorker` 派发。
- `projectionWorker<B>`：调用 `backend.evaluate`，完成后通知 `projectionOutput` 并把输入 Chunk 归还池。
- `recoveryProjection`：将 panic 转换为 backend 错误，尽力非阻塞通知当前输出，并记录 panic。
- `readProjection` / `sendProjection`：感知 finish 的通道收发帮助函数；前者超时轮询，后者通道满时 `yield_now` 重试。

## 执行流程

`ProjectRows` 的流程很短：逐行遍历，按 `columns` 顺序克隆字段，组装新 `Row`；任何字段下标无效时整次调用返回错误。`ExecuteLimitRows` 可在 Limit 前调用它，benchmark 则在 Limit/TopN 后用它模拟外层 Projection。

完整 `ProjectionExec` 生命周期如下：

1. `Open` 先调用 `backend.base_open`，成功后进入 `open`。后者重置 `prepared` 和 `parentReqRows`，复用或新建 tracker 并挂到语句 tracker；若 evaluator 不可向量化则把 worker 数降为 0。串行模式预分配一个子 Chunk 并记账。
2. `Next` 先对请求 Chunk 扩容并重置，然后按 `isUnparallelExec` 分派。
3. 串行 `unParallelExecute` 把请求的 required rows 下传到 `childResult`，调用一次 `child_next`，记录 Chunk 前后内存差；空批表示结束，非空批交给 `evaluate` 写入请求 Chunk。
4. 并行 `parallelExecute` 原子发布本次 required rows。第一次调用会执行 `prepare`：为每个 worker 建立容量为 1 的专用输入/输出通道，向共享输入/输出池各预填一个 Chunk，再启动一个 Fetcher 线程和 N 个 Worker 线程。
5. Fetcher 的 `run_loop` 从两个池各取一个资源，先把输出句柄送到全局结果通道，再按 `parentReqRows` 设置输入 Chunk 并拉取子节点。子节点错误或 EOF 时直接通过 output 完成通道通知主线程并退出；否则把配对资源发给目标 worker。
6. Worker 的 `run_loop` 取得成对资源，锁住输出 Chunk，调用 `evaluate`，更新输入与输出 Chunk 的合计内存差并发出完成信号。成功后输入 Chunk 回池；求值错误则通知后退出。
7. 主线程收到全局 output 后等待完成信号；成功时把输出 Chunk 的列交换到请求 Chunk，并将清空后的 output 归还输出池。全局通道断开代表 EOF。
8. `Close` 对串行 Chunk 解除记账；对已启动的并行状态设置 finish、join 全部线程、排空所有共享及逐 worker 通道。最后按实际模式登记并发度，并调用 `base_close`。

## 数据与状态

- `prepared` 只表示并行流水线是否已经惰性创建；`open` 会将其重置。`parallel` 应与它同步：准备后为 `Some`，Close 时被取走。
- `parentReqRows: Arc<AtomicI64>` 由主线程以 Release 写、Fetcher 以 Acquire 读。并行预取意味着连续请求可能出现流水线时序效应；Go 的 `TestProjectionParallelRequiredRows` 因 goroutine 调度不稳定而显式跳过，文档不能承诺每批严格对应最新一次写入。
- 每个 worker 初始拥有一个输入 Chunk 和一个输出 Chunk。容量为 worker 数的全局池与容量为 1 的专用通道形成有界背压，避免无界积累 Chunk。
- `projectionOutput.chk` 和其 receiver 用 `Arc<Mutex<_>>` 共享；完成通道容量为 1，保证正常求值结果或终止状态有一个待消费槽位。
- `memTracker` 记录预分配、子取数、求值、列交换以及排空资源造成的内存变化。代码用 `saturating_sub` 计算“新减旧”，因此单次增长路径不会产生负数；明确释放时直接传入负的 Chunk 使用量。
- `calculateNoDelay` 在结构中保留以对齐 Go 字段，但本文件没有读取它；当前行为不能由该字段推断。

## 依赖与调用关系

上游直接证据：

- `pkg/executor/lib.rs` 公开模块。
- `pkg/executor/physical_plan_runtime.rs` 导入 `ProjectRows`；`ExecuteLimitRows` 的 `child_columns: Some` 路径调用它。该文件自身的普通 `PhysicalProjection` 分支目前使用私有 `project_row`，并不构造本文件的 `ProjectionExec`。
- `pkg/executor/benchmark_test.rs::{top_n_benchmark_case, run_limit_go_case}` 调用 `ProjectRows`，验证外层投影与内联投影结果一致。

`ProjectionExec` 内部主要调用边为：`Open -> open`，`Next -> unParallelExecute | parallelExecute`，`parallelExecute -> prepare`，`prepare -> projectionInputFetcher::run + projectionWorker::run`，两个 `run -> run_loop`，异常时 `run -> recoveryProjection`，`Close -> drainInputCh + drainOutputCh + base_close`。RustCodeGraph 还确认 `child_next` 仅由串行路径和 Fetcher 调用，`evaluate` 仅由串行路径和 Worker 调用。

下游能力全部经 `ProjectionBackend`：基执行器生命周期、Chunk 分配与交换、子执行器取数、表达式求值、内存/统计、trace 和 panic 处理。唯一具体数据依赖是 `sortexec::Row/SortError`，仅服务 `ProjectRows`。

需要区分三个同名概念：`pkg/executor/detach.rs::ProjectionExec` 是执行上下文脱离模型；`pkg/executor/typed_projection.rs::TypedProjection` 是当前 builder 使用的 typed 执行器；它们都不是本文件的泛型 `ProjectionExec<B>`。

## 错误处理与边界

- `ProjectRows` 在第一个越界列返回包含列下标和行宽的 `SortError`；空输入得到空输出，空列集合为每个输入行生成空 `Row`。
- `Open` 原样传播 `base_open` 和 `open` 错误；当前 `open` 自身只做状态初始化，backend 的 tracker 操作为无错误返回接口。
- 串行路径先完成内存差记账，再传播 `child_next` 错误；EOF 以零行 Chunk 表示并正常返回。
- 并行主线程把全局输出通道断开视为 EOF；完成通道意外关闭、Mutex poisoned 或输出池关闭则通过 `backend.error` 生成显式错误。
- Fetcher 在子节点错误时用当前 output 通知主线程；Worker 在求值错误时同样通知后退出。线程 panic 被 `catch_unwind` 捕获，并由 `recoveryProjection` 尽力通知当前 output；如果 panic 发生在尚未取得 output 时，只能记录日志，不能向等待方发送对应错误。
- `projectionOutput::signal` 忽略发送失败；`try_signal` 还会忽略“已满”。这是关闭/竞态下的容错选择，扩展时不能假设每个错误一定能被消费者观察。
- `prepare` 中池初始化使用 `expect`，依赖“准备阶段接收端仍存活”的内部不变量；tracker 和串行 Chunk 未初始化时也用 `expect`，调用者必须遵守 Open → Next → Close 生命周期。

## 并发与资源生命周期

并行模式共有一个 Fetcher、N 个 Worker 和调用 `Next` 的主线程。Fetcher 串行访问子执行器，避免多个 worker 并发调用 child；worker 只做表达式求值。输入的 `targetWorker` 在池初始化时固定，因此归还后继续绑定同一 worker。

`finish: AtomicBool` 是统一取消信号。`readProjection` 用 2ms timeout 避免永久阻塞，`sendProjection` 通过非阻塞发送、`yield_now` 和 finish 检查避免有界通道满时无法关闭。Close 必须先置 finish 再 join，否则线程可能等待资源；join 后才排空通道，防止仍在运行的线程与内存释放竞争。

资源所有权以通道传递为主：输入 Chunk 在池、Fetcher、指定 Worker 间移动；输出对象在输出池、Fetcher、全局通道、Worker、主线程间移动，Chunk 本身再由 `Arc<Mutex<_>>` 保护。主线程等待 worker 的完成信号后才交换列，避免读取未完成结果。

trace guard 覆盖 Fetcher/Worker 各自整个 `run` 生命周期。线程 `join` 的 panic 结果在 Close 中被忽略，因为 panic 已预期由线程内部 `catch_unwind` 转换和记录；若 panic 发生在 recovery 逻辑之外，Close 仍不会把 join 错误返回给调用者。

## 与 Go 版本的对应关系

Rust 的 `projectionInput`、`projectionOutput`、`projectionExecutorContext`、`ProjectionExec`、Fetcher、Worker、`recoveryProjection` 和 `readProjection` 均直接对应 `pkg/executor/projection.go` 的同名结构与流程。串行 required-rows 下传、不可向量化时强制串行、并行对象池、内存差记账、Close 等待后台工作和运行时并发度登记都保留了 Go 意图。

实现层差异如下：

- Go 直接依赖 `BaseExecutorV2`、`chunk.Chunk`、`EvaluatorSuite` 和 session context；Rust 用 `ProjectionBackend` 关联类型抽象这些生产组件，但当前没有具体实现接线。
- Go 用关闭 `finishCh` 和 `select` 取消；Rust 用 `AtomicBool`、超时接收及重试发送模拟可取消通道。Go goroutine/WaitGroup 对应 Rust thread/JoinHandle。
- Go 的 failpoint `mockProjectionExecBaseExecutorOpenReturnedError` 和 `ConsumeRandomPanic` 没有在本文件直接实现；Rust 把 panic 转换与日志交给 backend。`pkg/executor/test/issuetest/executor_issue_test.rs` 复刻了 Open 错误 SQL 场景，但它通过测试设施命名故障，并非本泛型执行器的直接单元测试。
- Go builder 还依据估算行数、DML/锁状态等决定并发度；Rust 本文件只接受既定 `numWorkers` 并检查 evaluator 是否可向量化。并发度的上游选择属于 builder/backend 边界。
- `ProjectRows` 是 Rust 额外的简化行容器入口，不对应 Go 文件里的独立函数；它只支持已解析列下标，不执行任意表达式。

Go 测试证据包括 `executor_required_rows_test.go::TestProjectionSerialRequiredRows`、被跳过的 `TestProjectionParallelRequiredRows`、`test/issuetest/executor_issue_test.go::TestIssue24210` 以及 `benchmark_test.go` 的外层 Projection 基准路径。Rust 直接覆盖目前集中在 `benchmark_test.rs` 的 `ProjectRows` 成功路径；没有与本文件同名的独立 `projection_test.rs`，也没有泛型并行执行器的具体 backend 测试。

## 扩展指南

- 要让完整 `ProjectionExec<B>` 进入生产链，应在独立源文件实现具体 `ProjectionBackend`，并在 builder 的 Projection 分支构造它；不要把 backend 实现或测试内嵌进本文件。需要同时验证 BaseExecutor 生命周期、表达式/Chunk 语义以及当前 `TypedProjection` 的替换或共存边界。
- 扩展 `ProjectRows` 支持任意表达式会改变其“只按已解析下标复制字段”的契约；更适合复用 expression evaluator，而不是在此函数中逐步复制 SQL 表达式语义。
- 修改并行调度时必须保持：子执行器只有 Fetcher 调用、output 在发布给主线程后最终一定收到终止信号、输入/输出 Chunk 有界、Close 能唤醒所有阻塞点、每个已记账 Chunk 最终解除记账。
- 修改 required-rows 时应同步独立 Rust 测试，至少覆盖串行多次非均匀请求、并行 1/N worker、EOF 与不同调度顺序；Go 对照矩阵在 `pkg/executor/executor_required_rows_test.go`。调度敏感断言应避免固定 sleep。
- 修改错误或 panic 行为时应覆盖 child 错误、evaluate 错误、output 尚未取得时 panic、完成通道断开、锁 poisoning 和 Close 竞态，并对照 Go `TestIssue24210`。性能风险主要来自轮询间隔、`yield_now` 忙重试、字段克隆和过度锁竞争。
- Rust 单元测试应放在同目录独立 `*_test.rs` 并由 `lib.rs` 的 `#[cfg(test)] mod ...` 接入；不要放回 `projection.rs`。若只扩展 `ProjectRows`，可在独立测试中覆盖空列、重复列、列重排和越界错误。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；查询时目标文件完整可用。
- RustCodeGraph `node --file pkg/executor/projection.rs`：读取了 1–792 行，核对全部常量、类型、trait、函数、impl 和无条件编译事实。
- RustCodeGraph 精确查询：`ProjectionExec`、`ProjectRows`、`projectionInputFetcher`、`projectionWorker`；调用流确认 `ProjectRows -> physical_plan_runtime/benchmark_test`，以及 `Next -> 串行/并行`、`parallelExecute -> prepare`、`prepare -> Fetcher/Worker::run`、`run -> run_loop/recoveryProjection` 等边。
- crate/模块证据：`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"`、`astersql-executor-sortexec` 路径依赖和 `pkg/executor/lib.rs::pub mod projection`。
- Rust 调用/测试证据：`pkg/executor/physical_plan_runtime.rs`、`pkg/executor/builder.rs`、`pkg/executor/benchmark_test.rs`、`pkg/executor/test/issuetest/executor_issue_test.rs`；仓库搜索未找到 `ProjectionBackend` 实现或泛型 `ProjectionExec` 的生产构造点。
- Go 对照/测试证据：`pkg/executor/projection.go`、`pkg/executor/executor_required_rows_test.go`、`pkg/executor/test/issuetest/executor_issue_test.go`、`pkg/executor/benchmark_test.go`、`pkg/executor/pkg_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；完成前使用任务指定命令检查目标存在且固定二级标题恰好为 11，并人工复核源文件链接、接线边界和扩展风险。
