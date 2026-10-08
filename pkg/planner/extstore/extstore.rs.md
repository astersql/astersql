# `pkg/planner/extstore/extstore.rs`

本文说明 [`extstore.rs`](./extstore.rs) 当前实现的外部存储创建、进程级缓存和本地路径探测逻辑。它描述的是现有代码事实；底层对象存储的读写实现位于 `pkg/objstore`，不在本文件中。

## 文件定位

该文件是 `astersql-planner-extstore` crate 的生产实现，由同目录 [`lib.rs`](./lib.rs) 以 `root_feature_extstore` 私有模块载入后整体重导出。crate 的边界和依赖声明在 [`Cargo.toml`](./Cargo.toml)：默认无 feature，`nextgen` feature 会传递给 `astersql-config-kerneltype/nextgen`；直接依赖包括配置、内核类型、对象存储、会话变量，以及 `anyhow`、`chrono`、`path-clean` 和 `url`。

它位于 SQL 规划相关目录，但职责不是制定查询计划，而是为 Plan Replayer、诊断 extract 等上层功能提供统一的 `StorageRef`。生产调用证据包括：

- `pkg/session/runtime/dispatch.rs` 的 Plan Replayer 保存路径获取全局存储并调用 `WriteFile`；
- `pkg/server/http_status.rs` 的 Plan Replayer 下载路径通过全局存储执行 `FileExists` 和 `ReadFile`；
- `pkg/server/extract_runtime.rs` 使用全局存储写入 extract 归档，并通过 `Open` 返回流式读取器。

`pkg/domain/plan_replayer.rs` 与 `pkg/domain/extract.rs` 中还能看到 Go 迁移留下的注释代码，但它们不是当前可执行的 Rust 调用边。

## 核心职责

1. `GetGlobalExtStorage` 惰性创建并缓存进程级 `StorageRef`，让不同调用者共享同一后端实例。
2. `createGlobalExtStorage` 根据内核类型和 `vardef::CloudStorageURI` 选择云 URI 或本地 `file://` URI，并附加全局 keyspace 名称。
3. `NewExtStorage` 把原始 URI 解析成 `objstore` backend，再创建真正的对象存储实现；本文件只负责编排，不实现对象读写协议。
4. `getLocalPathDirName` 通过一次 open/write/close/remove 探测优先选择日志目录；探测失败则回退到配置的临时目录。
5. `SetGlobalExtStorageForTest`、`SetLocalPathFileSystemForTest` 和两个探测 trait 提供受控的测试替换边界。
6. `redact_url` 在错误日志中遮蔽常见 S3/KS3/OSS/Azure 凭证查询参数。

## 主要符号

- `globalExtStorage: Mutex<Option<StorageRef>>`：惰性全局缓存。`StorageRef` 是可克隆的共享引用，返回时使用 `Arc::clone`。
- `testLocalPathFS: Mutex<Option<Arc<dyn ProbeFileSystem>>>`：只服务于路径探测的可注入文件系统，而不是通用对象存储替身。
- `lock_unpoisoned<T>`：统一获取 mutex；若其他线程 panic 导致 poison，仍通过 `into_inner` 恢复内部值。
- `ProbeFile: Write + Send`：在 `Write` 之外增加消费 `Box<Self>` 的显式 `close`。
- `ProbeFileSystem: Send + Sync`：只暴露 `open_file` 与 `remove_file`，恰好覆盖可写性探测所需能力。
- `OsProbeFile` / `OsProbeFileSystem`：上述 trait 的真实 OS 实现。`OsProbeFile` 用 `Option<File>` 保证显式关闭时只 drop 一次。
- `GetGlobalExtStorage(&Context) -> Result<StorageRef>`：公开的全局入口。
- `createGlobalExtStorage(&Context) -> Result<StorageRef>`：内部选路和日志编排函数。
- `SetGlobalExtStorageForTest(Option<StorageRef>)`：公开测试钩子；传入 `None` 清空缓存。
- `SetLocalPathFileSystemForTest(Option<Arc<dyn ProbeFileSystem>>)`：公开测试钩子；安装或清除探测文件系统。
- `NewExtStorage(&Context, &str, &str) -> Result<StorageRef>`：公开的非缓存构造入口。
- `getLocalPathDirName() -> PathBuf`、`canWriteToReplayerDirFile`、`canWriteToFileInternal`：内部本地目录选择与探测链。
- `redact_url(&str) -> String`：内部日志脱敏辅助函数。

