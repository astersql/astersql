# `pkg/server/internal/packetio.rs`

## 文件定位

本文件实现 Rust 服务端的 MySQL 线协议包编解码器，属于 Cargo crate `astersql-server-internal`。crate 根模块 `pkg/server/internal/lib.rs` 以 `pub mod packetio` 声明并通过 `pub use packetio::*` 再导出这里的公开符号；`pkg/server/internal/Cargo.toml` 指定 `lib.rs` 为库入口，并为本实现引入 `flate2` 与 `zstd`。

它位于连接传输与命令处理之间：`pkg/server/runtime.rs` 中的 `TcpPacketIo` 和 Unix 平台上的 `UnixPacketIo` 持有 `astersql_server_internal::PacketIO`，把套接字字节交给 `PacketIO::read_packet`，并把上层响应交给 `PacketIO::write_packet`。本文件只负责协议帧、缓冲和序号，不解析 COM_QUERY 等命令，也不直接拥有写套接字；生产传输会在 `PacketIO::flush` 后用 `take_written_data` 取出完整编码字节并写入 TCP、TLS 或 Unix 流。

## 核心职责

- 按 MySQL 普通包格式处理 4 字节头：3 字节小端载荷长度和 1 字节序号（`read_u24`、`write_u24`、`read_one_packet`、`write_packet`）。
- 按 `MAX_PAYLOAD_LEN`（`0x00ff_ffff`）拆分超大写载荷，并在读取时把连续满长物理包重组为一个逻辑包（`write_packet`、`read_packet`）。
- 用独立的 `sequence` 与 `compressed_sequence` 管理普通协议和压缩协议序号；命令边界可由 `reset_sequence` 同时归零。
- 实现 MySQL 7 字节压缩头和 zlib/zstd 编解码（`CompressedWriter`、`CompressedReader`、`compress`、`decompress`）。短于或等于 50 字节的待写数据不实际压缩，压缩头中的未压缩长度写为 0。
- 对读入逻辑包执行累计大小限制，对普通包和压缩外层包执行序号检查，并将 I/O/编码错误转换成 `String` 暴露给上层。
- 提供内存输出的查看、转移和复用接口（`written_data`、`take_written_data`、`into_written_data`），让传输层在编码完成后安全接管字节。

## 主要符号

- 常量：`DEFAULT_WRITER_SIZE` 为输出 `Vec` 的初始容量；`MAX_PAYLOAD_LEN` 是普通包单帧载荷上限；`COMPRESSION_NONE`、`COMPRESSION_ZLIB`、`COMPRESSION_ZSTD` 是算法标识。内部常量 `MAX_COMPRESSED_SIZE` 将压缩写缓冲限定为 1 MiB，`MIN_COMPRESS_LENGTH` 为 50。
- `PacketIO`：公开的有状态编解码器。`reader`/`compressed_reader` 管理读侧，`output`/`compressed_writer` 管理写侧；`max_allowed_packet`、`accumulated_length` 管理单次逻辑读包限制；`compression_algorithm`、`zstd_level` 和两套序号保存协议状态。
- `PacketIO::new_packet_io(reader, max_allowed_packet)`：生产构造器，接收 `Box<dyn Read + Send>`；自由函数 `new_packet_io` 是等价便捷入口。`new_packet_io_for_test` 及其自由函数版本创建无 reader 的内存写实例。
- `PacketIO::read_packet`：公开读入口；重置累计长度，调用私有 `read_one_packet`，并在物理载荷恰为 `MAX_PAYLOAD_LEN` 时继续读取和拼接。
- `PacketIO::write_packet`：公开写入口；要求调用者提供至少 4 字节的预留头空间，原地回填长度和序号，再经 `write_protocol` 写入普通或压缩缓冲。
- `PacketIO::set_compression_algorithm`：只接受 zlib 或 zstd；创建压缩 writer，并在存在普通 reader 时将其移动进 `CompressedReader`。
- `PacketIO::flush`：把压缩 writer 的待处理数据编码后追加到 `output`，同步压缩序号，并按 Go 行为令普通序号等于压缩序号。未启用压缩时数据已直接位于 `output`，因此该函数无需额外动作。
- `CompressedWriter`：实现 `std::io::Write`，以 1 MiB 块累积普通包字节；`flush_packet` 生成 7 字节压缩头和载荷，`flush` 返回并清空已编码字节。
- `CompressedReader<R: Read>`：实现 `std::io::Read`；`load_packet` 校验外层序号、读取一帧、按头字段决定透传或解压，再由 `read` 分段复制给调用者。
- `compress`/`decompress`：私有算法分派；zlib 使用 `flate2`，zstd 使用 `zstd::bulk`。`read_u24`/`write_u24` 负责三字节长度字段。

