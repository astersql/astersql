# `pkg/util/disk/tempDir.rs`

## 文件定位

本文件属于 `astersql-util-disk` crate，是临时存储根目录及通用目录创建的生命周期实现。crate 入口 `pkg/util/disk/lib.rs` 将 `tempDir` 声明为公开模块，并通过 `pub use tempDir::*` 导出本文件的公开函数；`checkTempDirExist` 只在 crate 内可见。

它处于 SQL 执行的磁盘溢写辅助链上：`pkg/util/chunk/chunk_in_disk.rs` 的 `DataInDiskByChunks::initDiskFile` 在创建 Chunk 落盘文件前调用 `CheckAndInitTempDir`。它也提供与临时根无关的 `CheckAndCreateDir`，当前可见调用包括 `pkg/util/memoryusagealarm/memoryusagealarm.rs` 创建 `oom_record` 根目录和单次告警记录目录。

`pkg/util/disk/Cargo.toml` 将该 crate 定义为 `astersql-util-disk`，直接依赖 `astersql-config` 读取全局配置、依赖 `fs2` 获取文件排他锁，并依赖 `astersql-util-memory` 提供同 crate 的磁盘 `Tracker` 再导出。工作区还以 `facade_util_disk` 为它登记别名；Chunk 和 memoryusagealarm 分别以 `disk-crate`、`task-disk` 依赖它。

## 核心职责

1. `CheckAndInitTempDir` 对同一进程内的初始化请求串行化，只在配置路径无法通过 `metadata` 探测时调用初始化。
2. `InitializeTempDir` 必要时递归创建配置目录，创建并持有 `_dir.lock` 的排他锁，防止另一个进程同时占用同一临时根。
3. 初始化后对目录项做一次快照；仅当条目数大于 2 时，异步删除 `_dir.lock` 和 `record` 之外的陈旧文件或目录。
4. `CleanUp` 取走进程级锁文件句柄并尝试解锁。
5. `CheckAndCreateDir` 为其他子系统提供“已存在即成功，否则递归创建”的通用接口；Unix 上新建目录模式为 `0750`。

本文件不负责选择临时路径、创建具体 spill 文件、统计磁盘用量，也不负责等待后台清理完成。这些职责分别位于全局配置、Chunk 落盘实现、`tracker.rs` 和调用方。

## 主要符号

- `static tempDirLock: Mutex<Option<File>>`：保存已经取得排他锁的文件句柄。`Some(File)` 使锁跨越 `InitializeTempDir` 调用持续存在，直到 `CleanUp` 或后续句柄替换/进程退出。
- `static sf: Mutex<()>`：Go `singleflight.Group` 的进程内等价接线。它没有结果缓存或 key 表，只把 `CheckAndInitTempDir` 的检查和可能初始化包在一个互斥区内。
- `const lockFile: &str = "_dir.lock"`、`const recordDir: &str = "record"`：后台清理必须保留的两个名称。
- `fn poisoned_lock(name: &str) -> io::Error`：把 Rust 互斥锁中毒转成 `io::ErrorKind::Other`，供公开的 fallible API 返回。
- `pub fn CheckAndInitTempDir() -> io::Result<()>`：面向调用方的惰性入口；持有 `sf` 后执行存在性检查，缺失时调用 `InitializeTempDir`。
- `pub(crate) fn checkTempDirExist() -> bool`：对 `get_global_config().temp_storage_path` 执行 `fs::metadata`；只有成功才返回 `true`，所有错误均折叠为 `false`。
- `pub fn InitializeTempDir() -> io::Result<()>`：无条件尝试打开并锁定配置目录下的 `_dir.lock`，随后按阈值启动陈旧项清理线程。
- `pub fn CleanUp()`：非 fallible 清理接口；互斥锁中毒、解锁错误都不会传给调用者。
- `pub fn CheckAndCreateDir(path: impl AsRef<Path>) -> io::Result<()>`：接受字符串或路径类型；`metadata` 成功时不检查对象是否为目录。
- `fn create_dir_all(&Path) -> io::Result<()>`：条件编译的内部实现。Unix 使用 `DirBuilderExt::mode(0o750)`，非 Unix 使用标准库默认权限。

文件中没有自定义 struct、enum、trait 或显式 feature 开关；平台差异仅由 `cfg(unix)` / `cfg(not(unix))` 控制。

## 执行流程

`CheckAndInitTempDir` 的流程如下：

