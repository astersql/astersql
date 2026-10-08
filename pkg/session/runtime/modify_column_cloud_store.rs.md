# `pkg/session/runtime/modify_column_cloud_store.rs` 逻辑说明

## 文件定位

`modify_column_cloud_store.rs` 属于 `astersql-session` crate（见 `pkg/session/Cargo.toml`），由 `pkg/session/runtime.rs` 的 `mod modify_column_cloud_store;` 直接纳入 session runtime。它位于分布式 MODIFY COLUMN / 索引回填的云端 global-sort 数据通路中，负责把仓库内两套对象存储 API 统一适配为 `astersql_ingestor_globalsort::Storage`，并额外实现 `astersql_ingestor_simplesst::writer::WriterSink`。

该类型不是通用公开 API：`CloudStore` 为 `pub(super)`，只对 `runtime` 的父模块范围可见。当前直接构造点包括 `modify_column_cloud_planner.rs::Planner::cloud_plans`、`modify_column_cloud_executor.rs::CloudStep::RunSubtask`、`modify_column_dist_backfill.rs` 的读取/写外存路径，以及独立测试。文件没有 feature gate 或条件编译项；测试由 `pkg/session/runtime.rs` 通过独立文件 `modify_column_cloud_store_test.rs` 接入，生产源中不内嵌测试。

## 核心职责

- `CloudStore::open` 解析存储 URI，S3 后端走新的 `storeapi::Storage` / `s3store` 实现，其余后端走 legacy `objstore::storage::StorageRef`，对 globalsort 上层隐藏 API 差异。
- `impl sort::Storage for CloudStore` 提供文件大小、全量/范围读取、流式创建、整文件读写、批量删除和前缀遍历，并声明对象内记录采用 Go 兼容的 64 位大端长度格式。
- `Reader` 与 `Writer` 把底层对象 reader/writer 适配为标准库 `Read` / `Write` 和 globalsort `ObjectWriter`，在每次 I/O 前传播子任务取消，并在生命周期结束时关闭底层资源。
- `impl WriterSink for CloudStore` 让 simple-SST writer 可以把受其内存预算约束的 flush 块直接流式上传到同一对象存储，而无需额外复制出另一套存储实现。

本文件只提供传输和资源生命周期边界，不决定对象命名、归并分组、任务元数据格式或清理时机；这些分别由 `modify_column_cloud_planner.rs`、`modify_column_cloud_executor.rs`、`modify_column_cloud_meta.rs`、`modify_column_dist_backfill.rs` 以及任务级 cleaner 决定。

## 主要符号

- `failure(error) -> sort::Error`：把任意可显示错误压平成 `sort::Error::InvalidData(String)`，用于对象存储 API 到 globalsort API 的边界转换。它保留文本，但不保留底层错误类型。
- `io_failure(error) -> std::io::Error`：把底层错误转成 `std::io::Error::other`，用于标准库 `Read` / `Write` 接口。
- `enum Transport`：`Local(legacy::storage::StorageRef)` 保存 legacy 后端；`Cloud(Arc<dyn api::Storage>)` 保存新的 store API 后端。名称 `Local` 表示 legacy API 分支，不等同于只允许本地文件系统：除 S3 外的 `ParseBackend` 结果都会进入此分支。
- `CloudStore { transport, local_context, context }`：同时保存实际传输和两套带取消信号的 context。两个 context 源自同一个 `Arc<AtomicBool>`，保证分支切换不改变取消语义。
- `CloudStore::open(uri, cancelled) -> Result<Arc<Self>, String>`：文件唯一的构造入口。它先解析 URI；S3 字段逐项从 legacy `StorageBackend::S3` 转写到 `s3::backuppb::S3`，再调用 `astersql_objstore_s3store::NewS3Storage`；其他后端调用 `legacy::storage::NewWithDefaultOpt`。
- `enum Reader`：保存底层 reader 及其对应 context。`Read::read` 在每次读取前检查取消；cloud 分支还会在底层读取失败后再次判断取消，以把竞态期间的底层错误稳定映射为 `sort::Error::Cancelled`。`Drop` 总会尝试 `close`。
- `enum Upload` 与 `struct Writer(Option<Upload>)`：`Option` 使 `Writer::close` 幂等；第一次关闭通过 `take()` 取得所有权，后续 `finish` 或 `Drop` 返回成功而不会重复关闭。`Write::flush` 是 no-op，持久完成点是 `finish`/底层 `close`。
- `impl sort::Storage for CloudStore`：实现 `file_size`、`record_format`、`open`、`open_at`、`create`、`read`、`write`、`delete_files`、`list_prefix`。`open` 明确委托给 `open_at(path, 0)`。
- `impl WriterSink for CloudStore::write_file`：通过 `Storage::create`、`write_all`、`finish` 完成 simple-SST 文件写入，确保上传关闭错误能返回给调用者。

