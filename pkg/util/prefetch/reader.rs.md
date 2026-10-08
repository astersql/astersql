# `pkg/util/prefetch/reader.rs`

## 文件定位

本文件实现 `astersql-util-prefetch` crate 的核心业务逻辑：把一个同时支持读取和关闭的顺序字节流包装成带后台预取的读取器。crate 入口 [`lib.rs`](lib.rs) 声明 `reader` 模块并通过 `pub use reader::*` 再导出本文件的公开项；[`Cargo.toml`](Cargo.toml) 表明该 crate 只直接依赖 `crossbeam-channel = "0.5"`，并以 `pkg/util/prefetch` 为 Go 移植来源。

当前已接线的生产入口是对象存储读取路径。`pkg/objstore/s3like/store.rs::Storage::Open` 在 `ReaderOption.PrefetchSize > 0` 时，用 `NewReader(reader, range.RangeSize(), prefetchSize)` 包装新打开的对象 Range；`pkg/objstore/s3like/io.rs::S3ObjectReader::{reopen, seek}` 在重开或远距离 seek 后做相同包装。因此本文件位于“对象存储 Range 流 → 后台预取 → 上层顺序读取”的 I/O 边界，而不是 SQL 执行或存储事务逻辑本身。

## 核心职责

- `ReadCloser` 将 Rust 的 `Read` 与显式 `close` 合并为一个可装箱的边界 trait，对应 Go 的 `io.ReadCloser`。
- `NewReader` 分配两个半尺寸工作缓冲、无容量的数据通道和关闭通知通道，并启动一个后台线程。
- `Reader::run` 用 `readFullGo` 尽量填满一个半缓冲，再把已读取部分交给前台；它在两个工作缓冲间交替。
- `Reader::Read` 从当前 `Cursor<Vec<u8>>` 消费数据，必要时跨多个预取块填满调用方切片，并保持“本次已有数据时先返回数据，错误留到下次读取”的 Go 行为。
- `Reader::Close` 关闭底层资源、通知并等待后台线程，且通过 `closed` 保证显式重复关闭幂等。

实现的目标是把底层读取与前台消费重叠，降低顺序读取对象数据时的同步 I/O 等待；它不提供随机访问、seek、自动重试或数据校验，这些职责由 `S3ObjectReader` 等上层承担。

## 主要符号

- `pub trait ReadCloser: Read + Send`：要求底层流可跨线程转移，并提供 `fn close(&mut self) -> io::Result<()>`。`NewReader` 的输入和输出都是 `Box<dyn ReadCloser + Send>`，调用者无需依赖具体 `Reader` 类型。
- `pub struct Reader`：保存共享底层流 `r`、前台当前块 `curBufReader`、接收端 `bufCh`、后台错误槽 `err`、线程句柄 `wg`、幂等标记 `closed` 和关闭发送端 `closedCh`。
- `pub fn NewReader(r, rangeSize, prefetchSize)`：公开构造入口。每个工作缓冲的长度是 `prefetchSize / 2`，整数除法会向下取整；返回 trait object，并立即启动 `Reader::run`。
- `fn Reader::run(...)`：后台线程入口，交替选择两个工作缓冲，执行读满、发送、记录终止错误的循环。
- `fn Reader::readFullGo(reader, buf)`：局部模拟 Go `io.ReadFull`。它反复调用 Rust `Read::read`，直到缓冲填满、读到 `Ok(0)`，或遇到错误；返回“累计字节数 + 可选错误”。
- `pub fn Reader::Read(&mut self, data)`：保留 Go 命名与 `(usize, Option<io::Error>)` 返回形状的主体读取逻辑。
- `pub fn Reader::Close(&mut self)`：显式生命周期收尾入口。
- `impl Read for Reader` 与 `impl ReadCloser for Reader`：分别把标准 Rust `read` 和 trait `close` 委托给上述 Go 风格方法。

文件没有条件编译项、模块级常量、枚举或类型别名。`#![allow(dead_code)]` 与 `#![allow(non_snake_case)]` 用于容纳尚未被所有 Rust 路径使用的 Go 风格 API。

## 执行流程

