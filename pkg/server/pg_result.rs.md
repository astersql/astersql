# `pkg/server/pg_result.rs` 逻辑说明

## 文件定位

`pg_result.rs` 属于 `astersql-server` crate，由 `pkg/server/lib.rs` 以 `pub mod pg_result` 装配。它位于共享 SQL 执行结果与 PostgreSQL 前端协议之间：上游接收 `pkg/server/conn.rs` 定义的 `QueryResult`、`ColumnInfo`、`NativeType` 和 `Value`，下游生成 PostgreSQL 消息体，并由 `pkg/server/pg_conn.rs::write_message` 写入 `TcpStream`。

该文件同时承担三类边界工作：对少量 PostgreSQL 客户端探测 SQL 做安全适配、在执行前识别并限制简单查询命令、把引擎原生结果元数据和值编码成 PostgreSQL RowDescription/DataRow/CommandComplete。它不执行 SQL，不管理会话，也不负责消息长度外层 framing。`pkg/server/Cargo.toml` 表明它属于 `astersql-server`；本文件直接使用该 crate 已声明的 `astersql-parser`、`astersql-parser-ast`、`astersql-parser-mysql` 和 `chrono`。

## 核心职责

1. `adapt_session_query`/`adapt_query` 在 PostgreSQL 会话边界适配两类兼容查询：启动时间探测，以及投影位置上的 `current_catalog`。适配依赖词法边界或 AST 字段偏移，不做全局字符串替换。
2. `command` 在执行前解析完整 SQL，只接受单条、可映射到 PostgreSQL完成标签的语句；空输入返回 `None`，多语句、`REPLACE` 和未列出的语句返回 SQLSTATE 与消息。
3. `CatalogColumnType` 与 `pg_type` 把引擎原生类型和目录提供者的专用类型码映射为 PostgreSQL OID/定长宽度。映射只看元数据，禁止根据单元格内容猜类型。
4. `value_text`、`value_binary`、`binary_array` 和 `bytea` 将 `Value` 编为文本或大端二进制字段，同时拒绝 PostgreSQL 无法无损表达的引擎值。
5. `encode_formats` 先在内存中完成整个响应的校验和编码，再依次产生 `T`（RowDescription）、`D`（DataRow）和 `C`（CommandComplete）；`write_result` 仅负责把已编码消息交给 socket 写层。

## 主要符号

- `adapt_session_query(sql, startup_epoch_micros)`：先调用内部 `startup_time_query` 识别 `round(extract(epoch from pg_postmaster_start_time() at time zone 'UTC'))`，命中时以服务启动微秒时间生成十进制常量，再交给 `adapt_query`；未命中时直接返回借用或改写后的 SQL。
- `startup_time_query`：使用嵌套 `token` 逐 token、大小写不敏感匹配，并检查标识符边界。它保留别名或尾随内容给规范解析器验证；无后缀或只有分号时补 `AS round`。正数微秒按 PostgreSQL `ROUND(numeric)` 的半数远离零规则取整。
- `adapt_query`：仅在输入含大小写不敏感的 `current_catalog` 候选时解析 AST。`CatalogProjection` 只改写无 schema/table 限定、且确实位于 `SelectStmt.Fields` 投影中的同名列；无显式别名时写为 `DATABASE() AS current_catalog`，已有别名时仅写为 `DATABASE()`。
- `command`：返回 `Option<&'static str>` 完成标签。支持 SELECT/集合查询、INSERT、UPDATE、DELETE、常用 DDL、BEGIN/COMMIT/ROLLBACK 和 SET；语法错误为 `42601`，不支持的协议语义为 `0A000`。
- `CatalogColumnType`：为 PG 目录提供者保留 230..=241 的 crate 内类型码，覆盖内部 `"char"`、OID/regclass、数组、vector、bpchar 和 aclitem[]。`from_code` 恢复枚举，`wire_type` 给出 PostgreSQL OID 与 `typlen`。
- `pg_type`：优先读取 `NativeType`，后备才读 `ColumnInfo`。布尔标志优先；整数按有符号性扩宽，`uint64` 映射 numeric，二进制 charset 映射 bytea，字符串映射 text，时间映射无时区 date/time/timestamp；未知类型报错。
- `value_text`：NULL 返回 `None`；数值和浮点生成 PostgreSQL 文本形式；bytea 输出 `\\x` 十六进制；布尔归一化为 `t`/`f`；零日期、非法日期、超过 23 小时的 MySQL TIME、NUL 文本均被拒绝。
- `value_binary`：生成 PostgreSQL 网络字节序的 bool、整数、OID、浮点、日期、时间、timestamp 和目录数组/vector。日期/时间以 PostgreSQL 2000-01-01 epoch 编码，保持引擎微秒精度。
- `binary_array`：解析受控的一维目录数组或空白分隔 vector，支持引号、反斜杠转义和未加引号的 `NULL`；生成维数、null 标志、元素 OID、长度/下界及逐元素长度。嵌套数组和畸形分隔符被拒绝。
- `encode`：文本格式快捷入口，相当于 `encode_formats(result, command, &[])`。
- `encode_formats`：核心纯编码函数；校验格式码、列/原生类型/行宽一致性，构建元数据和各行，最后根据命令与影响行数生成完成标签。
- `write_result`：遍历 `encode` 的结果，通过 `pg_conn::write_message` 写 socket。由于先调用 `encode`，值或元数据错误不会留下半个 RowDescription/DataRow 流。

