# `pkg/domain/sqlsvrapi/mock/server_mock.rs`

## 文件定位

该文件属于 `astersql-domain-sqlsvrapi-mock` crate，是 `pkg/domain/sqlsvrapi/server.rs` 中 `Server` trait 的 Rust 测试替身。crate 入口 `pkg/domain/sqlsvrapi/mock/lib.rs` 通过 `pub mod server_mock` 挂载本文件，并在 `domain::sqlsvrapi::mock` 下再导出其公开项，以模拟 Go 包路径。它不是 SQL 请求的生产实现，也不启动服务器；调用者在测试中配置预期，让被测代码只依赖 `Arc<dyn Server>` 接口。

`pkg/domain/sqlsvrapi/mock/Cargo.toml` 将该 crate 声明为 workspace 成员 `astersql-domain-sqlsvrapi-mock`，直接依赖 `mockall = "0.13"` 和相邻的 `astersql-domain-sqlsvrapi`。后者经 `lib.rs` 再导出 `domain::sqlsvrapi` 与 `owner`，供本文件引用真实 trait 和返回类型。

## 核心职责

1. `mockall::mock!` 生成公开的 `MockServer`，并令其实现真实的 `Server` trait（`server_mock.rs:34-46`）。
2. 完整保留 `Server` 的三个可观察入口：当前实例运行时 `GetRuntime`、跨 keyspace 句柄获取 `AcquireKSRuntime`、DDL Owner 管理器 `GetDDLOwnerMgr`。
3. 让测试通过宏生成的 `expect_GetRuntime`、`expect_AcquireKSRuntime`、`expect_GetDDLOwnerMgr` 配置参数谓词、调用次数、返回值或错误；`pkg/domain/sqlsvrapi/mock/migration_aster_unit_test.rs` 和 `pkg/dxf/framework/dxfutil/util_test.rs` 展示了这些实际用法。
4. 提供 `ISGOMOCK`、`EXPECT` 和 `NewMockServer` 三个 GoMock 命名兼容辅助入口，降低 Go 测试迁移时的调用形态差异。真正的期望录制与校验由 `mockall` 生成代码负责，而不是这三个辅助函数自己实现。

## 主要符号

- `mockall::mock! { pub Server {} ... }`：宏输入中的 `Server` 是生成类型名的词根，产物是 `MockServer`。宏内 `impl ServerTrait for Server` 使生成对象满足 `crate::domain::sqlsvrapi::Server: Send + Sync`。
- `MockServer::GetRuntime(&self) -> Arc<dyn Runtime>`：与 `pkg/domain/sqlsvrapi/server.rs:92-94` 的 trait 签名一致，返回当前实例的 keyspace 运行时视图。
- `MockServer::AcquireKSRuntime(&self, targetKS: String, holderID: String) -> Result<Arc<dyn KSRuntimeHandle>, SqlSvrError>`：与 `server.rs:96-105` 一致；两个字符串分别标识目标 keyspace 和持有者，成功值是必须由上层最终 `Release` 的句柄，失败值是线程安全的装箱错误。
- `MockServer::GetDDLOwnerMgr(&self) -> Arc<dyn Manager>`：返回 DDL Owner 管理器 trait object，测试可据此观察选主/调度依赖。
- `MockServer::ISGOMOCK(&self)`：空标记方法，仅保留 GoMock 生成物的识别入口；它不改变状态，也不参与 mockall 校验。
- `MockServer::EXPECT(&mut self) -> &mut Self`：返回自身的可变引用。Rust 调用方仍通过 `expect_*` 方法配置 mockall 期望；它不像 Go 的 `EXPECT()` 那样返回独立 recorder 对象。
- `NewMockServer<C: ?Sized>(_ctrl: &C) -> MockServer`：接受任意可能为动态大小类型的控制器引用以兼容 `NewMockServer(ctrl)` 形态，但参数刻意未使用，实际只调用宏生成的 `MockServer::new()`。

本文件没有模块级常量、独立数据结构、条件编译项或手写错误类型；`MockServer` 及其期望上下文均由宏生成。

## 执行流程

典型测试流程如下：

1. 调用 `NewMockServer(&())` 或直接调用 `MockServer::new()` 创建一个尚未配置期望的 mock。前者只提供 Go 风格构造名，二者最终走同一个宏生成构造器。
2. 在可变 mock 上调用 `expect_*`。例如迁移测试为 `GetRuntime` 和 `GetDDLOwnerMgr` 各配置一次返回，为 `AcquireKSRuntime` 分别配置 `analytics` 成功分支与 `missing` 错误分支（`migration_aster_unit_test.rs:259-289`）。
3. 被测代码经 `Server` trait 调用三个方法。mockall 按方法、参数谓词和剩余调用次数选择匹配期望，然后执行 `return_const`/`returning` 闭包并返回 `Arc` 或 `Result`。
4. 测试显式调用 `checkpoint()` 时立即验证并清空当前期望；若未显式检查，mockall 还会在 mock 的析构路径验证未满足的期望。迁移测试在所有预期调用完成后执行 `server.checkpoint()`。
5. 对跨 keyspace 成功值，`MockServer` 只返回 `Arc<dyn KSRuntimeHandle>`；句柄的释放发生在使用它的上层逻辑。`pkg/dxf/framework/dxfutil/util_test.rs:215-244` 验证返回的释放闭包最终恰好调用一次 `Release`。

