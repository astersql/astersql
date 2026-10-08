# `pkg/server/pg_protocol.rs`

## 文件定位

`pg_protocol.rs` 属于 `astersql-server` crate。crate 根在 `pkg/server/lib.rs`，其中以公开模块 `pub mod pg_protocol` 挂载本文件；同一根模块在测试配置下通过 `#[path = "pg_protocol_test.rs"]` 挂载独立测试文件。`pkg/server/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `package.metadata.porting.go-package = "pkg/server"` 共同确认了这一归属。

本文件位于独立 PostgreSQL 监听器收到连接后的最前端，只解释 PostgreSQL 3.0/3.2 StartupMessage 的长度、版本和 NUL 分隔参数。它不创建会话、不认证、不处理 SQL，也不复用 MySQL PacketIO。生产侧 `pkg/server/pg_conn.rs` 的 `PgService::negotiate` 先用 `read_initial` 收取初始帧并分流 SSL、GSS 和取消请求，只有普通启动帧才调用这里的 `parse_startup`。

## 核心职责

- 定义受支持的协议版本：`PROTOCOL_VERSION_30` 表示 3.0，`PROTOCOL_VERSION` 表示 3.2。
- 用 `MIN_STARTUP_LENGTH = 9` 和公开上限 `MAX_STARTUP_LENGTH = 10_000` 约束完整启动帧；长度字段本身计入总长度。
- 将一个完整字节切片解析成 `StartupMessage { protocol_version, parameters }`，并保留未知参数，交由后续协商层决定是否支持。
- 通过 `read_startup` 从任意 `Read` 恰好读取一帧：先读四字节长度，再按已校验的长度读取余下内容，不吞掉下一帧。
- 用 `StartupError` 将长度、版本、参数 framing、UTF-8、重复键和底层读取错误分类。

职责边界很重要：`parse_startup` 只验证结构。`user` 是否存在、参数名和值是否受支持，以及认证和选择数据库，均由 `pkg/server/pg_conn.rs::PgService::negotiate` 完成。

## 主要符号

- `PROTOCOL_VERSION_30: u32 = 3 << 16`：PostgreSQL 3.0 的网络序版本值 `196608`。
- `PROTOCOL_VERSION: u32 = (3 << 16) | 2`：AsterSQL 同时接受的 3.2 版本值 `196610`。
- `MAX_STARTUP_LENGTH: usize = 10_000`：公开给连接层复用的启动帧分配上限；`pg_conn.rs::read_initial` 也使用它。
- `StartupMessage`：解析结果。`protocol_version` 保留原始版本值；`parameters` 使用 `BTreeMap<String, String>`，提供确定性键顺序并拥有输入字符串。
- `StartupError`：公开错误枚举。`Display` 给协商层提供可发送给客户端的诊断文本；同时实现 `std::error::Error`。
- `checked_length(u32) -> Result<usize, StartupError>`：内部统一长度守卫，只接受闭区间 `9..=10_000`。
- `parse_startup(&[u8]) -> Result<StartupMessage, StartupError>`：核心纯解析入口；要求输入恰好是一整个帧且包含四字节长度字段。
- `take_string(&mut &[u8]) -> Result<&str, StartupError>`：内部游标式 C 字符串读取器；寻找下一个 NUL、校验 UTF-8，并推进剩余切片。
- `read_startup(&mut impl Read) -> Result<StartupMessage, StartupError>`：有界流读取辅助入口。当前仓库中的直接调用均位于 `pg_protocol_test.rs`，生产连接链使用 `read_initial` 加 `parse_startup`，不要将两条读取路径误写为同一入口。

## 执行流程

`parse_startup` 的流程如下：

1. 用 `packet.get(..4)` 取得长度头；不足四字节返回 `LengthMismatch`。
2. 将头按大端 `u32` 解码并交给 `checked_length`；小于 9 或大于 10,000 返回 `InvalidLength`。
3. 要求声明长度与 `packet.len()` 完全相等，否则返回 `LengthMismatch`。因此该函数既不接受截断帧，也不接受尾随的下一帧。
4. 从偏移 `4..8` 解码协议版本，只接受 3.0 或 3.2；其他值返回 `UnsupportedVersion`。
5. 从偏移 8 开始反复用 `take_string` 读取“名称、值”对。空名称是参数区结束标志，且结束标志之后必须没有任何字节。
6. 每个名称和值必须以 NUL 结束并且是合法 UTF-8。名称首次插入 `BTreeMap`；重复名称返回 `DuplicateParameter`。
7. 仅当终止 NUL 正好结束帧时，返回包含版本和所有参数的 `StartupMessage`。

`read_startup` 先 `read_exact` 四字节头，在长度通过相同守卫后分配精确大小的 `Vec<u8>`，再 `read_exact` 余下 `length - 4` 字节并调用 `parse_startup`。连续帧场景下，一次调用只前进一帧长度，测试用两个拼接帧验证了这一点。

生产请求的完整局部链路是：`PgService::negotiate` 调用 `pg_conn.rs::read_initial` 收取一帧，检查第 5—8 字节的请求码；SSL/GSS 和 CancelRequest 在连接层处理，普通帧才进入 `parse_startup`。解析成功后连接层继续检查安全传输策略、必需的 `user`、启动参数白名单、认证、数据库和时区。

## 数据与状态

本文件没有全局可变状态。四个协议/长度常量在编译期固定，函数调用之间不共享状态。

解析时的状态由三个局部对象组成：指向尚未消费字节的 `remaining: &[u8]`、逐步填充的 `BTreeMap`、以及最终拥有字符串的 `StartupMessage`。`take_string` 返回借用输入的 `&str`，只有键值通过结构校验并插入结果时才 `to_owned`。`parse_startup` 在长度和版本校验之前不会创建参数映射或字符串所有权；`read_startup` 则在长度通过上限检查后分配整帧缓冲区。

参数映射的关键不变量是键唯一。空值是合法结构值，例如测试接受 `application_name\0\0\0`；空名称只用于终止参数列表，不能作为普通键。未知键也能通过本层结构解析，是否允许由 `PgService::negotiate` 判断。

## 依赖与调用关系

本文件只依赖标准库：`BTreeMap` 保存参数，`fmt` 实现错误文本，`io::Read` 和 `io::ErrorKind` 提供流读取抽象。`pkg/server/Cargo.toml` 没有为该文件声明专属第三方依赖或 feature gate，`lib.rs` 也无条件公开该模块。

已核实的上游关系：

- `pkg/server/pg_conn.rs::PgService::negotiate` 在排除特殊协商请求后调用 `parse_startup`，这是当前生产调用点。
- `pkg/server/pg_conn.rs::read_initial` 复用 `MAX_STARTUP_LENGTH`，先完成 TCP 帧读取；它允许最短 8 字节以便容纳 SSL/GSS 特殊请求，而普通 StartupMessage 随后还会被本文件的最短 9 字节规则约束。
- `pkg/server/pg_conn.rs::cancel_key_length` 使用 `PROTOCOL_VERSION_30` 决定后续取消密钥宽度，说明解析出的版本会影响连接生命周期中的后续协议格式。
- `pkg/server/pg_protocol_test.rs` 直接覆盖 `parse_startup`、`read_startup`、版本常量、长度上限和错误分类。

已核实的下游关系：RustCodeGraph 的 `callees` 结果确认 `parse_startup` 调用本文件的 `checked_length` 与 `take_string`，并引用两个版本常量、构造 `StartupMessage`；同一结果确认 `read_startup` 调用 `checked_length` 与 `parse_startup`。图工具还混入了同名通用符号的噪声，因此上游生产调用点以仓库精确搜索和 `pg_conn.rs` 源码复核为准。

## 错误处理与边界

`StartupError` 的边界含义如下：

- `InvalidLength(n)`：声明长度不在 `9..=10_000`。这在分配前拒绝过大帧。
- `LengthMismatch`：无法读取完整四字节头，或切片实际长度不等于声明长度。
- `UnsupportedVersion(v)`：版本不是 3.0/3.2；显示文本按高低 16 位输出主次版本。
- `InvalidParameters`：找不到字符串终止 NUL、名称后缺值，或参数列表终止 NUL 后仍有字节。
- `InvalidUtf8`：名称或值不是 UTF-8。
- `DuplicateParameter(name)`：同一名称出现两次；不会静默采用首值或末值。
- `Io(kind)`：`read_startup` 的头或正文 `read_exact` 失败，只保留稳定的 `io::ErrorKind`，不保留操作系统错误正文。

本层不验证空 `user`、具体参数白名单或安全策略。生产调用点对解析错误发送 PostgreSQL `FATAL`、SQLSTATE `0A000` 后结束协商；直接使用 `read_startup` 的调用者则必须自行设置读取截止时间，并在 framing/版本错误后关闭连接。

当前实现以 `length as usize` 转换网络长度。现有上限判断确保只有不超过 10,000 的值会继续，因此通过检查后的长度适合分配。`read_startup` 不在内部设置超时，阻塞策略属于调用者。

## 并发与资源生命周期

解析函数本身无锁、无线程、无任务、无通道，也不持有连接资源；输入借用只持续到函数返回，返回值拥有所有参数字符串。因此同一函数可由多个连接线程并发调用而不共享可变数据。

`read_startup` 对传入的可变 reader 做同步、顺序的 `read_exact`。它先消费四字节头；若长度非法便立即返回且不消费正文。长度合法后若正文截断，会返回 `Io`，reader 已被部分消费，调用者不能假定可从同一流恢复 framing。成功时恰好消费声明的一帧，后续字节留给下一次读取。

TCP 连接、10 秒读写超时、每连接 worker、连接关闭和认证上下文均由 `pkg/server/pg_conn.rs::PgService` 管理，而不是本文件。这里的 10,000 字节上限是主要资源保护：先校验再分配，避免按不受信任的 32 位长度申请内存。

## 与 Go 版本的对应关系

`pkg/server/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/server`，但仓库中不存在同路径 `pkg/server/pg_protocol.go`，在 `pkg/server/**/*.go` 中也没有 PostgreSQL StartupMessage、3.0/3.2 版本常量或等价解析入口。现有 Go `pkg/server/conn.go` 和 `pkg/server/internal/parse/parse.go` 中的 SSLRequest/握手逻辑属于 MySQL 协议，不是本文件的 Go 对照实现。

因此，本文件不是对某个同名 Go 文件的逐函数复刻，而是 AsterSQL Rust 服务端新增的独立 PostgreSQL 边界。可对齐的依据是 Rust 自身的生产连接层 `pg_conn.rs` 和独立测试 `pg_protocol_test.rs`。未来若 Go 侧加入对应协议，需逐项比较长度范围、受支持版本、UTF-8 规则、重复键策略、终止 NUL 规则及特殊请求分层，不能以现有 MySQL 握手逻辑替代。

## 扩展指南

- 新增协议版本时，修改版本常量和 `parse_startup` 的版本白名单，并同步检查 `pg_conn.rs::cancel_key_length` 等依赖版本的后续格式；在 `pg_protocol_test.rs::startup_packet_bounds` 增加接受与拒绝矩阵。
- 调整最大帧大小时，必须保持 `MAX_STARTUP_LENGTH` 在 `parse_startup`、`read_startup` 与 `pg_conn.rs::read_initial` 间一致，并覆盖恰好等于上限及上限加一的用例。提高上限会增加每个未认证连接的内存与阻塞读取风险。
- 改变参数 framing、编码或重复键策略时，优先修改 `take_string`/`parse_startup`，并在独立的 `pg_protocol_test.rs` 添加回归测试；不要把测试内嵌进生产源文件。
- 新增“允许哪些启动参数”时应修改 `PgService::negotiate` 的白名单及对应连接测试，而不是让结构解析器承担会话策略。保留未知参数是当前的分层契约。
- 新增 SSL、GSS 或 CancelRequest 支持时应接入 `pg_conn.rs::PgService::negotiate` 的特殊请求分流；这些 8 字节或取消请求不是普通 StartupMessage，不能放宽本文件最短长度或版本检查来兼容。
- 若计划让生产链直接使用 `read_startup`，必须先解决特殊请求最短长度差异、错误到 PostgreSQL 响应的映射，以及 socket 超时/失败后关闭责任；当前它只是通用读取辅助接口和测试覆盖面的一部分。

兼容性风险集中在客户端可接受版本和参数 framing；性能风险集中在未认证阶段的分配上限与阻塞读取；正确性风险集中在终止 NUL、精确长度、重复键和生产层/结构层职责混淆。

## 验证依据

- `pkg/server/pg_protocol.rs`：完整读取并核对常量、`StartupMessage`、`StartupError`、`checked_length`、`parse_startup`、`take_string`、`read_startup`。
- `pkg/server/pg_protocol_test.rs`：`startup_packet_bounds` 覆盖版本、长度、截断、终止符、UTF-8、重复键和最大帧；`startup_raw_tcp_client_v1_requests_32` 覆盖真实 TCP 上的 3.2/3.0；`startup_reader_bounds` 覆盖分配前拒绝和连续两帧的精确消费。
- `pkg/server/pg_conn.rs`：核对生产调用链 `PgService::negotiate -> read_initial -> parse_startup`，以及 SSL/GSS/CancelRequest 分流、参数策略、认证边界和 `PROTOCOL_VERSION_30` 对取消密钥宽度的影响。
- `pkg/server/lib.rs`：核对公开模块挂载及独立测试文件挂载。
- `pkg/server/Cargo.toml`：核对 crate 名、根文件、Go 包映射、依赖和无专属 feature 的事实。
- Go 对照检索：仓库精确搜索确认没有 `pkg/server/pg_protocol.go` 或 Go 版 PostgreSQL StartupMessage 解析；读到的 `conn.go`/`internal/parse/parse.go` SSLRequest 是 MySQL 握手路径。
- RustCodeGraph：`status` 显示项目索引可用；精确 `query` 定位到 `StartupMessage`（第 17 行）、`parse_startup`（第 67 行）和 `read_startup`（第 112 行）；`callees pg_protocol.rs::parse_startup` 确认 `checked_length`、`take_string`、版本常量与结果构造关系。`callers` 查询在本地未能在 90 秒内返回，因此生产调用点另以 `rg` 和 `pg_conn.rs` 源码核实。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证恰好存在 11 个固定二级章节，并人工复核文档能够回答文件为何存在、如何运行及如何安全扩展。
