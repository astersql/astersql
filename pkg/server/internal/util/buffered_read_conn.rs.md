# `pkg/server/internal/util/buffered_read_conn.rs` 逻辑说明

## 文件定位

本文件属于 Cargo 包 `astersql-server-internal-util`；包边界由 `pkg/server/internal/util/Cargo.toml` 定义，根模块 `pkg/server/internal/util/lib.rs` 以私有模块 `buffered_read_conn` 装载它，再通过 `pub use buffered_read_conn::*` 导出其公共项。该文件只使用 Rust 标准库，没有直接使用本 crate 在 Cargo 中声明的 `config_crate`、`encoding_rs` 或 `http` 依赖，也没有 feature 条件分支。

它是 Go `pkg/server/internal/util/buffered_read_conn.go` 的 Rust 移植：在 TCP 读流前增加 16 KiB 缓冲、非消费式窥视和短超时探活。需要区分设计位置与当前接线事实：Go 类型已接入 `clientConn -> PacketIO` 的 MySQL 网络主链；仓库搜索及 RustCodeGraph 的文件使用关系只发现 Rust 实现由本 crate 的独立测试使用，尚未发现 Rust 生产代码构造或持有 `BufferedReadConn`。因此当前 Rust 文件是已实现并有测试的基础组件，而不是已经接入 Rust MySQL 协议主链的组件。

## 核心职责

- `DefaultReaderSize` 固定默认读缓冲容量为 16 KiB，与 Go 常量一致。
- `BufferedReadConn` 用 `BufReader<TcpStream>` 保存底层 TCP 流及预读数据，实现标准 `Read`，让调用者可以把包装器当作普通读取端。
- `Peek` 返回接下来指定长度的字节但保持逻辑上的“未消费”：它从 `BufReader` 取出字节后暂存到 `peeked`，后续 `Read` 优先返回这些字节。
- `IsAlive` 用 30 微秒读超时做保守探活，三态返回值为 `0`（确认 EOF）、`1`（空闲超时，视为仍存活）和 `-1`（无法可靠判断）。

本文件不解析 MySQL 包、不处理写路径、不拥有连接关闭协议，也不实现 Go `net.Conn` 的完整接口。其职责仅是 TCP 读取包装与探活。

## 主要符号

- `pub const DefaultReaderSize: usize`（`buffered_read_conn.rs:28`）：`16 * 1024`，同时是 `BufReader` 容量和单次 `Peek` 的硬上限。
- `pub struct BufferedReadConn`（`:32`）：公共类型，字段均为私有。
  - `reader: BufReader<TcpStream>`：实际读取端和底层缓冲。
  - `peeked: Vec<u8>`：已从 `reader` 消费、但尚未向包装器调用方消费的字节。
  - `liveness_lock: Mutex<()>`：避免探活逻辑并发修改读超时；见“并发与资源生命周期”中的 Rust 可达性限制。
- `pub fn NewBufferedReadConn(TcpStream) -> io::Result<BufferedReadConn>`（`:44`）：以默认容量构造包装器。当前函数体没有可失败操作，`Result` 为 API 形状预留，而不是现有失败点。
- `pub fn BufferedReadConn::Peek(&mut self, usize) -> io::Result<&[u8]>`（`:55`）：填满并借用返回 `peeked[..n]`。
- `pub fn BufferedReadConn::IsAlive(&mut self) -> i32`（`:80`）：执行三态 TCP 探活。
- `impl Read for BufferedReadConn`（`:114`）：先排空 `peeked`，否则委托 `BufReader::read`。一次调用若命中 `peeked`，允许只返回部分结果，不会在同一次调用中继续从 `reader` 拼满输出；这符合 `Read` 允许短读的契约，需由 `read_exact` 等上层循环补齐。

文件没有 trait 定义、类型别名、宏、条件编译项或模块级可变状态。

## 执行流程

构造流程（`NewBufferedReadConn`）如下：接收已建立的 `TcpStream`，使用 `BufReader::with_capacity(DefaultReaderSize, stream)` 包装；将 `peeked` 初始化为空；创建未加锁的 `Mutex<()>`；返回包装器。

