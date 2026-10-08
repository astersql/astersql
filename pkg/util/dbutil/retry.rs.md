# `pkg/util/dbutil/retry.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-util-dbutil`（见 `pkg/util/dbutil/Cargo.toml`），由 crate 入口 `pkg/util/dbutil/lib.rs` 通过 `pub mod retry` 公开。它位于 dbutil 的 SQL 执行辅助层：自身不连接数据库、不执行 SQL，也不实施重试，只把一个已经规范化为 `crate::interface::DbError` 的数据库错误分类为“可由上层直接重试”或“不可直接重试”。当前生产调用点是 `pkg/util/dbutil/common.rs` 的 `ExecSQLWithRetry`。

## 核心职责

- `IsRetryableError` 维护与 Go `pkg/util/dbutil/retry.go` 相同的直接重试白名单，覆盖 MySQL 死锁以及一组 TiDB/PD/TiKV 暂态错误。
- 对旧 TiDB 常把 schema 过期或变化包装为 1105（`ErrUnknown`）的情况，使用消息子串进行兼容识别。
- 对只在特定上下文中可重试或明确不可重试的错误保持保守：不在白名单中的错误一律返回 `false`。此策略由 `pkg/util/dbutil/retry_test.rs::conditionally_retryable_and_unknown_codes_remain_non_retryable` 验证。
- 本文件只回答分类问题；重试次数、等待时间、SQL 再执行和最终错误返回均由 `common.rs::ExecSQLWithRetry` 负责。

## 主要符号

- `pub const Retryable1105Msgs: &[&str]`：1105 错误的兼容消息表，当前恰含 `Information schema is out of date` 与 `Information schema is changed`。命名沿用 Go API；匹配语义为大小写敏感的子串匹配。
- `pub const RETRYABLE_ERROR_CODES: &[u16]`：无条件直接重试的错误码表。当前包括 1213（锁死锁）、9001（PD 超时）、9003（TiKV 忙）、9004（解析锁超时）、8027/8028（information schema 过期/变化）、8005/9007（写冲突）、8022（事务可重试）与 8245（列正在变更）。
- `pub fn IsRetryableError(error: &DbError) -> bool`：唯一函数入口。参数是 `interface.rs::DbError { code, sql_state, message }` 的共享借用，不修改也不消费错误；返回值仅表示该错误分类是否允许上层自动重试。

文件内没有类型、trait、`impl`、宏或条件编译项。两个常量和函数均为公开符号，但 `lib.rs` 没有把它们提升到 crate 根；调用方通过 `astersql_util_dbutil::retry::...`（crate 外）或 `crate::retry::...`（crate 内）访问。

## 执行流程

1. `IsRetryableError` 先用切片 `contains` 检查 `error.code` 是否位于 `RETRYABLE_ERROR_CODES`；命中即短路返回 `true`。
2. 未命中时，仅当错误码等于 1105 才进入旧版本兼容分支。
3. 兼容分支遍历 `Retryable1105Msgs`，对 `error.message` 执行 `str::contains`；任一大小写完全一致的子串命中即返回 `true`。
4. 错误码既不在白名单中，或 1105 消息未命中时，整个布尔表达式返回 `false`。

在实际执行链中，`common.rs::ExecSQLWithRetry` 调用 `DBExecutor::ExecContext`；成功或可忽略 DDL 错误立即成功，失败时调用本函数。若分类为可重试，它记录最近错误、在尚有下一次机会时睡眠 10 ms，并在 `DefaultRetryTime`（10）次上限内重试；耗尽后返回最后一次错误。分类为不可重试则立即原样返回该次错误。

## 数据与状态

本文件只有两个编译期只读切片，不含可变静态数据、缓存、计数器或持久化状态。`DbError.sql_state` 不参与判断；分类完全由 `code` 以及 1105 时的 `message` 决定。因此同一输入总得到同一输出，函数没有副作用。

错误码使用 `u16`，与 `DbError.code` 类型一致。白名单查找和 1105 消息查找均为线性扫描；当前集合分别只有 10 项和 2 项，开销固定且很小。消息匹配不做大小写折叠、去空白、正则解析或本地化处理。

## 依赖与调用关系

- 直接上游：`pkg/util/dbutil/common.rs::ExecSQLWithRetry` 通过 `use crate::retry::IsRetryableError` 调用分类函数；`pkg/util/dbutil/retry_test.rs` 直接覆盖其分类合同。
- 直接下游：仅依赖 `pkg/util/dbutil/interface.rs::DbError` 的 `code` 和 `message` 字段，以及标准库切片/字符串的 `contains`；RustCodeGraph 对该函数的精确 callees 查询未产生额外函数边。
- 模块接线：`pkg/util/dbutil/lib.rs` 声明 `pub mod retry`，并以 `#[path = "retry_test.rs"] mod retry_test` 保持测试与生产文件分离。
- crate 边界：`pkg/util/dbutil/Cargo.toml` 将 lib 路径设为 `lib.rs`，porting 元数据指向 Go 包 `pkg/util/dbutil`。本文件不直接使用该 manifest 中唯一的常规外部依赖 `astersql-infoschema`，也不受其中 Windows 条件依赖影响。
- 应用位置：这是 dbutil 写 SQL 辅助链中的错误分类叶节点，不是全仓库统一重试策略；不能据此推断其他子系统的重试行为。

