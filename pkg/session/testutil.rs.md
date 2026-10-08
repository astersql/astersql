# `pkg/session/testutil.rs`

## 文件定位

`pkg/session/testutil.rs` 是 `astersql-session` crate 的测试适配层；`pkg/session/lib.rs` 通过 `pub mod testutil` 无条件导出它，`pkg/session/Cargo.toml` 则把该 crate 定位在 Go 包 `pkg/session` 的移植边界。它不处理线上连接或协议入口，而是把“创建 mock store、bootstrap Domain、创建单连接 Session、执行测试 SQL”收敛为一组与具体存储实现解耦的 trait 和便利函数。

实际执行能力不在本文件中：`pkg/session/runtime/session.rs` 的 `ConcreteTestRuntime<S, F>`、`RuntimeStore<S>`、`RuntimeDomain` 和 `ConcreteRecordSet` 实现主要 trait，`pkg/session/runtime/dispatch.rs` 的 `ConcreteSession` 实现 `TestSession`。因此本文件是测试调用方与会话运行时之间的窄接口，而不是另一套 session 实现。

## 核心职责

- 用 `TestStore`、`TestDomain`、`TestRecordSet`、`TestSession` 和 `TestRuntime` 描述测试所需的最小能力，避免辅助函数依赖具体 mock-storage/domain 类型。
- 用 `InstallTestRuntime` 和私有 `runtime()` 保存并取得进程级测试运行时；所有顶层便利函数共享这一个运行时。
- 用 `CreateStoreAndBootstrap` 固定环境初始化顺序：测试并行度钩子、next-gen 配置、创建 store、bootstrap Domain。
- 用 `CreateSessionAndSetID` 创建会话并分配非零、递增的 connection ID。
- 用 `MustExec`、`MustExecToRecodeSet` 和私有 `exec` 统一普通 SQL 与 prepared SQL 两条测试执行路径。
- 用 `RevertVersionAndVariables` 构造 bootstrap 升级测试需要的旧版本系统表状态。

当前接线有明确限制：仓库文本检索只找到 `InstallTestRuntime` 的定义，没有找到安装调用；现有可运行 Rust 测试通常直接构造 `ConcreteTestRuntime` 并调用其 trait 方法（例如 `pkg/session/runtime_test.rs::concrete_session`）。因此依赖私有 `runtime()` 的顶层便利函数在安装前会 panic，不能把它们描述为已由通用测试初始化自动接线。

## 主要符号

- `GetBootstrapVersion`、`CurrentBootstrapVersion`、`TiDBDDLTableVersionForTest: OnceLock<i64>`：为 bootstrap 版本相关测试预留的一次性数值注入点。与 Go 的同名变量不同，它们保存数值而非函数或普通包变量；本文件不负责赋值。
- `TestStore: Any + Send + Sync`、`TestDomain: Any + Send + Sync`：线程安全的擦除类型边界；`as_any` 允许具体运行时向下转型。`runtime/session.rs` 会据此把 store 还原为 `RuntimeStore<S>`，类型不匹配时返回错误。
- `TestRecordSet: Send`：公开 `Columns`、逐行 `Next` 和 `Close`。`ConcreteRecordSet` 的实现规定关闭后读取报错，并在关闭时清理 store read 与缓冲行。
- `TestSession`：单连接、非并发安全的会话接口；包含连接 ID 设置、文本 SQL 执行、prepare 和 prepared execute。它没有 `Send + Sync` 上界，符合文件注释所述的单连接约束。
- `TYPED_PREPARED_NUMERIC_PREFIX`、`TYPED_PREPARED_NULL`：跨窄字符串 ABI 保存 prepared 参数类型的内部标签。`runtime/dispatch.rs::bind_parameters` 将 null 标签还原为 `NULL`，校验 numeric 标签后的十进制文本，其余参数走字符串转义。
- `TestRuntime: Send + Sync`：全局运行时所需的工厂/环境接口。`ConcreteTestRuntime<S, F>` 是当前具体实现；`SetMaxProcsForTest` 在该实现中为空操作，next-gen 更新由注入闭包完成。
- `TEST_RUNTIME: OnceLock<Arc<dyn TestRuntime>>`：进程级、只能成功安装一次的运行时。
- `SESSION_KIT_ID_GENERATOR: AtomicU64`：连接 ID 生成器，初值为 0，首次分配为 1。
- `InstallTestRuntime`：尝试设置 `TEST_RUNTIME`；重复安装映射成 `SessionError("test runtime is already installed")`。
- `CreateStoreAndBootstrap`、`CreateSessionAndSetID`：环境与会话构造入口。
- `MustExec`、`MustExecToRecodeSet`、`exec`：断言式 SQL 辅助入口及其可返回错误的内部核心。
- `RevertVersionAndVariables`：写回指定 `tidb_server_version`；当版本不大于 195 时同时关闭 `tidb_enable_dist_task`。

