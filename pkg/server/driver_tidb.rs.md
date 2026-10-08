# `pkg/server/driver_tidb.rs`

## 文件定位

`driver_tidb.rs` 是 `astersql-server` crate 中一份 TiDB 兼容驱动实现，由 [`pkg/server/lib.rs`](lib.rs) 以 `pub mod driver_tidb` 对外公开。它用三层对象承接 MySQL 连接语义：`TiDBDriver` 从存储抽象创建会话，`TiDBContext` 表示单连接会话，`TiDBStatement` 管理单个预处理语句。

当前 Rust 生产连接主链并未使用这组类型：仓库搜索中 `NewTiDBDriver` 及本文件的 `TiDBContext`/`TiDBStatement` 只被 [`pkg/server/driver_tidb_test.rs`](driver_tidb_test.rs) 直接使用；现行连接主链是 [`pkg/server/conn.rs`](conn.rs) 中的同名 `TiDBContext` trait 与 [`pkg/server/runtime.rs`](runtime.rs) 中的 `ConcreteTiDBContext`。另一份 [`pkg/server/driver.rs`](driver.rs) 定义了尚未由本文件类型实现的 `IDriver`/`PreparedStatement` 契约。因此本文件应视为已有行为的兼容移植面，而不是已接线的生产入口。

## 核心职责

- 用 `TiDBStore::create_session` 与 `TiDBSessionRuntime::configure` 为连接创建、配置会话运行时（`TiDBDriver::OpenCtx`）。
- 代理文本 SQL、预处理 SQL、列信息、警告与语句统计（`TiDBContext::{ExecuteStmt,Prepare,FieldList,GetWarnings,GetStmtStats}`）。
- 保存二进制协议预处理语句的长数据参数、参数类型、活跃游标、结果集和行容器（`TiDBStatement`）。
- 在 RESET/CLOSE 时按顺序释放长数据记账、游标迭代器和行容器（`TiDBStatement::{Reset,Close}`）。
- 为会话迁移编码/恢复预处理语句，并拒绝迁移尚有绑定参数或未取完游标的会话（`TiDBContext::{EncodeSessionStates,DecodeSessionStates}`）。
- 实施沙箱改密限制：非 restricted SQL 的沙箱会话仅允许 `SET PASSWORD` 和 `ALTER USER` 前缀（`checkSandBoxMode`）。

## 主要符号

- `Error(String)`：本文件的统一字符串错误，实现 `Display` 与 `std::error::Error`。
- `Expression`：预处理参数的最小值集，包含 NULL、有/无符号整数、浮点和字节。
- `ColumnInfo`、`SqlWarning`、`StatementStats`：分别承载协议列元数据、SQL 警告和执行统计。
- `PreparedStmtInfo` 与 `SessionStates`：会话迁移快照，保存 SQL、原数据库、文本协议名称和二进制协议参数类型。
- `PrepareResult`：运行时 prepare 的返回值，包含 ID、参数数、结果列和数据库名。当前 `TiDBContext::Prepare` 使用前三者，不读取 `database`。
- `ResultSet`、`CursorResultSet`、`RowContainer`：用于抽象普通结果集关闭、游标迭代器/RU 结算以及行容器 tracker 分离与关闭。
- `TiDBSessionRuntime`：本文件与真实会话的边界。它提供创建/执行/删除预处理语句、文本执行、警告、统计、库名与 next prepared ID 等操作；`long_data_memory` 默认无 tracker，`max_allowed_packet` 默认为 `astersql_sessionctx_vardef::DefMaxAllowedPacket`。
- `TiDBStore`：每连接会话工厂。`NewTiDBDriver` 只包装 `Arc<dyn TiDBStore>`。
- `TiDBDriver`：驱动持有者；`OpenCtx` 是其唯一行为入口。
- `TiDBContext`：持有 `Arc<dyn TiDBSessionRuntime>` 和 `Mutex<HashMap<u32, Arc<Mutex<TiDBStatement>>>>`。
- `TiDBStatement`：持有语句 ID、参数数量/长数据状态、参数类型、运行时引用、游标结果集、行容器、SQL 文本和活跃游标标志。

## 执行流程