文件没有 trait、全局可变状态或条件编译项；公开面均为 `pub(crate)`，其余辅助函数为模块私有。

## 执行流程

简单查询主链见 `pkg/server/pg_conn.rs`：收到 `Q` 后，连接层先尝试 `SessionQuery` 与目录查询；普通 SQL 再依次经过 `pg_name::adapt`、`adapt_session_query` 和 `command`。`command` 返回 `None` 时写 EmptyQueryResponse（`I`）；返回标签时调用规范会话执行，单一 `QueryResult` 交给 `write_result`。多结果由连接层单独拒绝。

扩展协议主链见 `pkg/server/pg_extended.rs`：Parse 阶段对普通语句调用 `adapt_session_query` 与 `command`，保存命令标签和 prepare 元数据；Describe 用 `description_formats` 构造空行 `QueryResult` 并调用 `encode_formats` 取得 RowDescription；Execute 用 portal 的逐列格式码再次调用 `encode_formats`，并核对执行期 RowDescription 与 Describe 缓存完全一致。

编码内部顺序如下：先验证格式数组；若有列，验证 `native_types.len() == columns.len()` 并一次性计算全部 `(oid, typlen)`；生成列名、未知来源表/属性、OID、长度、typmod=-1、格式码；逐行验证宽度并按格式调用 `value_text` 或 `value_binary`；无列时不产生 `T`/`D`；最后始终产生 `C`。SELECT 的标签含返回行数，INSERT 为 `INSERT 0 <affected_rows>`，UPDATE/DELETE 含影响行数，其余原样使用命令标签。

## 数据与状态

核心输入是拥有所有权的 `QueryResult`：`columns` 给出协议可见列名和 MySQL 风格后备类型，`native_types` 保存未被 MySQL wire 截断的原始类型，`rows` 保存 `Value`，`state.affected_rows` 参与完成标签。`pg_type` 的设计不变量是元数据决定 OID，NULL 或某一行的实际值不能改变列类型；`pkg/server/pg_types_test.rs::missing_original_metadata_is_explicitly_unsupported` 验证缺失原生元数据会显式失败。

`CatalogColumnType` 的 230..=241 只在 PG 目录提供者与本编码器之间流通；`pkg/server/pg_catalog.rs` 为目录列选择这些码，`pg_result.rs` 再映射到真实 PostgreSQL OID。注释明确这些码避开原生 MySQL JSON 245 与 DECIMAL 246，不能扩散到共享引擎或 MySQL 协议映射。

输出 `Vec<(u8, Vec<u8>)>` 完全拥有消息体。`encode_formats` 不缓存、不修改输入，也不读 `response_lifecycle`/`result_set`；生命周期完成和写耗时由 `pg_conn.rs`、`pg_extended.rs` 的调用层处理。

## 依赖与调用关系

直接上游：

