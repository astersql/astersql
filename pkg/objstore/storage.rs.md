# `pkg/objstore/storage.rs`

## 文件定位

`storage.rs` 是 `astersql-objstore` crate 的实例化与统一操作边界，由 `pkg/objstore/lib.rs` 以 `pub mod storage` 对外暴露。它位于 URI/后端配置解析层 `pkg/objstore/parse.rs` 与具体存储实现之间：对上提供 `Storage` trait、取消上下文、读写选项和构造函数，对下分派到 `local.rs`、`hdfs.rs`、`memstore.rs`、`noop.rs` 或调用方注入的云后端工厂。

`pkg/objstore/Cargo.toml` 将 crate 命名为 `astersql-objstore`，入口为同目录 `lib.rs`。本文件直接使用 `anyhow` 统一错误，其余直接依赖为标准库的 IO trait、`Arc`、`AtomicBool` 和时间类型。

生产上游包括 `pkg/dxf/importinto/planner.rs::LogicalPlan::sort_store`、`pkg/dxf/importinto/scheduler.rs::prepareImportTask`、`pkg/dxf/importinto/conflictrows.rs::CleanConflictRowFiles`、`pkg/importsdk/file_scanner.rs::NewFileScanner` 和 `pkg/planner/extstore/extstore.rs::NewExtStorage`；它们通过 `NewFromURL` 或 `New` 创建可共享的 `StorageRef`。

## 核心职责

1. 用 `Storage`、`ObjectReader` 和 `ObjectWriter` 定义后端无关的对象删除、整体读写、范围读、遍历、流式写、重命名、预签名、跨存储复制和关闭契约。
2. 用 `Context` 把一个可共享的取消标志传到存储实现，并提供可中断的轮询等待。
3. 用 `New`、`NewWithDefaultOpt`、`Create` 和 `NewFromURL` 统一存储构造：本地、HDFS、noop 和内存后端在 crate 内直接创建，其他后端交给 `ExternalFactory`。
4. 保留与 Go `pkg/objstore/storage.go`/`storeapi` 形状对应的 options、权限、HTTP 连接池配置和 `TombstoneSize`。
5. `ReadDataInRange` 把“指定偏移精确填满缓冲区”组合为公共辅助函数，并在读取后关闭 reader。

## 主要符号

- `TombstoneSize: i64 = -1`：`WalkDir` 可用的已删除对象尺寸哨兵值，是 `WalkOption::include_tombstone` 契约的一部分。
- `Context`：内部是 `Arc<AtomicBool>`。`background` 创建未取消实例，`from_cancellation_flag` 复用上游标志，`cancel`/`is_cancelled` 分别以 Release/Acquire 语义写读，`check_cancelled` 转为 `context canceled` 错误，`wait_timeout` 每次最多睡眠 10 ms 后重查。
- `WalkOption`：携带 `sub_dir`、`obj_prefix`、`skip_sub_dir`、`include_tombstone` 和 `start_after`，控制遍历范围、递归、墓碑与起始键。
- `ReaderOption`：`start_offset` 为包含起点，`end_offset` 为不包含终点；二者都可为 `None`。`WriterOption` 目前是无字段兼容占位类型。
- `CopySpec { from, to }`：跨存储复制的源键与目标键。
- `ObjectReader: Read + Seek + Send`：在标准读/定位能力外要求 `close` 和 `get_file_size`。`ObjectWriter: Send` 以 `write(ctx, data)` 分块接收数据，并以 `close(ctx)` 完成提交。
- `Storage`：核心 trait，继承 `Any + Send + Sync`；`as_any` 支持后端类型下转。`DeleteFiles` 默认按输入顺序调用 `DeleteFile` 并在首个错误停止；`CopyFrom` 默认报“不支持”；`is_strong_consistent` 默认为 `false`。
- `StorageRef = Arc<dyn Storage>`：线程间共享的 trait object，克隆只增加引用计数。
- `Permission`：可请求检查 `ListObjects`、`GetObject` 或 `AccessBuckets`。`Options` 收集 `send_credentials`、`http_client`、`check_permissions`、`check_s3_object_lock_options` 和可选 `external_factory`。
- `ExternalFactory`：线程安全的共享闭包，签名为 `Fn(&Context, &StorageBackend, &Options) -> Result<StorageRef>`，用于在集成层接入云后端。
- `Create`/`NewWithDefaultOpt`/`NewFromURL`/`New`：构造入口组。`Create` 仅把 `send_credentials` 写入 options；`NewWithDefaultOpt` 传入默认 options；`NewFromURL` 先处理空 URI 与 `memstore://` 捷径；`New` 完成最终后端分派。
- `HttpTransport`/`HttpClient`：对 Go `http.Transport`/`http.Client` 中本文件关心的连接池字段的轻量描述。`CloneDefaultHTTPTransport` 返回默认值和始终为 `true` 的成功标志；`GetDefaultHTTPClient` 把全局与单主机最大空闲连接数设为请求并发度。
- `ReadDataInRange`：校验偏移和加法，以 `[start, start + p.len())` 构造 `ReaderOption`，用 `read_exact` 填满缓冲区。