本文件没有条件编译的行为主体；仅测试构建下会把父模块的 `root_feature_kerneltype` 别名为 `kerneltype`，并重导出 `config`、`objstore`、`vardef` 供独立测试使用。非测试构建使用 crate 根已重导出的同名依赖。

## 执行流程

全局获取流程如下：

1. `GetGlobalExtStorage` 锁住 `globalExtStorage`。
2. 若缓存为空，调用 `createGlobalExtStorage`；构造失败时不写缓存，错误直接返回，后续调用可以重试。
3. 若构造成功，将 `StorageRef` 放入缓存，并克隆同一个 `Arc` 返回。迁移测试用 `Arc::ptr_eq` 证明连续两次获取命中同一实例。

`createGlobalExtStorage` 的选路流程如下：

1. 从 `config::get_global_keyspace_name()` 读取租户命名空间，从 `vardef::CloudStorageURI.Load()` 读取配置 URI。
2. 当 `kerneltype::IsClassic()` 为真，或 URI 为空时，不使用现有云 URI，转入本地回退路径。
3. `getLocalPathDirName` 取日志文件的父目录，清理词法路径后探测 `<log-dir>/replayer/test_<本地时间>.txt`。探测成功选择日志目录，否则选择 `config.temp_dir`。
4. 将相对本地目录以当前工作目录为基准变成词法绝对路径；这里刻意不用 `canonicalize`，因此路径尚不存在也可成功转换。然后构造 `file://` URI。
5. 调用 `NewExtStorage`，成功时只记录 `storage.URI()`，失败时记录经 `redact_url` 处理的 URI 并传播错误。

`NewExtStorage` 先执行 `objstore::parse::ParseRawURL`。当 `namespace` 非空时，它过滤命名空间中的平台前缀和根目录组件，再与 URL 原路径拼接并执行词法清理，避免形如 `/test_namespace` 的输入直接替换原存储根路径。随后 `ParseBackendFromURL` 生成 backend 配置，最终由 `objstore::storage::New` 创建具体后端。

路径探测的最小操作序列是：`open_file` → 写入单字节 `0` → `close` → 若关闭成功则尝试 `remove_file`。函数最终只以 open 和 write 是否成功判断“可写”；关闭或删除失败会告警，但不会把已经成功的写探测改判为失败。

## 数据与状态

- 全局缓存状态是 `None` 或一个 `StorageRef`。初始化发生在持有 mutex 期间，因此同一进程内不会并发创建多个缓存实例。
- keyspace 名称在每次实际初始化时读取，并固化进 backend 路径；缓存命中后，配置或 keyspace 的后续变化不会自动重建存储，测试需先调用 `SetGlobalExtStorageForTest(None)`。
- `CloudStorageURI`、全局 config、当前工作目录和内核类型共同决定首次初始化结果。
- `testLocalPathFS` 只影响日志目录探测；真正的 `file://` backend 仍由 `objstore::storage::New` 创建。
- 探测文件名精确到秒，位于 `replayer` 子目录。真实 OS 实现会创建文件，但不会创建缺失的父目录，所以 `replayer` 不存在或不可写时会自然回退。
- namespace 被拼入 URL 的 path 字段，而不是保存为独立运行时状态。路径清理是词法操作，不解析符号链接。

## 依赖与调用关系

上游可执行调用集中在会话和服务层：

- `pkg/session/runtime/dispatch.rs`：生成 Plan Replayer 压缩内容后，经 `GetGlobalExtStorage` 写入 `GetPlanReplayerDirName()/文件名`。
- `pkg/server/http_status.rs`：HTTP 下载处理器从同一全局存储检查并读取 Plan Replayer 文件。
- `pkg/server/extract_runtime.rs`：诊断 extract 的持久化调用 `WriteFile`，读取路径调用 `Open` 并包装成 `ExtractReader`。

