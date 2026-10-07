# `pkg/objstore/local.rs`

## 文件定位

`pkg/objstore/local.rs` 是 `astersql-objstore` crate 的本地文件系统后端，实现 `file://` 对象存储。crate 入口 `pkg/objstore/lib.rs` 以 `pub mod local` 暴露本模块；通用构造器 `pkg/objstore/storage.rs::New` 在收到 `StorageBackend::Local` 时调用 `NewLocalStorage`，并把结果包装成 `Arc<dyn storage::Storage>`。此外，`pkg/executor/importer/production_storage.rs::ServerDiskImportStorageFactory::Open` 直接用它打开服务器磁盘上的 `IMPORT INTO` 数据源父目录。

文件同时实现两套接口：仓库内的兼容层 `crate::storage::Storage`，以及独立 crate `storeapi::Storage`。后一套实现通过 `StoreapiLocalReader`、`StoreapiLocalWriter` 把前一套的读写器适配为 `objectio::Reader` / `objectio::Writer`，使同一个 `LocalStorage` 可服务仍依赖旧抽象和已经迁移到新抽象的调用方。依赖边界由 `pkg/objstore/Cargo.toml` 中的 `objectio`、`storeapi` 路径依赖及 `uuid`、`walkdir` 外部依赖确认。

## 核心职责

- `NewLocalStorage` 创建或复用根目录，并初始化本地后端；`Base` 和 `URI` 暴露根目录的普通路径及 `file://` 表示。
- `object_path` 将对象名约束到 `base` 下。它主动移除对象名开头的 Windows prefix 或根目录组件，以模拟 Go `filepath.Join(base, name)` 的效果，避免 Rust `PathBuf::join` 遇到绝对路径时丢弃 `base`。
- `WriteFile` 提供整对象原子替换：写入带 UUID 的同目录临时文件，再 `rename` 到目标；`ReadFile`、`FileExists`、`DeleteFile`、`Rename` 提供直接文件操作。
- `Open` 和 `LocalFile` 提供可 seek 的半开字节范围读取；`Create`、`create_buffered` 和 `LocalWriter` 提供覆盖式缓冲流写入。
- `WalkDir` 以稳定文件名顺序列举对象，支持 `sub_dir`、`obj_prefix`、`skip_sub_dir`、`start_after` 和 tombstone，并处理软链接及断裂软链接的大小。
- `CopyFrom` 在两个 `LocalStorage` 之间以硬链接复制；`is_strong_consistent` 声明本地文件系统后端为强一致。

本文件不是云对象存储模拟器：`PresignFile` 只返回 basename，`Close` 没有连接资源可释放，`CopyFrom` 也不执行跨后端字节复制。

## 主要符号

- `localFilePerm: u32 = 0o644`：`WriteFile` 创建临时文件时使用的 Unix mode；在非 Unix 平台 `write_file_with_mode` 不设置 mode。
- `LocalURIPrefix: &str = "file://"`：`URI` 拼接根路径时使用的公开前缀。
- `LocalStorage { base, IgnoreEnoentForDelete }`：核心存储对象。`base` 私有，避免调用方绕过路径映射；公开开关 `IgnoreEnoentForDelete` 允许删除不存在对象时成功返回。
- `LocalStorage::object_path`：所有对象名到本地路径的统一映射点；绝对对象名仍落在 `base` 下，但它没有清理 `..` 组件，因此它不是面向不可信对象名的目录穿越防护器。
- `LocalStorage::create_buffered`：创建父目录、覆盖目标文件，并根据可选 `part_size` 创建 `BufWriter<File>`。正数容量至少为 16；旧 `storage::WriterOption` 没有字段，故旧 trait 的 `Create` 使用默认容量，新 `storeapi::WriterOption::PartSize` 才会传入容量。
- 两个 `impl Storage for LocalStorage`：旧接口实现完整本地能力；新接口先执行 `storeapi::Context::check`，转换 reader/walk 选项，再委托旧接口。`storeapi::WalkOption::ListCount` 与 `ReaderOption::PrefetchSize` 在本地适配中没有对应字段，因而不参与行为。
- `StoreapiLocalReader` / `StoreapiLocalWriter`：私有桥接器。reader 转发 `Read`、`Seek`、关闭及文件大小；writer 每次写和关闭前检查新接口 context，再以默认旧 context 调用 `ObjectWriter`。
- `LocalFile { file, position, end_position, closed }`：范围 reader。`end_position == -1` 表示无限制；有限范围按 `end_position - position` 限制每次读取长度。
- `LocalWriter { writer, closed }`：缓冲 writer。`close` flush 后标记关闭；关闭后的显式 `write` 返回 `writer closed`。
- `pathExists`、`write_file_with_mode`、`slash_path`、`should_skip_local_subtree`：分别封装存在性判断、带权限写入、跨平台对象名规范化，以及基于 `start_after` 的整棵子树剪枝。
- `NewLocalStorage`：公开构造入口；缺失根目录通过平台实现 `local_unix::mkdirAll` 或 `local_windows::mkdirAll` 创建。

