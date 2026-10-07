# `pkg/dxf/framework/scheduler/mock/storage_adapter.rs`

## 文件定位

本文件属于独立 crate `astersql-dxf-framework-scheduler-mock`，由同目录的 `lib.rs` 通过 `#[path = "storage_adapter.rs"]` 私有装入，再公开导出 `Error`、`SessionExecutor`、`TaskHandle`、`execute` 和 `sessionctx`。它不是 DXF 存储层的实现，而是给 `scheduler_mock.rs` 中 `MockExtension<H>` 提供一组最小、可独立编译的 Go 风格存储接口和数据类型。

这层适配存在于测试边界：`scheduler_mock.rs::MockExtension` 用 `H: storage::TaskHandle + Send + Sync + 'static` 约束回调句柄，但该 mock crate 的 `Cargo.toml` 只依赖 `anyhow`、`mockall` 和 proto crate，并不依赖 `astersql-dxf-framework-storage`。因此，本文件在 crate 内复刻了 mock 所需的接口形状，避免把完整存储 crate 引入生成 mock。生产 Rust 调度器使用的是 `pkg/dxf/framework/scheduler/interface.rs::TaskHandle`，真实存储契约则位于 `pkg/dxf/framework/storage/task_table.rs`；它们与本文件中的同名 trait 是不同的 Rust 类型。

## 核心职责

本文件有三项聚焦职责：

1. 以 `Error = anyhow::Error`、空的 `sessionctx::Context` 和 crate 根部的 `Context = ()`（定义于同目录 `lib.rs`）构成无需真实会话池的回调环境。
2. 以 `execute::Progress` 和 `execute::SubtaskSummary` 保留扩展回调可能读取的前序子任务汇总数据形状，包括原子计数器和进度历史。
3. 声明 `SessionExecutor` 与继承它的 `TaskHandle`，让生成的 `MockExtension<H>` 能在 `OnPrepare`、`OnNextSubtasksBatch` 和 `OnDone` 等回调签名中约束句柄类型。

文件没有 SQL、事务、持久化、序列化、汇总计算或 mock 期望录制逻辑。会话/事务如何创建、前序子任务如何查询、错误如何产生，全部由 trait 实现者决定；当前直接实现证据是 `migration_aster_unit_test.rs::TestHandle`。

## 主要符号

- `pub type Error = anyhow::Error`：统一 trait 方法和闭包的错误通道，允许测试实现包装任意可发送、可同步的错误原因。
- `sessionctx::Context`：`Clone + Debug + Default` 的零字段占位类型。它只保留 Go `sessionctx.Context` 在回调签名中的位置，不提供 SQL 执行、会话变量或事务状态。
- `execute::Progress`：一个可克隆、可比较的采样点，含 `RowCnt: i64`、通用进度量 `Processed: i64` 和 `UpdateTime: SystemTime`。
- `execute::SubtaskSummary`：运行时汇总载体。`RowCnt`、`Processed`、`ReadBytes` 使用 `AtomicI64`，`GetReqCnt`、`PutReqCnt` 使用 `AtomicU64`，`Progresses` 是普通 `Vec<Progress>`。类型只派生 `Debug + Default`，没有更新、重置、速率计算或序列化方法。
- `SessionExecutor`：声明 `WithNewSession` 与 `WithNewTxn`。两者接收 `FnOnce(sessionctx::Context) -> Result<(), Error>`；后者额外接收 `crate::Context`，在本 crate 中即 `()`。
- `TaskHandle: SessionExecutor`：在会话执行能力上增加 `GetPreviousSubtaskMetas(task_id, step)` 与 `GetPreviousSubtaskSummary(task_id, step)`，分别返回拥有所有权的 meta 字节数组和汇总数组。

命名保留 Go 的大写方法/字段风格，因此文件级 `#![allow(non_snake_case)]` 是有意的兼容措施，不表示仓库通用 Rust 命名规范。

## 执行流程

本文件只定义协议，实际调用流程由 mock 扩展测试组装：

