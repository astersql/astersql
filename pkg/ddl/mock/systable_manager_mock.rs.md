# `pkg/ddl/mock/systable_manager_mock.rs`

## 文件定位

本文件属于 `astersql-ddl-mock` crate，是 DDL 系统表访问接口 `ddl_systable::Manager` 的 GoMock 风格测试替身。crate 入口 `pkg/ddl/mock/lib.rs:10-16` 声明该模块并重新导出 `MockManager`、`MockManagerRecorder`、`new_mock_manager` 以及生产接口类型；`pkg/ddl/mock/Cargo.toml:14-15` 表明它只直接依赖同仓库的 `astersql-ddl-systable` crate。

它不读取 `mysql.tidb_ddl_job` 或 `mysql.tidb_mdl_info`，也不参与 DDL job 的持久化、状态迁移、reorg 或 schema version 同步。真实 SQL 和会话池生命周期位于 `pkg/ddl/systable/manager.rs:115-190` 的 `SystemTableManager`；本文件只让测试预先登记调用期望并经生产 `Manager` trait 注入可控结果。

## 核心职责

- 以 `MockManager` 实现生产 `Manager` trait 的五个查询：按 ID 取 Job、用已有 Session 取 Job 字节、取 MDL 版本、取最小 Job ID、检查 Flashback Cluster Job（`systable_manager_mock.rs:81-195`）。
- 将生产接口参数编码为共享控制器能匹配的 `Argument`：`Context.request_id` 变成 `Argument::Text`，ID 变成 `Argument::Int`，会话只以不透明的 `Argument::Session` 表示。
- 将 `Controller` 的类型化返回值还原成生产返回类型；其中 Job 字节会被实际解码成 `JobWrapper`，其余方法按 `Bytes`、`Int`、`Bool` 返回。
- 通过 `MockManagerRecorder` 提供与五个 trait 方法一一对应的期望登记 API，并保留调用者配置的具体 `ddl_systable::Error` 变体。

## 主要符号

- `production_error(MockError) -> ddl_systable::Error`（第 26-28 行）：把未登记调用、参数不匹配等控制器错误包装为生产侧 `Error::Execute`。
- `manager_return<T>(Result<T, Error>, success)`（第 30-38 行）：recorder 共用的返回值适配器；成功值由闭包变成对应 `ReturnValue`，生产错误保存为 `ReturnValue::ManagerError`。RustCodeGraph 显示它由五个 recorder 方法调用。
- `MockManager`（第 41-44 行）：持有一个执行调用的 `Controller` 和一个 recorder。它未实现自己的同步原语，而是共享控制器状态。
- `MockManagerRecorder`（第 48-50 行）：持有克隆后的同一个 `Controller`，用于登记期望。
- `new_mock_manager`（第 53-60 行）：克隆控制器给 recorder，原控制器给 manager，确保登记侧和调用侧观察同一状态。
- `MockManager::expect`（第 64-66 行）：返回 recorder 引用；`is_mock`（第 69 行）仅作 Mock 类型标记；私有 `call`（第 72-78 行）统一转发至 `Controller::call`。
- `impl Manager for MockManager`（第 81-195 行）：生产 trait 的实际适配层。
- `impl MockManagerRecorder`（第 197-268 行）：五个公开录制方法，用固定的 Go 风格方法名和匹配器序列调用 `Controller::record`。

文件没有模块级常量、条件编译项或异步函数；公开 API 是两个结构体、构造函数、`expect`/`is_mock`、五个 recorder 方法，以及从 `ddl_systable` 再导出的四个生产类型。

## 执行流程

典型测试流程如下：

1. 测试创建 `Controller`，再调用 `new_mock_manager(controller.clone())`；manager 和 recorder 因内部 `Arc` 克隆而共享期望队列与调用记录。
2. 测试通过 `manager.expect().get_*` 登记期望，或直接调用 `Controller::record`。recorder 使用固定方法名（如 `GetMDLVer`）、按生产签名排序的 `Matcher`，以及由 `manager_return` 包装的结果。
3. 被测代码通过 `&dyn ddl_systable::Manager` 调用 Mock。trait 方法把上下文和 ID 转换为 `Argument`，随后进入私有 `call` 和共享 `Controller::call`。
4. 控制器在所有尚未满足的期望中查找方法名、参数个数和匹配器均相符的一项；命中后移除并返回预设结果。该搜索默认不要求录制顺序，依据是 `schema_loader_mock.rs` 中 `Controller::call` 的实现及 `mock_aster_unit_test.rs:25-49` 的测试。
5. trait 方法检查 `ReturnValue` 变体：匹配则返回生产值；`ManagerError` 原样返回；变体错误则返回 `Error::Decode`；控制器自身失败则经 `production_error` 返回 `Error::Execute`。
6. 测试结束时应调用 `Controller::verify`，确认没有未消费期望。`mock_aster_unit_test.rs:106-159` 对五种接口映射和最终校验均有直接覆盖。