## 执行流程

构造与接线流程如下：

1. 上层可经 `storage::New` 的 `StorageBackend::Local` 分支，或生产导入工厂直接调用 `NewLocalStorage`。
2. `NewLocalStorage` 用 `pathExists` 区分“存在”“不存在”和其他 I/O 错误；仅不存在时调用平台 `mkdirAll`。
3. 返回的 `LocalStorage` 默认不忽略删除时的 `NotFound`。旧接口调用方直接使用 `crate::storage::Storage`；新接口调用方通过同一类型上的 `storeapi::Storage` 实现进入适配逻辑。

整对象写入 `WriteFile` 的流程是：`object_path` 解析目标，目标字符串后附加 `.tmp.<uuid>`；首次写临时文件失败时检查父目录，若父目录确实不存在则 `mkdirAll` 后重试，否则保留首次错误；临时文件完整写成后执行 `fs::rename`。临时文件与目标位于同一路径前缀下，使常规本地文件系统上的 rename 能让读者看到旧对象或完整新对象，而不是中间内容。

流式写入走 `Create -> create_buffered -> LocalWriter`，直接创建/截断目标文件，不使用临时文件，因此不具备 `WriteFile` 的原子可见性。数据先进入 `BufWriter`；只有缓冲区填满、底层写入发生、显式 `close` flush，或对象 drop 时，数据才可能落到底层文件。新 `storeapi` 入口会把 `PartSize` 映射为缓冲容量并包装 `StoreapiLocalWriter`。

范围读取走 `Open`：打开文件，默认 `position=0`、`end_position=-1`；有 `start_offset` 时先拒绝负数，再 seek 到起点；有 `end_offset` 时保存为不包含的上界。`LocalFile::read` 在有限范围中最多读取剩余字节，到达或越过上界后返回 `Ok(0)`；`Seek` 同步底层位置与内部 `position`。新接口再用 `StoreapiLocalReader` 转换错误类型和方法名。

目录遍历流程是：从 `base/sub_dir` 得到遍历根；根不存在时按 `include_tombstone`、`.` 前缀匹配和 `start_after` 决定是否回调一次 `TombstoneSize`；根存在时用 `WalkDir::sort_by_file_name` 稳定排序，`skip_sub_dir` 将最大深度设为 1，`filter_entry` 用 `should_skip_local_subtree` 跳过游标已越过的目录。每个非目录项先按相对遍历根应用 `obj_prefix`，再按相对存储根应用 `start_after`。普通文件使用 `symlink_metadata` 的长度；软链接尝试跟随目标取长度，解析失败则报告 0。

## 数据与状态

`LocalStorage` 的长期状态只有不可变的根路径和可由持有者设置的删除策略开关。类型没有内部锁或缓存；同一实例的 trait 方法接收 `&self`，并依赖文件系统协调并发访问。`StorageRef = Arc<dyn Storage>` 负责共享所有权，而非本文件内部的引用计数。

`LocalFile` 的 `position` 是范围读取上界计算所需的镜像状态。无上界读取也会更新它，以便随后 seek 或其他路径保持一致；`closed` 是逻辑关闭标记，关闭后读取返回 0，但底层 `File` 句柄直到 reader 被丢弃才由 RAII 真正关闭。`get_file_size` 查询底层 metadata，不返回剩余范围长度。

