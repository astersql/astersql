# `pkg/server/extension.rs`

## 文件定位

`pkg/server/extension.rs` 是 `astersql-server` crate 中的服务端扩展事件适配层，由 [`pkg/server/lib.rs`](./lib.rs) 的 `pub mod extension` 对外公开。它把连接生命周期和语句执行结果整理成监听器可消费的快照，目标场景是审计、观测等无需介入协议执行结果的扩展回调。

当前 Rust 仓库内没有生产模块调用本文件的四个派发入口；`rg` 只找到 [`pkg/server/extension_test.rs`](./extension_test.rs) 和 [`pkg/server/tests/commontest/tidb_part3_aster_unit_test.rs`](./tests/commontest/tidb_part3_aster_unit_test.rs) 的 Rust 调用。因此，本文件目前是公开且可测试的移植实现，但尚不能据此声称已经接入 Rust 连接状态机。相对地，Go 对照实现 [`pkg/server/extension.go`](./extension.go) 已被 `server.go`、`conn.go` 和 `conn_stmt.go` 的生产路径调用。

## 核心职责

- `onExtensionConnEvent` 从传输连接和可选会话变量中组装 `ConnEventInfo`，并向 `ExtensionListeners` 派发建连、握手或断连事件。
- `onExtensionStmtEnd` 把语句成功/失败、语句节点、语句上下文和预处理参数固化为 `stmtEventInfo`；`onExtensionBinaryExecuteEnd` 是其二进制执行适配入口。
- `onExtensionSQLParseFailed` 为尚无 AST/有效语句上下文的解析失败构造错误事件。
- `stmtEventInfo` 提供 SQL 原文、digest、用户、角色、数据库、影响行数、相关表和错误等查询接口，并实现预处理语句及上下文缺失时的回退规则。

这些职责均直接来自 `extension.rs` 的对应符号；文件不负责注册监听器、执行 SQL、修改协议响应或调度异步任务。

## 主要符号

- `Error(String)`：扩展事件使用的轻量错误包装，实现 `Display`，不保留错误链或类型信息。
- `ConnectionInfo`、`UserIdentity`、`TableEntry`：连接、用户和表的简化值对象。
- `Datum`：预处理参数快照，支持空值、整数、浮点和字节载荷。
- `Statement`：扩展层可见的语句枚举。`Execute { id }` 表示二进制执行，`Use { database }` 表示切库，`Sql { text, related_tables }` 表示普通 SQL。私有方法 `Statement::text` 负责生成展示文本并对 `USE` 库名中的反引号做双写转义。
- `StatementContext`：语句原文、归一化 SQL、digest、影响行和规划阶段相关表的快照。
- `PreparedMeta`、`SessionVars`：按预处理 ID 保存语句及 digest，并承载连接信息、会话别名、身份、角色、当前库、参数和语句上下文。
- `ConnEventTp`、`StmtEventTp`：本地事件类型。前者有 `Connected`、`Disconnected`、`HandshakeAccepted`、`HandshakeRejected`；后者区分成功和错误。
- `ExtensionListeners: Send + Sync`：监听器边界。`has_stmt_event_listeners` 用于快速跳过语句载荷构造，两个回调分别接收连接和语句事件。
- `ClientConn`：派发函数读取的最小连接视图，持有传输层连接信息、可选会话快照和可选的 `Arc<dyn ExtensionListeners>`。
- `stmtEventInfo`：语句事件载荷。字段保持私有，只通过 `ConnectionInfo`、`SessionAlias`、`StmtNode`、`ExecuteStmtNode`、`ExecutePreparedStmt`、`PreparedParams`、`OriginalText`、`SQLDigest`、`User`、`ActiveRoles`、`CurrentDB`、`AffectedRows`、`RelatedTables` 和 `GetError` 查询。
- `binaryExecuteStmtText`：把 ID 格式化为 `BINARY EXECUTE (ID n)`，作为无法还原预处理 SQL 时的稳定占位文本。

文件没有条件编译项、全局常量或全局可变状态。

## 执行流程

连接事件流程如下：调用方提供 `ClientConn`、事件类型和可选错误；`onExtensionConnEvent` 在未安装监听器时立即返回；若存在会话变量，则优先使用其中的 `connection_info`、`session_alias` 和 `active_roles`，否则连接信息回退到 `ClientConn.connection_info`，别名和角色回退为空；最后同步调用 `on_connection_event`。

普通语句结束流程如下：`onExtensionStmtEnd` 依次检查监听器是否存在、是否真的监听语句事件、会话变量是否存在。通过检查后，它从 `Statement::Execute` 提取执行 ID，非 `Execute` 使用 `0`；非空 `prepared_params` 覆盖克隆快照中的 `plan_cache_params`；`stmt_context_valid=false` 时使用全新的默认上下文，防止上一条语句的字段泄漏；错误是否存在决定 `StmtError` 或 `StmtSuccess`，随后同步派发 `stmtEventInfo`。

