# `pkg/executor/staticrecordset/recordset.rs`

## 文件定位

本文件属于独立 crate `astersql-executor-staticrecordset`，crate 入口 `pkg/executor/staticrecordset/lib.rs` 公开 `recordset` 模块并重导出其符号。它在内部执行器接口 `astersql_executor_internal_exec::executor::Executor` 与可逐批消费的 `RecordSet` 抽象之间提供适配：持有一个已经构造好的执行器，并让调用方通过 `NewChunk`、`Next`、`Close` 管理结果读取生命周期。

Cargo 清单 `pkg/executor/staticrecordset/Cargo.toml` 将 `pkg/executor/internal/exec` 声明为唯一无条件依赖；其余规划器、会话游标和工具依赖仅在 Windows 目标下声明。根 facade `pkg/lib.rs` 会重导出整个 crate，`pkg/executor/Cargo.toml` 与 `pkg/session/Cargo.toml` 也声明了依赖，但代码搜索没有发现生产 Rust 代码直接调用本文件的 `New`。当前可确认的 Rust 调用者只有 `pkg/executor/staticrecordset/integration_test.rs`；Rust 执行主链在 `pkg/executor/adapter.rs` 内另有 `detachedRecordSet` 实现。因此，本文件目前是已实现并有独立测试覆盖的迁移模块，而不能据此声称它已取代 Rust 主链中的 detached 结果集。

## 核心职责

- `RecordSet` 定义静态结果集所需的最小协议：返回列元信息、拉取下一批、分配结果块、关闭资源，以及测试用执行器访问钩子。
- `staticRecordSet` 把 `Box<dyn Executor>` 包装为 `Box<dyn RecordSet>`，并用 `Option` 表达执行器是否仍然打开。
- `Next` 把可选来源上下文中的 RU 明细继承到本次调用上下文，再通过 `exec::Next` 执行一批；执行器或适配边界的 panic 被归一为 `Error::Panic`。
- `NewChunk` 根据顶层执行器的返回类型、初始容量和最大块大小，使用默认路径或调用方提供的 `ChunkAllocator` 创建 `Chunk`。
- `Close` 先取走执行器再调用 `exec::Close`，从而保证重复关闭不会再次触碰底层资源。

这些职责只覆盖执行器结果的静态消费，不负责创建、打开或 detach 执行器，也不负责语句级事务收尾、游标登记、慢查询记录或协议编码。

## 主要符号

- `pub type Error = exec::Error` 与 `pub type Result<T> = exec::Result<T>`：直接沿用内部执行器错误域，调用方无需进行额外错误转换。
- `ResultField { name: String }`：当前 Rust 结果列元信息仅保留列名。`Fields` 返回其完整克隆，而不是借用内部切片。
- `RUDetails { read_ru, write_ru }`：可共享的 RU 数值载体。当前文件只传播该对象，没有读取或累计两个数值。
- `RecordContext { exec_context, ru_details }`：一次 `Next` 的上下文；私有方法 `inherit` 克隆调用方上下文，并且仅在来源上下文含 `ru_details` 时用来源值覆盖克隆值。
- `ChunkAllocator: Send + Sync`：自定义块分配接口，`Alloc(fields, capacity, max_size)` 接收执行器的列类型及容量约束。
- `RecordSet: Send`：对结果集的对象安全接口。`GetExecutor4Test` 的默认实现返回 `None`，生产消费逻辑不应依赖此钩子。
- `staticRecordSet`：内部持有 `fields`、`executor`、`sql_text` 和 `source_ctx`。类型名虽公开，但字段均私有，正常构造入口是 `New`。
- `New(fields, executor, sql_text, source_ctx)`：把执行器放入 `Some` 并返回 trait object；它不调用 `Executor::Open`。
- `impl RecordSet for staticRecordSet`：实现 `Fields`、`Next`、`NewChunk`、`Close` 和 `GetExecutor4Test`。

文件没有模块级常量、条件编译项或异步函数。`sql_text` 当前只被保存，没有参与错误或日志生成。

## 执行流程

1. 上游先自行准备并打开执行器，再调用 `New`；构造函数保存字段、SQL 文本和可选来源上下文，并取得执行器所有权。
2. 调用方通过 `Fields` 获取列描述的副本。修改返回值不会改变结果集内部字段。
3. 调用方通过 `NewChunk(None)` 请求首块时，代码调用 `exec::NewFirstChunk`，该函数依据 `RetFieldTypes`、`InitCap`、`MaxChunkSize` 构造块；若提供分配器，则同样把这三个参数交给 `ChunkAllocator::Alloc`。
4. `Next(ctx, req)` 先生成执行上下文：没有 `source_ctx` 时克隆传入上下文；存在时调用 `ctx.inherit(source_ctx)`，只覆盖来源侧存在的 RU 明细。
5. 若执行器已经被 `Close` 取走，`Next` 返回 `Error::Other("record set is closed")`；否则调用 `exec::Next`。后者在执行器调用前后检查 SQL killer、注册 TopSQL 信息，并记录耗时与行数。
6. `exec::Next` 自身已捕获底层执行器 panic；本文件外层 `catch_unwind` 再保护适配闭包边界，任一未传播为普通错误的 panic 都映射为 `Error::Panic`。
7. `Close` 使用 `Option::take` 先令对象进入关闭态，再调用 `exec::Close`。即使底层关闭返回错误，后续 `Close` 仍返回 `Ok(())`，且后续 `Next` 明确报已关闭。

