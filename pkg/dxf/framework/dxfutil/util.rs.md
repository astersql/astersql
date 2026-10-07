# `pkg/dxf/framework/dxfutil/util.rs`

## 文件定位

本文件是 Rust crate `astersql-dxf-framework-dxfutil` 的 keyspace Runtime 工具实现。crate 入口 [`lib.rs`](lib.rs) 通过 `#[path = "util.rs"] mod dxfutil_impl` 装入本文件并用 `pub use dxfutil_impl::*` 再导出公开项；[`Cargo.toml`](Cargo.toml) 将该 crate 对应到 Go 包 `pkg/dxf/framework/dxfutil`。在 DXF 的分布式任务框架中，它负责在“当前节点 keyspace”与“任务所属 keyspace”之间选择 Runtime，并在使用前校验 Runtime 内部的 store/session keyspace 一致性。

当前 Rust 仓库中，`AcquireTaskRuntime`、`CheckTaskRuntime` 和 `GenHolderID` 的直接调用点只出现在同 crate 的独立测试 `util_test.rs` 与 `migration_aster_unit_test.rs`；未发现 Rust 生产调用点。相应 Go 实现已经接入 scheduler manager、task-executor manager、scheduler 和 task executor。因此，本文件是已公开、已测试的移植实现，但不能据此断言 Rust DXF 生产主链已经使用它。

## 核心职责

- `AcquireTaskRuntime` 根据任务 keyspace 与当前会话 store 的 keyspace 是否相同，选择节点本地 Runtime 或向 SQL Server 申请跨 keyspace 的 `KSRuntimeHandle`，并把配套释放动作交给调用方。
- `CheckTaskRuntime` 做两层防御性一致性检查：Runtime 的 store 必须属于目标任务 keyspace；Runtime 系统会话池借出的 session store 必须与 Runtime store 属于同一 keyspace。
- `GenHolderID` 生成 SQL Server 跟踪 Runtime 持有者所需的稳定字符串标识。
- 私有辅助函数 `releaseTaskRuntime` 和 `taskRuntimeError` 分别封装条件释放与普通消息到 `SqlSvrError` 的转换。

这些职责只涉及 Runtime 的选择、验证和借用生命周期；文件不负责调度任务、执行子任务、创建真实 Runtime，也不持久化任何状态。

## 主要符号

- `pub trait sessionProvider`：最小会话提供接口。`WithNewSession` 接收一次性回调 `FnOnce(sessionctx::Context) -> Result<(), SqlSvrError>`，使 `AcquireTaskRuntime` 无需依赖具体 task manager 类型。trait 名和方法名保留了 Go 命名风格。
- `pub fn AcquireTaskRuntime<P>(sessionProvider: P, taskKS: String, holderID: String) -> Result<(Arc<dyn Runtime>, Box<dyn FnOnce() + Send + 'static>), SqlSvrError>`：公开获取入口。返回共享 Runtime 与只能调用一次的释放闭包；泛型参数必须实现 `sessionProvider`。
- `fn releaseTaskRuntime(Option<Arc<dyn KSRuntimeHandle>>)`：仅当跨 keyspace 路径实际获得 handle 时调用 `Release`，本地路径传入 `None`，因而是空操作。
- `fn taskRuntimeError(String) -> SqlSvrError`：以 `std::io::Error::other` 构造错误并转换为 SQL Server 统一错误类型。
- `pub fn CheckTaskRuntime(Arc<dyn Runtime>, String) -> Result<(), SqlSvrError>`：公开校验入口，先检查 Runtime store，再经 `storage::NewTaskManager` 从 `SysSessionPool` 借 session 检查其 store。
- `pub fn GenHolderID(String, i64) -> String`：返回 `DXF/{component}/{taskID}`；`taskID` 是有符号整数，负值会原样保留负号。

文件没有模块级常量、结构体、枚举、`impl` 块或条件编译项。

## 执行流程

`AcquireTaskRuntime` 的流程如下：

