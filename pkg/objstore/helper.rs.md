# `pkg/objstore/helper.rs`

## 文件定位

`helper.rs` 属于 `astersql-objstore` crate（见 `pkg/objstore/Cargo.toml`），由 `pkg/objstore/lib.rs` 以 `pub mod helper` 暴露。它不是具体对象存储后端，而是把三类横切能力集中在一起：云存储 URI 校验器注册与执行、GCS 上传 worker 计数读取、以及对目录中元数据文件的并行读取和反序列化。

当前 Rust 接线程度需要与 Go 区分：仓库搜索只发现 `UnmarshalDir` 被 Rust 独立测试调用，未发现生产 Rust 调用 `helper::init`、`registered_cloud_storage_uri_validator`、`ValidateCloudStorageURI`、`GetActiveUploadWorkerCount` 或 `UnmarshalDir`。因此这些接口目前是已实现、可公开访问的迁移能力，但不能据此断言它们已经进入 Rust 应用主链。Go 对应实现则已接入系统变量、DXF 指标和 BR 元数据读取链。

## 核心职责

1. `init` 与 `registered_cloud_storage_uri_validator`：用进程级 `OnceLock` 保存一个 `Validator` 函数指针，模拟 Go 包初始化时注册 `tidb_cloud_storage_uri` 校验函数的效果。
2. `ValidateCloudStorageURI`：解析 URI，构造禁用 HTTP keep-alive 的客户端配置，以 `ListObjects`、`GetObject`、`AccessBuckets` 三项权限要求创建存储，随后关闭存储。它验证“能否按所需权限打开后端”，不读写业务对象。
3. `activeUploadWorkerCnt` 与 `GetActiveUploadWorkerCount`：提供进程级原子计数及只读观测入口。当前 Rust GCS 实现未搜索到对该计数的增减操作，所以读取值通常仍为初始值 `0`。
4. `UnmarshalDir` 与 `UnmarshalDirIter`：在后台线程中遍历目录，把对象路径分发给最多 128 个 worker，并以阻塞迭代器形式返回成功值或一个终止错误。

## 主要符号

- `type Validator = fn(&Context, &str) -> Result<()>`：校验器 ABI；只接受普通函数指针，不接受捕获环境的闭包。
- `CLOUD_STORAGE_URI_VALIDATOR: OnceLock<Validator>`：进程生命周期内只能成功写入一次的校验器槽位。
- `pub fn init()`：尝试把 `ValidateCloudStorageURI` 放入槽位；忽略重复设置错误，因此重复调用是无报错的幂等尝试，但不会替换已有值。
- `pub fn registered_cloud_storage_uri_validator() -> Option<Validator>`：复制并返回已注册函数指针；调用 `init` 前返回 `None`。
- `pub fn ValidateCloudStorageURI(ctx: &Context, uri: &str) -> Result<()>`：公开 URI 校验入口。
- `pub static activeUploadWorkerCnt: AtomicI64`：公开的全局上传 worker 计数，使用 Go 风格命名以保持移植接口形状。
- `pub fn GetActiveUploadWorkerCount() -> i64`：以 `SeqCst` 顺序读取计数。
- `pub struct UnmarshalDirIter<T>`：持有 `crossbeam_channel::Receiver<Result<T>>` 的阻塞迭代器；字段私有，只能由 `UnmarshalDir` 构造。
- `impl Iterator for UnmarshalDirIter<T>`：`next` 阻塞于 `recv`；收到消息时返回 `Some(Result<T>)`，所有发送端释放后返回 `None`。
- `pub fn UnmarshalDir<T, F>(...) -> UnmarshalDirIter<T>`：泛型并行装载入口。`T` 必须可跨线程发送且为 `'static`；解析函数 `F` 必须可发送、可共享且为 `'static`。

本文件没有条件编译项、trait 定义或自定义错误类型。

## 执行流程

### URI 校验

1. `ValidateCloudStorageURI` 调用 `parse::ParseBackend(uri, None)`，空 URI、非法 scheme 参数或缺少 bucket 等解析错误直接通过 `?` 返回。
2. 创建 `HttpClient`，只覆盖 `HttpTransport::disable_keep_alives = true`，其余连接池字段沿用默认值。该选择对齐 Go 中为避免连接泄漏检测残留而关闭 keep-alive 的做法。
3. 调用 `storage::New`，传入三项权限检查。`Local`、`Hdfs`、`Noop`、`MemStore` 可由 `New` 直接构造；其他云后端需要 `Options.external_factory`。
4. 构造成功后调用 `Storage::Close`，然后返回 `Ok(())`。

