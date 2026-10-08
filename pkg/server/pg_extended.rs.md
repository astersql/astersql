# `pkg/server/pg_extended.rs`

## 文件定位

本文件实现 AsterSQL PostgreSQL 兼容监听器的 Extended Query 协议状态机，处理前端消息 `Parse`（`P`）、`Bind`（`B`）、`Describe`（`D`）、`Execute`（`E`）、`Close`（`C`）、`Sync`（`S`）和 `Flush`（`H`）。连接级入口在 `pkg/server/pg_conn.rs:356`：认证和启动响应完成后，每条连接创建一个 `Extended`，循环读取消息并调用 `Extended::handle`；当 `handle` 对简单查询 `Q` 返回 `false` 时，控制权回到 `pg_conn.rs` 的 simple-query 路径。

该模块属于 `astersql-server` crate。`pkg/server/Cargo.toml` 以 `lib.rs` 为 crate 根，`pkg/server/lib.rs:168-171` 将独立测试 `pg_extended_test.rs` 和生产模块 `pg_extended.rs` 分别装配。直接使用的外部 crate 只有 `chrono` 和 `astersql-types`，两者均由该 Cargo manifest 声明；协议、会话、目录查询和结果编码能力来自同 crate 模块。

## 核心职责

- 解析有严格边界的 PostgreSQL Extended Query 消息体，并将结构错误统一映射为 SQLSTATE `08P01`（`Reader::{take,string,count,u32,end}`，`pkg/server/pg_extended.rs:19-56`）。
- 将 PostgreSQL 的 `$1`…`$32767` 参数标记改写为引擎使用的 `?`，同时保留重复和乱序参数的映射；字符串、标识符和普通注释中的 `$n` 不参与改写（`markers`，`:61-144`）。
- 把文本或网络字节序的 PostgreSQL 参数按 OID 转成引擎 `BinaryParam`，或转成内建 `pg_catalog` 查询使用的 `Expr`；日期时间进一步转成引擎既有编码（`parameter`、`binary_parameter`、`catalog_parameter`、`catalog_binary_parameter`、`temporal`，`:147-400`）。
- 管理连接内 prepared statement 与 portal 的创建、绑定、描述、分段执行、关闭及错误恢复，并保证引擎 statement handle 在替换或关闭时得到释放（`Statement`、`Portal`、`Extended`，`:403-893`）。
- 复用 `pg_result::encode_formats` 生成 `RowDescription` 与执行结果，并拒绝 Prepare 与 Execute 元数据不一致的情况（`description_formats`，`:899-911`；`Extended::process` 的 `E` 分支，`:716-798`）。

## 主要符号

- `type Error = (&'static str, String)` / `Result<T>`：模块内部错误携带 SQLSTATE 和可发送给客户端的文本。`error` 构造协议/转换错误，`engine` 通过 `pg_conn::sqlstate` 转换 `ConnError`（`:11-17`）。
- `Reader<'a>`：对借用消息体进行前移式读取。`count` 读取有符号 16 位计数并拒绝负数；`end` 强制消息完整消费，避免默默接受尾随字节（`:19-56`）。
- `markers(sql) -> Result<(String, Vec<usize>)>`：返回改写后的 SQL 和“每个 `?` 应取哪个 Bind 参数”的零基下标表。它拒绝裸 `?`、标识符中的 `$n`、美元引号、越界索引、未闭合引号/块注释，以及可能暴露隐藏标记的 MySQL executable/hint comment（`:61-144`）。
- `parameter` / `binary_parameter`：分别处理文本格式与二进制格式参数。支持 bool、int2/int4/int8、float4/float8、text/varchar/bpchar、numeric、bytea、date/time/timestamp 的相应子集；`NULL` 生成 `is_null` 的引擎参数（`:147-288`）。
- `catalog_parameter` / `catalog_binary_parameter`：为 provider-owned `pg_catalog` 查询生成 `pg_catalog_query::Expr`。OID 26 特别保留完整 `u32` 范围，二进制值按网络字节序读取（`:290-360`）。
- `Statement`：保存目录查询或会话查询的特殊表示、引擎 `PreparedMetadata`、参数 OID、参数重排表以及命令标签（`:403-409`）。
- `Portal`：保存一次 Bind 后的执行实体，包括 statement 名、列与原生类型快照、引擎 statement id、实参、结果格式、缓存消息和分页偏移（`:411-424`）。
- `Extended`：连接级状态，包含启动时刻、`PgSession`、命名/匿名 statements 和 portals，以及“等待 Sync 恢复”的 `failed` 标志（`:426-432`）。公开到 crate 的入口为 `new`、`reset_unnamed` 和 `handle`；协议分派细节封装在私有 `process` 中。
- `description` / `description_formats`：以 Prepare 元数据构造只含描述的空 `QueryResult`，调用 `pg_result::encode_formats` 提取首个 `RowDescription`；无列时返回 `None`，上层发送 `NoData`（`:896-911`）。

