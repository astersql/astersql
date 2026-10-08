# `pkg/util/chunk/row_container_reader.rs` 逻辑说明

## 文件定位

本文件属于 `astersql-util-chunk` crate，是 `RowContainer` 与逐行游标消费方之间的异步读取适配层。crate 入口 `pkg/util/chunk/lib.rs` 以公开模块 `row_container_reader` 暴露它；`pkg/util/chunk/Cargo.toml` 声明的直接实现依赖是 `crossbeam-channel = "0.5"`，其余核心类型 `Row`、`ChunkError` 来自 crate 根。

它对应 Go 文件 `pkg/util/chunk/row_container_reader.go`，目标是把按 Chunk 保存的行容器转成只前进的逐行读取接口。Rust 当前已有完整实现、`RowContainer` 数据源适配和服务端游标消费接口，但仓库检索显示 `NewRowContainerReader` 的 Rust 调用者只有 `pkg/util/chunk/iterator_3_aster_unit_test.rs`；尚未发现与 Go `pkg/server/conn_stmt.go:442-448` 等价的 Rust 生产构造接线。因此它目前是“实现和下游接口已具备、生产创建主链尚未验证接通”，不能表述为 Rust 服务端游标已经实际使用该 worker。

## 核心职责

- `RowContainerReader` 定义只前进游标协议：`Next`、`Current`、`End`、`Error`、`Close`（`pkg/util/chunk/row_container_reader.rs:27-38`）。
- `RowContainerSource` 把 worker 所需能力缩到只读的 Chunk 数量、首块容量估算和按块取行，避免读取器依赖 `RowContainer` 的全部接口（第 43-50 行）。
- `rowContainerReader` 在后台线程按 Chunk 顺序预取行，通过有界 channel 向前台逐行交付，并保存当前行、错误和关闭状态（第 61-78 行）。
- `NewRowContainerReader` 完成容量选择、channel/共享状态创建、worker 启动，并预先调用一次 `Next`，使新建读取器的 `Current` 已指向首行或结束哨兵（第 123-175 行）。
- `Close` 与 `Drop` 保证取消并回收后台线程，防止 worker 永久阻塞在满 channel 上（第 104-121 行）。

## 主要符号

- 公开 trait `RowContainerReader`：面向消费方的动态接口。`pkg/server/internal/resultset/cursor.rs:66-111` 接受 `Box<dyn RowContainerReader>`，并由 `RowContainerReaderIter` 转发到服务端 `RowIterator`。
- 公开 trait `RowContainerSource: Send + Sync`：面向数据源的 worker 契约。生产实现位于 `pkg/util/chunk/row_container.rs:325-344`；测试还定义了可注入失败的 `TestSource`。
- 私有 tuple struct `SendRow(Row)`：channel 载荷。文件对它执行 `unsafe impl Send`；安全依据依赖读取器持有的 `Arc<dyn RowContainerSource>` 能让 `Row` 背后的 Chunk 在跨线程传递期间保持有效（第 52-59 行）。这是本文件最重要的 unsafe 不变量。
- 公开但采用 Go 风格小写命名的 struct `rowContainerReader`：具体实现。字段 `source` 主要承担生命周期保活；`rowCh` 接收行；`cancel` 发出取消；`worker` 保存可 join 的线程；`err` 是共享错误槽；`closed` 保证关闭幂等。
- 公开函数 `NewRowContainerReader<S>(Arc<S>) -> Box<rowContainerReader>`：唯一构造入口，要求 `S: RowContainerSource + 'static`。
- `impl Drop for rowContainerReader`：兜底调用 `Close`；显式关闭后再次析构不会重复 join。

文件中没有模块级常量、enum、类型别名或条件编译项。

## 执行流程

