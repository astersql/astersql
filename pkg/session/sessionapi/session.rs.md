# `pkg/session/sessionapi/session.rs`

源码：[session.rs](./session.rs)。

## 文件定位

本文件属于 `astersql-session-sessionapi` crate，是 Rust 侧客户端连接级会话 ABI 的核心定义。crate 入口 `pkg/session/sessionapi/lib.rs` 将本文件声明为私有 `session` 模块，再以 `pub use session::*` 对外导出，因此使用方看到的是 crate 根上的 `Session`、`ErrIdentityNotFound` 和 `Any`。

它位于协议/服务器、SQL 解析与执行、事务、鉴权和会话管理之间，但只定义接口与两个辅助类型，不保存具体会话状态，也不实现 SQL 执行。`pkg/session/runtime.rs:16-24` 进一步明确：当前可执行运行时与完整 planner/executor 版 `sessionapi::Session` 是分开的；不能仅凭本文件推断完整 trait 已被某个生产类型实现或接入服务器主链。

crate 边界由 `pkg/session/sessionapi/Cargo.toml` 给出：包名为 `astersql-session-sessionapi`，库入口为 `lib.rs`，并通过路径依赖连接 AST、用户身份、鉴权连接、扩展、结果字段、会话管理、会话上下文、状态迁移、结果集和事务信息等 crate；TLS 状态直接使用外部依赖 `rustls = "0.23"`。

## 核心职责

1. 以 `Session: sessionctx::Context` 统一描述一条客户端连接生命周期内需要暴露的能力：SQL 解析/执行、事务提交/回滚、预编译语句、鉴权、连接属性、进程信息、状态迁移和资源关闭（`session.rs:47-183`）。
2. 用三个关联类型把接口与正式表达式类型、鉴权连接实现、会话状态处理器绑定，同时避免本 crate 直接依赖具体会话实现（`session.rs:48-54`）。
3. 提供稳定的“身份未匹配”错误值 `ErrIdentityNotFound`，保持与 Go `errors.New("identity not found")` 的可观察文案一致（`session.rs:27-41`；`session.go:36-37`）。
4. 提供 `Any = Box<dyn std::any::Any + Send + Sync>`，模拟 Go `...any` 形式的内部 SQL 参数，并要求被装箱值可以安全跨线程边界传递和共享（`session.rs:185-186`）。

本文件是契约层而不是调度器：所有 trait 方法都无默认方法体，实际解析、执行、事务和鉴权算法必须由实现者及其下游组件提供。

## 主要符号