这里存在当前 Rust 边界：本函数自己构造的 `Options` 没有填充 `external_factory`，而 `storage::New` 对 S3/GCS/Azure 等分支会在缺少该工厂时报 `storage <kind> is not supported yet`。因此代码形状保留了云校验意图，但仓库当前实现不能仅靠此函数完成真实云后端构造。

### 目录反序列化

1. `UnmarshalDir` 建立无界结果通道，将解析闭包放入 `Arc`，再启动一个协调线程；函数立即把接收端包装为 `UnmarshalDirIter` 返回。
2. 协调线程创建共享的 `failed: AtomicBool`、首错槽 `Mutex<Option<anyhow::Error>>`、无界路径工作队列和 worker 句柄列表。
3. 调用 `Storage::WalkDir`。每次回调先检查 `failed`；若尚未失败且 worker 数少于 128，就创建一个 worker。随后把当前路径复制为 `String` 并发送到工作队列。
4. 每个 worker 循环接收路径。处理前再次检查 `failed` 和 `Context::check_cancelled`；随后执行 `Storage::ReadFile`，再调用 `unmarshal(path, bytes)`。
5. 成功值发送到结果通道。读取、取消或反序列化失败时，仅第一个通过 `failed.swap(true, AcqRel)` 的 worker 写入 `worker_error`，然后退出；其他 worker观察到失败后停止继续处理。
6. `WalkDir` 返回后，协调线程丢弃工作发送端并逐一 `join` 所有已创建 worker，保证已经提交的任务先结束。它优先保留 `walk_result` 的错误；只有遍历成功时才取 worker 首错。
7. 若有终止错误，协调线程向结果通道发送一次 `Err`。线程退出并释放最后的发送端后，迭代器最终得到通道关闭并返回 `None`。

结果顺序由 worker 完成顺序决定，不保证与 `WalkDir` 顺序一致。最多创建 128 个线程，但 worker 是随前 128 次回调逐个启动的；路径和结果通道均为无界通道，不提供生产者背压。

## 数据与状态

- 校验器状态存于 `CLOUD_STORAGE_URI_VALIDATOR`，是不可重置、不可替换的进程级状态。`Validator` 是函数指针，因此读取不需要额外锁。
- 上传计数存于 `activeUploadWorkerCnt: AtomicI64`。本文件仅初始化和读取它；Rust 仓库当前未发现写入方。
- 每次 `UnmarshalDir` 调用拥有独立的结果通道、路径通道、失败标志、首错槽和线程集合，不在调用之间共享任务状态。
- `Context` 按值传入并克隆给 worker；其内部共享同一个取消 `AtomicBool`，所以任一持有者调用 `cancel` 后，worker 的后续 `check_cancelled` 都能观察到。
- `StorageRef` 是 `Arc<dyn Storage>`，由所有 worker 共享；`Storage` trait 要求 `Send + Sync`，保证跨线程调用接口在类型层面成立。
- 结果值 `T` 不要求 `Sync`，因为每个值只从 worker 所有权转移到结果通道和消费者。

## 依赖与调用关系

RustCodeGraph 对 `helper.rs::ValidateCloudStorageURI` 给出的直接边为：由 `helper.rs::init` 引用；向下调用 `parse.rs::ParseBackend`、`storage.rs::New` 和 `Storage::Close`，并构造 `Options`、`HttpTransport`、`HttpClient`。`UnmarshalDir` 的主要动态边通过 `StorageRef` trait object 和用户闭包发生，图工具只稳定识别到 `UnmarshalDirIter` 构造与 `drop`；源码核验补充了 `Storage::WalkDir`、`Storage::ReadFile`、`Context::check_cancelled` 和 `F` 的调用。

外部 crate 依赖来自 `pkg/objstore/Cargo.toml`：`anyhow` 提供统一错误及路径上下文，`crossbeam-channel` 提供多消费者工作队列和结果通道；线程、原子量、锁、`Arc` 与 `OnceLock` 来自标准库。crate 内部依赖集中在 `parse.rs` 和 `storage.rs`。

Rust 当前调用证据：

