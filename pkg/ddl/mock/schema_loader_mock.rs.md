# [`pkg/ddl/mock/schema_loader_mock.rs`](./schema_loader_mock.rs)

## 文件定位

本文件属于独立 crate `astersql-ddl-mock`，crate 根在 `pkg/ddl/mock/lib.rs` 中声明 `pub mod schema_loader_mock` 并公开再导出本文件的符号。`pkg/ddl/mock/Cargo.toml` 表明它只直接依赖 `astersql-ddl-systable`，后者提供生产接口 `SchemaLoader` 与错误类型 `SchemaLoaderError`；主 DDL crate 则在 `pkg/ddl/Cargo.toml` 中把本 crate 列为开发依赖。因此它是 DDL 测试基础设施，不参与生产 InfoSchema 的真实加载。

在完整运行链中，`pkg/session/runtime/normal_ddl_service.rs` 持有 `Arc<dyn astersql_ddl::SchemaLoader>`，成为 DDL owner 后把加载器交给 `JobScheduler::must_reload_schemas_with`；该方法反复调用 `SchemaLoader::reload`，直到成功或观察到取消。本文件实现同一个 trait，使测试可以在不访问真实元数据存储和 InfoSchema 缓存的情况下观察这条依赖注入边界。它本身不创建 DDL job、不推进 schema state、不执行 reorg，也不更新持久化元数据。

## 核心职责

本文件有两层职责。

第一层是共享的轻量 GoMock 风格控制器：`Controller::record` 保存期望，`Controller::call` 在全部尚未满足的期望中按方法名、参数个数和参数匹配器查找，命中后消费一次并返回预设结果，`Controller::verify` 检查是否仍有未消费期望。`Argument`、`Matcher` 和 `ReturnValue` 是这一控制器与同 crate 其他 mock（例如 system-table manager mock）共用的类型化协议。

第二层是 `SchemaLoader` 专用适配：`MockSchemaLoader` 实现生产 trait，并把 `reload()` 转换成控制器中的方法名 `"Reload"`、空参数列表和 `ReturnValue::Unit`；`MockSchemaLoaderRecorder::reload` 提供针对这一方法的类型化期望录制入口。这样测试既能使用具体 mock 的 `expect()`，又能把它擦除为 `Arc<dyn SchemaLoader>` 后走真实的选项和调度器接口。

## 主要符号

- `Argument`：控制器看到的实际参数，当前变体为 `Text(String)`、`Int(i64)` 和不暴露生产会话内容的 `Session`。`SchemaLoader::reload` 无参数，因此本文件的专用适配只传空向量；这些变体主要服务同 crate 的其他 mock。
- `Matcher`：期望参数规则。`Any` 接受任意同位置实参，`Exact(Argument)` 要求值完全相等。
- `ReturnValue`：控制器的统一返回值容器。`MockSchemaLoader` 只接受 `Unit`；`Job`、`Bytes`、`Int`、`Bool` 和 `ManagerError` 是共享控制器供其他 mock 使用的变体。
- `MockError(pub String)`：录制失败结果、意外调用和未满足期望的测试错误；实现 `Display` 与 `std::error::Error`。
- `ExpectedCall`：私有的一次性期望，保存静态方法名、参数匹配器列表和 `Result<ReturnValue, MockError>`。
- `ControllerState`：私有可变状态，`expected` 保存待消费期望，`calls` 保存所有已发起调用（包括随后匹配失败的调用）。当前没有公开读取 `calls` 的 API。
- `Controller`：`Arc<Mutex<ControllerState>>` 的可克隆句柄。克隆后仍操作同一组期望和调用记录。
- `new_mock_schema_loader(Controller) -> MockSchemaLoader`：构造函数；控制器被克隆给 recorder，原句柄存入 mock，确保录制与执行共享状态。
- `MockSchemaLoader::expect(&self) -> &MockSchemaLoaderRecorder`：取得类型化 recorder；`is_mock()` 是与 Go mock 标记约定相呼应的空方法。
- `impl ddl_systable::SchemaLoader for MockSchemaLoader`：生产接口实现。`reload()` 调用控制器并把 `MockError` 的文本转换为 `SchemaLoaderError`；只有 `ReturnValue::Unit` 表示成功，其他变体返回固定错误 `Reload returned the wrong type`。
- `MockSchemaLoaderRecorder::reload(Result<(), MockError>)`：录制一次无参数 `Reload`。每调用一次只增加一个可消费一次的期望。