## 数据与状态

本文件手写部分没有业务字段。状态由 mockall 在生成的 `MockServer` 内部维护，核心是每个 trait 方法的期望集合、匹配规则、调用次数和返回动作。`EXPECT` 借用 `&mut self`，因此配置阶段受 Rust 独占可变借用约束；方法调用阶段只需 `&self`，满足 `Server` trait 的共享引用接口。

跨线程/共享边界通过真实 trait 的类型约束表达：`Server`、`Runtime` 和 `Manager` 均以 `Arc<dyn ...>` 返回，`Server` 本身要求 `Send + Sync`；`SqlSvrError` 是 `Box<dyn Error + Send + Sync + 'static>`。这些包装允许测试将 mock 作为 `Arc<dyn Server>` 注入，但不代表本文件管理返回对象的业务生命周期。

`NewMockServer` 的 `_ctrl` 不被保存，因此 Rust mock 与传入控制器没有所有权或生命周期关系。这一点与 Go `MockServer` 持有 `*gomock.Controller` 的数据模型不同。

## 依赖与调用关系

上游装配与调用证据：

- `pkg/domain/sqlsvrapi/mock/lib.rs` 声明 `pub mod server_mock`，并通过 `domain::sqlsvrapi::mock` 再导出 `server_mock::*`。
- `pkg/domain/sqlsvrapi/mock/migration_aster_unit_test.rs` 调用 `NewMockServer`，覆盖三个 trait 方法及成功/错误结果。
- `pkg/dxf/framework/dxfutil/util_test.rs` 通过 dev-dependency `sqlsvrapimock_dependency` 使用 `MockServer::new()`；它把 mock 转为 `Arc<dyn Server>` 后注入 session，验证 `AcquireTaskRuntime` 的当前 keyspace、跨 keyspace和获取失败分支。
- workspace 和多个迁移 crate 的 Cargo manifest 引用 `astersql-domain-sqlsvrapi-mock`；当前仓库中能直接定位到 `MockServer` Rust 调用的独立文件是上述两个测试文件。其他 manifest 依赖不能单独证明其当前已调用本类型。

下游依赖：

- `crate::domain::sqlsvrapi::{Server, Runtime, KSRuntimeHandle, SqlSvrError}` 定义被实现的契约和方法返回类型，真实定义在 `pkg/domain/sqlsvrapi/server.rs`。
- `crate::owner::Manager` 定义 DDL Owner 返回接口。
- `std::sync::Arc` 承担 trait object 的共享所有权。
- `mockall` 生成构造器、`expect_*`、匹配/次数/返回动作、`checkpoint` 与析构校验逻辑。

RustCodeGraph 将 `server_mock.rs` 识别为含 7 个符号的已索引文件，但文件级结果显示 `used by 0 files`，且对 `mockall` 宏生成调用边未解析出可用关系；因此上述调用关系以索引内符号查询配合直接 Rust 引用搜索为依据，不将“0”误解为没有测试使用。

## 错误处理与边界

本文件不构造领域错误。`AcquireKSRuntime` 原样暴露 `Result<Arc<dyn KSRuntimeHandle>, SqlSvrError>`，具体成功值和错误由测试配置的返回闭包产生；迁移测试用 `"keyspace missing"` 验证错误可原样传播，DXF 测试用 `"ks runtime not found"` 验证被测逻辑的获取失败路径。

没有匹配期望、调用次数超限或检查时仍有未满足期望时，mockall 会令测试失败（通常表现为 panic）；这属于测试契约违规，而非 `SqlSvrError` 返回路径。`GetRuntime` 与 `GetDDLOwnerMgr` 的签名没有 `Result`，因此要模拟正常调用只能返回相应 `Arc`；若要模拟异常退出，只能在返回闭包中 panic，不能凭空增加生产 trait 不具备的错误分支。

`AcquireKSRuntime` 的 `String` 参数按值传入，参数匹配会观察完整的 `targetKS` 与 `holderID`。扩展测试时应同时校验二者，避免只验证目标 keyspace 而漏掉持有者身份。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。并发能力来自真实 `Server: Send + Sync` 契约、`Arc` 返回值以及 mockall 为该 trait 实现生成的内部同步机制；文档不能据此推断生产服务器的调度策略。

`MockServer` 生命周期由持有它的测试或 `Arc<dyn Server>` 决定。期望应在对象被共享前完成配置；共享后通过 `&self` 调用。显式 `checkpoint()` 可在对象尚未析构时验证并清空期望，适合分阶段测试；否则由 drop 进行最终校验。

资源责任边界尤其重要：`GetRuntime` 返回共享运行时，无释放方法；`AcquireKSRuntime` 返回的 `KSRuntimeHandle` 按 `server.rs:74-84` 的约定在使用后必须调用 `Release`，且释放后不得继续使用。mock 只负责交付句柄，不会自动释放它。`GetDDLOwnerMgr` 返回共享 `Arc`，本文件不控制 Owner 选举或关闭过程。