1. 打开连接：`NewTiDBDriver` 保存 store；`OpenCtx` 调用 `create_session`，再用连接 ID、capability 和 collation 调用 `configure`，成功后创建空语句表。任一步错误都直接返回。
2. 文本执行：`ExecuteStmt` 先调用 `checkSandBoxMode`，通过后把 SQL 文本和 `non_transactional` 标志交给 `execute_statement`。此文件不解析 AST，只做带词边界的大写前缀比较。
3. Prepare：`TiDBContext::Prepare` 从运行时取得 `PrepareResult`，按 `param_count` 生成 MySQL BLOB 类型字节 `252` 的参数占位列，初始化等长 `bound_params`、`LongDataState`和语句状态，最后按 ID 登记到 `stmts`。
4. 参数与执行：`AppendParam` 将 COM_STMT_SEND_LONG_DATA 交给 [`pkg/server/conn_stmt.rs`](conn_stmt.rs) 的 `LongDataState::append`；`CheckLongDataSize` 在 EXECUTE 前检查延迟错误与当前 packet 上限；`Execute` 按 ID 调用 `execute_prepared`。
5. RESET/CLOSE：两者都先 `LongDataState::release`、上报 RU、关闭游标迭代器，然后分离 tracker 并关闭行容器。`Reset` 会清除活跃游标标志并 `take` 出两类资源；`Close` 保留字段和游标标志，最后调用 `drop_prepared`。
6. 编码迁移状态：`EncodeSessionStates` 先从运行时取完整 prepare 元数据，再遍历本地二进制语句；任一参数槽为 `Some` 或游标活跃都拒绝迁移，否则把 `params_type` 写回对应 ID。
7. 解码迁移状态：空快照立即返回且不改动 next ID。非空快照先保存 next ID 和 current DB，逐项将 next ID 设为 `id.wrapping_sub(1)` 并切换到原库：无名语句调用本地 `Prepare` 后恢复参数类型，有名语句委托 `prepare_named`。闭包无论成功失败结束后都恢复库名，并用 `saved_id.wrapping_sub(1)` 调用 setter。
8. 关闭连接：`TiDBContext::Close` 先在锁内 drain 本地语句表，然后逐个尝试加锁和关闭，最后关闭会话；语句锁毒化、语句关闭和会话关闭的清理错误均被忽略，唯有语句表锁毒化会返回错误。

## 数据与状态

`TiDBContext::stmts` 是连接级的二进制预处理语句索引。键是运行时生成的 `u32` ID，值使用 `Arc<Mutex<_>>` 使查找方可在释放 map 锁后持有语句。`GetStatement` 对负 `i32` 或超出 `u32` 的 ID 直接返回 `None`。

`bound_params: Vec<Option<Vec<u8>>>` 区分“未传参”（`None`）和“显式空字节”（`Some(Vec::new())`）。`bound_params_too_large` 与 `LongDataState` 内部的内存超限状态是延迟到 `CheckLongDataSize` 报告的；`Reset`/`Close` 通过 `release` 清空缓冲并退还记账。`params_type` 来自首次 COM_STMT_EXECUTE，也是会话迁移时二进制协议语句需额外保存的状态。

`result_set` 仅保留游标迭代器生命周期，`row_container` 保留预取行与其跟踪器，`has_active_cursor` 是迁移安全检查的独立标志。`SessionStates::prepared_stmts` 使用 `HashMap`，所以解码多个语句的遍历顺序不应被依赖。

## 依赖与调用关系

- crate 边界：[`pkg/server/Cargo.toml`](Cargo.toml) 声明 crate 名 `astersql-server`、库入口 `lib.rs` 且 `autotests = false`；因此 [`pkg/server/lib.rs`](lib.rs) 通过 `#[cfg(test)] #[path = "driver_tidb_test.rs"]` 显式挂载独立测试。
- 文件内直接外部依赖只有 `astersql-sessionctx-vardef`，用于 `max_allowed_packet` 默认值；该依赖已在 `Cargo.toml` 中指向 `../sessionctx/vardef`。
- crate 内依赖是 [`pkg/server/conn_stmt.rs`](conn_stmt.rs) 的 `LongDataMemory` 和 `LongDataState`，后者实际完成追加、packet/内存限额、延迟检错与释放。
- 向下调用全部通过 `TiDBSessionRuntime`、`ResultSet`、`CursorResultSet` 和 `RowContainer` trait 边界，文件本身不实现真实 session/store 适配器。
- RustCodeGraph 对本文件识别到 81 个符号，文件节点报告与 `driver_tidb_test.rs`、`sessionctx/variable/session.rs`、`sessionctx/variable/sysvar_test.rs` 存在索引关联；但精确 Rust 全仓搜索只找到 `driver_tidb_test.rs` 对本文件特有类型/构造函数的调用，其他两个关联是同名会话状态符号，不能视为本实现的生产调用边。
- Go 参考实现是 [`pkg/server/driver_tidb.go`](driver_tidb.go)，Go 对照测试是 [`pkg/server/driver_tidb_test.go`](driver_tidb_test.go)；Rust 行为测试是 [`pkg/server/driver_tidb_test.rs`](driver_tidb_test.rs)。