- `IdentityNotFound`：私有零字段错误类型。`Display` 固定写出 `identity not found`，并实现 `std::error::Error`；它只用于构造公开错误值（`session.rs:27-37`）。
- `ErrIdentityNotFound: LazyLock<sessionctx::GoError>`：公开、惰性初始化的全局错误。首次解引用时把 `IdentityNotFound` 装箱为兼容 Go 错误边界的 `GoError`（`session.rs:39-41`）。
- `Session`：公开 trait，并继承 `sessionctx::Context`。实现者同时必须满足上下文契约（`session.rs:47`）。
- `Session::PreparedExpression`：传给 `ExecutePreparedStmt` 的参数表达式类型；对应 Go 的 `expression.Expression`（`session.rs:48-50`；`session.go:62`）。
- `Session::AuthConnection`：必须实现 `conn::AuthConn<Context = sessionctx::ExecutionContext>`，把鉴权回调使用的上下文类型固定为本 crate 的执行上下文（`session.rs:51-52`）。
- `Session::StateHandler`：必须实现 `SessionStatesHandler`，且其 `SessionContext` 和 `SessionStates` 与当前实现者及其父 trait 的状态类型一致（`session.rs:53-54`）。
- 结果读取方法：`Status`、`LastInsertID`、`LastMessage`、`AffectedRows`、`String`、`TxnInfo`，暴露当前或最近一次命令的连接状态（`session.rs:56-63, 89-90, 168-169`）。
- SQL 路径：`Parse` 将文本转换为 AST 列表；`Execute` 接收文本且可能返回多个结果集；`ExecuteStmt` 执行单个 AST；`ExecuteInternal` 接收类型擦除参数并返回单个结果集（`session.rs:64-88`）。
- 事务路径：`PrepareTxnCtx` 在语句执行前准备事务上下文，`CommitTxn` 返回提交错误，`RollbackTxn` 不返回错误（`session.rs:91-94, 170-175`）。
- 预编译语句路径：`PrepareStmt` 返回语句 ID、参数数和字段元数据；`ExecutePreparedStmt` 使用关联表达式类型执行；`DropPreparedStmt` 释放注册项（`session.rs:95-108`）。
- 连接与观察状态设置：`SetClientCapability`、`SetConnectionID`、`SetCommandValue`、压缩参数、`SetProcessInfo`、`SetTLSState`、`SetCollation`、`SetPort`（`session.rs:115-136, 179-180`）。
- 外部协作对象设置：`SetSessionStatesHandler`、`SetSessionManager`、`SetExtensions`（`session.rs:109-114, 137-138, 181-182`）。
- 鉴权路径：`Auth`、`AuthWithoutVerification`、`AuthPluginForUser`、`MatchIdentity` 分别承担完整认证、仅身份检查、插件选择和用户名/远端地址匹配（`session.rs:141-167`）。
- `Close` 和 `FieldList`：分别定义显式资源结束点和 MySQL `COM_FIELD_LIST` 元数据查询边界（`session.rs:139-140, 176-178`）。
- `Any`：内部 SQL 参数的类型擦除容器，保留运行时 downcast 能力（`session.rs:185-186`）。

## 执行流程

本文件没有可执行编排代码；以下是由方法签名规定、由调用方与实现者共同完成的契约流程。

典型文本 SQL 路径是：调用方提供 `ExecutionContext` 和 SQL；实现者可经 `Parse` 得到 `Vec<Box<dyn ast::StmtNode>>`，再逐条调用 `ExecuteStmt`；兼容入口 `Execute` 可直接完成文本到一个或多个 `RecordSet` 的过程。Go 对照明确把 `Execute` 和 `Parse` 标为 deprecated，并建议分别转向 `ExecuteStmt` 与 `ParseWithParams`（`session.go:46-53`）；Rust 接口当前保留这两个方法，但没有移植 `ParseWithParams`，因此扩展时不能把废弃入口当作首选新路径。

内部 SQL 路径由 `ExecuteInternal(ctx, sql, args)` 表达。`args` 是 `&[Any]`，返回值只有一个 `RecordSet`；Go 注释要求该辅助入口不允许多语句（`session.go:52-53`），但 Rust trait 本身没有默认校验逻辑，限制必须由实现者执行。

预编译路径先通过 `PrepareStmt` 注册 SQL，并取得 `stmt_id`、参数数量与结果字段；之后用同一 ID 和 `PreparedExpression` 切片调用 `ExecutePreparedStmt`；生命周期结束时调用 `DropPreparedStmt`。Go 侧把执行入口标为“仅测试保留、待删除”（`session.go:57-63`），Rust 目前仍保留完整签名。

事务路径在语句执行前调用 `PrepareTxnCtx` 建立或更新事务上下文，然后由执行路径读写；正常结束调用 `CommitTxn` 并处理其错误，放弃路径调用 `RollbackTxn`。本文件只固定顺序所需的能力，不定义懒开启、两阶段提交或回滚细节。

鉴权路径可先用 `MatchIdentity` 根据用户名和远端地址解析实际身份，再由 `AuthPluginForUser` 选择插件，最后由 `Auth` 使用认证数据、salt 和 `AuthConnection` 完成验证；需要跳过密码验证的受控路径可调用 `AuthWithoutVerification`。匹配不到身份时实现者应使用 `ErrIdentityNotFound` 表达与 Go 一致的错误类别。