1. 调用方把数据源放进 `Arc` 后调用 `NewRowContainerReader`。构造函数立即把它擦除为 `Arc<dyn RowContainerSource>`，并克隆一份给 worker。
2. 构造函数计算行 channel 容量：`NumChunks() == 0` 时使用 1024；否则使用首个 Chunk 行数的两倍，并通过 `saturating_mul(2)` 避免整数溢出。首块为空时容量可为 0，即同步 channel，而不是失败。
3. 创建有界行 channel、有界容量 1 的取消 channel，以及 `Arc<Mutex<Option<ChunkError>>>` 错误槽，然后启动后台线程。
4. worker 对 `0..NumChunks()` 顺序扫描。每个 Chunk 先调用一次 `RowsOfChunk`；若失败，将错误写入共享槽并退出。成功后依次发送每一行。
5. 每次发送用 `crossbeam_channel::select!` 在“行发送成功”和“收到取消”之间竞争。接收端被丢弃或取消到达时，worker 直接结束；全部扫描完成后，线程闭包结束并丢弃 `rowSender`。
6. 构造线程在返回前调用一次 `reader.Next()`。因此正常非空数据源返回时已经消费 channel 的第一项；空源、首块读取失败或全部发送结束时，`recv` 因发送端关闭而返回错误，`currentRow` 被置为 `End()`。
7. 后续 `Next` 阻塞等待下一行或 channel 关闭，更新并克隆返回 `currentRow`；`Current` 只克隆当前值，不推进。
8. 调用 `Close` 时先标记 `closed`，尝试发送取消，再取出并 join worker。析构时执行相同路径。

推荐消费形态由 Rust 单测和 Go 接口注释共同验证：以 `Current() != End()` 为循环条件，在循环末尾调用 `Next()`，结束后检查 `Error()` 并关闭读取器。由于构造函数已经预取首行，若循环开始前额外调用 `Next`，会跳过第一行。

## 数据与状态

`currentRow` 是前台唯一的游标位置，初始为 `Row::default()`，构造末尾的预取决定它是首行还是结束哨兵。`End()` 每次返回 `Row::default()`；协议因此依赖默认空 `Row` 不会与有效容器行混淆。

`rowCh` 是有界背压边界。非空容器按首块行数估算为两倍容量，目的是允许 worker 完整准备至少一个典型 Chunk 而不立即阻塞；它不是全容器缓存，后续 Chunk 更大时仍会自然背压。空容器使用 1024 只是容量默认值，不会产生行。

`err` 只由 worker 在 `RowsOfChunk` 失败时写入，前台通过 `Error` 加锁并克隆读取。互斥锁中毒时两处都用 `poisoned.into_inner()` 继续访问槽位，而不是 panic。错误不会作为 channel 元素传递；消费者需要读到 `End` 后单独查询 `Error`，才能区分正常耗尽和失败终止。

`source`、worker 捕获的 `workerSource` 与 `SendRow` 共同构成生命周期约束：读取器实例保留一份 source `Arc`，worker 再保留一份，以覆盖后台取行和已发送 `Row` 的使用期。`pkg/util/chunk/row_container.rs:334-342` 的 `RowsOfChunk` 会为指定 Chunk 的每个下标调用 `GetRow`，汇集成 `Vec<Row>` 后才交给发送循环。

## 依赖与调用关系

直接下游依赖如下：

- `crate::Row`：当前行、结束哨兵和 channel 载荷。
- `crate::ChunkError`：数据源失败和共享错误槽。
- `crossbeam_channel::{bounded, Receiver, Sender}` 及 `select!`：有界背压与取消竞争。
- `std::sync::{Arc, Mutex}`：数据源保活和跨线程错误共享。
- `std::thread::{spawn, JoinHandle}`：worker 生命周期。

直接上游和相邻接线如下：

- `pkg/util/chunk/row_container.rs:325-344` 为 `RowContainer` 实现 `RowContainerSource`，把读取请求转到 `NumChunks`、`NumRowsOfChunk` 和 `GetRow`。
- `pkg/server/internal/resultset/cursor.rs:66-111` 是接口消费边：`WrapWithRowContainerCursor` 接收一个已构造的 trait object，`RowContainerReaderIter` 转发全部五个方法。
- `pkg/util/chunk/iterator_3_aster_unit_test.rs:221-272` 是目前检索到的 Rust 构造调用边，覆盖真实 `RowContainer` 和故障测试源。
- Go 的完整生产链位于 `pkg/server/conn_stmt.go:425-455`：先把 `ResultSet` 全量写入 `RowContainer`，再构造 reader、包装为 cursor 并交给协议写出。该 Go 边只能作为 Rust 后续接线的对照，不能作为 Rust 已接线证据。

