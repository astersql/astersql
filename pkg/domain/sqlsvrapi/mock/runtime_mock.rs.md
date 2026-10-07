# `pkg/domain/sqlsvrapi/mock/runtime_mock.rs`

## 文件定位

[`runtime_mock.rs`](runtime_mock.rs) 位于 `astersql-domain-sqlsvrapi-mock` crate 中，是 `pkg/domain/sqlsvrapi` 的测试替身层，而不是 SQL Server 的生产运行时实现。crate 入口 [`lib.rs`](lib.rs) 以 `runtime_mock` 模块加载本文件，并在 `domain::sqlsvrapi::mock` 下再导出其公开项，使迁移代码可以沿用接近 Go 包 `pkg/domain/sqlsvrapi/mock` 的导入路径。

文件头的 MockGen 标记说明它对齐 Go 生成文件 [`runtime_mock.go`](runtime_mock.go) 中的 `Runtime` mock；Rust 侧用 `mockall::mock!` 重建相同接口能力。它直接实现的契约定义在上级 crate 的 [`server.rs`](../server.rs) 中：`Runtime: Send + Sync` 表示一个 keyspace 作用域的 KV 存储、系统 session 池和 table-mode DDL 提交视图。

## 核心职责

本文件只负责构造“可编程的 `Runtime` 测试替身”，不持有真实 Domain、KV 连接、session 池或 DDL 执行器，也不实现 table-mode 变更业务。其职责有三项：

1. 由 `mockall::mock!` 生成 `MockRuntime` 及三个 `Runtime` 方法对应的 `expect_*` 配置入口。
2. 将测试调用的参数、调用次数和返回行为交给 mockall 期望引擎匹配；实际返回的 `Storage`、`DestroyableSessionPool` 或 `SqlSvrError` 都由测试配置提供。
3. 用 `ISGOMOCK`、`EXPECT` 和 `NewMockRuntime` 保留 GoMock 生成 API 的表面形态，降低 Go 测试向 Rust 迁移时的概念差异。

因此，这个文件为何存在的直接答案是：让依赖 `Runtime` trait 的代码在不启动真实 SQL Server/Domain 的情况下，精确验证对 keyspace 资源与 DDL 提交入口的调用协议。

## 主要符号

- `mockall::mock! { pub Runtime {} ... }`：宏输入中的 `Runtime` 是 mock 名称，展开后公开类型名为 `MockRuntime`。宏同时生成 `MockRuntime::new`、`checkpoint`、`expect_Store`、`expect_SysSessionPool` 和 `expect_AlterTableMode` 等期望管理 API；这些生成符号没有在源文件中逐项手写。
- `impl RuntimeTrait for Runtime`：要求生成的 `MockRuntime` 实现上游 `Runtime` trait。别名 `RuntimeTrait` 用于避免与宏内 mock 名称 `Runtime` 冲突。
- `Store(&self) -> Arc<dyn Storage + Send + Sync>`：返回 keyspace 作用域的共享 KV 存储 trait object。mock 不创建存储，只返回期望闭包或常量中提供的 `Arc`。
- `SysSessionPool(&self) -> Arc<dyn DestroyableSessionPool>`：返回共享系统 session 池。资源获取、归还、销毁和关闭仍由所返回对象负责。
- `AlterTableMode(&self, ctx: Context, target: AlterTableModeTarget) -> Result<(), SqlSvrError>`：录制带取消语义的上下文和 table-mode 目标，并原样转发测试配置的成功或错误结果；mock 本身不解析元数据或提交 DDL。
- `MockRuntime::ISGOMOCK(&self)`：空操作兼容标记。Rust 返回 `()`，对应 Go 版本返回空结构体；它不参与期望引擎。
- `MockRuntime::EXPECT(&mut self) -> &mut Self`：把 mock 自身作为 recorder 风格门面返回。真正的配置方法仍是 mockall 生成的 `expect_*`，并非 Go 的独立 `MockRuntimeMockRecorder` 类型。
- `NewMockRuntime<C: ?Sized>(_ctrl: &C) -> MockRuntime`：接受任意借用的 controller 形参数但不使用，随后调用 `MockRuntime::new()` 创建空期望集；泛型和 `_ctrl` 仅用于保留构造调用形态。