连接建立或协议协商期间，调用方通过各 `Set*` 方法注入 capability、连接 ID、命令、压缩、TLS、排序规则、端口、会话管理器与扩展；执行期间 `SetProcessInfo` 更新可观察状态；连接终止时调用 `Close`。具体清理次序与幂等性未由 trait 声明。

## 数据与状态

`Session` 自身没有字段，状态归具体实现所有。接口可观察或修改的状态可分为：

- 最近语句状态：插入 ID、消息、影响行数和状态位（`LastInsertID`、`LastMessage`、`AffectedRows`、`Status`）。
- SQL/事务状态：解析出的 AST、结果集、预编译语句注册表、当前 `TxnInfo` 和由 `PrepareTxnCtx` 管理的事务上下文。
- 连接状态：客户端 capability、连接 ID、当前命令、压缩算法/级别、TLS 状态、collation、端口和进程信息。
- 安全状态：匹配后的 `UserIdentity`、认证插件、认证数据和 salt。
- 扩展与迁移状态：`StateHandler`、`sessmgr::Manager` 和 `SessionExtensions` 的共享引用。

所有结果集和 AST 都以 trait object 装箱，允许实现多态；`SetSessionManager` 与 `SetExtensions` 使用 `Option<Arc<...>>` 表达 Go 可空指针和共享所有权。`TxnInfo` 返回借用 `Option<&TxnInfo>`，其有效期不能超过会话借用。`SetTLSState` 只接收 `Option<&rustls::ServerConnection>`；实现者若需长期保存信息，必须提取或复制所需状态，不能保存短生命周期借用。

`Any` 的 `Send + Sync` 约束比裸 `dyn Any` 更强，但它仍不提供序列化、克隆或静态类型校验；实现者应按约定 downcast，并对类型不匹配返回可诊断错误。

## 依赖与调用关系

`pkg/session/sessionapi/lib.rs:13-51` 将依赖类型包装在本 crate 的稳定模块名下，本文件通过 `crate::{ast, auth, conn, extension, resolve, sessionctx, sessionstates, sessmgr, sqlexec, txninfo}` 使用这些 re-export。对应 Cargo 依赖分别指向解析器 AST/鉴权、权限连接、扩展、planner resolve、会话管理、sessionctx、sessionstates、sqlexec 和 txninfo；`sqlexec` 启用 `formal-crate` feature（`pkg/session/sessionapi/Cargo.toml`）。

关键下游依赖为：

- SQL 输入依赖 `ast::StmtNode`，输出依赖 `sqlexec::RecordSet`，字段元数据依赖 `resolve::ResultField`。
- 运行上下文、错误和状态迁移依赖 `sessionctx::{Context, ExecutionContext, GoError, SessionStatesHandler}`。
- 鉴权依赖 `auth::UserIdentity` 与 `conn::AuthConn`。
- 连接协作对象依赖 `sessmgr::Manager`、`extension::SessionExtensions` 和 `rustls::ServerConnection`。
- 事务观察依赖 `txninfo::TxnInfo`。

RustCodeGraph 将目标文件识别为被 44 个索引文件使用，并能精确定位 `Session` trait（节点 `trait:35acc4ea10cafbdd991a2a82d38c1b7c`）、`ErrIdentityNotFound` 和 `Any`。但图的通用名调用查询混入了同名 `Session`，不能作为具体实现证据。补充的精确文本搜索只确认若干 Cargo 反向依赖（包括 `pkg/session`、`pkg/server`、`pkg/executor`、`pkg/ddl`、`pkg/testkit` 等）以及 `pkg/session/upgrade_def.rs` 中的接口类型引用，未找到 `impl ... sessionapi::Session for ...`。因此当前可确认的是“公共 ABI 被依赖和引用”，而不是“完整 trait 已有生产实现”。

## 错误处理与边界