## 与 Go 版本的对应关系

`pkg/domain/sqlsvrapi/mock/server_mock.go` 是 MockGen 生成的直接对照。Rust 保留相同的三个业务方法、参数顺序和 Go 风格名称，也保留 `NewMockServer`、`EXPECT`、`ISGOMOCK` 迁移入口。`pkg/domain/sqlsvrapi/server.go` 与 `server.rs` 进一步证明接口职责一致。

关键差异如下：

- Go `MockServer` 显式保存 `ctrl` 和独立的 `MockServerMockRecorder`；Rust 的记录状态由 mockall 生成，`EXPECT()` 只是返回 `&mut MockServer`。
- Go `NewMockServer` 必须接收并保存 `*gomock.Controller`；Rust 泛型 `_ctrl` 仅作源码形态兼容，实际校验依靠 mockall 和对象析构/`checkpoint`。
- Go `ISGOMOCK()` 返回空结构体；Rust `ISGOMOCK()` 返回单元值 `()`，两者都只作标记。
- Go 接口值天然是引用语义；Rust 明确用 `Arc<dyn Runtime>`、`Arc<dyn KSRuntimeHandle>`、`Arc<dyn Manager>` 表达共享所有权，并用 `SqlSvrError` 约束错误可跨线程安全传递。
- GoMock 使用 `server.EXPECT().Method(...).Return(...)`；Rust mockall 使用 `server.expect_Method().with(...).times(...).returning(...)`。兼容 `EXPECT` 方法不会把 Rust 链式 API 变成 GoMock API。

Go 测试 `pkg/dxf/framework/dxfutil/util_test.go` 与 Rust 的 `util_test.rs` 对齐了当前/跨 keyspace、释放句柄和错误传播行为；Go 仓库还有 taskexecutor、scheduler、importinto 与 DDL 测试导入该 mock 包，但不能据此宣称这些 Go 测试都已逐一迁移成 Rust 测试。

## 扩展指南

当 `pkg/domain/sqlsvrapi/server.rs::Server` 增加或修改方法时，应首先同步 `mockall::mock!` 内的 trait 实现签名；同时核对 `server.go`/`server_mock.go` 的 Go 契约，再在独立测试文件中配置并调用新期望。不要把测试写进 `server_mock.rs`，应优先扩展 `pkg/domain/sqlsvrapi/mock/migration_aster_unit_test.rs`；若行为属于具体消费者，则同步其同目录 `*_test.rs`（例如 DXF 的 `pkg/dxf/framework/dxfutil/util_test.rs`）。

若只增加 Go 迁移兼容入口，可在 `impl MockServer` 中添加薄适配，但不得复制 mockall 已生成的记录器逻辑。任何辅助构造器都应说明参数是否真正参与状态管理；尤其不要让调用者误以为 `_ctrl` 控制 Rust mock 生命周期。

新增返回资源时，应明确其 `Arc` 所有权、显式释放协议和错误类型，并用测试验证正常值、错误值、参数匹配、调用次数以及资源释放。性能风险通常不在本薄 mock 本身，而在过度克隆大型捕获值或在 `returning` 闭包中加入阻塞工作；兼容风险主要来自 trait 签名与 Go 对照漂移，以及期望命名随方法变化而变化。

如果需要改变生产行为，应修改真实 `Server` 实现及独立生产测试，而不是让此 mock 自行补业务逻辑。该文件的职责是不多不少地反映接口契约。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件；`files --filter pkg/domain/sqlsvrapi/mock` 确认目标及相邻 Go/Rust 文件被索引；`node --file pkg/domain/sqlsvrapi/mock/server_mock.rs` 读取 65 行源码并报告 7 个符号；`query MockServer`、`query server_mock`、`query NewMockServer --json` 核对类型、三个 trait 方法和兼容辅助函数。`callers`/`callees` 对宏生成边未产生可用输出，因此用直接引用搜索补证。
- 目标源码：`pkg/domain/sqlsvrapi/mock/server_mock.rs`。
- crate 与装配：`pkg/domain/sqlsvrapi/mock/Cargo.toml`、`pkg/domain/sqlsvrapi/mock/lib.rs`、根 `Cargo.toml`。
- 真实 Rust 契约：`pkg/domain/sqlsvrapi/server.rs`。
- Go 对照：`pkg/domain/sqlsvrapi/server.go`、`pkg/domain/sqlsvrapi/mock/server_mock.go`。
- Rust 测试：`pkg/domain/sqlsvrapi/mock/migration_aster_unit_test.rs`、`pkg/dxf/framework/dxfutil/util_test.rs`；后者的 `Cargo.toml` 证明 mock crate 是 dev-dependency。
- Go 测试对照：`pkg/dxf/framework/dxfutil/util_test.go`，并通过引用搜索确认 taskexecutor、scheduler、importinto 与 DDL 测试对 mock 包的使用范围。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定的 11 章节结构命令、路径存在性和差异范围检查作为验证。
