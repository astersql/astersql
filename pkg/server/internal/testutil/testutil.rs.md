# `pkg/server/internal/testutil/testutil.rs`

## 文件定位

本文件是 `astersql-server-internal-testutil` crate 的实现文件，提供 server 测试代码复用的两个轻量工具：以内存字节模拟连接读侧的 `BytesConn`，以及从 TCP 套接字地址提取端口的 `get_port_from_tcp_addr`。同目录 [`lib.rs`](./lib.rs) 以 `pub mod testutil` 装入本文件并通过 `pub use testutil::*` 再导出公开符号；[`Cargo.toml`](./Cargo.toml) 将其定义为无第三方依赖的工作区库，并以 `package.metadata.porting.go-package = "pkg/server/internal/testutil"` 记录 Go 来源。

它位于服务端测试基础设施边界，而不是线上连接处理主链：`BytesConn` 让协议/缓冲逻辑可在无真实 socket 的情况下消费确定字节；端口辅助函数被 [`../../tests/servertestkit/testkit.rs`](../../tests/servertestkit/testkit.rs) 用于把测试服务器实际绑定的 SQL 与 status 地址回填给测试客户端。工作区根 `Cargo.toml` 还以 `facade_server_internal_testutil` 登记此 crate；多个 server 测试 crate 声明了它，但精确 Rust 引用表明当前目标文件的跨 crate 直接调用集中在 `servertestkit` 的端口提取流程。

## 核心职责

- `BytesConn` 保存一个 `Cursor<Vec<u8>>`，实现 `std::io::Read`，使读取按当前位置顺序消费构造时提供的字节，并在耗尽后按标准 `Read` 语义返回 `Ok(0)`。
- `BytesConn` 实现 `std::io::Write`，但与 Go 测试桩一致地丢弃输入并返回 `Ok(0)`；`flush`、`close` 和三种 deadline 操作都是成功空操作，地址查询恒为 `None`。
- `From<Vec<u8>>` 把字节向量便捷转换为 `BytesConn`，行为等同于 `BytesConn::new`。
- `get_port_from_tcp_addr` 对 `SocketAddr` 调用 `port()`，同时支持 IPv4 和 IPv6 地址。

这些能力只为测试构造确定输入和读取真实监听端口，不建立连接、不发送数据、不实施超时，也不模拟完整的异步或同步 socket 类型。

## 主要符号

- `pub struct BytesConn { buffer: Cursor<Vec<u8>> }`：唯一字段私有，外部只能通过构造器/转换和 I/O trait 操作数据；派生 `Debug` 与 `Default`，默认值是空字节游标。
- `pub fn BytesConn::new(buffer: Vec<u8>) -> Self`：取得向量所有权，用 `Cursor` 从偏移 0 包装它。
- `pub fn close(&mut self) -> io::Result<()>`：不改变状态，恒返回 `Ok(())`。
- `pub fn local_addr(&self) -> Option<SocketAddr>` 与 `remote_addr`：都恒返回 `None`，明确表示桩没有网络端点。
- `pub fn set_deadline(&mut self, SystemTime) -> io::Result<()>`、`set_read_deadline`、`set_write_deadline`：接受但忽略时间，恒成功。
- `impl From<Vec<u8>> for BytesConn`：调用 `BytesConn::new`，不复制向量内容。
- `impl Read for BytesConn`：把读取直接委托给内部 `Cursor<Vec<u8>>::read`，因此保留标准游标推进、部分读取和 EOF 行为。
- `impl Write for BytesConn`：`write` 丢弃切片并返回 `Ok(0)`，`flush` 恒成功。这个返回值是刻意复刻 Go 桩，不表示输入已经写入。
- `pub fn get_port_from_tcp_addr(addr: SocketAddr) -> u16`：按值接收地址并返回其 16 位端口。

文件没有常量、枚举、自定义 trait、条件编译项或模块级可变状态。

## 执行流程