`LocalWriter` 的 `BufWriter<File>` 同时拥有缓冲与文件句柄，`closed` 只阻止后续显式写入。`close` 会先 flush，flush 失败时不会执行 `closed = true`，允许调用方观察错误并决定是否重试。没有显式 close 时，Rust drop 仍会尝试刷新 `BufWriter`，但 drop 无法向调用方报告刷新错误，因此需要可靠错误传播的调用方必须调用 `close`。

遍历本身不保存快照：目录项从实时文件系统获得，文件可能在遍历与 metadata 查询之间变化。不存在的遍历根可产生尺寸为 `TombstoneSize`（定义于 `storage.rs`，值为 -1）的墓碑；遍历中遇到的断裂软链接则以尺寸 0 区分。

## 依赖与调用关系

上游直接关系：

- `pkg/objstore/lib.rs` 声明 `pub mod local`，并把 `pkg/objstore/local_test.rs` 作为独立测试模块编入 crate。
- `pkg/objstore/storage.rs::New` 的 `StorageBackend::Local` 分支调用 `NewLocalStorage`，形成 URI/Backend 解析后的标准入口；其返回类型是旧接口 `StorageRef`。
- `pkg/executor/importer/production_storage.rs::ServerDiskImportStorageFactory::Open` 对绝对服务器路径取父目录后直接调用 `NewLocalStorage`，再包装为导入流程需要的共享存储。
- RustCodeGraph 对 `local.rs` 的文件级反向关系还显示 `pkg/dumpformat/sqlfile/writer_test.rs`、`pkg/dxf/importinto/conflict_resolution_test.rs`、`pkg/executor/importer/precheck_test.rs`、`pkg/lightning/config/config_test.rs` 使用本文件；这些是集成/测试调用面，不是本文件内部实现。

下游直接关系：

- 标准库 `std::fs` / `File` / `OpenOptions` 完成真实 I/O，`BufWriter` 完成流式写缓冲，`Path` / `PathBuf` 完成路径映射。
- `uuid::Uuid::new_v4` 为 `WriteFile` 生成并发友好的临时文件名；`walkdir::WalkDir` 提供递归、排序和目录剪枝。
- `crate::storage` 提供旧 `Storage`、reader/writer traits、选项、共享引用和 tombstone 常量；`objectio` 与 `storeapi` 提供新接口。
- `local_unix::mkdirAll` 临时清零并恢复进程 umask，以 0o777 递归建目录；Windows 版本直接 `create_dir_all`。`local.rs` 通过条件编译只选择当前平台实现。

RustCodeGraph 的精确符号 trail 确认：`create_buffered` 被本文件两套 `Create` 调用，并调用 `object_path`、实例化 `LocalWriter`；`should_skip_local_subtree` 只由 `WalkDir` 调用；`StoreapiLocalReader`、`StoreapiLocalWriter` 分别由新接口 `Open`、`Create` 实例化；`LocalFile` 由旧接口 `Open` 实例化；`NewLocalStorage` 调用 `pathExists`、平台 `mkdirAll` 并实例化 `LocalStorage`。

## 错误处理与边界

- 对象路径操作保留底层 I/O 错误；`DeleteFile` 额外加入对象名上下文。仅当开关为真且错误类型是 `NotFound` 时删除不存在对象成功。
- `WriteFile` 的首次错误是目录补建分支的主错误：父目录检查或创建失败时把次级原因附到首次错误；父目录已存在则直接返回首次错误。第二次写入和最终 rename 的错误直接传播。rename 失败时临时文件不会在本函数中清理，这是调用方或运维可见的残留风险。
- `Open` 明确拒绝负 `start_offset`；`end_offset` 没有单独负数检查，且 `end <= current position` 时 reader 立即返回 EOF 形状的 `Ok(0)`。起点大于文件大小由底层 seek 接受，后续读取为空。
- 旧接口方法忽略传入的 `crate::storage::Context`，因此不会主动响应该 context 的取消；新 `storeapi` 方法在进入操作前检查 context，`WalkDir` 每次回调前再次检查，writer 每次写/关闭前检查。reader 创建后不持有 context，后续 `read` 不会因创建时的 context 被取消而中断。
- 新接口适配把底层 `anyhow::Error` 转为 `io::Error::other`，保留文字信息但不承诺原始具体 `io::ErrorKind`。
- `WalkDir` 会传播 walk、相对路径、metadata 和用户 callback 的错误；软链接目标 metadata 的任何错误被折叠为 size 0。根目录不存在且未请求 tombstone 时正常返回空结果。
- `CopyFrom` 先通过 `Any` downcast 限制来源必须是 `LocalStorage`，否则返回 `expect source to be LocalStorage`；硬链接还要求底层文件系统支持且源目标通常位于同一文件系统，目标已存在等错误直接传播。
- `Rename` 不创建目标父目录；`Create` 和 `CopyFrom` 会创建父目录。`PresignFile` 对没有 basename 的输入返回空字符串，而不是 URL。
- `object_path` 只去掉绝对路径根/prefix，不消解或拒绝 `..`；若对象名来自不可信输入，上层必须另行校验路径边界。