## 执行流程

1. 测试装配代码应先以 `Arc<dyn TestRuntime>` 调用 `InstallTestRuntime`。若未安装，之后任何经 `runtime()` 的顶层辅助函数都会以固定消息 panic；重复安装则返回 `SessionError`。
2. `CreateStoreAndBootstrap` 先调用 `SetMaxProcsForTest`；若 `IsNextGen()` 为真，再调用 `UpdateConfigForNextgen`，保证配置修改发生在建 store 之前。随后 `NewMockStore` 创建擦除类型的 store，`BootstrapSession` 消费/初始化其内部存储并返回 Domain。
3. `CreateSessionAndSetID` 让运行时从已经 bootstrap 的 store 创建 session，再用 `fetch_add(1, Ordering::AcqRel) + 1` 分配 ID，最后调用 `SetConnectionID`。
4. `MustExec` 和 `MustExecToRecodeSet` 均进入 `exec`。无参数时调用 `TestSession::Execute`，无结果集返回 `None`，有多个结果集时只移除并返回第一个；有参数时依次调用 `PrepareStmt`、`TestRuntime::ArgsToExpressions`、`ExecutePreparedStmt`。
5. `MustExec` 对执行错误进行 panic 式断言，并在存在结果集时立即 `Close`；`MustExecToRecodeSet` 要求结果集必须存在并把所有权交给调用方，调用方负责读取和关闭。
6. `RevertVersionAndVariables` 通过两次以内的 `MustExec` 修改系统表：始终回退 server version，旧版本（`version <= 195`）额外关闭 distributed task，以复现 Go 升级前置状态。

## 数据与状态

该文件自身不保存业务行数据，持久状态只有三个 `OnceLock<i64>`、全局 `TEST_RUNTIME` 与原子连接 ID 计数器。所有 store、domain、session 都以 `Arc` 在边界上传递；`CreateStoreAndBootstrap` 返回的两个对象共享同一底层运行时环境。

`runtime/session.rs::ConcreteTestRuntime::BootstrapSession` 会向下转型 `TestStore`，从 `Mutex<Option<S>>` 中 `take` 出 storage，初始化 `Domain`，再把 Domain 写入 store 的 `RwLock<Option<Arc<Domain>>>`。这形成重要不变量：同一 store 只能 bootstrap 一次，且 `CreateSession4Test` 只能在 bootstrap 完成后成功。错误分别是 `test store belongs to another runtime`、`test store is already bootstrapped` 和 `test store is not bootstrapped`。

prepared 参数在 `TestSession` 边界仍是 `Vec<String>`。数值和 NULL 的语义由两个以 NUL 开头的标签保留，普通字符串由 `runtime/dispatch.rs::quote_argument` 转义；该协议是测试适配 ABI，扩展时不能让合法普通输入与标签碰撞。

## 依赖与调用关系

上游方面，RustCodeGraph 将本文件标记为被 58 个文件使用，但对精确函数查询未生成 callers 边；`rg` 进一步确认大量测试仅导入 `TestRecordSet`、`TestSession` 或 `TestRuntime`，例如 `pkg/session/runtime_test.rs`、`pkg/session/dml_runtime_test.rs`、`pkg/session/runtime_test/typed_adapter_bridge.rs`。`pkg/session/test/bootstraptest2/boot_test.rs` 中出现的 Go 风格调用位于 `_GO_DRAFT_ARCHIVE` 字符串内，不是可执行 Rust 调用边。

下游方面，顶层函数只直接依赖本文件 trait、`Arc/OnceLock/AtomicU64` 以及 crate 根的 `SessionError`、`SessionResult`。trait 的生产级测试实现位于：

