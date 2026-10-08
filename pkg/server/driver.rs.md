# `pkg/server/driver.rs`

## 文件定位

`pkg/server/driver.rs` 属于 `astersql-server` crate；`pkg/server/Cargo.toml` 将该 crate 的根设为 `pkg/server/lib.rs`，后者以 `pub mod driver` 导出本文件。它定义 MySQL 服务端与会话驱动之间的一组 Rust 抽象：连接打开、结果逐行拉取、预处理语句参数与游标状态、以及相关资源的显式关闭。

当前文件是**契约层而不是生产实现层**。仓库搜索只发现 `pkg/server/driver_test.rs` 直接导入并实现这些 trait；生产 Rust 路径没有 `impl IDriver`、`impl DriverContext` 或 `impl PreparedStatement for ...`。当前实际连接与 COM_STMT 流程使用 `pkg/server/conn.rs`、`pkg/server/conn_stmt.rs` 和 `pkg/server/driver_tidb.rs` 中另一套具体类型及 trait。因此，本文件表达了 Go `pkg/server/driver.go` 的移植边界，但尚未接入完整应用主链。

文件没有条件编译项、全局变量或模块级常量；全部条目均为公开类型或公开 trait。源码顶部保留 PingCAP Apache License，并标记为 AsterSQL 已处理文件。

## 核心职责

1. 用 `IDriver::OpenCtx` 定义“每个客户端连接打开一个会话上下文”的边界，并传入连接 ID、能力位、校对规则、初始数据库、TLS 状态和可选会话扩展。
2. 用 `DriverContext` 为连接侧会话资源规定显式 `close` 生命周期。
3. 用 `ResultSet`、`CursorResultSet` 和 `RowContainer` 分离普通结果读取、游标收尾与行缓存释放职责。
4. 用 `PreparedStatement` 汇总 COM_STMT 协议需要的状态操作：执行、长参数追加与检查、参数类型缓存、游标结果集保存、重置/关闭以及行容器保存。
5. 用 `Expression`、`TlsState`、`SessionExtensions` 和本地 `Error` 提供契约两侧共同可传递的最小数据模型。

这些职责只规定方法形状，不提供参数解析、SQL 执行、限额检查、锁策略或资源清理次序的默认实现；这些语义必须由实现者保证。

## 主要符号

- `Error(pub String)`：公开的字符串错误包装。实现 `Clone`、`Debug`、`Eq`、`PartialEq`、`Display` 和 `std::error::Error`；`Display` 原样输出内部字符串。它没有错误码、来源链或分类字段。
- `TlsState`：TLS 握手状态快照，包含可选协议版本字符串、可选密码套件字符串和零到多个对端证书 DER 字节串。派生 `Default` 时两个可选字段为 `None`、证书列表为空。
- `SessionExtensions`：会话扩展名称列表 `names: Vec<String>`。`Option<SessionExtensions>` 的 `None` 与“存在但名称列表为空”是不同状态。
- `Expression`：协议边界值枚举，支持 `Null`、`Signed(i64)`、`Unsigned(u64)`、`Float(f64)` 和 `Bytes(Vec<u8>)`。它不携带 SQL 类型、字符集、精度或时间/十进制定制表示。
- `ResultSet: Send`：`next(&mut self, &CancellationToken)` 每次返回一行 `Vec<Expression>`、结束标记 `None` 或 `Error`；`close` 显式释放资源。
- `CursorResultSet: ResultSet`：在普通结果集之上增加 `finish`，供 COM_STMT_FETCH 游标结束时完成额外收尾。
- `RowContainer: Send`：只暴露 `close`，代表可由预处理语句持有并释放的行容器。
- `DriverContext: Send`：单连接上下文的最小生命周期接口，仅暴露 `close`。
- `IDriver: Send + Sync`：驱动工厂接口。`OpenCtx` 借用驱动、返回拥有所有权的 `Box<dyn DriverContext>`，允许同一驱动在多线程间共享并为连接创建独立上下文。
- `PreparedStatement: Send`：预处理语句完整协议契约。`ID` 返回 `i32`；`Execute` 接受取消令牌和表达式切片；`AppendParam`/`CheckLongDataSize` 管理长参数；`NumParams`、`BoundParams`、`SetParamsType`、`GetParamsType` 暴露绑定状态；`StoreResultSet`/`GetResultSet` 和 `StoreRowContainer`/`GetRowContainer` 管理可选资源；`Reset`/`Close` 执行清理；`GetCursorActive`/`SetCursorActive` 管理游标标志。

方法名沿用 Go 风格的大写命名，因此不符合惯常 Rust `snake_case`，但有利于逐项对照 `pkg/server/driver.go`。

## 执行流程

本文件自身没有可执行主流程，预期协议流程由调用者和实现者共同组成：

