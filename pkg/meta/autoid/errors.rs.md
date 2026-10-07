# `pkg/meta/autoid/errors.rs`

## 文件定位

[源文件](errors.rs) `errors.rs` 是 `astersql-meta-autoid` crate 的错误边界和 `AUTO_RANDOM` 文案目录。crate 根 `pkg/meta/autoid/lib.rs` 通过 `pub mod errors` 挂载它，再用 `pub use errors::*` 将公开符号重导出；因此上游既可以写 `crate::errors::AutoIdError`，也可以通过 `astersql_meta_autoid::AutoIdError` 使用。`pkg/meta/autoid/Cargo.toml` 表明该 crate 的库入口是 `lib.rs`，本文件唯一直接外部依赖是 `thiserror = "2"`。

它不执行 ID 分配，也不保存元数据。其两类输出分别服务于：Rust AutoID 实现的可分类错误传播（`AutoIdError` 和 `Result<T>`），以及与 Go DDL/Executor 保持文案兼容的 `AUTO_RANDOM_*` 常量。

## 核心职责

1. 用 `AutoIdError` 将分配器校验、ID 空间耗尽、存储事务、取消、RPC 和远程业务错误收敛到一个可 `Clone` / `Eq` / `Error` 的类型。
2. 用 `Result<T>` 统一 AutoID crate 内 trait 和实现的返回类型，使调用链保留结构化错误身份。
3. 提供三个轻量辅助入口：`is_rpc_retry_limit_error` 识别终止重试，`invalid_increment_and_offset` 和 `autoinc_read_failed` 统一构造常见错误。
4. 保存 20 个 `AUTO_RANDOM_*` 模板/提示常量，使 `%s` / `%d` 占位符、大小写和措辞与 `pkg/meta/autoid/errors.go` 对应。

## 主要符号

- `pub type Result<T> = std::result::Result<T, AutoIdError>`：AutoID API 的通用结果类型。`pkg/meta/autoid/autoid.rs`、`autoid_service.rs` 和 `memid.rs` 均将它用于分配、rebase、存储和服务接口。
- `pub enum AutoIdError`：共 13 个 variant。`InvalidTableId`、`InvalidIncrementAndOffset`、`InvalidAutoRandom` 是本地输入/状态校验；`AutoIncrementReadFailed`、`AutoRandomReadFailed` 表示读水位或 ID 空间失败；`Storage` 是底层持久化失败；`Canceled` 是取消标记；`Rpc`、`Service`、`RpcRetryLimit` 区分可重试传输失败、远程业务拒绝和达到终止阈值；`NotImplemented`、`WrongAutoKey`、`InvalidAllocatorType` 表示不支持操作或内部类型不匹配。
- `pub fn is_rpc_retry_limit_error(&AutoIdError) -> bool`：只对 `RpcRetryLimit(_)` 返回 `true`，不通过字符串判断。`pkg/domain/domain.rs::DomainError::from_auto_id` 据此决定是否保留 AutoID 作为 error source，`pkg/session/runtime/session_test.rs` 验证标记经 SQL 错误包装后仍可向下转型识别。
- `pub fn invalid_increment_and_offset(i64, i64) -> AutoIdError`：创建带原始 `increment` / `offset` 值的结构体 variant；`memid.rs::Allocator::alloc` 和 `autoid_service.rs::alloc` 在步长校验失败时调用。
- `pub fn autoinc_read_failed(impl Into<String>) -> AutoIdError`：保留具体失败语境并构造 `AutoIncrementReadFailed`；例如 `memid.rs::alloc_signed` / `alloc_unsigned` 在有符号或无符号 ID 空间耗尽时使用。
- `AUTO_RANDOM_*`：包括主键首列、聚簇主键、与 `AUTO_INCREMENT`/`DEFAULT` 互斥、shard/range bits 边界、显式插入、rebase 和 ALTER 限制等文案。当前 Rust 生产代码直接引用的是 `AUTO_RANDOM_NON_POSITIVE`（`autoid.rs::auto_random_shard_bits_normalize`）；其余常量当前主要是 Go 兼容对照，不应误述为已在 Rust DDL 链路中全部接线。

## 执行流程

`errors.rs` 自身没有长流程，它参与的典型错误路径如下：

