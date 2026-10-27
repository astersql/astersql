# `pkg/server/pg_extended.rs`

## 文件定位

管理连接内 prepared statement 与 portal 的创建、绑定、描述、分段执行、关闭及错误恢复，并保证引擎 statement handle 在替换或关闭时得到释放（`Statement`、`Portal`、`Extended`，`:403-893`）。

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

当前明确限制包括：不支持 dollar-quoted SQL；不接受客户端直接发送 `?`；普通引擎查询仅从共享 AST 的直接 int4/int8 显式转换推断未给定/为 0 的参数 OID，未受约束的参数返回 `42P18`；文本/binary OID 仅支持列出的子集；bytea 文本只支持 `\\x` 十六进制；时间精度最多微秒；日期限于引擎的 1..=9999 年；空 prepared statement 不支持。对普通 SQL，metadata 参数数与改写后的 marker 出现次数不一致会先关闭刚创建的引擎 handle，再报错，避免资源泄漏。

`Extended::handle` 将语义错误转成 PostgreSQL ErrorResponse 并进入等待 Sync 状态；真正的 socket 写错误仍以 `io::Result` 返回连接循环。Execute 在引擎错误和正常编码后都调用 `finish_protocol_response`，若 `QueryResult` 带 `response_lifecycle` 则显式 `finish()`。

## 并发与资源生命周期

本文件自身不创建线程、任务、锁或通道。每个连接在 `pg_conn.rs` 的连接处理线程中拥有一个可变 `Extended`，消息按顺序处理；`TcpStream`、`&mut self` 以及一次性执行闭包共同保证单连接内没有并发修改 statement/portal 状态。跨连接执行和取消的并发管理位于 `PgService::with_query`、`TiDBContext` 与 `CancellationToken`，不在本文件实现。

真实引擎 statement 的生命周期从成功 `prepare_statement` 开始，在同名匿名替换、显式 `Close Statement` 或 `reset_unnamed` 时结束。`pg_catalog` 和 `pg_session` statement 没有引擎 handle，关闭时不得误关其他 statement。portal 只借用 statement id 的逻辑身份并持有自身参数/消息缓存；关闭 statement 会级联删除关联 portals，关闭 portal 不关闭 statement。非事务 `Sync` 会清空 portals，事务内 `Sync` 保留它们；连接断开后的整体 context 清理由上层负责。

分页 Execute 只在第一次运行查询并完成响应生命周期，之后从内存消息缓存切片；这避免重复副作用，但也意味着大结果会被完整编码并驻留到 portal 被清理。扩展时需要评估内存上限，不能把当前 `limit` 误解为引擎侧流式限流。

## 与 Go 版本的对应关系

仓库 `pkg/server` 下没有 `pg_extended.go` 或等价的 Go PostgreSQL Extended Query 状态机；相邻 Go server 文件主要是原 TiDB/MySQL 协议实现。因此本文件不是同路径 Go 文件的逐函数复刻，而是 Rust PostgreSQL 兼容入口的新增实现，不能从 Go 代码推断其协议行为。

## 扩展指南

- 新增 frontend tag：在 `Extended::handle` 的允许列表和 `process` 分支同时接入，明确错误后的 Sync 行为、响应 tag 与消息体完整消费；在 `pg_extended_test.rs` 通过真实 socket 增加正常、截断、尾随字节和恢复用例。
- 新增参数 OID：同步修改 Parse 阶段普通 SQL 的 OID 白名单、`parameter` 与需要时的 `binary_parameter`；若目录查询也使用该类型，还要扩展两套 catalog 转换器。测试必须覆盖文本、二进制、NULL、上下界、错误 SQLSTATE、大小端和无损性。
- 修改 marker 语法：优先改 `markers`，保持 mapping 对重复/乱序参数的每次出现记录；同步 `marker_mapping_preserves_literals_comments_and_index_order`，并特别检查引号、注释、标识符边界及 executable comments，防止 SQL 注入或参数错绑。
- 修改 statement/portal 生命周期：集中检查 `Parse` 替换、`reset_unnamed`、`Close` 和 `Sync` 四个清理点，确保仅真实引擎 statement 调用 `close_prepared_statement`，且关联 portal 不悬挂。相关回归应留在独立 `pg_extended_test.rs`，不要把测试嵌入生产源文件。
- 新增结果格式或流式执行：`description_formats` 与 Execute 编码必须保持同一 OID、长度和 format code；当前 portal 完整缓存结果，若改为流式需要重新定义 response lifecycle、取消、错误后 Sync、事务边界和 portal 重入语义。
- 兼容风险集中在 PostgreSQL SQLSTATE、消息顺序、statement/portal 命名规则和 Describe/Execute 元数据一致性；性能风险集中在 SQL marker 线性扫描、metadata/参数克隆及完整结果缓存。任何优化都应保持客户端可观察顺序，并用真实监听器测试验证。

