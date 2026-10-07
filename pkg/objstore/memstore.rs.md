# `pkg/objstore/memstore.rs`

## 文件定位

`memstore.rs` 是 `astersql-objstore` crate 的进程内对象存储后端。crate 入口 `pkg/objstore/lib.rs` 以公开模块 `pub mod memstore` 暴露它；它实现 `pkg/objstore/storage.rs` 定义的 `Storage`、`ObjectReader` 和 `ObjectWriter` 抽象，因此可以作为 `StorageRef = Arc<dyn Storage>` 进入统一对象存储调用链。

生产侧的直接接线位于 `storage.rs`：`NewFromURL` 遇到 `memstore://` 时直接构造 `NewMemStorage()`，`New` 遇到 `StorageBackend::MemStore` 时也返回该实现。它不访问磁盘或云服务，数据只存在于当前进程内；`URI` 固定返回 `memstore://`。仓库中它主要用于测试、内存替身和无需持久化的轻量场景，不能据此推断数据能够跨进程或跨重启保存。

所属 crate 由 `pkg/objstore/Cargo.toml` 的 `[package] name = "astersql-objstore"` 和 `[lib] path = "lib.rs"` 确认。此文件直接使用标准库同步原语及 `anyhow`；Cargo 清单没有为内存后端设置独立 feature。

## 核心职责

- `MemStorage` 用以对象名为键的哈希表保存 `MemFile`，实现整对象写入、读取、存在性检查、删除、重命名、遍历、流式创建和范围读取。
- `MemFile` 把某一对象的当前内容保存为 `RwLock<Arc<Vec<u8>>>`。替换内容时发布新的 `Arc<Vec<u8>>`，读取时取得当前快照。
- `MemFileReader` 将打开时的内容复制进独立 `Cursor<Vec<u8>>`，实现带 `end` 上界的 `Read`、任意 `Seek`、关闭和完整文件大小查询。
- `MemFileWriter` 先把各次 `write` 追加到私有 `Vec<u8>`，直到 `close` 才一次性发布给 `Create` 时插入的 `MemFile`。
- `WalkDir` 在短暂持有存储读锁时筛选并复制键名，排序后释放锁，再逐项取当前文件并调用回调，避免在用户回调期间持有全局锁。

该实现的主要不变量是：外部传入或取出的字节不会与存储内部缓冲共享可变所有权；已打开 reader 不受后续覆盖或删除影响；`Create` 创建的对象立即存在但在 writer 关闭前内容为空。

## 主要符号

- `struct MemFile { data: RwLock<Arc<Vec<u8>>> }`：单对象容器。私有方法 `load` 克隆 `Arc`，`store` 用新 `Arc` 原子式地替换锁保护下的当前值；锁中毒通过 `expect` 触发 panic。
- `pub struct MemStorage { data_store: RwLock<Option<HashMap<String, Arc<MemFile>>>> }`：公开存储类型。`Some(map)` 表示可用，`None` 表示已经 `Close`；私有 `load_file` 在读锁内克隆指定文件的 `Arc`。
- `pub fn NewMemStorage() -> MemStorage`：唯一公开构造函数，创建空的 `Some(HashMap)`。命名保留 Go API 形状。
- `fn go_path_base(&str) -> &str`：模拟 Go 的斜杠路径 basename 规则；空串为 `.`，全斜杠根路径为 `/`，其余先去除末尾 `/` 再取最后一段。`WalkDir` 和 `PresignFile` 共用该规则。
- `impl Storage for MemStorage`：实现 `as_any`、`DeleteFile`、`WriteFile`、`ReadFile`、`FileExists`、`Open`、`WalkDir`、`URI`、`Create`、`Rename`、`PresignFile`、`Close` 和 `is_strong_consistent`；批量删除和不支持的跨存储复制沿用 trait 默认实现。
- `struct MemFileReader`：字段 `cursor` 保存独立内容，`position` 与 `end` 控制范围读，`size` 保存打开时完整对象长度，`closed: AtomicBool` 管理关闭状态。它实现标准库 `Read`、`Seek` 及 `ObjectReader`。
- `struct MemFileWriter`：字段 `buffer` 是未提交数据，`file` 指向目标 `MemFile`，`closed: AtomicBool` 拒绝关闭后的写入。它实现 `ObjectWriter`。

公开 API 是 `MemStorage` 与 `NewMemStorage`；reader、writer、单文件容器和路径辅助函数均为模块内部细节。本文件没有条件编译项、模块级常量或自定义 trait。

## 执行流程