## 执行流程

URL 构造主流程如下：

1. `NewFromURL(ctx, uri)` 首先拒绝空串。
2. 如果 URI 以 `memstore://` 开头，直接返回新的 `MemStorage`，不进入通用解析。
3. 其他输入交给 `parse.rs::ParseBackend(uri, None)`，得到 `StorageBackend`，然后经 `NewWithDefaultOpt` 进入 `New`。
4. `New` 对 `None` options 使用栈上默认值，再按枚举分支：`Local` 用 path 构造 `NewLocalStorage`；`Hdfs` 用 remote 构造 `NewHDFSStorage`；`Noop` 与 `MemStore` 直接构造。
5. S3/GCS/Azure 等其他枚举分支必须存在 `options.external_factory`；缺失时返回 `storage <kind> is not supported yet`，存在时将 context、backend 和完整 options 交给工厂。

`ReadDataInRange` 的流程是：先拒绝负 `start`，将缓冲区长度安全转为 `i64`，用 `checked_add` 计算不包含终点，再调用 `Storage::Open`。`read_exact` 要么返回整个 `p.len()`，要么返回 IO 错误；随后总是尝试 `ObjectReader::close`，但关闭失败只写入 stderr，不覆盖读结果。

## 数据与状态

本文件没有全局可变状态。`Context` 的取消位是唯一的内部可变状态；克隆后的 contexts 共享同一 `AtomicBool`，因此任意一个克隆执行 `cancel` 都会被其他克隆观察到。取消是单向的，没有重置 API。

`StorageRef` 使后端对象可被多个流程共享；具体后端必须自行满足 `Send + Sync` 并保护内部可变数据。本 trait 没有统一锁、缓存或事务状态。`WalkOption`、`ReaderOption`、`CopySpec`、`Options` 和 HTTP 配置都是调用期值；`New` 不保存 options 本身，是否复制其内容由具体构造器或外部工厂决定。

`WriterOption` 当前没有状态；`HttpClient` 也只是配置描述，并未在此文件中建立套接字或运行连接池。

## 依赖与调用关系

主要上游调用与使用关系：

- `pkg/dxf/importinto/planner.rs::LogicalPlan::sort_store` 在未注入现成 store 时用 `NewFromURL`，并记录是否由当前流程拥有该 store。
- `pkg/dxf/importinto/scheduler.rs::prepareImportTask` 用分布式任务的 cancellation flag 创建 `Context`，然后构造 sort store 并写入 prepared metadata。
- `pkg/dxf/importinto/conflictrows.rs::CleanConflictRowFiles` 从 URI 打开 store，清理文件后显式调用 `Close`。
- `pkg/importsdk/file_scanner.rs::NewFileScanner` 先以 `ParseBackend` 解析来源，再以 `New` 构造供 loader 扫描的 store；`pkg/planner/extstore/extstore.rs::NewExtStorage` 在后端 path 追加 namespace 后调用 `New`。
- `br/pkg/task/operator/base64ify.rs::runEncode` 用 `New` 验证后端并随后 `Close`，同时把 `send_credentials` 和 S3 object-lock 检查意图写入 `Options`。
- `pkg/planner/extstore/extstore.rs` 再导出 `Context`、`Storage` 和 `StorageRef`；`pkg/objstore/locking.rs`、DXF import-into 和 import SDK 直接依赖这些抽象。

主要下游依赖：

- `parse.rs::ParseBackend` 完成 URI 到 `StorageBackend` 的转换，`StorageBackend::kind` 为不支持错误提供后端名。
- `local.rs::NewLocalStorage`、`hdfs.rs::NewHDFSStorage`、`memstore.rs::NewMemStorage` 和 `noop.rs::newNoopStorage` 承担 crate 内直接实例化。精确搜索另外确认 `LocalStorage`、`MemStorage`、`NoopStorage` 和 `HDFSStorage` 实现本文件的 `Storage` trait。
- `std::io::Read::read_exact` 固定 `ReadDataInRange` 的“全部填满或报错”语义；`anyhow` 承载构造、IO 和边界校验错误。