1. 上层取得底层 `Box<dyn ReadCloser + Send>` 及准确的 Range 长度；例如 `Storage::Open` 使用 `RangeInfo::RangeSize()`。
2. `NewReader` 创建两个 `bounded(0)` rendezvous channel：一个交付数据块，一个传达关闭；随后把底层流和错误槽放入 `Arc<Mutex<_>>`，并生成两个长度为 `prefetchSize / 2` 的工作缓冲。
3. 后台 `run` 先把索引从 0 切到 1，锁住底层流并调用 `readFullGo`。分片源即使单次只返回少量字节，也会被反复读取，直到半缓冲填满或终止。
4. 后台把本轮实际读取的前 `n` 字节复制成独立 `Vec<u8>`，累计 `readSize`，再通过 `select!` 在“关闭通知”和“向 `bufCh` 交付”之间选择。零容量通道使发送与前台接收同步；交付后后台可继续填充另一个工作缓冲。
5. 若本轮有错误，后台在交付已读字节之后处理它：由预取缓冲偏大产生、且累计读取量恰好等于 `rangeSize` 的 `UnexpectedEof` 被当作正常结束；其他错误以 `(ErrorKind, String)` 写入共享错误槽。线程返回时数据通道发送端被丢弃，前台据此识别终止。
6. 前台 `Read` 在没有当前块时阻塞接收一个 `Vec<u8>`，用 `Cursor` 顺序消费。调用方切片大于当前块剩余数据时，它丢弃耗尽的 `Cursor` 并继续接收下一块，直至填满或通道断开。
7. 通道断开时，若当前调用已经取得字节，`Read` 先返回 `(total, None)`；下一次调用才从错误槽重建错误。错误槽为空代表正常 EOF，在 Rust `Read` 约定中表现为 `Ok(0)`。
8. 显式 `Close` 首先调用底层 `close`，随后丢弃唯一的 `closedCh` 发送端以唤醒后台 `select!`，再 `join` 后台线程并设置 `closed = true`。

## 数据与状态

`Reader` 的状态分成前台独占、跨线程共享和生命周期三类：

- 前台独占：`curBufReader` 只由 `&mut self` 的 `Read` 修改，保存尚未消费完的已交付块；`closed` 与两个 `Option` 句柄也只在 `Close` 中推进。
- 跨线程共享：底层 `r` 由 `Arc<Mutex<Box<dyn ReadCloser + Send>>>` 保护；`err` 是一次性终止错误槽。错误只在后台停止前写入，前台只在数据通道断开后读取。
- 通道状态：`bufCh` 是数据接收端；后台独占发送端。`closedCh` 的发送端由前台持有，后台只观察接收端断开，并不发送实际的 `()` 值。
- 后台局部状态：`buf: [Vec<u8>; 2]`、`bufIdx` 和累计的 `readSize` 归工作线程所有。交付时会复制有效区间，因此已交付块不会因下一轮复用工作缓冲而改变。
- `rangeSize` 不存入返回的 `Reader`，只移动到后台线程，作为区分“预取缓冲大于剩余 Range”与真实短读错误的终止边界。

关键不变量是：每个块先完整读取或遇到终止条件，再按顺序交付；错误只能在相同轮次已读数据交付之后对前台可见；`Close` 返回后线程句柄已被取走，后台不再运行。当前实现每次交付都由 `to_vec()` 新建所有权缓冲，所以实际瞬时内存除两个工作缓冲外，还可能包含前台 `Cursor` 持有的块和后台等待交付的复制块，不能把 `prefetchSize` 解释为严格内存上限。

## 依赖与调用关系

上游调用关系：

- `pkg/objstore/s3like/store.rs::Storage::Open → prefetch::reader::NewReader`：首次打开 S3-like 对象或 Range 时可选启用预取。
- `pkg/objstore/s3like/io.rs::S3ObjectReader::reopen → NewReader`：可重试读错误后从当前位置重开，并重新建立预取器。
- `pkg/objstore/s3like/io.rs::S3ObjectReader::seek → NewReader`：远距离或反向 seek 重开底层流后重新包装；短距离前向 seek 则由上层读取并丢弃数据。
- `pkg/util/prefetch/{reader_test.rs,migration_aster_unit_test.rs}` 直接构造预取器，验证公开读取和关闭契约。

下游依赖关系：

- `NewReader → std::thread::spawn → Reader::run` 建立后台执行单元。
- `Reader::run → Reader::readFullGo → ReadCloser::read` 驱动底层流。
- `Reader::run ↔ crossbeam_channel::{bounded, select}` 完成前后台块交付和关闭仲裁。
- `Reader::Read → Receiver::recv → Cursor<Vec<u8>>::read` 把块流适配回连续的 `Read` 接口。
- `Reader::Close → ReadCloser::close → JoinHandle::join` 关闭底层资源并等待后台结束。

crate 内没有 feature 开关。直接依赖只有 `crossbeam-channel`；`Arc`、`Mutex`、线程、`Cursor` 和 I/O 错误类型均来自标准库。仓库根 `Cargo.toml` 以 `facade_util_prefetch` 指向本 crate，`pkg/objstore/{s3like,s3store,ossstore}/Cargo.toml` 也声明了同一路径依赖。