1. 测试定义句柄类型并实现 `SessionExecutor` 和 `TaskHandle`。现有 `migration_aster_unit_test.rs::TestHandle` 在 `WithNewSession`/`WithNewTxn` 中构造默认空 `sessionctx::Context` 后同步执行一次 `FnOnce`。
2. 测试通过 `scheduler_mock.rs::NewMockExtension::<H, _>` 创建 `MockExtension<H>`。`H` 必须实现本文件的 `TaskHandle`，同时满足 `Send + Sync + 'static`。
3. `MockExtension` 的期望方法接收 `&H`。扩展回调若需要会话边界，可调用 `WithNewSession` 或 `WithNewTxn`；若需要前一步输入，可调用两个 `GetPrevious...` 方法。
4. 句柄实现负责返回数据或错误。现有 `TestHandle::GetPreviousSubtaskMetas` 将 `task_id` 与 `step` 编成一项字节串，`GetPreviousSubtaskSummary` 返回空数组；这证明接口传递语义，但不模拟数据库查询。

对照真实 Go 流程，`scheduler.go::BaseScheduler.GetPreviousSubtaskMetas` 会查询指定任务、步骤下状态为 succeed 的全部 subtask，并按查询结果提取 `Meta`；`GetPreviousSubtaskSummary` 委托 task manager 查询汇总。上述行为没有在本适配文件中实现，不能把 mock 返回值视为真实存储语义。

## 数据与状态

所有数据均由调用者拥有：meta 使用 `Vec<Vec<u8>>`，summary 使用 `Vec<execute::SubtaskSummary>`，没有借用数据库行或共享缓存。`Progress` 记录某一时刻的行数、通用处理量和墙钟时间；`SystemTime` 可表达 UNIX epoch 之前或之后的时间，也可能在计算持续时间时产生时钟回拨问题，但本文件不执行时间运算。

`SubtaskSummary` 的五个计数器可通过原子操作在共享引用上更新。字段类型没有在本文件内规定内存序；现有 `storage_adapter_test.rs::subtask_summary_preserves_the_go_execute_contract` 使用 `Ordering::SeqCst` 写入并读取，验证五个值和一个进度点能被原样保存。`Progresses` 是普通向量，只能通过独占可变访问安全修改；它不因其他字段是原子类型而获得并发追加能力。

`Default` 会把原子计数器置零、把进度列表置空；空 `sessionctx::Context` 的 `Default` 不承载状态。文件没有全局变量、缓存、数据库连接或隐藏生命周期状态。

## 依赖与调用关系

上游装配关系为：`mock/lib.rs` 装入本文件并将符号再导出到 crate 根及 `storage` 模块；`scheduler_mock.rs` 导入 `crate::storage`，在 `MockExtension<H>` 的泛型界限以及三个带 handle 的回调中使用 `TaskHandle`。`migration_aster_unit_test.rs::TestHandle` 是已核验的直接 trait 实现者；`storage_adapter_test.rs` 直接使用 `execute::{Progress, SubtaskSummary}`。

下游类型依赖包括：

- `anyhow::Error`，对应 `Error` 别名；
- 标准库 `AtomicI64`、`AtomicU64`，承载并发计数；
- 标准库 `SystemTime`，承载采样时间；
- crate 根的 `Context`（当前为 `()`）与 `proto::Step`；
- 回调自身提供的 `FnOnce`，由实现者在会话或事务边界内调用。

RustCodeGraph 将本文件标记为被 `mock/lib.rs`、`scheduler_mock.rs`、相关测试及其他引用方使用，但对这些泛型 trait 的 `callers`/`callees` 查询没有生成静态边。因此，具体使用关系以 `lib.rs` 的再导出、`scheduler_mock.rs` 的泛型约束和独立测试实现作为补充证据。

真实应用中还有两条相邻但独立的边界：`storage/task_table.rs` 声明生产存储侧的 Go 风格 `SessionExecutor`/`TaskHandle`，`storage/lib.rs` 为 `TaskManager` 实现其中的 `SessionExecutor`；`scheduler/interface.rs` 则声明对象安全、snake_case 的调度侧 `TaskHandle`，由 `scheduler/scheduler.rs::BaseScheduler` 实现并委托其 task manager。这些符号不能仅凭同名或同签名与本文件的 trait 互换。

