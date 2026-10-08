# `pkg/store/mockstore/unistore/lockstore/load_dump.rs`

## 文件定位

源文件：[`load_dump.rs`](load_dump.rs)。

本文件属于 Cargo crate `astersql-store-mockstore-unistore-lockstore`，为内存锁存储 `MemStore` 提供二进制持久化与恢复能力。crate 入口 `pkg/store/mockstore/unistore/lockstore/lib.rs` 以 `pub mod load_dump` 声明本模块，并通过 `pub use load_dump::*` 再导出这里的公开项；核心存储类型本身定义在同 crate 的 `lockstore.rs`，有序遍历能力定义在 `iterator.rs`。

它位于 mock TiKV 的 UniStore 锁存储边界，不负责锁的事务语义、跳表查找或 arena 分配，只把当前 `MemStore` 的有序键值快照编码到文件，或从相同格式的文件向一个可变 `MemStore` 填充数据。仓库文本搜索没有找到生产 Rust 调用者；当前可核实的直接使用点是同 crate 的独立测试 `migration_aster_unit_test.rs` 和 `load_dump_test.rs`，因此不能据现有证据声称它已接入生产启动/关闭主链。

## 核心职责

- 定义固定的小端长度编码：每一项均为 `4` 字节 little-endian `u32` 长度，紧随对应字节内容（`littleEndian::{Uint32, PutUint32}`）。
- 读取文件时先取一项作为不写入 `MemStore` 的 `meta`，随后按 `key`、`value` 两项一组调用 `MemStore::Put` 恢复内容（`MemStore::LoadFromFile`）。
- 转储时先写 `meta`，再通过 `MemStore::NewIterator` 按键序写出全部键值对；数据先进入 `<目标>.tmp`，完成 `flush`、`sync_all` 和关闭后再 `rename` 到目标路径（`MemStore::DumpToFile`）。
- 保留 Go 版本中可观察的特殊错误语义：文件成功打开后发生的读取或格式错误，会因 Go 命名返回值被延迟 `Close` 结果覆盖而最终表现为无数据、无错误；Rust 用 `Ok(None)` 模拟该结果（`LoadFromFile` 第 63—89 行及 `load_dump.go` 的 deferred close）。

## 主要符号

- `pub struct littleEndian`：无状态的小端编解码辅助类型。名称刻意沿用 Go 风格；crate 根允许 `non_camel_case_types`。
- `pub const endian: littleEndian`：模块级编解码实例，对应 Go 的 `var endian = binary.LittleEndian`。
- `littleEndian::Uint32(&self, [u8; 4]) -> u32`：把四字节长度头解为主机上的 `u32`。
- `littleEndian::PutUint32(&self, u32) -> [u8; 4]`：生成四字节小端长度头。
- `MemStore::LoadFromFile(&mut self, &str) -> io::Result<Option<Vec<u8>>>`：公开加载入口。`Some(meta)` 表示完整读到文件结尾，文件不存在或打开后的读取失败均返回 `None`；只有非 `NotFound` 的打开错误直接成为 `Err`。
- `MemStore::readItem<R: Read>(&self, &mut R, Option<Vec<u8>>) -> io::Result<Option<Vec<u8>>>`：私有通用读取器。干净地在下一项长度头之前遇到 EOF 时返回 `None`；长度头或内容被截断时返回 `UnexpectedEof` 等 I/O 错误；可复用传入 `Vec` 的容量。
- `MemStore::writeItem<W: Write>(&self, &mut W, &[u8]) -> io::Result<()>`：私有通用写入器，连续写长度头和内容。
- `MemStore::DumpToFile(&self, &str, &[u8]) -> io::Result<()>`：公开转储入口，以只读借用遍历存储并执行临时文件替换。

## 执行流程

加载流程由 `LoadFromFile` 驱动：

1. `File::open` 打开目标；`NotFound` 立即得到 `Ok(None)`，其他打开错误原样传播。
2. `BufReader` 上第一次调用 `readItem(None)` 读取元数据。空文件在内部先表现为缺失 metadata 的 `UnexpectedEof`。
3. 循环读取 key。若在新 key 的长度头开始前遇到 EOF，认为记录序列正常结束。
4. 每读到 key，必须再读到一个 value；否则构造“missing lockstore value”错误。完整键值对立即经 `Put` 写入当前存储。
5. 所有项完整时返回 `Ok(Some(meta))`；步骤 2—4 的任意错误最终被兼容层折叠为 `Ok(None)`。

转储流程由 `DumpToFile` 驱动：

1. 以 `create + truncate + read + write` 和 Unix 权限 `0600` 打开 `<fileName>.tmp`。
2. 使用 `writeItem` 写入 meta。
3. 建立 `NewIterator`，`SeekToFirst` 后按 `Valid`/`Next` 逐项写 key 和 value。迭代器依据跳表顺序运行，因此文件中的记录按键递增，而不是按插入顺序排列。
4. 刷新 `BufWriter`，通过 `into_inner` 取回文件并执行 `sync_all`，随后显式 drop 文件句柄。
5. `fs::rename` 用已落盘的临时文件替换目标；返回值就是整个操作的最终结果。