## 数据与状态

`fields` 和 `sql_text` 在构造后不再改变；其中 `fields` 通过克隆输出，`sql_text` 暂无读路径。`source_ctx` 同样保持不变，每次 `Next` 都从传入上下文重新克隆，再选择性继承来源 RU 的 `Arc`，不会反向修改任一原上下文。

唯一的生命周期状态是 `executor: Option<Box<dyn Executor>>`：`Some` 表示仍可拉取和分配块，`None` 表示已经执行过关闭流程。`NewChunk` 在关闭后返回空的 `Chunk::default()`，`GetExecutor4Test` 返回 `None`，而 `Next` 返回显式错误。该状态设计使关闭幂等，但也意味着第一次关闭错误不会在第二次关闭时重放。

`RUDetails` 通过 `Arc` 共享，不在本文件中更新。当前 `ExecContext` 也不含对 `RUDetails` 的连接，因此这里可确认的行为是“保存和继承句柄”，不能声称数值已被执行器计量逻辑消费。

## 依赖与调用关系

下游依赖集中在 `pkg/executor/internal/exec/executor.rs`：

- `Executor` 提供 `Next`、`Close`、`RetFieldTypes`、`InitCap`、`MaxChunkSize` 等能力。
- `exec::Next` 负责 killer 检查、TopSQL 注册、运行时统计和底层 panic 转换。
- `exec::NewFirstChunk` 根据执行器元数据创建默认结果块。
- `exec::Close` 捕获关闭 panic 并记录关闭耗时。

同 crate 的 `pkg/executor/staticrecordset/cursorrecordset.rs` 把任意 `Box<dyn RecordSet>` 包装为带游标生命周期的结果集，并转发本文件定义的接口；关闭时先关游标，再调用底层 `RecordSet::Close`。

RustCodeGraph 将 `pkg/executor/staticrecordset/integration_test.rs` 标为本文件的直接使用者，测试通过 `New` 构造结果集并覆盖各接口。代码搜索没有发现非测试 Rust 调用 `New`。Go 侧真实上游位于 `pkg/executor/adapter.go` 的 `recordSet.TryDetach`：detach 成功后调用 `staticrecordset.New`，使结果集可以脱离原会话继续消费。Rust 对应主链当前在 `pkg/executor/adapter.rs::recordSet::TryDetach` 中构造该文件自己的 `detachedRecordSet`，两套类型尚未接线合并。

## 错误处理与边界

- `Next` 原样传播 `exec::Next` 返回的 `Error`；关闭后调用则产生固定的 `Error::Other("record set is closed")`。
- 执行器 `Next` panic 被 `exec::Next` 捕获，本层仍设置第二层 unwind 边界，最终统一返回 `Error::Panic`。与 Go 版本不同，Rust 当前不会使用 `sql_text` 写 panic 日志，也不会保留 panic 细节。
- `Close` 原样返回第一次 `exec::Close` 的错误；因为执行器在调用前已被取走，关闭失败后也不能重试。重复关闭成功返回 `Ok(())`。
- `NewChunk` 在关闭后静默返回默认空块，而不是错误；调用方若需要区分“零列执行器”和“已关闭”，必须在关闭前维持自己的生命周期约束。
- `Fields` 在关闭后仍可用，因为字段元信息与执行器生命周期分离。
- `New` 不验证字段数与 `Executor::RetFieldTypes()` 是否一致，也不负责调用 `Open`。这两个前置条件必须由上游保证。
- `GetExecutor4Test` 明确是测试接口；游标包装器会在底层未暴露执行器时 panic，因此自定义 `RecordSet` 若要被该测试钩子访问，必须覆盖默认实现。

## 并发与资源生命周期

`RecordSet: Send` 允许所有权在线程之间转移，但 `Next`、`Close` 需要 `&mut self`，同一实例的消费和关闭必须串行化；本类型没有内部锁，也没有声明 `Sync`。`ChunkAllocator` 同时要求 `Send + Sync`，便于共享无状态或自行同步的分配器。`RecordContext` 与 RU 明细使用克隆和 `Arc` 共享，但本文件不创建线程、任务、通道或异步工作。

执行器所有权从 `New` 转移给 `staticRecordSet`。正常生命周期是构造后反复 `Next`，遇到空批次或调用方终止时显式 `Close`；`Close` 只调用底层一次。若调用方直接丢弃结果集而不调用 `Close`，Rust 会释放 `Box`，但 `Executor` trait 并未承诺 `Drop` 等价于 `Close`，所以需要显式关闭以保证执行器统计及外部资源收尾。带游标时应使用 `WrapRecordSetWithCursor`，其顺序不变量是游标先关闭、执行器后关闭。

## 与 Go 版本的对应关系