## 执行流程

读取普通包时，`runtime.rs` 的传输实现调用 `PacketIO::read_packet`。该方法先把 `accumulated_length` 清零，再由 `read_one_packet` 通过 `read_exact_protocol` 读满 4 字节头。普通模式直接访问 `reader`，压缩模式则从 `CompressedReader` 暴露的解码后字节流读取。随后解析长度、检查（或在压缩模式兼容性忽略）内层序号、推进 `sequence`、累计长度并验证 `max_allowed_packet`，最后精确读取载荷。如果载荷长度为 `MAX_PAYLOAD_LEN`，`read_packet` 重复这一过程，直到出现短包；短包也包括长度为 0 的终止包。

读取压缩包时，`CompressedReader::read` 在当前 `decoded` 已耗尽后调用 `load_packet`。后者先读 7 字节外层头，严格检查 `compressed_sequence` 并推进序号，再读指定长度的外层载荷。未压缩长度为 0 时载荷原样成为普通包字节流；否则调用 `decompress`，并验证实际解压长度等于头中声明值。`PacketIO::read_exact_protocol` 在成功读取后把 reader 的最新外层序号同步回 `PacketIO`。

写入时，上层传输先在业务载荷前预留 4 字节，调用 `write_packet`。方法对每个满长块回填 `0xFFFFFF` 和当前序号，经 `write_protocol` 写出并推进序号；最后为余量（包括恰好整除时的零长度终止包）写头并推进序号。普通模式直接追加到 `output`；压缩模式写入 `CompressedWriter::pending`。

压缩写端把普通协议字节按最多 1 MiB 聚合。缓冲满或显式 `flush` 时，`flush_packet` 对大于 50 字节的数据压缩，否则透传；随后写入“压缩长度、外层序号、未压缩长度”三部分头字段并推进外层序号。`PacketIO::flush` 将编码结果并入 `output`，生产传输随后 `take_written_data`、写套接字并 flush。`take_written_data` 只转移 `Vec` 内容而保留 `PacketIO` 实例，后续写入仍可继续使用。

## 数据与状态

`PacketIO` 的普通序号和压缩序号都是 `u8`，使用 `wrapping_add(1)`，因此按协议自然从 255 回绕到 0。压缩模式下外层序号由 `CompressedReader`/`CompressedWriter` 各自推进并同步回 `PacketIO`；`reset_sequence` 同时清零两套序号、重建压缩 writer，并清空压缩 reader 尚未消费的解码缓存。

`accumulated_length` 只覆盖一次 `read_packet` 调用，使用饱和加法，避免长度累计溢出绕回后绕过 `max_allowed_packet`。构造时必须由调用者显式传入上限；测试构造器把它设为 0，但该构造器没有 reader，设计用途是写侧测试。

输出采用分层缓冲：普通模式直接积累在 `PacketIO::output`；压缩模式先进入 `CompressedWriter::pending`，每帧编码进 `encoded`，再由 `PacketIO::flush` 移入 `output`。`written_data` 借用查看，`take_written_data` 转移并留下空 `Vec`，`into_written_data` 消费整个实例。

`read_timeout` 被保存但本文件不设置底层 reader 的 deadline。`pkg/server/runtime.rs` 的 TCP/Unix `set_read_timeout` 先对实际套接字调用 `set_read_timeout`，再把值记录到 `PacketIO`；因此当前 Rust 超时语义由传输层实现，本字段本身不驱动 I/O。

## 依赖与调用关系

上游生产调用关系由 RustCodeGraph 和 `pkg/server/runtime.rs` 共同确认：`TcpPacketIo::new_with_options` 与 `UnixPacketIo::new` 构造 `PacketIO`；两者的 `PacketIo::read_packet`、`write_packet`、`flush`、`reset_sequence`、`set_read_timeout` 分别委托给同名/对应方法。TCP 的 `set_compression` 根据协商结果选择 `COMPRESSION_ZLIB` 或 `COMPRESSION_ZSTD`，TLS 升级后通过 `set_buffered_read_conn` 将 reader 切换为 `SharedTlsReader`。连接命令循环通过 `pkg/server/conn.rs` 的 `PacketIo` trait 间接使用这些传输实现。