窥视流程（`Peek`）如下：

1. 若 `n` 大于 `reader.capacity()`，立即返回 `InvalidInput`，避免无法满足的窥视请求。
2. 当 `peeked.len() < n` 时调用 `BufRead::fill_buf`。因为 TCP 可能分片到达，单次 `fill_buf` 不保证得到全部 `n` 字节，所以必须循环。
3. 若 `fill_buf` 返回空切片，转换为 `UnexpectedEof`。
4. 从当前缓冲最多复制仍缺少的字节到 `peeked`，再用 `reader.consume(copied)` 推进内部 `BufReader` 游标。
5. 返回 `&peeked[..n]`；数据虽然已从内部 `BufReader` 消费，但对包装器调用者仍未消费。

读取流程（`Read::read`）如下：空输出缓冲直接返回 `0`；若 `peeked` 非空，复制 `min(output.len(), peeked.len())` 个字节并从 `peeked` 头部删除；只有 `peeked` 为空时才读取 `reader`。因此 `Peek` 后的读取顺序不变且不丢数据。

探活流程（`IsAlive`）如下：尝试取得 `liveness_lock`；失败则返回 `-1`。随后在底层 `TcpStream` 设置 30 微秒读超时，设置失败也返回 `-1`。若 `peeked` 已有数据，则不再读取并返回 `-1`，防止探活影响已窥视数据；否则调用 `reader.fill_buf()`：空缓冲表示 EOF，返回 `0`；`TimedOut` 或 `WouldBlock` 表示超时期间没有数据但连接未证实关闭，返回 `1`；其他结果均返回 `-1`。最后尝试把读超时恢复为 `None`。

## 数据与状态

关键不变量是“逻辑未消费数据 = `peeked` 在前，`reader` 缓冲及底层流在后”。`Peek` 必须同时执行复制与 `reader.consume`，否则同一字节会在后续 `Read` 中重复；`Read` 必须先处理 `peeked`，否则顺序会颠倒或窥视数据会丢失。`pkg/server/internal/util/buffered_read_conn_test.rs::peek_waits_for_fragmented_bytes_without_consuming_them` 用分两次到达的 `a`、`b` 验证循环填充及随后读取仍得到 `ab`。

`peeked` 可以在多次调用间积累：较小的后续 `Peek` 只借用已有前缀，较大的后续 `Peek` 继续补足，但永远不能超过 `BufReader` 的 16 KiB 容量。`Read` 使用 `Vec::drain(..copied)` 删除前缀，语义直接但会移动剩余元素；若未来存在大量小步读取，需关注这一路径的复制和移动成本。

读超时属于底层 socket 状态，而非 `BufReader` 状态。`IsAlive` 临时设置该状态并在正常探活分支末尾恢复。`reader.fill_buf()` 若读到真实数据会把数据留在 `BufReader` 中，因此即使返回 `-1` 也不会丢失该数据。

## 依赖与调用关系

直接下游依赖均来自标准库：`std::net::TcpStream` 提供网络流和读超时；`std::io::BufReader`、`BufRead`、`Read` 提供缓冲、填充、消费和读取契约；`std::io::ErrorKind` 表达边界错误；`std::sync::Mutex` 保护探活临界区；`std::time::Duration` 构造 30 微秒超时。

Rust 模块入口是 `pkg/server/internal/util/lib.rs`，它公开再导出 `DefaultReaderSize`、`BufferedReadConn` 和 `NewBufferedReadConn`。RustCodeGraph 对目标文件的使用关系指出 `pkg/server/internal/util/migration_aster_unit_test.rs`，精确仓库搜索还确认同 crate 的 `buffered_read_conn_test.rs` 通过 `crate::NewBufferedReadConn` 使用它；没有发现生产 `.rs` 文件引用这些符号。虽然 `pkg/server/Cargo.toml`、若干 server 子 crate 和测试 crate 依赖整个 `astersql-server-internal-util` 包，这只能证明 crate 依赖，不能证明它们使用了本类型。