本文件没有模块级常量、枚举、显式状态结构或条件编译项。

## 执行流程

典型调用流程可由 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `runtime_records_expectations_and_forwards_all_results` 复核：

1. 测试调用 `NewMockRuntime(&())`；函数忽略 controller 占位参数并返回 `MockRuntime::new()`。
2. 测试在可变 mock 上调用 `expect_Store()`、`expect_SysSessionPool()` 和 `expect_AlterTableMode()`，为每个方法登记参数谓词、预期次数以及返回常量或闭包。
3. 被测代码通过 `Runtime` 方法调用 mock。mockall 根据方法和参数选择期望：`Store`/`SysSessionPool` 返回预置 `Arc`，`AlterTableMode` 把 `Context` 与 `AlterTableModeTarget` 传给匹配器，再返回预置的 `Result`。
4. 测试使用返回的真实测试对象继续操作，例如读取 `Storage::GetKeyspace`，或从 session 池 `Get` 后 `Put`。这些后续动作不经过本文件。
5. 测试显式调用 `checkpoint()`，或者让 mock 在 drop 时校验仍未满足的期望；次数或参数不匹配由 mockall 报告为测试失败。

跨 crate 的实际使用例见 `pkg/dxf/framework/dxfutil/util_test.rs` 中的 `new_check_task_runtime_mock_runtime`：它直接用 `MockRuntime::new()` 配置 `Store` 和可选的 `SysSessionPool`，再擦除为 `Arc<dyn Runtime>` 注入被测逻辑。这说明 Go 兼容构造器不是唯一入口，生成的原生 mockall API 也是受用法验证的入口。

## 数据与状态

可见源码没有自定义字段；期望、匹配器、剩余调用次数和返回闭包都保存在 `mockall::mock!` 生成的内部状态中。重要数据边界如下：

- `Arc<dyn Storage + Send + Sync>` 允许测试与被测代码共享同一存储替身，并满足 `Runtime: Send + Sync` 的跨线程类型约束。
- `Arc<dyn DestroyableSessionPool>` 只共享池句柄，不改变池内资源的生命周期规则；本文件不会自动 `Put`、`Destroy` 或 `Close` 资源。
- `Context` 是 `tokio_util::sync::CancellationToken` 的类型别名。mock 仅把 token 当参数传给匹配器；测试可用 `is_cancelled()` 验证调用发生前的取消状态。
- `AlterTableModeTarget` 按值传入，包含 schema/table ID、名称及当前/目标 mode；mock 不补全、验证或改写字段。
- `SqlSvrError` 是 `Box<dyn Error + Send + Sync + 'static>`。错误内容和具体类型来自期望返回闭包，没有本地错误枚举或包装层。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 明确：运行依赖只有 `mockall = "0.13"` 和路径依赖 `astersql-domain-sqlsvrapi`。后者经 [`lib.rs`](lib.rs) 再导出 `kv`、`meta::model`、`util` 和 `domain::sqlsvrapi`，使本文件通过 `crate::...` 引用统一依赖类型。

上游调用者主要是测试辅助代码：同 crate 的 `migration_aster_unit_test.rs` 直接覆盖全部三个方法；`pkg/dxf/framework/dxfutil/util_test.rs` 将 mock 转为 `Arc<dyn Runtime>`；`pkg/dxf/importinto/scheduler_testkit_test.rs` 通过再导出的 `sqlsvrapimock::NewMockRuntime` 构造调度测试运行时。RustCodeGraph 对目标文件报告 `used by 0 files`，这是宏生成符号与 crate 再导出导致的静态文件边局限；仓库级 `rg` 找到的上述具体引用补足了调用证据。