`get_job_by_id` 比其他四个方法多一步：`ReturnValue::Job(bytes)` 会调用 `ddl_systable::Job::decode`，成功后同时保存解码对象和原字节到 `JobWrapper`（第 98-102 行）。

## 数据与状态

`MockManager` 与 `MockManagerRecorder` 的唯一状态都是 `Controller`。控制器在 `schema_loader_mock.rs` 中以 `Arc<Mutex<ControllerState>>` 保存两组数据：尚未满足的 `ExpectedCall` 列表和已发生调用列表。因此 `Controller::clone` 不复制期望，而是共享同一状态。

每条期望包括静态方法名、`Vec<Matcher>` 和一次性 `Result<ReturnValue, MockError>`。命中时该期望会从列表移除，所以当前 API 的一条记录只满足一次调用；需要多次调用时应登记多条期望。`Matcher::Any` 接受任意对应参数，`Matcher::Exact` 要求类型化 `Argument` 完全相等。

会话参数故意不保存、克隆或执行，只编码成 `Argument::Session`（第 113、121 行）。`mock_aster_unit_test.rs:15-27,126-140` 的 `DummySession` 若被执行会 panic，测试成功证明本 Mock 不越过该边界。Job 返回值在 recorder 侧只保留 `JobWrapper.bytes`（第 208 行），调用侧再从字节解码，因此预设的 `job` 字段本身不会直接传给调用者。

## 依赖与调用关系

上游边界：

- `pkg/ddl/mock/lib.rs:10-16` 将构造器和 Mock 类型公开到 crate 根；当前仓库内直接 Rust 使用证据位于独立测试 `pkg/ddl/mock/mock_aster_unit_test.rs:106-182`。
- RustCodeGraph 对 `new_mock_manager` 的精确节点确认定义在本文件第 53 行；由于生产调用经 trait 动态分派，静态图没有给出可靠的业务调用主链，不能据此声称它已接入生产 DDL 流程。
- Go 对照 Mock 被 `pkg/ddl/index_nokit_test.go` 和 `pkg/ddl/systable/min_job_id_test.go` 等测试使用，但这不等同于 Rust Mock 的调用者。

下游依赖：

- 共享 Mock 基础设施来自同 crate 的 `schema_loader_mock.rs`：`Argument`、`Matcher`、`ReturnValue`、`MockError`、`Controller`。
- 生产契约来自 `ddl_systable`：`Context`、`Session`、`Manager`、`Job`、`JobWrapper` 和 `Error`。Cargo 没有 feature 条件，依赖是相对路径 `../systable`。
- 真实管理器的下游是会话池与 DDL 系统表；本 Mock 不调用真实 `Session::execute`，也不持有 `SessionPool`。

## 错误处理与边界

- 未登记调用、方法名/参数不匹配会产生 `MockError`；trait 边界将其转换为 `ddl_systable::Error::Execute`，错误文本被保留，但 `MockError` 类型身份不会穿透生产接口。
- recorder 接收的 `Err(ddl_systable::Error)` 被放入 `ReturnValue::ManagerError`，调用侧原样返回。`mock_aster_unit_test.rs:162-182` 明确验证 `Error::NotFound` 不会降级成通用错误。
- 预设返回值的变体与方法不匹配时返回带方法名的 `Error::Decode`，不会 panic 或静默采用默认值（第 104-106、129-131、148-150、167-169、190-192 行）。
- `get_job_by_id` 的 Job 字节无法解码时也返回 `Error::Decode`；因此即使控制器期望匹配，畸形字节仍会按生产解码边界失败。
- `get_job_bytes_by_id_with_session` 不检查具体 Session 实例，只能匹配“存在一个 Session 参数”；若测试需要按 Session 身份区分，当前 `Argument::Session` 模型不支持。
- `is_mock` 没有运行时行为或返回标记值；不要把它当成期望校验。未满足期望只能由共享 `Controller::verify` 发现。

## 并发与资源生命周期

本文件不创建线程、任务、通道、事务、SQL 会话或外部资源。manager 与 recorder 通过 `Controller` 内部的 `Arc<Mutex<_>>` 共享状态，因此类型满足生产 `Manager: Send + Sync` 的要求；每次 `record`、`call`、`verify` 都在互斥锁保护下访问状态。

锁只覆盖一次控制器操作，不跨生产 SQL 或用户回调持有，因为 Mock 根本不执行 SQL。若互斥锁中毒，控制器使用 `expect("mock controller mutex poisoned")` 直接 panic；本文件不恢复 poisoned mutex。

