# `pkg/domain/sqlsvrapi/mock/ksruntime_mock.rs`

## 文件定位

本文件属于 `astersql-domain-sqlsvrapi-mock` crate，是 [`KSRuntimeHandle`](../server.rs) 的 Rust 测试替身。crate 入口 [`lib.rs`](lib.rs) 将本模块挂到 `domain::sqlsvrapi::mock` 兼容路径，并把依赖 crate 的 `kv`、`meta`、`util` 与 sqlsvrapi trait 再导出，因此宏中的路径与生产接口保持一致。

它不是 keyspace runtime 的真实实现，也不负责获取、缓存或销毁 runtime；它只让测试为 `Runtime` 的三个调用和 `KSRuntimeHandle::Release` 配置返回值、参数匹配、调用次数及回调。真实接口契约定义在 [`server.rs`](../server.rs)，实际跨 keyspace 使用链可见 `pkg/dxf/framework/dxfutil/util.rs::AcquireTaskRuntime`。

## 核心职责

- 通过 `mockall::mock!` 生成公开类型 `MockKSRuntimeHandle`，并让同一个对象同时实现 `RuntimeTrait` 与 `KSRuntimeHandleTrait`。这对应 Go 中 `KSRuntimeHandle` 嵌入 `Runtime` 的关系，确保 `Store`、`SysSessionPool`、`AlterTableMode`、`Release` 共用同一套 mock 期望状态。
- 保留 GoMock 风格的迁移兼容入口：`NewMockKSRuntimeHandle`、`EXPECT` 和 `ISGOMOCK`。其中真正的 Rust 期望配置由 mockall 生成的 `expect_Store`、`expect_SysSessionPool`、`expect_AlterTableMode`、`expect_Release` 完成。
- 支撑调用方测试跨 keyspace handle 的使用与释放，而不启动真实 Domain、KV、系统 session 池或 DDL 执行器。

## 主要符号

- `mockall::mock! { pub KSRuntimeHandle {} ... }`：源码级声明。宏展开后核心公开类型名为 `MockKSRuntimeHandle`，并生成构造器 `new`、各方法对应的 `expect_*` 配置器、期望检查等 mockall API。
- `RuntimeTrait for KSRuntimeHandle`：声明三个继承方法。
  - `Store(&self) -> Arc<dyn Storage + Send + Sync>` 返回 keyspace 作用域存储句柄。
  - `SysSessionPool(&self) -> Arc<dyn DestroyableSessionPool>` 返回系统 session 池。
  - `AlterTableMode(&self, ctx: Context, target: AlterTableModeTarget) -> Result<(), SqlSvrError>` 模拟内部 table-mode DDL 的成功或错误结果。
- `KSRuntimeHandleTrait for KSRuntimeHandle`：增加 `Release(&self)`，用于验证调用方释放已获取 handle 的契约。
- `MockKSRuntimeHandle::ISGOMOCK(&self)`：无状态、无返回值的 GoMock 生成标记兼容方法；不参与 mockall 期望记录。
- `MockKSRuntimeHandle::EXPECT(&mut self) -> &mut Self`：返回自身以保留 GoMock 风格入口。它不是独立 recorder；Rust 调用方仍在返回的同一对象上调用 `expect_*`。
- `NewMockKSRuntimeHandle<C: ?Sized>(_ctrl: &C) -> MockKSRuntimeHandle`：接受任意借用的 controller 形参但故意不使用，返回 `MockKSRuntimeHandle::new()` 创建的空期望集。常见 Rust 调用传入 `&()`；controller 生命周期不会被 mock 保存。

## 执行流程

1. 测试通过 `NewMockKSRuntimeHandle(&())` 或 `MockKSRuntimeHandle::new()` 创建空 mock。构造过程不连接真实 runtime，也不读取 `_ctrl`。
2. 测试持有可变 mock，并调用一个或多个 `expect_*` 配置参数谓词、期望次数和返回动作。例如 `expect_AlterTableMode().withf(...).times(1).returning(...)` 可同时验证 `AlterTableModeTarget` 并注入 `Result`。
3. mock 可直接使用，也可包装成 `Arc<dyn KSRuntimeHandle>`；由于 `KSRuntimeHandle: Runtime`，同一对象也可向上转成 `Arc<dyn Runtime>`。调用四个 trait 方法时，mockall 在同一对象的期望集合中匹配并执行对应动作。
4. 跨 keyspace 典型链路中，`Server::AcquireKSRuntime` 返回该 handle，`AcquireTaskRuntime` 将其作为 `Runtime` 返回给业务代码，同时构造一个最终调用 `Release` 的一次性释放闭包。
5. 测试调用释放闭包或直接调用 `Release`，随后可用 `checkpoint()` 立即核对全部已配置期望；未主动 checkpoint 时，mockall 也会依其对象销毁校验语义检查剩余期望。

## 数据与状态

本文件没有自定义字段、全局变量、静态缓存或持久化状态。可观察状态主要由宏生成并封装在 `MockKSRuntimeHandle` 内：每个方法的匹配规则、允许/要求次数、返回常量或回调，以及已发生调用次数。