文件没有模块级常量、条件编译项、异步函数或后台任务。

## 执行流程

典型测试流程如下：

1. 测试创建 `Controller::default()`，再调用 `new_mock_schema_loader(controller.clone())`。mock、recorder 和测试保留的句柄共享同一个互斥状态。
2. 测试通过 `loader.expect().reload(Ok(()))` 录制一次成功，或传入 `Err(MockError(...))` 录制一次加载失败。也可以直接调用 `Controller::record` 构造错误返回类型等负向场景。
3. 被测代码通过具体类型或 `dyn SchemaLoader` 调用 `reload()`。适配层向 `Controller::call` 提交 `"Reload"` 和空参数。
4. `Controller::call` 先把调用写入 `calls`，再从 `expected` 开头向后搜索第一个完整匹配项。匹配不要求录制顺序；方法名、参数数量或任一参数不符都会跳过该期望。
5. 找到后以 `remove(position)` 消费该期望。预设 `Err(MockError)` 会被转换成生产 `SchemaLoaderError`；预设 `Unit` 返回成功；其他返回类型产生类型错误。没有匹配项时返回包含方法名和实参调试值的 `unexpected call` 错误。
6. 测试最后显式调用 `Controller::verify()`；只有 `expected` 为空才成功。`pkg/ddl/mock/mock_aster_unit_test.rs` 验证了无序匹配、错误保留、错误返回类型、漏调用以及通过生产 trait object 调用；`pkg/ddl/schema_loader_contract_aster_unit_test.rs` 进一步验证 mock 能通过 `with_schema_loader` 进入生产选项链。

## 数据与状态

所有可变状态集中在 `ControllerState`。`expected: Vec<ExpectedCall>` 同时承担期望集合和稳定搜索顺序：调用匹配第一个符合条件的未消费期望，随后从向量删除它；因此同方法同参数的多个期望会按录制先后逐个消费，而不同方法或不同参数的期望默认可乱序命中。删除中间元素是线性移动，但测试期望通常很少，本实现没有为高吞吐生产负载优化。

`calls` 追加每次尝试及其参数克隆，匹配失败也会留下记录。不过它是私有字段且当前没有断言或读取接口，所以可观察的校验结果仍由调用当下的错误和剩余 `expected` 决定。`ExpectedCall::result` 在命中时被移动出去；同一条期望不能重复使用，需要多次调用时必须录制多条。

`MockSchemaLoader` 同时持有执行端 `Controller` 和 recorder。recorder 再持有一个控制器克隆，而不是反向引用 mock，因此没有引用环。`Argument`、`Matcher`、`ReturnValue` 与 `MockError` 都可克隆，方便测试构造和比较；实际期望结果在消费时仍按所有权移动。

## 依赖与调用关系

上游测试调用关系为：

- `pkg/ddl/mock/mock_aster_unit_test.rs` → `new_mock_schema_loader` / `MockSchemaLoader::expect` / `SchemaLoader::reload` / `Controller::verify`，覆盖本文件的控制器和 trait 适配语义。
- `pkg/ddl/schema_loader_contract_aster_unit_test.rs` → `new_mock_schema_loader` → `Arc<dyn SchemaLoader>` → `with_schema_loader`，覆盖跨 crate 的生产注入契约。
- `pkg/ddl/mock/lib.rs` 公开再导出本文件全部公开符号；`pkg/ddl/Cargo.toml` 以 dev-dependency 引用该 crate。

下游依赖关系为：

- `ddl_systable::SchemaLoader` 定义 `reload(&self) -> Result<(), SchemaLoaderError>`，并要求实现者 `Send + Sync`。
- `ddl_systable::SchemaLoaderError::new` 接收字符串诊断，本文件用它包装控制器错误及返回类型错误。
- Rust 标准库的 `Arc` 让多个句柄共享状态，`Mutex` 串行化录制、调用和验证，`fmt` 支持错误展示。