## 显式整数参数约束

普通 Parse 先取得 `PreparedMetadata.parameter_cast_oids`，其索引是 engine `?` 的出现顺序。PG 层用现有 mapping 将约束折叠为 `$n` 编号顺序；省略/0 OID 使用首先出现的直接整数 cast，显式非零 OID 是源类型，不被目标 cast 覆盖。重复或乱序引用仍按 mapping 扩展 Bind 参数。未知参数在注册 PG statement 前关闭刚创建的 engine statement，返回 `42P18`。

int2/int4/int8文本参数复用共享PG18整数扫描，接受ASCII首尾空白、进制前缀与下划线，并区分语法 `22P02` 和溢出 `22003`。Bind 在全部参数和格式校验后才插入 portal。对于参数均受直接整数转换约束的语句，Bind 后共享内核会重解析字面值；结果列名沿用 Parse 描述，结果类型仍来自实际执行并接受原有一致性校验。`pg_core_integer_parameter_types` 通过真实 TCP 与 libpq 验证参数描述字节、编号绑定、NULL、显式源类型和 Sync 恢复。

## 2026-10-25 PG scalar 01

OID16的NULL参数保持共享TypeTiny并设置is_null，PG语句独立保留OID16；复用既有bool结果codec与一致性检查。

## 2026-10-25 PG scalar 02

OID16文本参数仅在PG入口清理ASCII的六种isspace字符，忽略ASCII大小写，接受true/false、yes/no、on/off、1/0及唯一前缀；歧义o和空输入仍报22P02。UTF8及NUL先由既有校验报22021。目录参数复用此入口；binary与MySQL转换保持原行为。独立pg_scalar_02通过真实TCP验证词集、NULL/描述、错误恢复及MySQL隔离。

## 整数文本统一解析（任务6）

OID21/23/20的非NULL文本参数分别复用astersql_expression::pg_cast_int2/4/8，与正式CAST及关系求值一致；保留共享错误sqlstate。支持PG18进制前缀与下划线，不再单独使用Rust FromStr。Bind拒绝及portal发布时序保持。pg_extended_test::pg_scalar_06覆盖文本常量、列、推断/显式参数、typed NULL、空描述、错误阶段、Sync/ROLLBACK恢复与拒绝写入无副作用，以及同Server MySQL实际值/元数据/告警。

## PostgreSQL 有限 numeric 承载

OID1700 文本参数经 PgNumeric 验值规范化；用文本共享参数载荷绕开 MySQL prepared DECIMAL 的容量限制，目标身份由准备元数据保留。


## PostgreSQL float4 文本输入（任务16）

OID700文本参数复用 pg_cast_float4；在Bind阶段检查词法、ASCII空白及binary32溢出/下溢，UTF-8/NUL仍由既有22021检查负责。

## PostgreSQL float8 文本参数（任务17）

OID701文本参数按binary64直接解析，允许ASCII空白和显式特殊值，有限溢出及非零下溢为零拒绝22003；非法文本22P02和编码22021保持原入口。

### 2026-10-26 浮点真实客户端差分（任务20）

OID701文本参数改为复用pg_cast_float8，保持原合法值和溢出/下溢检查。

## bytea hex 输入

OID17文本参数允许\\x后的字节对之间及首尾出现空格、tab、LF、CR；逐对解码，半字节内部空白、奇数及非法数字返回22023。空hex保留空bytes，00/ff及前后零不做字符转换。UTF8/NUL检查仍在解码前执行（22021），NULL独立处理。

## bytea escape 文本输入

