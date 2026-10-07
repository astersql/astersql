# `pkg/extension/session.rs`

## 文件定位

`session.rs` 是 `astersql-extension` crate 的会话级扩展边界：它把扩展清单（`Manifest`）里声明的连接事件处理器、语句事件处理器和认证插件整理成每个会话可持有的 `SessionExtensions`。crate 根 `pkg/extension/lib.rs` 通过 `pub mod session` 与 `pub use session::*` 对外暴露本文件的公开符号；`pkg/extension/extensions.rs` 的 `Extensions::NewSessionExtensions` 是构造入口，并转调本文件的 `newSessionExtensions`。

该文件不负责建立网络连接、执行 SQL、解析语句或验证认证插件。它只定义事件数据契约、收集回调并同步广播。Go 生产链的对应入口位于 `pkg/server/extension.go`、`pkg/server/conn.go` 等文件；在当前 Rust 源码中，检索到的直接使用主要是 `pkg/extension/event_listener_test.rs`、`pkg/extension/auth_1_aster_unit_test.rs` 和会话 API 的类型接线。`pkg/server/extension.rs` 目前定义了另一套服务端事件接口，未发现其调用本文件的分发函数，因此不能据此声称 Rust 服务端已经完成同等生产接线。

## 核心职责

1. 用 `ConnEventTp`、`StmtEventTp` 和 `ConnEventInfo` 描述扩展可观察的连接/语句事件。
2. 用 `StmtEventInfo` trait 定义语句监听器读取会话、AST、prepared statement、SQL digest、影响行数、相关表和错误的统一视图。
3. 用 `SessionHandler` 表示单个扩展为一个会话提供的可选连接/语句回调。
4. `newSessionExtensions` 按 `Extensions::Manifests()` 的顺序调用各 `sessionHandlerFactory`，收集非空回调；同时按 Go 语义生成会话认证插件表。
5. `SessionExtensions` 的方法按收集顺序同步扇出事件，并提供监听器存在性和按名查认证插件的查询。
6. 四个包级便捷函数接受 `Option<&SessionExtensions>`，把 Go 的 nil receiver 行为显式映射为 Rust 的 `None` 无操作或未命中。

文件自身不做回调隔离、错误聚合、异步调度、重试或 panic 捕获；这些都属于调用方或扩展实现的责任。

## 主要符号

- `ConnEventInfo`：连接事件载荷。`ConnectionInfo`、`Error` 是可空值，`SessionAlias` 默认为空字符串，`ActiveRoles` 默认为空列表；其手写 `Default` 明确产生这组空状态。字段保持 Go 风格命名以对齐移植接口。
- `ConnEventTp`：`#[repr(u8)]` 的连接事件枚举。判别值固定为 `ConnConnected = 0`、`ConnHandshakeAccepted = 1`、`ConnHandshakeRejected = 2`、`ConnReset = 3`、`ConnDisconnected = 4`。
- `StmtEventTp`：`#[repr(u8)]` 的语句结果枚举，`StmtError = 0`、`StmtSuccess = 1`。
- `StmtEventInfo`：只读事件视图 trait。`StmtNode`、`ExecuteStmtNode`、`ExecutePreparedStmt`、`ConnectionInfo`、`User` 和 `GetError` 可返回空；`ActiveRoles`、`PreparedParams`、`RelatedTables` 及 `SQLDigest` 的规范化文本按值返回，调用实现可能发生克隆或分配。
- `ConnectionEventFunc`：`Arc<dyn Fn(ConnEventTp, &ConnEventInfo) + Send + Sync>`。
- `StmtEventFunc`：`Arc<dyn Fn(StmtEventTp, &dyn StmtEventInfo) + Send + Sync>`。动态 trait 对象使调用方无需暴露具体事件结构。
- `SessionHandler`：包含 `OnConnectionEvent`、`OnStmtEvent` 两个可选回调，`Default` 表示两者都未注册。
- `newSessionExtensions(&Extensions) -> SessionExtensions`：内部聚合器。函数虽声明为 `pub`，但名称和调用位置表明通常由 `Extensions::NewSessionExtensions` 包装。
- `SessionExtensions`：私有保存 `connectionEventFuncs`、`stmtEventFuncs` 和 `authPlugins`；外部只能通过其方法分发或查询，不能直接改变已收集列表。
- `SessionExtensions::{OnConnectionEvent, HasStmtEventListeners, OnStmtEvent, GetAuthPlugin}`：非空会话句柄上的方法。
- 包级 `OnConnectionEvent`、`OnStmtEvent`、`HasStmtEventListeners`、`GetAuthPlugin`：对应方法的可空句柄适配层。

