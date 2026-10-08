# `pkg/server/conn_stmt.rs`

## 文件定位

[`conn_stmt.rs`](conn_stmt.rs) 位于 `astersql-server` crate，crate 根由 [`pkg/server/Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 指向 [`lib.rs`](lib.rs)，后者通过 `pub mod conn_stmt` 暴露本模块。它承载 MySQL 二进制预处理语句协议的共享状态和协议语义，覆盖 `COM_STMT_PREPARE`、`COM_STMT_EXECUTE`、`COM_STMT_FETCH`、`COM_STMT_CLOSE`、`COM_STMT_SEND_LONG_DATA`、`COM_STMT_RESET` 与 `COM_SET_OPTION`。

本文件同时存在两种接线层次，阅读时必须区分：

- 生产连接主链是 [`conn.rs`](conn.rs) 的 `ClientConn::dispatch` → `ClientConn::handleStmt`。它直接复用本文件的 `PreparedStatement`、`LongDataState`、`ParseExecuteParams` 和 `Error`，但自行负责真实 Session 调用、PacketIO 写包、flush 和生产游标。
- 本文件的 `clientConn`、`StatementRuntime`、`ProtocolEvent` 及 `HandleStmtPrepare` 等自由函数是一套精简的可注入运行时/可观察事件模型，完整表达协议状态转换、重试和游标资源规则，主要由 [`conn_stmt_test.rs`](conn_stmt_test.rs) 及 `pkg/server/tests/{commontest,cursor}` 使用。仓库内生产 `dispatch` 并不直接调用这些自由处理器。

文件没有条件编译项；独立 Rust 测试通过 [`lib.rs`](lib.rs) 中 `#[path = "conn_stmt_test.rs"] mod conn_stmt_test` 接入，符合源文件与测试文件分离约束。

## 核心职责

1. 定义预处理语句的连接内状态：SQL、参数数目与类型、long-data、最后一次参数、游标和最大包限制，核心载体是 `PreparedStatement`。
2. 解码 `COM_STMT_EXECUTE` 二进制参数：校验包头和游标标志，处理 null bitmap、`new-params-bound-flag`、类型复用、定长/变长值与 long-data 优先级，入口为 `ParseExecuteParams`，内部由 `parse_params` 完成。
3. 管理 `COM_STMT_SEND_LONG_DATA` 的延迟错误和内存记账：`LongDataState::append/check/release` 把“接收阶段记录、执行阶段报错、重置阶段释放”组成一个生命周期。
4. 在精简运行时中表达执行、事务错误重试和 TiFlash 回退：`executePlanCacheStmt` 驱动 `StatementRuntime`，`executePreparedStmtAndWriteResult` 决定 OK、普通结果或游标结果。
5. 管理 eager/lazy 游标的安装、FETCH、耗尽和异常清理：`executeWithCursor`、`executeWithLazyCursor`、`writeExecuteResultWithCursor` 与 `PreparedStatement::reset` 共同保证结果集关闭。
6. 提供辅助协议语义：关闭/重置语句、切换 `CLIENT_MULTI_STATEMENTS`、预处理 SQL 的诊断渲染和计划缓存文本查询。

## 主要符号

### 常量与错误

- `CURSOR_TYPE_NO_CURSOR` / `CURSOR_TYPE_READ_ONLY`：本实现接受的 EXECUTE 游标标志分别为 `0` 和 `1`；其他值由 `ParseExecuteParams` 判为 `MalformedPacket`。
- `MYSQL_OPTION_MULTI_STATEMENTS_ON` / `OFF` 与 `CLIENT_MULTI_STATEMENTS`：供 `handleSetOption` 修改连接能力位。
- `Error`：模块统一错误枚举。`MalformedPacket` 表示协议截断或非法编码；`NetPacketTooLarge` 和 `MemoryQuotaExceeded(connection_id)` 对接服务端标准错误文本；`StatementNotFound`、`WrongArguments` 表示语句/命令状态错误；`Runtime` 承载下层错误。
- `PreparedStmtCount: AtomicI64`：精简连接模型的语句计数；`HandleStmtPrepare` 首次插入递增，`handleStmtClose` 实际移除时递减。

### 数据类型与 trait

- `BinaryParam { tp, unsigned, is_null, value }`：EXECUTE 解码后的参数，不在本文件中做 SQL 类型转换，只保留 MySQL 类型码、无符号位、NULL 标志和原始字节。
- `ColumnInfo { name, column_type }`：精简运行时/事件层使用的结果列元信息。
- `ResultSet: Send`：游标/普通结果的最小抽象，提供 `columns`、逐行 `next`、`close`、`exhausted`；`supports_lazy_cursor` 默认返回 `false`。
- `LongDataMemory: Send + Sync`：Session 级内存边界。`charge(正数)` 在接收前尝试记账，`charge(负数)` 释放；返回 `(accepted, connection_id)` 使配额拒绝可以延迟成带连接号错误。
- `LongDataState`：记录可选内存记账器、本语句已记账字节 `bytes` 与粘性的拒绝原因 `rejected`。
- `PreparedStatement`：连接缓存的一条语句。`bound_params`/`bound_params_too_large`/`long_data` 管参数分片，`params_type` 支持后续 EXECUTE 省略类型，`last_params` 支持诊断字符串，`cursor` 是精简结果集，`protocol_cursor` 是生产 [`conn.rs`](conn.rs) 的 `QueryResult`。
- `ProtocolEvent::{Prepared, Result, Ok}`：精简处理器的输出，不等同于真实网络包；测试用它检查协议意图、游标状态位和行数据。
- `StatementRuntime: Send + Sync`：注入 prepare/execute/close、计划缓存、RU、事务错误决策和 TiFlash 开关；`long_data_memory` 与 `max_allowed_packet` 有默认实现。
- `clientConn`：精简连接视图，持有能力位、`HashMap<u32, PreparedStatement>`、事件输出和 `Arc<dyn StatementRuntime>`。
- `CursorRUV2Tracker`：精简 RU 记录 `{ statement_id, fetched_rows }`；当前 `writeExecuteResultWithCursor` 直接通过 `StatementRuntime::cursor_ru_delta` 上报实际 FETCH 行数。
- `MaterializedResultSet`：eager 游标的内存实现，通过 `offset` 和 `closed` 管理读取状态。

### 协议入口与内部辅助

- `HandleStmtPrepare`；`handleStmtExecute`；`handleStmtFetch`；`handleStmtClose`；`handleStmtSendLongData`；`handleStmtReset`；`handleSetOption`：精简连接模型的命令入口。
- `ParseExecuteParams`：共享给生产 `ClientConn::handleStmt` 的关键解码入口。
- `executePlanCacheStmt`、`executePreparedStmtAndWriteResult`、`executeWithCursor`、`executeWithLazyCursor`、`writeExecuteResultWithCursor`：精简执行与结果状态机。
- `preparedStmt2String`、`preparedStmt2StringNoArgs`、`preparedStmtID2CachePreparedStmt`：诊断和缓存查询辅助。
- `parse_params`、`read_length_encoded_int`、`read_u16`、`read_u32`、`render_param`、`check_long_data_size`：私有/`pub(crate)` 编解码与校验辅助。

## 执行流程

### PREPARE 与连接登记

`HandleStmtPrepare` 调用 `StatementRuntime::prepare(sql)` 取得 ID、参数数和列元信息，按参数数建立 `bound_params`，从 runtime 获取 Session 内存边界与当前 `max_allowed_packet`，将语句写入 `clientConn::statements`，最后追加 `ProtocolEvent::Prepared`。只有新 ID 才增加 `PreparedStmtCount`。

生产路径在 [`conn.rs`](conn.rs) 的 `ClientConn::handleStmt(Command::StmtPrepare, ...)` 完成同类工作：由 Session `prepare_statement` 取得元数据，创建本文件的 `PreparedStatement`，为 `LongDataState` 注入 `ConnectionLongDataMemory`，再写真实 prepare response 并 flush。

### EXECUTE 参数解码

`handleStmtExecute` 先要求至少 9 字节、读取 statement ID、更新该语句的包上限，然后调用 `ParseExecuteParams`：

1. 再次验证最小长度、ID 一致性，以及游标标志只能是 no-cursor/read-only。
2. 跳过 4 字节 iteration count，从参数数计算 null bitmap 长度。
3. `new-params-bound-flag == 1` 时读取每参数 2 字节类型并缓存到 `params_type`；否则复用上次类型。
4. 先通过 `LongDataState::check` 报告此前延迟的内存/包错误，再要求类型数组长度严格等于 `num_params * 2`，最后调用 `parse_params`。
5. 无论参数解析成功与否，都调用 `PreparedStatement::reset` 消费 long-data 并关闭旧游标；解析成功后才更新 `last_params` 并返回参数与游标请求。

`parse_params` 对每个参数按以下优先级解码：已有 `bound_params` 的 long-data；null bitmap；包内值。类型 `1/2/13/3/4/9/5/8` 使用固定长度，日期时间类型 `7/10/11/12` 用一字节长度，类型 `0/15/16/246..=255` 使用 MySQL length-encoded integer；未知类型返回 `Runtime("stmt unknown field type ...")`。所有切片和长度相加都经过边界检查。

生产 [`conn.rs`](conn.rs) 在 `Command::StmtExecute` 分支直接调用 `ParseExecuteParams`，随后交给 Session 的 `execute_prepared_streaming`，再通过真实二进制协议写结果或把 `QueryResult` 保存到 `protocol_cursor`。

### 执行、重试与结果写出

精简路径的 `executePlanCacheStmt` 首次调用 runtime `execute`，把结果交给 `executePreparedStmtAndWriteResult`。失败后先问 `retry_after_statement_error`；若可重试则再执行一次。否则若 `should_fallback_tiflash`，临时 `set_tiflash_enabled(false)` 重试，随后恢复为 `true`，并把首次错误追加为 statement warning。两种策略都由 runtime 决定，本文件不识别具体事务或 TiFlash 错误类型。

`executePreparedStmtAndWriteResult` 对无结果集追加 `Ok`；普通结果逐行收集后关闭并追加完整 `Result`；游标请求转入 `executeWithCursor`。普通读取出错时会先尝试 `close` 再传播原错误。

### 游标安装与 FETCH

`executeWithCursor` 先检查 `supports_lazy_cursor`：支持则由 `executeWithLazyCursor` 原样保存结果集；否则先读完并关闭源结果集，再包装成 `MaterializedResultSet`。两条路径都设置 `cursor_active`、清空绑定参数，并追加仅含列信息且 `cursor_exists=true` 的首个 `Result`。

`handleStmtFetch` 要求包长度恰为 8 字节，解析 ID 与 fetch size，并把客户端值限制到 1024；`writeExecuteResultWithCursor` 要求语句存在且游标活跃，最多读取指定行数，按实际行数调用 `cursor_ru_delta`。游标耗尽时先清除 active、取出并关闭游标，再输出 `last_row_sent=true`；读取/关闭/输出路径出错则调用 `reset` 清理整个语句游标状态。

### LONG_DATA、RESET、CLOSE 与 SET_OPTION

- `handleStmtSendLongData` 解析 4 字节 ID、2 字节参数下标，把剩余字节交给 `LongDataState::append`。非空分片在包上限检查后才尝试 Session 内存记账；被拒绝或已超限后，后续非空分片静默忽略，错误延迟到 EXECUTE。
- `handleStmtReset` 调用 `PreparedStatement::reset`，精简模型与 Go 行为一致地忽略 reset 错误并仍追加 `Ok`；生产 `conn.rs` 当前会把 reset 错误转换为连接错误，这是与 Go 精确语义的差异。
- `handleStmtClose` 对短于 4 字节的包静默返回；存在语句时 reset、通知 runtime 关闭、更新计数，不存在时幂等成功。
- `handleSetOption` 读取小端 `u16`，只接受 0/1 来设置或清除 `CLIENT_MULTI_STATEMENTS`，否则返回畸形包；精简层输出 `Ok`，生产层由 [`conn.rs`](conn.rs) 写 EOF 并 flush。

## 数据与状态

- 语句所有权：每个连接以 statement ID 为键持有 `PreparedStatement`；PREPARE 创建，CLOSE 移除。生产连接把同一类型放在互斥锁保护的 `prepared_statements` 中。
- 参数类型缓存：`params_type` 跨 EXECUTE 保留，这是 `new-params-bound-flag = 0` 能工作的必要状态；`reset` 不清除它。
- long-data 状态：`bound_params` 保存各槽字节，`bound_params_too_large` 保存包超限，`LongDataState::bytes` 只统计实际成功记账的字节，`rejected` 保存延迟配额错误。`release` 同时释放记账、清空槽位和两个错误状态。
- 空分片语义：向已有槽发送空 long-data 会先释放该槽此前记账的字节，再把槽设置为 `Some(Vec::new())`；即“显式空值”，不是 `None`。
- 错误优先级：`LongDataState::check` 先报告粘性的 `MemoryQuotaExceeded`，再由 `check_long_data_size` 报 `NetPacketTooLarge`；但单个新分片的 `append` 先检查包上限、后检查配额。
- 游标状态：`cursor_active` 是协议状态门；精简结果集存于 `cursor`，生产流式结果存于 `protocol_cursor`。`PreparedStatement::reset` 两者都关闭，并在生产 cursor 上调用 `response_lifecycle.finish()`。
- 输出状态：精简 `ProtocolEvent::Result` 用 `cursor_exists` 和 `last_row_sent` 表达 MySQL 状态；生产路径在 `QueryResult.state.status` 中设置/清除 `SERVER_STATUS_CURSOR_EXISTS` 与 `SERVER_STATUS_LAST_ROW_SENT`。
- 全局计数：`PreparedStmtCount` 使用 `AcqRel` 原子更新，避免精简连接并发创建/关闭时丢计数；生产路径未在此处更新该静态量。

## 依赖与调用关系

上游主链与直接调用边：

- [`lib.rs`](lib.rs) → `pub mod conn_stmt`，把模块纳入 `astersql-server`。
- [`conn.rs`](conn.rs) `ClientConn::dispatch` → `ClientConn::handleStmt`，分发全部 `COM_STMT_*`；EXECUTE 分支 → `conn_stmt::ParseExecuteParams`。
- [`conn.rs`](conn.rs) 的 PREPARE/SEND_LONG_DATA/RESET/CLOSE 分支分别构造 `PreparedStatement`、调用 `LongDataState::append`、`PreparedStatement::reset`、`LongDataState::release`。
- [`conn_stmt_test.rs`](conn_stmt_test.rs)、`pkg/server/tests/commontest/tidb_part4_aster_unit_test.rs`、`pkg/server/tests/cursor/cursor_test.rs` 直接调用精简处理器和类型。

本文件内部主要调用链：

`handleStmtExecute` → `ParseExecuteParams` → `LongDataState::check` / `parse_params` / `PreparedStatement::reset` → `executePlanCacheStmt` → `StatementRuntime::execute` → `executePreparedStmtAndWriteResult` → 普通结果循环或 `executeWithCursor` → `executeWithLazyCursor`/`MaterializedResultSet`。FETCH 则为 `handleStmtFetch` → `writeExecuteResultWithCursor` → `ResultSet::{next,exhausted,close}` / `StatementRuntime::cursor_ru_delta`。

从 [`Cargo.toml`](Cargo.toml) 可核对到的直接跨 crate 依赖只有错误与默认配置：`astersql-server-err` 提供 `ErrNetPacketTooLarge`，`astersql-util-dbterror-exeerrors` 提供查询内存超限错误，`astersql-sessionctx-vardef` 提供 `DefMaxAllowedPacket`。`crate::conn::QueryResult` 是同 crate 生产游标桥接。标准库依赖为 `HashMap`、`Arc`、原子计数与格式化。

RustCodeGraph 对目标文件报告 88 个符号并显示被 `conn.rs`、`conn_stmt_test.rs`、`conn_test.rs`、`driver_tidb.rs` 等 17 个文件引用；精确 callees 查询确认了上述内部边。由于同名 Go/Rust 符号并存，图的未限定 callers 结果为空或混入同名节点，因此生产上游边另由 [`conn.rs`](conn.rs) 源码直接核验。

## 错误处理与边界

- 所有整数读取与切片均用 `get`/`checked_add`/`try_into`，截断包统一映射为 `MalformedPacket`；FETCH 比其他命令更严格，必须恰好 8 字节。
- EXECUTE 的 statement ID 必须同时存在于连接表并与传入 `PreparedStatement.id` 一致；否则返回 `StatementNotFound`。
- 游标仅接受无游标和只读两种精确标志。Go 版本按位拒绝 for-update/scrollable、允许 read-only 位；Rust 对组合值更严格。
- 当参数数大于零而未曾缓存完整类型数组时，即使包声明复用旧类型也会返回 `MalformedPacket`。
- length-encoded `0xfb` 表示 NULL，`0xfc/0xfd/0xfe` 分别读取 2/3/8 字节；`0xff` 非法，8 字节长度无法转成当前平台 `usize` 也判畸形。
- long-data 超包与配额错误故意在 SEND 阶段延迟，避免立即打断协议；EXECUTE 即使解码失败也执行 reset，防止旧游标、绑定字节或内存记账泄漏。
- 普通结果和 eager 游标在 `next` 失败时尽力关闭源结果集，返回原迭代错误；FETCH 已进入活动游标后任一错误都会 reset。相反，FETCH 在发现游标未激活前不会 reset，避免误清仍需保留的绑定参数。
- `PreparedStatement::reset` 在 long-data 释放错误时会提前返回，后续游标可能尚未关闭；调用端对 reset 错误的处理并不完全一致：CLOSE 传播，精简 RESET/EXECUTE 清理点忽略，生产 RESET 传播为 `ConnError::Session`。
- TiFlash 回退恢复开关不是 RAII guard：第二次 `runtime.execute` 返回普通 `Err` 时仍会走到恢复；若 runtime 实现发生 panic，则本函数无法保证恢复。trait 的实现应避免 panic。
- `render_param` 仅用于诊断：UTF-8 值加引号并转义反斜杠/单引号，非 UTF-8 输出无 `0x` 前缀的十六进制；它不是完整 SQL literal 编码器，也没有 Go 版本的日志脱敏能力。

## 并发与资源生命周期

- `StatementRuntime`、`LongDataMemory` 要求 `Send + Sync` 并由 `Arc` 共享；`ResultSet` 只要求 `Send`，且作为 `Box<dyn ResultSet>` 由单个 `PreparedStatement` 独占可变访问。
- 精简 `clientConn` 通过 `&mut self` 串行修改语句表和输出，不在内部加锁；生产 `ClientConn` 在 [`conn.rs`](conn.rs) 使用互斥锁保护 `prepared_statements`，命令分发仍以单连接顺序为主。
- long-data 的记账生命周期是 append 成功时增加，空分片替换时局部释放，EXECUTE/RESET/CLOSE 时整体释放。测试证明共享 Session 已有消耗不会被误释放。
- eager 游标先完全物化并关闭源，再由 `MaterializedResultSet` 持有行；内存占用与总结果大小成正比。lazy 游标直接持有源，资源跨命令存活至耗尽、reset 或 close。
- `protocol_cursor` reset 时先 `ResultSet::close`，再 `response_lifecycle.finish`；精简 `cursor` 随后关闭。任何一步返回错误都会停止余下清理，所以实现新资源时要考虑分阶段清理和错误覆盖。
- FETCH 耗尽时立即 take/close 精简游标并清 active；未耗尽时资源继续属于语句。`cursor_ru_delta` 在每次成功读取批次后以实际行数上报。
- `PreparedStmtCount` 是进程级原子量，但只覆盖使用精简 `HandleStmtPrepare/handleStmtClose` 的路径，不能据此推断生产服务器的全局预处理语句数。

## 与 Go 版本的对应关系

直接对照文件是 [`conn_stmt.go`](conn_stmt.go)，测试证据来自 [`conn_stmt_test.go`](conn_stmt_test.go) 和 [`conn_stmt_params_test.go`](conn_stmt_params_test.go)。主要对应关系如下：

- Rust `HandleStmtPrepare` 对应 Go `(*clientConn).HandleStmtPrepare`；Rust 事件化元信息，Go 直接编码参数/列定义、兼容 `ClientDeprecateEOF` 并 flush。
- Rust `ParseExecuteParams` 抽取了 Go `handleStmtExecute` 前半段的二进制参数解析和无条件 `stmt.Reset` 语义；生产 Rust `conn.rs` 再负责 Session 状态、执行和真实写包。
- Rust `executePlanCacheStmt` 保留 Go 的“事务管理器允许时重试”和“TiFlash timeout 时临时回退 TiKV、恢复并追加 warning”的控制结构，但具体判定下沉到 `StatementRuntime`，精简层没有 Go 的 parse duration、RetryInfo、execdetails 等会话细节。
- Rust eager/lazy 游标保留 Go 的两种策略和资源清理意图；精简 eager 实现只用内存 `Vec`，未复刻 Go `RowContainer` 的磁盘 spill、内存/磁盘 tracker 和 fetch notifier。生产 Rust 则通过 `QueryResult` 的流式 cursor 走 [`conn.rs`](conn.rs)。
- Rust FETCH 保留活动游标校验、错误 reset、耗尽关闭和 1024 上限；Go 还重置告警/统计、接入 TopSQL、process info 和完整写包计时。
- Rust long-data 对齐 Go 的最大包限制、延迟 EXECUTE 报错和 Session 内存配额记账；Rust 独立测试进一步固定空分片替换、精确配额边界、共享消耗和释放行为。
- Rust 诊断字符串是简化实现；Go 会通过 parser Normalize、redact 模式和 plan-cache 参数生成安全日志文本，并能区分缓存类型失效。
- Rust 精简 `handleSetOption` 只改本地能力位并产出 `Ok`；Go 同时把能力同步进 Session、写 EOF 并 flush。生产 Rust 的 `ClientConn::handleSetOption` 实现了真实连接侧行为。

因此，本文件不是对 Go 文件所有服务器副作用的一比一替换；它是“共享协议状态/解码 + 精简可测试模型”，生产网络、Session 和观测功能由 [`conn.rs`](conn.rs) 等模块补齐。

## 扩展指南

- 新增或修改 EXECUTE 参数类型时，应优先改 `parse_params`/`read_length_encoded_int`，保持 long-data > NULL bitmap > 包内值的优先级，并在独立 [`conn_stmt_test.rs`](conn_stmt_test.rs) 增加截断、无符号位、类型复用和错误后 reset 回归；若涉及类型语义，还需同步 [`conn_stmt_params.rs`](conn_stmt_params.rs) 及其独立测试，而不是把测试写入源文件。
- 修改游标标志或 FETCH 行为时，应同步检查 `ParseExecuteParams`、`handleStmtFetch`、`writeExecuteResultWithCursor` 与生产 [`conn.rs`](conn.rs) 的 `Command::StmtExecute`/`write_prepared_cursor_fetch`，并覆盖 eager、lazy、空结果、迭代错误、关闭错误、1024 上限和状态位。
- 增加 `PreparedStatement` 字段时，必须更新本文件 `HandleStmtPrepare`、生产 [`conn.rs`](conn.rs) 的结构体构造，以及所有测试 fixture（例如 `install_statement`）；资源字段必须纳入 `reset` 和 CLOSE 路径。
- 改 long-data 配额时，应保持“只释放自己成功记账的字节”和错误粘性，并同步生产 `ConnectionLongDataMemory` 适配；重点回归 [`conn_stmt_test.rs`](conn_stmt_test.rs) 的 `long_data_*` 测试与 Go `TestStmtSendLongData*`。
- 改重试/TiFlash 策略时，应实现或扩展 `StatementRuntime` 方法，并与 Go `executePlanCacheStmt` 的重试资格、warning 时序和引擎恢复语义逐项核对；不要在精简层硬编码某个 Session 实现。
- 改诊断输出时必须先决定它是否用于日志。若用于生产日志，应补齐 Go 的 Normalize/redaction 语义，不能直接扩展当前 `render_param` 后宣称安全。
- 性能风险集中在 eager 游标全量物化、参数/行的多次 clone 和诊断字符串分配；兼容风险集中在包长度、游标标志、类型复用、EOF/OK 差异和错误码文本。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`pkg/server/conn_stmt.rs` 已索引，共 88 个符号。已执行目标文件 `files`、分段 `node`，以及 `HandleStmtPrepare`、`handleStmtExecute`、`ParseExecuteParams`、`executePlanCacheStmt`、`executePreparedStmtAndWriteResult`、`executeWithCursor`、`writeExecuteResultWithCursor`、FETCH/CLOSE/LONG_DATA/RESET/SET_OPTION/诊断函数的 `query`、`callers`、`callees` 查询。
- 已读 Rust 生产路径：[`conn_stmt.rs`](conn_stmt.rs) 全部 890 行；[`conn.rs`](conn.rs) 的 `dispatch`、`handleStmt`、`write_prepared_cursor_fetch`、`handleSetOption`；[`lib.rs`](lib.rs) 模块装配。
- 已读 crate 声明：[`Cargo.toml`](Cargo.toml)，核对 crate 名、lib 路径、porting 元数据和本文件实际使用的依赖。
- 已读 Rust 测试：[`conn_stmt_test.rs`](conn_stmt_test.rs)，重点包括 prepare/long-data/reset/close、畸形包、解析失败清游标、eager/lazy 关闭、FETCH 边界、真实 TCP/Session 写响应、包上限和内存配额；另通过调用检索核对 `pkg/server/tests/commontest/tidb_part4_aster_unit_test.rs` 与 `pkg/server/tests/cursor/cursor_test.rs` 的直接覆盖。
- 已读 Go 对照：[`conn_stmt.go`](conn_stmt.go) 全文件，并检索 [`conn_stmt_test.go`](conn_stmt_test.go)、[`conn_stmt_params_test.go`](conn_stmt_params_test.go) 中的 cursor、long-data、execute 参数测试入口。
- 人工复核结论：本文件存在是为了集中预处理协议共享状态、二进制参数/long-data 规则和可注入的状态机；生产运行由 `conn.rs` 复用核心类型与解码器并补齐真实 Session/网络副作用；安全扩展必须同时检查共享状态、生产接线和独立测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时使用任务指定命令验证恰有 11 个固定二级章节，并用 `git diff --check` 检查文档补丁格式。
