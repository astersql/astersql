# `pkg/util/mock/store.rs`

## 文件定位

`store.rs` 属于 `astersql-util-mock` crate，定义 util 测试工具中的最小化存储占位类型 `Store`。crate 根 `pkg/util/mock/lib.rs` 通过 `mod store; pub use store::*;` 对外重导出该类型；`pkg/util/mock/Cargo.toml` 则表明它依赖的 KV 抽象来自同工作区 `astersql-kv` crate（在 `lib.rs` 中重导出为 `crate::kv`）。

该文件的定位是“可直接调用固定返回值的测试替身”，不是 TiKV、MPP、Oracle 或事务引擎。Go 对照文件 `pkg/util/mock/store.go` 的 `Store` 实现了 `kv.Storage`；但当前 Rust `Store` 只定义了固有方法，没有 `impl kv::Storage for Store`，且部分签名与 `pkg/kv/kv.rs::Storage` 不同。因此它尚不能作为 `Arc<dyn kv::Storage>` 注入 `pkg/util/mock/context.rs::Context::Store`。

## 核心职责

- 保存一个可空的 KV 客户端句柄，并在 `GetClient` 中克隆 `Arc`。
- 为 Go `pkg/util/mock/store.go::Store` 中的 Storage 方法提供同名 Rust API 与固定结果，便于迁移测试核对默认语义。
- 明确表示“没有真实后端”：事务、快照、MPP、Oracle、内存管理器、编解码器、锁等待与状态查询均不产生真实对象。
- 提供稳定身份值：`UUID == "mock"`、`Name == "UtilMockStorage"`、`GetClusterID == 1`、空 keyspace，供单元测试断言。

## 主要符号

- `pub struct Store`：唯一生产类型，派生 `Default`。它只有公开字段 `Client: Option<Arc<dyn kv::Client + Send + Sync>>`；默认构造得到 `None`。
- 客户端与后端查询：`GetClient`返回客户端 `Arc` 的克隆；`GetMPPClient`、`GetOracle`、`GetMemCache`、`GetCodec` 恒返回 `None`。
- 事务与读视图：`Begin(&[kv::tikv::TxnOption]) -> Result<Option<Box<dyn kv::Transaction>>, _>` 返回 `Ok(None)`；`GetSnapshot(kv::Version)` 返回 `None`；`CurrentVersion` 返回 `Version { Ver: 0 }`。
- 生命周期与能力：`Close` 恒成功；`SupportDeleteRange` 返回 `false`；`GetMinSafeTS` 返回 `0`。
- 身份与说明：`UUID`、`Name`、`Describe`、`GetClusterID`、`GetKeyspace` 返回源码中的固定值。
- 状态与选项：`ShowStatus` 返回 `Ok(None)`；`GetLockWaits` 返回 `Ok(None)`；`GetOption` 始终返回 `(None, false)`；`SetOption` 忽略键值。

文件没有模块级常量、trait、宏、条件编译项或私有辅助函数。

## 执行流程

1. 调用方通常用 `Store::default()` 构造空 Store，或直接设置公开的 `Client` 字段。
2. `GetClient` 对 `Option<Arc<_>>` 执行 `clone`：已配置时增加强引用计数，未配置时保持 `None`。
3. 其余方法不读写字段也不访问外部系统：它们直接返回空对象、零值、固定字符串或成功结果。
4. `SetOption` 不保存输入；后续以同一键调用 `GetOption` 仍得到“未找到”。`pkg/util/mock/migration_aster_unit_test.rs::store_returns_the_same_fixed_defaults_as_go` 显式验证了这一流程。

没有从此文件进入真实 SQL 执行、KV RPC 或后台任务的路径；它也没有实现能被 `Context::Store` 调用的 `kv::Storage` trait。

## 数据与状态