## 执行流程

1. 上游从 DDL task meta 或 session runtime 配置得到 `cloud_storage_uri` 和共享取消标记，调用 `CloudStore::open`。
2. `open` 为 legacy API 和 store API 分别构造 context，二者都引用同一个取消原子量；随后用 `legacy::parse::ParseBackend` 解释 URI。
3. 若解析结果为 S3，代码显式复制 endpoint、region、bucket、prefix、鉴权、加密、ACL、path-style、role/profile 等字段，创建 `s3store` 并放入 `Transport::Cloud`。其他 backend 交给 `NewWithDefaultOpt` 并放入 `Transport::Local`。
4. globalsort 通过 `Storage` trait 使用实例。所有公开存储操作先执行 `self.context.check()`；随后按 `Transport` 选择对应 API，并把错误转为 `sort::Error`。
5. `open_at` 将 `u64` 偏移安全转换为 `i64`，在底层 reader option 中设置起始偏移且不设置结束偏移。返回的 `Reader` 直接从该范围起点输出数据，不再二次跳过字节。
6. `create` 返回包装了底层上传对象的 `Writer`。globalsort 或 simple-SST 连续调用 `write`；每次写入都先检查取消。正常完成时 `ObjectWriter::finish` 调用幂等 `close`，异常提前退出时 `Drop` 仍尝试关闭。
7. `read`/`write` 是整对象便捷路径；`delete_files` 批量删除；`list_prefix` 用 `WalkDir` 回调收集所有匹配对象路径。它们不在本文件内排序、去重或重写路径。
8. simple-SST 调用 `WriterSink::write_file` 时复用上述流式 create/write/finish 路径；注释指出单次 flush 的内存上限已由 simple-SST writer 控制。

## 数据与状态

`CloudStore` 的稳定状态只有传输实现和两个 context；创建后没有本文件自有的可变集合、缓存或计数器。`Arc<Self>` 允许 planner、读/写 worker、merge operator 和 external engine 共享同一个逻辑 store；实际每次 `open_at`/`create` 都创建独立 reader/upload，避免跨 worker 共享流位置。

取消状态由调用方提供的 `Arc<AtomicBool>` 所有。`local_context` 与 `context` 各自封装该标记，因而无论后端落在哪个 API 分支，都观察相同的子任务取消。源码没有在此处修改取消标记，也没有自行启动线程或任务。

`Writer(Option<Upload>)` 的 `Some -> None` 是唯一局部状态转换：关闭时取走上传对象，从而保证 `finish` 后的析构不会重复 close；若调用者在 finish 后继续 `write`，会得到 `DDL cloud upload closed`。`Reader` 没有显式 closed 状态，由所有权和 `Drop` 保证单次析构关闭。

`record_format` 固定返回 `sort::RecordFormat::GoBigEndian64`。globalsort 的 reader/split/merge 代码据此使用 Go 格式的记录边界；这不是存储后端自动探测的属性，变更会直接影响既有对象的读取兼容性。

## 依赖与调用关系

上游构造关系由源码与仓库搜索确认：

- `modify_column_cloud_planner.rs::Planner::cloud_plans` 打开 store，读取前序 subtask 外置元数据并写入下一阶段计划元数据。
- `modify_column_cloud_executor.rs::CloudStep::RunSubtask` 使用与导入控制共享的取消标记打开 store，交给 merge operator 或 external engine。
- `modify_column_dist_backfill.rs` 在云 URI 非空时打开 store，并注入 `modify_column_pipeline::CloudWriteConfig`，使读取索引阶段的 writer 把 simple-SST 数据/统计对象写到 `<task>/<subtask>` 前缀。
- `modify_column_cloud_store_test.rs` 与 `modify_column_cloud_executor_test.rs` 构造真实本地文件后端进行回归验证。

直接下游包括 `astersql-objstore` 的 URI 解析、legacy storage/context、reader/writer/walk API；`astersql-objstore-storeapi` 的新 context/storage API；`astersql-objstore-s3like` 的 S3 protobuf 与 object I/O traits；`astersql-objstore-s3store::NewS3Storage`；以及 `astersql-ingestor-globalsort` 和 `astersql-ingestor-simplesst` 的存储接口。上述依赖均在 `pkg/session/Cargo.toml` 中显式声明；crate 的 `nextgen` feature 不改变本文件逻辑。