## 并发与资源生命周期

`LocalStorage` 可作为 `Storage: Send + Sync` 在多线程之间共享，但本文件没有串行化锁。不同对象的 I/O 可并发；同一对象的并发语义交给文件系统。`WriteFile` 用随机 UUID 临时名避免不同写者竞争同一个临时文件，最终 rename 的胜者决定最后可见内容。它不执行 `fsync`，所以“原子”描述的是命名空间可见性，不等价于断电持久性。

`Create` 会立即创建并截断目标，同一对象上的并发 reader 或 writer 可能观察正在增长的文件。`LocalWriter::close` flush 缓冲但不显式 sync；writer 和 reader 的 OS 句柄在结构体 drop 时关闭。`LocalFile::close` 仅设置标记，适配后的 `objectio::Reader::close` 也沿用这一逻辑。

`WalkDir` 是同步、单线程、回调驱动的；回调在遍历线程中执行，慢回调会阻塞遍历。它不持锁，故文件在列举、取 metadata 或回调前后均可能被创建、删除或替换；tombstone 与软链接 size=0 是对部分竞态/断链情形的兼容表达，而非一致性快照。

Unix `mkdirAll` 为对齐 Go 行为会暂时修改进程级 umask，并用 RAII guard 恢复。进程级 umask 本质上是全局资源；虽然 guard 保证正常返回和 unwind 时恢复，本文件没有为并发调用增加互斥保护，这是新增并发目录创建逻辑时需要特别评估的边界。

## 与 Go 版本的对应关系

主要行为对应 `pkg/objstore/local.go`：`LocalStorage` 字段、`Base`、删除策略、UUID 临时文件加 rename、整文件读、存在性判断、遍历选项、URI、范围 reader、流式 Create、Rename、basename 式 Presign、空 Close、硬链接 CopyFrom、`pathExists` 和 `NewLocalStorage` 均有直接同名或等价实现。平台建目录逻辑对应 `local_unix.go` / `local_windows.go`。

Rust 有几处为保持或加强兼容而显式实现的差异：

- Rust `PathBuf::join` 遇到绝对后件会替换 base，因此 `object_path` 去掉根/prefix；`local_test.rs::test_absolute_object_name_stays_under_base` 固化了 Go `filepath.Join` 语义。
- Go `filepath.Walk` 自带词法顺序；Rust 使用 `walkdir.sort_by_file_name` 明确保持稳定顺序，并通过 `filter_entry` 实现 Go `filepath.SkipDir` 风格的 `start_after` 子树剪枝。
- Go 有 failpoint 注入和日志记录；当前 Rust 文件没有对应 failpoint，也不会记录目录补建、tombstone 或软链接 metadata 失败日志。Rust 用错误上下文及 size=0 保留对调用方可见的主要结果。
- Go `localFile::Read` 到达范围上界返回 `io.EOF`；Rust `Read` trait 按惯例返回 `Ok(0)` 表示 EOF。两侧测试都验证第二次读取长度为 0，错误值的接口形状不同。
- Rust 对负 `start_offset` 主动报错；Go 交给 `os.File.Seek`，不同平台的底层错误行为可能不同。
- Go `Create` 通过 `newFlushStorageWriter` 管理 flush、文件关闭及可选 `PartSize` 缓冲；Rust `LocalWriter` 只 flush 并逻辑关闭，文件句柄依赖 drop。新 `storeapi` 路径支持 `PartSize`，旧 Rust `storage::WriterOption` 是空类型。
- Rust 在一个类型上同时实现旧 `crate::storage::Storage` 和新 `storeapi::Storage`，这是移植期双接口接线；Go 文件只实现其原生 `storeapi.Storage` 形状。