## 错误处理与边界

trait 边界的业务错误通过 `Result<_, Error>` 原样传播；`LongDataState` 错误被转换为字符串 `Error`。锁获取失败统一映射为 `statement lock poisoned`，但 `GetStatement` 为了保持 `Option` API 会将锁毒化折叠为 `None`，`TiDBContext::Close` 则会忽略单个语句锁毒化。

`Prepare` 只有在运行时 prepare 成功后才登记本地语句；但如果 map 锁毒化，运行时已经建立的 prepare 状态不会在此处回滚。`EncodeSessionStates` 要求运行时元数据包含每个本地 ID，否则返回 `prepared statement {id} not found`。`DecodeSessionStates` 遇错会恢复 next ID/current DB，但不回滚出错前已重建的语句。

沙箱检查是词边界感知的文本前缀策略，会拒绝 `SET PASSWORDLESS`/`ALTER USERNAME`；但它不如 Go 版的 AST 类型检查严格，任何以合法关键词前缀开头的整段文本都会通过此层，实际语法仍由下游运行时决定。

## 并发与资源生命周期

`TiDBStore` 和 `TiDBSessionRuntime` 必须 `Send + Sync`，共享所有权使用 `Arc`。语句 map 和每个语句分别使用 `Mutex`，因此 map 锁只负责定位/更新句柄，语句可在其后独立加锁。`Close` 先 drain 再逐句关闭，避免在执行运行时清理时长期持有 map 锁。

资源释放顺序是重要契约：先释放长数据及内存记账，再上报游标 RU 增量并关闭迭代器，然后分离行容器 tracker 并关闭容器，最后才 drop prepared statement。`Reset` 使用 `take` 确保已取走资源不再被访问；`Close` 没有 `take` ，重复调用是否安全取决于下游实现，本文件未提供幂等保证。

本文件没有自建线程、异步任务或通道。所有 API 是同步的，也没有为 `TiDBContext`/`TiDBStatement` 实现 `Drop`；调用方必须显式调用 `Close`/`Reset`。

## 与 Go 版本的对应关系

Rust 的 `TiDBDriver`/`TiDBContext`/`TiDBStatement` 及大部分 Go 风格方法名直接对应 [`pkg/server/driver_tidb.go`](driver_tidb.go) 的同名符号。`Prepare` 的 BLOB 参数占位、长数据延迟报错、RESET 清空游标、CLOSE 保留游标标志、Close 忽略清理错误以及会话迁移的基本不变量均保留。

已确认的简化/差异有：

- Go `OpenCtx` 直接创建 session，配置 TLS、collation、capability、connection ID、Starter 模式 packet 上限、session-state handler 和 extensions；Rust 将它们折叠为 `TiDBStore`/`configure` 边界，且 API 不接收 db name/TLS/extensions。
- Go `ExecuteStmt` 接收 AST，为 `NonTransactionalDMLStmt` 选择专用路径并把错误追加到 statement context；Rust 传递字符串和布尔标志，不维护该错误列表。
- Go 沙箱规则按 `SetPwdStmt`/`AlterUserStmt` AST 类型判定；Rust 使用带词边界的文本前缀。
- Go `FieldList` 执行 `column.ConvertColumnInfo`；Rust 要求运行时直接返回本地 `ColumnInfo`。Go 的 `TestConvertColumnInfo` 与 Rust 的 `canonical_convert_column_info_preserves_mysql_display_width_rules` 实际验证的是独立 `internal/column` 转换器，不是本文件中的转换代码。
- Go `TiDBStatement::Close` 根据事务重试和 plan-cache 配置选择 drop/remove/cache delete，并删除 `tc.stmts` 项；Rust 总是调用 `runtime.drop_prepared` 且不从 `TiDBContext::stmts` 移除单项，只在 context 关闭时 drain。
- Go 迁移编码直接从 session vars 组装元数据；Rust 要求 `prepared_metadata` 提供。Go 有名语句通过转义后的 `PREPARE` SQL 解析/执行；Rust 委托 `prepare_named`。
- Go 的 next-ID 算术使用无符号减一；Rust 显式使用 `wrapping_sub(1)` 保留零值环绕语义。