内存读路径从 `BytesConn::new(bytes)` 或 `BytesConn::from(bytes)` 开始。向量被移动进 `Cursor`，初始位置为 0；每次 `Read::read` 都把输出切片交给游标，复制当前偏移起的至多 `output.len()` 个字节并推进位置。连续读取因此拼接为原输入，字节耗尽后再次读取返回 0。独立测试 `bytes_conn_reads_buffer_in_order_and_reports_eof` 用 `"packet"` 验证了先读 3 字节、再读剩余 3 字节、最后 EOF 的顺序。

写与连接管理路径不接触 `buffer`：`write` 返回 0，`flush`、`close` 和 deadline setter 返回成功，地址 getter 返回 `None`。`bytes_conn_write_and_connection_metadata_match_go_stub` 在执行这些操作后仍能完整读出原始 `"input"`，证明写操作不会追加、覆盖或消费读缓冲。

端口路径是纯投影：测试服务器在 [`../../tests/servertestkit/testkit.rs`](../../tests/servertestkit/testkit.rs) 完成 listener 和 status listener 启动后取得两个 `SocketAddr`，分别调用 `get_port_from_tcp_addr`，再将结果写入测试客户端的 `port` 与 `status_port`。同目录迁移测试分别用 IPv4 `127.0.0.1:4000` 与 IPv6 `[::1]:10080` 锁定两种地址族的结果。

## 数据与状态

唯一持久实例状态是 `BytesConn.buffer` 中拥有所有权的 `Vec<u8>` 和 `Cursor` 保存的当前位置。读取会推进位置；写入及连接管理方法不改变位置或字节内容。字段私有意味着调用方不能替换缓冲、倒带或读取当前 position；若测试需要重新播放输入，应新建实例，而不是假设 `close` 或 deadline 操作会重置它。

`Default` 产生空缓冲，因此第一次读取即返回 EOF。构造器按值接收 `Vec<u8>`，不会为输入另做一份逻辑副本；读取时才把数据复制进调用者提供的输出切片。`get_port_from_tcp_addr` 没有状态，只从 `SocketAddr` 的端口字段返回 `u16`。

本文件没有全局缓存、计数器、环境变量、锁、原子量、事务或持久化副作用。传入 deadline 的 `SystemTime` 被忽略，不会保存在对象中。

## 依赖与调用关系

下游依赖全部来自标准库：`std::io::{Cursor, Read, Write}` 提供内存游标与同步 I/O trait，`io::Result` 维持类似连接 API 的返回形状；`std::net::SocketAddr` 提供 IPv4/IPv6 地址和 `port()`；`std::time::SystemTime` 仅用于 deadline 方法签名。目标 crate 的 `Cargo.toml` 没有 `[dependencies]`。

文件内调用边只有 `From<Vec<u8>>::from -> BytesConn::new`，`BytesConn::new -> Cursor::new`，`Read::read -> Cursor::read`，以及 `get_port_from_tcp_addr -> SocketAddr::port`。RustCodeGraph 对 `new` 确认了 `from` 调用边，对端口函数确认了两个上游：迁移测试 `tcp_port_is_extracted_for_ipv4_and_ipv6` 与 `servertestkit::create_tidb_test_suite_with_cfg`。