`pkg/objstore/local_test.go` 与 `pkg/objstore/local_test.rs` 共同证明删除、软链接遍历、前缀过滤、缺失目录、跳过子目录、`start_after`、URI、范围读和断裂软链接 size=0 的对齐。Rust 独立测试还覆盖新接口取消检查、绝对对象名约束、不可读旧子树剪枝、tombstone 的 `.` 前缀语义及 `PartSize` 缓冲。

## 扩展指南

- 修改对象名映射或增加安全校验时，应集中在 `object_path`，并同步 `pkg/objstore/local_test.rs` 的绝对路径测试；若要拒绝 `..`，必须先核对 Go 兼容性和所有通过 `storage::New`、服务器磁盘导入入口传入的对象名。
- 修改整文件写入的原子性、权限、临时文件清理或持久化保证时，应改 `WriteFile` / `write_file_with_mode`，同时增加独立 Rust 回归测试，覆盖父目录缺失、rename 失败残留、并发覆盖和平台权限。不要把测试内嵌进 `local.rs`。
- 修改流式 writer 时，应保持旧 `ObjectWriter` 与新 `objectio::Writer` 两层契约同步；重点检查 `create_buffered`、`LocalWriter`、`StoreapiLocalWriter`，以及 `local_test.rs::local_writer_honors_part_size_buffer`。若需要可靠关闭文件或 sync，应明确错误传播与重复 close 语义。
- 扩展范围读时，应同时更新 `Open`、`LocalFile::read`、`LocalFile::seek` 和 `StoreapiLocalReader`；测试需覆盖半开区间、空区间、负偏移、越过 EOF、seek 后再读及 close 后读取。
- 扩展列举选项时，应在两套 `WalkOption` 的转换处、`WalkDir` 过滤顺序及 `should_skip_local_subtree` 同步实现。尤其要保持“`obj_prefix` 相对 `sub_dir`、callback 名相对 storage base、`start_after` 与 callback 名同域比较”的不变量，并在 Rust/Go 独立测试中验证稳定顺序、目录剪枝、tombstone 和并发删除。
- 扩展复制能力时，不应悄悄改变 `CopyFrom` 的硬链接语义。跨文件系统或跨后端复制应另行设计字节复制/服务端复制路径，并明确覆盖已有目标、权限、原子性和大文件性能。
- 新增生产行为后，测试继续放在 `pkg/objstore/local_test.rs`；Go 对齐行为应同步核对 `pkg/objstore/local.go` 与 `pkg/objstore/local_test.go`。涉及平台目录行为时还需核对 `local_unix.rs`、`local_windows.rs` 及对应 Go 文件。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/objstore/local.rs` 确认目标文件被索引并含 66 个符号。
- RustCodeGraph 源码与结构查询：`node --file pkg/objstore/local.rs` 阅读 1–551 行；精确 `node` 查询 `NewLocalStorage`、`create_buffered`、`should_skip_local_subtree`、`StoreapiLocalReader`、`StoreapiLocalWriter`、`LocalFile`，核对构造、调用者、被调用者与实例化边。
- crate 与模块证据：`pkg/objstore/Cargo.toml`、`pkg/objstore/lib.rs`；接口及工厂证据：`pkg/objstore/storage.rs`、`pkg/objstore/storeapi/storage.rs`、`pkg/objstore/objectio/interface.rs`。
- 直接入口与平台证据：`pkg/executor/importer/production_storage.rs`、`pkg/objstore/local_unix.rs`、`pkg/objstore/local_windows.rs`。
- Go 对照证据：`pkg/objstore/local.go`、`pkg/objstore/local_test.go`。
- Rust 独立测试证据：`pkg/objstore/local_test.rs`，覆盖双接口桥接和取消、删除、绝对对象名、遍历过滤/剪枝/tombstone、软链接、URI、范围读与写缓冲。本文仅分析这些测试，未运行 Cargo，符合本任务纯文档约束。