下游直接依赖只有标准库 `Read`/`Write`、`Duration`，以及 `flate2::{ZlibEncoder, ZlibDecoder}` 和 `zstd::bulk`。crate 配置见 `pkg/server/internal/Cargo.toml`；本文件不依赖 SQL 会话、执行器或存储层。

测试调用者主要位于 `pkg/server/internal/packetio_test.rs` 和 `pkg/server/internal/migration_aster_unit_test.rs`。前者覆盖协议样例、分片读取、压缩编解码和畸形输入压力语料；后者明确验证 Go 迁移契约。Go 基准与对应行为测试位于 `pkg/server/internal/packetio_test.go`。

## 错误处理与边界

- `write_packet` 在输入少于 4 字节时返回 `"malformed packet"`；调用者必须遵守“前 4 字节可被覆盖”的 API 契约。
- 未压缩读模式严格校验普通包序号；压缩模式为兼容 MariaDB Connector/J 2.x 而忽略内层普通包序号不匹配，但外层压缩序号始终由 `CompressedReader::load_packet` 严格校验。
- 一次逻辑读包的累计载荷超过 `max_allowed_packet` 时返回 `"packet too large"`。头或载荷截断、reader 缺失、压缩 reader/writer 缺失都会返回错误，不会把部分包当作成功。
- `set_compression_algorithm` 只接受两个已知算法，未知值立即返回 `"unknown compression algorithm"`。私有 `compress`/`decompress` 也保留未知算法防御分支。
- 压缩帧载荷若编码后超过三字节长度上限，`CompressedWriter::flush_packet` 返回 `InvalidData`。当前 1 MiB 聚合上限通常使该边界留有充分余量。
- 解压后长度必须与头中声明的未压缩长度完全一致。zlib 路径最多读取“声明长度 + 1”字节，避免伪造的高膨胀数据在长度校验前无限扩展 `Vec`；zstd 以声明长度作为输出容量上限。
- `CompressedReader::load_packet` 首字节读到 EOF 时返回无新帧；一旦已开始读取头或载荷，后续短读通过 `read_exact` 报错。
- 所有公开 `PacketIO` 编解码错误目前简化为 `String`。`runtime.rs` 再用 `packet_error` 转换为连接层错误，并在读失败或真正套接字写失败时把连接标记为不存活。

## 并发与资源生命周期

`PacketIO`、`CompressedWriter` 和 `CompressedReader` 都依靠 `&mut self` 串行推进缓冲与序号，内部没有锁、任务或通道；同一实例不应由多个协议操作并发修改。生产 TCP TLS reader 的共享与加锁发生在 `runtime.rs::SharedTlsReader`，不是本文件的职责。

reader 以 `Box<dyn Read + Send>` 归 `PacketIO` 所有。启用压缩时，普通 reader 被 `take` 并移动进 `CompressedReader`；TLS 升级或替换连接时，`set_buffered_read_conn` 安装新 reader、丢弃旧压缩 reader，并清空/重新预留输出缓冲。注意该方法不会自动重新创建压缩 reader；当前生产调用是在握手 TLS 切换阶段使用，扩展新的“压缩已启用后替换 reader”流程时必须显式处理这一状态组合。