定向检索还显示 `pkg/session/runtime_test/session.rs`、`pkg/server/http_status_test.rs`、`pkg/server/handler/optimizor/plan_replayer_test.rs` 等跨 crate 测试会创建或注入存储。Cargo 直接依赖者包括 `pkg/session`、`pkg/server`、`pkg/domain`、`pkg/executor` 以及相关 handler/test crate；仓库根 `pkg/lib.rs` 还通过 facade 重导出该 crate。

下游调用链为：

- 配置：`config::get_global_keyspace_name`、`config::get_global_config`；
- 运行模式与变量：`kerneltype::IsClassic`、`vardef::CloudStorageURI.Load`；
- 后端构造：`objstore::parse::ParseRawURL` → `ParseBackendFromURL` → `objstore::storage::New`；
- 系统资源：`std::env::current_dir`、`OpenOptions`、`std::fs::remove_file`、`chrono::Local::now`；
- 日志与错误：`log::{info,warn}` 和 `anyhow::Context`。

## 错误处理与边界

- URI 解析、backend 解析和存储创建错误均由 `anyhow::Context` 添加阶段信息后返回；上下文中的 URI 经过 `redact_url`。
- `redact_url` 对 `s3`、`ks3`、`oss` 的 access key、secret、session token，以及 `azure`、`azblob` 的 account key、encryption key、SAS token 做大小写与下划线/连字符归一化后替换。未知 scheme 不脱敏；若输入本身无法被 `Url::parse` 解析，则原字符串会原样返回，因此新增含密钥的 scheme 或非标准 URI 格式时必须同步审查此函数。
- `current_dir` 获取失败不会终止初始化：相对路径保持原样并继续构造 URI。这与“正常情况下得到绝对路径”的主路径不同，调用者不应把绝对性当成无条件保证。
- 探测时 open 或 write 失败返回 `false` 并触发临时目录回退。close/remove 失败仅告警；特别是 remove 失败可能遗留探测文件。
- mutex poison 被有意忽略，以免一次 panic 永久破坏后续测试或服务访问；代价是调用者必须接受被恢复状态可能来自发生过 panic 的临界区。
- `NewExtStorage` 仅过滤 namespace 的根/前缀组件并做词法清理，没有自行建立“不得出现父目录组件”的安全策略；若未来允许不可信 namespace 输入，需要在此入口补充约束并加入独立回归测试。
- `SetGlobalExtStorageForTest` 和 `SetLocalPathFileSystemForTest` 虽为公开函数，但语义是测试钩子。生产代码误用会改变整个进程的共享状态。

## 并发与资源生命周期

两个全局状态各由独立 `Mutex` 保护，trait object 还要求 `Send + Sync`。`GetGlobalExtStorage` 在整个初始化过程中持有缓存锁，保证单次初始化和 `Arc` 身份稳定；这也意味着 URI 解析、路径探测和 backend 创建期间，其他获取或测试替换请求都会等待。

返回的 `StorageRef` 通过 `Arc` 共享，调用者获得的是同一 backend 的引用而非新建 backend。清空或替换全局项只会释放缓存持有的那一份 `Arc`；仍持有克隆的调用者可以继续使用旧对象。本文件不在替换时显式调用 `Storage::Close`，资源的业务关闭由持有者/底层实现约定管理。

`OsProbeFile::close` 消费 boxed 文件并取走内部 `File`，通过 drop 关闭描述符。无论写是否成功，`canWriteToFileInternal` 都会尝试 close；只有 close 成功时才删除文件。测试修改 config、URI 和两个全局钩子时使用 `#[serial]`，并由 guard 的 `Drop` 恢复状态，避免并发用例相互污染。

## 与 Go 版本的对应关系

直接对照文件为 [`extstore.go`](./extstore.go)，Rust 独立测试与 [`extstore_test.go`](./extstore_test.go) 保持相同的主要意图。