Go 侧真实主链为 `pkg/server/conn.go::clientConn.setConn -> util.NewBufferedReadConn -> internal.NewPacketIO/PacketIO.SetBufferedReadConn`；`clientConn.isConnectionAlive` 调用 `IsAlive`，并把 `-1` 这种未知状态按“仍存活”保守处理。`pkg/server/internal/packetio.go::PacketIO` 通过该连接读取 MySQL 包并设置读 deadline。这些调用边说明本抽象为何存在，但当前不能当作 Rust 已接线证据。

## 错误处理与边界

- `Peek(n)` 在 `n > 16 KiB` 时返回 `io::ErrorKind::InvalidInput`；这是 Rust 实现的显式边界。Go `bufio.Reader.Peek` 对超容量请求通常表现为缓冲区已满错误，错误种类并不一一对应。
- `fill_buf` 的底层 I/O 错误通过 `?` 原样传播；在尚未补足 `n` 字节时遇到空缓冲则返回 `UnexpectedEof`。
- `Peek(0)` 无需读取并返回空切片。空切片 `Read` 返回 `Ok(0)`，符合标准 `Read` 约定。
- `IsAlive` 刻意压缩错误信息为三态整数：锁冲突、设置超时失败、缓冲中已有数据、读到数据及非超时 I/O 错误都返回 `-1`。调用者不能据此诊断具体错误。
- 清除读超时的结果被忽略；若清除失败，底层流可能保留 30 微秒超时，后续正常读可能出现意外超时。这与 Go 中注释忽略 defer 清除错误的策略相近，是扩展或故障诊断时的重要风险点。
- `NewBufferedReadConn` 只接受 `TcpStream`，不能像 Go 构造器那样包装任意 `net.Conn`（例如 TLS 包装器或测试内存连接）。

## 并发与资源生命周期

`BufferedReadConn` 独占 `TcpStream`；本文件不克隆 socket、不生成后台任务，也没有显式 `Drop`。包装器离开作用域时，`BufReader`、`Mutex` 与底层 `TcpStream` 按 Rust 所有权顺序自动释放，最终关闭其持有的 socket handle。

所有会改变读取状态的公共操作都要求 `&mut self`。在安全 Rust 中，这已经排斥同一实例上的并发 `Peek`、`Read` 和 `IsAlive`；若外部放入互斥容器，则外层锁也会串行化调用。因此 `liveness_lock.try_lock()` 主要保留 Go `mu.TryLock()` 的探活语义，在当前 API 下通常不会自然遇到竞争。若未来把探活改成 `&self`、拆分 socket 句柄或引入内部可变性，才必须重新验证该锁能否同时保护 deadline 设置、探测读取和恢复。

探活期间 deadline 的生命周期是“加锁后设置，探测完成后清除”。但当前实现不是 RAII 清理：设置成功后代码现有分支都会走到清除语句，未来若在中间加入提前返回，则可能遗留 timeout。扩展时应保持单出口或引入恢复守卫。

## 与 Go 版本的对应关系

共同语义包括：默认缓冲均为 16 KiB；构造器创建缓冲读端与探活锁；普通读取从缓冲读端取数据；`Peek` 不应改变调用方随后观察到的字节序列；`IsAlive` 使用 30 微秒 deadline 和 `TryLock`，以 `0/1/-1` 表示死/活/未知，并在探测后恢复 deadline。

主要差异如下：

- Go `BufferedReadConn` 嵌入任意 `net.Conn`，自然转发写、关闭、地址和 deadline 等完整连接能力；Rust 类型只持有具体 `TcpStream` 并实现 `Read`，没有公开写、关闭或底层访问接口。
- Go `Peek` 直接借用 `bufio.Reader.Peek` 的缓冲；Rust 因 `BufRead::fill_buf` 契约不同，引入 `peeked: Vec<u8>`，循环等待 TCP 分片并在后续 `Read` 中回放。
- Go 构造器返回指针且不返回错误；Rust 返回拥有值的 `io::Result`，当前构造过程实际不会失败。
- Go 方法使用值接收者，但内部含指针/接口/`*sync.Mutex`，副本共享连接和锁；Rust 通过唯一的 `&mut self` 修改状态，不具有这种浅复制行为。
- Go 已由 `clientConn.setConn`、`PacketIO` 和 `clientConn.isConnectionAlive` 使用；Rust 当前证据仅覆盖测试接线。

