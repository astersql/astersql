# `lightning/pkg/importinto/mock/import_mock.rs`

## 文件定位

本文件属于独立 crate `astersql-lightning-pkg-importinto-mock`。crate 入口 `lightning/pkg/importinto/mock/lib.rs` 先装配本地 `stubs.rs`，再公开重导出本文件的全部符号；`Cargo.toml` 只依赖父目录的 `astersql-lightning-pkg-importinto`，因此这里是 import-into 子系统的测试替身边界，而不是导入作业的生产实现。

文件是 Go MockGen 产物 `lightning/pkg/importinto/mock/import_mock.go` 的手工 Rust 语义镜像，覆盖 `CheckpointManager`、`JobSubmitter`、`JobMonitor`、`JobOrchestrator` 和 `ProgressUpdater` 五个接口。它让 Rust 测试能以 GoMock 风格先登记预期调用和返回值，再把 mock 作为对应 trait 对象注入被测代码。真实 checkpoint 持久化、作业提交、轮询、编排和进度统计分别位于父 crate 的实现文件中；本文件不执行这些业务动作。

## 核心职责

1. 为五个父 crate trait 提供可注入的 mock 类型及完整 trait 实现，维持与 Go 生成代码相同的方法集合。
2. 为每个 mock 配置一个共享 `Controller` 和一个 recorder。`EXPECT()` 暴露 recorder，recorder 的同名方法通过 `RecordCallWithMethodType` 登记期望；实际方法通过 `Controller::Call` 消费匹配项。
3. 在动态返回值与静态 Rust 类型之间转换：无返回值/错误用 `take_error`，单返回值用 `take_one`，`(T, error)` 用 `take_pair`，Go 指针返回用 `take_opt_pair`。
4. 保留跨语言边界语义。例如 `SubmitTable` 收到“nil job、nil error”时构造零值 `ImportJob`，而 `Get` 的 nil checkpoint 映射成 `None`。

这里的“mock”是测试基础设施，不代表生产调用链依赖简化控制器。其可观察契约由 `lightning/pkg/importinto/mock/parity_test.rs` 直接验证。

## 主要符号

- `NewMockCheckpointManager`、`NewMockJobSubmitter`、`NewMockJobMonitor`、`NewMockJobOrchestrator`、`NewMockProgressUpdater`：五个公开构造器。每个构造器克隆传入的 `Controller` 给 recorder，并把原控制器保存在 mock 中；二者因此操作同一预期队列。
- `MockCheckpointManager`：实现 `CheckpointManager` 的测试替身，覆盖 `Initialize`、`Get`、`Update`、`Remove`、`IgnoreError`、`DestroyError`、三种 `Dump*`、`GetCheckpoints` 和 `Close`。
- `MockJobSubmitter`：实现 `JobSubmitter`，覆盖 `SubmitTable` 与 `GetGroupKey`。`SubmitTable` 是本文件唯一带额外兼容分支的方法：`take_opt_pair::<ImportJob>` 返回 `None` 且没有错误时，返回字段均为零值的 `ImportJob`。
- `MockJobMonitor`：实现 `JobMonitor`，只转发 `WaitForJobs`。
- `MockJobOrchestrator`：实现 `JobOrchestrator`，转发 `SubmitAndWait` 和 `Cancel`。
- `MockProgressUpdater`：实现 `ProgressUpdater`，转发 `UpdateTotalSize` 和 `UpdateFinishedSize`；两者忽略控制器返回数组，因为接口本身没有返回值。
- 五个 `*MockRecorder`：登记相同方法名和参数匹配器，并返回可继续设置返回值的 `Call`。
- 每个 mock 的 `EXPECT()` 返回其 recorder；`ISGOMOCK()` 是与 Go 生成 mock 对齐的标记方法，不产生状态变化。
- `_stubs_ty`：仅用于保持 `stubs` 模块导入被引用，构造一个新 `Controller` 后立即返回；它不参与 mock 的正常调用流程。

