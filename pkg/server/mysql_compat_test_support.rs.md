# [`pkg/server/mysql_compat_test_support.rs`](./mysql_compat_test_support.rs)

## 文件定位

该文件属于 `astersql-server` crate，但不是线上请求路径的一部分：`pkg/server/lib.rs:105-107` 只在 `#[cfg(test)]` 下把它声明为 `mysql_compat_test_support` 模块。它是多组 Rust MySQL 线协议测试共享的测试基础设施，通过真实回环 TCP 监听器启动生产 `Server`、安装生产 `ConcreteSessionDriver`，再用文件内的最小同步客户端直接编解码 MySQL 包。这样，上层测试验证的是实际监听、握手、会话与协议输出，而不是绕过网络层的桩。

crate 边界由 `pkg/server/Cargo.toml` 确认：包名为 `astersql-server`、库入口是 `lib.rs`，且 `autotests = false`，因此这些测试模块由库入口显式装配。启动夹具直接依赖同 crate 的 `conn`、`runtime`、`server` 模块以及工作区依赖 `astersql-session`；文件本身没有 feature 条件，唯一条件编译边界来自 `lib.rs` 的 `#[cfg(test)]` 和文件末尾对独立测试文件的 `#[cfg(test)]` 声明。

## 核心职责

1. `MysqlCompatServer` 组装内存会话域、生产连接运行时和真实 MySQL/状态监听器，并在夹具销毁时关闭服务器。
2. `MysqlTestClient` 完成 4.1 握手、能力协商、认证以及常用命令的同步收发，严格检查包序号。
3. `Packet`、`Handshake`、`OkPacket`、`ErrPacket`、`ColumnDefinition`、`TextResultSet`、`PreparedResponse`、`WireResponse` 等类型把原始字节转换为便于断言的结构。
4. 读取路径处理 MySQL 16 MiB 边界上的连续帧、文本结果集、二进制结果集、传统 EOF 与 `CLIENT_DEPRECATE_EOF` 两种结束方式、多结果状态位，以及预处理语句元数据。
5. `PacketCursor` 和一组解析函数集中实施边界检查；`put_lenenc_int`/`put_lenenc_bytes` 为测试构造握手属性和预处理参数提供长度编码。

这个模块刻意只实现当前兼容测试需要的客户端子集。它不是通用 MySQL 客户端：不实现 TLS、压缩协议、认证挑战计算或大负载写入拆帧；压缩与畸形包测试在 `pkg/server/mysql_protocol_stress_test.rs` 中直接操作原始 `TcpStream`。

## 主要符号