Go 相关行为证据分布在 `pkg/server/conn_test.go`、`pkg/server/server_test.go` 与 `pkg/server/internal/packetio_test.go`：这些测试大量以 `NewBufferedReadConn` 给 `PacketIO` 提供输入，验证包读取、连接替换、压缩及服务流程。不过同目录 `util_test.go` 没有该类型的专门用例；Rust 的窥视、分片与探活直接回归由两个独立 Rust 测试文件承担。

## 扩展指南

- 若要把该类型接入 Rust MySQL 服务主链，应从 Rust 侧连接接收处和 packet I/O 类型开始接线，不能仅依据 Go 路径假定接口兼容；尤其要决定是否补齐 `Write`、关闭、地址、deadline、TLS 升级及底层流访问能力。
- 修改缓冲大小或 `Peek` 上限时，应同步 `DefaultReaderSize`、容量检查及 `pkg/server/internal/util/buffered_read_conn_test.rs`；新增超容量、零长度、EOF 和重复 Peek 用例可覆盖当前未直接断言的边界。
- 修改窥视/读取协调逻辑时，必须保住 `peeked` 先于 `reader` 的顺序不变量，并同步两个测试：`buffered_read_conn_test.rs::peek_waits_for_fragmented_bytes_without_consuming_them` 与 `migration_aster_unit_test.rs::buffered_connection_peeks_then_reads_without_losing_data`。
- 修改 `IsAlive` 时，要分别测试空闲超时返回 `1`、对端关闭返回 `0`、已有缓冲数据及其他错误返回 `-1`，并检查 deadline 总能恢复。30 微秒是调度和平台敏感值，测试应使用本地 TCP 与明确同步，避免只靠睡眠制造竞态。
- 若性能分析显示 `Vec::drain` 成本突出，可考虑带游标的暂存区，但必须防止未消费前缀无限保留，并保持 `Peek` 返回借用的生命周期安全。
- 若希望支持泛型流或 TLS，应先定义所需能力边界；简单泛化为 `Read` 不足以支持 `IsAlive` 所需的 read-timeout。兼容风险集中在公开构造签名、错误种类和三态返回约定。

## 验证依据

事实核对读取了以下路径：生产实现 `pkg/server/internal/util/buffered_read_conn.rs`；crate 声明 `pkg/server/internal/util/Cargo.toml`；模块入口 `pkg/server/internal/util/lib.rs`；Go 对照 `pkg/server/internal/util/buffered_read_conn.go`；Rust 测试 `pkg/server/internal/util/buffered_read_conn_test.rs`、`pkg/server/internal/util/migration_aster_unit_test.rs`；Go 调用与测试证据 `pkg/server/conn.go`、`pkg/server/internal/packetio.go`、`pkg/server/conn_test.go`、`pkg/server/server_test.go`、`pkg/server/internal/packetio_test.go`。`pkg/server` 及目标子目录未发现适用于本包的 `doc.go`。

RustCodeGraph 索引状态为 11,467 个文件、307,296 个节点和 1,848,419 条边。执行了目标目录文件查询、`explore "pkg/server/internal/util/buffered_read_conn.rs BufferedReadConn read_data"`、目标文件 `node --file`、`BufferedReadConn`/`NewBufferedReadConn`/`Peek`/`IsAlive`/`read` 符号查询，以及构造器 callers/callees 查询。图中目标文件标为 11 个符号，并报告由 `pkg/server/internal/util/migration_aster_unit_test.rs` 使用；精确源码搜索补充确认 `buffered_read_conn_test.rs` 的 crate 内测试引用，且未发现 Rust 生产调用者。

行为结论以源码和测试静态核对为准；依任务约束未运行 Cargo 或测试。交付前另运行任务指定的结构验证，确认文件存在且恰好包含上述 11 个固定二级章节。