期望的生命周期从 `record` 开始，第一次匹配调用时被移除；manager、recorder 和测试持有的控制器克隆均释放后，共享状态随最后一个 `Arc` 释放。传入的 `&mut dyn Session` 只在调用期间借用且不被保存，Job/字节返回值则按所有权移出 `ReturnValue`。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/ddl/mock/systable_manager_mock.go`，由 MockGen 针对 `systable.Manager` 生成。主要一一对应关系为：

| Go | Rust | 说明 |
| --- | --- | --- |
| `MockManager` / `MockManagerMockRecorder` | `MockManager` / `MockManagerRecorder` | 被调用对象与期望录制对象分离 |
| `NewMockManager` | `new_mock_manager` | manager 与 recorder 共享控制器 |
| `EXPECT` / `ISGOMOCK` | `expect` / `is_mock` | Rust 遵循 snake_case，标记方法无返回值 |
| `GetJobByID` | `get_job_by_id` | Rust 从预设字节实际解码 `JobWrapper` |
| `GetJobBytesByIDWithSe` | `get_job_bytes_by_id_with_session` | Rust 只将 Session 表示为不透明占位符 |
| `GetMDLVer`、`GetMinJobID`、`HasFlashbackClusterJob` | 对应 snake_case 方法 | 方法字符串保持 Go 名称以对齐期望协议 |

Rust 版本是手写的强类型替身，不是生成代码：它复用 crate 内共享 `Controller`，将 GoMock 的 `any`/精确匹配压缩为 `Matcher::{Any, Exact}`，并显式枚举允许的返回类型。GoMock 支持更丰富的 matcher、调用次数和顺序约束；当前 Rust 控制器仅有“一条期望消费一次”、默认无序匹配和最终 `verify`。这些差异是当前代码事实，扩展时不能假设完整 GoMock API 已存在。

## 扩展指南

若生产 `ddl_systable::Manager` 新增方法，应同时完成以下最小闭环：

1. 在 `impl Manager for MockManager` 添加对应转发，明确参数如何映射到 `Argument`、使用哪个稳定方法名以及允许哪个 `ReturnValue` 变体。
2. 在 `MockManagerRecorder` 添加同签名语义的 recorder；如现有 `ReturnValue`/`Argument` 无法表达新类型，应在 `schema_loader_mock.rs` 扩展共享枚举，避免用错误变体凑合。
3. 在独立测试 `pkg/ddl/mock/mock_aster_unit_test.rs` 扩展 `mock_manager_implements_the_production_systable_trait`，至少验证参数顺序、成功值、生产错误保真、错误返回类型；不要把 Rust 测试内嵌回本生产文件。
4. 对照 `pkg/ddl/mock/systable_manager_mock.go` 和生产 Go `pkg/ddl/systable/manager.go` 的接口增量，保持方法语义与错误边界一致；若 Go 生成文件尚未包含新方法，应以生产 trait 为准并明确迁移状态。

修改共享控制器时需关注所有 DDL Mock 的兼容性。增加严格顺序、调用次数或 Session 身份匹配会改变现有默认无序语义，属于测试兼容风险；在热路径中扩展复杂 matcher 还会增加锁内搜索成本。生产正确性风险主要是方法字符串、参数顺序或 `ReturnValue` 映射写错，可能让测试错误匹配或把预期生产错误变成解码错误。

## 验证依据

- 源文件：`pkg/ddl/mock/systable_manager_mock.rs`，核对两个辅助函数、两个结构体、构造器、固有实现、生产 trait 的五个方法和 recorder 的五个方法。
- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`query MockManager --kind struct`、`query new_mock_manager --kind function`、`node new_mock_manager`、`node manager_return`、`callers manager_return`、`callees manager_return` 与 `explore` 确认符号位置、五条 recorder 调用边及共享 `call`/`manager_return` 关系。常见方法名存在歧义，调用者结论因此同时用源码与测试交叉核验。
- crate/模块边界：`pkg/ddl/mock/Cargo.toml`、`pkg/ddl/mock/lib.rs`、`pkg/ddl/mock/BUILD.bazel`。Cargo 侧只有 `ddl-systable` 直接依赖；Bazel 文件描述的是同目录 Go library，不是 Rust crate 构建证据。
- 生产契约与真实实现：`pkg/ddl/systable/manager.rs:26-43,90-137,140-190`，确认 `Context`、`Error`、`Session`、`Manager`、真实会话池归还与系统表 SQL。
- 共享控制器：`pkg/ddl/mock/schema_loader_mock.rs:27-137`，确认参数/匹配器/返回枚举、`Arc<Mutex<_>>`、默认无序消费、错误和 `verify` 语义。
- Rust 独立测试：`pkg/ddl/mock/mock_aster_unit_test.rs:104-182`，确认生产 trait 动态分派、Job 解码、Session 不被执行、五个返回映射、期望消费和 `Error::NotFound` 保真。
- Go 对照与测试：`pkg/ddl/mock/systable_manager_mock.go`、`pkg/ddl/index_nokit_test.go`、`pkg/ddl/systable/min_job_id_test.go`；前者确认 MockGen API，后两者证明 Go 测试中的实际使用场景，但不作为 Rust 接线证据。
- 人工边界复核：本文件是 metadata/system-table 查询的测试替身，不是 job-based DDL 执行步骤；不负责 schema 状态、reorg、取消/回滚、schema diff、MDL 协调或 durable metadata 写入。

本任务为纯文档分析，未运行 Cargo 或代码测试。交付前仅执行任务指定的 11 章节结构检查和文档差异自审。