RustCodeGraph `query CloudStore` 将 `run_merge`、`run_import`、cloud planner 和独立测试识别为相关使用点；精确方法 callers/callees 对同名 `open` 消歧不完整，因此本说明以索引定位加直接源码引用交叉确认具体边，不将空查询结果解释为“没有调用者”。

## 错误处理与边界

构造阶段的 URI 解析、S3 初始化或 legacy store 初始化错误被转为 `String`，由上游再包装为 DDL executor 错误。trait 操作大多经 `failure` 变成 `sort::Error::InvalidData`；取消是例外，会明确返回 `sort::Error::Cancelled`，便于 globalsort 停止工作。

所有存储入口先检查 `self.context`，reader/writer 的每次流式 I/O 又检查与实际分支配套的 context。这一双层检查覆盖“调用开始前已取消”和“长流操作过程中取消”。cloud reader 在底层错误后重新查询取消状态，避免取消与网络错误竞态时向上暴露随机传输错误；legacy reader 依赖预检查和底层 reader 返回值。

`open_at` 拒绝不能转换为 `i64` 的巨大偏移。`file_size` 同样用 `u64::try_from` 拒绝负数或不可表示的底层结果。范围读取只设置 start offset，不做文件大小预检，也不在本层限制超出 EOF 的行为。

`Drop` 无法返回错误，因此 reader/writer 关闭失败只写入标准错误；`finish` 则会把 writer 关闭错误返回给正常调用链。注释明确采用 Go 的 defer-close 语义：即使 merge 失败也关闭上传，同时保留主错误；未完成对象的删除属于持久云任务清理，而非本文件析构逻辑。`flush` 不保证持久化，调用者必须调用 `finish` 或让对象析构。

S3 是唯一切到新 store API 的解析后端；其他后端即使是远端存储也走 legacy 分支。新增 backend 时不能根据 `Transport::Local` 名称推断其部署位置。`list_prefix` 只转发 `WalkDir` 结果，没有稳定顺序保证，调用者不应依赖列表顺序。

## 并发与资源生命周期

`CloudStore` 通过 `Arc` 跨 worker 共享，底层 trait 对象满足相应的线程安全约束；文件自身没有锁。每个 worker/操作持有独立的 `Reader` 或 `Writer` 和 context 克隆，因此读取游标、上传状态与关闭动作互不共享。`Writer` 并未提供内部同步，同一个 writer 仍应由单一调用流顺序使用。

reader 生命周期从 `open/open_at` 成功开始，到包装对象析构结束；无论正常读完还是错误提前返回，`Drop` 都尝试 close。writer 生命周期从 `create` 成功开始，推荐由 `finish` 明确结束；`finish` 消耗 boxed writer 并传播 close 错误，随后的 `Drop` 因内部已为 `None` 不会重复关闭。若调用链提前退出，`Drop` 执行兜底 close，但只能记录错误。

取消标记的生命周期由 `Arc` 保持到 store 及所有派生 reader/writer 释放。它与 `CloudStep` 的 cancellation token、读取阶段 `ImportControl` 使用同一根标记，使任务取消能同时到达对象 I/O、归并和导入。对象删除不属于 reader/writer 生命周期：Go 的 `BackfillCleaner.Clean` 和对应 Rust 持久任务清理负责按 task 前缀删除残留文件。

## 与 Go 版本的对应关系

Go 路径没有一个与本文件同名、逐类型对应的 `cloud_store.go`；Rust 把散布在 Go DDL 与 objstore/global-sort 调用点中的存储行为集中成适配器。`pkg/ddl/backfilling_merge_sort.go` 使用 `handle.NewObjStoreWithRecording` 打开 `storeapi.Storage` 并交给 `globalsort.NewMergeOperator`；`pkg/ddl/backfilling_operators.go::NewWriteIndexToExternalStoragePipeline` 把外部 store 交给 simple-SST 写入管线；`pkg/ddl/backfilling_clean_s3.go::BackfillCleaner.Clean` 用 `objstore.ParseBackend`、`NewWithDefaultOpt` 和 `globalsort.CleanUpFiles` 执行任务级清理。

语义对齐点包括：按 URI 创建对象存储；全量和带起始偏移的读取；通过 writer close/finish 提交上传；前缀遍历与批量删除；取消 context 贯穿 I/O；归并失败仍 defer/析构关闭对象；对象记录格式采用 Go 的大端 64 位长度编码。Rust 独立测试对 `open_at(75)` 明确验证底层 range 已定位后不会再次跳过前缀，这是与 Go range-reader 语义兼容的关键边界。