RustCodeGraph 的文件视图确认目标文件被 `pkg/util/chunk/row_container.rs` 和 `pkg/server/internal/resultset/cursor.rs` 两个文件使用；精确 `rg` 进一步确认 Rust `NewRowContainerReader` 仅在本文件与上述单测出现。

## 错误处理与边界

- `RowsOfChunk` 的第一个错误会覆盖空错误槽并终止 worker；不会继续读取后续 Chunk。已成功进入 channel 的前序行仍可被消费。
- 正常耗尽、数据源错误、发送端退出在 `Next` 层都表现为 `End()`；只有 `Error()` 能区分数据源错误。
- `rowCh.recv()` 是阻塞调用，没有超时和外部上下文参数。资源释放协议依赖持有读取器的一方调用 `Close` 或丢弃读取器。
- `Close` 忽略取消发送和 `join` 的结果：worker 已退出、取消 channel 已满或 worker panic 都不会从接口返回错误。接口本身没有 `Result` 返回位，因此扩展错误报告时需决定是否复用 `Error` 槽。
- 空容器在构造期间等待 worker 结束并关闭 channel，随后直接处于 `End`。首块行数为 0 时使用容量 0 的同步 channel，但 worker 可继续扫描后续 Chunk；若后续有行，构造线程与 worker 会直接交接首行。
- `NumRowsOfChunk(0)` 只用于容量估算；真正读取由 `RowsOfChunk` 决定。自定义 source 必须保证两者与 `NumChunks` 的下标契约一致，否则越界或不一致行为由 source 自身承担。
- `Mutex` 中毒被恢复，但 worker panic 本身不会写入 `err`，而 `Close` 又忽略 `join` 错误；当前接口无法向调用方明确报告这种 panic。

## 并发与资源生命周期

每个读取器恰好启动一个 OS 线程。前台只持有 `Receiver`，worker 独占行 `Sender`；发送端随 worker 退出而丢弃，这也是 `Next` 识别结束的机制。channel 有界，因此生产速度受消费速度限制，不会按总行数无界增长。

提前关闭时，`try_send(())` 不会因 worker 正在向满的行 channel 发送而阻塞；worker 的 `select!` 可选择取消分支并退出，随后 `join` 等待资源完全回收。`closed` 和 `worker.take()` 使重复 `Close` 与 `Drop` 后续调用成为空操作。

unsafe 边界集中在 `unsafe impl Send for SendRow`。维护者必须持续保证：所有可由 `RowContainerSource::RowsOfChunk` 返回的 `Row` 在跨线程和消费期间都有稳定 backing storage，且 `Row` 的读访问不会与 source 的变更形成未同步的数据竞争。当前注释以 source 的 `Arc` 保活为依据；若新增不同 source 实现、改变 `Row` 内部指针模型，或允许容器并发替换/释放 Chunk，必须重新审计这一假设。Rust 单测目前没有覆盖 Go 测试中的并发 spill 场景。

## 与 Go 版本的对应关系

两版的公共协议、构造时预取首行、按 Chunk/行顺序输出、首块行数两倍的缓冲策略、错误在迭代结束后查询，以及显式关闭 worker 的总体语义一致。对应关系为：Go `context.Context/cancel + WaitGroup + chan Row` 对应 Rust `cancel channel + JoinHandle + Receiver<SendRow>`；Go `err error` 对应 Rust `Arc<Mutex<Option<ChunkError>>>`。

关键差异如下：

- Go worker 每个 Chunk 调用一次 `rc.GetChunk`，确认完整 Chunk 读取成功后再逐行发送；Rust 把这一要求抽象为 `RowsOfChunk`，`RowContainer` 实现通过逐行 `GetRow` 收集 `Vec<Row>`。二者都在发送前形成完整的块级行集合，但底层取数路径不同。
- Go 通过 `context` 的 Done channel 取消，并用 runtime finalizer 在遗漏 `Close` 时记录警告；Rust 用容量 1 的取消 channel，并由确定性的 `Drop` 静默兜底。
- Go 测试通过 failpoint 注入 `GetChunk` 错误；Rust 通过自定义 `RowContainerSource` 返回 `ChunkError`，目标文件本身没有 failpoint 条件编译。
- Go `row_container_test.go:338-441` 覆盖落盘读取、读到一半关闭、读取期间并发 spill、读取中途 spill。Rust `iterator_3_aster_unit_test.rs:221-272` 目前只覆盖顺序读取、显式关闭和错误传播；并发 spill、空容器、提前关闭、重复关闭和析构兜底仍缺少直接 Rust 回归证据。
- Go 已由 `pkg/server/conn_stmt.go` 生产调用；Rust 只有服务端消费接口，未检索到生产构造调用，因此迁移接线尚不对等。