1. 建立两个局部槽位：`taskRuntime` 保存最终 Runtime，`acquiredHandle` 只保存跨 keyspace handle。
2. 通过 `sessionProvider.WithNewSession` 获取一次会话，从 `se.GetStore().GetKeyspace()` 读取节点当前 keyspace，并从 `se.GetSQLServer()` 取得服务器接口。
3. 若 `taskKS != currentKS`，调用 `AcquireKSRuntime(taskKS, holderID)`；成功后把同一个 handle 分别向上转型为 `Runtime` 视图和保留为 `KSRuntimeHandle`，供返回值使用与日后释放。
4. 若 keyspace 相同，直接调用 `GetRuntime()` 取得节点本地 Runtime，不建立可释放 handle。
5. 会话回调错误原样向外传播；成功后要求回调已经写入 `taskRuntime`，否则以 `expect` panic。
6. 返回 Runtime 和捕获 `acquiredHandle` 的 `FnOnce` 闭包。调用闭包时，跨 keyspace 路径恰好执行一次 `Release`，本地路径无动作。

`CheckTaskRuntime` 先读取 `runtime.Store().GetKeyspace()` 并与 `taskKS` 比较。不匹配时立即返回错误，不访问会话池；匹配时以 `runtime.SysSessionPool()` 创建轻量 `TaskManager`，借出一条 session，再比较 session store keyspace 与 Runtime store keyspace。只有两层均一致且借用过程无错时才返回 `Ok(())`。

`GenHolderID` 没有分支，只用格式化操作依次连接固定前缀、组件名与任务 ID。

## 数据与状态

- Runtime、handle、store 与会话池均通过 `Arc<dyn ...>` 共享所有权；本文件不复制其底层资源。
- `AcquireTaskRuntime` 的可变局部状态仅在同步会话回调期间写入：`taskRuntime: Option<Arc<dyn Runtime>>` 和 `acquiredHandle: Option<Arc<dyn KSRuntimeHandle>>`。函数返回后，后者移入释放闭包。
- `taskKS` 与 `holderID` 以拥有所有权的 `String` 传入；跨 keyspace 调用时在回调中克隆，避免回调捕获与错误传播造成借用冲突。
- `CheckTaskRuntime` 不改变 Runtime；它读取 store keyspace，并临时借还一个池化 session。具体借还行为在 `lib.rs` 的 `storage::TaskManager::WithNewSession` 中实现：无论回调成功或返回错误，正常路径都会执行 `pool.Put(resource)`。
- `GenHolderID` 的输出没有转义或规范化：组件字符串中的斜杠、空串等会直接进入结果。目前测试只固定普通组件名及正、负 task ID。

## 依赖与调用关系

上游边界：

- `lib.rs` 将本文件的公开 API 再导出为 crate 顶层 API；同文件定义本实现依赖的 `sessionctx::Context` 与 `storage::TaskManager` 适配层。
- Rust 独立测试 `util_test.rs` 直接覆盖获取、释放和两层 keyspace 校验；`migration_aster_unit_test.rs` 额外覆盖关闭会话池的错误传播与 `GenHolderID`。
- 在 Rust 生产代码检索中没有发现这三个顶层函数的调用。`pkg/dxf/framework/taskexecutor/manager.rs` 当前调用的是 `taskTable.AcquireTaskRuntime`，其声明位于 `taskexecutor/interface.rs`，不是本文件函数。
- Go 生产链中，`scheduler/scheduler_manager.go` 与 `taskexecutor/manager.go` 用 `GenHolderID` 后调用 `AcquireTaskRuntime`；`scheduler/scheduler.go` 与 `taskexecutor/task_executor.go` 调用 `CheckTaskRuntime`。这些位置说明该工具在完整 Go DXF 生命周期中的预期接入点。

下游依赖：