- 能力与协议常量：公开到 crate 内的 `CLIENT_CONNECT_WITH_DB`、`CLIENT_PROTOCOL_41`、`CLIENT_SECURE_CONNECTION`、`CLIENT_MULTI_STATEMENTS`、`CLIENT_MULTI_RESULTS`、`CLIENT_PLUGIN_AUTH`、`CLIENT_CONNECT_ATTRS`、`CLIENT_DEPRECATE_EOF`；内部常量 `DEFAULT_CLIENT_CAPABILITIES`、`COM_QUERY`、`COM_PING`、`SERVER_MORE_RESULTS_EXISTS` 和 30 秒 `IO_TIMEOUT`。默认客户端能力固定包含 4.1、secure-connection 和 plugin-auth。
- `CompatibilityDriver: ServerDriver`：`name()` 固定返回 `"tidb"`，只满足测试服务器所需的驱动标识。
- `CompatibilityDomain: Domain`：提供确定性的 `server_id() == 7` 和 `start_timestamp() == 1`，避免测试依赖外部域服务。
- `MysqlCompatServer`：保存 `Arc<Server>` 与操作系统分配的 `mysql_addr`。公开的 crate 内入口为 `start`、`mysql_addr`、`connect_root`、`connect_root_with_attrs`、`connect_user`；`Drop::drop` 调用 `Server::close`。
- `Handshake`：记录协议版本、服务端版本、连接 ID、服务端能力、最终协商能力、字符集和状态。
- `Packet`：保存逻辑包的起始序号和重组后的负载。多帧重组后仍保留首帧序号。
- `OkPacket`、`ErrPacket`、`ColumnDefinition`：分别表示 OK、ERR 与列定义包中测试关注的字段。
- `TextValue` 与 `TextResultSet`：把 SQL `NULL` 表示为 `TextValue::Null`，其余文本值或已格式化的二进制值统一保存为字节串；结果集同时保留列、行、结束包状态与告警数。
- `PreparedResponse`：保存 `COM_STMT_PREPARE` 返回的 statement ID、参数数和结果列元数据。
- `WireResponse`：顶层响应联合类型，仅有 `Ok`、`Err`、`ResultSet` 三类。
- `MysqlTestClient`：持有独占 `TcpStream` 和已协商 `Handshake`。命令入口包括 `ping`、`query`、`query_all`、`command`、`prepare`、`execute_prepared`、`send_long_data`、`reset_prepared`、`close_prepared`、`field_list`，并暴露 `write_command`、`read_packet` 供特殊协议测试使用。
- 编解码函数：`read_packet_from`/`read_frame_from`/`write_packet_to` 处理帧；`parse_handshake`、`parse_simple_response`、`parse_ok_packet`、`parse_err_packet`、`parse_column_definition`、`parse_text_row`、`parse_binary_row`、`parse_binary_temporal`、`parse_terminator` 处理协议结构；`response_status` 与 `is_result_set_terminator` 提供分支判定。
- `PacketCursor<'a>`：借用单个包负载，以 `offset` 维护读取位置，并提供小端整数、NUL 结尾串、长度编码整数/字节串和“必须恰好消费完”的读取原语。

除上述 `pub(crate)` 项外，驱动、域、游标及大部分解析函数均为模块私有。文件末尾通过 `#[path = "mysql_compat_test_support_test.rs"] mod tests` 将自身单元测试保持在独立文件中。

## 执行流程

### 启动与连接

1. `MysqlCompatServer::start` 调用 `astersql_session::runtime::CreateAnalyzeSession` 创建内存会话域。
2. 它以 `BootstrapAuthMode::InsecureRootOnly` 创建 `ConcreteSessionDriver::new_for_test`，并用同一域创建 `CanonicalConnectionDomain`。
3. `Server::new_test` 使用 `127.0.0.1:0` 创建 MySQL 监听器，并开启同样绑定随机回环端口的状态监听器；随后 `set_connection_runtime` 安装会话与连接域，`run` 启动真实监听线程。
4. `listener_addr` 取得实际 MySQL 地址；若监听器没有公开地址，启动失败而不返回半初始化夹具。
5. `connect_root*`/`connect_user` 进入 `MysqlTestClient::connect_as`。客户端建立 TCP 连接并设置读写超时，读取序号 0 的握手，解析服务端能力；请求能力若有任一位未被服务端声明，则在发送认证响应前失败。
6. 客户端按 HandshakeResponse41 布局写入能力、64 MiB 最大包、字符集 45、用户名、空认证数据、可选数据库、`mysql_native_password` 和可选连接属性，再要求认证响应序号为 2。只有 OK 响应才构造客户端；ERR 被转换为含错误码与消息的 `String`。

### 普通命令与结果集

1. `write_command` 为命令字加负载并以序号 0 写出；`read_response` 从预期序号 1 开始。
2. `read_response_at` 根据首字节把 `0x00`/`0xff` 交给简单响应解析，其余负载解释为结果集列数。
3. `read_result_set` 按列数读取并解析列定义。未协商 `CLIENT_DEPRECATE_EOF` 时，它要求列元数据之后存在旧式 EOF；新式路径省略该中间 EOF。
4. 后续包在短 `0xfe` 结束包出现前逐行由 `parse_text_row` 解码。结束包提供结果状态和告警数。
5. `query_all` 根据每个响应的 `SERVER_MORE_RESULTS_EXISTS` 状态持续读取，并把下一个响应的预期序号接在前一个响应之后；ERR 的状态按 0 处理，因此会结束循环。