1. 新连接握手完成后，连接层收集连接 ID、客户端 capability、collation、数据库名以及可选 TLS/扩展信息，调用 `IDriver::OpenCtx`。
2. 驱动创建并配置连接专属会话，返回 `Box<dyn DriverContext>`；连接结束时调用 `DriverContext::close`。
3. COM_STMT_PREPARE 的实现创建一个 `PreparedStatement`，并以 `ID` 和 `NumParams` 建立协议侧语句状态。
4. COM_STMT_SEND_LONG_DATA 通过 `AppendParam` 向指定参数槽追加数据；后续执行前必须由协议层调用 `CheckLongDataSize`，因为契约允许追加阶段延迟报告超限错误。
5. COM_STMT_EXECUTE 使用 `BoundParams`、`GetParamsType` 等状态组装 `Expression` 参数，再以取消令牌调用 `Execute`。结果通过 `ResultSet::next` 逐行拉取，`Ok(None)` 表示耗尽。
6. 非游标结果处理完毕后调用 `ResultSet::close`。游标模式下，调用者可用 `StoreResultSet` 保存 `CursorResultSet`，用 `SetCursorActive` 维护活动状态，并在 FETCH/结束路径调用继承的结果集方法及 `finish`。
7. 物化游标可通过 `StoreRowContainer` 把行容器挂到语句上。RESET 应通过 `Reset` 清除参数和游标资源；CLOSE 应通过 `Close` 释放全部语句资源并从后端移除语句。

以上第 3 至 7 步是接口形状与 Go 对照共同表达的调用协议，不是本文件中的默认实现。当前 Rust 生产路径的真实 COM_STMT 流程在 `pkg/server/conn.rs::handleStmt` 和 `pkg/server/conn_stmt.rs` 中运行，未通过本文件的 `PreparedStatement` trait 分派。

## 数据与状态

本文件唯一有自身行为的状态是若干值对象；所有会话和语句状态均由 trait 实现者持有：

- TLS 数据由 `TlsState` 拥有字符串和证书字节，传给 `OpenCtx` 时整体移动；`None` 表示无 TLS 状态。
- 扩展数据由 `SessionExtensions.names` 拥有。`pkg/server/driver_test.rs::driver_open_context_preserves_nil_session_extensions` 验证 `None` 必须原样传给驱动，不能自动改成空列表。
- `Expression::Bytes` 拥有载荷，执行入口只借用表达式切片，因此实现不得假定参数在调用后仍可借用。
- `BoundParams` 返回 `&[Option<Vec<u8>>]`：`None` 可表达未提供参数，`Some(Vec::new())` 可表达已提供空长数据。只读借用阻止调用者绕过 `AppendParam` 修改内部缓存。
- 参数类型由实现拥有；`SetParamsType(Vec<u8>)` 转移所有权，`GetParamsType` 只读借用。
- 结果集和行容器通过 `Option<Box<dyn ...>>` 转移所有权。`GetResultSet`/`GetRowContainer` 只返回与 `&mut self` 绑定的可变 trait-object 借用，避免资源从语句中被无意移出。
- 游标活动标志的具体存储不在接口中；实现必须保持它与已保存结果集、FETCH 耗尽及 RESET 的状态一致。

文件不定义序列化格式、共享缓存、原子计数或内部锁，也不保证 `Reset`/`Close` 幂等；这些均是实现层责任。

## 依赖与调用关系

直接 Rust 依赖只有标准库 `std::fmt` 和 `crate::conn::CancellationToken`。`pkg/server/Cargo.toml` 未为本文件声明专属 feature；模块随 `astersql-server` 库一起编译。`CancellationToken` 把连接层的查询取消信号传入语句执行和逐行读取，是本文件与连接运行时唯一的直接类型耦合。

RustCodeGraph 对 `pkg/server/driver.rs` 建立了 41 个符号，并报告该文件被 9 个文件使用；进一步用精确仓库搜索排除同名符号后，直接的 `crate::driver` 导入/实现只出现在 `pkg/server/driver_test.rs`。这意味着图中的部分“used by”来自同名符号或宽松引用关系，不能据此宣称生产调用已接线。

相关但不相同的生产实现包括：

- `pkg/server/conn.rs`：定义另一种 `TlsState` 和连接级 `CancellationToken`，其 `handleStmt` 直接创建 `crate::conn_stmt::PreparedStatement`。
- `pkg/server/conn_stmt.rs`：定义实际协议路径使用的 `PreparedStatement` 结构和 `StatementRuntime`，负责报文解析、长数据、游标及结果关闭。
- `pkg/server/driver_tidb.rs`：定义另一套 `Error`、`Expression`、`ResultSet`、`CursorResultSet`、`RowContainer`，以及具体 `TiDBDriver`/`TiDBContext`/`TiDBStatement` 方法；这些类型没有实现本文件的 trait。
- `pkg/server/internal/resultset/*.rs`：提供当前生产游标结果集实现，但实现的是该子模块自己的结果集接口，而非本文件的 `CursorResultSet`。