Rust 的 `staticRecordSet`、`New`、`Fields`、`Next`、`NewChunk`、`Close`、`GetExecutor4Test` 与 `pkg/executor/staticrecordset/recordset.go` 中同名结构和方法一一对应，核心委托模式一致：结果集拥有 detached 执行器，按 chunk 拉取并在结束时关闭。

已确认的语义差异如下：

- Go `ResultField` 使用完整的 `resolve.ResultField` 指针，Rust 本文件的 `ResultField` 只有 `name`，尚不具备数据库、表、类型等协议元数据。
- Go `New` 通过可变参数接受零个或多个来源 context，并只取第一个；Rust 使用显式 `Option<RecordContext>`。
- Go `Next` 从来源 context 同时继承 client RU details 和 RUV2 metrics；Rust 只覆盖自定义 `RUDetails` 句柄，且内部执行上下文目前不消费该字段。
- Go panic 恢复会将具体 panic 转为错误，并以 `sqlText` 记录带栈日志；Rust统一返回 `Error::Panic`，保存的 `sql_text` 未使用。
- Go `Fields` 返回内部指针切片，Rust 返回克隆值；Rust 因而隔离了调用方修改。
- Go `Close` 每次都调用 `exec.Close(s.executor)` 后置空，是否可重复调用取决于底层 nil 处理；Rust 明确定义为至多一次调用并保证幂等。

Go 集成测试 `pkg/executor/staticrecordset/integration_test.go` 验证了真实 SQL detach 后的字段和行、事务提交后继续读取、GC safe point 错误、显式事务不可 detach、游标释放与最小 start TS 生命周期。Rust 独立测试只用 `TestExecutor` 验证其中可由当前抽象表达的转发、错误、容量、panic 与关闭顺序；它没有覆盖真实存储、事务或 min-start-TS 行为。

## 扩展指南

- 若要把本 crate 接入 Rust 主链，应先比较并统一 `pkg/executor/adapter.rs::detachedRecordSet` 与本类型的接口、错误类型、字段模型和 `ExecutionContext`；不要同时保留两套行为逐渐漂移。接线点是 `recordSet::TryDetach`，对应测试应放在独立的 `pkg/executor/staticrecordset/integration_test.rs` 或 adapter 测试文件，不能内嵌到生产源文件。
- 扩充列元数据时应修改 `ResultField`、`Fields` 的所有使用者以及协议转换边界，并对照 Go `resolve.ResultField` 保留名称、类型、库表来源等兼容语义。返回克隆可能带来宽元数据和多列查询的分配成本，需要评估是否改为借用接口。
- 完善 RU 传播时应明确 `RecordContext.ru_details` 与 `ExecContext` 或统一 RUV2 指标对象的连接方式，并新增测试证明来源上下文覆盖规则及实际计量消费，不能只断言 `Arc` 被复制。
- 若要对齐 Go panic 诊断，应在不泄露敏感 SQL 的前提下让 `sql_text` 进入统一日志/错误设施，并测试 panic 到错误的转换及日志脱敏；当前字段闲置不代表可以无审查删除，因为它是 Go API 对齐参数。
- 修改块分配策略时同时覆盖默认 `exec::NewFirstChunk` 和自定义 `ChunkAllocator` 两条路径，保持列类型、初始容量、最大容量一致；关闭后的空块行为若改变，也应增加明确回归测试。
- 修改关闭语义时要保留“底层最多关闭一次”和游标先于结果集关闭的不变量，并覆盖底层关闭报错、重复关闭、关闭后 `Next`/`NewChunk`/测试钩子的行为。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件，目标目录中列出 `recordset.rs`、`cursorrecordset.rs`、`integration_test.rs` 等 8 个源文件；`node --file pkg/executor/staticrecordset/recordset.rs` 展示完整 156 行及其直接使用者。
- 主要图查询：`query StaticRecordSet` 定位 Rust/Go 两个 `staticRecordSet`；`node staticRecordSet` 确认 Rust `New` 实例化该结构；对通用名 `New`/`Next` 的全局 callers/callees 查询存在大量同名歧义，因此最终调用关系以目标文件节点、精确路径检索和相邻实现交叉核对。
- 已读 Rust 实现：`pkg/executor/staticrecordset/recordset.rs`、`pkg/executor/staticrecordset/lib.rs`、`pkg/executor/staticrecordset/cursorrecordset.rs`、`pkg/executor/internal/exec/executor.rs`、`pkg/executor/adapter.rs`。
- 已读清单与接线证据：`pkg/executor/staticrecordset/Cargo.toml`、`pkg/executor/Cargo.toml`、`pkg/session/Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`。
- 已读对照与测试：`pkg/executor/staticrecordset/recordset.go`、`pkg/executor/adapter.go`、`pkg/executor/staticrecordset/integration_test.rs`、`pkg/executor/staticrecordset/integration_test.go`；同目录没有 `recordset_test.rs`，Rust 回归集中在独立的 `integration_test.rs`。
- 文档只描述上述文件可证实的现状；未运行 Cargo，未验证真实 Rust 会话/存储集成，因为本任务是纯文档分析且计划明确禁止运行 Cargo。
