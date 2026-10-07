# `pkg/dxf/importinto/mock/import_mock.rs`

## 文件定位

[目标源文件](import_mock.rs) 是 `astersql-dxf-importinto-mock` crate 的主要实现，为 IMPORT INTO 最小任务执行器提供手写的 GoMock 风格 Rust mock。crate 根 `pkg/dxf/importinto/mock/lib.rs` 公开 `import_mock` 模块并再导出本文件的符号；`pkg/dxf/importinto/mock/Cargo.toml` 将它标记为 Go 包 `pkg/dxf/importinto/mock` 的移植对应物。上层 `pkg/dxf/importinto/Cargo.toml` 只在 `[dev-dependencies]` 中引入该 crate，所以它当前是测试边界，不是 IMPORT INTO 生产执行路径中的真实执行器。

直接 Go 对照是由 MockGen 生成的 `pkg/dxf/importinto/mock/import_mock.go`，其源接口是 `pkg/dxf/importinto/subtask_executor.go` 中的 `MiniTaskExecutor`。Rust 文件不是自动生成代码：它用显式 trait 和同步器仿真 GoMock 所需的最小公开契约。

## 核心职责

- 用 `MiniTaskExecutor::run` 描述单个 chunk 的测试执行边界，参数依次是 `Context`、数据引擎 writer、索引引擎 writer 和进度 `Collector`。
- 用 `MockMiniTaskExecutor` 把实际 `Run` 调用交给可注入的 `Controller`，让测试决定记录方式、副作用和返回错误。
- 用 `MockMiniTaskExecutorMockRecorder` 和 `ExpectedRunArguments` 保留 GoMock 的 `EXPECT().Run(...)` 形状，将四个 matcher/期望值类型擦除后交给 controller。
- 提供 Go 风格公开名称 `NewMockMiniTaskExecutor`、`EXPECT`、`ISGOMOCK` 和 `Run`，同时提供 Rust 风格别名 `new_mock_mini_task_executor`、`expect`、`is_gomock` 和 trait 方法 `run`。
- 再导出 `Collector`、`BackendError`、`ChunkFlushStatus`、`EngineWriter`、`Context` 和 `Rows`，使 controller 及独立测试可以直接实现完整参数契约。

## 主要符号

- `NEXT_RECEIVER_ID: AtomicU64`：进程内的 mock 接收者 ID 分配器，初值为 1；构造时以 `Ordering::Relaxed` 递增。
- `Error(pub String)`：该 mock 边界的轻量错误，实现 `Display` 和 `std::error::Error`，也可克隆、调试和相等比较。
- `RunArguments`：一次真实分发的所有权容器。`Context` 按值传递，两个 writer 是可缺省的 `Box<dyn EngineWriter>`，collector 是可缺省的 `Arc<dyn Collector + Send + Sync>`。
- `ExpectedRunArguments`：期望登记容器，四个字段都为 `Arc<dyn Any + Send + Sync>`，对应 Go 生成代码中的四个 `any`。
- `RecordedCall`：保存 `receiver_id`、方法名、方法类型标识和期望参数；其实例由 controller 创建并以 `Arc` 返回。
- `Controller: Send`：核心扩展接口。`helper` 镜像 `ctrl.T.Helper()`，`call` 执行实际分发，`record_call_with_method_type` 登记期望。controller 实现者定义匹配、记录、副作用和错误策略。
- `MiniTaskExecutor`：Rust 的可调用抽象，其 `run` 签名与 mock 的实际参数模型一致。
- `MockMiniTaskExecutor`：保存唯一 `receiver_id`、共享 controller 和一个预绑定 recorder。`Run` 是主分发入口，`EXPECT` 返回 recorder 引用，`ISGOMOCK` 是无状态标记方法。
- `MockMiniTaskExecutorMockRecorder`：与 mock 使用相同的 `receiver_id` 和 controller；其 `Run<A0, A1, A2, A3>` 要求每个期望值实现 `Any + Send + Sync`。
- `new_mock_mini_task_executor` / `NewMockMiniTaskExecutor`：前者是 Rust 命名别名，后者实际分配 ID、构造 recorder 并返回 mock。

## 执行流程

1. 测试先实现 `Controller`，再把 `Arc<Mutex<dyn Controller>>` 传给 `NewMockMiniTaskExecutor`。构造函数从 `NEXT_RECEIVER_ID` 取得 ID，将同一 controller 的 `Arc` 克隆给 recorder，原始 `Arc` 存入 mock。
2. 登记期望时，调用链为 `mock.EXPECT().Run(...)`。`EXPECT` 只借用已创建的 recorder；recorder 先对 controller 加锁，锁中顺序调用 `helper()` 和 `record_call_with_method_type(...)`。
3. recorder 把四个泛型期望参数分别装入 `Arc<dyn Any + Send + Sync>`，固定传入方法名 `"Run"` 和方法类型标识 `"MockMiniTaskExecutor::Run"`，然后返回 controller 产生的 `Arc<RecordedCall>`。
4. 实际执行时，`MockMiniTaskExecutor::Run` 对 controller 加锁，先调用 `helper()`，再将所有实参移入 `RunArguments` 并调用 `Controller::call(receiver_id, "Run", arguments)`。
5. controller 的 `Result<(), Error>` 原样成为 mock 的返回值。`MiniTaskExecutor for MockMiniTaskExecutor` 的 trait 方法和小写别名都只转发到上述 Go 风格入口，不增加分支。