下游依赖不是具体实现，而是四个接口/数据类型：`Runtime` trait、`Storage` trait、`DestroyableSessionPool` trait 与 `AlterTableModeTarget`/`Context`/`SqlSvrError`。真实行为位于测试注入的对象或闭包中。应用主链只通过被测组件消费 `dyn Runtime`；这个 mock crate 本身不应进入生产 SQL 请求链。

## 错误处理与边界

本文件唯一显式可返回错误的接口是 `AlterTableMode`，其 `Result<(), SqlSvrError>` 完全由期望配置决定。`runtime_records_expectations_and_forwards_all_results` 配置 `std::io::Error("ddl rejected")`，随后验证错误文本未被 mock 改写，证明这里是透明转发边界。

未配置方法、参数谓词不匹配、超过或未达到 `.times(...)` 约束时，行为由 mockall 以 panic/期望校验失败的方式报告，而不是转换成 `SqlSvrError`。`NewMockRuntime` 返回空期望集，因此调用者必须在执行被测路径前配置会被触发的方法。`EXPECT` 只返回 `&mut Self`，不能照搬 Go 的 `EXPECT().Store().Return(...)` 链式语法；Rust 调用应使用 `EXPECT().expect_Store()` 或更常见的直接 `expect_Store()`。

该 mock 不验证 `AlterTableModeTarget` 的必填字段，也不实现 `Runtime` 文档所述的名称解析、当前 mode 解析、DDL 提交/等待或取消传播。需要验证这些业务边界时，应测试真实 `Runtime` 实现；此处最多用 `.with(...)`/`.withf(...)` 断言调用者传入的数据。

## 并发与资源生命周期

`RuntimeTrait` 要求 `Send + Sync`，宏生成类型及其方法签名必须满足该 trait 约束；返回的存储还显式要求 `Send + Sync`。但是，本文件不创建线程、异步任务、锁、通道或事务，也不承诺调用顺序之外的并发调度语义。若并发测试共享一个 mock，必须遵守 mockall 对期望状态和返回闭包的线程安全要求，并优先返回线程安全的 `Arc` 对象。

`MockRuntime` 的期望生命周期从 `new()` 开始，到 `checkpoint()` 或 drop 时校验结束。`checkpoint()` 会验证并清理当前期望，适合在同一实例上分阶段重新配置；`migration_aster_unit_test.rs` 在断言后显式调用它。`Store` 与 `SysSessionPool` 返回的 `Arc` 通过引用计数独立存活，mock drop 不等于关闭存储或 session 池。与 `KSRuntimeHandle` 不同，`Runtime` 没有 `Release` 方法，因此本文件也没有跨 keyspace handle 的释放责任。

## 与 Go 版本的对应关系

Go 对照文件是 [`runtime_mock.go`](runtime_mock.go)，两边覆盖相同的 `Runtime` 三方法：`Store`、`SysSessionPool`、`AlterTableMode`，并都允许测试指定参数、次数和返回结果。

关键差异如下：

- Go `MockRuntime` 显式保存 `*gomock.Controller` 和 `*MockRuntimeMockRecorder`；Rust 状态由 mockall 宏生成，没有公开 recorder 结构。
- Go `NewMockRuntime` 必须接收 controller 并把 recorder 与 mock 关联；Rust 的泛型 controller 参数被忽略，每个实例由 mockall 独立校验。
- Go `EXPECT()` 返回独立 recorder；Rust `EXPECT()` 返回 `&mut MockRuntime`，再使用生成的 `expect_*` 方法。
- Go 接口返回 `kv.Storage`/`DestroyableSessionPool` 接口值；Rust 用 `Arc<dyn ...>` 表达共享 trait object 与所有权。
- Go `context.Context` 对应 Rust `CancellationToken` 别名；Go `error` 对应装箱且满足 `Send + Sync + 'static` 的动态错误。
- Go controller 调用通过 `reflect.TypeOf` 录制方法；Rust 在编译期由宏生成分派。二者的测试失败呈现与精确调用时机可能不同，因此不能假定 panic 文本或 recorder 类型兼容。

