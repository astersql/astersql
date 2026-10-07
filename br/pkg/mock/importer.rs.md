# `br/pkg/mock/importer.rs`

## 文件定位

`br/pkg/mock/importer.rs` 属于 Cargo 包 `astersql-br-pkg-mock`；该包的入口是 [`br/pkg/mock/lib.rs`](lib.rs)，入口以 `pub mod importer` 装载本文件，再通过 `pub use importer::*` 平铺导出其公开符号。它不是线上 ImportKV 客户端，而是供 Rust 测试隔离 ImportKV gRPC 边界的可编程 mock。文件头明确其来源是同目录的 Go MockGen 产物 [`br/pkg/mock/importer.go`](importer.go)，覆盖 Go 的 `ImportKVClient` 与 `ImportKV_WriteEngineClient` 两组接口。

[`br/pkg/mock/Cargo.toml`](Cargo.toml) 将该目录声明为独立 library crate，并以 `package.metadata.porting.go-package = "br/pkg/mock"` 记录 Go 包来源。manifest 没有外部依赖：请求、响应、上下文、元数据、控制器和流 trait 均来自 [`br/pkg/mock/stubs.rs`](stubs.rs)，因此这里模拟的是测试协议，不会建立真实 gRPC 连接或触发真实导入。

## 核心职责

本文件提供两层 mock：

- `MockImportKVClient` 模拟 ImportKV 客户端的一元 RPC 和 `WriteEngine` 流创建入口。`CleanupEngine`、`CloseEngine`、`CompactCluster`、`GetMetrics`、`GetVersion`、`ImportEngine`、`OpenEngine`、`SwitchMode`、`WriteEngineV3` 都把调用装箱后交给 `Controller`；`WriteEngine` 返回实现 `WriteEngineClient` 的 trait object。
- `MockImportKV_WriteEngineClient` 模拟已经创建的 WriteEngine 客户端流，覆盖 `Send`、`RecvMsg`、`SendMsg`、`Header`、`Trailer`、`Context`、`CloseSend` 与 `CloseAndRecv`。
- 两个配套 recorder 将 `EXPECT()` 调用转换成 `Call`，让测试登记预期返回值或错误。mock 本身不验证导入引擎状态机，也不产生指标、版本或 SST 数据；这些结果完全由测试预设。

## 主要符号

- `take_response<Resp>`：内部通用返回值解包器。若首槽已经是 `Error`/`Option<Error>`，按“仅错误”形态处理；否则尝试把首槽 downcast 为 `Resp`，缺失或类型不符时使用 `Resp::default()`，最后从剩余槽提取错误。
- `MockImportKVClient` / `MockImportKVClientMockRecorder`：共享同一个可克隆 `Controller` 的客户端与预期记录器。`NewMockImportKVClient` 同时构造二者，`EXPECT` 暴露 recorder。
- `MockImportKVClient::call_unary<Req, Resp>`：一元 RPC 的统一实现；按 `Context`、请求、零到多个 `CallOption` 的顺序构造参数，再调用 `Controller::Call` 和 `take_response`。
- `MockImportKVClient::WriteEngine`：流创建的特殊分支。成功返回 `Box<dyn WriteEngineClient>`；没有返回值或返回值类型不匹配时使用 `NilWriteEngineClient`；错误槽仍通过 `take_error` 传播。
- `MockImportKV_WriteEngineClient` / `MockImportKV_WriteEngineClientMockRecorder`：流客户端及 recorder。该结构实现 `WriteEngineClient` trait，各 trait 方法只转发到同名固有方法，确保两种调用形式进入同一套控制器逻辑。
- `NewMockImportKV_WriteEngineClient`：创建流 mock，并让客户端与 recorder 共享控制器。
- 两个 recorder 的私有 `record`：用稳定的方法名及 `MockImportKVClient.<method>` 或 `MockImportKV_WriteEngineClient.<method>` 描述登记预期。公开 recorder 方法只选择方法名；形参以 `_` 开头，当前实现不把 matcher 参数传给 `RecordCallWithMethodType`。

本文件没有模块级常量、枚举、条件编译项或后台任务。公开 API 使用 Go 风格大写命名，以保持移植接口可辨识；crate 根对相应 lint 做了集中豁免。

## 执行流程

一元调用（例如 `ImportEngine`）的主流程如下：