二进制执行由 `onExtensionBinaryExecuteEnd` 构造 `Statement::Execute { id }` 后完全复用上述流程。SQL 解析失败由 `onExtensionSQLParseFailed` 直接产生 `StmtError`：语句节点为空、执行 ID 为零、上下文为空，原 SQL 存入 `failed_parse_text`。

监听器查询语句信息时，`OriginalText` 优先级是有效上下文原文、预处理缓存中的语句文本、节点文本、二进制占位文本，最后才是解析失败文本。`SQLDigest` 优先返回上下文中的归一化文本/digest，其次返回预处理缓存值；只有无法获得这些信息时才返回二进制占位或原文并令 digest 为空。

## 数据与状态

所有事件数据都是拥有所有权的值或克隆快照。连接事件会克隆连接信息、别名和角色；语句事件会克隆整个 `SessionVars`，之后只在该副本上替换本次 `plan_cache_params`。所以监听器看到的是派发时的快照，不能借此修改真实会话。

`execute_stmt_id=0` 被用作“不是二进制预处理执行”的哨兵；预处理查找由 `ensureExecutePreparedCache` 对 `SessionVars.prepared` 做只读查询。`stmt_context_valid` 是防止复用陈旧 `StatementContext` 的关键不变量。成功语句的 `RelatedTables` 无条件采用 `StatementContext.tables`，即使为空也不回退到 `Statement::Sql.related_tables`；[`extension_test.rs`](./extension_test.rs) 的 `successful_statement_uses_stmt_context_tables_even_when_empty` 固化了该规则。

相关表还有两条分支：`USE` 始终返回一个“库名 + 空表名”的条目；错误语句从 `Statement::Sql.related_tables` 回退提取表，并用 `SessionVars.current_db` 补齐空库名。`AffectedRows` 在事件含错误时固定返回零，否则读取上下文值。

## 依赖与调用关系

本文件的直接 Rust 代码依赖只有标准库的 `HashMap`、`fmt` 和 `Arc`。[`pkg/server/Cargo.toml`](./Cargo.toml) 将 crate 定义为 `astersql-server`、入口为 `lib.rs`，并声明了 `astersql-extension`、parser、session、planner、types 等完整服务端依赖；但 `extension.rs` 当前没有使用这些外部 crate，而是定义了自己的简化事件类型和监听器 trait。

下游调用关系均为文件内同步调用：`Statement::text -> binaryExecuteStmtText`，`onExtensionBinaryExecuteEnd -> onExtensionStmtEnd`，`OriginalText`/`SQLDigest -> ensureStmtContextOriginalSQL`，后者再调用 `ensureExecutePreparedCache` 和 `Statement::text`。`RelatedTables` 只读取本地快照，不调用 planner。

上游方面，RustCodeGraph 能定位 `extension.rs::onExtensionStmtEnd`、`stmtEventInfo`、`SQLDigest` 和 `RelatedTables`，并报告该文件被测试及若干模块引用；但调用边命令未在本次查询中返回可用结果。仓库级 `rg` 对四个派发入口的复核只发现两个 Rust 测试文件，没有生产调用。Go 生产调用边则是 `server.go -> onExtensionConnEvent`、`conn.go -> onExtensionStmtEnd/onExtensionSQLParseFailed/onExtensionConnEvent`、`conn_stmt.go -> onExtensionBinaryExecuteEnd`。

## 错误处理与边界

派发入口不返回 `Result`，监听器回调也不返回错误；监听器失败无法由本层传播或隔离。没有监听器、没有语句监听器或没有会话快照时，语句事件静默跳过。连接事件允许没有会话快照，并回退到传输层连接信息。

`onExtensionSQLParseFailed` 虽然检查了会话是否存在，但 Go 对照实现直接解引用 `cc.getCtx()`；Rust 的提前返回更安全，也意味着无会话时不会产生解析失败事件。`Error` 只保留字符串。预处理 ID 查找失败不会报错，而是逐级回退到执行节点或 `BINARY EXECUTE` 占位文本。空的 `prepared_params` 不会清空快照内已有的 `plan_cache_params`，调用方若需要表达“本次确实没有参数”，必须注意这一语义。

当前 Rust 事件枚举还没有 Go `ConnReset` 对应项；本地 `ExtensionListeners` 也不是 [`pkg/extension/session.rs`](../extension/session.rs) 的 `SessionExtensions`/`StmtEventInfo` 抽象。二者尚未接线是功能边界，而非文档可以假定已完成的能力。

## 并发与资源生命周期

`ExtensionListeners` 要求 `Send + Sync`，并通过 `Arc<dyn ExtensionListeners>` 共享；派发函数借用 `ClientConn`，在调用栈内同步执行监听器，不创建线程、future、任务、通道或锁。具体监听器必须自行保证内部并发安全；测试监听器用 `Mutex<Vec<_>>` 记录事件正是这一约束的示例。

事件载荷拥有其字符串、向量、错误和会话快照，因此回调期间没有指向可变会话的借用。回调结束后局部载荷被释放；监听器若要长期保存信息，需要在回调中复制或移动出自己需要的数据。由于 trait 方法接收整个 `stmtEventInfo`，实现可以在回调期间拥有该快照，但无法访问其私有字段，只能使用公开查询方法。