`pkg/dxf/importinto/mock/import_mock_test.rs` 将流程具体化为事件序列 `helper, record, helper, Run`，并验证 context 取消状态、数据/索引 writer 顺序、collector 副作用和 controller 错误均穿过该路径。

## 数据与状态

持久到 mock 生命周期的状态只有接收者 ID、controller 共享引用和 recorder。mock 与 recorder 的 ID 在构造时一次性绑定，使多个 mock 即使共用 controller，controller 也能区分调用来源。`NEXT_RECEIVER_ID` 只用于身份区分，不表示调用次序或业务任务 ID。

实际 `RunArguments` 持有参数所有权：两个 trait-object writer 被移交给 controller，collector 通过 `Arc` 共享。期望参数则使用 `Any` 类型擦除，文件本身不解释 matcher 语义，也不自动比较期望与实际参数；这些工作属于具体 `Controller` 实现。

## 依赖与调用关系

本 crate 在 `pkg/dxf/importinto/mock/Cargo.toml` 中只有三个直接路径依赖：

- `astersql-dxf-framework-taskexecutor-execute` 提供 `Collector`；
- `astersql-lightning-backend` 提供 `EngineWriter`、`BackendError` 和 `ChunkFlushStatus`；
- `astersql-lightning-backend-encode` 提供 `Context` 和 `Rows`。

文件内部调用边为：`new_mock_mini_task_executor -> NewMockMiniTaskExecutor`，`expect -> EXPECT`，`is_gomock -> ISGOMOCK`，trait `MiniTaskExecutor::run -> MockMiniTaskExecutor::Run -> Controller::{helper, call}`，recorder 的小写 `run -> Run -> Controller::{helper, record_call_with_method_type}`。RustCodeGraph 的 `explore` 结果还识别到独立测试 `generated_gomock_public_contract_and_dispatch_order_match_go` 对 `NewMockMiniTaskExecutor`、`EXPECT`、`ISGOMOCK` 和 `Run` 的使用。

仓库搜索未发现除本 crate 自身测试外的 Rust 直接使用者；上层 IMPORT INTO crate 虽将它声明为 dev-dependency，当前 Rust 测试未把它注入与 Go `encode_and_sort_operator_test.go` 相同的完整算子场景。因此“供 IMPORT INTO 编码/排序管线测试注入”是该 mock 的接口定位和 Go 使用事实，不应解读为 Rust 上层链路已完整接线。

## 错误处理与边界

- controller 互斥锁如果因其他线程 panic 而 poison，实际分发和期望登记都不会 panic，而是返回 `Error("mock controller lock poisoned")`。
- `Controller::call` 的错误不包装、不替换，由 `MockMiniTaskExecutor::Run` 原样向上返回。独立测试以 `Error("run failed")` 验证该契约。
- 参数的 `Option` 允许两个 writer 或 collector 缺席；本层不把 `None` 视为错误，具体 controller 可自行断言。
- 期望参数必须是 `Any + Send + Sync`，因此需要满足 `'static` 类型擦除边界；短生命周期借用值不能直接登记。
- `ISGOMOCK` 仅返回 Rust 单元值 `()`，没有运行时校验。`method` 和 `method_type` 也是约定字符串，不具有 Go `reflect.TypeOf` 的反射强度。
- 本文件没有自动的期望消耗计数、调用次数校验或 controller `Finish` 阶段；需要时必须在 controller 实现和测试中明确补充。

## 并发与资源生命周期

`Arc<Mutex<dyn Controller>>` 是并发模型的核心。`Controller` 只要求 `Send`，因为访问它必须先经过 `Mutex`；mock 与 recorder 共享同一 controller。每次期望登记会在同一次持锁期间完成 `helper` 和 `record_call_with_method_type`，每次实际分发也会在同一次持锁期间完成 `helper` 和 `call`。这保证单个 controller 的事件不被其他通过该锁的调用插入，但也意味着 controller 回调执行期间锁始终被占用；回调若重入同一 controller，可能死锁。

`NEXT_RECEIVER_ID` 的 relaxed 原子操作只保证无数据竞争的递增取值，没有额外的跨线程内存顺序含义。代码不检查 `u64` 回绕，在理论上极端数量的构造后 ID 可重复。writer 在 `Run` 时被移入 controller，其释放时机由 controller 是否保存/consume `RunArguments` 决定；`Arc` collector、controller 和 `RecordedCall` 在最后一个强引用离开作用域后释放。