因此，安全扩展时必须先判断目标是维护这一移植契约，还是修改当前已接线的生产路径，不能仅修改本文件便期待运行时行为变化。

## 错误处理与边界

- 所有可失败操作统一返回本地 `Error(String)`，错误信息完全由实现者提供；接口没有协议错误码映射。若接入生产路径，需要在连接边界将其稳定映射为 MySQL 错误，避免依赖字符串判断。
- `ResultSet::next` 的三态必须区分：`Ok(Some(row))` 是一行，`Ok(None)` 是正常结束，`Err` 是读取失败。空行应表示 `Some(Vec::new())`，不能与结束混淆。
- `CancellationToken` 是协作式取消：trait 只传递令牌，不自动检查。`pkg/server/driver_test.rs` 的测试实现主动检查 `is_cancelled()`，并验证已取消时 `PreparedStatement::Execute` 与 `ResultSet::next` 返回 `Error("query cancelled")`。
- `AppendParam` 使用 `usize` 参数下标，但不规定越界、空分片、单参数/总量上限或延迟报错策略。Go 实现把错误延迟到 `CheckLongDataSize`/EXECUTE，并区分未绑定与空数据；Rust 实现者若要兼容必须显式复现这些边界。
- `ID()` 返回 `i32`，而协议语句 ID 通常是无符号 32 位值；实现从 `u32` 转换时应定义超出 `i32::MAX` 的行为。当前接口本身没有保护。
- `TlsState` 使用可选字符串而非 `rustls` 原生枚举，且不验证协议名、密码套件名或证书 DER；它只是边界快照。
- `finish`、`close`、`Reset`、`Close` 的调用次序及重复调用语义未编码。实现必须避免双重释放，并决定一个清理步骤失败后是否继续清理其余资源。

## 并发与资源生命周期

`IDriver: Send + Sync` 表示驱动对象可跨线程发送并共享；实现若持有 store 或全局配置，必须自行同步。`DriverContext`、`PreparedStatement`、`ResultSet` 和 `RowContainer` 只要求 `Send`，允许所有权跨线程移动，但不允许在没有额外同步的情况下并发共享。所有可变操作都要求 `&mut self`，在类型层面串行化同一 trait object 的正常访问。

资源生命周期是显式而非 RAII 强制：

- `OpenCtx` 创建的上下文需要调用 `DriverContext::close`；trait 没有 `Drop` 兜底。
- `Execute` 返回的普通结果集需要 `close`；游标结果集还可能需要 `finish`。
- `StoreResultSet` 和 `StoreRowContainer` 把资源所有权交给语句；后续 `Reset`/`Close` 必须负责释放。
- `Reset` 预期保留语句本身但清空一次执行状态；`Close` 预期终结语句及后端 prepared state。接口没有自动清理或状态机来阻止关闭后再次使用。

Go 的 `TiDBStatement::Reset`/`Close` 会报告游标 RU、关闭迭代器、分离行容器内存/磁盘 tracker、关闭容器并释放长参数记账；这些是兼容实现需要维护的资源顺序，但本 Rust trait 没有对应的 tracker 方法，不能声称已经覆盖。Go 结果集还用互斥锁与原子 closed 标志协调 `Next`、`Finish`、`Close`；本文件只通过 `&mut self` 和 `Send` 提供较弱的并发契约。

## 与 Go 版本的对应关系

主要原型是 `pkg/server/driver.go`：Rust `IDriver` 和 `PreparedStatement` 基本按方法逐项映射，`pkg/server/driver_tidb.go` 的 `TiDBDriver`/`TiDBStatement` 提供 Go 侧真实实现证据。

关键差异如下：

- Go `IDriver::OpenCtx` 返回具体 `*TiDBContext`；Rust 返回 `Box<dyn DriverContext>`，隐藏具体会话能力。目前 Rust `DriverContext` 只有 `close`，不足以表达 Go `TiDBContext` 的查询、Prepare、警告和状态迁移接口。
- Go TLS 参数是 `*tls.ConnectionState`，扩展参数是 `*extension.SessionExtensions`；Rust 用最小快照结构替代，并用 `Option` 保留 nil/非 nil 区别。
- Go `expression.Expression` 是完整表达式接口；Rust `Expression` 只是五种已求值标量。复杂类型、类型元数据和惰性求值不在此文件覆盖范围内。
- Go `resultset.ResultSet` 暴露列、chunk 分配、`Next(context.Context, *chunk.Chunk)`、关闭、`Finish`、detach 等能力；Rust `ResultSet` 只逐行返回 `Vec<Expression>`。Rust `CursorResultSet::finish` 也不等价于 Go `CursorResultSet::GetRowIterator`，属于简化契约。
- Go `chunk.RowContainer` 提供 reader 和内存/磁盘 tracker；Rust `RowContainer` 只提供 `close`，不能直接表达 tracker detach 或游标 reader。
- Go `context.Context` 同时携带取消、截止时间和值；Rust 仅传 `CancellationToken`，没有 deadline 或 context values。
- Go `BoundParams() [][]byte` 以 nil slice 表达未绑定；Rust用 `Option<Vec<u8>>` 明确区分未绑定与空字节。
- Go `TiDBStatement` 已实现长数据 packet 上限、内存配额记账、游标 RU 和 plan cache 清理；本文件只声明入口，不实现这些行为。