1. 分配器入口先校验 table ID、`increment`/`offset` 或 `AUTO_RANDOM` bits；失败时直接返回对应 `AutoIdError`。
2. 本地内存分配器在推进水位前检查数值空间；溢出则通过 `autoinc_read_failed` 返回带有 signed/unsigned 上下文的 `AutoIncrementReadFailed`（`memid.rs`）。
3. 单点 AutoID 客户端遇到 `Rpc` 时可重置连接并重试；达到次数与耗时阈值后，`autoid_service.rs::retry_rpc` 构造 `RpcRetryLimit`。`Service` 和 `Canceled` 不是同一类重试终态。
4. Domain 接收 AutoID 错误后，`DomainError::from_auto_id` 仅对 `RpcRetryLimit` 保留结构化 source，其他 variant 当前转为 `Store(String)`。Session 层因而能在 error chain 中识别终止 AutoID 错误，防止 `INSERT IGNORE` 将其降级后继续写入。
5. `AUTO_RANDOM_*` 常量不会自动格式化；调用者负责传入占位参数并将结果封装成用户可见错误或 Note。Go 的 `pkg/ddl/tests/serial/serial_test.go::TestAutoRandom` 是这项文案契约的主要回归证据。

## 数据与状态

本文件不持有全局可变状态。`Result<T>` 是零运行时成本的类型别名；`AUTO_RANDOM_*` 是 `'static` 字符串；三个函数只做匹配或值构造。

`AutoIdError` 中的文本负载使用拥有所有权的 `String`，以便从存储/RPC 错误或动态参数生成详细上下文。`InvalidIncrementAndOffset` 保存两个 `i64` 字段，便于结构化比较；`Canceled` 是无负载标记。枚举派生 `Clone, Debug, Eq, PartialEq`，所以测试可以精确比较 variant 和负载；`thiserror::Error` 则提供 `Display` 和 `std::error::Error`。

## 依赖与调用关系

- 下游依赖：仅 `thiserror::Error`，用于派生标准错误实现和每个 variant 的展示文案。文件不调用存储、网络、时钟或格式化引擎。
- crate 内上游：`autoid.rs` 消费 `Result`、多个 variant 和 `AUTO_RANDOM_NON_POSITIVE`；`memid.rs` 消费两个构造函数；`autoid_service.rs` 消费 `Result`、`Rpc`、`Service`、`Canceled`、`RpcRetryLimit` 等分类。
- crate 外上游：`pkg/domain/autoid_store.rs` 将存储错误转为 `AutoIdError::Storage`；`pkg/domain/domain.rs` 使用 `is_rpc_retry_limit_error`；`pkg/autoid_service/client.rs` 在客户端适配层产生 `Storage`；`pkg/autoid_service/autoid.rs` 将 AutoID 错误编码到 RPC 响应或 gRPC status。
- 装配边：`pkg/meta/autoid/lib.rs` 公开模块并重导出全部符号。RustCodeGraph 文件节点报告 `errors.rs` 被 22 个文件使用；精确 `callers/callees` 命令在当前索引上未返回结果，上述具体边由 `rg` 与相邻源码复核。

## 错误处理与边界

- `#[error(...)]` 是对外文本契约的一部分。修改前必须检查 SQL 层、RPC 响应和 Go 兼容测试；例如 `RpcRetryLimit` 刻意使用与 `AutoIncrementReadFailed` 相同的 `auto-increment read failed: ...` 前缀，但通过 variant 保留可机器识别的终止身份。
- `is_rpc_retry_limit_error` 只检查直接传入的 `AutoIdError`；它不遍历任意 `source()` 链。遍历责任在上游，`session_test.rs` 展示了逐层 `source()` 并 `downcast_ref` 的做法。
- `autoinc_read_failed` 和两个 RPC 辅助路径保留原始消息，但没有自动 error source 字段；底层错误通常先转为字符串。因此依赖 source-chain 语义的新需求需要显式扩展枚举负载。
- `%s` / `%d` 是 Go `fmt.Sprintf` 风格占位符，不是 Rust `format!` 占位符。Rust 调用者不能直接将这些模板作为 `format!` 格式字符串；新接线必须选择与 Go 输出等价的安全替换/格式化方式并增加独立测试。
- `WrongAutoKey`、`InvalidAllocatorType`、`AutoRandomReadFailed` 在当前 Rust 生产引用搜索中未发现构造点；它们是对照 Go 标准错误保留的 API，不应据此宣称对应 Rust 路径已完整移植。

## 并发与资源生命周期

本文件不创建锁、线程、异步任务、通道、事务或网络连接，也没有 `Drop` 行为。其公开值可跨线程移动的能力来自所含基本类型，但本文件未显式宣告或校验 `Send` / `Sync` 边界。

并发语义发生在调用者中：`memid.rs` 在 `Mutex` 保护的水位更新中返回这些错误；`autoid_service.rs` 在连接重试、取消检查和退避周期中传播它们。错误值不持有锁守卫、事务引用或连接句柄，因而错误离开临界区后不会延长这些资源的生命周期。

## 与 Go 版本的对应关系