本文件没有模块级常量、条件编译项、异步函数或 `unsafe` 代码。

## 执行流程

会话扩展的构造流程如下：

1. 上游持有已经建立的 `Extensions`，调用 `Extensions::NewSessionExtensions`（`pkg/extension/extensions.rs`）。
2. `newSessionExtensions` 创建空的 `SessionExtensions`，再遍历 `extensions.Manifests()` 返回的 Manifest 列表。
3. 若某 Manifest 有 `sessionHandlerFactory`，立即同步调用工厂。工厂返回 `None` 时跳过；返回 `SessionHandler` 时，分别把其中存在的连接和语句回调追加到对应向量。
4. 若 Manifest 的 `authPlugins` 是 `Some`，先清空当前映射，再把该切片中的插件按 `Name` 插入。于是最后一个 `Some`（包括空切片）决定最终会话插件集合；同一切片内同名插件由后项覆盖。
5. 返回完成快照。后续 Manifest 的改变不会回写这个 `SessionExtensions`；回调和插件通过 `Arc` 共享其对象。

事件到来后，`SessionExtensions::OnConnectionEvent` 或 `OnStmtEvent` 依次遍历向量并直接调用每个闭包。顺序等于构造时 Manifest 遍历顺序，并分别忽略没有注册该类回调的 Manifest。包级函数仅先判断 `Option`：`Some` 转发，`None` 直接返回。

语句路径可先调用 `HasStmtEventListeners`，避免在没有监听器时构造昂贵的事件信息。`GetAuthPlugin` 以名称执行 `HashMap` 查找并克隆内部 `Arc`，返回 `(Option<Arc<AuthPlugin>>, bool)`；布尔值与 `Option::is_some()` 一致。

## 数据与状态

`SessionExtensions` 是构造期写入、运行期只读的会话快照：两个 `Vec` 保留回调顺序，`HashMap<String, Arc<AuthPlugin>>` 提供认证插件按名查询。它没有全局变量，也不会修改 `Extensions` 或 `Manifest`。

回调与插件使用 `Arc`，因此一个会话快照拥有共享对象的强引用；`GetAuthPlugin` 再增加一次插件强引用并把它交给调用方。事件载荷只在调用期间借用，监听器若需延长数据生命周期必须自行复制所需字段。`StmtEventInfo` 中按值返回的集合和 `Datum` 列表由具体实现决定如何生成，本文件既不缓存也不校验它们。

认证插件存在一个必须保留的 Go 兼容不变量：`newSessionExtensions` 不是把所有 Manifest 插件累加，而是在每个 `authPlugins: Some(_)` 处重建表。与之相对，`Extensions::GetAuthPlugins`（`pkg/extension/extensions.rs`）会跨 Manifest 累加并让同名后者覆盖；两者用途和语义不可互换。`pkg/extension/auth_1_aster_unit_test.rs` 的 `extension_auth_map_accumulates_but_session_map_uses_last_non_nil_manifest` 专门锁定了这一区别。

## 依赖与调用关系

上游关系：

- `pkg/extension/extensions.rs::Extensions::NewSessionExtensions` → `newSessionExtensions`，是 Rust 构造门面。
- `pkg/extension/manifest.rs::WithSessionHandlerFactory` 将 `SessionHandlerFactory` 存入 `Manifest::sessionHandlerFactory`；`WithCustomAuthPlugins` 写入 `Manifest::authPlugins`。
- `pkg/extension/event_listener_test.rs` 通过 `Extensions::from_manifests(...).NewSessionExtensions()` 构造快照并调用包级分发函数。
- `pkg/extension/auth_1_aster_unit_test.rs` 直接调用快照方法，验证 Registry 排序后的回调次序及认证表行为。
- `pkg/session/sessionapi/session.rs::SetExtensions` 接受 `Option<Arc<extension::SessionExtensions>>`，表明会话 API 预留了挂载点；当前静态搜索未证明完整 Rust 服务执行链调用本文件的事件广播。

下游类型依赖：

- `ast`、`parser::Digest` 来自 `pkg/extension/lib.rs` 对 parser 子 crate 的再导出。
- `auth_identity::{UserIdentity, RoleIdentity}`、`variable::ConnectionInfo`、`types::Datum`、`stmtctx::TableEntry` 分别来自 parser-auth、sessionctx-variable、types、sessionctx-stmtctx 依赖。
- `AuthPlugin` 来自同 crate 的 `auth` 模块；`ExtensionError` 来自 `util`。
- 标准库只使用 `Arc`、`HashMap` 和顺序容器 `Vec`。