1. 锁住 `sf`；中毒则返回由 `poisoned_lock("temp directory init")` 生成的 I/O 错误。
2. 调用 `checkTempDirExist` 读取当前全局配置快照并探测路径。
3. 路径不存在或探测失败时调用 `InitializeTempDir`；错误原样通过 `?` 返回。
4. 初始化完成或路径本来可探测后释放 `sf` 并返回成功。

`InitializeTempDir` 的流程如下：

1. 从当前 `get_global_config()` 快照复制 `temp_storage_path` 为 `PathBuf`。
2. 若 `fs::metadata` 失败，调用平台对应的 `create_dir_all`。若现存对象是普通文件，`metadata` 成功，因此此步刻意跳过；随后打开 `<path>/_dir.lock` 时返回 `NotADirectory`。
3. 以读写、可创建但不截断的方式打开 `_dir.lock`，并调用 `fs2::FileExt::try_lock_exclusive`。该非阻塞调用失败时直接返回，不等待其他实例释放。
4. 锁住 `tempDirLock` 并写入文件句柄，使排他锁在函数返回后继续有效；互斥锁中毒则返回错误。
5. 读取目录全部条目。若总数不大于 2，不启动清理；若大于 2，把目录路径和这次读取的条目快照移动到一个 detached 线程。
6. 后台线程跳过名称恰为 `_dir.lock` 或 `record` 的条目；其余路径按运行时 `is_dir` 结果选择 `remove_dir_all` 或 `remove_file`。单项删除失败只写标准错误，继续处理剩余项。

`CleanUp` 锁住 `tempDirLock`，用 `take` 先清空全局槽位，再对取出的句柄调用 `unlock`。`CheckAndCreateDir` 则先探测目标；探测成功立即返回，失败才递归创建。

## 数据与状态

进程级可变状态只有两个 `Mutex`。`sf` 不携带业务值，仅保护“检查后初始化”的临界区；`tempDirLock` 持有 OS 文件句柄，是排他锁生命周期的所有者。两者相互独立，固定调用路径是先持有 `sf`，初始化内部再短暂持有 `tempDirLock`；`CleanUp` 只访问后者。

路径状态来自 `astersql-config` 的进程级全局配置。每次 `checkTempDirExist` 和 `InitializeTempDir` 都分别获取配置快照，因此若其他线程恰在两者之间整体替换配置，检查路径和实际初始化路径可能不同；本文件没有额外的配置版本校验。正常调用约定应是在临时目录生命周期内保持 `temp_storage_path` 稳定。

目录清理使用 `read_dir` 得到的条目快照而非持续扫描。触发条件是“总条目数大于 2”，不是“至少存在一个陈旧项”：例如只有 `_dir.lock` 和一个陈旧项时总数为 2，不会清理；这与 Go 实现保持一致。保留规则只比较顶层名称，不检查 `record` 的对象类型或内容。

## 依赖与调用关系

直接下游依赖为：

- `crate::config::get_global_config`（由 `lib.rs` 转发 `astersql-config`）：提供 `temp_storage_path`。
- `std::fs`、`File`、`OpenOptions`、`Path`、`PathBuf`：路径探测、目录创建、锁文件打开和陈旧项删除。
- `fs2::FileExt`：`try_lock_exclusive` 与 `unlock`。
- `std::sync::Mutex`：进程内初始化串行化和锁句柄保护。
- `std::thread::spawn`：异步陈旧项清理。

已核实的直接上游关系为：

- `pkg/util/chunk/chunk_in_disk.rs`：`DataInDiskByChunks::initDiskFile -> disk::CheckAndInitTempDir`；这是执行器等 Chunk 使用者发生落盘时抵达本文件的路径。
- `pkg/util/chunk/lib.rs` 与 `pkg/util/chunk/internal/group1/lib.rs`：从 `disk_crate` 再导出 `CheckAndInitTempDir`。
- `pkg/util/memoryusagealarm/memoryusagealarm.rs`：`initMemoryUsageAlarmRecord` 和 `doRecord` 调用 `task_disk::CheckAndCreateDir`。
- `pkg/util/disk/tempDir_test.rs` 与 `migration_aster_unit_test.rs`：直接调用初始化、存在性检查和清理 API 验证行为。

当前 Rust `cmd/tidb-server/main.rs` 中名为 `disk::InitializeTempDir` / `disk::CleanUp` 的调用解析到 `cmd/tidb-server/stubs.rs` 的本地桩。现有直接证据不足以宣称本 crate 的 `InitializeTempDir` 和 `CleanUp` 已接入真实 Rust 服务启动/退出链；可以确认的是 Chunk 的惰性初始化链和 memoryusagealarm 的通用建目录链。