1. 构造：`NewMemStorage` 建立空哈希表。统一工厂可从 `memstore://` 或 `StorageBackend::MemStore` 进入这一构造函数。
2. 整对象写入：`WriteFile` 先检查 `Context`，取得全局写锁；已有键调用 `MemFile::store` 替换内容，新键则创建并插入 `MemFile`。输入切片通过 `to_vec` 复制。
3. 整对象读取：`ReadFile` 检查取消，通过 `load_file` 克隆文件句柄，再从 `MemFile::load` 取得内容快照并复制成新的 `Vec<u8>` 返回。调用方修改返回值不会改变存储内容。
4. 流式写入：`Create` 检查取消后立即以一个空 `MemFile` 覆盖同名键，并返回绑定该文件的 `MemFileWriter`。`write` 只追加私有 buffer；`close` 检查取消，将整个 buffer 克隆并发布到该文件，然后标记 writer 已关闭。
5. 范围读取：`Open` 检查取消和对象存在性，取得内容快照，应用 `ReaderOption` 的起止偏移，将游标 seek 到 `start`，并保存完整长度。每次 `read` 最多读取 `end - position` 个字节；`Seek` 更新绝对位置但不把位置强制限制在原范围内。
6. 遍历：`WalkDir` 在全局读锁下按 `sub_dir` 的完整键前缀和 `obj_prefix` 的 basename 前缀筛选键，复制后排序；随后逐键检查取消、重新查找对象、读取当前大小并调用回调。遍历开始后新增的键不出现，已删除的键被跳过，仍存在但被改写的键以上报时的当前长度为准。
7. 删除与重命名：两者检查取消后持有全局写锁。删除不存在对象报错；重命名先移除源键，再插入目标键，因此目标已存在时被覆盖，源不存在时不改变目标。
8. 关闭：`Close` 将整个 `data_store` 改为 `None` 并丢弃 map。此后写入、删除、创建和重命名返回 `mem storage closed`；读取表现为找不到文件，存在性检查为 `false`，遍历为空。`URI`、`PresignFile` 和已经打开或创建的独立 reader/writer 不依赖 map。

## 数据与状态

状态层次为 `MemStorage -> HashMap<String, Arc<MemFile>> -> RwLock<Arc<Vec<u8>>>`。对象名原样作为键，不做路径清理；所谓目录仅由字符串前缀解释。存储没有容量限制、持久化格式、时间戳、对象元数据或目录节点。

`Option<HashMap<...>>` 同时承担生命周期标记：`Some` 是开启，`None` 是关闭。没有重新打开操作，重复 `Close` 仍保持 `None`。`MemFile::default` 的内容是空 `Vec<u8>`，所以 `Create` 后、writer `close` 前，读者能够看到一个长度为零的对象。

读取有两种快照粒度：`ReadFile` 返回当前内容的拥有型副本；`Open` 也复制打开时内容，因此之后的 `WriteFile`、`Rename` 或 `DeleteFile` 不会改变该 reader。`WalkDir` 只快照键名，不快照内容或大小。`Create` 替换的是 map 中的 `Arc<MemFile>`；若同一名字再次 `Create`，旧 writer 之后关闭只会更新已脱离 map 的旧 `MemFile`，不会覆盖新对象。

范围状态采用绝对偏移 `[position, end)`。`get_file_size` 始终返回打开时完整对象长度，而非范围长度。`end` 小于当前位置时读取零字节；`end` 超过文件长度时最终由 `Cursor` 的 EOF 截断。代码只显式拒绝负 `start`，没有校验负 `end`、`start <= end` 或 `end <= size`。

## 依赖与调用关系

上游关系：

- `pkg/objstore/lib.rs` 声明公开 `memstore` 模块，并在测试配置下将 `memstore_test.rs` 作为独立测试模块接入。
- `pkg/objstore/storage.rs::NewFromURL` 对 `memstore://` 直接返回 `Arc::new(NewMemStorage())`；`storage.rs::New` 的 `StorageBackend::MemStore` 分支执行相同构造。这是完整应用通过统一工厂选择该后端的生产接线。
- 包内 `helper_test.rs`、`locking_test.rs`、`storage_test.rs`、`helper_2_aster_unit_test.rs` 以及若干跨包 Rust 测试把它作为真实 `Storage` 的无外部服务实现；这些是使用证据，不应误写为额外生产工厂。

下游关系：

- 接口和选项来自 `pkg/objstore/storage.rs`：`Context`、`Storage`、`ObjectReader`、`ObjectWriter`、`ReaderOption`、`WalkOption`、`WriterOption`。
- `anyhow::{Result, anyhow}` 统一承载取消、对象不存在、关闭及 writer 状态错误，并自动接收 `std::io::Error`。
- `HashMap` 提供对象索引；`Arc` 保证对象/内容快照可在解锁后继续存活；`RwLock` 区分并发查询与结构性更新；`AtomicBool` 保存 reader/writer 关闭标记；`Cursor<Vec<u8>>` 实现内存 seek/read。