## 扩展指南

- 要把此实现接入生产主链，首先需明确它与 [`pkg/server/driver.rs`](driver.rs) 及 [`pkg/server/conn.rs`](conn.rs) 的重叠契约；不应只添加一个 trait impl 就假定类型可互换，因为错误、表达式、结果集、TLS/extensions 参数和取行 API 均不同。
- 新增会话功能应先扩展 `TiDBSessionRuntime`，再在 `TiDBContext` 中保持“检查—委托—错误传播”边界，同时在独立的 [`pkg/server/driver_tidb_test.rs`](driver_tidb_test.rs) 中用 mock runtime 验证，不要把测试内嵌到源文件。
- 修改长数据行为时必须同步检查 [`pkg/server/conn_stmt.rs`](conn_stmt.rs) 与其独立测试 [`pkg/server/conn_stmt_test.rs`](conn_stmt_test.rs)，覆盖空分片、非法参数 ID、packet 上限、内存配额、RESET 和 CLOSE 退还记账。
- 修改 RESET/CLOSE 时要保留 RU 上报在 iterator close 之前、tracker detach 在 container close 之前的顺序，并决定是否需要保持当前 `Reset` 清游标而 `Close` 不清标志的 Go 语义。
- 修改迁移时应添加编码成功、绑定参数/活跃游标拒绝、元数据缺 ID、有名/无名恢复、中途失败以及 next ID/current DB 恢复测试；当前 Rust 测试只直接覆盖空快照 no-op。
- 修改 Go 对齐语义时，要同时对照 `driver_tidb.go` 和 `driver_tidb_test.go`，但应注意 Go 测试目前仅直接覆盖 `column.ConvertColumnInfo`，不能当作本文件所有边界的回归保障。
- 性能敏感点包括每次 `WarningCount` 克隆整个警告向量、Prepare 的参数占位分配、迁移时的全表锁定/克隆和 Close 的全表 drain；优化前应保留上述一致性语义。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/server/driver_tidb.rs` 确认目标文件被索引且有 81 个符号；`query` 区分了 Rust/Go 的 `TiDBDriver` 以及 Go 的 `OpenCtx`/`EncodeSessionStates`/`DecodeSessionStates`；`node --file ... --offset 140 --limit 340` 核对了 Rust 主体代码和索引关联文件。自然语言 `explore` 与部分 `callers/callees` 查询在 30 秒限时内未产生结果，因此调用面另用精确仓库搜索复核。
- Rust 源与模块：[`pkg/server/driver_tidb.rs`](driver_tidb.rs)、[`pkg/server/lib.rs`](lib.rs)、[`pkg/server/driver.rs`](driver.rs)、[`pkg/server/conn.rs`](conn.rs)、[`pkg/server/runtime.rs`](runtime.rs)、[`pkg/server/conn_stmt.rs`](conn_stmt.rs)。
- crate 声明：[`pkg/server/Cargo.toml`](Cargo.toml)，确认 `astersql-server` 边界、`lib.rs` 入口、禁用自动测试发现和 `astersql-sessionctx-vardef` 依赖。
- Rust 测试：[`pkg/server/driver_tidb_test.rs`](driver_tidb_test.rs) 验证空迁移快照 no-op、context close 忽略清理错误、statement close 释放长数据但保留游标标志、沙箱允许集、packet/内存配额延迟错误及 RESET/CLOSE 内存退还。
- Go 对照：[`pkg/server/driver_tidb.go`](driver_tidb.go) 的同名类型与方法，以及 [`pkg/server/driver_tidb_test.go`](driver_tidb_test.go) 唯一直接测试 `TestConvertColumnInfo`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求本文档存在且上述固定二级标题恰好 11 个。