OID 17 的文本 Bind 保留 hex 解码，并支持普通 UTF-8 字节、双反斜线和三位八进制 000–377。非法或截断 escape 返回 22P02；原始 NUL/非法 UTF-8 仍返回 22021，解码后的任意字节允许进入既有 bytea 桥接。`pg_extended_test::pg_scalar_26` 通过真实 TCP 验证值、NULL、描述、拒绝写入和恢复，以及 MySQL 隔离。

## bytea 真实客户端差分诊断

PostgreSQL 18.6/libpq 实测要求非法 hex 字符及不完整字节对返回 `22023`；OID 17 文本 Bind 的 hex 分支保留这一诊断，escape 格式错误仍为 `22P02`，无效 UTF-8/NUL 文本仍为 `22021`。`pg_scalar_28` 独立双端差分与 `pg_scalar_25` 协议拒绝/恢复回归覆盖此局部修正。

## UUID参数输入

Parse支持OID2950，未声明/0参数沿现有共享AST cast上下文推断。parameter复用PgUuid严格词法解析，输入先检查UTF-8/NUL（22021），非法词法22P02；规范文本经BinaryParam的既有tp253载荷交给真实prepared参数桥接，UUID身份由2950上下文保留（含NULL），不拼SQL字符串回填。binary_parameter要求恰16字节，短载荷08P01、多余字节22P03，按文本从左到右的字节序调用PgUuid::from_bytes，不做GUID换序或version/variant筛选。

独立测试pg_extended_test.rs::pg05_uuid_input_text_codec与pg05_uuid_input_parse_bind_execute_and_recovery覆盖词法、两种输入、显式/推断参数身份、NULL与2950/16/-1描述、原字节序、任意位模式、Bind诊断与Sync恢复。UUID二进制结果及参考端四方向差分属于后续任务。

## JSON参数输入（任务8）

普通Parse接受OID114；显式OID或明确json cast推断保留身份。文本与binary Bind共用PgJson严格词法校验和既有tp253 BinaryParam载荷；binary是原始JSON文本，没有jsonb版本前缀。共享prepared OID上下文负责JSON身份与typed NULL，不拼SQL字符串，不使用MySQL BinaryJSON，不重序列化。空白、成员顺序/重复、大数、数字拼写及Unicode转义原样保留，SQL NULL与JSON null独立。非法JSON及01前缀返回22P02，非法UTF-8/原始NUL返回22021；错误沿既有Bind失败/Sync恢复路径处理。

独立pg_extended_test.rs的pg05_json_input_text_binary_codec覆盖两种解码器原文及诊断；pg05_json_input_parse_bind_execute_and_recovery使用真实CreateAnalyzeSession/ConcreteSessionDriver与TCP，覆盖声明114、cast推断、OID0、114/-1/-1描述、NULL、重绑定、Bind拒绝与Sync恢复及Close。pg05_json_input_shared_prepared_identity_and_rebinding补充真实共享session的定型、NULL与重绑定断言。binary_parameter的crate可见性仅用于同目录独立codec测试；BinaryParam结构及共享执行桥接均无需修改。二进制结果与参考端四方向差分属于后续任务。


## JSONB参数与版本边界（任务14）

普通Parse接受OID3802，声明、cast推断及OID0沿现有共享prepared上下文保留JSONB身份和typed NULL。text参数调用PgJsonb::from_bytes严格校验并规范化，再复用tp253文本桥接；binary参数先读取一字节版本，空载荷08P01，非01版本XX000，仅01无文档22P02，01后的文档复用text路径。未知版本诊断经精确PostgreSQL18.6（server_version_num=180006）独立临时数据库采集，00/02/ff均XX000。文档语法22P02、编码22021、转义u0000为22P05、数字超域22003按原阶段保留，不统一包装22P03；孤立代理项经共享解析器修正为参考22P02。SQL NULL在取版本前处理，不要求版本字节。

独立pg05_jsonb_input测试覆盖两种输入的规范值、精确数字、Unicode和NULL；真实TCP覆盖声明/推断参数3802及结果3802/-1/-1、重绑定、Bind拒绝、Sync恢复、载荷声明截断和Close清理。二进制结果及四方向差分由后续任务承担，本任务不开放新的结果codec。conn_stmt无需修改，复用既有BinaryParam和OID定型。