`Store` 唯一可变数据是公开的 `Client` 字段。该字段用 `Option` 表示 Go interface 的 `nil`，用 `Arc` 表示共享所有权，并要求 trait object 同时满足 `Send + Sync`。`Default` 不需要自定义逻辑，因为 `Option` 的默认值就是 `None`。

文件不维护事务、快照、锁、选项表、状态字典、时间戳或 keyspace 数据。`CurrentVersion`、`GetMinSafeTS`、`GetClusterID` 等返回的数字是测试常量，不是随调用演进的状态。

## 依赖与调用关系

- 上游装配：`pkg/util/mock/lib.rs` 声明 `mod store` 并公开重导出其符号；外部 crate 因此可通过 `astersql_util_mock::Store` 引用该类型。
- 已核实的直接调用者：`pkg/util/mock/migration_aster_unit_test.rs::store_returns_the_same_fixed_defaults_as_go` 构造 `Store::default()` 并调用全部 20 个方法，是当前固定语义的回归证据。精确全仓搜索未发现其他 Rust 文件构造这个特定 `Store`。
- 下游类型：所有外部类型均来自 `crate::kv`，包括 `Client`、`MPPClient`、`oracle::Oracle`、`tikv::TxnOption`、`Transaction`、`Version`、`Snapshot`、`MemManager`、`context::Context`、`deadlockpb::WaitForEntry` 和 `tikv::Codec`。`std::any::Any` 支撑状态/选项的异构值签名，`std::sync::Arc` 支撑共享 Client。
- crate 边界：`pkg/util/mock/Cargo.toml` 中与本文件直接相关的依赖是 `kv-crate = astersql-kv`；其他依赖服务于同 crate 的 Context、指标等模块，不应归因为 `store.rs` 的运行时调用。

RustCodeGraph 可列出该文件的 21 个符号（`Store` 加 20 个方法），但仓库中 `Store`、`GetClient` 等名称高度重载，通用 callers/callees 结果无法精确归属。因此上述调用关系只采用文件定位后的精确源码证据，不把其他子系统的同名符号计入。

## 错误处理与边界

`Begin`、`Close`、`CurrentVersion`、`ShowStatus` 和 `GetLockWaits` 的签名保留 `kv::errors::SharedError` 错误通道，但当前实现不构造错误，始终返回 `Ok`。这只证明当前占位实现的行为，不代表真实 Storage 不会失败。

最重要的使用边界是：

- `Begin` 的成功不等于创建了事务，返回值仍为 `None`。
- `SetOption` 是静默 no-op，不适合验证选项持久化或依赖选项生效的逻辑。
- `ShowStatus` 和 `GetLockWaits` 以 `None` 表示没有数据；调用方不得把它解释为一个真实的空状态对象/空列表。
- `GetClient` 可返回 `None`，使用方必须处理缺少 Client；它不符合当前 `kv::Storage::GetClient(&self) -> &dyn Client` 的非空契约。
- 因为没有 `kv::Storage` trait 实现，这些同名方法不会被 trait object 动态分派。

## 并发与资源生命周期

`Store` 本身不启动线程、异步任务或通道，也没有锁和可变内部状态。Client 以 `Arc<dyn kv::Client + Send + Sync>` 持有；`GetClient` 只克隆 `Arc`，不克隆 Client 本体，因此句柄在最后一个强引用离开作用域时才释放。

`Close(&self)` 是无操作，不会主动断开 Client、降低引用计数或阻止之后的方法调用。因此如果新功能引入需要关闭的资源，必须另行定义关闭后状态、幂等性和并发调用约束，不能沿用当前 no-op 语义后假定资源已释放。

## 与 Go 版本的对应关系

Rust `Store`、字段 `Client` 及 20 个方法与 `pkg/util/mock/store.go` 中的同名符号一一对应。固定值保持一致：`UUID` 为 `mock`，版本与 safe TS 为 0，不支持 DeleteRange，名称/描述文本一致，集群 ID 为 1，keyspace 为空。