- `pkg/objstore/helper_test.rs` 验证多个坏 JSON 文件只产生一个终止错误。
- `pkg/objstore/storage_test.rs` 验证遍历错误、worker 错误以及遍历失败后仍等待已提交 worker。
- `pkg/objstore/helper_2_aster_unit_test.rs` 验证成功产出 `(path, value)` 和反序列化错误包含文件名。
- 除上述测试外，仓库搜索未发现生产 Rust 调用这些公开入口。

Go 的生产调用包括：`pkg/sessionctx/variable/sysvar.go` 通过已注册函数校验系统变量；`pkg/dxf/framework/taskexecutor/manager.go` 读取上传 worker 指标；`br/pkg/stream/stream_metas.go` 与 `br/pkg/restore/log_client/log_file_manager.go` 使用 `UnmarshalDir` 读取元数据。它们是迁移对照证据，不是 Rust 已接线证据。

## 错误处理与边界

- `ValidateCloudStorageURI` 保留 `ParseBackend` 与 `New` 的原始错误链；`Close` 无返回值，无法传播关闭错误。
- `UnmarshalDir` 为读取错误添加 `during reading meta file {path} from storage`，为闭包错误添加 `failed to unmarshal file {path}`，便于定位对象键；`storage_test.rs` 还确认闭包原始错误保留在 `anyhow` 错误链中。
- 首错由 `AtomicBool::swap` 决定，最多向消费者发布一个 worker 错误；`helper_test.rs` 对 16 个坏文件断言错误数恰为 1。
- 若 `WalkDir` 与 worker 都失败，`walk_result.err().or_else(...)` 使遍历错误优先，worker 错误不会再发布。这与 Go 中“若遍历已有错误则不以 `eg.Wait` 覆盖”的语义一致。
- 工作队列发送失败会转换为 `metadata worker queue closed` 并使 `WalkDir` 返回错误。结果消费者提前丢弃时，结果发送失败被有意忽略，worker 可继续收尾。
- worker `join` 的返回值被忽略；若 worker 因 panic 退出且没有先写入错误槽，消费者可能只看到正常结束或已有遍历错误。`Mutex::lock` 使用 `expect`，锁中毒会令协调路径 panic。这些是当前实现的明确边界。
- `UnmarshalDirIter::next` 是阻塞调用且自身不接受 `Context`。若目录遍历或某个存储调用永久阻塞，仅丢弃迭代器不会主动取消后台工作；调用方应持有并取消传入的 `Context`，但取消能否中断进行中的存储操作仍取决于具体后端。

## 并发与资源生命周期

协调线程拥有所有 worker 句柄和结果发送端的一个副本。它在 `WalkDir` 结束后先关闭工作队列，再 `join` worker，最后发布终止错误；因此已经成功解析的结果可以先于终止错误到达，且通道关闭发生在所有 worker 退出之后。`storage_test.rs::unmarshal_dir_waits_for_workers_after_walk_error` 直接验证了这个顺序。

worker 数上限为 128，与 Go 的 `util.NewWorkerPool(128, "metadata")` 对齐。由于路径通道无界，快速遍历可能把大量路径和拥有的 `String` 暂存在内存中；结果通道同样无界，慢消费者不会限制 worker 继续读取和解析。新增大规模目录场景时应评估这一内存风险。

取消由 `Context` 的共享原子标志传播。worker 在取到每条路径后检查一次取消；协调线程本身没有在 `WalkDir` 外另行轮询取消。首错使用 Acquire/Release/AcqRel 排序协调停止标志，具体错误值由 `Mutex` 保护。全局上传计数采用最强的 `SeqCst` 读取，但当前 Rust 上传路径未维护该计数。

`ValidateCloudStorageURI` 对成功构造的存储显式调用 `Close`；`UnmarshalDir` 则通过 `Arc` 克隆管理存储生命周期，最后一个线程/调用方引用释放时才销毁存储对象，不会主动调用 `Storage::Close`。

## 与 Go 版本的对应关系