## 扩展指南

- 若改变迭代协议或结束表示，优先修改 `RowContainerReader`、`Next/Current/End` 实现，并同步 `pkg/server/internal/resultset/cursor.rs` 的 `RowContainerReaderIter` 以及 `pkg/util/chunk/iterator_3_aster_unit_test.rs`。尤其要保留或明确废弃“构造时预取首行”的兼容语义。
- 若改变数据源读取粒度，修改 `RowContainerSource` 与 `pkg/util/chunk/row_container.rs:325-344` 的实现。不要让 worker 在尚未完整取得一个 Chunk 的行集合时发送该 Chunk 的部分行，除非同时重新评估 Go 的并发 spill 语义。
- 若调整 channel 容量，关注首块为空、极大 Chunk、内存占用和背压。容量计算应继续防溢出，并补充容量为 0 与后续非空 Chunk 的测试。
- 若增加 source 实现，必须验证其 backing storage 生命周期、并发可读性和 `SendRow` 的 unsafe 前提；不能仅凭 trait 的 `Send + Sync` 推断内部 `Row` 指针可跨线程。
- 若把它接入 Rust 服务端生产主链，应以 Go `pkg/server/conn_stmt.go:425-455` 为行为对照，复用现有 `WrapWithRowContainerCursor`，并新增服务端级测试证明 reader 的创建、错误传递与关闭责任。
- 建议扩展独立 Rust 测试 `pkg/util/chunk/iterator_3_aster_unit_test.rs`，覆盖空 source、提前关闭、重复关闭、Drop 回收、首块为空、worker panic/错误可见性；并参照 `pkg/util/chunk/row_container_test.go:338-441` 增加 spill 前后与并发 spill 行为。测试逻辑应继续放在独立测试文件，不能内嵌到生产源文件。
- 性能风险集中在每 Chunk 先分配 `Vec<Row>`、channel 容量与逐行同步发送；兼容风险集中在首行预取、默认 `Row` 哨兵和错误查询时机；正确性风险集中在 unsafe 跨线程行视图及 source 的并发变更。

## 验证依据

本说明核对了以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；目标文件已索引为 20 个符号。
- RustCodeGraph `files --filter pkg/util/chunk/row_container_reader.rs`、`explore` 与 `node --file ... --offset 1 --limit 320`：确认目标源码全貌、符号定义，以及文件级使用边 `pkg/util/chunk/row_container.rs`、`pkg/server/internal/resultset/cursor.rs`。
- 源与 crate 边界：`pkg/util/chunk/row_container_reader.rs`、`pkg/util/chunk/row_container.rs:150-344`、`pkg/util/chunk/lib.rs:123-138`、`pkg/util/chunk/Cargo.toml`。
- Rust 上游与测试：`pkg/server/internal/resultset/cursor.rs:58-112`、`pkg/util/chunk/iterator_3_aster_unit_test.rs:221-272`；精确 Rust 引用检索未发现其他 `NewRowContainerReader` 调用。
- Go 对照与真实测试：`pkg/util/chunk/row_container_reader.go`、`pkg/util/chunk/row_container_test.go:338-441`、`pkg/server/conn_stmt.go:425-455`。

人工复核结论：本文件存在是为了把按块存储的 `RowContainer` 转成可取消、带背压且能传播读取错误的逐行流；运行核心是“后台按块取行、channel 逐行交付、前台维护当前行、关闭时取消并 join”；安全扩展必须同时维护首行预取协议、错误与结束的区分、worker 回收和 `SendRow` 的 unsafe 生命周期不变量。任务要求的结构校验命令应在文档落盘后单独执行并以退出码为最终结构证据。