RustCodeGraph 的文件节点报告 `storage.rs` 被 90 个文件使用；对 `NewFromURL`、`Storage`、`Context`、`GetDefaultHTTPClient` 和 `ReadDataInRange` 的 `node --file` 查询确认了文件内调用边和部分测试调用者。`New` 这一常见名称的图结果夹杂了无关同名边，因此跨文件生产调用使用带完整路径的 `rg` 结果与调用点源码补证，不将模糊同名边当作结论。

## 错误处理与边界

- `Context::check_cancelled` 只返回文本为 `context canceled` 的 `anyhow` 错误；它不带结构化错误类型。`wait_timeout` 会在进入和每次睡眠后重查，但取消观察延迟最多受 10 ms 轮询粒度影响。
- `Storage::DeleteFiles` 不回滚已删除对象，因此失败可能留下部分完成状态。`CopyFrom` 仅是默认拒绝；需要服务端复制的后端必须覆写它。
- `NewFromURL` 拒绝空 URI；其 `memstore://` 判断是字符串前缀捷径，与其他 scheme 使用的通用解析路径不同。解析错误和本地构造错误使用 `?` 原样传播。
- `New` 不为本地/HDFS/noop/memstore 分支主动检查 `Context`；`storage_test.rs::test_new_hdfs_storage_ignores_cancelled_context` 明确固定了 HDFS 构造对已取消 context 的当前行为。云工厂是否检查取消由工厂实现决定。
- 对 S3/GCS/Azure 等分支，默认 `Options` 没有 `external_factory`，因此会返回不支持错误。全仓库对 `external_factory`/`ExternalFactory` 的精确搜索只找到本文件的定义与调用，未找到生产注入点；因此不能宣称默认构造器已接通云实现。
- `ReadDataInRange` 拒绝负偏移，并分别防御 `usize -> i64` 转换与 `start + len` 溢出。它不允许短读作为成功；对象不足会由 `read_exact` 返回错误。无论读成功还是失败都尝试关闭，但关闭错误不会成为函数返回值。

## 并发与资源生命周期

`Context` 的 `Arc<AtomicBool>` 允许上游任务、存储调用和等待循环共享取消状态，不需要 mutex。`wait_timeout` 是同步阻塞轮询，会占用当前 OS 线程；不应把它视为 async timer。

`Storage: Send + Sync` 与 `StorageRef = Arc<dyn Storage>` 使同一后端能在多线程中共享，但本文件没有规定单个操作的事务性或多操作顺序。`WalkDir` 的 callback 是 `FnMut`，在一次调用内由后端掌握调用时机；此 trait 签名本身不承诺并行 callback。

reader/writer 是显式关闭资源：`ObjectWriter::close(ctx)` 是提交边界，`helper_2_aster_unit_test.rs` 中的内存后端用例验证写入数据在 close 前尚未落入对象，close 后才可读，已关闭 writer 继续写会报错。`ReadDataInRange` 显式关闭 reader；其他直接调用 `Open`/`Create` 的上游也必须完成对应关闭。`Storage::Close` 没有返回值，调用方无法从该接口观察存储关闭错误。

## 与 Go 版本的对应关系

同路径 `pkg/objstore/storage.go` 是构造器、HTTP 辅助和范围读的主要对照，而 Go 的完整 `Storage`、options 和 reader/writer 契约位于 `pkg/objstore/storeapi/storage.go`。Rust 将这些界面类型合并到本文件，便于本 crate 的 local/memory/noop/HDFS 实现共用。

已对齐的主要意图包括：`TombstoneSize == -1`；`Create` 仅保留 send-credentials 兼容参数；`NewWithDefaultOpt` 转发默认 options；`NewFromURL` 拒绝空输入并特判 memstore；`New` 按 backend 分派；HTTP 客户端把空闲连接数对齐并发度；`ReadDataInRange` 使用左闭右开范围并要求填满整个缓冲区。

当前实现差异必须保留为明确事实：

- Go `New` 直接分派 S3/KS3/OSS/GCS/Azure 的生产构造器，并对 protobuf oneof 中的 nil 配置返回 `ErrStorageInvalidConfig`。Rust 的自有 `StorageBackend` 枚举不需要相同 nil 检查，但云后端统一依赖 `external_factory`；当前未发现仓库内注入点。
- Go 使用标准 `context.Context`；Rust 使用只表达共享取消位的轻量 `Context`，没有 deadline、value 或取消原因。
- Go 的 `http.Client` 和 `http.Transport` 是可执行网络类型，`CloneDefaultHTTPTransport` 实际 clone 当前默认 transport 并返回类型断言结果。Rust 类型只保留所需配置字段，clone 辅助的 bool 固定为 `true`。
- Go `ReadDataInRange` 用 `start + int64(len(p))` 后的回绕判断溢出；Rust 同时使用 `i64::try_from` 和 `checked_add`。Go 用 logger 记录 reader close 错误，Rust 写入 stderr；两者都不覆盖原读结果。
- Go 通过包装的 `berrors` 保留结构化错误身份；Rust 使用 `anyhow` 文本与错误链，上游不应假设两者可做同样的错误身份匹配。