## 错误处理与边界

四个 trait 方法均通过 `Result<_, anyhow::Error>` 原样传播失败，本文件不分类错误、不重试、不记录日志，也不补充上下文。闭包返回的错误是否直接传出取决于实现者；现有 `TestHandle` 是直接返回闭包结果。

接口边界包括：

- `WithNewSession`/`WithNewTxn` 的闭包是 `FnOnce`，协议只允许消费一次；本文件没有强制实现者一定调用它，但正确的测试实现应明确验证调用次数与错误传播。
- 两个泛型闭包方法使 `SessionExecutor` 以及继承它的 `TaskHandle` 不具备 trait-object 兼容性。`scheduler_mock.rs` 因而以泛型 `H` 使用句柄，而不是 `dyn TaskHandle`；该限制已写在其源码注释中。
- `task_id` 和 `step` 没有本地合法性校验；负 ID、未识别步骤、无历史记录和查询失败应由实现者定义并由独立测试覆盖。
- 返回值拥有所有权，空数组可表示“没有记录”；协议没有分页、排序、去重或最大结果数约束。
- `SubtaskSummary` 不含 Go JSON 标签，也没有 Go 版本的 `Update`、`Reset`、`GetSpeedInTimeRange`、`UpdateTime`、`MergeObjStoreRequests` 行为，因此只能作为数据形状占位，不能承担完整汇总逻辑。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、session 或事务。会话和事务资源的获取、提交、回滚与释放全部属于 `SessionExecutor` 实现者的职责；仅从 trait 签名无法证明 panic 时清理、事务提交顺序或取消传播。

`SubtaskSummary` 的原子字段支持多个线程在不取得 `&mut self` 的情况下更新单个计数器，但跨字段快照不是原子的：读取者可能观察到不同时间点的 `RowCnt`、`Processed` 和请求计数。`Progresses` 需要外部独占访问或同步保护。`MockExtension<H>` 额外要求 `H: Send + Sync + 'static`，但这个约束来自消费者 `scheduler_mock.rs`，并未写入本文件的 trait 定义；单独实现本 trait 不自动保证可跨线程使用。

回调参数按值接收空 `sessionctx::Context`，回调结束后即释放。`GetPrevious...` 返回的向量也由调用者负责生命周期。本文件没有需要显式关闭的资源。

## 与 Go 版本的对应关系

`storage/task_table.go` 是接口语义的直接 Go 对照：其 `SessionExecutor` 同样提供 `WithNewSession` 和 `WithNewTxn`，`TaskHandle` 同样嵌入会话执行能力并读取前序步骤的 metas 与 summaries。本文件保留方法名、参数顺序和错误返回形状；Rust 用 `FnOnce` 表达一次性回调，用 `Vec<Vec<u8>>` 表达 `[][]byte`，用 `Result` 表达“值加 error”。Go 的 `context.Context` 在 mock crate 中退化为 `()`，Go 的 `sessionctx.Context` 退化为空 struct。

`taskexecutor/execute/interface.go` 是数据形状的直接 Go 对照。字段逐一对应，但有以下已核验差异：

- Go 的 `time.Time` 对应 Rust `SystemTime`；Go 原子类型对应标准库原子类型。
- Go summary 返回 `[]*execute.SubtaskSummary`，本文件返回拥有所有权的 `Vec<SubtaskSummary>`，没有可空元素或共享指针语义。
- Go 字段带持久化兼容所需 JSON 标签，尤其 `Processed` 仍使用 `"bytes"`；本文件没有序列化派生或标签。
- Go 类型实现请求数合并、采样保留上限、区间速率、最后更新时间和 reset；这些逻辑存在于完整 Rust `taskexecutor/execute/interface.rs`，但不在本 mock 适配类型中。
- Go `scheduler/mock/scheduler_mock.go` 直接导入生产 `storage.TaskHandle`；Rust mock crate 为保持较小依赖面，改用本文件的平行 trait，并以泛型句柄绕过非对象安全限制。