`CompressedWriter::flush` 通过 `mem::take` 交出完整编码缓冲，但保留算法、序号和可复用容器；`PacketIO::take_written_data` 同样支持多轮套接字 flush。资源释放依赖 Rust 所有权和析构，没有显式 close；真正的 TCP/Unix 关闭生命周期由 `runtime.rs` 的传输类型管理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/server/internal/packetio.go`。Rust 保留了 Go 的主要线协议语义：4/7 字节头、`MaxPayloadLen` 分包和重组、1 MiB 压缩聚合、50 字节压缩阈值、zlib/zstd、双序号、压缩模式忽略内层错误序号，以及 flush 后普通序号与压缩序号对齐。`pkg/server/internal/packetio_test.rs` 中的样例与 `packetio_test.go` 对应，`migration_aster_unit_test.rs` 进一步固定迁移契约。

实现边界存在明确差异：Go `PacketIO` 直接持有 `BufferedReadConn` 和 `bufio.Writer`，并在 `readOnePacket` 内设置 deadline、记录协议 metrics、返回 TiDB/MySQL 类型化错误；Rust 将套接字写入、deadline 和连接存活状态放在 `pkg/server/runtime.rs`，本文件使用内存 `Vec` 与字符串错误，且没有 metrics。Go `NewPacketIO` 从全局配置读取 `maxAllowedPacket`，Rust 构造器要求上层传入。

Rust 对未知压缩算法在 `set_compression_algorithm` 时立即拒绝；Go setter 先安装算法，实际 writer/reader 使用时才可能报告未知算法。Rust 的 zlib 解压增加了声明长度 + 1 的有界读取和精确长度验证，这是对畸形压缩数据的额外内存安全边界。Rust 的 `set_read_timeout` 只是状态记录，Go 则在每次头/载荷读取前直接设置 deadline。

Go 压缩 reader/writer 通过共享序号指针维护 `compressedSequence`；Rust 让 reader/writer 各自持有序号并在 `PacketIO` 边界同步。二者对正常串行命令流的可观察协议结果一致，但修改 flush、重置或半双工交错行为时必须同时复核两套状态机。

## 扩展指南

- 新增或改变普通包格式、分片规则时，优先修改 `read_one_packet`、`read_packet`、`write_packet`、`read_u24`/`write_u24`，并同步扩展独立的 `pkg/server/internal/packetio_test.rs`；不要把测试内嵌回生产文件。
- 新增压缩算法时，需要同时更新算法常量、`set_compression_algorithm` 白名单、`compress`、`decompress`，以及 `CompressedWriter`/`CompressedReader` 的往返测试；还要在 `pkg/server/runtime.rs::set_compression` 的 TCP/Unix 接线和握手协商处增加映射，并核对 Go 兼容性。
- 修改序号语义时，必须同时检查 `read_one_packet`、`CompressedReader::load_packet`、`CompressedWriter::flush_packet`、`PacketIO::flush` 和 `reset_sequence`。尤其不能取消“压缩模式忽略内层错误序号”的 MariaDB Connector/J 兼容行为，除非协议兼容策略明确变化。
- 修改输出接管流程时保持不变量：传输层只应在 `PacketIO::flush` 成功后调用 `take_written_data`；否则压缩 writer 中仍可能有未编码数据。相关回归点是 `encoded_output_can_be_safely_taken_between_socket_flushes` 以及 `runtime.rs` 的 TCP/Unix `flush`。
- 改变包大小或解压限制时，应覆盖截断头/载荷、错误序号、超限累计载荷、伪造未压缩长度、高膨胀 zlib、1 MiB 压缩块和 `MAX_PAYLOAD_LEN` 边界。内存占用与压缩 CPU 是主要性能风险。
- 若要让 `PacketIO` 自身执行超时，需要重新设计 `Read` 抽象或传入可设置 deadline 的能力；当前不要误以为修改 `read_timeout` 字段就会影响自定义 reader。
- 若引入压缩开启后的 reader 替换，必须修复或约束 `set_buffered_read_conn` 与 `compressed_reader` 的生命周期，避免算法状态为压缩但 reader 缺失。行为变化还应与 `pkg/server/internal/packetio.go` 及 `packetio_test.go` 对照。

## 验证依据

- 源码事实：RustCodeGraph `status` 显示索引覆盖 7,032 个 Rust 文件；通过 `node --file pkg/server/internal/packetio.rs` 阅读了全 568 行，并核对 `PacketIO`、`CompressedWriter`、`CompressedReader`、`read_packet`、`write_packet`、压缩辅助函数及条件编译测试模块。
- 调用证据：RustCodeGraph `explore "pkg/server/internal/packetio.rs packetIO PacketIO readPacket writePacket"` 找到 Rust `runtime.rs`/`conn.rs` 的读、写、flush、reset、压缩和地址相关调用链；随后通过 `node --file pkg/server/runtime.rs` 核对 TCP/Unix 构造、委托、TLS reader 替换和套接字 flush 的实际代码。
- crate 证据：`pkg/server/internal/Cargo.toml`、`pkg/server/internal/lib.rs`，确认 crate 名、库入口、再导出以及 `flate2`/`zstd` 依赖。
- Go 对照：`pkg/server/internal/packetio.go` 与 `pkg/server/internal/packetio_test.go`，核对原始字段、普通/压缩流程、MariaDB Connector/J 兼容分支、分包和压缩样例。
- Rust 测试证据：`pkg/server/internal/packetio_test.rs` 覆盖分片 reader、10,000 例确定性畸形语料、包头重写、满长分包、zlib/zstd、短载荷透传、输出接管和错误内层序号兼容；`pkg/server/internal/migration_aster_unit_test.rs` 覆盖普通序号、最大包限制、两种压缩往返和错误外层序号。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务指定的 11 章节结构检查，并人工复核本说明没有把 Go 独有能力描述为 Rust 已支持。
