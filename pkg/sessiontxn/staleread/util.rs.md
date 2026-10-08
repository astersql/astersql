# `pkg/sessiontxn/staleread/util.rs`

## 文件定位

`util.rs` 位于 `astersql-sessiontxn-staleread` crate，是 Rust 过期读实现的公共数据模型与时间戳工具层。crate 入口 `pkg/sessiontxn/staleread/lib.rs` 将本模块公开并重新导出其符号；同 crate 的 `processor.rs` 使用这里的时间戳计算、语句状态和快照 InfoSchema 工具，`provider.rs` 使用这里的会话、事务、快照模型与 InfoSchema 工具，`failpoint.rs` 查询语句是否为过期读。

该文件目前不是对真实 TiDB `sessionctx`、PD、TiKV 类型的直接绑定，而是以 `SessionBackend` trait 和一组轻量 Rust 类型隔离外部系统。`pkg/sessiontxn/staleread/Cargo.toml` 中完整 TiDB 子 crate 依赖均位于永不成立的 `target.'cfg(any())'.dependencies` 下；当前可编译边界只依赖标准库以及 crate 内的 `Error`、`ErrorKind`。因此文中“后端”“会话”“事务”“快照”均指当前 Rust 抽象，不能据此宣称已接入完整 SQL 引擎。

## 核心职责

本文件承担四组职责：

1. 定义过期读所需的最小公共模型，包括 `Datum`、`Expression`、`InfoSchema`、`ReplicaRead`、`TransactionContext`、`Snapshot`、`Transaction`、`Session` 与线程安全别名 `SessionRef`。
2. 通过 `SessionBackend` 抽象表达式求值、日期时间解析、读时间戳校验、PD/外部时间戳获取、快照元数据获取和事务/快照创建，供生产逻辑与测试后端以相同接口驱动。
3. 将 `AS OF TIMESTAMP`、`tidb_read_staleness` 和 `tidb_external_ts` 转换成确定的 TSO，并维持语句级缓存与过期读标记。
4. 提供 TSO 编解码及快照 InfoSchema 装饰工具，使 `processor.rs` 和 `provider.rs` 不重复实现边界规则。

当前实现刻意把 SQL AST、时区转换、PD oracle、真实 InfoSchema 和临时表包装隐藏在 `SessionBackend` 后面；这些行为由后端实现负责，而不是在本文件内完成。

## 主要符号

- `MIN_TSO_PHYSICAL_MS` 是合法原始 TSO 的物理毫秒下界，值为 2013-01-01 00:00:00 UTC；`TSO_LOGICAL_BITS` 固定为 18，与 Go oracle 的 TSO 编码一致。
- `Context` 与 `Expression(String)` 是上下文和已编译表达式的轻量占位。`Expression` 不解析 SQL；测试后端把字符串解释为测试规格。
- `Datum::{Null, String, Bytes, Int, Uint, DateTimeMillis}` 只覆盖 AS OF 求值所需的 Go `types.Datum` 子集。
- `InfoSchema` 保存 `snapshot_ts` 以及是否挂接本地临时表。`get_session_snapshot_info_schema` 总会把 `local_temporary_tables_attached` 置为 `true`。
- `ReplicaRead::{Leader, Follower, Mixed}` 表示副本偏好；`is_follower_read` 对 `Follower` 和 `Mixed` 返回真。
- `TransactionContext`、`Snapshot`、`Transaction` 保存过期读事务链需要的时间戳、只读标志、事务作用域、断言级别、分片步长和临时表拦截状态。它们主要由 `provider.rs` 消费。
- `SessionBackend: Send + Sync` 是外部能力边界。其方法按职责分为表达式/时间戳求值、读安全校验、元数据读取、事务创建和快照创建；调用者均通过 `Arc<dyn SessionBackend>` 共享实现。
- `Session` 汇总语句与事务状态。`new` 设置 autocommit、global scope、Leader read 等默认值；`use_txn_read_ts` 在返回事务读 ts 时标记已消费；`begin_statement` 清除语句过期读标志以及 stale/external ts 缓存。
- `SessionRef = Arc<Mutex<Session>>` 是所有公共流程共享的会话句柄。
- `calculate_as_of_ts_expr` 将 AS OF 表达式求值为 TSO；`tso_from_datum` 处理原始 TSO 的 Datum 形态。
- `calculate_ts_with_read_staleness` 根据语句当前时间、负偏移和 GC 最小安全时间计算读 ts。
- `is_stmt_staleness`、`get_external_timestamp`、`get_session_snapshot_info_schema` 分别读取语句标记、解析并缓存外部 ts、获取并装饰快照 InfoSchema。
- `millis_to_tso` 和 `extract_physical` 实现物理毫秒与 TSO 高位之间的转换。