`pkg/extension/Cargo.toml` 确认 crate 名为 `astersql-extension`，`lib.rs` 是库入口，且没有控制本文件的 feature。与本文件直接相关的 workspace 路径依赖包括 parser AST/auth/root、session context variable/stmtctx 和 types；测试侧另有 `serial_test`，但事件扇出测试本身用 `Mutex` 做确定性记录。

RustCodeGraph 对目标文件识别出 31 个符号，文件引用摘要显示它被多处测试/接口使用；然而对重名 Go/Rust 符号执行精确 `callers`/`callees` 未产生可用静态边，所以以上接线同时由模块源码和 `rg` 引用结果核验。

## 错误处理与边界

本文件的公开分发和查询 API 不返回 `Result`。`ConnEventInfo::Error` 与 `StmtEventInfo::GetError` 只是向监听器传递已发生错误的观察数据，不代表分发失败。

关键边界如下：

- 没有 `SessionExtensions` 时，包级连接/语句分发是 no-op，监听器查询为 `false`，插件查询为 `(None, false)`；这对应 Go 方法对 nil receiver 的处理。
- 工厂缺失、工厂返回 `None`、或 `SessionHandler` 某字段为 `None` 都会被安静跳过。
- 空监听器列表允许正常分发，循环执行零次。
- `authPlugins: None` 保留先前插件表；`Some(vec![])` 会清空表。这与 Go 的 nil slice / non-nil empty slice 差异相对应。
- 本文件不捕获回调 panic。某个回调 panic 时，本次同步分发会展开栈，后续监听器不会继续执行。
- 本文件不验证事件类型与载荷内容是否一致，例如 `StmtSuccess` 携带错误或 `StmtError` 未携带错误不会被拒绝；正确组合由事件生产者保证。
- `GetAuthPlugin` 的 `bool` 不提供额外状态，只重复表达返回插件是否存在。

## 并发与资源生命周期

两类回调都要求 `Send + Sync` 并保存在 `Arc` 中，`SessionExtensions` 的分发方法只借用 `&self`，因此其不可变快照可被安全共享；具体是否跨线程调用由上游决定。本文件没有锁、通道、任务、线程、事务或异步运行时，也不序列化并发回调。监听器若维护可变状态，必须像 `pkg/extension/event_listener_test.rs` 那样自行使用 `Mutex`、原子类型或其他同步手段。