所有公开 mock 方法都先调用无副作用的 `Controller::Helper()`，以对应 Go 的 `ctrl.T.Helper()`，然后再登记或消费调用。

## 执行流程

典型测试流程如下：

1. 测试创建 `Controller::new()`，再把同一个控制器传给一个或多个 `NewMock*` 构造器。
2. 测试调用 `mock.EXPECT().Method(matcher...)`。recorder 调用 `RecordCallWithMethodType`，把方法名、参数匹配信息和空返回数组压入共享预期队列；随后以 `Return`、`ReturnError`、`Return1` 或 `Return2` 配置结果。
3. 被测代码经具体类型或 `dyn CheckpointManager` 等 trait 对象调用 mock。trait impl 只调用同名固有方法，不改变参数或结果。
4. 固有方法克隆需要拥有的数据后调用 `Controller::Call`。控制器从队列中查找方法名、参数数量和可识别参数值都匹配的第一项；找到后移除该项并取出返回数组，未找到则 panic。
5. `take_*` 辅助函数把类型擦除的返回数组还原成相应 `Result`/值。若返回错误，错误直接传给调用者；若类型缺失或不匹配，辅助函数按 Go 类型断言失败后的零值语义返回默认值或 `None`。
6. 测试可用 `Controller::remaining()` 确认所有预期已消费。多个 mock 可共享控制器，丢弃其中一个 mock 不会清空队列。

`DumpTables`、`DumpEngines`、`DumpChunks` 的 `writer` 参数在实际调用时被替换成 `()` 交给控制器，只验证“存在一个参数”，不会写入或检查 writer 内容。这些方法只能用于验证调用发生和错误传播，不能验证真实输出字节。

## 数据与状态

每个 mock 有两个字段：公开的 `ctrl: Controller` 与公开的 `recorder`；recorder 内部也持有一个 `Controller`。`Controller` 在 `stubs.rs` 中是 `Arc<Mutex<ControllerInner>>`，克隆只增加共享所有权，不复制预期队列。队列元素记录方法名、简化后的参数匹配器以及一个受 `Mutex` 保护的动态返回数组。

本文件自身不保存 checkpoint、作业、表元数据或进度值。调用参数只为匹配而被克隆：`Context`、字符串、`TableCheckpoint`、`TableMeta`、`ImportJob` 切片都会转成控制器拥有的值。实际业务状态完全由测试配置的返回数组决定。

本地 matcher 只精确识别 `i64` 和 `String`；`()` 以及其他类型都变成通配符。因此 parity 测试能验证进度值不匹配会失败，但上下文、复杂结构和多数引用参数只验证参数个数而不验证内容。这是本地轻量替身相对完整 GoMock 的重要限制。

## 依赖与调用关系

- 上游接口：`CheckpointManager` 定义在 `lightning/pkg/importinto/checkpoint.rs`，`JobSubmitter` 定义在 `job_submitter.rs`，`JobMonitor` 定义在 `job_monitor.rs`，`JobOrchestrator` 定义在 `job_orchestrator.rs`，`ProgressUpdater` 定义在 `importer.rs`。五个 trait 都要求 `Send + Sync`。
- 下游测试控制层：本文件依赖 `lightning/pkg/importinto/mock/stubs.rs` 的 `Controller`、`Call`、`Result` 和四个 `take_*` 转换函数。
- crate 装配：`lightning/pkg/importinto/mock/lib.rs` 将 `stubs` 与本文件符号公开重导出；`Cargo.toml` 的唯一直接依赖提供接口及 `Context`、`TableCheckpoint`、`ImportJob`、`TableMeta` 等模型。
- 直接验证者：RustCodeGraph 显示本文件由 `lightning/pkg/importinto/mock/parity_test.rs` 以及 importinto 的 checkpoint、job monitor、job orchestrator 和 parity 测试引用。前者直接构造本文件所有五种 mock；其余测试主要验证相同父接口的生产实现和边界语义。
- 应用主链中的位置：生产的 orchestrator 组合 submitter、monitor、checkpoint manager 和 progress updater；本文件通过实现这些接口替换链中的边界，供独立测试控制成功、空值、错误和取消等结果，但不会自行进入生产路径。