- `pkg/server/pg_conn.rs` 调用 `adapt_session_query`、`command`、`write_result`，构成简单查询路径。
- `pkg/server/pg_extended.rs` 调用 `adapt_session_query`、`command`、`encode_formats`，构成 Parse/Describe/Execute 路径。
- `pkg/server/pg_catalog.rs`、`pkg/server/pg_oid.rs` 使用 `CatalogColumnType` 为目录结果携带专用类型信息。
- 测试 `pg_query_test.rs`、`pg_types_test.rs`、`pg_extended_test.rs`、`pg_datagrip_test.rs` 直接调用编码或类型枚举。

直接下游：

- `astersql_parser::New().ParseSQL` 与 `astersql_parser_ast::Visitor` 提供规范解析和安全 AST 改写。
- `astersql_parser_mysql::type::{IsBooleanFlag, UnsignedFlag}` 解释引擎类型标志。
- `chrono::{NaiveDate, NaiveTime, NaiveDateTime, Timelike}` 实现 PostgreSQL 二进制时间 epoch/精度转换。
- `pkg/server/conn.rs` 提供协议无关结果桥，`pkg/server/pg_conn.rs::write_message` 添加 tag 与四字节长度并写 socket。

RustCodeGraph 对 `encode_formats` 的 callee 边确认了本文件的 `invalid`、`count`、`length`、`pg_type`、`value_text`、`value_binary`。索引对部分 crate 内 caller 未返回边，因此上游调用由精确引用搜索和上述调用文件源码补证。

## 错误处理与边界

SQL 适配/门禁使用 `Result<_, (&'static str, String)>`，让调用层直接形成 PostgreSQL SQLSTATE。解析错误映射为 `42601`；多语句、REPLACE、空 prepared statement或不支持命令等协议能力限制使用 `0A000`（其中部分由调用层产生）。改写只触及 AST 已确认的投影字段，字符串、注释、相似标识符、限定列与别名均保持。

编码错误统一为 `io::ErrorKind::InvalidData`，包括字段数超过 i16、字段长度超过 i32、格式码不是 0/1、格式数与列数不一致、原生元数据数不一致、行宽错误、列名/NUL 文本、未知类型、非法布尔或数值、无法表达的 MySQL 时间、畸形数组及精度/范围溢出。socket 写失败则保留底层 `io::Error`。

边界策略是“拒绝而非悄悄改变”：无符号 smallint/int 会提升到更宽 PG 整数，uint64 提升为 numeric；MySQL 零日期和 duration TIME 不强转；二进制 `Value::Bytes` 只有元数据为 bytea 才可编码；JSON 等未支持类型即使值为 NULL 也不会伪装为其他类型。`encode_formats` 在返回任何消息前构建并验证完整向量，但 `write_result` 开始写后若 socket 中途失败，已写网络字节无法回滚。

## 并发与资源生命周期

本文件自身无锁、线程、异步任务、通道或共享可变状态；所有函数除 `write_result` 外均为同步纯计算。`adapt_session_query` 接收由 `PgService` 启动时固定的 `startup_epoch_micros`，因此不同连接对启动时间探测得到一致值，但本文件不拥有该状态。

`write_result` 借用当前连接的 `TcpStream` 并顺序写消息；它不关闭、克隆或保存 socket。`QueryResult` 和编码缓冲仅在调用栈内存在。扩展协议把编码后的消息保存在 portal 中以支持分批 Execute，缓存归 `pg_extended.rs` 管理；响应 lifecycle 的 `finish` 也由调用者在编码/写入后执行，不属于本文件职责。

资源风险主要是内存放大：整个结果先复制到消息向量，文本/bytea/数组还会产生中间 `String`/`Vec`。这换取了编码失败前不发送部分元数据的协议一致性。新增无界类型或超大目录数组时应评估该峰值，而不是绕过 `length` 检查直接流式写入。

## 与 Go 版本的对应关系

`pkg/server/Cargo.toml` 的 porting 元数据把 crate 对齐到 Go `pkg/server`，但仓库搜索未发现 `pkg/server/pg_result.go`，也未发现 Go 侧 `current_catalog`、`pg_postmaster_start_time`、PG RowDescription/DataRow 或 PG wire 调用链。现有 Go `pkg/server` 主要是 TiDB/MySQL 协议实现，因此本文件是 Rust 服务端新增的 PostgreSQL 边界能力，不是某个同路径 Go 文件的逐函数复刻。