## 错误处理与边界

- 所有 fallible 公开函数使用 `std::io::Result`，目录创建、锁文件打开、排他锁、读取目录等错误均保留操作系统错误种类向上传播。
- `checkTempDirExist` 和 `CheckAndCreateDir` 把所有 `metadata` 错误都当成“未存在”，包括权限错误或瞬时 I/O 错误；之后的创建尝试通常会给出更具体错误，但存在检查与创建之间仍有 TOCTOU 窗口。
- 配置路径为普通文件时，`InitializeTempDir` 不把它替换成目录；打开子锁文件返回 `NotADirectory`。该边界由 `initialize_existing_file_matches_go_not_a_directory_error` 固定。
- `CheckAndCreateDir` 对任何可被 `metadata` 读取的对象返回成功，因此现存普通文件也被视为成功；迁移测试明确断言了这一 Go 兼容行为。调用方若要求“必须是目录”，应自行追加类型检查。
- `try_lock_exclusive` 是非阻塞操作；目录已被其他实例锁住时返回错误。本实现没有 Go 版针对“锁已占用”和其他锁错误的分类日志。
- 排他锁句柄在读取目录之前写入 `tempDirLock`。因此若随后 `read_dir` 失败，函数返回错误但句柄仍保留，直到显式 `CleanUp`、被下一次成功赋值替换或进程退出。
- 后台删除错误不会使初始化失败，只通过 `eprintln!` 报告；调用方也无法从返回值判断清理是否完成或成功。
- `CleanUp` 遇到 `tempDirLock` 中毒直接返回；`unlock` 失败也仅写标准错误。它是幂等的：槽位为空时无操作。
- Unix 的 `0750` 会受进程 umask 影响；非 Unix 分支不承诺等价权限位。

## 并发与资源生命周期

同一进程内，所有 `CheckAndInitTempDir` 调用在 `sf` 上串行执行，互斥范围覆盖存在性检查和完整初始化。测试 `TestRemoveDir` 与 `temp_dir_recreates_concurrently_and_preserves_reserved_entries` 均以 10 个线程验证目录删除/缺失后的并发重建。

跨进程互斥由 `_dir.lock` 的 OS 排他文件锁提供。锁的有效期由 `tempDirLock` 中的 `File` 句柄决定，而不是局部变量作用域；`CleanUp` 是明确的释放点。再次直接调用 `InitializeTempDir` 会先取得一个新锁，再替换全局句柄；是否允许同进程对同一文件重复加锁依赖平台锁语义，因此常规惰性调用应走 `CheckAndInitTempDir`，不应把直接重复初始化当成可移植协议。

陈旧项清理线程没有保存 `JoinHandle`，不会被 `CleanUp` 等待或取消。初始化成功只表示目录和锁已经就绪，不表示清理结束。线程持有独立的 `PathBuf` 和 `DirEntry` 列表；它与后续文件创建并发执行，扩展时必须考虑同名条目的创建/删除竞争。

互斥锁中毒的处理不一致是有意可见的 API 差异：初始化路径将其转换为错误，`CleanUp` 因无返回值而静默退出。测试通过 `serial_test::serial(temp_dir)` 串行化会修改全局配置和锁状态的用例，并在 Drop guard 中调用 `CleanUp`、恢复配置，避免测试间泄漏。

## 与 Go 版本的对应关系

Rust 文件直接对应 `pkg/util/disk/tempDir.go`：公开函数名、`_dir.lock` / `record` 常量、`0750` Unix 创建权限、条目数大于 2 才异步清理，以及保留两个名称的规则均保持一致。`pkg/util/disk/tempDir_test.go::TestRemoveDir` 的核心场景由 Rust 同名测试复刻：删除配置目录后并发调用 10 次并确认恢复。

主要实现差异如下：