RustCodeGraph 对 `NewMockCheckpointManager`、`NewMockJobSubmitter`、`NewMockJobMonitor`、`NewMockJobOrchestrator`、`NewMockProgressUpdater` 的调用者检索均指向 `mock/parity_test.rs` 的正常、边界、错误或资源清理场景；方法级调用还出现在相应接口测试中。

## 错误处理与边界

- `take_error` 将缺失返回值或 `None::<Error>` 解释为成功，将 `Some(Error)` 或裸 `Error` 原样返回。所有返回 `Result<()>` 的 mock 方法采用此路径。
- `take_pair` 先取得值，再解析错误；错误存在时丢弃值并返回错误。值缺失或类型错误时使用 `Default`，适用于列表等 Go 零值兼容场景。
- `take_opt_pair` 将 `Option<T>`、裸 `T` 和其他/缺失类型分别转换为对应 `Option<T>`、`Some(T)` 和 `None`。`Get` 保留 `None`；`SubmitTable` 再将成功的 `None` 转成默认 `ImportJob`，因为 Rust trait 返回非可选值。
- 未登记的调用、方法名不符、参数个数不符或可识别参数值不符会在 `Controller::Call` 中 panic；`gomock_rejects_mismatched_arguments` 和 `contract_error` 明确验证了该行为。
- 预期项按“首个匹配项”消费，不隐含登记顺序。`gomock_matches_expectations_without_implicit_ordering` 验证相反次序调用仍成功；只有找不到任何匹配项才失败。
- `Dump*` 不保留 writer 身份或输出，复杂参数多为通配符，因此本文件不能证明参数深比较或 I/O 内容正确。需要这些保证时，应在生产实现的独立测试中断言。

## 并发与资源生命周期

`Controller` 的队列由 `Arc<Mutex<_>>` 保护，返回数组也各自有 `Mutex`，使五种 mock 满足父 trait 的 `Send + Sync` 约束，并可被多个 mock 或线程共享。一次成功调用会原子地从队列移除一个匹配预期，再在队列锁之外取走其返回数组，避免持有全局队列锁解析返回值。

构造器让 mock 与 recorder 共享同一个控制器；clone mock 控制器或把多个 mock 建在同一 controller 上，也共享同一队列。丢弃 mock、recorder 或某个 controller clone 只减少 `Arc` 引用计数，不会隐式验证或清空剩余预期。`contract_resource_cleanup` 验证：消费前 `remaining()` 为 1，调用后为 0；丢弃共享控制器上的一个 mock 后，其他预期仍存在。

本文件没有线程、异步任务、通道、文件句柄、网络连接或显式关闭协议。`MockCheckpointManager::Close` 只是一个可录制调用，不释放控制器资源。若测试要求“结束时没有未满足预期”，必须显式检查 `remaining()`；当前本地 controller 没有 GoMock controller 结束阶段的自动失败钩子。

## 与 Go 版本的对应关系

Go 文件由 MockGen 生成，五组类型、构造器、`EXPECT`/`ISGOMOCK`、接口方法和 recorder 方法在 Rust 中逐项对应。Go recorder 持有 `*MockType`，Rust recorder 直接持有共享 `Controller`；这是所有权表达差异，不改变登记预期再调用的外部模式。

主要类型映射为：Go `context.Context` 对应 Rust `context::Context`；Go 指针参数在 Rust 中多为借用；Go `[]*ImportJob`/`[]*TableMeta` 对应 Rust 切片；Go `error` 对应父 crate 的 `Result`。Go 的 `*TableCheckpoint` 返回可为 nil，所以 Rust 使用 `Option<TableCheckpoint>`。Go `SubmitTable` 返回 `*ImportJob`，但 Rust trait 返回非可选 `ImportJob`，因此 nil 成功结果被显式映射为零值对象。