方法数据均沿用生产 trait 的所有权边界：`Store` 和 `SysSessionPool` 返回 `Arc` trait object，允许测试与被测代码共享资源；`AlterTableMode` 按值接收 `Context`（Rust 中是 `CancellationToken`）和 `AlterTableModeTarget`；`SqlSvrError` 是 `Box<dyn Error + Send + Sync + 'static>`；`Release` 不返回底层资源，也不自动改变 `Arc` 的所有权。

`EXPECT` 返回对同一 mock 的可变借用，没有 Go 版本独立 recorder 的额外状态。`NewMockKSRuntimeHandle` 的 `_ctrl` 只为调用形态兼容存在，既不保存也不决定验证时机。

## 依赖与调用关系

上游装配与调用：

- [`lib.rs`](lib.rs) 用 `pub mod ksruntime_mock` 注册模块，并在 `domain::sqlsvrapi::mock` 中再导出其公开符号。
- [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 直接调用 `NewMockKSRuntimeHandle`，配置并验证四个 trait 方法，还把它装入 `Arc<dyn KSRuntimeHandle>` 作为 `MockServer::AcquireKSRuntime` 的返回值。
- `pkg/dxf/framework/dxfutil/util_test.rs` 通过依赖别名 `sqlsvrapimock_dependency` 使用 `MockKSRuntimeHandle`，验证跨 keyspace 分支在释放闭包执行前后分别为 0/1 次 `Release`。
- Go 侧 `pkg/dxf/framework/dxfutil/util_test.go`、`pkg/dxf/framework/taskexecutor/manager_test.go` 和 `pkg/dxf/framework/scheduler/scheduler_manager_nokit_test.go` 使用同名 GoMock，体现该替身服务的业务场景：跨 keyspace 任务 runtime 的取得、传递与退出释放。

下游类型与 crate 边界：

- [`Cargo.toml`](Cargo.toml) 仅直接依赖 `mockall = "0.13"` 和路径依赖 `astersql-domain-sqlsvrapi`；后者经 [`lib.rs`](lib.rs) 提供接口及 `Storage`、`AlterTableModeTarget`、`DestroyableSessionPool` 等类型。
- [`server.rs`](../server.rs) 是 `Context`、`SqlSvrError`、`Runtime`、`KSRuntimeHandle` 的权威定义。本文件用别名 `RuntimeTrait`、`KSRuntimeHandleTrait` 避免与宏中待生成的 `KSRuntimeHandle` 名称冲突。
- `Storage`、`DestroyableSessionPool` 与 `AlterTableModeTarget` 只出现在签名中；本文件不调用它们的方法。真实 KV、session pool 和 DDL 行为由测试提供的返回对象及被测调用方决定。

RustCodeGraph 能索引出 `NewMockKSRuntimeHandle`，但宏生成的 `MockKSRuntimeHandle` 和 `expect_*` 调用边没有展开为 Rust 图节点；因此这些边由源码、crate 测试及 `rg` 引用结果交叉核对，而不是把缺失图边误判为“未使用”。

## 错误处理与边界

- 本文件唯一显式可返回业务错误的方法是 `AlterTableMode`，其结果完全由测试配置。mock 不生成或吞掉 `SqlSvrError`；`migration_aster_unit_test.rs` 同时覆盖 `Ok(())` 和注入 `std::io::Error` 后的错误透传。
- `Store`、`SysSessionPool` 和 `Release` 的 trait 签名没有 `Result`。资源本身的失败由返回对象处理，不属于本 mock 文件的职责。
- 空期望集不是“所有调用均返回默认值”。对未配置的方法发起调用、参数不匹配或调用次数不符，会进入 mockall 的测试失败/恐慌路径；调用方测试若要允许多次调用，必须显式配置期望次数或相应策略。
- `Release` 的接口不保证幂等。本文件也没有“已经释放”标志；是否恰好释放一次由 `.times(1)` 等期望表达。生产契约规定调用后不应继续使用 handle，mock 不会从类型层面禁止后续调用。
- `ISGOMOCK` 和 `EXPECT` 是兼容门面：前者不执行接口检查，后者不复制 Go recorder 的反射行为。新增测试不应把它们当成真实 runtime 操作。

## 并发与资源生命周期

生产 `Runtime` 要求 `Send + Sync`，`KSRuntimeHandle` 继承该约束；返回的存储也显式为 `Arc<dyn Storage + Send + Sync>`。因此 mock 可按 trait object 形式跨共享边界使用，但具体返回回调捕获的数据仍须满足 mockall 生成实现和调用场景的线程安全约束。

`Arc` 只管理 Rust 对象引用计数，不等价于业务层 `Release`。跨 keyspace handle 是 runtime 的持有视图，不拥有底层 runtime 生命周期；调用方必须显式执行 `Release`。`AcquireTaskRuntime` 用 `Box<dyn FnOnce() + Send>` 封装这一动作，既限制释放闭包只消费一次，也让它能被任务执行链传递。`util_test.rs` 以 `AtomicUsize` 验证闭包执行才触发一次释放。

本文件不创建线程、异步任务、锁、通道或事务。`Context` 可携带取消状态，但 mock 只将其交给配置的匹配器/回调；是否观察取消取决于测试动作。`AlterTableMode` 的真实等待、取消及 DDL 生命周期均不在此处实现。

## 与 Go 版本的对应关系

直接对照文件是 [`ksruntime_mock.go`](ksruntime_mock.go)，其来源注释表明它由 MockGen 针对 `KSRuntimeHandle` 生成。两版均覆盖 `Store`、`SysSessionPool`、`AlterTableMode` 和 `Release`，并保留 `NewMockKSRuntimeHandle`、`EXPECT`、`ISGOMOCK` 命名；Go commit `b38149d321` 增加 `Runtime::AlterTableMode` 时也同步再生成了该 Go mock，Rust 文件已包含同一方法。

主要实现差异如下：

- Go 使用 `gomock.Controller`、独立 `MockKSRuntimeHandleMockRecorder` 和反射调用；Rust 使用 mockall 宏内建的期望状态，没有独立 recorder 类型。
- Go 构造器保存 controller；Rust 泛型 `_ctrl` 被忽略，真正构造由 `MockKSRuntimeHandle::new()` 完成。
- Go `EXPECT()` 返回 recorder；Rust `EXPECT()` 返回 `&mut Self`，随后使用 `expect_*`，这是调用形态兼容而非 API 的逐字复刻。
- Go `context.Context` 对应 Rust `CancellationToken` 别名；Go 接口值对应 Rust `Arc<dyn ...>`；Go `error` 对应 `Result<(), SqlSvrError>`。
- GoMock 通常由 controller/测试结束检查期望；Rust 可用 `checkpoint()` 显式提前核对，相关迁移测试已经这样使用。

## 扩展指南

当 `Runtime` 或 `KSRuntimeHandle` 新增、改签名或移除方法时，应首先以 [`server.rs`](../server.rs) 和 `server.go` 的真实接口差异为依据，然后同步修改本文件 `mockall::mock!` 中对应的 trait impl；不要只为让测试编译而省略继承方法。Go 生成物若随接口变化，也应重新生成或同步核对其方法集合。

新增返回类型时，应通过 sqlsvrapi 依赖 crate/现有再导出取得真实类型，避免在 mock crate 中复制生产定义。涉及资源句柄时要明确 `Arc`、`Send + Sync`、错误类型和显式释放语义；涉及取消时应在独立测试中用参数谓词或回调验证，而不是在 mock 内实现生产逻辑。

测试必须放在独立文件，优先扩展 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 以覆盖方法的参数匹配、成功/错误返回和期望次数；若变化影响业务链，再同步 `pkg/dxf/framework/dxfutil/util_test.rs` 等调用方测试。至少覆盖：该方法能通过 `Arc<dyn KSRuntimeHandle>` 调用、继承的 `Runtime` 视图仍可用、`Release` 次数没有回归，以及错误原样传播。不要把单元测试嵌入本生产源文件。

兼容风险主要是 trait object 不再可构造、Go/Rust mock 方法集合漂移和释放契约失真；性能风险很低，因为本文件仅供测试，但高频测试中的复杂谓词或加锁回调可能增加测试耗时或产生竞态，应保持确定性。

## 验证依据

- 目标源码：[`ksruntime_mock.rs`](ksruntime_mock.rs)，核对宏声明、两个 trait impl、四个方法签名及三个兼容入口。
- 接口与类型契约：[`server.rs`](../server.rs)，核对 `Runtime: Send + Sync`、`KSRuntimeHandle: Runtime`、`Context`、`SqlSvrError` 及“Release 后不再使用”约束。
- crate 装配：[`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)，核对直接依赖、模块注册、再导出路径和独立测试文件。
- Rust 测试：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `keyspace_runtime_records_release_and_runtime_methods`、`server_records_arguments_and_returns_runtime_handle_owner_and_error`；`pkg/dxf/framework/dxfutil/util_test.rs::test_acquire_task_runtime` 的跨 keyspace 分支。
- Go 对照：[`ksruntime_mock.go`](ksruntime_mock.go)、`server.go`，以及 `pkg/dxf/framework/dxfutil/util_test.go`、`pkg/dxf/framework/taskexecutor/manager_test.go`、`pkg/dxf/framework/scheduler/scheduler_manager_nokit_test.go` 的构造与释放期望。
- RustCodeGraph：`status` 显示目标位于已索引的 11,467 个文件中；`files --filter pkg/domain/sqlsvrapi/mock` 返回目标 Rust/Go 文件与迁移测试；`query MockKSRuntimeHandle` 返回 Go mock 及调用方，`query NewMockKSRuntimeHandle --kind function` 同时定位 Go/Rust 构造器。`explore`、`node --file`、`callers/callees` 未返回宏展开关系，故使用上述源码和 `rg` 引用补证。
- 历史差异：`git show b38149d321 -- pkg/domain/sqlsvrapi/server.go pkg/domain/sqlsvrapi/mock/ksruntime_mock.go` 证明 Go 接口增加 `AlterTableMode` 时同步扩充 mock。
- 本任务是纯文档分析，未运行 Cargo；结构验收要求本文恰有规定的十一个二级章节。