- `pkg/session/runtime/session.rs`：`ConcreteTestRuntime`、`RuntimeStore`、`RuntimeDomain`、`ConcreteRecordSet`；
- `pkg/session/runtime/dispatch.rs`：`ConcreteSession` 的 `TestSession` 实现及 prepared 参数绑定；
- `pkg/session/Cargo.toml`：声明 `nextgen` feature，并提供 Domain、KV、mockstore、parser/executor 等实现所需的 crate 依赖；本文件本身没有条件编译项。

Go 侧的直接消费者主要是 `pkg/session/bootstrap_test.go`、`pkg/session/starter_bootstrap_file_test.go`、`pkg/session/upgrade_backfill_test.go` 和 `pkg/session/tidb_test.go`。这些文件证明辅助函数原本用于 bootstrap/升级与会话行为测试，但不能证明 Rust 顶层便利入口已经接线。

## 错误处理与边界

- 可恢复环境错误以 `SessionResult` 向上传播：mock store 创建、bootstrap、session 创建、SQL 执行、prepare、prepared execute 和 record-set close 都可能失败。
- `InstallTestRuntime` 对重复安装返回错误；`runtime()` 对缺少安装选择 panic。这意味着调用方必须把安装视为进程初始化前置条件。
- `MustExec` 与 `MustExecToRecodeSet` 是测试断言 API，分别以 `test SQL failed`、`statement did not return a record set` 或 `close record set` 触发 panic；不适合生产错误恢复路径。
- 普通 `Execute` 返回多个结果集时，`exec` 只取第一个，剩余结果集随向量销毁；若测试需要验证多结果集，应该直接调用 `TestSession::Execute`，不应扩张 `MustExecToRecodeSet` 的语义。
- prepared 路径的参数数目和 SQL 形态约束由 `ConcreteSession` 实现承担：只能 prepare 一条语句，参数不足/过多、未知 statement ID、非法 numeric 标签或返回多个结果集都会产生 `SessionError`。
- `RevertVersionAndVariables` 直接拼接整数版本值，输入类型本身避免 SQL 字符串注入；但两条更新不是一个由本函数显式管理的事务，第一条成功后第二条仍可能失败并 panic。

## 并发与资源生命周期

`TestRuntime`、`TestStore` 和 `TestDomain` 要求 `Send + Sync`，允许测试框架在线程间共享运行时和环境对象；`TestRecordSet` 只要求 `Send`；`TestSession` 刻意没有线程安全上界，文件注释明确其与 Go `session.Session` 一样绑定单连接且不可并发使用。

`TEST_RUNTIME` 和三个版本注入点使用 `OnceLock`，进程生命周期内不能重置；这适合一次性测试装配，但不支持同一进程串行切换不同 runtime。连接 ID 使用 `AcqRel` 原子递增，避免并发创建 session 时重复分配；没有溢出保护，理论上 `u64::MAX + 1` 会在 debug/release 模式下表现不同，但实际测试规模不会接近该边界。

