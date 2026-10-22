# `pkg/server/internal/util/buffered_read_conn.rs`

## 文件定位

属于 `astersql-server-internal-util`，由 `pkg/server/internal/util/lib.rs` 重导出；`Cargo.toml` 声明 crate。它提供缓冲读取包装及可被生产 TcpPacketIo 单独调用的非消费 TCP 探针。

## 核心职责

保持 Peek/Read 字节顺序、支持显式暴露底层 TCP 的读取包装器，并将探活与读取缓冲彻底分离。探针不调整 nonblocking flag 或 deadline。

## 主要符号

`DefaultReaderSize=16*1024`；`BufferedReadTransport: Read + Send` 提供默认返回 None 的 net_stream；TcpStream 实现返回自身。`BufferedReadConn` 持有 Mutex<BufferedReaderState> 与 Option<TcpStream> 探针 handle。公开 NewBufferedReadConn、Peek、Read、IsAlive、probe_tcp_stream_alive，另实现标准 Read trait。

## 执行流程

构造器对 net_stream 暴露的 socket 执行 try_clone，失败传播；读取端被装入 BufReader<Box<dyn BufferedReadTransport>>。Peek(&self,n) 在锁内检查容量，循环 fill_buf/consume，将字节存入 peeked，返回 Vec 副本。Read(&self) 先返回 peeked 前缀，否则读 BufReader；标准 Read 委托此方法。IsAlive 只探测独立 socket handle。

## 数据与状态

逻辑未消费数据按 peeked、BufReader 缓冲、底层读取端排序。Peek 对内部缓冲消费后保留 peeked，后续 Read 必须先排空它；Read 允许短读。读取状态由单一 Mutex 保护；探活不访问此状态。

## 依赖与调用关系

Unix 探针直接用 libc::recv，flags 为 MSG_PEEK | MSG_DONTWAIT。生产 `pkg/server/runtime.rs::TcpPacketIo::connection_alive_probe` 使用此 helper；这证明探针接入生产，并不证明 BufferedReadConn 包装类型本身已进入 packet 主链。测试位于同目录 buffered_read_conn_test.rs 与 migration_aster_unit_test.rs。

## 错误处理与边界

Peek 超容量返回 InvalidInput，未补齐就遇 EOF 返回 UnexpectedEof，锁中毒返回 io::Error，I/O 错误直接传播。Unix recv>0 返回1，=0 返回0；EAGAIN/EWOULDBLOCK 返回1，ECONNRESET/EPIPE/ENOTCONN/ETIMEDOUT 返回0，其余返回-1。无显式 socket 或非 Unix 平台返回-1；这是未知，不是已死亡。

## 并发与资源生命周期

Peek/Read 串行持有 reader 锁，可能阻塞等待数据；IsAlive 使用 &self 且不取该锁，可并发探测，不消费 TLS/PROXY 包装器的协议数据。socket clone 随包装器或探针闭包 drop 释放；本文件没有主动 shutdown 或后台线程。Vec 返回与 drain 会复制或移动数据，不能视为零拷贝。

## 与 Go 版本的对应关系

Go `pkg/server/internal/util/buffered_read_conn.go` 也将探活从共享 bufio.Reader 分离，经 NetConn/syscall.Conn 解包；Rust 依赖 BufferedReadTransport::net_stream 显式暴露 socket，不执行 Go 的最多16层动态解包。Go 包装完整 net.Conn，Rust 仅实现读取能力；Peek 返回拥有 Vec，构造器因 socket clone 可失败。

## 扩展指南

新增 TLS/PROXY 读取包装器时显式实现 net_stream，不能强行探测未知 Read 对象。维护探活不改变 deadline/flags、不消费字节的合同；独立回归包含 pending data、并发 Read/IsAlive、exposed stream 和 NetConn wrapper。修改平台 errno 映射时同步平台测试。

## 验证依据

冻结范围为 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`。经 `git diff ad0a6f4..2c944fb -- pkg/server/internal/util/buffered_read_conn.rs`、`git show 2c944fb:pkg/server/internal/util/buffered_read_conn.rs` 核对，目标 blob 为 `c77d748cd24b8a01c796c8fc8e99e3e1534e2c7e`。模块入口及 Cargo 声明均以同一提交读取。独立测试 `pkg/server/internal/util/buffered_read_conn_test.rs` 和 Go 对照 `pkg/server/internal/util/buffered_read_conn.go` 为静态证据，未运行测试。

RustCodeGraph 当前索引包含 Rust，但不是冻结快照；本篇结论以 Git blob 为准。Ready 文档校验确认 11 章节、引用存在和差异无空白错误；按计划不运行 Cargo，不验证真实网络或存储。