### 预处理语句与二进制行

1. `prepare` 发送 `COM_STMT_PREPARE`，先识别 ERR，否则严格解析 PREPARE_OK 的 statement ID、列数、参数数和保留字段。
2. 它依序消费参数定义及其结束包、列定义及其结束包；所有阶段共享递增且可回绕的包序号。
3. `execute_prepared` 发送调用方已经编码好的 `COM_STMT_EXECUTE` 负载；OK/ERR 直接解析，其余进入 `read_binary_result_set`。
4. 二进制行先验证 `0x00` 行头，再按 `(列数 + 9) / 8` 读取带两个保留位偏移的 NULL bitmap。整数依据列的 UNSIGNED 标志格式化，float/double 转为十进制字符串，DATE/DATETIME/TIMESTAMP/TIME 由 `parse_binary_temporal` 格式化，其他类型按长度编码字节读取。
5. `send_long_data`、`reset_prepared`、`close_prepared` 分别构造 `COM_STMT_SEND_LONG_DATA`、`COM_STMT_RESET`、`COM_STMT_CLOSE` 所需的小端 statement/parameter 字段；关闭语句命令按协议不读取响应。

### 帧与长度编码

`read_frame_from` 读取 3 字节小端长度、1 字节序号和精确长度的负载。`read_packet_from` 在帧长等于 `0x00ff_ffff` 时继续读取，校验每个续帧序号，直到遇到短帧并拼成一个逻辑包。`write_packet_to` 只接受不超过该上限的单帧负载。`put_lenenc_int` 覆盖单字节、`0xfc`+2 字节、`0xfd`+3 字节、`0xfe`+8 字节四个区间；`PacketCursor::read_lenenc_int` 实施逆操作并拒绝 NULL/非法前缀。

## 数据与状态

- 服务器状态由 `Arc<Server>` 持有，保证监听线程运行期间底层服务器仍存活；地址在启动后复制为 `SocketAddr`。
- 每个 `MysqlTestClient` 独占一个阻塞式 `TcpStream`。文件没有连接池、异步 runtime 或客户端内部锁，同一客户端需要通过 `&mut self` 串行发命令和读响应。
- `Handshake.server_capabilities` 是服务端公告值，`negotiated_capabilities` 是默认能力、调用方追加能力和可选数据库隐式能力的并集；解析 OK/ERR/EOF 及结果集时始终使用后者。
- 包序号是命令内状态：客户端命令固定从 0 发出，普通响应从 1 开始，认证固定检查 0/1/2，多结果和元数据阶段逐包递增并用 `wrapping_add` 符合一字节回绕规则。
- `PacketCursor` 的 `offset` 是解析局部状态；几乎每个完整结构最后调用 `finish`，把多余尾字节也视为协议错误，而不仅防止越界。
- 结果值统一为 `Vec<u8>` 会保留文本协议的原始字节；二进制数值和时间则被格式化为可直接与 SQL 文本期望比较的字节。此抽象方便测试，但不保留二进制数值的原始编码类型。
- 文件没有可变全局状态。常量能力位和超时时长在所有夹具实例间共享；实际 SQL/事务/认证状态位于生产 `Server` 与会话域中。

## 依赖与调用关系

上游调用者均是 `astersql-server` 的测试模块：

- `pkg/server/mysql_compat_test_support_test.rs` 直接验证长度编码往返、多帧读取、真实握手、PING、错误包、SELECT 结果集及新旧 EOF。
- `pkg/server/mysql_protocol_compat_test.rs` 使用连接属性、初始数据库、多语句/多结果、会话重置与连接命令路径。
- `pkg/server/mysql_prepared_protocol_test.rs` 使用 `prepare`、`execute_prepared`、long-data/reset/close 和 `put_lenenc_bytes` 验证二进制值。
- `pkg/server/mysql_metadata_protocol_test.rs`、`mysql_catalog_protocol_test.rs` 和 `mysql_type_protocol_test.rs` 使用列定义与结果集检查 catalog、JDBC 元数据、类型、flags、charset、decimals 等线协议字段。
- `pkg/server/mysql_error_protocol_test.rs` 使用结构化 ERR 响应检查 errno、SQLSTATE 和消息。
- `pkg/server/mysql_protocol_stress_test.rs` 复用服务器夹具和健康检查客户端，同时用原始 TCP 覆盖本客户端未实现的压缩及畸形帧路径。