## 数据与状态

磁盘格式没有 magic、版本号、校验和或条目数：逻辑布局是 `item(meta), item(key1), item(value1), ...`，其中 `item(x) = u32_le(len(x)) + x`。空 meta、空 value 可编码；长度由 `usize` 直接转换为 `u32`，所以调用方必须保证单项长度不超过 `u32::MAX`，当前实现没有显式拒绝超长数据。

`readItem` 从文件提供的 `u32` 长度直接分配或扩容 `Vec`，没有大小上限。处理不可信文件时，恶意长度可造成高内存压力。key/value 缓冲在循环间复用容量，但 `MemStore::Put` 会把内容复制进 arena；返回的 meta 则由调用者拥有。

加载不是替换或事务操作：它不清空接收者，而是逐对调用 `Put`，因此会新增键或覆盖同名键。若后续记录损坏，之前成功插入的记录仍留在 `MemStore` 中，即使最终返回 `Ok(None)`；本文件没有回滚日志或临时存储。

## 依赖与调用关系

RustCodeGraph 核实的内部调用边为 `LoadFromFile -> readItem` 和 `DumpToFile -> writeItem`。源码还直接表明 `LoadFromFile -> MemStore::Put`，以及 `DumpToFile -> NewIterator -> SeekToFirst/Valid/Next`；图索引对跨模块同名/方法调用并未完整呈现，因此后两组以源文件为依据。

下游依赖如下：

- `std::fs::{File, OpenOptions, rename}`：文件打开、临时文件创建和最终替换。
- `std::io::{BufReader, BufWriter, Read, Write}`：缓冲 I/O 及可测试的泛型 item 编解码。
- `std::os::unix::fs::OpenOptionsExt`：设置 `0600`，也使本模块当前依赖 Unix API；源码没有非 Unix 条件编译分支。
- `super::lockstore::MemStore`：扩展核心存储类型；转储时还通过其在 `iterator.rs` 中的 inherent impl 获得迭代器。

`Cargo.toml` 中该功能没有额外运行时第三方依赖；crate 的 `rand` 用于 `MemStore` 本体，`tempfile` 是测试依赖。模块入口明确装配 `load_dump_test.rs` 和 `migration_aster_unit_test.rs` 为独立测试模块，符合测试不内嵌源文件的布局。

## 错误处理与边界

- 文件不存在是正常状态，返回 `Ok(None)`；权限、路径类型等其他 `File::open` 失败返回 `Err`。
- 成功打开后的空文件、截断长度头、截断内容、key 后缺 value 等错误都先由 `readItem`/闭包产生，却统一被 `LoadFromFile` 的最终 `match` 转为 `Ok(None)`。这是刻意对齐 Go deferred `Close` 覆盖命名返回错误的行为，不应误写成一般性的健壮错误处理。
- 损坏发生前已执行的 `Put` 不回滚；所以 `None` 同时可能伴随接收者的部分变更。现有测试覆盖空文件和一个 key 缺 value 时存储仍为空，也覆盖截断元数据，但未覆盖“先有完整记录、后有损坏记录”的部分写入情形。
- `readItem` 仅把“读取长度头第一个字节时就是 EOF”视为正常序列结束；读到 1—3 个长度字节再 EOF 是错误。这使正常结束与截断头可以区分。
- `DumpToFile` 的创建、编码、flush、取回 writer、sync 或 rename 任一步失败都会返回 `Err`。失败时 `.tmp` 可能保留，本文件没有清理逻辑。
- 原子性仅指同一路径所在文件系统内最终 `rename` 不暴露半写目标；没有目录 `fsync`，因此不能扩张为断电后目录项必然持久化的保证。临时名固定为 `<目标>.tmp`，并发 dump 同一目标会争用、截断同一临时文件。
- dump 写长度时 `data.len() as u32` 可能截断超大切片的长度；load 对声明长度无上限。演进格式时应先解决这两项边界。

## 并发与资源生命周期

`LoadFromFile` 需要 `&mut self`，类型系统排除了加载期间通过安全 Rust 同时借用同一 `MemStore`；每个完整记录在读到后立刻成为存储状态。`DumpToFile` 只需 `&self`，但其 `Iterator<'_>` 在整个遍历期持有同一只读借用。`MemStore` 本体声明 `Send + Sync`，底层设计是单写多读；本文件没有额外锁或一致性快照协议，因此不应把 dump 描述为能与外部写者并发产生事务一致快照。

转储资源顺序是 `BufWriter::flush -> into_inner -> File::sync_all -> drop(File) -> rename`。`into_inner` 还能报告缓冲区向底层文件刷写失败。加载依靠 `BufReader`/`File` 的 RAII drop 关闭；Rust 无法从普通 drop 获得 close 错误，本实现直接复刻 Go 在成功 close 时掩盖读取错误的最终返回值。