构造期间，工厂和 Manifest 遍历均为串行同步执行。分发同样在调用线程串行执行；慢监听器会线性增加事件延迟，重入、阻塞及死锁风险由监听器实现承担。删除 `SessionExtensions` 时，向量和映射释放各自的 `Arc`；只有最后一个强引用释放时，对应闭包捕获状态或认证插件才会销毁。本文件没有显式关闭钩子，扩展级关闭生命周期由 Registry/Manifest 的 `WithClose` 路径管理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/extension/session.go`。主要映射为：Go 的指针/接口可空值在 Rust 中改为 `Option` 或借用，Go 函数值改为 `Arc<dyn Fn + Send + Sync>`，Go slice/map 改为 `Vec`/`HashMap`，Go 的 nil receiver 语义由包级 `Option<&SessionExtensions>` 适配函数承接。

行为保持点：

- 两组枚举的数值顺序与 Go `iota` 完全一致；`event_discriminants_match_go_iota_order` 有显式回归断言。
- 回调按 Manifest 顺序收集并按同序同步调用；`statement_events_fan_out_in_order_and_preserve_go_record_fields` 与 `connection_events_fan_out_in_order_and_none_is_no_op` 验证此约束。
- `authPlugins` 每遇到非 nil/`Some` 切片就重建，因此最终只保留最后一个此类 Manifest 的插件；Rust 测试明确验证该语义。
- `StmtEventInfo` 保留 Go 接口的字段集合和 prepared statement 特殊视图，Rust 以 `Option<&dyn ast::Node>` 等签名表达 nil。

需要注意的表达差异：Go 的 `newSessionExtensions` 返回指针，Rust 返回值类型；Go 方法可以直接在 nil receiver 上调用，Rust 必须经过包级适配函数。Rust 回调还额外施加 `Send + Sync`，便于跨线程共享。当前 Rust 服务端的 `pkg/server/extension.rs` 并非本文件的直接等价调用方，因此 Go 文件中已经存在的连接/语句生产链只能作为目标语义证据，不能当作 Rust 已完成接线的证据。

Go 测试 `pkg/extension/event_listener_test.go`、`pkg/server/conn_test.go` 和 `pkg/server/tests/commontest/tidb_test.go` 覆盖生产链事件；Rust 独立测试 `pkg/extension/event_listener_test.rs` 与 `pkg/extension/auth_1_aster_unit_test.rs` 覆盖本文件自身的聚合和分发契约。

## 扩展指南

- 新增连接或语句事件种类时，修改 `ConnEventTp`/`StmtEventTp`，保持显式且稳定的 `u8` 判别值；同步更新 Go 对照枚举以及 `pkg/extension/event_listener_test.rs::event_discriminants_match_go_iota_order`。若涉及生产接线，还需分别审查 Go `pkg/server/extension.go` 和 Rust `pkg/server/extension.rs` 的事件产生点。
- 新增语句观察字段时，扩展 `StmtEventInfo`，同步所有实现者，至少包括 `pkg/extension/event_listener_test.rs::TestStmtInfo`，并核对 Go `StmtEventInfo`。优先返回借用；若必须按值返回，要评估热路径克隆和分配成本。
- 新增一类回调时，需要同时扩展 `SessionHandler`、`SessionExtensions` 的存储、`newSessionExtensions` 的收集、实例方法、可空适配函数、crate 再导出及独立测试，不能只增加类型声明。
- 调整认证插件聚合前，必须确认是否有意偏离 Go 的“最后一个非 nil 切片重建”语义，并同步 `extension_auth_map_accumulates_but_session_map_uses_last_non_nil_manifest`；不要误用 `Extensions::GetAuthPlugins` 的累加语义替代它。
- 监听器运行在同步事件路径。新增工作应避免阻塞、无界分配和持锁调用外部代码；若引入异步转交，需要明确事件数据的所有权、顺序、背压和关闭行为。
- Rust 单元测试应继续保留在独立 `*_test.rs` 文件中，不应嵌入 `session.rs`。推荐扩展现有 `pkg/extension/event_listener_test.rs`；认证映射/Registry 顺序相关用例放在 `pkg/extension/auth_1_aster_unit_test.rs` 或更聚焦的独立测试文件。
- 若要宣称 Rust 服务端完成接线，应新增并验证真实调用方，而不是仅依赖扩展 crate 单测；需要覆盖连接各阶段、成功/失败语句、prepared statement、无监听器快速路径和监听器 panic/延迟策略。

兼容风险主要是枚举 ABI 数值、事件字段可空性、监听器顺序和认证映射覆盖规则；性能风险主要是每次事件的同步多播及 `StmtEventInfo` 按值返回集合/字符串带来的分配；正确性风险主要是监听器 panic 中断后续监听器，以及服务端事件类型与本模块尚未证实接线造成的两套契约漂移。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/extension/session.rs` 报告该文件 31 个符号；`node --file pkg/extension/session.rs --offset 1 --limit 500` 读取完整 203 行源码；`query` 核对 `newSessionExtensions`、`SessionExtensions`、`OnConnectionEvent`、`GetAuthPlugin` 的 Go/Rust 候选。
- RustCodeGraph 调用分析：对精确符号执行 `callers`/`callees` 未返回可用边；随后使用 `explore "pkg/extension/manifest.rs sessionHandlerFactory WithSessionHandlerFactory"` 获得工厂与相关测试的 blast radius，并用引用搜索补齐边界。这一限制意味着“未发现生产调用”是静态证据范围内的结论，不等于动态运行时绝无调用。
- 已读 Rust 源码/配置：`pkg/extension/session.rs`、`pkg/extension/lib.rs`、`pkg/extension/extensions.rs`、`pkg/extension/manifest.rs`、`pkg/extension/Cargo.toml`、`pkg/session/sessionapi/session.rs`。
- 已读 Rust 测试：`pkg/extension/event_listener_test.rs`、`pkg/extension/auth_1_aster_unit_test.rs`；另通过引用搜索定位 `pkg/session/sessionapi/session_test.rs` 等类型接线测试。
- 已读 Go 对照：`pkg/extension/session.go`；通过引用搜索核对 `pkg/server/extension.go`、`pkg/server/conn.go`、`pkg/extension/event_listener_test.go`、`pkg/server/conn_test.go`、`pkg/server/tests/commontest/tidb_test.go` 的生产/测试入口。
- 已核对的关键边：`Extensions::NewSessionExtensions` → `newSessionExtensions`；`WithSessionHandlerFactory` → `Manifest::sessionHandlerFactory` → `SessionHandler`；`newSessionExtensions` → `SessionExtensions::{connectionEventFuncs, stmtEventFuncs, authPlugins}`；包级分发函数 → 同名实例方法。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求目标文档存在，且恰好包含本文的 11 个固定二级标题。