## 执行流程

1. `pg_conn.rs:356-416` 在连接循环中把消息交给 `Extended::handle`。未知命令返回 `0A000`；`Q` 返回 `false` 交给简单查询；其余支持的 tag 进入扩展协议。发生协议/业务错误后 `failed=true`，后续非 `Sync` 消息被忽略。
2. `Sync` 必须是空消息体；它清除失败态，在非事务状态清空 portals，并发送 `ReadyForQuery`，状态字节由 `TiDBContext::in_transaction` 决定。`Flush` 只校验空消息体，因为 `handle` 已逐条同步写出所有生成的响应（`handle`/`process`，`:450-504`、`:883-886`）。
3. `Parse` 依次读取 statement 名、SQL 和参数 OID。SQL 先尝试识别 `pg_session::SessionQuery`，再尝试带类型的 `pg_catalog::CatalogQuery`；普通 SQL 则经过 `pg_name::adapt`、`markers` 和 `pg_result::adapt_session_query`。普通 SQL 调用 `TiDBContext::prepare_statement`，特殊查询直接生成 metadata。参数 OID 数量、是否可推断、支持范围和引擎参数计数全部通过后，才替换同名匿名 statement、关闭旧引擎 handle 并返回 `ParseComplete`（`:510-632`）。
4. `Bind` 校验 portal 名、statement 是否存在、参数格式数量和值数量；按每项格式调用文本或二进制转换器。普通 SQL 按 `Statement.mapping` 重排/复制实参，使 `$2,$1,$2` 对应三个引擎 `?`；目录查询则把 `Expr` 绑定到克隆的查询对象。结果格式被展开为每列一个 0/1 格式码，随后创建 portal 并返回 `BindComplete`（`:633-715`）。
5. `Describe Statement` 返回 `ParameterDescription`，再返回 `RowDescription` 或 `NoData`；`Describe Portal` 返回绑定结果格式对应的描述（`:716-748`）。
6. `Execute` 首次执行 portal 时，session 特殊查询由 `PgSession::execute` 处理，其余通过 `pg_conn.rs` 提供的闭包进入目录执行或 `TiDBContext::execute_prepared_statement`。结果编码和响应生命周期结束后，消息缓存在 portal 中。`limit=0` 表示不限行；正数限制按 `DataRow` 计数，未耗尽时追加 `PortalSuspended`，后续 Execute 从 `offset` 继续，耗尽时保留/重放完成消息（`:749-843`）。
7. `Close Statement` 关闭真实引擎 statement 并删除其所有 portals；特殊目录/会话 statement 没有引擎 handle，不调用关闭。关闭不存在对象也成功。`Close Portal` 只删除目标 portal；两者返回 `CloseComplete`（`:844-882`）。

## 数据与状态

`Extended.statements` 与 `Extended.portals` 都以协议名称为键；空字符串表示 PostgreSQL 匿名对象。重新 Parse 匿名 statement 会移除旧匿名 portal，`reset_unnamed` 在 simple query 前释放匿名真实 statement 并删除关联 portal（`:441-448`，调用点 `pkg/server/pg_conn.rs:386`）。命名 statement 不允许重复 Parse；匿名 statement 允许替换，但旧引擎 handle 必须先关闭。

`Statement.metadata` 是 Parse 时的稳定契约：参数数、列信息、原生 PostgreSQL 类型映射所需信息及引擎 statement id。`Portal` 在 Bind 时复制列/类型和格式，并在首次 Execute 后缓存编码消息；因此分页不会重复执行 SQL。对无参数且执行结果遗漏原生类型的常量投影，执行阶段可以回填 Prepare 快照；带参数投影缺失原生类型则不使用这一兜底（`:778-787`）。Prepare 与 Execute 的 `RowDescription` 不一致会返回 `0A000`，避免客户端依据过期元数据解码。

`failed` 表示 PostgreSQL 扩展协议的 error-recovery 状态，而不是事务本身。它只由错误设置、由合法空 `Sync` 清除。`startup_epoch_micros` 被传给查询适配器，以保证依赖服务器启动时刻的兼容查询在 Prepare/Simple 路径上一致。

## 依赖与调用关系