## 与 Go 版本的对应关系

[`pkg/server/extension.go`](./extension.go) 是直接对照。Rust 保留了四个派发入口、`stmtEventInfo` 查询面、无效上下文清空、预处理 SQL/digest 回退、失败时影响行数为零、成功时优先使用规划相关表，以及二进制占位文本等核心意图。

差异必须显式看待：Go 使用真实 `clientConn`、session context、AST、`PlanCacheStmt`、`types.Datum` 和 `extension.SessionExtensions`，并已接入服务端生产流程；Rust 使用本文件自定义的简化模型，未实现 `pkg/extension/session.rs::StmtEventInfo`，也未连接生产 `conn.rs/server.rs`。Go 对 `PreparedStatement` 会再次解析二进制参数并构造 `ast.ExecuteStmt`，Rust 直接接收已经转换好的 `Vec<Datum>`。Go 失败路径通过 `resolve.NewNodeW`/`core.ExtractTableList` 遍历 AST，Rust 依赖调用方预先放入 `Statement::Sql.related_tables`。Go 连接事件包含 `ConnReset`，Rust 本地枚举缺失。Rust 的 `SQLDigest` 对预处理缓存有显式直接分支，而 Go 通过 `ensureExecutePreparedCache` 初始化 statement context 后读取 digest。

这些差异说明本文件是结构化移植，而不是 Go 完整运行时对象的等价替换；扩展时应以真实接线需求决定是继续维护适配模型，还是复用 `astersql-extension` 的公共 trait。

## 扩展指南

新增连接事件时，应同步修改 `ConnEventTp`、实际连接状态机调用点和连接事件独立测试；特别要先决定是否补齐 Go 的 `ConnReset`。新增语句元数据时，应把字段放在 `StatementContext`/`SessionVars`/`PreparedMeta` 中最符合生命周期的位置，并在 `stmtEventInfo` 增加只读查询方法，避免向监听器暴露可变会话。

修改原文或 digest 回退顺序时，重点检查 `ensureExecutePreparedCache`、`ensureStmtContextOriginalSQL`、`OriginalText` 和 `SQLDigest`，并在独立的 [`pkg/server/extension_test.rs`](./extension_test.rs) 增加成功、错误、解析失败、缓存命中/缺失和上下文失效用例。修改连接快照时同步更新 [`pkg/server/tests/commontest/tidb_part3_aster_unit_test.rs`](./tests/commontest/tidb_part3_aster_unit_test.rs)。Rust 单元测试不得内嵌到生产文件。

若要接入生产 Rust 服务端，最小必要工作不是仅调用这些函数：还需统一 `ClientConn` 与真实连接/会话类型、决定如何桥接 [`pkg/extension/session.rs`](../extension/session.rs) 的事件抽象、在握手/断连/解析/执行完成点接线，并证明事件恰好派发一次。兼容风险主要是事件时序、缺失上下文的回退及 Go/Rust API 类型差异；性能风险主要来自每条语句克隆整个 `SessionVars` 和相关向量。监听器仍应保持同步、短耗时，或由监听器自行把工作移交到有界异步设施。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query onExtensionStmtEnd` 同时定位 Go 第 63 行与 Rust 第 197 行；`query stmtEventInfo` 定位 Go 第 131 行与 Rust 第 294 行；`query SQLDigest`、`query RelatedTables` 定位 Rust 方法及 Go/公共扩展对照符号；`node --file pkg/server/extension.rs --offset 1 --limit 470` 读取了完整 437 行源码。`callers/callees` 本次未返回可用边，故调用关系又以 `rg` 复核，并明确保留该验证限制。
- 已读实现与装配：[`pkg/server/extension.rs`](./extension.rs)、[`pkg/server/lib.rs`](./lib.rs)、[`pkg/server/Cargo.toml`](./Cargo.toml)、[`pkg/extension/session.rs`](../extension/session.rs)。目标包没有 `pkg/server/doc.go`。
- 已读 Go 对照：[`pkg/server/extension.go`](./extension.go)、[`pkg/extension/session.go`](../extension/session.go)，并用 `rg` 核对 `pkg/server/server.go`、`pkg/server/conn.go`、`pkg/server/conn_stmt.go` 的生产调用点。
- 已读独立 Rust 测试：[`pkg/server/extension_test.rs`](./extension_test.rs) 验证预处理 digest 回退和成功语句相关表规则；[`pkg/server/tests/commontest/tidb_part3_aster_unit_test.rs`](./tests/commontest/tidb_part3_aster_unit_test.rs) 验证连接事件顺序、身份、别名、角色和拒绝错误。未找到 `pkg/server/extension_test.go`，仓库内也未找到直接针对 Go `stmtEventInfo` 的同路径测试。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前只执行固定十一章节结构检查，并人工复核“文件为何存在、如何运行、如何安全扩展”均有源码或对照文件依据。