1. 测试通过 `client.EXPECT().ImportEngine(...)` 取得 recorder 并登记方法预期；recorder 调用 `Controller::Helper` 后记录方法名。
2. 被测代码调用 `client.ImportEngine(context, request, options)`；该包装器进入 `call_unary`。
3. `call_unary` 调用 `Helper`，依次装箱上下文、请求和所有 `CallOption`，以方法字符串查询 `Controller::Call`。
4. `take_response` 将控制器返回槽还原成 `Result<Response>`：兼容 Go 的 `(response, error)` 布局，也兼容测试只录制错误的布局。

流调用分成创建和使用两段。`MockImportKVClient::WriteEngine` 先按上下文及 options 调控制器，取得流 trait object或错误；随后上层可调用流对象的 `Send` 发送克隆后的强类型请求，以 `CloseSend` 只关闭发送侧，或以 `CloseAndRecv` 获取最终 `WriteEngineResponse`。`Header`/`Trailer`/`Context` 提供测试可控的附属数据。`RecvMsg` 与 `SendMsg` 接受 `dyn Any`，但只向控制器传递 `()` 占位值，因此当前实现只适合按方法名和返回错误编排，不能依据实际消息内容匹配。

## 数据与状态

持久状态只有 `Controller` 及 recorder 持有的控制器克隆。所有请求、响应与 RPC 附属值都是调用期临时对象：一元请求和 options 被移动后装入 `Vec<Box<dyn Any + Send>>`；`Send` 会克隆 `WriteEngineRequest` 后装箱；返回槽也用 `Any` 承载并在运行时 downcast。

客户端不保存“引擎是否打开”“是否已发送”“发送侧是否关闭”等协议状态，因而调用顺序、不变量和响应内容由 `Controller` 的预期表表达。`Metadata`、`Context` 和各响应在未提供可识别返回值时可能退回 `Default`；流创建还会退回 `NilWriteEngineClient`。这些默认值是 mock 的容错行为，不代表真实 ImportKV 服务成功执行了动作。

## 依赖与调用关系

直接下游全部来自 `crate::stubs`：

- `Controller::{Helper, Call, RecordCallWithMethodType}` 和 `Call` 实现预期登记与调用分派。
- `take_error`、`Result`、`Error` 负责 Go 风格尾部错误槽到 Rust `Result` 的转换。
- `Context`、`CallOption`、`Metadata` 以及各 RPC request/response 是轻量本地边界类型。
- `WriteEngineClient` 定义流接口，`NilWriteEngineClient` 提供未录制或类型不匹配时的默认实现。

直接上游由 [`br/pkg/mock/lib.rs`](lib.rs) 装载并重导出。RustCodeGraph 的文件关系显示本文件被 7 个 Rust 文件使用，其中包括 [`br/pkg/mock/importer_test.rs`](importer_test.rs)、`br/pkg/stream/crr/internal/checkpoint/` 下的 checkpoint 测试，以及 [`br/pkg/task/restore_lifecycle_test.rs`](../task/restore_lifecycle_test.rs)；精确符号搜索还确认 [`br/pkg/mock/parity_test.rs`](parity_test.rs) 构造这两个 mock，以校验公共 mock 表面和流方法。这里出现于测试依赖链而非生产导入主链；真实 ImportKV 网络客户端位于其他实现层，不应由本文件替代。

## 错误处理与边界

`take_response` 和 `WriteEngine` 都先检查首槽是否直接携带 `Error` 或 `Option<Error>`，从而支持只登记错误而不先登记响应的测试。正常 `(response, error)` 形态中，先移出响应，再由 `take_error` 检查余下槽。独立测试 `importer_stream_creation_propagates_recorded_error` 证明 `WriteEngine` 会原样传播录制的 `"write stream unavailable"`；`importer_stream_response_methods_propagate_recorded_error` 证明 `Header` 和 `CloseAndRecv` 同样传播对应错误，并消费完控制器预期。

需要特别注意三类静默降级边界：`take_response` 在响应缺失或 downcast 失败时返回 `Default`；`WriteEngine` 在流缺失或 downcast 失败时返回 `NilWriteEngineClient`；`Context` 与 `Trailer` 依赖 `take_one` 的本地桩语义。若测试必须发现响应类型录错，不能只依赖这里的默认行为，应在控制器或测试断言中显式约束。`RecvMsg`/`SendMsg` 丢弃实际 `Any` 内容也是已知能力边界。

## 并发与资源生命周期

文件自身不创建线程、异步任务、锁、通道、事务或网络资源。客户端和 recorder 各持有一个 `Controller` 克隆，使预期登记与实际调用汇聚到同一控制器状态；具体共享与同步保证取决于 [`br/pkg/mock/stubs.rs`](stubs.rs) 的 `Controller` 实现。本文件没有实现流状态机或析构清理：`CloseSend` 和 `CloseAndRecv` 只是可录制的方法调用，不会真实关闭 socket；对象释放也没有额外 `Drop` 行为。