RustCodeGraph 将 `memstore.rs::NewMemStorage` 与 `memstore.rs::go_path_base` 识别为函数，但对其 `callers/callees` 未输出边；因此上述调用边以索引符号结果、`storage.rs` 精确引用和模块入口交叉核实，而不是根据同名 `MemStorage` 推断。

## 错误处理与边界

- 除 `URI`、`PresignFile`、`Close` 和 reader 自身操作外，存储读写入口会调用 `Context::check_cancelled`；`WalkDir` 在每一项回调前检查，而不是在获取键快照前检查。writer 的 `write` 与 `close` 也检查取消。
- `DeleteFile`、`ReadFile`、`Open` 对不存在对象返回带对象名的错误；`Rename` 对不存在源返回错误。`FileExists` 把不存在作为正常的 `false`。
- 用户提供的 `WalkDir` 回调错误通过 `?` 原样向上传播并终止遍历；`Cursor::seek/read` 的 I/O 错误同样传播。
- reader 关闭后，`read` 返回 `Ok(0)`，`seek` 返回 `reader closed`；`get_file_size` 仍可调用。writer 关闭后再次 `write` 返回 `writer closed`；再次 `close` 会重新发布同一 buffer，并未显式拒绝。
- 所有 `RwLock` 获取都使用 `expect`。若持锁线程 panic 导致锁中毒，后续操作会 panic，而不是返回 `anyhow::Error`。
- `PresignFile` 不是实际签名操作，只返回 Go basename 兼容值且忽略 context 与 duration；不能把结果当作可访问 URL。
- 当前 `WalkDir` 只解释 `WalkOption.sub_dir` 与 `obj_prefix`，忽略 `skip_sub_dir`、`include_tombstone` 和 `start_after`。删除了键只会跳过，不会产生 `TombstoneSize` 回调。
- `is_strong_consistent` 返回 `true`，含义限于该进程内由锁与快照定义的可见性，不代表分布式持久化一致性。

## 并发与资源生命周期

`MemStorage` 的 map 由全局 `RwLock` 保护，`MemFile` 的内容由独立 `RwLock` 保护。对象查找先克隆 `Arc<MemFile>` 再释放全局锁，所以长时间读取或已打开的 reader 不阻塞 map 的删除/重命名。结构更新串行化，但不同对象内容的读取和替换不必一直持有全局锁。

`WalkDir` 特意在执行回调前释放全局锁。`pkg/objstore/memstore_test.rs::test_mem_store_write_during_walk_dir` 证明遍历回调阻塞期间仍可完成写入和删除；测试还证明遍历使用起始键集快照，并对被删键采用跳过语义。回调本身在调用线程同步运行，没有内部线程、任务或通道。

reader/writer 的 `closed` 使用 Acquire/Release 原子访问，但对象本身通过 `&mut self` 调用读写方法，原子标记主要对齐 Go 状态语义，并不是共享同一个可变 reader/writer 的同步许可。writer buffer 也没有锁。`Arc<MemFile>` 使已打开 writer 或文件句柄能越过 map 项删除及存储关闭继续存活；`Close` 不主动关闭或撤销这些句柄，也没有 `Drop` 中的隐式提交。

资源完全由 Rust 所有权释放：map、对象内容、游标和缓冲在最后一个所有者离开作用域时释放。没有文件描述符、网络连接、后台 worker 或显式内存回收策略；大量或大型对象会直接占用进程内存。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/objstore/memstore.go`，行为测试是 `pkg/objstore/memstore_test.go`；Rust 的对应独立测试位于 `pkg/objstore/memstore_test.rs`。

主要保持一致之处：Go 的 `map + RWMutex` 对应 Rust 的 `RwLock<Option<HashMap<...>>>`；Go `atomic.Pointer[[]byte]` 的整内容替换对应 Rust `RwLock<Arc<Vec<u8>>>`；两者都复制整对象输入/输出、按排序后的键快照遍历、在 `Create` 时插入空对象并在 writer 关闭时提交、允许 Rename 覆盖目标、让已打开 reader 在对象删除后继续读取，并固定返回 `memstore://`。`go_path_base` 专门保持 Go `path.Base/filepath.Base` 对空串和根路径的结果，相关 Rust 测试覆盖 `.` 与 `/`。

可观察差异与迁移边界：

