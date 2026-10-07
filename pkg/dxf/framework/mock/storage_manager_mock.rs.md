# `pkg/dxf/framework/mock/storage_manager_mock.rs`

## 文件定位

本文件属于 Cargo crate `astersql-dxf-framework-mock`（`pkg/dxf/framework/mock/Cargo.toml`），由 crate 根 `pkg/dxf/framework/mock/lib.rs` 以 `mod storage_manager_mock` 装入并用 `pub use storage_manager_mock::*` 对外导出。它是 `astersql_dxf_framework_storage::Manager` 的测试替身，不访问数据库，也不实现任务存储算法；调用结果完全由测试安装的闭包决定。

该文件对应 Go 生成文件 `pkg/dxf/framework/mock/storage_manager_mock.go`。仓库路径搜索确认，Rust 版本目前的直接使用点是同 crate 的独立测试 `pkg/dxf/framework/mock/migration_aster_unit_test.rs::storage_manager_mock_forwards_arguments_and_result`。它具备 `storage::Manager` trait 实现，因而可以注入要求该 trait 的代码，但当前未检索到生产 Rust 路径实际构造这个 `MockManager`；不能据此宣称它已接入完整生产调用链。

## 核心职责

- 用 `MockManager` 的三个公开 `Handler` 字段分别承载 CPU 核数查询、按 ID 读取任务和按 ID 修改任务的可变闭包。
- 提供与 GoMock 命名相近的 `EXPECT`、`ISGOMOCK`、`NewMockManager`，降低 Go 测试移植时的结构差异。
- 为三个接口提供同名固有方法，将参数原样传入对应 handler，并原样返回 `Result`。
- 实现 `storage::Manager`，把 trait 调用再次转发到固有方法，使替身满足应用侧存储管理接口。

这里的“mock”是轻量回调容器，不是完整 GoMock：它不解析参数 matcher，不维护期望调用序列，也不在析构时自动验证调用次数。调用者必须用 `Handler::set` 配置行为，并按需用 `Handler::call_count` 自行断言。

## 主要符号

- `type StorageResult<T> = Result<T, storage::Error>`：文件内结果别名，使三项接口与 `storage::Manager` 的错误类型保持一致。
- `pub struct MockManager`：核心替身。三个字段均为 `Handler<dyn FnMut(...) -> StorageResult<_> + Send>`：
  - `GetCPUCountOfNode` 接收 `storage::Context`，返回 `i32`；当前 `storage::Context` 在 `pkg/dxf/framework/storage/lib.rs` 中是 `()`。
  - `GetTaskByID` 接收上下文和 `i64` 任务 ID，返回拥有所有权的 `storage::proto::Task`。
  - `ModifyTaskByID` 接收上下文、任务 ID 和拥有所有权的 `storage::proto::ModifyParam`，成功值为 `()`。
- `pub type MockManagerMockRecorder = MockManager`：仅为命名兼容提供的类型别名；它没有 Go recorder 的独立状态或方法集合。
- `MockManager::EXPECT(&mut self) -> &mut MockManagerMockRecorder`：返回自身。与 Go 版返回独立 recorder 不同，Rust 调用方也可以直接访问公开 handler 字段，现有测试采用后者。
- `MockManager::ISGOMOCK(&self)`：无返回值、无副作用的兼容标记占位。
- 三个固有方法 `GetCPUCountOfNode`、`GetTaskByID`、`ModifyTaskByID`：分别调用字段的 `Handler::invoke`；传入的字符串只用于未配置期望时的 panic 消息。
- `impl storage::Manager for MockManager`：逐项调用同名固有方法，没有附加验证、转换或重试。
- `NewMockManager<C: ?Sized>(&C) -> MockManager`：忽略 controller 参数并返回 `Default` 实例；泛型引用允许移植代码传入任意 controller 占位对象。

## 执行流程

典型调用流程如下：

1. 测试调用 `NewMockManager(&controller)`；函数忽略 controller，得到三个 handler 都未安装闭包、计数均为零的 `MockManager`。
2. 测试在目标字段上调用 `set(Box::new(...))`。`Handler::set`（定义于 `pkg/dxf/framework/mock/lib.rs`）把闭包放入 `Mutex<Option<Box<F>>>`，并把调用计数重置为零。
3. 被测代码通过固有方法或 `storage::Manager` trait 方法调用替身。trait 实现只是进入对应的同名固有方法。
4. 固有方法调用 `Handler::invoke`。`invoke` 从 mutex 中暂时取出闭包、增加计数，在不持有 mutex 的情况下执行闭包，然后在没有新闭包被安装时放回旧闭包。
5. 闭包产生的成功值或 `storage::Error` 不经改写直接返回调用方。