`pkg/meta/autoid/errors.go` 是最近的权威对照。Go 版通过 `dbterror.ClassAutoid.NewStd(mysql.Err...)` 绑定 MySQL/TiDB 错误码；Rust 版用 `thiserror` 枚举保留类别和文本，但没有在本文件中绑定 MySQL 数字错误码。大致映射为：`errInvalidTableID` ↔ `InvalidTableId`，`errInvalidIncrementAndOffset` ↔ `InvalidIncrementAndOffset`，`errNotImplemented` ↔ `NotImplemented`，`ErrAutoincReadFailed` ↔ `AutoIncrementReadFailed`，`ErrWrongAutoKey` ↔ `WrongAutoKey`，`ErrInvalidAllocatorType` ↔ `InvalidAllocatorType`，`ErrAutoRandReadFailed` ↔ `AutoRandomReadFailed`。Rust 另为存储、取消、RPC、服务端返回和重试终止增加了结构化 variant。

20 个 Rust `AUTO_RANDOM_*` 常量的文本与 Go 的 `AutoRandom*` 常量逐项对应，包括占位符。Go `pkg/ddl/tests/serial/serial_test.go::TestAutoRandom` 使用 `fmt.Sprintf` 和 `dbterror.ErrInvalidAutoRandom` 验证多个 DDL 边界；`pkg/meta/autoid/autoid_service_test.go` 验证 RPC 重试达限时既等价于 `ErrAutoincReadFailed` 又可被 `IsRPCRetryLimitError` 识别。Rust 对应回归分布在 `autoid_service_test.rs`、`autoid_service_1_aster_unit_test.rs` 和 `pkg/session/runtime/session_test.rs`。

## 扩展指南

- 新增错误类别时，先在 `AutoIdError` 增加具体 variant 和稳定 `#[error]` 文本，再在生产构造点使用该 variant；避免把可分类状态压成 `Storage(String)` 或 `Rpc(String)`。
- 修改 RPC 终止语义时，需同步检查 `autoid_service.rs::retry_rpc`、`domain.rs::DomainError::from_auto_id` 和 `session_test.rs` 的 error-chain 回归；性能风险主要来自误将非 RPC 错误纳入退避或让终止错误被 `INSERT IGNORE` 吞掉。
- 修改 `AUTO_RANDOM_*` 文案时，必须同步核对 `errors.go` 和 Go DDL/Executor 测试；如果将它们接入 Rust DDL，应在对应 Rust DDL 模块的独立 `*_test.rs` 中覆盖参数替换和完整错误文本，不把测试内嵌到 `errors.rs`。
- 若需保留底层 error source，应评估将字符串负载改为带 `#[source]` 的类型化负载。这会影响 `Clone` / `Eq` 派生、公开 API 兼容性和跨线程边界，不应仅为丰富日志而直接替换。
- 测试放置遵循 crate 的独立文件模式：分配语义放入 `autoid_test.rs` 或 `memid_test.rs`，远程重试/取消放入 `autoid_service_test.rs`，跨层错误链放入相应包的独立测试文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，其中 7,032 个 Rust 文件；`files --filter pkg/meta/autoid` 确认本 crate 的 Rust/Go 对照与独立测试面；`node --file pkg/meta/autoid/errors.rs --offset 1 --limit 240` 读取了全部 141 行并报告 22 个使用文件；`query` 确认 `AutoIdError`、三个函数和常量符号位置。
- 源码/Crate：`pkg/meta/autoid/errors.rs`、`pkg/meta/autoid/lib.rs`、`pkg/meta/autoid/Cargo.toml`、`pkg/meta/autoid/autoid.rs`、`pkg/meta/autoid/memid.rs`、`pkg/meta/autoid/autoid_service.rs`、`pkg/domain/domain.rs`、`pkg/domain/autoid_store.rs`、`pkg/autoid_service/autoid.rs`。
- Rust 测试：`pkg/meta/autoid/memid_test.rs` 验证 signed/unsigned ID 耗尽；`pkg/meta/autoid/autoid_test.rs` 验证非法 table ID 和取消；`pkg/meta/autoid/autoid_service_test.rs` 验证 RPC/Service/Canceled 分类与重试；`pkg/session/runtime/session_test.rs` 验证 `RpcRetryLimit` 在 SQL 错误链中保留身份。
- Go 对照：`pkg/meta/autoid/errors.go`、`pkg/meta/autoid/autoid.go`、`pkg/meta/autoid/autoid_service.go`；Go 测试 `pkg/meta/autoid/autoid_service_test.go` 和 `pkg/ddl/tests/serial/serial_test.go::TestAutoRandom`。
- 人工复核：用 `rg` 逐项检查 13 个 variant、3 个函数和 20 个 `AUTO_RANDOM_*` 常量的 Rust 引用，区分已接线符号与仅保留的 Go 兼容 API。本任务是纯文档分析，未运行 Cargo 或代码测试。