所有上述类型、常量和函数均为 `pub`；只有 `Session.stale_tso_cache` 与 `Session.external_ts_cache` 是模块私有字段，防止调用者绕过缓存初始化规则。

## 执行流程

`calculate_as_of_ts_expr(session, expression)` 的主流程如下：

1. 锁住 `SessionRef`；锁中毒时返回 Backend 错误。
2. 若 `stale_tso_cache` 尚未初始化，调用一次 `SessionBackend::stale_timestamp` 并缓存其 `Result`。该值只用于维持 Go 求值路径的预热/缓存语义，本函数后续不直接读取该 TSO；即使预热返回错误，当前实现也不会立即传播该错误。
3. 使用同一后端调用 `evaluate_expression`，随后释放会话锁。
4. `Datum::Null` 直接返回 AsOf 错误。其他值先尝试 `parse_datetime_millis`；成功时由 `millis_to_tso` 左移 18 位并返回。
5. 日期时间解析失败后，`tso_from_datum` 尝试把字符串、UTF-8 字节串或正整数解释为原始 TSO。解析失败返回统一 AsOf 错误。
6. 原始 TSO 的物理部分必须严格大于 `MIN_TSO_PHYSICAL_MS`，随后调用 `validate_snapshot_read_ts` 防止不安全的未来读。注意：日期时间分支在本函数中不调用该校验；上层 `processor.rs::parse_and_validate_as_of` 会再次统一校验解析结果。

`calculate_ts_with_read_staleness(session, offset)` 先从后端取语句时间 `now`，用 `saturating_add` 加偏移得到请求时间，再取 GC safe point。选择规则与 Go `expression.CalAppropriateTime` 对齐：safe point 早于请求时间时使用请求时间；safe point 晚于当前时间时退回当前时间；其余情况钳制到 safe point。结果经 `millis_to_tso` 转换；只有最终时间严格晚于 safe point 时才额外调用读 ts 校验。

`get_external_timestamp` 在持有会话锁时检查 `external_ts_cache`。缓存为空时调用后端；成功结果写入缓存，失败转换成 `ErrorKind::AsOf` 但不写入缓存，因此下次调用会重试。`StaleReadProcessor::new` 会调用 `Session::begin_statement`，使该缓存以语句为生命周期重置。

`get_session_snapshot_info_schema` 只在锁内克隆后端，然后释放锁、调用 `snapshot_info_schema(snapshot_ts)`，最后把本地临时表挂接标记强制设为真。这避免在可能较慢的后端调用期间长期持有会话锁。

## 数据与状态

TSO 采用 `physical_milliseconds << 18 | logical_part`。`millis_to_tso` 只生成逻辑部分为零的 TSO；从用户提供的原始 TSO 进入时，`calculate_as_of_ts_expr` 原样保留低 18 位。`extract_physical` 丢弃逻辑位并以 `i64` 返回物理毫秒。

`Session` 中与本文件流程直接相关的状态包括：