- Go 的 `storeapi.Storage + sync.Mutex` 对应 Rust 的 `StorageRef + Mutex<Option<_>>`；Rust 用 `Arc::clone` 暴露共享实例。
- Go 的 `context.Context`、`objstore.ParseRawURL`、`ParseBackendFromURL`、`objstore.New` 分别对应 Rust 的 `Context` 和同名移植 API。
- Go 从 `keyspace.GetKeyspaceNameBySettings()` 取命名空间；Rust 通过 `config::get_global_keyspace_name()` 获取已移植配置值。
- Go 的可选 `afero.Fs`/`testLocalPathFS` 对应 Rust 的窄接口 `ProbeFileSystem`。Rust 没有保留 `getLocalPathDirName(vfs ...afero.Fs)` 的可变参数形式，而是统一从受 mutex 保护的测试钩子选择替身。
- Go 使用 `filepath.Abs` 和 `filepath.Join`。Rust 以 `current_dir + path_clean` 保留“不要求路径真实存在”的词法绝对化语义，并专门过滤 namespace 的根组件以保持既有 URL path；`migration_aster_unit_test.rs` 覆盖了相对路径和以 `/` 开头的 namespace。
- Go 在测试构建中用 `intest.Assert` 约束 close/remove 成功；Rust 当前对这两类失败记录 warning，但仍以写入结果判定可写。这是可观察的错误处理差异，不能在文档中声称完全等价。
- Go 和 Rust 都在 classic 内核或空 URI 时选本地路径；`migration_aster_unit_test.rs` 进一步验证 nextgen 配置云 URI 时不会执行本地探测。

## 扩展指南

- 新增存储 scheme 或 backend：优先扩展 `pkg/objstore` 的解析和构造实现；本文件通常只需确认 `NewExtStorage` 能透传。若 scheme 携带凭证，必须同步扩展 `redact_url`，并在独立测试文件增加大小写、下划线/连字符和解析失败边界用例。
- 修改全局选路：集中调整 `createGlobalExtStorage`，同时覆盖 classic、nextgen、空 URI、非空 URI和 keyspace 拼接；不要绕过 `NewExtStorage` 的统一解析链。
- 修改本地目录策略：调整 `getLocalPathDirName` 或探测链，并保持 open/write/close/remove 生命周期明确。若要自动创建 `replayer` 目录，应明确谁负责创建、失败是否回退，以及是否改变 Go 行为。
- 收紧 namespace：在 `NewExtStorage` 解析完成、backend 构造之前验证组件，特别关注 `..`、平台前缀和绝对路径。不能通过删除 Go 已有语义来简化实现。
- 修改全局或测试钩子时，要保持所有共享状态受锁保护，并继续把状态型测试放在同目录独立测试文件中；不要把测试内嵌回 `extstore.rs`。
- 应同步的直接测试是 `pkg/planner/extstore/extstore_test.rs` 和 `pkg/planner/extstore/migration_aster_unit_test.rs`，并根据调用行为检查 session/server 的 Plan Replayer 与 extract 测试。兼容风险主要是 URI/path 变化，正确性风险主要是缓存失效与目录逃逸，性能风险主要是持锁初始化和远端 backend 构造延迟。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/extstore` 确认本目录的 Rust、Go 和测试文件均已索引。
- RustCodeGraph `node --file pkg/planner/extstore/extstore.rs --offset 1 --limit 500`：读取目标文件 281 行全貌，并确认图索引报告该文件被 26 个文件使用。
- RustCodeGraph `query`：分别定位 Rust/Go 的 `GetGlobalExtStorage`、`NewExtStorage`、`getLocalPathDirName`、`canWriteToFileInternal` 及对应测试符号。
- RustCodeGraph `callers/callees` 的精确查询没有输出可用调用边；因此没有把缺失结果推断为“无调用者”，而是用已索引文件节点与定向 `rg` 核对当前可执行上游，并用 `node --file` 阅读 `pkg/session/runtime/dispatch.rs`、`pkg/server/http_status.rs`、`pkg/server/extract_runtime.rs` 的调用现场。
- 已读直接文件：`pkg/planner/extstore/Cargo.toml`、`lib.rs`、`extstore.go`、`extstore_test.rs`、`extstore_test.go`、`migration_aster_unit_test.rs`；另以 Cargo manifest 检索核对直接依赖 crate。
- 测试证据：`extstore_test.rs` 覆盖本地 backend CRUD/流式 IO/rename/批量删除以及日志目录探测与临时目录回退；`migration_aster_unit_test.rs` 覆盖绝对形态 namespace、未落盘相对路径、classic/nextgen 选路、探测操作序列和 `Arc` 缓存身份。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的结构命令确认目标文件存在且恰有 11 个固定二级标题，并人工复核文档未把注释代码写成当前调用关系。