- Rust `NewMemStorage` 返回值而非指针，通常由调用方再包进 `Arc`；Go 返回 `*MemStorage`。
- Go `Close` 把 map 设为 `nil`，其后某些写操作可能触发 Go 的 nil-map 行为；Rust 用 `None` 明确让结构性写操作返回 `mem storage closed`，读取则呈现为空/不存在。
- Go reader 在关闭或达到范围末端时返回 `io.EOF`；Rust `Read` 按 Rust 约定返回 `Ok(0)`。两边测试分别断言各自接口惯例。
- Rust `Open` 显式拒绝负 `start`，并把打开时数据复制进 `Cursor<Vec<u8>>`。Go `bytes.Reader` 引用当时的字节切片；因 Go 写入也以新切片替换，所以同样保持已打开 reader 的内容稳定。
- Go 文件显式实现 `DeleteFiles`；Rust 使用 `Storage::DeleteFiles` 的顺序默认实现，仍是逐项删除、首错返回。
- Rust `WalkOption` 比这段 Go 实现的当前使用面包含更多字段，但本实现与 Go `memstore.go::WalkDir` 一样只处理 `SubDir/sub_dir` 和 `ObjPrefix/obj_prefix`。

## 扩展指南

- 若新增对象级操作，优先通过 `Storage` trait 增加或复用统一入口，再在 `impl Storage for MemStorage` 落实；同时检查本地、noop 和云后端是否需要同样语义，避免内存后端形成私有契约。
- 若改变读写可见性，重点审查 `MemFile::load/store`、`WriteFile`、`Create` 和 `MemFileWriter::close`。必须决定未关闭 writer、同名多个 writer、覆盖/删除后的既有 reader 应看到什么，并在独立的 `pkg/objstore/memstore_test.rs` 增加回归测试，不能把测试嵌入生产源文件。
- 若补齐 `WalkOption.skip_sub_dir/include_tombstone/start_after`，修改点是 `WalkDir` 的快照筛选和删除竞态处理；需与 `storage.rs::TombstoneSize` 及其他后端语义核对，并覆盖排序、边界键、回调错误和并发删除。直接在回调期间持有 map 锁会破坏当前允许回调重入存储及并发写入的性质。
- 若扩展范围读取校验，应在 `Open` 构造 reader 前统一定义负 end、反向范围和超文件末尾的行为，并同步 Go 实现/测试或明确兼容差异。注意 `Seek` 当前可离开初始范围，`end` 仍是绝对上界。
- 若改变关闭语义，应分别覆盖存储关闭、reader 关闭、writer 关闭和存储关闭前取得的句柄；当前四者并非联动。若需要可恢复错误，应避免新增 `expect` 锁获取，并评估既有锁中毒策略。
- 若优化大对象内存，当前至少存在输入复制、`Arc<Vec<u8>>` 内容和 `Open` 的游标复制；任何零拷贝方案都必须保留外部修改隔离与已打开 reader 快照语义。
- Go 对照变更时同步检查 `pkg/objstore/memstore.go` 与 `memstore_test.go`；Rust 生产代码变更后按仓库规则更新独立的 `memstore_test.rs`，并执行适用验证。本说明任务本身不运行 Cargo。

## 验证依据

- 源码全貌：`pkg/objstore/memstore.rs`，核对 `MemFile`、`MemStorage`、`NewMemStorage`、`go_path_base`、完整 `Storage` 实现、`MemFileReader` 和 `MemFileWriter`；文件无条件编译项。
- crate 与接线：`pkg/objstore/Cargo.toml`（crate 名、lib 路径、`anyhow` 依赖、无专属 feature），`pkg/objstore/lib.rs`（公开模块和独立测试模块），`pkg/objstore/storage.rs`（trait 契约、选项、`NewFromURL` 与 `New` 两条构造路径）。
- Go 对照：`pkg/objstore/memstore.go` 与 `pkg/objstore/memstore_test.go`，核对锁、拷贝、Create/Close、range reader、排序遍历、重命名和并发修改语义。
- Rust 测试：`pkg/objstore/memstore_test.rs`。`test_mem_store_basic` 覆盖基本 IO、Create 延迟提交、reader 快照和 Rename 覆盖；`test_mem_store_open_range_seek` 覆盖范围与 seek；`test_mem_store_walk_dir` 和 `test_mem_store_walk_dir_root_basename` 覆盖筛选与路径边界；`test_mem_store_presign_file_go_basename_edges` 覆盖 Go basename；`test_mem_store_manipulate_bytes` 覆盖拷贝隔离；`test_mem_store_write_during_walk_dir` 覆盖键快照与回调期间并发修改。
- RustCodeGraph：`status` 显示索引包含 `pkg/objstore/memstore.rs`；`files --filter pkg/objstore` 将其列为含 37 个符号的 Rust 文件；`query NewMemStorage --kind function --json` 定位 `memstore.rs::NewMemStorage`（第 58 行），`query go_path_base --kind function --json` 定位 `memstore.rs::go_path_base`（第 76 行）。`explore/node/callers/callees` 对本文件未返回正文或边，因此调用关系另由上述源码精确引用与 `rg` 交叉验证，未把同名符号当作证据。
- 结构验收使用任务指定命令，要求文件存在且固定二级标题恰好为 11 个；本任务是纯文档分析，未运行 Cargo 或代码测试。