- `txn_read_ts` 与 `txn_read_ts_used`：读取事务级 AS OF 时间戳及其消费标记。
- `read_staleness_millis`：相对语句当前时间的毫秒偏移，按 TiDB 用法通常为负数；本函数没有单独拒绝正数，而是依赖读 ts 校验。
- `enable_external_ts_read`、`restricted_sql`：由 `processor.rs` 决定是否调用外部时间戳路径。
- `statement_is_staleness`：由 processor 固化求值结果，`is_stmt_staleness` 读取。
- `stale_tso_cache` 与 `external_ts_cache`：语句级缓存，由 `begin_statement` 清空。
- `txn_context`、`active_transaction`、副本读和断言/分片配置：供 `provider.rs` 创建只读历史事务及快照。

`InfoSchema`、`Snapshot` 和 `Transaction` 当前都是值类型桩，克隆会复制状态而不是共享真实存储句柄；后续接入真实实现时不能沿用这一资源语义而不评估代价。

## 依赖与调用关系

上游生产调用关系经 RustCodeGraph 与源码搜索核对如下：

- `processor.rs::parse_and_validate_as_of` 调用 `calculate_as_of_ts_expr`，再通过后端统一校验结果；`StaleReadProcessor` 的 AS OF、read-staleness、external-ts 分支分别调用 `calculate_as_of_ts_expr`、`calculate_ts_with_read_staleness`、`get_external_timestamp`，并通过 `get_session_snapshot_info_schema` 固化元数据视图。
- `processor.rs::StaleReadProcessor::new` 调用 `Session::begin_statement`；`BaseProcessor::set_evaluated_values` 更新 `statement_is_staleness`。
- `provider.rs::StalenessTxnContextProvider` 使用 `SessionRef` 及事务/快照模型，并在激活或替换 provider 时调用 `get_session_snapshot_info_schema`。
- `failpoint.rs::assert_stmt_staleness` 调用 `is_stmt_staleness`。

主要下游边为：`calculate_as_of_ts_expr -> SessionBackend::{stale_timestamp,evaluate_expression,parse_datetime_millis,validate_snapshot_read_ts}`，以及 `tso_from_datum`、`millis_to_tso`、`extract_physical`；`calculate_ts_with_read_staleness -> SessionBackend::{statement_timestamp_millis,statement_min_safe_millis,validate_snapshot_read_ts}` 和 `millis_to_tso`；`get_external_timestamp -> SessionBackend::external_timestamp`；`get_session_snapshot_info_schema -> SessionBackend::snapshot_info_schema`。

RustCodeGraph 的文件级输出还把 `pkg/executor/set.rs` 列为使用文件，但精确符号调用查询与 `rg` 未发现它引用本 crate 的上述符号；该文件只有自己 `SetBackend::validate_snapshot_read_ts` 的同名概念。因此本说明不把它列为已验证的直接调用者。

## 错误处理与边界

- 所有需要锁住 `SessionRef` 的可失败函数都把 poisoned mutex 映射为 `Error::backend("session lock poisoned")`。`is_stmt_staleness` 是例外：锁失败时静默返回 `false`，适合断言/查询但可能掩盖内部故障。
- AS OF 的 NULL、无法解析、早于或等于 2013 下界分别产生明确的 AsOf 错误。字符串 `"0"` 可被 `u64` 解析，随后走“早于 2013”分支；有符号整数零或负数则在 `tso_from_datum` 中不可解析。
- 日期时间转 TSO 时，负毫秒由 `millis_to_tso` 返回 `InvalidTimestamp`；非负值直接左移。这里没有显式防止极大毫秒在左移时截断高位，后端输入实现需约束范围。
- 原始 TSO 分支会在本函数中校验未来时间；日期时间分支依赖上层 `parse_and_validate_as_of` 的再次校验。直接调用 `calculate_as_of_ts_expr` 的新代码必须理解这一区别。
- `calculate_ts_with_read_staleness` 用饱和加法避免 `i64` 加法溢出，并通过 safe point 钳制过旧读；若 safe point 本身晚于 `now`，回退 `now`。后端取时、safe point 或校验错误原样传播。
- 外部时间戳后端错误被改标为 AsOf 并保留消息；错误不缓存。成功值包括零都会缓存。
- 快照 InfoSchema 后端错误原样传播；只有成功后才修改临时表标记。