资源所有权由 `Arc` 和 `Box<dyn TestRecordSet>` 表达。`MustExec` 主动关闭返回结果集；`MustExecToRecodeSet` 把关闭责任移交给调用方。具体 `ConcreteSession` 的 `Drop` 会回滚遗留事务、释放 prepared 计数与行锁，`ConcreteRecordSet::Close` 会释放 store-read 状态并清空缓冲行；这些清理由实现文件保证，不是本 trait 的默认行为。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/session/testutil.go`。主流程保持一致：创建 embedded Unistore mock store 后 bootstrap；会话创建后分配递增 connection ID；无参数 SQL 走 `Execute`，有参数 SQL 走 prepare/execute；`MustExec` 关闭结果集；旧版本回退在 `<= version195` 时关闭 `tidb_enable_dist_task`。

Rust 为适应 crate 边界做了以下显式改造：

- Go 直接依赖 `kv.Storage`、`*domain.Domain`、`sessionapi.Session` 和 `sqlexec.RecordSet`；Rust 通过五个 trait 和 `Any::as_any` 解耦具体类型。
- Go 通过 `testing.T` 与 `require.NoError` 报告失败；Rust 可恢复核心返回 `SessionResult`，`Must*` 包装使用 `expect` panic。
- Go 的 `GetBootstrapVersion` 是函数别名，另外两个是普通版本变量；Rust 当前三个符号均为尚待注入的 `OnceLock<i64>`，语义并非完全等价。
- Go 的参数类型是 `...any`，经 `expression.Args2Expressions4Test` 保留类型；Rust trait 的窄 ABI 是字符串切片，额外用 typed prefix/null sentinel 补偿数值与 NULL 类型。
- Go 的 `context.Background()` 随执行调用传入；Rust `TestSession` 接口不暴露 context，由具体会话内部管理。
- Go 的 session ID 使用 `atomicutil.Uint64.Inc()`；Rust 使用 `AtomicU64::fetch_add(AcqRel) + 1`，同样从 1 开始。

## 扩展指南

新增测试运行时能力时，先判断它是否确属所有测试实现都需要的最小边界：若是，修改 `TestRuntime` 或相应 trait，并同步 `runtime/session.rs::ConcreteTestRuntime`、`RuntimeStore/RuntimeDomain/ConcreteRecordSet` 或 `runtime/dispatch.rs::ConcreteSession` 的实现；若只是单个测试的能力，优先在具体类型上扩展，避免扩大公共 trait。

修改环境创建顺序时，应同步核对 `pkg/session/testutil.go::CreateStoreAndBootstrap` 的 Go 语义，尤其保持 next-gen 配置先于 store 创建、bootstrap 只发生一次。修改 prepared 参数协议时，应同步 `TYPED_PREPARED_*`、`TestRuntime::ArgsToExpressions` 和 `runtime/dispatch.rs::bind_parameters`，并在独立测试文件 `pkg/session/runtime_test/typed_adapter_bridge.rs` 增加字符串、NULL、数值、转义、参数数量不匹配等回归；不要把测试内嵌回生产源文件。

若要启用顶层便利入口，应在明确的进程级测试初始化位置调用一次 `InstallTestRuntime`，并为未安装、重复安装、异源 store、重复 bootstrap、未 bootstrap 建 session 与并发 connection ID 增加独立测试。由于 `OnceLock` 不可重置，测试设计必须避免多个用例争用不同全局 runtime，或先把可替换性设计成显式实例 API。

兼容风险主要在 Go/Rust 参数类型和 bootstrap 版本变量的语义差异；正确性风险集中在结果集关闭责任、只返回首个结果集和全局 runtime 初始化顺序；性能风险较低，但全局共享 runtime、`Arc`/锁和 prepared 字符串重写不应被引入生产热路径。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/session/testutil.rs`；`node --file pkg/session/testutil.rs --offset 1 --limit 260` 读取了全部 189 行；`query` 确认 `CreateStoreAndBootstrap`、`CreateSessionAndSetID`、`InstallTestRuntime`、`RevertVersionAndVariables` 的定义。精确 `callers/callees` 未返回边，因此调用证据由后续文本检索补齐。
- 源码与装配：`pkg/session/testutil.rs`、`pkg/session/lib.rs`、`pkg/session/Cargo.toml`。
- 具体实现：`pkg/session/runtime/session.rs` 中的 `ConcreteTestRuntime`、`RuntimeStore`、`RuntimeDomain`、`ConcreteRecordSet`；`pkg/session/runtime/dispatch.rs` 中的 `impl TestSession for ConcreteSession` 与 `bind_parameters`。
- Go 对照：`pkg/session/testutil.go`；真实 Go 使用点抽查了 `pkg/session/bootstrap_test.go`、`pkg/session/starter_bootstrap_file_test.go`、`pkg/session/upgrade_backfill_test.go` 和 `pkg/session/tidb_test.go`。
- Rust 测试证据：`pkg/session/runtime_test.rs` 与 `pkg/session/dml_runtime_test.rs` 直接构造 `ConcreteTestRuntime`；`pkg/session/runtime_test/typed_adapter_bridge.rs` 使用 `TestSession` 验证具体会话边界；`pkg/session/test/bootstraptest2/boot_test.rs` 的 Go 风格辅助调用仅存在于 `_GO_DRAFT_ARCHIVE`，不参与运行。
- 仓库全局检索：未发现 `InstallTestRuntime` 的调用，故文档将全局便利函数标记为“实现存在但当前未接线”，而非声称已被测试框架自动初始化。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务给定命令验证恰有 11 个固定二级章节，并人工检查重要结论均能回溯到上述符号或路径。