因此迁移状态应描述为“足以支撑当前 Extension mock 与契约测试的最小适配”，而不是完整替代 Go storage/execute 实现。

## 扩展指南

若只给 Extension mock 增加不涉及存储的新方法，通常应修改 `scheduler_mock.rs` 及其独立测试，不应扩张本文件。若新回调确实需要额外句柄能力，应按以下顺序处理：

1. 先在 Go `storage.TaskHandle`、真实 Rust 存储边界和生产 scheduler 接口中确认该能力的真实契约，避免只为 mock 发明行为。
2. 在本文件的 `TaskHandle` 增加最小必要签名，并同步所有实现者，至少包括 `migration_aster_unit_test.rs::TestHandle`；同时更新 `scheduler_mock.rs` 中使用该能力的回调或类型约束。
3. 在独立测试文件中覆盖成功、空结果、实现者错误和参数转发。不要把测试模块内嵌回 `storage_adapter.rs`。
4. 若修改 `Progress`/`SubtaskSummary`，同步核对 `taskexecutor/execute/interface.go`、完整 Rust `taskexecutor/execute/interface.rs` 以及 `storage_adapter_test.rs`。新增持久化或跨 crate 传输前必须明确序列化格式与旧 JSON 的兼容风险。
5. 若需要以 `dyn TaskHandle` 传递句柄，必须重新设计泛型回调（例如引入对象安全的擦除边界），不能只把 `dyn` 加到现有 trait 上。

主要风险是三套同名接口漂移、mock 数据类型与生产类型被误混用、给原子计数器附加不存在的整体快照保证，以及扩张本 mock crate 依赖导致循环依赖或编译面增大。性能上，当前 meta/summary 均整批分配并返回；若历史数据变大，应先在真实接口中定义分页或流式语义，而不是只优化 mock。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/dxf/framework/scheduler/mock` 确认目标、入口、生成 mock 与两个独立测试；`node --file pkg/dxf/framework/scheduler/mock/storage_adapter.rs` 核对全部 81 行和符号；`query` 核对本文件的 `SessionExecutor`、`TaskHandle`、`SubtaskSummary` 与仓库同名类型。对目标 trait 执行 `callers`/`callees` 未返回静态边，故未把缺失的图边当作“没有调用者”。
- Rust 源码：`scheduler/mock/lib.rs`（路径装入、再导出、`Context = ()`、测试挂载）、`scheduler/mock/scheduler_mock.rs`（`MockExtension<H>` 泛型界限和 handle 回调）、`scheduler/mock/migration_aster_unit_test.rs::TestHandle`（trait 实现与同步回调行为）、`scheduler/mock/storage_adapter_test.rs::subtask_summary_preserves_the_go_execute_contract`（字段保存契约）。
- 相邻真实实现：`storage/task_table.rs`（生产侧同名 trait）、`storage/lib.rs`（`TaskManager` 的 `SessionExecutor` 实现）、`scheduler/interface.rs` 与 `scheduler/scheduler.rs`（当前生产调度侧对象安全接口及 `BaseScheduler` 委托）。
- Cargo：`scheduler/mock/Cargo.toml` 确认独立 crate、`lib.rs` 入口、三个直接依赖和 Go 包映射；`scheduler/Cargo.toml` 确认该 mock crate 是 Windows 目标下的 dev-dependency，而生产 scheduler 依赖真实 storage crate。
- Go 对照：`storage/task_table.go`（`SessionExecutor`/`TaskHandle`）、`taskexecutor/execute/interface.go`（`Progress`/`SubtaskSummary` 字段与完整方法）、`scheduler/scheduler.go::BaseScheduler.GetPreviousSubtaskMetas/GetPreviousSubtaskSummary/WithNewSession/WithNewTxn`（真实委托行为）、`scheduler/mock/scheduler_mock.go`（生成 mock 对生产 `storage.TaskHandle` 的直接引用）。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工检查：本文明确回答了文件存在原因、mock 调用方式、数据/错误/并发边界、Go 差异和安全扩展位置，且没有把占位接口描述为真实存储实现。