可对齐的共享语义来自 Rust `pkg/server/conn.rs` 对 Go server/session 结果的移植：`QueryResult`、`ColumnInfo`、`Value` 和 `SessionState.affected_rows` 仍承载规范执行结果；本文件只做 PG 表达。它刻意不改变 MySQL fallback，例如 `pg_types_test.rs::native_metadata_bridge_preserves_boolean_and_mysql_columns` 证明布尔差异保存在 `NativeType` 标志，而共享 MySQL 列和值保持不变。

因此后续 Go/Rust 对齐时，应把本文件视为协议适配层：执行、事务和影响行数必须继续来自共享引擎；不能为了 PostgreSQL 兼容在此实现第二套 SQL 语义，也不能把目录专用 230..=241 类型码泄漏到 Go/MySQL 路径。

## 扩展指南

- 新增普通 SQL 命令：先确认规范 parser AST 类型，再扩展 `command` 的标签映射，并在 `pg_query_test.rs` 添加支持/拒绝和多语句边界。若完成标签需要行数或 OID，需同步 `encode_formats` 的 completion 分支。
- 新增引擎类型：在 `pg_type` 添加基于 `NativeType` 的稳定 OID 映射，同时实现 `value_text` 和（若允许 binary portal）`value_binary`；在 `pg_types_test.rs` 同时验证 OID、文本值、二进制值、NULL、上下界和错误输入。不要根据首行值推断类型。
- 新增目录类型：只在 `CatalogColumnType::{from_code, wire_type}` 与 `pg_catalog.rs` 提供者之间分配不冲突的内部码；数组/vector 还需更新 `binary_array` 的元素 OID，并扩展 `pg_extended_test.rs` 的 Describe/Execute 与 array wire assertions。
- 新增查询适配：优先使用 AST 节点及源码 offset；若是 parser 前的特殊探测，必须像 `startup_time_query` 一样逐 token 验证边界，并在 `pg_query_test.rs` 覆盖字符串、注释、相似标识符、大小写、空白和尾随语句。
- 改动编码顺序或流式输出：必须保留“编码错误不会先发送 RowDescription”的不变量，并验证 socket 写失败、portal 重放和 response lifecycle。性能优化应先测量整结果缓冲的峰值，再设计有等价原子性的分段方案。
- PostgreSQL binary 时间与数组格式涉及 epoch、网络序、维度下界和 NULL 标志，兼容风险高；应使用真实客户端集成面 `pg_client_integration_test.rs` 或现有 `pg_extended_test.rs` 补回归，而不是把测试嵌入生产文件。

## 验证依据

- 源与装配：`pkg/server/pg_result.rs`（完整 645 行）、`pkg/server/lib.rs`（模块声明）、`pkg/server/Cargo.toml`（crate 边界与 parser/chrono 依赖）、`pkg/server/conn.rs`（输入类型与 lifecycle）、`pkg/server/pg_conn.rs` 和 `pkg/server/pg_extended.rs`（简单/扩展协议调用链）。目标包不存在 `pkg/server/doc.go`。
- RustCodeGraph：`status` 显示本仓库索引包含 Rust/Go 文件；`node --file pkg/server/pg_result.rs` 读取完整源码；精确 `query` 定位 `adapt_session_query`、`adapt_query`、`pg_type` 等符号；`callees encode_formats` 确认内部类型/值编码调用。caller 查询未给出边，已用精确源码引用补证。
- Rust 测试：`pkg/server/pg_query_test.rs` 覆盖 current_catalog 与启动探测的 SQL 边界、不可表达结果、空/多语句与 REPLACE；`pkg/server/pg_types_test.rs` 覆盖原生类型桥、unsigned/bytea/时间拒绝、元数据缺失、目录 char/acl、binary 数值/时间/vector 与格式错误；`pkg/server/pg_extended_test.rs` 覆盖目录数组 OID、文本/二进制数组和空结果元数据；`pkg/server/pg_datagrip_test.rs` 覆盖实际目录查询产生的 OID/vector/char/acl 行为。另有 `pkg/server/pg_client_integration_test.rs` 作为真实客户端协议面。
- Go 对照：对仓库 Go 源搜索 `current_catalog`、`pg_postmaster_start_time`、PostgreSQL/pgwire 及 RowDescription/DataRow，未发现本文件同路径或等价 PG 结果编码实现；只发现与该协议层无直接对应的通用 PostgreSQL 文字引用。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工复核唯一生产物、路径与上述证据一致。