- 注册：Go `helper.go::init` 直接赋值 `variable.ValidateCloudStorageURI`，随包初始化自动执行；Rust 用显式 `init` 加 `OnceLock`，但当前没有找到调用它的生产接线，也没有把函数写入 Rust session 变量模块。
- URI 校验：两端都调用 `ParseBackend`，禁用 HTTP keep-alive，请求 `ListObjects`、`GetObject`、`AccessBuckets` 权限，成功后关闭存储。差异是 Rust `New` 的云后端依赖 `Options.external_factory`，而本函数没有注入它，故真实云 URI 当前会停在“不支持”错误。
- worker 指标：Go `gcs_extra.go::GCSWriter.readChunk` 在实际上传工作区间执行 `Add(1)`/`Add(-1)`，并由 DXF manager 读取；Rust `gcs_extra.rs` 使用 multipart future，但没有引用 `activeUploadWorkerCnt`，因此只保留了指标 API 形状，未对齐实际计数行为。
- 目录装载：Go 返回 `iter.TryNextor[*T]`，Rust 返回标准 `Iterator<Item = Result<T>>`；Go 的闭包修改预分配的 `T`，Rust 闭包直接返回 `T`。两端都允许最多 128 个并行 worker、结果无序、首错停止、等待已提交 worker，并在读取/反序列化错误中附加路径。
- 错误/取消消费：Go 的每次 `TryNext(ctx)` 可使用消费者提供的 context；Rust `Iterator::next` 没有逐次 context，只依赖创建时传入并共享的 `Context`。这是调用方式和取消粒度上的实际差异。
- Go 的 `UnmarshalDir` 已被 BR 恢复/流元数据生产代码使用；Rust 当前只在测试中出现，不能把 Go 上游调用者视为 Rust 调用者。

## 扩展指南

- 接入系统变量校验时，应由明确的 Rust 启动装配点调用 `helper::init`，并让 session 变量校验路径读取 `registered_cloud_storage_uri_validator`；同时增加独立测试覆盖未注册、首次注册、重复调用和实际调用。不要把测试写入 `helper.rs`。
- 让 `ValidateCloudStorageURI` 真正支持云后端时，优先修改构造依赖注入边界，使它获得生产 `ExternalFactory`，同时保持三项权限及禁用 keep-alive 语义；应同步云后端工厂测试和 URI 校验错误测试，关注凭证泄露、网络连接生命周期与权限兼容性。
- 对齐上传指标时，应在 Rust GCS 真正执行每个分片上传的最小作用域内成对增减 `activeUploadWorkerCnt`，保证错误、取消和 panic 路径不会永久抬高计数；测试应独立放在 `gcs_test.rs` 或相邻测试文件，并覆盖并发与失败清理。
- 扩展 `UnmarshalDir` 时，优先保持四个已验证不变量：最大并行度 128、结果可无序、遍历错误优先、发布终止错误前等待已提交 worker。若改为有界通道或异步 runtime，需要专门分析背压和死锁，尤其是“遍历线程同时负责消费/等待”的循环依赖。
- 若要改变错误策略（收集多错、worker 错误优先或 panic 转错误），应修改 `failed`/`worker_error`/`terminal_error` 三处协调逻辑，并同步 `helper_test.rs`、`storage_test.rs`、`helper_2_aster_unit_test.rs` 以及 Go 对照测试意图。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/objstore/helper.rs` 确认目标文件有 23 个符号；`query`/`node` 确认 `ValidateCloudStorageURI`、`UnmarshalDir`、`GetActiveUploadWorkerCount`、`registered_cloud_storage_uri_validator` 的定义，及 `ValidateCloudStorageURI` 到 `ParseBackend`、`New`、`Close` 的调用边。`callers/callees` 对泛型和常见文件名解析存在噪声，因此生产调用结论另以精确 `rg` 复核。
- Rust 源码：`pkg/objstore/helper.rs`；模块入口：`pkg/objstore/lib.rs`；crate 清单：`pkg/objstore/Cargo.toml`；直接依赖：`pkg/objstore/parse.rs`、`pkg/objstore/storage.rs`；指标实现对照：`pkg/objstore/gcs_extra.rs`。
- Rust 独立测试：`pkg/objstore/helper_test.rs`、`pkg/objstore/storage_test.rs`、`pkg/objstore/helper_2_aster_unit_test.rs`。
- Go 对照：`pkg/objstore/helper.go`、`pkg/objstore/gcs_extra.go`、`pkg/objstore/storage_test.go`；生产调用搜索覆盖 `pkg/sessionctx/variable/sysvar.go`、`pkg/dxf/framework/taskexecutor/manager.go`、`br/pkg/stream/stream_metas.go`、`br/pkg/restore/log_client/log_file_manager.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核所有“当前已接线/未接线”结论均由源码或精确搜索支撑。