生产侧对应调用链是 `pkg/session/runtime/normal_ddl_service.rs` → `JobScheduler::must_reload_schemas_with` → `SchemaLoader::reload`。mock 不由生产服务自动选择，也不负责该链中的重试、休眠或取消判断；这些策略属于调度器。

## 错误处理与边界

`Controller::call` 对意外调用返回 `MockError`，不会提供默认值，也不会消费仅部分匹配的期望。匹配到预设错误时，该期望已经被消费，错误原样返回；这使“调用发生但下游失败”与“调用从未发生”能够区分。`verify` 只报告剩余期望数量，不列出具体期望，也不检查 `calls` 内容。

`MockSchemaLoader::reload` 把控制器错误降为仅保留文本的 `SchemaLoaderError`，因为生产 trait 的错误类型与 `MockError` 不同。统一控制器允许任意 `ReturnValue`，所以适配层必须做运行时类型检查；非 `Unit` 会返回 `Reload returned the wrong type`。正常的 `expect().reload(...)` 会自动映射到 `Unit`，只有直接使用低层 `record` 才容易制造类型不符。

互斥锁若因持锁线程 panic 而中毒，`record`、`call` 和 `verify` 都通过 `expect("mock controller mutex poisoned")` 再次 panic；本文件没有恢复策略。它也没有 `Drop` 时自动验证，测试必须显式调用 `verify()` 才能发现从未发生的期望。忽略一次 `reload()` 返回的意外调用错误、且没有剩余期望时，单独的 `verify()` 不会再次报告该意外调用。

## 并发与资源生命周期

`Controller` 的 `Arc<Mutex<_>>` 使克隆句柄可跨线程共享；其字段类型也使 `MockSchemaLoader` 满足生产 trait 的 `Send + Sync` 约束。每次 `record`、`call` 或 `verify` 在一个互斥临界区内完成，因此期望不会被两个并发调用重复消费，调用记录与期望消费也不会发生数据竞争。

并发调用的实际锁获取顺序是不确定的；当多个期望都能匹配时，先拿到锁的调用先消费向量中第一个匹配项。因此测试不应借助线程调度推断严格调用顺序。控制器没有条件变量、通道、任务、超时或取消状态，调度器重试期间的等待和取消仍由 `JobScheduler::must_reload_schemas_with` 管理。