## int4 数组共享求值与协议

普通 Parse 开放声明1007；text Bind 委派规范 PgArray::from_text_with 与既有 int4 标量输入，binary Bind 委派 from_binary_with 并要求每个整数元素恰好消费4字节。BinaryParam 复用完整规范数组文本，参数1007上下文保留身份与 typed NULL。尚未开放的其他数组OID继续0A000。


## text 数组逐族接入（任务25）

普通Parse新增1009，text数组Bind逐元素委派parameter(25)，binary数组Bind逐元素委派binary_parameter(25)，复用既有UTF8/NUL诊断。规范PgArray保留shape/lower和两层NULL，完全成功后才返回BinaryParam。int4_array_text保留原接口并委派array_text；未开放其他族仍0A000。


## bool 数组逐族接入（任务26）

普通Bind开放数组OID1000，text元素委派parameter(16)，binary元素将完整元素载荷委派binary_parameter(16)校验值与长度，成功后标记已消费。作为闭环必需局部纠错，通用标量OID16的binary入口接受任一单字节并规范化非零为01，空载荷08P01、多字节22P03；目录专用入口未改。TCP和真实libpq覆盖声明/推断/零OID、格式四组合、NULL、恢复和清理。

## 2026-10-27 int2数组局部接线

OID1005文本/二进制Bind逐元素委派parameter(21)/binary_parameter(21)；现有标量21分支区分短载荷08P01和多余22P03，不复制标量语义。

行为与验证见 `docs/postgresql-pg05-array-int2-evidence.md`；不开放完整跨族cast、存储或数组运算。


## 2026-10-27 int8数组局部接线

普通Parse和text/binary Bind注册1016。逐元素委派parameter(20)/binary_parameter(20)，保留i64精度与既有PgArray的形状/下界/两层NULL；OID20短载荷0–7字节08P01，超过8字节22P03，按真实18.6纠正共享入口诊断。

行为与验证见 `docs/postgresql-pg05-array-int8-evidence.md`；不新增完整跨族cast、存储或数组运算。


## 任务29 bytea数组局部接线

普通Parse/Bind白名单注册1001；array_text逐元素复用parameter(17)，binary数组逐元素复用binary_parameter(17)，全部成功后才发布。标量parameter(17)改调下层同一提取codec，原hex/escape与诊断保留。


## numeric数组开放（任务30）

普通标量Bind OID1700调用共享numeric binary接收；OID1231数组text/binary分派逐元素调用同一标量入口。

当前精确18.6差分与验证记录见 `docs/postgresql-pg05-array-numeric-evidence.md`；只承诺记录范围。


## float4数组与原始binary32位型（任务31）

parameter/binary_parameter为OID700保存原始binary32字节，通过内部BinaryParam标签0xf0传递；OID1021通过0xf1携带已验证规范数组binary。float4_binary_input是共享标量接收codec，短于4字节08P01、超过4字节22P03。数组文本/二进制元素复用该标量路径，任一非法元素拒绝整值。

验证与兼容性范围见 `docs/postgresql-pg05-array-float4-evidence.md`；测试位于对应独立测试文件，生产文件不嵌入单元测试。


## float8数组与binary64位型（任务32）

OID1022 Parse白名单与text/binary Bind注册，元素调用已共享标量codec，私有Float8参数标签保留原位型。

行为证据与本机验证限制见 `docs/postgresql-pg05-array-float8-evidence.md`；测试保持在同目录独立测试文件。

## 2026-10-28 varchar数组局部接线

1015参数text/binary codec委托已有1043标量codec；头元素OID固定1043，UTF8/NUL/NULL沿标量契约，维度与元素长度沿通用容器。独立测试见同目录既有测试文件及server的pg05_array_varchar前缀；真实18.6证据见`docs/postgresql-pg05-array-varchar-evidence.md`。

## 2026-10-28 bpchar数组局部接线

本文件在既有数组消费者中注册Bpchar元素1042/数组1014身份，保留字符长度typmod、shape/lower及两层NULL。标量转换复用pg_text_cast，按字符截断并补U+0020，不复制标量实现。独立测试见对应测试文件的pg05_array_bpchar前缀；18.6差分及限制见`docs/postgresql-pg05-array-bpchar-evidence.md`。