## 并发与资源生命周期

`SessionBackend` 必须同时满足 `Send + Sync`，并通过 `Arc` 跨会话使用者共享；`SessionRef` 用 `Arc<Mutex<Session>>` 串行化可变会话状态。`calculate_as_of_ts_expr` 在锁内调用表达式求值和 stale timestamp 预热，后端实现若回调同一 `SessionRef` 可能死锁；其他较慢路径通常先克隆后端再释放锁。

`stale_tso_cache` 和 `external_ts_cache` 的生命周期是一个 Rust `StaleReadProcessor` 语句：构造 processor 时 `begin_statement` 重置二者。外部 ts 成功值在同一语句内保持确定性，失败可重试；stale ts 缓存保存 `Result`，包括错误。`txn_read_ts_used` 则跨调用保留，直到更高层会话逻辑清理。

本文件本身不创建线程、异步任务或通道，也不拥有网络连接。`Transaction`、`Snapshot` 和 `InfoSchema` 目前为普通内存值，真实 PD/TiKV 资源的建立与释放被委托给未来的 `SessionBackend` 实现及 `provider.rs` 的调用流程。

## 与 Go 版本的对应关系

直接 Go 对照为 `pkg/sessiontxn/staleread/util.go`，另有 `processor.go::GetSessionSnapshotInfoSchema`：

- Rust `calculate_as_of_ts_expr` 对应 Go `CalculateAsOfTsExpr`：二者都先建立 stale timestamp provider/缓存语义、求值表达式、拒绝 NULL、优先按日期时间解析、再回退原始 TSO，并用 2013 下界及快照读校验保护原始 TSO。
- Rust `tso_from_datum` 对应 Go `tsoFromDatum`，接受字符串、字节、正 `int64` 和正 `uint64`。Rust 特意保留字符串零可解析，从而与 Go `strconv.ParseUint("0")` 的错误分类一致。
- Rust `calculate_ts_with_read_staleness` 对应 Go `CalculateTsWithReadStaleness`：`statement_timestamp_millis`、`statement_min_safe_millis` 和手写选择分支分别抽象 Go 的 `GetStmtTimestamp`、`GetStmtMinSafeTime`、`CalAppropriateTime`。
- Rust `is_stmt_staleness` 对应 Go `IsStmtStaleness`；Rust 从 mutex 包装的字段读取，Go 直接读 `StmtCtx.IsStaleness`。
- Rust `get_external_timestamp` 对应 Go `GetExternalTimestamp`：均确保同一语句确定性并将错误包装成 AS OF。Rust 缓存在 `Session`，Go 缓存在 `StatementContext`；Rust `begin_statement` 是保证语句边界的关键补偿机制。
- Rust `get_session_snapshot_info_schema` 对应 Go `processor.go::GetSessionSnapshotInfoSchema`：Go 从 domain 获取真实历史 InfoSchema 并用 `temptable.AttachLocalTemporaryTableInfoSchema` 包装；Rust 调后端并以布尔字段记录已挂接。

差异与迁移状态：Rust 用自定义 Datum/Expression/InfoSchema/Transaction 桩和毫秒整数代替真实 Go 类型；时区、MySQL 类型转换、failpoint 和 oracle 精度由测试后端模拟。Cargo 中真实子系统依赖目前被 `cfg(any())` 禁用，所以这是行为对齐的隔离实现，不是完整生产集成。

## 扩展指南