- 上游：`PgService` 的连接处理循环在 `pkg/server/pg_conn.rs:356` 构造 `Extended`，在 `:367` 调用 `handle`，并在 simple-query 分支调用 `reset_unnamed`。这是目标文件在完整应用中的实际接线证据。
- 执行边界：`TiDBContext::{prepare_statement,execute_prepared_statement,close_prepared_statement,in_transaction,finish_protocol_response}` 负责真实 SQL handle、事务状态与协议响应收尾；执行闭包由 `pg_conn.rs` 的 `with_query` 包装，以纳入连接的活动查询/取消管理。
- SQL 适配：`pg_name::adapt` 处理 PostgreSQL 名称兼容，`pg_result::{adapt_session_query,command,encode_formats}` 负责查询适配、命令分类和 PG wire 结果编码。
- 特殊查询：`pg_catalog::CatalogQuery`/`pg_catalog_query::Expr` 处理目录查询并允许 Bind 后观察最新目录状态；`pg_session::{PgSession,SessionQuery}` 处理当前 schema 等连接内会话行为。
- 数据类型：`conn_stmt::BinaryParam` 是引擎预编译参数表示；`astersql_types::decimal::mydecimal::MyDecimal::FromString` 验证 numeric 文本；`chrono` 验证日期时间并提取字段。
- 写网：`pg_conn::{write_message,write_error,sqlstate}` 将本模块的逻辑响应和错误写回同一 `TcpStream`。

RustCodeGraph 的 `query`/`node` 确认 `Extended`（`:426`）、`markers`（`:61`）、`process`（`:506`）和 `description_formats`（`:899`）。`callees pg_extended.rs::process` 识别到本文件解析/转换函数以及 `pg_name::adapt`、`pg_result::adapt_session_query`、`pg_result::command`、`pg_result::encode_formats` 等边；索引对常见短名存在过度匹配，且目标 callers 查询没有给出可靠结果，因此上游边采用上述直接源码调用点，不把噪声边当作事实。

## 错误处理与边界

协议 framing 错误使用 `08P01`：截断、负计数、尾随字节、非法 target/format、参数或结果格式数量不匹配、非法长度、过大的 Execute 行数以及非空 Sync/Flush。不存在 statement/portal 分别使用 `26000`/`34000`，命名对象重复使用 `42P05`/`42P03`。

参数标记错误区分语法 `42601`、未定义/越界参数 `42P02` 和不支持能力 `0A000`。文本解码使用 `22021`，文本值不合法使用 `22P02`，二进制形状不合法使用 `22P03`，日期时间不合法使用 `22007`，OID 数值溢出使用 `22003`。引擎 `ConnError` 经 `sqlstate` 映射；执行闭包自身的 `io::Error` 在协议层包装为 `XX000` 文本。

当前明确限制包括：不支持 dollar-quoted SQL；不接受客户端直接发送 `?`；普通引擎查询不能推断未给定/为 0 的参数 OID；文本/binary OID 仅支持列出的子集；bytea 文本只支持 `\\x` 十六进制；时间精度最多微秒；日期限于引擎的 1..=9999 年；空 prepared statement 不支持。对普通 SQL，metadata 参数数与改写后的 marker 出现次数不一致会先关闭刚创建的引擎 handle，再报错，避免资源泄漏。

`Extended::handle` 将语义错误转成 PostgreSQL ErrorResponse 并进入等待 Sync 状态；真正的 socket 写错误仍以 `io::Result` 返回连接循环。Execute 在引擎错误和正常编码后都调用 `finish_protocol_response`，若 `QueryResult` 带 `response_lifecycle` 则显式 `finish()`。

## 并发与资源生命周期

本文件自身不创建线程、任务、锁或通道。每个连接在 `pg_conn.rs` 的连接处理线程中拥有一个可变 `Extended`，消息按顺序处理；`TcpStream`、`&mut self` 以及一次性执行闭包共同保证单连接内没有并发修改 statement/portal 状态。跨连接执行和取消的并发管理位于 `PgService::with_query`、`TiDBContext` 与 `CancellationToken`，不在本文件实现。

真实引擎 statement 的生命周期从成功 `prepare_statement` 开始，在同名匿名替换、显式 `Close Statement` 或 `reset_unnamed` 时结束。`pg_catalog` 和 `pg_session` statement 没有引擎 handle，关闭时不得误关其他 statement。portal 只借用 statement id 的逻辑身份并持有自身参数/消息缓存；关闭 statement 会级联删除关联 portals，关闭 portal 不关闭 statement。非事务 `Sync` 会清空 portals，事务内 `Sync` 保留它们；连接断开后的整体 context 清理由上层负责。

分页 Execute 只在第一次运行查询并完成响应生命周期，之后从内存消息缓存切片；这避免重复副作用，但也意味着大结果会被完整编码并驻留到 portal 被清理。扩展时需要评估内存上限，不能把当前 `limit` 误解为引擎侧流式限流。

## 与 Go 版本的对应关系

仓库 `pkg/server` 下没有 `pg_extended.go` 或等价的 Go PostgreSQL Extended Query 状态机；相邻 Go server 文件主要是原 TiDB/MySQL 协议实现。因此本文件不是同路径 Go 文件的逐函数复刻，而是 Rust PostgreSQL 兼容入口的新增实现，不能从 Go 代码推断其协议行为。