- `sessionctx::Context::{GetStore, GetSQLServer}` 提供当前 keyspace 与 SQL Server。
- `sqlsvrapi::Server::{GetRuntime, AcquireKSRuntime}` 选择本地或目标 keyspace Runtime；`KSRuntimeHandle::Release` 结束跨 keyspace 持有关系。
- `sqlsvrapi::Runtime::{Store, SysSessionPool}` 为一致性检查提供存储与系统会话池。
- `storage::NewTaskManager(...).WithNewSession(...)` 负责从池中取 session、下转型、回调执行和归还。
- Cargo 直接依赖包括 `astersql-kv`、`astersql-meta-model`、`astersql-owner`、`astersql-domain-sqlsvrapi` 与 `astersql-util`；本文件直接使用其中由 crate 入口暴露的 KV/session、SQL Server API 和 session pool 适配。

## 错误处理与边界

- `sessionProvider.WithNewSession` 的错误直接由 `?` 返回；测试确认会话错误文本保持为 `session error`。
- 跨 keyspace 的 `AcquireKSRuntime` 错误同样直接传播；发生错误时函数不会返回 Runtime 或释放闭包，也不会留下本函数持有的 handle。
- Runtime store 与任务 keyspace 不一致时，错误文本为 `store keyspace mismatch with task: {storeKS} vs {taskKS}`。
- Runtime store 与借出 session 的 store 不一致时，错误文本为 `invalid task runtime with mismatched keyspace: {storeKS} vs {sessKs}`。
- 获取 session 失败、池关闭或池资源类型不正确的错误由 `TaskManager::WithNewSession` 传播；迁移测试确认关闭池返回 `session pool closed`。
- `AcquireTaskRuntime` 假定成功的 `WithNewSession` 必定执行回调。若某个实现返回 `Ok(())` 却跳过回调，`taskRuntime.expect(...)` 会 panic；这是 trait 实现者必须遵守、但类型系统未表达的不变量。
- 会话的 SQL Server 是 `Option`，而 `GetSQLServer` 在缺失时 panic。即使任务与当前 keyspace 相同，本函数也会在分支判断前调用它，因此提供者必须始终配置 SQL Server。
- 释放不是 RAII：调用方若丢弃返回闭包，跨 keyspace handle 的显式 `Release` 不会由本文件自动执行。反之，`FnOnce` 类型防止同一个闭包被安全 Rust 重复调用。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。接口使用 `Arc` 共享 Runtime 与 handle，使返回对象可与系统其他持有者共享；释放闭包声明为 `Send + 'static`，可移交到其他线程或存入长生命周期任务对象，但文件本身不保证具体 trait 对象的业务操作顺序。

跨 keyspace 生命周期是显式配对的：`AcquireKSRuntime` 成功后，handle 同时提供 Runtime 视图并被释放闭包持有；调用方完成任务后必须调用闭包，才会执行一次 `Release`。同 keyspace 路径仅借用服务器已有 Runtime，释放闭包为空操作。测试使用原子计数器验证跨 keyspace 调用释放前计数为 0、调用后为 1，本地路径始终为 0。

`CheckTaskRuntime` 的 session 生命周期由 `TaskManager::WithNewSession` 管理。池资源在回调完成后归还；回调的 keyspace 校验失败也先记录结果、归还资源，再把错误返回。若取资源或下转型阶段失败，则没有可归还的成功 session。

## 与 Go 版本的对应关系

Rust 文件逐项对应同目录 `util.go`：`sessionProvider`、`AcquireTaskRuntime`、`releaseTaskRuntime`、`CheckTaskRuntime` 与 `GenHolderID` 的核心分支、错误文本和 holder 格式均保持一致。`util_test.rs` 的四类获取分支和三类校验分支也与 `util_test.go` 对齐。

需要注意的语言实现差异：