## 与 Go 版本的对应关系

Go `import_mock.go` 是从 `MiniTaskExecutor` 接口生成的 GoMock：`NewMockMiniTaskExecutor` 绑定 `*gomock.Controller`，`EXPECT` 返回 recorder，实际 `Run` 执行 `Helper` 后调用 `ctrl.Call`，recorder `Run` 执行 `Helper` 后用 `RecordCallWithMethodType` 登记。Rust 逐项保留了这些公开名称、四参数顺序、helper-再-调用的顺序以及错误返回语义。

主要差异是：

- Go 直接使用 `gomock.Controller` 和 `*gomock.Call`；Rust 以本地 `Controller` trait 和 `Arc<RecordedCall>` 代替，不包含完整 GoMock matcher/次数/收尾能力。
- Go 的实参接口不使用 `Option`；Rust 的 writer 和 collector 显式允许 `None`，是 Rust mock 边界的额外表达能力。
- Go recorder 用 `reflect.TypeOf((*MockMiniTaskExecutor)(nil).Run)` 保存方法类型；Rust 仅保存字符串 `"MockMiniTaskExecutor::Run"`。
- Go `MockMiniTaskExecutor` 当前被 `pkg/dxf/importinto/encode_and_sort_operator_test.go` 用来替换 `newImportMinimalTaskExecutor`，覆盖执行错误取消和并发错误处理；Rust 当前只有 `import_mock_test.rs` 对 mock 自身契约的独立验证，没有证据表明已迁移上述整体算子用例。

## 扩展指南

- 如果 Go `MiniTaskExecutor.Run` 的参数或返回类型改变，应同步修改 `MiniTaskExecutor`、`RunArguments`、`ExpectedRunArguments`、`Controller::{call, record_call_with_method_type}`、mock/recorder 的两个 `Run` 以及构造 `RecordedCall` 的代码，不能只改某一层签名。
- 新增被 mock 方法时，需同时增加实际参数容器、期望参数容器、controller 入口、mock 分发和 recorder 登记，并保持 `helper` 调用顺序及正确的 receiver ID。
- 如果需要更接近 GoMock 的 matcher、次数、顺序或 `Finish` 校验，应扩展 `Controller` 契约或建立专门 controller 实现，而不应把匹配策略硬编码到 `MockMiniTaskExecutor::Run`。这类改动需评估持锁回调的重入风险。
- 所有行为扩展都应同步修改独立测试 `pkg/dxf/importinto/mock/import_mock_test.rs`，不要把测试嵌入本生产文件。至少应覆盖参数顺序、方法名/类型标识、helper 顺序、副作用和错误传递；修改并发策略时还应增加多线程与重入测试。
- 需要接入 Rust 上层 IMPORT INTO 测试时，应在对应的独立测试文件中使用已有 dev-dependency，并参照 Go `encode_and_sort_operator_test.go` 的工厂替换与恢复边界；不应因测试注入而把本 mock 加入生产依赖。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标 `import_mock.rs` 被识别为包含 24 个符号的 Rust 文件。
- RustCodeGraph `node --file pkg/dxf/importinto/mock/import_mock.rs`：核对了全文 241 行的常量、类型、trait、函数、impl 和所有分支；文件中没有条件编译项。
- RustCodeGraph `query MockMiniTaskExecutor`、`query MockMiniTaskExecutorMockRecorder` 和 `query new_mock_mini_task_executor`：核对 Rust/Go 同名符号及 Rust 命名别名。
- RustCodeGraph `explore "pkg/dxf/importinto/mock/import_mock.rs symbols callers callees role"`：核对内部调用边和 `generated_gomock_public_contract_and_dispatch_order_match_go` 对关键公开符号的覆盖。精确 `callers` 子命令在当前索引上持续无输出，已终止；调用者范围另以仓库 `rg` 搜索交叉核对。
- RustCodeGraph `node` 读取的直接证据：`pkg/dxf/importinto/mock/lib.rs`、`pkg/dxf/importinto/mock/import_mock_test.rs`、`pkg/dxf/importinto/mock/import_mock.go`、`pkg/dxf/importinto/subtask_executor.go` 和 `pkg/dxf/importinto/encode_and_sort_operator_test.go`。
- 未索引配置证据：`pkg/dxf/importinto/mock/Cargo.toml` 确认 crate 边界、三个路径依赖和 Go 包对应；`pkg/dxf/importinto/Cargo.toml` 确认它只是上层 crate 的 dev-dependency；根 `Cargo.toml` 确认该 crate 是 workspace 成员。
- 独立 Rust 测试的现有断言覆盖：公开 GoMock 形状、方法标识、参数顺序和状态、collector 副作用、controller 错误传递、事件顺序以及 `RecordedCall` 的 `Arc` 同一性。本任务是纯文档分析，按计划未运行 Cargo；验收依据为上述静态事实和文档结构检查。