可对齐的引擎契约仍来自 Rust `TiDBContext` 对既有 server/session 能力的封装：Prepare、Execute、Close、事务状态、取消和结果生命周期均通过该抽象接入，而非在协议层复制 SQL 执行器。语义回归的直接依据是独立 Rust 测试 `pkg/server/pg_extended_test.rs`，以及真实客户端覆盖 `pkg/server/pg_client_integration_test.rs`；当前无同路径 Go 测试可一一对应。

## 扩展指南

- 新增 frontend tag：在 `Extended::handle` 的允许列表和 `process` 分支同时接入，明确错误后的 Sync 行为、响应 tag 与消息体完整消费；在 `pg_extended_test.rs` 通过真实 socket 增加正常、截断、尾随字节和恢复用例。
- 新增参数 OID：同步修改 Parse 阶段普通 SQL 的 OID 白名单、`parameter` 与需要时的 `binary_parameter`；若目录查询也使用该类型，还要扩展两套 catalog 转换器。测试必须覆盖文本、二进制、NULL、上下界、错误 SQLSTATE、大小端和无损性。
- 修改 marker 语法：优先改 `markers`，保持 mapping 对重复/乱序参数的每次出现记录；同步 `marker_mapping_preserves_literals_comments_and_index_order`，并特别检查引号、注释、标识符边界及 executable comments，防止 SQL 注入或参数错绑。
- 修改 statement/portal 生命周期：集中检查 `Parse` 替换、`reset_unnamed`、`Close` 和 `Sync` 四个清理点，确保仅真实引擎 statement 调用 `close_prepared_statement`，且关联 portal 不悬挂。相关回归应留在独立 `pg_extended_test.rs`，不要把测试嵌入生产源文件。
- 新增结果格式或流式执行：`description_formats` 与 Execute 编码必须保持同一 OID、长度和 format code；当前 portal 完整缓存结果，若改为流式需要重新定义 response lifecycle、取消、错误后 Sync、事务边界和 portal 重入语义。
- 兼容风险集中在 PostgreSQL SQLSTATE、消息顺序、statement/portal 命名规则和 Describe/Execute 元数据一致性；性能风险集中在 SQL marker 线性扫描、metadata/参数克隆及完整结果缓存。任何优化都应保持客户端可观察顺序，并用真实监听器测试验证。

## 验证依据

- 生产源码：`pkg/server/pg_extended.rs` 全文；重点为 `Reader`（`:19-56`）、参数解析（`:61-400`）、状态结构（`:403-432`）、入口/恢复（`:434-504`）、消息分派（`:506-893`）和描述编码（`:896-911`）。
- 应用接线：`pkg/server/pg_conn.rs:356-416`，确认连接级实例、`handle`/`reset_unnamed` 调用、prepared execute 闭包及 simple-query 分流；`pkg/server/lib.rs:168-171` 确认生产模块与独立测试装配。
- crate 边界：`pkg/server/Cargo.toml`，确认 crate 名 `astersql-server`、根 `lib.rs`、`chrono`、`astersql-types` 和测试依赖；没有为本任务引入或修改依赖。
- 独立测试：`pkg/server/pg_extended_test.rs`。`parse_bind_execute_sync` 覆盖完整消息链、目录查询、描述、portal/statement 生命周期、分页与恢复；`marker_mapping_preserves_literals_comments_and_index_order` 覆盖 marker 语义；`text_parameters_are_typed_and_reject_loss` 覆盖类型和精度；`canonical_prepare_parameter_syntax_probe` 证明底层引擎仍使用 `?`；`pg_introspection_parameters_live` 覆盖 OID、重排/重复/NULL、注入输入和错误状态；`pg_introspection_binary_results_and_recovery` 覆盖结果格式及 Sync 恢复。
- RustCodeGraph：`status` 显示索引存在；`query Extended --kind struct`、`query markers --kind function`、`query description_formats --kind function` 定位目标定义；`node Extended` 返回目标结构；`callees pg_extended.rs::process`/`callees pg_extended.rs::markers` 给出目标内部及 `pg_name`、`pg_result` 调用。`files --filter pkg/server/pg_extended` 未命中且 callers 对短名未返回可靠边，故这些缺口由上述直接源码证据补齐并在本文显式限定。
- Go 对照检查：搜索 `pkg/server/**/*.go` 及同目录 `*pg*` 文件，未发现 `pg_extended.go` 或 Go PostgreSQL Extended Query 实现；因此本文只陈述“无同路径对应”，不臆造迁移关系。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核唯一生产物、源码符号、调用边、限制和独立测试位置。