## 错误处理与边界

- 正常 EOF：Rust 底层返回 `Ok(0)` 时，`readFullGo` 合成 `UnexpectedEof`。若累计字节恰等于 `rangeSize`，后台不写错误槽；前台最终得到 `Ok(0)`。这对应 Go 将预取导致的尾部 `io.ErrUnexpectedEOF` 收敛为 `io.EOF`，但 Rust 标准 `Read` 用零字节成功表示 EOF。
- 真实错误：其他错误在已读部分交付后写入错误槽；前台如果本次已有数据先成功返回，下一次读取再报错。槽只保存 `ErrorKind` 和文本，原错误的具体类型、source 链和附加字段不会跨线程保留。
- 底层直接 `UnexpectedEof`：现有测试以读取 0 字节、`rangeSize = 10` 验证其不会被误当正常 EOF。代码的判定条件仅是错误种类和累计长度；若某个自定义底层源恰在累计量等于 `rangeSize` 时直接返回 `UnexpectedEof`，当前实现无法区分该错误与 `readFullGo` 合成的错误。
- 关闭错误：`Close` 即使收到底层 `close` 错误，仍会通知、等待线程并标记已关闭，最后返回该底层错误；第二次 `Close` 返回成功，不会再次关闭底层流。
- 线程异常：锁中毒会通过 `expect` panic；`Close` 忽略 `join` 返回的 panic 信息。调用者只会看到底层 `close` 的结果。
- 参数边界：`prefetchSize` 未在本文件校验。值为 0 或 1 时，半缓冲长度为 0，后台会反复交付空块，前台 `Read` 不能取得有效进展。当前生产调用点仅检查 `prefetchSize > 0`，并未排除 1；安全调用方应保证至少为 2，或扩展构造器时增加显式校验。`rangeSize` 的准确性也由调用方负责；不匹配会改变尾部错误是否被视为正常 EOF。
- API 边界：注释明确 `Read` 与 `Close` 不可并发调用；两者都要求 `&mut self`，普通安全 Rust 调用会执行该限制。关闭后继续读取、空目标切片以及未显式关闭便直接 drop 的行为没有单独的公开契约测试。

## 并发与资源生命周期

每个 `NewReader` 创建一个原生线程。数据 channel 为零容量：后台必须等到前台接收当前块才能完成发送，但一旦交付即可在前台消费当前块期间读取下一块，从而形成一块消费、一块填充的流水线。关闭 channel 同样为零容量，但实际以丢弃发送端表示关闭；后台在下一次 `select!` 时退出。

底层流被 `Arc<Mutex<_>>` 共享。后台在整个 `readFullGo` 期间持锁，`Close` 必须先取得同一把锁才能调用底层 `close`；因此若底层 `read` 长时间阻塞且自身不会返回，当前 Rust 锁布局不能像 Go 版本那样从另一执行流并发调用 `Close` 来打断它，`Close` 也会等待锁。这是扩展网络流或取消语义时必须评估的资源回收风险。

`Close` 的既定顺序是“底层 close → 断开关闭通道 → join → 标记 closed”。这一顺序确保正常返回后后台不再访问共享资源。类型没有实现 `Drop`：只丢弃返回的 trait object 会断开关闭通道并最终释放 `Arc`，但不会保证调用底层 `ReadCloser::close`，也不会在当前线程显式 `join`；需要确定性释放外部资源的上层必须调用 `close`。生产侧 `S3ObjectReader` 的 `objectio::Reader::close` 会向内委托这一调用。

## 与 Go 版本的对应关系

主要映射以 [`reader.go`](reader.go) 为准：

| Go | Rust | 语义说明 |
| --- | --- | --- |
| `io.ReadCloser` | `ReadCloser: Read + Send` | Rust 额外要求可发送到后台线程。 |
| `Reader.buf [2][]byte`、`bufIdx` | `run` 的 `[Vec<u8>; 2]`、`bufIdx` | Go 把缓冲保存在结构体，Rust 把后台独占状态移动进线程。 |
| `chan []byte` | `crossbeam_channel::bounded::<Vec<u8>>(0)` | 都是无缓冲交付；Rust 因所有权需要复制有效字节。 |
| `go ret.run()`、`sync.WaitGroup` | `thread::spawn`、`JoinHandle` | `run` 返回代表 worker 完成，`join` 对应 `Wait`。 |
| `io.ReadFull` | `readFullGo` | 保留分片读取、部分字节和终止错误的处理顺序。 |
| `bytes.Reader` | `Cursor<Vec<u8>>` | 保存并消费当前预取块。 |
| `err error` | `Option<(ErrorKind, String)>` | Rust 重建错误，只保留 kind 与文本。 |
| `close(closedCh)` | `closedCh.take()` | 丢弃唯一发送端使接收端观察到断开。 |