Rust 用显式类型表达 Go `nil`：客户端、事务、快照和若干服务返回 `Option`，Go 的 `(nil, nil)` 在 `Begin` 中映射为 `Ok(None)`。Go 的 `any` 映射为 `dyn Any`，Go 的可空 Client interface 映射为 `Option<Arc<dyn Client + Send + Sync>>`。

尚未完全对齐的部分是接口接线：Go 通过方法集隐式满足 `kv.Storage`；Rust 没有 `impl kv::Storage for Store`。同时 Rust trait 要求的 `Begin`、`GetSnapshot`、`GetClient`、`GetMPPClient`、`GetOracle`、`ShowStatus`、`GetMemCache`、`GetLockWaits`、`GetCodec`、`GetOption` 签名与此文件的可空签名不兼容，`Close` 的接收者也是 `&self` 与 trait 的 `&mut self` 之别。这是当前代码事实，不应宣称 Rust 版已完成 Go Storage 替换。

## 扩展指南

- 如只增加测试查询能力，应在 `impl Store` 中增加最小方法，保持默认构造确定、无 I/O，并在独立测试文件 `pkg/util/mock/migration_aster_unit_test.rs` 扩展回归断言；不要将 Rust 测试写入 `store.rs`。
- 如要让它真正作为 `kv::Storage`，必须先处理“Go 允许 nil，Rust trait 要求非空引用/对象”的契约冲突，再实现 `impl kv::Storage for Store`。不应用 panic、悬空引用或伪造成功对象来隐藏缺少后端。
- 如使 `SetOption` 开始持久化值，需增加内部同步和类型擦除策略，验证覆盖/缺失/错误类型/并发访问，并重新评估 `Store` 的 `Send + Sync` 性质与性能。
- 如增加事务、快照、Oracle 或编解码器，应优先复用 `astersql-kv` 已有测试替身，并同步检查 `pkg/util/mock/context.rs` 中的 `Context::Txn`、`GetClient`、`GetMPPClient`、`Txn`、`GetStore`。
- 任何改变固定值或空值语义的改动，都要与 `pkg/util/mock/store.go` 的对应行为核对；若有意与 Go 分歧，需在独立 Rust 测试中明确记录理由及兼容风险。

## 验证依据

- RustCodeGraph：`status` 显示当前索引包含 7,032 个 Rust 文件；`files --filter pkg/util/mock` 确认目标源文件与相邻测试；`node --file pkg/util/mock/store.rs --symbols-only` 列出 `Store` 和 20 个方法；`node Store --file pkg/util/mock/store.rs` 定位唯一结构体。callers/callees 对同名符号产生歧义候选，未将其作为精确调用边。
- Rust 源码：`pkg/util/mock/store.rs` 提供全部类型、签名和固定返回值；`pkg/util/mock/lib.rs` 提供模块声明与重导出；`pkg/kv/kv.rs::Storage` 提供 trait 签名对照；`pkg/util/mock/context.rs::Context::Store` 提供实际注入边界。
- Cargo：`pkg/util/mock/Cargo.toml` 确认 crate 名、`lib.rs` 入口、`astersql-kv` 路径依赖和开发测试依赖；本文件没有 feature 条件。
- Go 对照：`pkg/util/mock/store.go` 提供 `Store` 的原始字段、方法集、固定值及 `kv.Storage` 实现意图。`pkg/util/mock/mock_test.go` 只测试 Context，未覆盖 Store。
- Rust 测试：`pkg/util/mock/migration_aster_unit_test.rs::store_returns_the_same_fixed_defaults_as_go` 是 Store 的独立回归测试，覆盖默认 Client、全部占位方法、固定身份值和 `SetOption` 的 no-op 行为。
- 结构验证按任务文件的命令执行，要求本文件存在且恰好包含 11 个固定二级标题。本任务是纯文档分析，按总计划不运行 Cargo。