上游 Rust 证据分为两类：[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 直接覆盖 `BytesConn` 的全部公开行为和两种地址族；[`../../tests/servertestkit/testkit.rs`](../../tests/servertestkit/testkit.rs) 在真实测试服务器启动流程中使用端口函数。虽然 `pkg/server/Cargo.toml` 及 handler、commontest、standby 等测试 crate 的 Cargo manifest 也声明了该依赖，当前精确 `.rs` 引用未显示它们直接使用 `BytesConn` 或端口函数，不能只凭依赖声明推断实际调用。

## 错误处理与边界

`Read::read` 原样传播 `Cursor` 的 `io::Result`；对内存 `Vec<u8>` 的正常读取不会产生 socket、超时或系统调用错误。其余返回 `io::Result` 的桩方法当前恒成功，因此无法用本类型测试关闭失败、deadline 失败、短写之外的写错误或真实网络错误路径。

最重要的写边界是 `write` 返回 `Ok(0)`：这与 Go 对照桩完全一致，但对非空输入而言代表“未写入任何字节”。依赖“全部写完”的 `write_all` 会把该结果转成 `WriteZero` 错误；调用者也不能把成功的 `Result` 误解为输入已被接受。若测试要收集输出或模拟短写后的继续写，本类型并不适合，应另建有明确写缓冲/故障策略的测试替身。

`local_addr`/`remote_addr` 返回 `None`，而不是一个虚构地址；需要地址元数据的代码必须显式处理缺失或使用其他替身。deadline 方法不执行时间检查，超过给定时间后读取仍会按内存缓冲正常完成。`close` 也不使实例失效，关闭后仍可继续读取，这只是接口形状兼容，不是真实连接生命周期。

端口函数的参数已经是有效的 `SocketAddr`，不存在 Go 版类型断言失败；返回类型 `u16` 覆盖合法 TCP/UDP 端口的完整范围，包括 0。函数不区分地址来自 TCP listener、UDP 或手工构造，名称表达的是调用意图而非运行时验证。

## 并发与资源生命周期

`BytesConn` 在构造时取得字节向量所有权，销毁时由 Rust 自动释放 `Cursor` 和向量；没有文件描述符或 socket，所以 `close` 不负责释放资源。每次读取需要 `&mut self`，安全 Rust 会阻止同一实例在未同步的情况下同时推进游标。不同实例彼此独立，可以在不同线程各自使用；若调用方主动共享同一实例，则同步和访问顺序应由调用方明确管理。

deadline 不会创建计时器或后台任务，`flush` 不触发系统调用，端口函数也不借用或保存地址。整个文件没有异步任务、线程、通道、锁、回调、事务或需要等待的清理过程。

读取生命周期由游标位置决定而非连接状态：数据只在 `read` 时被消费，`write`、`flush`、`close` 和地址/deadline 方法都不会缩短缓冲生命周期。新功能若引入可观测关闭状态或写缓冲，必须明确它与现有“所有连接管理操作都是空操作”测试契约的兼容关系。

## 与 Go 版本的对应关系

直接对照文件是 [`testutil.go`](./testutil.go)。Go `BytesConn` 嵌入 `bytes.Buffer`，Rust 则用私有 `Cursor<Vec<u8>>`；两者都按顺序读取、耗尽后报告 EOF，并让 `Write` 返回 `(0, nil)`。Go 为满足 `net.Conn` 实现 `Read`、`Write`、`Close`、`LocalAddr`、`RemoteAddr` 与三个 deadline 方法；Rust 没有对应的统一连接 trait，本文件分别实现标准 `Read`/`Write`，并把其余方法作为固有方法保留。

地址语义保持一致：Go 两个 getter 返回 nil，Rust 返回 `None`；deadline 和 close 都忽略输入/状态并成功。一个表现差异是 Go 的 `bytes.Buffer.Read` 在空缓冲通常返回 `io.EOF`，而 Rust `Read` 约定通过 `Ok(0)` 表示 EOF；调用 `read_to_end` 等上层 API 时两者都正常结束。Rust 额外提供了 `new`、`From<Vec<u8>>`、`Default` 和 `Debug`，用于适应 Rust 的构造与测试习惯。

Go `GetPortFromTCPAddr(net.Addr) uint` 在内部把接口值断言为 `*net.TCPAddr`，错误地址类型会 panic，且结果扩宽为平台字宽的 `uint`。Rust `get_port_from_tcp_addr(SocketAddr) -> u16` 在类型层排除了非 IP socket 地址，直接返回端口的协议宽度；IPv4 和 IPv6 由同一枚举覆盖。两端测试服务器调用都在监听建立后读取实际绑定端口，语义一致。

Go 侧 `BytesConn` 被 `server_test.go`、`conn_test.go` 和 `internal/packetio_test.go` 等大量测试用作 buffered connection 的底层 `net.Conn`；Rust 侧当前精确引用只在本 crate 的迁移测试中出现。因此这些 Go 调用说明该替身的原始测试用途，但不能写成 Rust PacketIO 已使用它的证据。

## 扩展指南

新增连接替身能力时应先判断是否必须保持 Go `net.Conn` 桩语义。若只是补齐当前 API，修改 `BytesConn` 的相应方法，并在独立文件 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 增加回归测试；不要把测试内嵌进 `testutil.rs`。涉及 Go 行为变更时同步核对 `testutil.go` 以及实际使用它的 `server_test.go`、`conn_test.go`、`internal/packetio_test.go`。

若需要记录写入数据，不应悄悄把现有 `write` 从 0 改为输入长度：现有语义由 Go 对照和迁移测试锁定，改变后会影响短写/`write_all` 行为。更安全的扩展是新增名称明确的独立替身或构造模式，并分别测试写缓冲、部分写、错误注入与读取互不干扰。类似地，模拟关闭、地址或 deadline 时应定义状态转换和错误优先级，而不是让当前空操作逐步产生隐式副作用。

扩展端口辅助函数时优先保持窄类型输入和 `u16` 返回值。若确实要接受更抽象的地址，应返回可判定的 `Result`/`Option`，避免复刻 Go 的运行时断言 panic；同时覆盖 IPv4、IPv6、端口 0 和非 IP 地址。真实测试服务器接线变化需同步验证 `servertestkit::create_tidb_test_suite_with_cfg` 的 SQL/status 两个回填点。

性能风险很低，主要是大输入向量的持有和每次读取复制；应继续按值接管 `Vec`，避免无必要克隆。兼容风险集中在 EOF 表达、`write` 返回 0、关闭后仍可读及 deadline 无效这四个刻意的桩行为。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/server/internal/testutil` 列出目标实现、crate 入口、Go 对照和迁移测试；`node --file pkg/server/internal/testutil/testutil.rs --offset 1 --limit 400` 读取完整 102 行及 14 个符号；`node` 核对 `BytesConn`、所有方法和端口函数；`new` 的 trail 显示 `from` 调用边，端口函数的 callers 显示迁移测试与 `create_tidb_test_suite_with_cfg`。对 `BytesConn` 的 callers 查询返回空结果，因此又以精确 Rust 引用搜索核验使用点。
- 已读 Rust 源码与 crate 边界：`pkg/server/internal/testutil/testutil.rs`、`lib.rs`、`Cargo.toml`，以及工作区根 `Cargo.toml`、`pkg/server/Cargo.toml` 和 `pkg/server/tests/servertestkit/Cargo.toml` 的依赖声明。目标目录不存在 `doc.go`。
- 已读独立 Rust 测试：`pkg/server/internal/testutil/migration_aster_unit_test.rs`，覆盖顺序读取/EOF、写与连接元数据空操作、写不破坏输入，以及 IPv4/IPv6 端口。
- 已读 Rust 直接调用：`pkg/server/tests/servertestkit/testkit.rs::create_tidb_test_suite_with_cfg`，确认监听启动后回填 SQL 与 status 端口的生产化测试流程。
- 已读 Go 对照与引用：`pkg/server/internal/testutil/testutil.go`；精确搜索确认 `BytesConn` 用于 `pkg/server/server_test.go`、`conn_test.go`、`internal/packetio_test.go`，`GetPortFromTCPAddr` 用于 `pkg/server/tests/servertestkit/testkit.go`、commontest、standby、TLS 和 handler 测试。
- 本任务为纯文档分析，按计划未运行 Cargo。交付验证使用任务指定命令确认目标文件存在且恰有 11 个固定二级标题，并人工复核唯一产物、符号名称、调用边、Go/Rust 差异和独立测试建议。