`pkg/objstore/storage_test.rs` 与 `storage_test.go` 共同固定默认 HTTP transport、并发度连接池和 `memstore://` 构造；Rust 测试额外覆盖 HDFS 构造及已取消 context 不阻止该构造的现状。`helper_2_aster_unit_test.rs::noop_and_range_read_keep_go_edge_cases` 直接覆盖范围读的成功值和负偏移错误。

## 扩展指南

- 新增存储操作时，先确认是统一 `Storage` 契约还是后端特有能力。若修改 trait，必须同步所有生产 impl（至少 local、memstore、noop、HDFS）、`pkg/objstore/mockobjstore/objstore_mock.rs` 与独立测试 mock；避免用无意义默认实现掩盖未接线后端。
- 新增 backend 时，同步 `parse.rs::StorageBackend`、`StorageBackend::kind`、`New` 的分派和 URI 格式化，并明确是 crate 内直接构造还是由 `ExternalFactory` 接入。云后端不能只增加枚举分支而不提供实际工厂注入。
- 扩展 `Options` 时，必须同时更新具体构造器/外部工厂的消费逻辑和 Go `storeapi.Options` 对照；仅添加字段不会自动生效。`send_credentials` 和权限检查涉及安全兼容性，不应在默认路径中静默改变。
- 改动 `Context` 时必须保留 clone 共享取消语义，并检查 `from_cancellation_flag` 的 DXF/session 调用者。如需 deadline 或 async 等待，应单独设计，不要把阻塞 `wait_timeout` 直接用在 async executor 的工作线程上。
- 修改 reader/writer 生命周期时，重点覆盖短读、范围边界、close 失败、close 后写、重复 close 和取消。`ReadDataInRange` 的关闭错误优先级是显式兼容契约，若要改变必须评估 Go 对齐。
- 测试保持在独立文件：核心构造/HTTP 契约放在 `pkg/objstore/storage_test.rs`，跨模块范围读现有用例在 `pkg/objstore/helper_2_aster_unit_test.rs`，具体后端行为应同步各自的 `*_test.rs`；涉及迁移语义时对照 `storage_test.go` 和具体 Go 后端测试。
- 兼容风险主要在公开 trait 破坏、URI 分派、云工厂默认行为、错误文本/身份和 close 语义；性能风险主要在 `DeleteFiles` 串行执行、`wait_timeout` 轮询和范围读强制填满整个缓冲区。没有基准或后端契约证据时，不应擅自并行化批量删除或缓存存储实例。

## 验证依据

- RustCodeGraph `status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/objstore/storage.rs --offset 1 --limit 420` 覆盖源文件 1–368 行，并报告它被 90 个文件使用。
- RustCodeGraph `query` 区分了 `ReadDataInRange`、`NewFromURL`、`CloneDefaultHTTPTransport` 的 Rust/Go 定义；`node <symbol> --file pkg/objstore/storage.rs` 核对了 `Storage`、`Context`、`Create`、`New`、`NewFromURL`、`GetDefaultHTTPClient` 和 `ReadDataInRange` 的源码、文件内 callees 及可靠测试调用者。精确 `callers ReadDataInRange --file ...` 查询长时间无输出后被中止，模糊 `New` 边不作为证据；生产调用用完整路径搜索与局部源码补齐。
- 已阅读生产与 crate 边界：`pkg/objstore/storage.rs`、`pkg/objstore/Cargo.toml`、`pkg/objstore/lib.rs`、`pkg/objstore/parse.rs.md`、`pkg/dxf/importinto/planner.rs`、`pkg/dxf/importinto/scheduler.rs`、`pkg/dxf/importinto/conflictrows.rs`、`pkg/importsdk/file_scanner.rs`、`pkg/planner/extstore/extstore.rs` 和 `br/pkg/task/operator/base64ify.rs`。
- 已阅读 Go 对照与独立测试：`pkg/objstore/storage.go`、`pkg/objstore/storage_test.go`、`pkg/objstore/storage_test.rs` 以及 `pkg/objstore/helper_2_aster_unit_test.rs` 的范围读/writer 生命周期用例。
- 精确源码搜索核对了 `Storage` 生产 impl、`New`/`NewFromURL` 上游和 `external_factory` 的仓库内出现位置。其结果支持“默认未接通云工厂”的限制说明，不延伸为对未搜索的运行时注入的推测。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的命令验证本文档存在且恰好包含 11 个固定二级章节，并人工复核所有重要结论均能追溯到上述符号、调用点或测试。