例如独立回归测试给 `GetCPUCountOfNode` 安装闭包，验证传入的 `()` 上下文并返回 `8`，随后断言方法结果为 `8`、该 handler 的计数为 `1`。另外两个方法当前没有同文件 Rust 回归用例。

## 数据与状态

`MockManager` 自身只保存三个 handler，没有数据库连接、事务、任务缓存或 controller。每个 `Handler` 的状态在 crate 根中定义为：一个受 `Mutex` 保护的可选闭包，以及一个 `AtomicUsize` 调用计数。`MockManager` 派生 `Default`，因此新实例的全部闭包为 `None`。

参数均按 Rust 签名传递：当前上下文是零大小的 `()`；任务 ID 为复制的 `i64`；`ModifyParam` 被移动给闭包；`GetTaskByID` 返回拥有所有权的 `Task`。这与 Go 版返回 `*proto.Task`、接收 `*proto.ModifyParam` 的指针语义不同，闭包不能依赖 Go 指针身份或 `nil` 值。错误值没有本地状态，完全来自闭包。

调用计数由 handler 独立维护。重新 `set` 某一字段会把该字段计数清零，不影响另外两个字段。`EXPECT` 返回的仍是同一个 manager，因此不存在 recorder 与 mock 之间的双向引用。

## 依赖与调用关系

直接下游依赖有两项：

- `astersql_dxf_framework_storage`：提供 `Manager` trait、`Context`、`Error` 以及 `proto::Task` / `proto::ModifyParam`。`pkg/dxf/framework/storage/task_table.rs::Manager` 的三项签名与本文件的 trait 实现一一对应。
- `crate::Handler`：提供闭包安装、调用计数和实际派发。所有并发及未配置行为都由 `pkg/dxf/framework/mock/lib.rs::Handler` 决定。

crate 清单还依赖 planner、proto、taskexecutor-execute 和 sessionctx，但这些不是本文件的直接 import；它们服务于同一 mock crate 的其他模块。crate 根负责公开再导出，因此外部通常从 `astersql_dxf_framework_mock::NewMockManager` / `MockManager` 访问。

RustCodeGraph 的文件查询列出本文件 11 个符号；其 `explore` 结果确认 `storage_manager_mock_forwards_arguments_and_result` 调用 `NewMockManager` 与 CPU 方法。由于 `MockManager`、`NewMockManager`、`GetTaskByID` 在仓库中高度重名，图的未限定 callers 还会列出 `pkg/owner` 等无关符号；使用路径限定 `rg` 复核后，本文件当前唯一确定的 Rust 直接构造点仍是上述同 crate 测试。

## 错误处理与边界

- 三个业务错误路径都不捕获、不包装 `storage::Error`；handler 返回什么，调用方就收到什么。
- 若方法调用前未 `set` 对应 handler，`Handler::invoke` 会 panic，消息包含 `MockManager.<方法名>`。这属于测试配置错误，不会转换为 `StorageResult::Err`。
- handler mutex 中毒时，`set` 或 `invoke` 会以 `mock handler lock poisoned` panic。
- `Handler::invoke` 在执行用户闭包前先增加计数。因此闭包正常返回或在闭包内部 panic，该次尝试都已计数。
- 用户闭包 panic 时，执行流程不会到达恢复闭包的代码，handler 中保持 `None`；之后再次调用会按“未配置期望” panic，除非测试重新 `set`。
- `NewMockManager` 不验证传入 controller，`EXPECT` / `ISGOMOCK` 也不产生 GoMock 的期望校验；不能用 `ctrl.Satisfied()` 一类语义评价这个 Rust 替身。
- 当前 Rust 回归仅直接验证 `GetCPUCountOfNode` 的参数、返回值和计数；`GetTaskByID`、`ModifyTaskByID` 的参数所有权、成功值和错误透传尚无该 mock 的独立测试证据。

## 并发与资源生命周期

三个闭包要求 `Send`，`Handler` 又以 `Mutex` 和原子计数保存状态，因此 manager 可在满足类型自动 trait 的前提下跨线程共享。计数使用 `SeqCst`，读取、重置和递增具有全序原子语义。

`invoke` 会先在锁内 `take` 闭包，然后释放锁执行用户代码；`migration_aster_unit_test.rs::handler_does_not_hold_callback_lock_during_callback` 验证了闭包能够在执行期间为自身 handler 安装新闭包而不死锁，且执行结束不会覆盖这个新安装值。

不过，同一 handler 并不支持两个调用同时执行：第一个调用取出闭包后，第二个并发调用会看到 `None` 并 panic，而不是等待第一个调用归还闭包。不同字段拥有独立 mutex，可以并行派发。文件不创建线程、异步任务、通道或事务；闭包及其捕获资源随 `MockManager` 生命周期持有，替换闭包或丢弃 manager 时释放。

## 与 Go 版本的对应关系