Go 测试 `pkg/server/conn_stmt_test.go` 覆盖 RESET 后 FETCH 失败、长数据超过 `max_allowed_packet` 延迟到 EXECUTE 报错、内存配额拒绝及 CLOSE 归还记账；`pkg/server/conn_test.go` 覆盖真实 `OpenCtx` 和 TLS 状态。这些是未来接线时的兼容基线，不是当前 `driver.rs` 已通过的 Rust 行为。

## 扩展指南

- 新增协议值类型时，优先扩展 `Expression`，并同步所有实现、编码/解码边界和独立测试；同时检查当前生产路径 `driver_tidb.rs::Expression`，避免两套枚举继续漂移。需评估 SQL 类型精度、字符集、无符号范围与复制成本。
- 扩展连接初始化信息时修改 `IDriver::OpenCtx`、`TlsState` 或 `SessionExtensions`，并同步 `pkg/server/driver_test.rs`。新增可选字段要保留 `None` 与空值语义；敏感 TLS/扩展数据不应进入普通错误或调试输出。
- 增加预处理语句能力时修改 `PreparedStatement`，并在独立的 `pkg/server/driver_test.rs` 中增加实现与契约测试；不要把测试嵌入生产源文件。若目标是运行时行为，还必须同步当前 `conn_stmt.rs`/`driver_tidb.rs` 路径及对应测试。
- 实现这些 trait 并接入生产路径前，应先消除或适配与 `driver_tidb.rs`、`internal/resultset` 的重复抽象，明确单一错误类型、结果集模型和关闭次序。仅添加空实现或桥接桩不能作为完成证据。
- 长数据实现应复现 Go 的延迟错误、单参数上限、会话内存记账和 RESET/EXECUTE/CLOSE 释放不变量；重点回归 `pkg/server/conn_stmt_test.go` 中相应场景，并在 Rust 的独立测试文件中添加等价测试。
- 游标实现应保证结果集、迭代器、行容器和资源计量按一致顺序完成，且错误路径不会泄漏；需要评估物化行的内存/磁盘占用和每行 `Vec<Expression>` 分配开销。
- 若改变线程安全界限（例如要求共享结果集），应审慎考虑是否从 `Send` 提升到 `Send + Sync`，并用锁策略和并发测试证明 `next`/`finish`/`close` 的互斥与幂等性。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/server/driver.rs` 确认目标文件已索引；`node --file pkg/server/driver.rs --offset 1 --limit 500` 读取到全文件 130 行和 41 个符号，并报告 9 个宽松使用关系。精确 caller 查询未在限定时间内返回，因此没有把未解析的图边当作事实。
- 目标源码：`pkg/server/driver.rs`，核对全部公开结构、枚举、trait、方法签名、派生实现及无条件编译项。
- crate 边界：`pkg/server/Cargo.toml`、`pkg/server/lib.rs`，核对 crate 名、根文件、`pub mod driver` 和独立测试挂载方式。
- Rust 直接测试：`pkg/server/driver_test.rs`，核对取消传播和 `None` 会话扩展语义；仓库精确搜索确认它是当前唯一直接导入/实现本文件 trait 的 Rust 文件。
- Rust 相邻生产路径：`pkg/server/conn.rs`、`pkg/server/conn_stmt.rs`、`pkg/server/driver_tidb.rs`、`pkg/server/internal/resultset/cursor.rs`，核对当前连接、COM_STMT、驱动与游标实际使用的独立类型。
- Go 对照：`pkg/server/driver.go`、`pkg/server/driver_tidb.go`、`pkg/server/internal/resultset/resultset.go`、`pkg/server/internal/resultset/cursor.go`，核对接口原型、具体实现、结果集和资源生命周期差异。
- Go 回归证据：`pkg/server/conn_stmt_test.go`、`pkg/server/conn_test.go`，核对游标 RESET、长数据上限/内存配额、CLOSE 释放、`OpenCtx` 与 TLS 行为。
- 本任务是纯文档分析，按计划不运行 Cargo；验证只包含事实搜索、人工交叉核对和任务指定的 11 章节结构检查。