所有可能失败的解析、执行、提交、预编译、排序规则设置、鉴权、身份匹配、事务准备和字段查询都返回 `sessionctx::GoError`；这保持 Go `error` 的统一边界。`RollbackTxn`、状态 setter 和 `Close` 不返回错误，意味着实现者只能内部处理或记录失败，调用方不能从签名获得失败信息。

`ErrIdentityNotFound` 的可比较身份语义没有在 Rust 类型中声明；已验证且稳定的是其 `Display` 文案。调用方若需分类，优先使用项目既有错误约定，不应仅依赖字符串比较，也不应假设每次构造都会产生同一裸指针。

空值边界包括 `SetTLSState(None)`、`SetSessionManager(None)`、`SetExtensions(None)` 和 `TxnInfo() == None`。`session_test.rs` 通过函数指针类型断言专门验证 `SetExtensions` 保留 Go `*SessionExtensions` 的 nullable 契约。

Go 侧对 `ExecuteInternal` 的“仅单语句”限制、`Execute`/`Parse` 的废弃状态及 `ExecutePreparedStmt` 的测试专用状态只存在于 Go 注释，Rust 签名没有静态强制。实现者和新调用方必须主动遵守这些兼容边界。

## 并发与资源生命周期

trait 没有声明 `Send` 或 `Sync`，绝大多数变更操作要求 `&mut self`，因此本文件不承诺同一会话可被多线程并发调用。若上层需要跨任务共享，必须由具体实现或外部同步容器提供串行化，不能从 `Arc` 参数反推整个 `Session` 是线程安全的。

`Arc<dyn sessmgr::Manager>` 和 `Arc<SessionExtensions>` 表示会话可与其他所有者共享管理器或扩展对象；`Option` 允许解除绑定。`Any` 中的值必须 `Send + Sync`，但参数切片只在调用期间借用。

预编译语句的资源区间从 `PrepareStmt` 成功开始，到 `DropPreparedStmt` 或 `Close` 结束；事务资源从 `PrepareTxnCtx`/执行路径建立，到 `CommitTxn` 或 `RollbackTxn` 结束；连接级资源最终由 `Close` 回收。trait 没有 `Drop`、异步方法、任务句柄、锁或通道，也没有规定 `Close` 是否幂等，相关保证必须查看具体实现；当前仓库搜索未定位到完整 trait 的实现，因此这些细节属于未验证项。