- Go 使用 `singleflight.Group.Do("tempDir", ...)`；Rust 使用无 key 的 `Mutex<()>`。对当前唯一共享临时根，两者都合并/串行化并发初始化，但 Rust 等待者不会复用首个调用的返回值，而是取得锁后重新检查路径。
- Go 使用 `gofslock/fslock.Handle`；Rust 使用 `fs2::FileExt` 和长期保存的 `File`。两者目标都是跨进程排他占用，但具体错误值、同进程重复锁和平台行为由不同库决定。
- Go 用 `os.RemoveAll` 同时处理文件和目录；Rust先用 `Path::is_dir` 分支到 `remove_dir_all` 或 `remove_file`。符号链接和检查后类型变化时可能呈现不同边界。
- Go 使用结构化日志并区分锁占用错误；Rust 当前使用 `eprintln!`，且锁获取错误直接传播。
- Rust 显式处理 `Mutex` 中毒，并为非 Unix 提供无权限模式承诺的创建分支；Go 源文件没有对应概念。
- Rust 补充测试还固定了“配置路径是文件时返回 `NotADirectory`”“通用创建函数接受现存文件”“后台清理保留记录目录和锁文件”等迁移语义。

## 扩展指南

- 若改变临时根初始化策略，优先修改 `CheckAndInitTempDir` / `InitializeTempDir`，并同步 `pkg/util/disk/tempDir_test.rs`、`migration_aster_unit_test.rs` 以及 Go 对照测试意图；不要把测试嵌回生产文件。
- 若新增必须保留的顶层目录，应集中调整保留名称与清理判断，并增加“条目数阈值、文件、目录、符号链接、删除失败”的独立测试。注意当前阈值按总条目数计算，改成按陈旧项计数属于可观察行为变化。
- 若需要知道清理完成或传播删除错误，必须重新设计 detached 线程接口（例如保存句柄或返回任务状态），同时评估初始化延迟和进程退出语义；不能仅修改日志。
- 若要强化路径类型校验，应分别评估 `InitializeTempDir` 和 `CheckAndCreateDir` 的 Go 兼容性。后者接受现存普通文件已有测试保护，直接收紧会影响 memoryusagealarm 等调用方。
- 若接入服务启动/退出生命周期，应在真实 server 依赖图中显式引用 `astersql-util-disk`，并用启动失败、双实例锁冲突、正常退出解锁的独立集成测试证明；不要把当前 `cmd/tidb-server/stubs.rs` 事件桩当成完成证据。
- 若修改全局锁顺序或增加新锁，应保持清晰的一致顺序并检查 `sf`、`tempDirLock` 与配置锁之间的死锁风险。若允许运行期切换 `temp_storage_path`，还需把路径版本与锁句柄绑定，避免检查和初始化使用不同配置快照。
- 性能上，初始化会完整收集顶层 `read_dir` 结果后才返回；超大临时目录可能产生线性内存和扫描成本。优化时必须保留锁先取得、保留项不删除、错误边界可观测这些约束。

## 验证依据

- RustCodeGraph `status`：项目索引包含 11,467 个文件、7,032 个 Rust 文件；目标位于已索引集合。
- RustCodeGraph `files --filter pkg/util/disk`：确认 `tempDir.rs`、`lib.rs`、Go 对照和两份 Rust 测试均在图内。
- RustCodeGraph `node --file pkg/util/disk/tempDir.rs --offset 1 --limit 260`：读取目标文件全部 147 行，核对所有常量、静态状态、函数、条件编译分支及源码注释。
- RustCodeGraph `explore 'pkg/util/disk/tempDir.rs TempDir create_temp_dir'`：确认 `CheckAndInitTempDir -> InitializeTempDir`、测试调用以及 `CheckAndCreateDir` 的 memoryusagealarm 调用边；精确 `callers/callees` 子命令本次在 30 秒内未返回输出，因此未将其作为额外完成证据。
- RustCodeGraph `node`：读取 `pkg/util/chunk/chunk_in_disk.rs` 的 `DataInDiskByChunks::initDiskFile`，以及 `pkg/util/memoryusagealarm/memoryusagealarm.rs` 两处目录创建调用，核实应用位置。
- crate 与装配：读取 `pkg/util/disk/Cargo.toml`、`pkg/util/disk/lib.rs`、工作区 `Cargo.toml`、`pkg/util/chunk/Cargo.toml`、`pkg/util/memoryusagealarm/Cargo.toml` 及 Chunk 两个再导出入口。
- Go 对照：读取 `pkg/util/disk/tempDir.go`、`pkg/util/disk/tempDir_test.go`。
- Rust 测试：读取 `pkg/util/disk/tempDir_test.rs`、`pkg/util/disk/migration_aster_unit_test.rs`；未运行 Cargo，符合本纯文档任务约束。
- 人工复核重点：惰性初始化与直接初始化的区别、锁句柄持有期、`len > 2` 清理阈值、异步错误不可传播、普通文件边界，以及真实调用与 server stubs 的区分。