Go 原件 `pkg/dxf/framework/mock/storage_manager_mock.go` 由 MockGen 从 `storage.Manager` 生成：`MockManager` 保存 `*gomock.Controller` 和独立 `MockManagerMockRecorder`；调用方法通过 `ctrl.Call` 查找匹配期望，recorder 方法通过反射登记方法、参数 matcher、返回值与次数约束。

Rust 版本保留了类型名、构造器名、`EXPECT`、`ISGOMOCK` 和三项接口，但实现策略明显不同：每个方法只对应一个可替换的 `FnMut`；controller 被忽略；recorder 是 manager 的别名；没有 matcher、调用顺序、次数上下限或自动满足性检查。Rust 的 `GetTaskByID` 返回 `Task` 而不是可空指针，`ModifyTaskByID` 接收值而不是指针，这是 `pkg/dxf/framework/storage/task_table.rs::Manager` 自身采用所有权类型的结果。

Go 侧多个 DXF 测试导入 `pkg/dxf/framework/mock`，但该包还生成了 scheduler、task executor 等多种替身；不能只凭包 import 认定它们使用本文件的 `MockManager`。路径搜索未找到 Go 测试直接调用 `NewMockManager`，因此 Go 生成文件与接口声明是语义对照依据，而不是当前可确认的专属测试覆盖。

## 扩展指南

若 `storage::Manager` 增加方法，应同步完成以下最小闭环：在 `MockManager` 添加签名一致的独立 `Handler` 字段；添加同名固有派发方法；在 trait impl 中转发；同步检查 Go 生成文件/源接口的变更；在独立的 `pkg/dxf/framework/mock/migration_aster_unit_test.rs` 增加成功、错误及关键参数透传用例。不要把测试写回生产源文件。

若要增强现有三项接口，优先改相应 handler 签名和固有方法，保持 `storage::Manager` 精确一致。涉及 `Task` / `ModifyParam` 时要明确值语义与 Go 指针语义的差异；若需要模拟 `nil`，必须先在 trait 层定义可表达的 Rust 类型，而不是在 mock 中私自约定。若希望实现真正的 GoMock recorder、并发同方法调用或调用顺序校验，应扩展通用 `Handler` 或引入独立期望模型，并同步审查所有 mock 模块，不能只在本文件局部伪装支持。

建议补充的独立测试包括：`GetTaskByID` 的 ID、任务和错误透传；`ModifyTaskByID` 的 ID、参数及错误透传；trait object/泛型边界调用；未配置 handler 的 panic 消息；同一 handler 并发调用的明确契约。兼容风险主要是 GoMock API 只部分兼容；正确性风险集中在参数所有权和 panic 边界；性能通常不是测试替身的主目标，但每次调用包含 mutex 与 `SeqCst` 原子操作。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/dxf/framework/mock` 列出目标 Rust/Go 文件及独立测试；`explore "pkg/dxf/framework/mock/storage_manager_mock.rs StorageManagerMock"` 返回目标文件完整源码、11 个符号、`NewMockManager` 与回归测试的调用关系。另执行了 `query MockManager`、`query NewMockManager`、`query GetTaskByID`、`node MockManager` 及 callers/callees 查询；查询暴露了同名歧义，结论再用限定路径搜索复核。
- 目标实现：`pkg/dxf/framework/mock/storage_manager_mock.rs`，核对了结果别名、结构字段、兼容 API、三个派发方法、trait impl 与构造器。
- 通用 handler：`pkg/dxf/framework/mock/lib.rs`，核对了 `set`、`call_count`、`invoke` 的锁、原子计数、重入替换与 panic 行为，以及模块导出关系。
- crate 边界：`pkg/dxf/framework/mock/Cargo.toml`，核对 crate 名称、lib 入口、Go 包元数据及对 storage crate 的路径依赖。
- 接口定义：`pkg/dxf/framework/storage/task_table.rs::Manager`、`pkg/dxf/framework/storage/lib.rs::Context`，核对三项 trait 签名及当前上下文别名。
- Go 对照：`pkg/dxf/framework/mock/storage_manager_mock.go` 和 `pkg/dxf/framework/storage/task_table.go::Manager`，核对 MockGen recorder/controller 行为、接口方法及指针语义。
- Rust 独立测试：`pkg/dxf/framework/mock/migration_aster_unit_test.rs::storage_manager_mock_forwards_arguments_and_result` 与 `handler_does_not_hold_callback_lock_during_callback`，分别核对 CPU 方法转发/计数和通用 handler 的回调锁生命周期。
- 使用面搜索：对 `pkg/dxf`、`tests` 和 Cargo manifests 执行 `rg`，确认直接 Rust 构造点、Go 测试 import、crate 消费方，并排除 RustCodeGraph 同名符号造成的无关调用者。