## 错误处理与边界

函数不返回 `Result`，不会创建、包装或记录错误；所有未知情况保守返回 `false`。这意味着 1046、9002、8020、1317、9005、1064 等未列入的错误即使文本看似暂态也不会自动重试。特别地，Go 注释明确将 TiKV server timeout、table locked 视为仅特定场景可重试，将 query interrupted 视为不可重试，将 region unavailable 视为未知；Rust 测试固定了这些排除项。

1105 分支是精确大小写的子串判断：带前后文的 `server: Information schema is changed; retry` 会命中，而小写 `information schema is changed` 不会命中。非 1105 错误不会因消息相同而走兼容分支，除非其独立错误码本身已在白名单中。

Rust 入口要求调用者已经得到 `DbError`，所以不存在 Go 测试中的 `nil`、任意非 MySQL 错误或错误链解包分支。将驱动错误映射为 `DbError` 的边界不在本文件；如果映射丢失原始 MySQL/TiDB 错误码，函数将无法补救。

## 并发与资源生命周期

`IsRetryableError` 只读取不可变参数和静态切片，没有锁、原子变量、线程、异步任务、通道、连接、事务或需要释放的资源；因此可由多个线程并发调用。`DbError` 仅在调用期间被共享借用，生命周期不越过函数返回。

重试造成的线程阻塞与数据库资源生命周期属于上游：`common.rs::ExecSQLWithRetry` 使用 `std::thread::sleep(10 ms)`，并对同一个 `&dyn DBExecutor` 重新调用 `ExecContext`。本文件既不提供退避/抖动，也不处理取消或超时；扩充分类会直接扩大上游可能重复执行 SQL 的范围，必须考虑语句幂等性与连接状态。

## 与 Go 版本的对应关系

Rust 的两个消息、10 个无条件重试错误码、1105 子串匹配顺序和大小写语义与 `pkg/util/dbutil/retry.go::IsRetryableError` 对齐。`pkg/util/dbutil/retry_test.rs` 还把 Go 中“条件性/不可/未知”错误的排除策略编码成独立回归测试。

主要边界差异在输入模型：Go 接收任意 `error`，先用 `errors.Cause` 解包，再只接受 `*mysql.MySQLError`，因而自然覆盖 `nil`、普通错误和连接哨兵错误；Rust 接收非空的 `&DbError`，没有错误链或运行时类型判断。另一个维护差异是 Go 通过 `pkg/errno` 常量表达错误码，Rust 当前在 `RETRYABLE_ERROR_CODES` 中使用带注释的数值字面量。修改列表时必须同时核对 Go 常量的数值，避免错误码漂移。

## 扩展指南

- 新增或删除无条件可重试错误时修改 `RETRYABLE_ERROR_CODES`，并在 `pkg/util/dbutil/retry_test.rs` 增加正例及相邻的反例；同时核对 `pkg/util/dbutil/retry.go` 与 `retry_test.go`，确认这是对齐而非 Rust 单边策略。
- 扩充旧 1105 兼容文本时修改 `Retryable1105Msgs`，至少覆盖精确命中、嵌入上下文、大小写差异和“不应对其他错误码生效”四类边界。消息策略会受服务端版本及文本变化影响，优先使用独立错误码，避免宽泛子串造成误重试。
- 若要支持任意驱动错误或错误链，应在 `interface.rs::DbError` 的转换边界设计明确的解包/映射合同，不应在本函数里用文本猜测所有错误类型。
- 若要改变次数、间隔、退避、取消或超时，应修改 `common.rs::ExecSQLWithRetry` 及其独立测试，而不是把执行策略塞入本分类文件。
- 所有更改都应继续保持生产逻辑与测试逻辑分文件；除分类测试外，还应同步 `common_test.rs` 中 `ExecSQLWithRetry` 的集成式单元场景，验证重试次数、停止条件和最终错误。

## 验证依据

- RustCodeGraph：`status` 显示索引含 `pkg/util/dbutil/retry.rs`；`files --filter pkg/util/dbutil` 定位 Rust/Go 源与独立测试；`node --file pkg/util/dbutil/retry.rs` 核对 53 行完整实现，并报告该文件被 `common.rs`、`retry_test.rs` 使用；`query IsRetryableError` 定位目标函数及同路径 Go 函数；`query RETRYABLE_ERROR_CODES`、`query Retryable1105Msgs` 定位两个常量。精确 `callers`/`callees` 未返回可用输出，因此调用细节又由局部源码搜索核验。
- 已读生产与接线文件：`pkg/util/dbutil/retry.rs`、`common.rs`（尤其 `ExecSQLWithRetry`）、`interface.rs`（`DbError`/`DBExecutor`）、`lib.rs`、`Cargo.toml`，以及根 `Cargo.toml` 的 workspace/facade 声明。
- 已读 Go 对照：`pkg/util/dbutil/retry.go`；已读测试：`pkg/util/dbutil/retry_test.rs`、`retry_test.go`、`common_test.rs` 中的 `ExecSQLWithRetry` 场景。
- 本任务是纯文档分析，按计划不运行 Cargo；交付时使用任务给定的 11 章节结构命令验证，并人工复核本文只陈述上述源码、调用边和测试能够支持的事实。