测试应为独立场景创建独立 `Controller`，并在末尾检查 `remaining() == 0`，避免预期跨场景泄漏。若未来让同一 mock 跨线程使用，必须先验证 `Controller`、trait object 及其返回槽的线程安全约束，而不能从参数中的 `Send` 界限推断整个 mock 可安全并发共享。

## 与 Go 版本的对应关系

[`br/pkg/mock/importer.go`](importer.go) 是 MockGen 生成的基准。Rust 保留了两个 mock、两个 recorder、两个构造函数、`EXPECT` 入口，以及一元方法、流创建方法和流客户端方法集合。`Vec<CallOption>` 对应 Go 的可变参数 `...grpc.CallOption`；`Result<T>` 对应 Go 的 `(T, error)`；`Box<dyn WriteEngineClient>` 对应 Go 的 `ImportKV_WriteEngineClient` 接口值；`Any` 承担 Go `any` 的动态参数角色。

两者并非逐字等价。Go recorder 把 matcher 参数和反射得到的方法类型传入 gomock，而当前 Rust recorder 最终向 `RecordCallWithMethodType` 传空参数列表，并用格式化字符串描述所属 mock。Go 类型断言失败会产生 `nil` 接口或指针，Rust 为满足具体返回类型常回退到 `Default` 或 `NilWriteEngineClient`。Go 的 `RecvMsg`/`SendMsg` 把原始消息传给控制器，Rust 当前传 `()` 占位。因此，方法集合和错误编排语义已移植，但参数匹配精度及错误响应下的零值语义应以 Rust `stubs` 和独立测试为准，不能直接套用 Go gomock 假设。

## 扩展指南

新增 ImportKV 一元 RPC 时，应同时：在 `stubs.rs` 定义或接入 request/response；在 `MockImportKVClient` 添加调用 `call_unary` 的包装；在 recorder 添加同名方法；在 `lib.rs` 需要时重导出边界类型；并在独立的 `importer_test.rs` 或 `parity_test.rs` 增加成功、仅错误、`(response, error)` 和 options 顺序测试。不要把测试嵌入本生产源文件。

扩展流协议时，应同步 `WriteEngineClient` trait、固有方法、trait 转发、recorder 和 Go 方法集合。若要支持消息内容 matcher，应改变 `RecvMsg`/`SendMsg` 的装箱策略并增加类型与生命周期测试；这是兼容性敏感改动，因为现有控制器可能依赖 `()` 占位。若要把 downcast 失败改为硬错误，也需评估已有测试对默认响应或 `NilWriteEngineClient` 的依赖。

性能通常不是该测试 mock 的主风险，但 `Any` 装箱、options 逐项移动以及 `WriteEngineRequest` 克隆会随调用次数产生分配/复制成本。正确性风险更集中在方法字符串拼写、返回槽顺序、recorder 与实现方法集合漂移，以及误把 mock 的默认成功当作真实服务行为。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter br/pkg/mock` 确认 `importer.rs`、Go 对照、独立测试和 crate 入口均被索引；`node --file br/pkg/mock/importer.rs --offset 1 --limit 500` 读取到完整 458 行及 59 个符号；`explore` 确认 `record` 被本文件各 RPC 包装调用，并识别 `importer_test.rs` 对 `WriteEngine`、`Header`、`CloseAndRecv` 的调用。精确 `callers NewMockImportKVClient` 查询在本环境未完成，因此上游范围又用仓库文本搜索核对，未据此虚构额外调用边。
- 源文件：[`br/pkg/mock/importer.rs`](importer.rs)，核对完整类型、函数、impl、返回槽处理和 trait 转发。
- crate 边界：[`br/pkg/mock/Cargo.toml`](Cargo.toml) 与 [`br/pkg/mock/lib.rs`](lib.rs)，核对包名、Go 包映射、无外部依赖、模块装载、重导出及独立测试挂载。
- Go 对照：[`br/pkg/mock/importer.go`](importer.go)，核对 MockGen 来源、方法集合、可变参数、返回值和 recorder 行为。
- Rust 测试：[`br/pkg/mock/importer_test.rs`](importer_test.rs) 核对流创建及流响应方法的错误传播；[`br/pkg/mock/parity_test.rs`](parity_test.rs) 核对两类 mock 的构造和公共方法使用。
- 本任务只新增说明文档，不修改 Rust、Go 或 Cargo；按照计划不运行 Cargo。结构验证要求本文恰有十一项固定二级标题。