- Go 返回 `sqlsvrapi.Runtime, func(), error`；Rust 用 `Result<(Arc<dyn Runtime>, Box<dyn FnOnce() + Send + 'static>), SqlSvrError>` 表达成功/失败互斥，并用 `FnOnce` 表达单次释放。
- Go 的 `releaseTaskRuntime` 对 Runtime 做运行时类型断言，仅 handle 才释放；Rust 在获取时显式保留 `Option<Arc<dyn KSRuntimeHandle>>`，无需在释放时下转型。
- Go 的错误经 `errors.Trace(fmt.Errorf(...))` 包装；Rust 以 `std::io::Error::other(...).into()` 转成 `SqlSvrError`。测试证明用户可见消息一致，但两种语言的错误链/类型并非完全相同。
- Go 的生产调用已经接入 scheduler 与 executor；Rust 当前只公开和测试该实现。移植状态应描述为“逻辑实现已对齐，生产接线未由本文件调用证据证实”。

## 扩展指南

- 新增 Runtime 选择条件时，优先修改 `AcquireTaskRuntime`，同时保持“是否取得 handle”与释放闭包严格一致；不要让本地 Runtime 误调用 `Release`，也不要让成功取得的 handle 脱离释放责任。
- 若要消除“提供者成功但未执行回调”的 panic，应重新设计 `sessionProvider` 返回值契约，而不是仅移除 `expect`；需同步所有实现和独立测试。
- 若要支持没有 SQL Server 的同 keyspace 会话，需要把 `GetSQLServer` 延迟到跨 keyspace 分支或调整 `Context` API，并补充无 Server 的本地路径测试。
- 修改 keyspace 校验时，应同时评估 store 与 session 两层检查，避免只验证其中一层而引入跨租户正确性风险。错误文本若属于兼容接口，也应同步 Go/Rust 测试。
- 修改 holder ID 格式时，需要同步 Go 的 `GenHolderID`、scheduler/task-executor 调用预期以及 `migration_aster_unit_test.rs`；格式变化可能影响 SQL Server 的 holder 跟踪和诊断兼容性。
- Rust 测试逻辑必须继续放在独立文件。常规分支扩展到 `util_test.rs`；Go 移植一致性、资源计数或额外错误边界可补到 `migration_aster_unit_test.rs`。不应把测试模块内嵌回 `util.rs`。
- 若完成 Rust 生产接线，重点核对 Go 的四处入口：两个 manager 获取/释放 Runtime，以及 scheduler/executor 开始工作前的校验；同时确认所有退出路径都会调用释放闭包。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/dxf/framework/dxfutil` 确认本 crate 的实现、入口和两份独立测试均在图中。
- RustCodeGraph `node --file pkg/dxf/framework/dxfutil/util.rs`：核对全部 130 行源码、公开/私有符号与实现分支。
- RustCodeGraph `query AcquireTaskRuntime`、`query CheckTaskRuntime`、`query GenHolderID`、`query sessionProvider`：核对 Rust/Go 对应定义和测试符号；`callers/callees` 未产生可用边，因此调用点另以仓库文本检索交叉核对。
- RustCodeGraph `node --file` 已读路径：`pkg/dxf/framework/dxfutil/lib.rs`、`util.go`、`util_test.rs`、`util_test.go`、`migration_aster_unit_test.rs` 及 `pkg/dxf/framework/doc.go`。
- 直接读取 `pkg/dxf/framework/dxfutil/Cargo.toml`，核对 crate 名、`lib.rs` 入口、直接依赖、mock 开发依赖和 `go-package` 元数据。
- `rg` 调用点核对：Go 生产调用位于 `scheduler/scheduler_manager.go`、`scheduler/scheduler.go`、`taskexecutor/manager.go` 和 `taskexecutor/task_executor.go`；Rust 未发现本文件三项公开函数的生产调用，`taskexecutor/manager.rs` 的同名方法属于 `taskTable` 接口。
- 独立测试证据：`util_test.rs` 覆盖本地/跨 keyspace、Acquire/session 错误、store/session 不一致；`migration_aster_unit_test.rs` 还覆盖释放次数、关闭会话池错误和正负 task ID 格式。按本任务约束未运行 Cargo。