- 新增 AS OF 可接受类型或解析规则时，优先修改 `Datum`、`SessionBackend::parse_datetime_millis` 契约和 `tso_from_datum`，并同步独立测试 `processor_test.rs`；必须维持“日期时间优先于原始整数 TSO”的兼容顺序及字符串零的错误分类。
- 调整 read-staleness 钳制时修改 `calculate_ts_with_read_staleness`，并新增针对 `safe < requested`、`requested <= safe <= now`、`safe > now` 和未来校验分支的独立 Rust 测试。当前 Rust 测试主要通过 processor 覆盖常规负偏移，safe-point 三分支缺少直接单元用例。
- 改变缓存生命周期时同时审查 `Session::begin_statement`、`StaleReadProcessor::new`、`get_external_timestamp` 和 stale ts 预热路径；同步 `processor_test.rs::external_timestamp_cache_is_reset_for_each_processor_statement`、`externalts_test.rs` 与 `util_test.rs`，避免把失败意外缓存。
- 接入真实 sessionctx/PD/TiKV 时应实现或替换 `SessionBackend`，而不是把网络/存储细节散入计算函数；同时启用真实 Cargo 依赖前需要核对 `cfg(any())` 清单，并验证锁外调用以免重入死锁。
- 扩充事务或快照字段时同步 `TransactionContext`、`Snapshot`、`Transaction`、`provider.rs` 和 `provider_test.rs`。测试逻辑必须继续放在独立 `*_test.rs` 文件，不嵌入 `util.rs`。
- 若改变快照 InfoSchema 装饰逻辑，应同步 `get_session_snapshot_info_schema`、processor/provider 调用点和临时表断言，并与 Go `GetSessionSnapshotInfoSchema` 的包装顺序对齐。

兼容风险集中在 SQL 字面量解析优先级、TSO 逻辑位保留、错误种类/消息和语句级缓存确定性；性能风险集中在 mutex 临界区内的后端调用与对值型 InfoSchema/Transaction 的克隆。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标目录包含 `util.rs`、`processor.rs`、`provider.rs`、相关 Go 文件和独立测试。
- RustCodeGraph 源码读取：`node --file pkg/sessiontxn/staleread/util.rs --offset 1 --limit 400`，确认目标文件 346 行、全部公开符号和函数体。
- RustCodeGraph 符号/调用查询：对 `calculate_as_of_ts_expr`、`calculate_ts_with_read_staleness`、`get_external_timestamp`、`get_session_snapshot_info_schema`、`is_stmt_staleness`、`millis_to_tso`、`tso_from_datum`、`Session::begin_statement` 执行 `query`、`callers`、`callees`。图确认内部边，例如 `calculate_as_of_ts_expr -> evaluate_expression/parse_datetime_millis/validate_snapshot_read_ts/stale_timestamp/tso_from_datum/millis_to_tso/extract_physical`；跨模块 caller 缺失处用精确源码搜索补齐。
- 读取的生产与配置文件：`pkg/sessiontxn/staleread/util.rs`、`lib.rs`、`Cargo.toml`、`processor.rs`、`provider.rs`、`failpoint.rs`，以及 Go 对照 `util.go`、`processor.go`、`provider.go`；另核对根 `Cargo.toml` 的 workspace 成员和 `facade_sessiontxn_staleread` 路径别名。
- 读取/检索的独立测试：`util_test.rs`、`main_test.rs`、`processor_test.rs`、`externalts_test.rs`、`provider_test.rs`，以及 Go `processor_test.go`、`provider_test.go`、`externalts_test.go`。这些测试证实 NULL/坏格式/旧 TSO/未来 ts、逻辑位保留、read-staleness 优先级、外部 ts 成功缓存与失败不缓存、语句重置、临时表标记和 provider 事务状态。
- 未运行 Cargo：本任务是纯文档分析，计划明确禁止 Cargo。结构验证命令及最终退出状态在任务交付时记录。