主要下游调用链是：

```text
协议测试
  -> MysqlCompatServer::start
     -> CreateAnalyzeSession
     -> ConcreteSessionDriver::new_for_test / CanonicalConnectionDomain::new
     -> Server::new_test -> set_connection_runtime -> run -> listener_addr
  -> connect_root* / connect_user
     -> MysqlTestClient::connect_as
        -> read_packet_from -> parse_handshake
        -> write_packet_to -> parse_simple_response
  -> query / prepare / execute_prepared / field_list / query_all
     -> read_response_at
        -> parse_simple_response 或 read_result_set/read_binary_result_set
        -> PacketCursor 与各 parse_* 函数
```

RustCodeGraph 精确 `callees` 结果确认：`connect_as` 调用本文件的 `put_lenenc_bytes`、`read_packet_from`、`write_packet_to`、`parse_handshake`、`parse_simple_response`；`read_response_at` 调用 `read_packet`、`read_result_set`、`parse_simple_response`；`parse_binary_row` 调用 `parse_binary_temporal` 和 `PacketCursor` 的各读取方法。图索引对常见方法名 `start` 的查询存在大量同名结果，因此上游边以 `lib.rs` 的模块装配和上述测试文件的显式导入/调用交叉确认，不把索引的模糊“used by”文件列表当作真实调用边。

## 错误处理与边界

- 所有可恢复的夹具、I/O 和解析失败统一返回 `Result<_, String>`，并在错误文字中加入阶段（连接、设置超时、包头/负载、启动监听器等）。这是测试辅助 API，未保留具体 `io::Error` 类型或错误源链。
- 能力协商采用“调用方请求必须是服务端能力子集”的强约束；初始数据库会自动加入 `CLIENT_CONNECT_WITH_DB`。这可在测试开始前暴露错误的能力假设。
- 包序号在握手、认证、首响应、列/行/多结果间逐个验证；续帧序号不连续会立即失败。
- `PacketCursor::read_exact` 用 `checked_add` 防止游标加法溢出，并同时拒绝截断；`finish` 拒绝尾随字节。NUL 字符串缺少终止符、长度编码 NULL 被当作整数、`0xff` 长度前缀、NULL 列定义字符串、错误的列定义固定长度都会产生明确错误。
- `parse_err_packet` 只有协商 `CLIENT_PROTOCOL_41` 才要求 `#` 与五字节 SQLSTATE；消息及多个协议字符串使用 `String::from_utf8_lossy`，因此非法 UTF-8 会被替换而非保留或报错。
- `is_result_set_terminator` 仅把“首字节 `0xfe` 且负载短于 9 字节”视为结束包，避免把长度编码整数前缀误识别为 EOF。
- 二进制 TIME 只接受长度 8/12，日期时间只接受 4/7/11；零长度编码有明确零值文本。未知列类型走长度编码字节分支，意味着新增固定宽度 MySQL 类型若未显式接入会被错误解释，必须配套扩展。
- 写路径只支持单帧且明确拒绝大于 `0x00ff_ffff` 的负载；读路径支持多帧重组。它也不支持压缩帧，不能把此测试客户端的能力误认为服务器能力边界。
- 握手解析只读取测试关心的固定字段，不验证完整认证插件数据或包尾；连接认证只发送空密码的 native-password 格式，配合 `InsecureRootOnly`。因此它不适合验证真实密码插件流程。

## 并发与资源生命周期