当前可见差异是 Rust 对 S3 单独使用新 `s3store`，其余后端保留 legacy API，并把错误压平为字符串/`InvalidData`；Go merge executor 还通过 recording wrapper 汇总对象存储请求，Rust `CloudStore` 本身没有计量或请求摘要。Rust 的 `Drop` 关闭失败仅 `eprintln!`，Go 通常在 defer 中调用 `Close`，部分位置同样不能改变已经形成的主错误。不能由本文件推断两边拥有完全相同的 backend 支持、日志字段或指标。

## 扩展指南

新增存储后端时，首先确认 `ParseBackend` 产生的枚举和目标 Rust API。若后端需要新 store API，应给 `Transport` 增加明确变体或建立完整转换，不能默认落入名为 `Local` 的 legacy 分支；同时同步 `open`、所有 `Storage` 方法、reader/writer close 语义和 context 取消检查。

修改 S3 配置转换时，应逐项核对 Go protobuf/URI parser 的字段，尤其是 prefix、path-style、临时凭证、role/external ID、SSE/KMS 和 profile；漏字段可能表现为权限、寻址或加密兼容问题。不要把秘密字段写入错误或日志。

新增 globalsort 存储操作时，应同时实现两套 backend 分支，保持入口预检查取消、错误映射和资源关闭行为一致。若改变 `record_format` 或 `open_at`，必须检查 `pkg/ingestor/globalsort/{reader,split,merge}.rs` 的格式分支和已有 Go 对象兼容性；偏移只能在底层或上层应用一次。

修改流式写入时，应保留 `finish` 可报告 close 错误、`Drop` 可兜底且幂等的性质。若底层增加真正的 flush 语义，需明确它是否只是发送缓冲还是保证对象可见，不能继续用无条件 no-op 掩盖契约差异。

回归测试应扩展独立文件 `pkg/session/runtime/modify_column_cloud_store_test.rs`，不要写进生产源。现有测试覆盖真实本地文件上的 simple-SST 写入、range read、元数据解码、前缀遍历/删除，以及取消后禁止读写；新增 backend 或错误路径时还应覆盖 URI 字段映射、超大 offset、close/finish 失败、重复 finish、读取中取消和无序列表假设。需要验证上层 merge/import 生命周期时，同步扩展 `modify_column_cloud_executor_test.rs`。正确性风险集中在偏移/记录格式和关闭提交；兼容风险集中在 URI/S3 字段与旧对象；性能风险集中在额外复制、缓冲和把流式操作误改成整对象操作。

## 验证依据

本说明直接核对了目标源 `pkg/session/runtime/modify_column_cloud_store.rs`、模块入口 `pkg/session/runtime.rs`、上游 `modify_column_cloud_planner.rs`、`modify_column_cloud_executor.rs`、`modify_column_dist_backfill.rs`、`modify_column_pipeline.rs`，crate 声明 `pkg/session/Cargo.toml`，独立测试 `modify_column_cloud_store_test.rs` 与 `modify_column_cloud_executor_test.rs`，以及 globalsort trait/格式使用位置 `pkg/ingestor/globalsort/lib.rs`、`reader.rs`、`split.rs`、`merge.rs`。`pkg/session` 下不存在 `doc.go`。

Go 对照读取了 `pkg/ddl/backfilling_merge_sort.go`、`pkg/ddl/backfilling_operators.go`、`pkg/ddl/backfilling_dist_scheduler.go`、`pkg/ddl/backfilling_clean_s3.go`，并用 `pkg/objstore/parse.go`、`storage.go`、`storeapi/storage.go` 确认对象存储边界。DDL 框架定位参考了 `docs/agents/ddl/README.md`，具体结论仍以代码和测试为准。

RustCodeGraph `status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query CloudStore` 定位目标类型，并给出 `run_merge`、`run_import`、cloud planner 与测试关联；`node CloudStore` 核对了结构字段。组合 explore 和精确同名 `open` callers/callees 查询分别超时或发生消歧污染，因此具体调用边由索引结果、`rg` 引用位置和直接源码交叉确认，未把工具空输出当作否定证据。

本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定命令确认文件存在且恰有 11 个固定二级章节，并人工复核文档覆盖文件存在原因、后端选择、读写/取消/关闭流程、Go 对照和安全扩展入口。