资源随最后一个 `Arc` 句柄释放而释放；recorder 与 mock 共享控制器但不形成环。文件不持有数据库连接、事务、锁租约、DDL job 或 InfoSchema 快照。方法返回前 `MutexGuard` 被释放，生产代码不会持续持有 mock 锁。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/mock/schema_loader_mock.go`。Go 文件由 MockGen 针对 `pkg/ddl/ddl.go` 的 `SchemaLoader.Reload() error` 生成：`NewMockSchemaLoader` 对应 `new_mock_schema_loader`，`EXPECT()` 对应 `expect()`，`ISGOMOCK()` 对应 `is_mock()`，基础 `Reload()` 与 recorder 的 `Reload()` 分别对应 Rust trait 实现和 `MockSchemaLoaderRecorder::reload`。

核心语义保持一致：默认不强制调用顺序，未声明的调用失败，调用结果由测试预设，且 `Reload` 没有参数。Go 的真实使用证据在 `pkg/ddl/job_scheduler_test.go::TestMustReloadSchemas`：分别录制直接成功、失败后重试成功、失败并取消三种结果，验证 owner schema 重载门槛；Rust 调度器对应行为由 `pkg/ddl/job_scheduler_test.rs::must_reload_schemas_succeeds_retries_and_stops_when_cancelled` 验证，不过该测试使用更简单的 `SequenceLoader`，本 mock 的独立测试集中在前述两个 `*_unit_test.rs` 文件。

Rust 版本不是 GoMock DSL 的完整复刻。Go recorder 返回 `*gomock.Call`，可继续配置 `Return`、次数、`After`/`InOrder` 和回调，且 `gomock.Controller` 与 `testing.T` 集成完成期望校验；Rust recorder 直接接收最终 `Result`，一次录制只代表一次调用，没有顺序约束、次数 DSL、回调或析构自动校验。Rust 还用统一 `ReturnValue` 做运行时返回类型检查，并用 `Mutex` 中毒 panic 表达内部状态损坏。这些差异是当前代码事实，扩展时不应假定完整 GoMock 能力已经存在。

## 扩展指南

若 `SchemaLoader` 新增方法，应同步修改 `MockSchemaLoader` 的 trait 实现和 `MockSchemaLoaderRecorder`，为每个参数选择或新增不会泄漏生产对象内部细节的 `Argument` 表示，并在返回适配层严格检查 `ReturnValue`。同时扩展 `pkg/ddl/mock/mock_aster_unit_test.rs`，至少覆盖成功、预设错误、参数不匹配、错误返回类型和遗漏调用；若影响依赖注入，再更新 `pkg/ddl/schema_loader_contract_aster_unit_test.rs`。Rust 源与测试逻辑必须继续分离，不要把单元测试内嵌到本文件。

若要增加调用次数、顺序、回调或更详细诊断，优先扩展 `ExpectedCall`/`Controller` 的通用协议并检查 `systable_manager_mock.rs` 的兼容性，因为两个 mock 共享这些类型。严格顺序不应破坏当前“搜索所有未满足期望”的默认行为；可考虑显式的可选约束。新增调用记录查询时，应返回快照或只读摘要，避免把内部锁暴露给测试。

兼容风险主要是方法名字符串和返回类型映射：`"Reload"` 必须与 recorder 完全一致，生产 trait 错误必须继续保留可诊断文本。并发扩展需保持一次期望最多消费一次，不能在持锁期间执行用户回调以免重入死锁。性能风险当前很低；只有大量期望时，`Vec::position` 加 `Vec::remove` 的线性成本才值得重新设计。任何生产接口变化还需同步 Go 对照与 owner 重试测试，不能只让 mock 编译通过。

## 验证依据

- RustCodeGraph `status`：索引可用，覆盖本仓库 Rust 与 Go 文件。
- RustCodeGraph `node --file pkg/ddl/mock/schema_loader_mock.rs`：读取本文件 1–220 行，核对全部枚举、结构体、函数、impl 与无条件编译事实。
- RustCodeGraph `query MockSchemaLoader --kind struct`、`query new_mock_schema_loader --kind function`：定位 Rust/Go 同名结构及 Rust 构造入口。
- RustCodeGraph `query SchemaLoader --kind trait` 与 `node pkg/ddl/systable/schema_loader.rs::SchemaLoader`：确认生产签名及 `Send + Sync` 约束。
- RustCodeGraph `node --file pkg/ddl/job_scheduler.rs`：确认 `must_reload_schemas_with` 的成功、取消、重试与休眠边界。`callers`/`callees` 精确查询未返回可用边文本，因此调用点又由下列直接引用检索核实，未据此臆造图边。
- crate/模块证据：`pkg/ddl/mock/Cargo.toml`、`pkg/ddl/mock/lib.rs`、`pkg/ddl/Cargo.toml`、根 `Cargo.toml`。
- Rust 调用与测试证据：`pkg/ddl/mock/mock_aster_unit_test.rs`、`pkg/ddl/schema_loader_contract_aster_unit_test.rs`、`pkg/ddl/job_scheduler.rs`、`pkg/ddl/job_scheduler_test.rs`、`pkg/session/runtime/normal_ddl_service.rs`。
- Go 对照证据：`pkg/ddl/mock/schema_loader_mock.go`、`pkg/ddl/ddl.go::SchemaLoader`、`pkg/ddl/job_scheduler.go::mustReloadSchemas`、`pkg/ddl/job_scheduler_test.go::TestMustReloadSchemas`。
- 本任务是纯文档分析，未运行 Cargo；交付前仅执行任务规定的 11 章节结构校验，并人工检查唯一新增生产物、源码链接、事实限定和独立测试指引。