`MysqlCompatServer::start` 启动生产服务器的后台监听线程，但本文件的客户端逻辑是同步阻塞的。`TcpStream` 设置 30 秒读写超时，防止协议错误导致测试永久挂起。`MysqlCompatServer` 的 `Drop` 无条件调用 `server.close()`，使夹具离开作用域时关闭监听线程、释放随机端口和后台资源；测试应让客户端生命周期不超过服务器夹具。

`Arc<Server>` 仅用于匹配生产服务器的共享所有权模型，本文件没有显式锁。并发连接隔离由生产 `Server` 实现；`pkg/server/mysql_protocol_stress_test.rs` 的畸形连接循环会在每次破坏性输入后创建新客户端并执行 PING/SELECT，以验证单连接失败不污染监听器。单个 `MysqlTestClient` 的方法使用 `&mut self`，自然禁止无同步的并发读写；若未来需要多线程共享客户端，不能简单包裹当前读写步骤而忽略命令与响应的序列原子性。

独立单元测试 `packet_reader_reassembles_go_multi_packet_payloads` 自建临时 `TcpListener`，写线程发送最大帧和续帧，主线程读取后 `join`，证明重组完成前资源不会提前释放。它测试的是本模块读端，不替代生产服务器线程关闭行为的专门断言。

## 与 Go 版本的对应关系

Go 目录没有 `pkg/server/mysql_compat_test_support.go`，因此本文件不是逐文件一一翻译，而是 Rust 兼容性测试为验证 Go/TiDB 既有线协议语义新增的共享客户端与夹具。可核对的语义基线如下：

- `pkg/server/server.go` 的 `defaultCapability` 包含本文件用到的 connect-with-db、4.1、secure-connection、multi-statements、multi-results、connect-attrs、plugin-auth、deprecate-EOF 等位；位值来自 `pkg/parser/mysql/const.go`。
- `pkg/server/conn.go:1697` 的 Go `writeOkWith` 把 affected rows、last insert ID、状态、告警以及非空消息写入 OK 包。尤其消息虽与旧协议手册描述不同，实际使用长度编码字符串；Rust `parse_ok_packet` 明确按此行为解析，`ok_packet_decodes_go_length_encoded_info` 锁定该兼容点。
- `pkg/server/conn.go` 的 `writeError` 在 `ClientProtocol41` 下写 `#` 和 SQLSTATE，`writeEOF` 在 `ClientDeprecateEOF` 下以 EOF header 的 OK 包替代旧 EOF；Rust 的 `parse_err_packet` 与 `parse_terminator` 对应这两条分支。
- `pkg/server/internal/packetio.go` 的 `PacketIO::ReadPacket` 在负载达到 `mysql.MaxPayloadLen` 时持续拼接后续包，`WritePacket` 对大包拆帧并递增序号；`pkg/server/internal/packetio_test.go` 覆盖该 Go 行为。Rust `read_packet_from` 对齐读端重组，独立测试复现最大帧加短续帧；Rust 测试客户端写端则有意限制为单帧。
- `pkg/server/conn_stmt.go` 在未启用 `ClientDeprecateEOF` 时为预处理参数/列元数据发送旧式 EOF，本文件 `prepare` 对相同能力分支消费结束包。

语义差异必须保留在认知中：Go 文件实现服务端，Rust 文件实现测试客户端；Go `PacketIO` 支持写端多帧及生产配置，Rust 客户端只覆盖断言所需子集。后续扩展应以生产 Rust 协议实现与这些 Go 基线共同核对，不能仅在辅助客户端中“模拟通过”。

## 扩展指南