临时文件权限在 Unix 上设为 `0600`。成功 rename 后临时路径消失；测试 `migration_dump_load_round_trip_and_missing_file_match_go` 明确断言这一点。错误路径不保证删除临时文件，也没有锁定目标或生成唯一临时名。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/lockstore/load_dump.go`。Rust 保留了相同的 item 格式、meta 先行、key/value 成对、有序迭代、`0600` 临时文件、flush/sync/close/rename 顺序，以及缺失文件返回空结果的语义。

主要表达差异如下：

- Go 返回 `(meta []byte, err error)`；Rust 用 `io::Result<Option<Vec<u8>>>` 区分成功 meta、无 meta 和打开错误。
- Go `readItem` 用 `io.ReadFull`，在边界 EOF 也先返回错误；`LoadFromFile` 只把读取下一个 key 时的 `io.EOF` 当结束。Rust `readItem` 直接用 `None` 表示项边界 EOF，截断仍为错误，最终循环效果一致。
- Go 复用 `[]byte` 的 capacity；Rust 复用 `Option<Vec<u8>>` 的 allocation。两者写入存储前都会复用临时 key/value 缓冲，但 `Put` 承担持久复制。
- Go 的 deferred `f.Close()` 无条件覆盖命名返回 `err`，导致打开成功后的读取错误在正常 close 时消失。Rust 在读取闭包失败时返回 `Ok(None)` 明确模拟这一现有行为；`load_dump_test.rs` 与 migration 测试对此有注释和断言。
- Go 记录加载/转储条目数日志；Rust 只累加 dump 的 `cnt` 后丢弃，也没有 load 计数日志。计数不影响文件或存储语义，但可观测性并未完全移植。
- Rust 使用 `std::os::unix` 设置权限，当前没有 Go 所具有的跨平台 `os.OpenFile` 抽象范围。

## 扩展指南

- 修改磁盘格式应集中在 `readItem`/`writeItem` 及两个公开入口，并同步更新 Go `load_dump.go`。由于现格式无版本标记，加入 magic/version 时必须设计旧文件探测与向后兼容，不能直接改变首项含义。
- 若要拒绝超大项，应在 `writeItem` 的 `usize -> u32` 前检查，在 `readItem` 分配前设置对称上限，并在独立 `load_dump_test.rs` 增加超长/恶意长度用例。
- 若要修正“读取错误变 `None`”或增加事务式加载，属于跨语言可观察行为变更：需要决定是否同时修 Go，对空文件、截断头、截断值以及“完整前缀 + 损坏尾部”分别加回归测试，并明确已有接收者内容是否保留。
- 若要支持并发或多进程 dump，同一固定 `.tmp` 名不够安全；应设计唯一临时名、冲突策略与清理路径，并验证 rename 的平台语义。若要求崩溃持久性，还需评估父目录同步。
- 若要支持非 Unix 目标，应将 `OpenOptionsExt::mode` 隔离到条件编译实现，并为权限/替换行为提供平台独立契约。
- 测试应继续放在独立文件：格式、I/O 错误和临时文件生命周期优先扩展 `load_dump_test.rs`；Go/Rust 移植一致性场景可扩展 `migration_aster_unit_test.rs`，不要把测试模块写入 `load_dump.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/store/mockstore/unistore/lockstore` 确认模块文件集合；`node --file .../load_dump.rs` 阅读了完整 158 行；`query` 定位 `LoadFromFile`、`DumpToFile`、`readItem`、`writeItem`、`littleEndian`；`callees` 核实两条核心内部调用边。`callers` 未给出公开方法调用者，因此又以仓库文本搜索确认直接使用点。
- 目标源码：`pkg/store/mockstore/unistore/lockstore/load_dump.rs`，核实格式、错误折叠、缓冲复用和落盘顺序。
- crate 与模块边界：`pkg/store/mockstore/unistore/lockstore/Cargo.toml`、`lib.rs`；该目录没有 `doc.go`。
- 核心直接依赖：RustCodeGraph 读取 `lockstore.rs` 的 `MemStore`/`Put`/`Send`/`Sync` 定义，以及 `iterator.rs` 的 `Iterator`/`NewIterator`/遍历方法。
- Go 对照：`pkg/store/mockstore/unistore/lockstore/load_dump.go`，核实 wire format、缓冲复用、日志和 deferred close 行为。
- 独立 Rust 测试：`pkg/store/mockstore/unistore/lockstore/load_dump_test.rs` 覆盖读取错误被成功 close 结果覆盖；`migration_aster_unit_test.rs` 覆盖 dump/load 往返、键值恢复、缺失文件、临时文件消失与截断输入。
- 仓库搜索：`rg` 仅在上述 Rust 测试中发现 `LoadFromFile`/`DumpToFile` 的 Rust 调用，未发现可确认的生产调用；这也是本文将接线状态限定为“测试可核实”的依据。