Rust 轻量 controller 并非 `go.uber.org/mock/gomock` 的完整重写：它只支持有限参数匹配、动态返回与无隐式顺序的首个匹配消费，也没有反射方法类型检查或测试结束自动校验。`Dump*` 尤其不会向真实 writer 写数据。这些差异已由 `mock/parity_test.rs` 固定当前可观察范围，扩展时不能把完整 GoMock 能力当作既有事实。

## 扩展指南

- 父 trait 新增或修改方法时，应同步更新对应 `Mock*` 固有方法、trait impl 与 `*MockRecorder` 三处，并核对 Go 同路径生成文件的方法签名。
- 新增返回形态时优先复用 `take_error`、`take_one`、`take_pair`、`take_opt_pair`；若现有转换不能表达 Go 零值/指针语义，应在独立的 `stubs.rs` 及其独立测试中扩展，不能把测试逻辑嵌入本生产源文件。
- 若需要精确匹配 `Context`、结构体、切片或 writer，应扩展 `ExpectedArg`，同时保留 `()` 作为通配符的既有测试契约，并为误匹配与正确匹配各加独立回归测试。
- 若要求验证 `Dump*` 输出，不应只增强本 mock 的返回值；应让控制器能够执行 writer 副作用，或直接在 checkpoint 生产实现测试中验证字节内容，并清楚区分“调用发生”和“输出正确”。
- 涉及共享 controller、并发调用或自动验证剩余预期时，应评估锁顺序、panic 时队列状态以及与 GoMock 生命周期的兼容性。相关回归测试应放在 `lightning/pkg/importinto/mock/parity_test.rs` 或新的独立 `*_test.rs`，不要放进 `import_mock.rs`。
- 文件标注为 Go MockGen port。若 Go 接口重新生成，应逐项对照方法集合与 nil/错误语义，避免直接覆盖 Rust 特有的 trait 实现和类型转换分支。

## 验证依据

- RustCodeGraph `status`：索引包含目标文件；`files --filter lightning/pkg/importinto/mock` 列出 Go/Rust 对照、crate 入口、parity 测试和 stubs。
- RustCodeGraph `node --file lightning/pkg/importinto/mock/import_mock.rs`：核对 808 行文件、五组 mock/recorder/trait impl、所有方法及 `_stubs_ty`；`explore` 核对构造器和方法调用者。
- RustCodeGraph `node`：核对 `CheckpointManager`（`checkpoint.rs:220`）、`JobSubmitter`（`job_submitter.rs:85`）、`JobMonitor`（`job_monitor.rs:90`）、`JobOrchestrator`（`job_orchestrator.rs:143`）、`ProgressUpdater`（`importer.rs:91`）的真实 trait 签名与 `Send + Sync` 边界。
- `lightning/pkg/importinto/mock/Cargo.toml` 与 `lib.rs`：核对独立 crate、唯一父 crate 依赖、模块装配和公开重导出。
- `lightning/pkg/importinto/mock/import_mock.go`：核对 MockGen 来源、五个 Go 接口的构造器、方法、recorder、指针返回和错误返回语义。
- `lightning/pkg/importinto/mock/stubs.rs`：核对 `Arc<Mutex<_>>` 共享队列、首个匹配消费、参数匹配范围、panic 条件及 `take_*` 默认值/错误转换。
- `lightning/pkg/importinto/mock/parity_test.rs`：核对正常调用、nil/空集合、错误传播、未登记调用 panic、参数不匹配、无隐式调用顺序、trait 对象适配和共享 controller 生命周期。
- 相关独立测试路径：`lightning/pkg/importinto/checkpoint_test.rs`、`job_monitor_test.rs`、`job_orchestrator_test.rs` 与 `parity_test.rs`，用于确认父接口在真实实现中的边界；本任务未运行 Cargo，符合纯文档计划约束。