Rust 主体保持了 Go 的双缓冲、先交付部分数据再暴露错误、精确 Range 尾部转换、跨块填充和重复关闭语义。差异包括 Rust EOF 表示为 `Ok(0)` 而非返回 `io.EOF`、底层流和错误槽需要同步原语、块交付发生复制，以及底层读取期间持有互斥锁造成的关闭/阻塞关系。

[`reader_test.rs`](reader_test.rs) 基本逐项对应 [`reader_test.go`](reader_test.go)：`test_basic`/`TestBasic`、`test_convert_unexpected_eof`/`TestConvertUnexpectedEOF`、`test_close_before_drain_read`/`TestCloseBeforeDrainRead`、`test_fill_prefetch_buffer`/`TestFillPrefetchBuffer`。Rust 的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 另补了重复关闭只触发一次底层关闭的断言。

## 扩展指南

- 调整预取大小或缓冲策略：修改 `NewReader` 的半缓冲分配与 `Reader::run` 的交付所有权模型；同步更新 `reader_test.rs::test_fill_prefetch_buffer` 和 Go 对照测试表达的“两个半缓冲”不变量。应测奇数、0、1、极大尺寸及内存峰值，避免空缓冲活锁和无界复制。
- 改变 EOF/错误语义：集中修改 `readFullGo`、`run` 的 `UnexpectedEof` 判定和 `Read` 的通道断开分支；必须覆盖部分数据加错误、`rangeSize` 过大/过小、底层自发 `UnexpectedEof` 以及错误 source 保真。不能仅让标准内存 `Cursor` 场景通过。
- 增加取消或超时：接入点是 `run` 的 `select!`、底层读取的同步方式以及 `Close` 顺序。需要先解决后台持锁读取使 `Close` 无法并发打断的限制，并增加阻塞型 `ReadCloser` 的独立测试。
- 减少复制或改为缓冲池：需要让块所有权在前后台之间安全归还，而不仅是把 `to_vec()` 删除；应保持块顺序、当前块消费完才复用和关闭时不悬挂，并用压力测试验证峰值内存及吞吐。
- 增加自动资源回收：若实现 `Drop`，不能在不可控上下文中无限阻塞 `join`，且需明确底层 `close` 失败无法从 `Drop` 返回。显式 `close` 的错误契约仍应保留。
- 增加生产调用点：调用方须提供准确的总 Range 字节数、至少 2 的有效预取尺寸，并确保最终调用 `close`。对象存储侧若改变 Range/seek 约定，应同步复核三个 `NewReader` 调用点。

测试逻辑应继续放在独立的 `reader_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产文件。若行为也要求与上游 Go 保持一致，应同步检查 `reader.go` 和 `reader_test.go`，区分有意的 Rust API 差异与移植偏差。

## 验证依据

- RustCodeGraph 索引状态：项目包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/prefetch` 确认本 crate 的 Rust/Go 源与测试均已索引。
- RustCodeGraph `node --file pkg/util/prefetch/reader.rs`：核对了完整 261 行源码、`ReadCloser`、`Reader`、`NewReader`、`run`、`readFullGo`、`Read`、`Close` 及两个 trait impl；精确 `node NewReader --file ...` 给出 `NewReader → run` 调用边。
- RustCodeGraph `node --file`：读取并核对了 `pkg/util/prefetch/lib.rs`、`reader.go`、`reader_test.rs`、`reader_test.go`、`migration_aster_unit_test.rs`，以及生产调用点 `pkg/objstore/s3like/{store.rs,io.rs}`。
- 配置读取：`pkg/util/prefetch/Cargo.toml` 确认 crate 名、入口、唯一直接依赖和 Go 包元数据；仓库 Cargo 清单搜索确认对象存储 crate 与根 facade 的路径依赖。
- 调用点搜索：Rust 生产源码中直接调用本 `NewReader` 的位置为 `s3like/store.rs` 一处和 `s3like/io.rs` 两处；其他命中位于本 crate 测试或同名 API。RustCodeGraph 的精确 `callers/callees` 命令在限定时间内未返回结果，因此调用方集合同时用已索引调用文件源码和仓库符号搜索交叉核验。
- 测试事实：Rust 与 Go 测试共同覆盖跨缓冲顺序、整段/短段读取、正常 EOF、底层 `UnexpectedEof`、未消费完即关闭、分片源填满半缓冲；迁移补充测试覆盖重复关闭幂等和底层只关闭一次。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核文档只描述当前代码可证事实。