- 新增顶层命令：优先在 `MysqlTestClient` 增加表达命令语义的方法，复用 `write_command`/`read_response_at`；命令若无响应（如 `COM_STMT_CLOSE`）不要调用通用读取。同步在独立的 `pkg/server/*_test.rs` 增加真实监听器回归测试。
- 新增响应类型：扩展 `WireResponse` 及 `read_response_at` 的首包判定，并审查 `response_status` 与 `query_all` 的多结果终止规则。改变枚举会影响所有 `WireResponse` 模式匹配测试。
- 新增二进制列类型：修改 `parse_binary_row` 的 type-code 分支，明确 signed/unsigned、定长/长度编码和文本规范化方式；在 `mysql_type_protocol_test.rs` 或 `mysql_prepared_protocol_test.rs` 增加 NULL、边界值和 metadata flags 测试。不要让固定宽度类型落入默认长度编码分支。
- 新增时间编码：修改 `parse_binary_temporal` 时必须覆盖零值、合法长度、负 TIME、跨天小时和微秒；保持 DATE、DATETIME/TIMESTAMP 与 TIME 的长度集合不变量。
- 新增能力位：定义 `pub(crate)` 常量后，要同时更新握手响应布局、服务端公告检查和相关协议测试。能力影响 EOF、SQLSTATE、连接属性或认证字段时，解析与编码两侧必须一起审查。
- 支持大包写入或压缩：应在 `write_packet_to`/帧抽象层实现并验证序号与终止空帧规则，而不是在各命令方法手写。由于这会扩大测试客户端职责，还需与 `pkg/server/internal/packetio.go`、`packetio_test.go` 和生产 Rust packet I/O 对照。
- 修改启动夹具：保持随机回环端口、生产 `Server`/`ConcreteSessionDriver` 和 `Drop::close`，避免引入固定端口、外部 TiKV 或泄漏线程。认证策略改变时要明确 `InsecureRootOnly` 仍是否符合测试目的。
- 测试组织：本文件的白盒单元测试继续放在 `pkg/server/mysql_compat_test_support_test.rs`，协议场景放在相应独立 `mysql_*_test.rs`，不要把测试逻辑内嵌回源文件。

兼容风险主要是错误接受宽松/错误包布局或把 Go 服务端特例误当标准协议；性能风险较低，因为该模块只在测试构建中使用，但无上限地收集结果行或重组远端宣称的大包仍可能放大测试内存。扩展解析器时应优先保持严格序号、长度和尾字节检查。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被索引为 1,125 行、96 个符号。
- RustCodeGraph 源码与图查询：`node --file pkg/server/mysql_compat_test_support.rs --offset 1 --limit 500`、`node --file ... --offset 500 --limit 700`；`query MysqlCompatServer`、`query MysqlTestClient`、`query parse_handshake`、`query parse_binary_row`、`query PacketCursor`、`query put_lenenc_int`；`callees connect_as`、`callees read_response_at`、`callees parse_binary_row`。同名 `start` 的全局图结果存在歧义，未作为单独结论使用。
- crate 与装配证据：`pkg/server/Cargo.toml`、`pkg/server/lib.rs:103-119`。目标包没有 `doc.go`；本任务未发现额外包级契约文件。
- Rust 测试证据：`pkg/server/mysql_compat_test_support_test.rs`；直接调用方 `pkg/server/mysql_protocol_compat_test.rs`、`mysql_protocol_stress_test.rs`、`mysql_prepared_protocol_test.rs`、`mysql_metadata_protocol_test.rs`、`mysql_catalog_protocol_test.rs`、`mysql_type_protocol_test.rs`、`mysql_error_protocol_test.rs`。
- Go 对照证据：`pkg/server/server.go`（默认能力）、`pkg/parser/mysql/const.go`（命令/能力/最大负载常量）、`pkg/server/conn.go`（包收发、OK/ERR/EOF）、`pkg/server/conn_stmt.go`（预处理元数据 EOF）、`pkg/server/internal/packetio.go` 与 `pkg/server/internal/packetio_test.go`（多帧读写）。不存在同名 Go 支撑文件，文档已明确这一点。
- 人工复核结论：该文件存在的理由是让多组 Rust 兼容测试共享一个严格、真实 TCP、生产服务器驱动的最小客户端；运行链、协议分支、资源关闭和安全扩展位置均可由上述符号与文件反向定位。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；交付验证仅执行任务规定的 11 章节结构检查和文档差异自审。