`ISGOMOCK` 和生成标记是迁移兼容面，不代表本 Rust 文件仍由 Go MockGen 自动生成；修改时应以 Rust trait 与 mockall 宏语法为准，同时人工核对 Go 接口增量。

## 扩展指南

当 `Runtime` trait 新增或修改方法时，最小且安全的接入点是 `mockall::mock!` 内的 `impl RuntimeTrait for Runtime`：签名必须与 [`server.rs`](../server.rs) 完全一致，返回的 trait object 应延续现有 `Arc`、`Send`、`Sync` 和错误边界。不要在 mock 中补写真实 Domain/DDL 逻辑，也不要用宽松默认值掩盖调用者遗漏。

同步工作至少包括：

1. 对照 Go `pkg/domain/sqlsvrapi/mock/runtime_mock.go` 以及其源接口，确认方法集合、参数顺序与语义是否发生变化。
2. 在独立测试 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 中为新方法增加期望、参数匹配、返回值/错误和次数验证；测试逻辑不得内嵌回本源文件。
3. 搜索依赖该 mock 的 `pkg/dxf/**` 等测试辅助函数，判断它们是否需要配置新调用；严格 mock 若新增执行路径而缺少期望会直接失败。
4. 若兼容门面需要扩展，优先保持 `NewMockRuntime`/`EXPECT` 的现有调用形态；不要虚构 Go recorder 类型。只有确有迁移调用者需要时才添加薄适配。

主要风险是接口漂移造成编译失败、把拥有所有权的对象错误改成借用或非共享值、漏配新方法导致测试 panic，以及用 mock 参数匹配代替真实 table-mode 业务验证。性能通常不是此测试替身的目标；若在高并发或大量调用测试中使用，复杂 `.withf` 谓词和克隆大型返回值可能增加测试成本，但不应据此弱化行为断言。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标 `pkg/domain/sqlsvrapi/mock/runtime_mock.rs` 已收录。
- RustCodeGraph `files --filter pkg/domain/sqlsvrapi/mock`：确认 mock crate 的 Rust/Go 源、入口与独立迁移测试集合。
- RustCodeGraph `node --file pkg/domain/sqlsvrapi/mock/runtime_mock.rs --offset 1 --limit 240`：核对本文件 70 行完整源码、宏定义、兼容方法和构造器；索引报告该文件静态 `used by 0 files`。
- RustCodeGraph `node --file pkg/domain/sqlsvrapi/server.rs --offset 1 --limit 220`：核对 `Context`、`SqlSvrError`、`Runtime` 三方法以及 `Send + Sync`/keyspace 契约。
- RustCodeGraph `node --file pkg/domain/sqlsvrapi/mock/migration_aster_unit_test.rs --offset 160 --limit 105`：核对期望配置、参数谓词、错误转发、资源使用和 `checkpoint()`。
- 读取 [`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)：核对 crate 名、`mockall`/上游路径依赖、模块装配和公开再导出路径。
- 读取 Go 对照 [`runtime_mock.go`](runtime_mock.go)：核对 GoMock controller/recorder、构造器、marker 和三个方法的对应关系。
- `rg` 搜索 `NewMockRuntime|MockRuntime|expect_(Store|SysSessionPool|AlterTableMode)|runtime_mock`：确认同 crate 测试及 `pkg/dxf/framework/dxfutil/util_test.rs`、`pkg/dxf/importinto/scheduler_testkit_test.rs` 等直接使用点，并排除仓库中无关的同名局部 mock。
- 本任务是纯文档分析，按计划不运行 Cargo；结构校验应确认目标文件存在且恰有上述 11 个固定二级标题。