`LazyLock` 保证 `ErrIdentityNotFound` 的初始化是线程安全且只发生一次。除此以外，本文件不创建后台任务、不持锁，也不拥有网络连接。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/session/sessionapi/session.go`。Rust 保留了 Go `Session` 的方法集合、参数语义和返回形态，并用 Rust 所有权/trait 机制表达 Go 接口：

- Go 的接口嵌入 `sessionctx.Context` 对应 Rust 超 trait `Session: sessionctx::Context`。
- Go 的 `context.Context` 对应借用的 `sessionctx::ExecutionContext`；Go `error` 对应 `sessionctx::GoError`。
- Go `[]sqlexec.RecordSet`、`ast.StmtNode` 等接口值对应 Rust `Vec<Box<dyn RecordSet>>`、`Box<dyn StmtNode>`。
- Go `[]expression.Expression` 被抽象为关联类型 `PreparedExpression` 的切片，避免 sessionapi crate 直接绑定表达式 crate。
- Go `conn.AuthConn` 和 `sessionctx.SessionStatesHandler` 被抽象为有关联约束的 `AuthConnection`、`StateHandler`。
- Go `*tls.ConnectionState` 在 Rust 中近似为 `Option<&rustls::ServerConnection>`；两者库与具体状态类型不同，只能确认“可空 TLS 连接状态”意图对齐，字段级等价未验证。
- Go `sessmgr.Manager` 接口值在 Rust 中为 `Option<Arc<dyn Manager>>`；Go `*SessionExtensions` 对应 `Option<Arc<SessionExtensions>>`。
- Go `...any` 对应 `&[Any]`，其中每项是带 `Send + Sync` 的装箱类型擦除值。
- Go `*TxnInfo`、`*UserIdentity` 和字段指针切片在 Rust 中分别用 `Option<&TxnInfo>`、按值 `UserIdentity` 和值向量表达；这改变了所有权表示，但保留了“可能无事务/返回解析身份/返回字段集合”的接口意图。

差异与迁移缺口：Go 注释中的废弃标记没有完整复制到 Rust API 文档；Rust 没有 Go 已推荐的 `ParseWithParams`；仓库内尚未搜索到完整 trait 的 Rust 实现。`pkg/session/sessionapi/Cargo.toml` 的 `package.metadata.porting` 将 Go 包映射到 `pkg/session/sessionapi` 并记录 `legacy-tasks = ["task-712"]`，说明这是按 Go 包迁移的 crate 边界。

## 扩展指南

新增会话能力时，应先判断它是否真是所有客户端连接都必须具备的稳定 ABI。若是，需同步修改 `Session`、Go 同路径接口以及实现类型；若只服务具体运行时，应优先放在运行时的窄接口中，避免继续扩大尚未完整接线的全量 ABI。

修改 SQL 执行签名时，重点检查 `Execute`、`ExecuteStmt`、`Parse`、`ExecuteInternal` 的单/多语句和单/多结果集约束，并同步独立测试。若引入带参数解析，应与 Go 的 `ParseWithParams` 语义核对，而不是在旧 `Parse` 上悄然改变行为。

修改关联类型时，要同时维护约束：`AuthConnection::Context` 必须仍为 `ExecutionContext`，`StateHandler::SessionContext` 必须仍为实现者自身，`StateHandler::SessionStates` 必须与父 `Context` 的状态类型一致。破坏这些等式会使实现无法组合。

修改可空对象时，应保持 `Option` 与 Go nil 语义，并在独立的 `pkg/session/sessionapi/session_test.rs` 增加编译期签名断言。错误或 `Any` 行为应在 `migration_aster_unit_test.rs` 扩展，不要把测试嵌入生产文件。新增行为测试还应覆盖失败路径、资源释放和与 Go 的相同边界。

性能风险主要来自在热路径增加装箱、克隆或锁：AST/结果集已使用动态分发，`Any` 还需要 downcast；扩展时避免无依据地复制大型结果、把 `Arc` 深拷贝成数据副本，或在每条语句上引入全局锁。兼容风险集中在方法签名、错误文案、nullable 语义和 Go 已废弃接口；修改前应搜索所有 Cargo 反向依赖和具体实现。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件完整收录为 186 行。
- RustCodeGraph 源码与符号：`pkg/session/sessionapi/session.rs`；`Session` 节点 `trait:35acc4ea10cafbdd991a2a82d38c1b7c`，`ErrIdentityNotFound` 节点 `constant:28430b65b72f68c7355d41116a06c647`，`Any` 节点 `type_alias:c7bb9e1c6b77e1d95c4d43a25ddf47bc`。
- crate 装配与依赖：`pkg/session/sessionapi/lib.rs`、`pkg/session/sessionapi/Cargo.toml`。
- Go 语义基线：`pkg/session/sessionapi/session.go`。
- 独立 Rust 测试：`pkg/session/sessionapi/session_test.rs` 验证 `SetExtensions` 可空签名；`pkg/session/sessionapi/migration_aster_unit_test.rs` 验证错误文案和 `Any` downcast。
- 实现状态旁证：`pkg/session/runtime.rs:16-24` 明确区分窄可执行运行时与完整 `sessionapi::Session` ABI；`rg` 对 Rust 实现与引用的补充搜索未发现完整 trait 实现，因此实现细节标记为未验证。
- 本任务是纯文档分析，按计划不运行 Cargo；验收采用固定章节结构检查和人工事实复核。
