# `pkg/ingestor/engineapi/engine.rs`

## 文件定位

本文件属于 `astersql-ingestor-engineapi` crate，是导入引擎与上层 region-job/IMPORT INTO 流程之间的公共协议层。crate 根 `pkg/ingestor/engineapi/lib.rs` 将 `engine` 模块公开并再导出其中全部符号；`pkg/ingestor/engineapi/Cargo.toml` 指定该 crate 对应 Go 包 `pkg/ingestor/engineapi`，自身仅直接依赖 `astersql-lightning-membuf`（该依赖由同 crate 的 `ingest_data.rs` 使用）。

文件不实现存储、排序或 TiKV ingest。它定义本地引擎和外部排序引擎都必须暴露的 `Engine` trait，以及批次范围、冲突汇总和重复键策略。当前直接实现/适配证据包括 `pkg/ingestor/globalsort/engine_api.rs::ExternalEngineAdapter`、`pkg/ingestor/ingestctrl/import_pipeline.rs::{ExternalEngineSource, LocalEngineSource}`；`pkg/session/runtime/import_sst.rs::CloudEngine` 再把全局排序适配器接到 ingestctrl 的外部引擎边界。

## 核心职责

- 用 `Engine: Send + Sync` 统一引擎身份、数据装载、总量/已导入统计、冲突信息、键范围、Region 切分键与关闭操作。
- 用 `DataAndRanges` 将一份拥有所有权的 `Box<dyn IngestData>` 与若干已排序 `Range` 绑定；每个范围是下游 region job 的输入边界，而本文件本身不创建任务。
- 用 `ConflictInfo` 区分需要做行冲突消解的 PK/UK conflict KV 与更宽泛的 duplicate KV，并提供可累计合并的摘要。
- 用 `OnDuplicateKey` 及四个稳定常量在编码、归并排序、IMPORT INTO 和加索引场景之间传递重复键处理意图，同时保留 Go 命名整数可表达未知值的兼容性。

## 主要符号

- `Range { Start: Vec<u8>, End: Vec<u8> }`：未经重复检测编码的键边界。正常约定为半开区间 `[Start, End)`；源码特别注明 `import_sstpb.SSTMeta` 的结束键语义例外。字段沿用 Go 大写命名。
- `DataAndRanges { Data: Box<dyn IngestData>, SortedRanges: Vec<Range> }`：一次生产者到消费者的所有权转移单元。trait object 允许本地快照与外部排序数据使用同一消费路径。
- `Engine`：公开 trait，要求实现者可跨线程共享。`ID` 返回标识；`LoadIngestData` 通过 `SyncSender` 推送批次；`KVStatistics`/`ImportedStatistics` 分别返回总字节数与总条数、已导入字节数与已导入条数；`ConflictInfo` 返回快照；`GetKeyRange`/`GetRegionSplitKeys` 返回解码后的边界；`Close(&mut self)` 负责实现侧资源收尾。
- `ConflictInfo { Count: u64, Files: Vec<String> }`：冲突 KV 数量及承载这些 KV 的文件名列表。`Default` 表示零冲突、无文件。
- `ConflictInfo::Merge(&mut self, other: &ConflictInfo)`：以 `wrapping_add` 累加计数，并克隆、按原顺序追加 `other.Files`。它不去重、不校验文件、不转移 `other` 所有权。
- `OnDuplicateKey(pub i32)`：开放的整数新类型，而非封闭 Rust enum。常量值依次为 `Ignore=0`、`Record=1`、`Remove=2`、`Error=3`。
- `Display::fmt` 与 `OnDuplicateKey::String`：四个已知值输出 `ignore`、`record`、`remove`、`error`，其他整数输出 `unknown`；显式 `String` 方法保留 Go `fmt.Stringer` 的调用形状。

## 执行流程

典型装载链路如下：

1. 上层以 `Arc<dyn Engine>` 或具体适配器持有引擎，通过 `GetKeyRange` 和 `GetRegionSplitKeys` 取得未携带重复检测编码的导入边界与 Region 切分候选。
2. 生产侧调用 `LoadIngestData(ctx, sender)`。实现构造 `DataAndRanges`，把一份 `IngestData` 所有权和对应 `SortedRanges` 发送到有界同步通道。
3. 消费侧为每个 `Range` 建立 region job，在 `[Start, End)` 内通过 `IngestData` 创建迭代器并写入/ingest 数据；完成后由数据对象记录本批统计和释放引用。具体消费行为属于 `ingest_data.rs` 与 ingest pipeline，不由本文件执行。
4. 运行期间，上层分别查询 `KVStatistics` 和 `ImportedStatistics` 计算进度；需要冲突消解时读取 `ConflictInfo`，多个摘要可通过 `Merge` 聚合。
5. 生命周期结束时调用 `Close`。trait 只规定可失败的关闭边界，文件句柄、reader、缓存或临时数据如何释放由实现负责。

重复键策略走另一条配置链：IMPORT INTO 在 `pkg/dxf/importinto/task_executor.rs::{getOnDupForConflictedKV,getOnDupForKVGroup,getOnDupForIndex}` 选择 engineapi 常量；实际编码/归并实现把它转换为自身策略。`Record` 用于需要保留 PK/UK 冲突证据的路径，`Remove` 用于非唯一二级索引，`Error` 用于遇到重复即失败的路径，`Ignore` 保留历史行为。

## 数据与状态

本文件中的数据结构都不持有锁、线程、运行时句柄或全局状态。`Range`、`ConflictInfo` 和 `OnDuplicateKey` 是值对象；`DataAndRanges` 独占一个 `IngestData` trait object，因此批次发送成功后数据所有权随消息进入消费侧。

`ConflictInfo::Merge` 的两个重要不变量是：计数采用与 Go `uint64` 一致的模 2^64 回绕；文件列表保持“原列表在前、被合并列表在后”的稳定顺序。它只汇总元数据，不保证文件存在或内容有效。

统计二元组的顺序不可交换：`KVStatistics` 是 `(total_kv_size, total_kv_count)`，`ImportedStatistics` 是 `(imported_kv_size, imported_kv_count)`。`GetKeyRange` 返回 `(start_key, end_key)`，而 `GetRegionSplitKeys` 还要求包括本次导入的起止键。这些均是 trait 契约，实际缓存方式和一致性由实现决定；例如 `globalsort/engine_api.rs::ExternalEngineAdapter` 使用原子计数读取已导入统计，并在构造时缓存范围与切分键。

## 依赖与调用关系

下游类型依赖只有标准库 `std::sync::mpsc::SyncSender`，以及同 crate `ingest_data.rs` 中的 `Context`、`EngineError`、`IngestData`。`lib.rs` 公开 `engine` 并再导出接口，使消费者可直接从 crate 根导入这些符号。

主要实现关系：

- `pkg/ingestor/globalsort/engine_api.rs::ExternalEngineAdapter` 实现 `Engine`，把全局排序引擎批次转换为本文件的 `DataAndRanges`；它用 `try_send` 处理有界通道背压，并把取消或通道断开映射为引擎错误。
- `pkg/ingestor/ingestctrl/import_pipeline.rs::ExternalEngineSource` 转发已注册的外部引擎；`LocalEngineSource` 从本地引擎快照构造一批 `DataAndRanges`，让二者进入相同 region-job 流程。
- `pkg/session/runtime/import_sst.rs::CloudEngine` 在会话层转发 `LoadIngestData`、统计、冲突、范围和切分键，并将 globalsort 的重复键、取消、关闭错误转换为 ingestctrl 错误。
- `pkg/dxf/importinto/proto.rs`、`collect_conflicts.rs`、`conflict_resolution.rs` 使用 `ConflictInfo` 聚合和处理冲突；`task_executor.rs` 选择 `OnDuplicateKey`。

RustCodeGraph 对该文件报告 17 个符号并显示被 42 个文件使用。由于 trait 方法存在多个同名实现，泛化的 `callers` 查询无法可靠给出单一静态调用边；上述关系以索引的文件使用边、精确实现源码和 `rg` 的限定引用共同核验。

## 错误处理与边界

`LoadIngestData`、`GetKeyRange`、`GetRegionSplitKeys` 和 `Close` 都返回 `EngineError`；该别名来自 `ingest_data.rs`，使不同引擎实现可以保留具体错误。接口不吞错，也不规定重试。纯查询 `ID`、统计和 `ConflictInfo` 没有错误返回，因此实现需在内部选择一致快照或降级值。

`SyncSender::send` 可能因通道已断开而失败，也可能因有界通道已满而阻塞；trait 注释要求实现维护取消传播和通道语义。真实 globalsort 适配器通过循环 `try_send` 避免永久阻塞，以毫秒级轮询同时观察上下文和内部 token；取消或接收端断开时释放尚未发送的批次数据。engineapi 自身只定义边界，不能据此假设所有实现都采用相同轮询策略。

`Range` 不在构造时验证 `Start <= End`，也不验证排序、重叠或空边界；这些是生产实现和调用方必须维护的前置条件。`OnDuplicateKey` 允许任意 `i32`，未知值只在字符串化时回退为 `unknown`，不会自动报错。`ConflictInfo::Merge` 明确允许计数溢出回绕，与 Go 一致而非饱和或 panic。

## 并发与资源生命周期

`Engine: Send + Sync` 允许共享引用跨线程使用，所以除 `Close` 外的方法必须能在并发调用环境下安全工作；trait 不替实现选择 `Mutex`、原子或消息传递方案。`LoadIngestData` 接收共享引用，意味着装载过程的可变状态必须使用内部可变性保护。`Close(&mut self)` 要求独占可变访问，但部分外层适配器会再以 `Mutex` 或共享关闭方法协调 `Arc` 下的资源收尾。

`SyncSender<DataAndRanges>` 是有界背压边界。发送成功后，接收侧拥有 `Data`；发送失败时，生产实现仍负责释放未转移的数据。`pkg/ingestor/globalsort/engine_api_test.rs` 验证零容量满通道可以由取消打断、未发送数据会释放内存预算，以及接收端丢弃批次后资源计数归零。

本文件不管理 `IngestData` 的引用计数，但 `DataAndRanges` 的生命周期与其直接相连：消费者取得批次后应遵守 `IngestData::{IncRef,DecRef,Finish}` 契约。engineapi 的 `migration_aster_unit_test.rs` 用内存实现验证发送、接收、取消和关闭形状；实际并发释放语义由 globalsort 独立测试补充。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ingestor/engineapi/engine.go`。Rust 保留了 Go 的 `Range`、`DataAndRanges`、`Engine`、`ConflictInfo`、`OnDuplicateKey` 名称、字段顺序和方法语义，并使用 `#[allow(non_snake_case)]`/`#[allow(non_upper_case_globals)]` 保持迁移可追踪性。

关键映射为：Go `IngestData` 接口值对应 `Box<dyn IngestData>`；Go `chan<- DataAndRanges` 对应 `&SyncSender<DataAndRanges>`；Go `error` 对应 `EngineError`；Go `Engine interface` 对应 `Engine: Send + Sync`；Go `OnDuplicateKey int` 对应开放的 `OnDuplicateKey(i32)`；Go `String()` 同时对应 Rust `Display` 和显式 `String()`。

Rust 的 `ConflictInfo` 未携带 Go 的 JSON tags，因为它不是 serde 数据模型；需要协议序列化的上层在自己的 proto/结构中处理。Rust `Merge` 使用 `wrapping_add` 是刻意差异：它显式复现 Go `uint64` 溢出回绕，避免 Rust debug 构建发生 panic。Rust `Close(&mut self)` 比 Go 接口的接收者形状更明确地表达独占关闭，但共享适配器可能提供额外的内部加锁关闭入口。

## 扩展指南

新增引擎能力前先判断它是否真是本地和外部引擎共同契约。若只属于外部引擎（当前 `ConflictInfo` 注释已指出这一历史包袱），优先在具体适配层扩展，避免继续扩大公共 trait。确需新增 trait 方法时，必须同步所有实现：至少检查 `globalsort/engine_api.rs::ExternalEngineAdapter`、`ingestctrl/import_pipeline.rs::{ExternalEngineSource,LocalEngineSource}`、`session/runtime/import_sst.rs::CloudEngine`，以及测试中的 mock 实现。

新增 `OnDuplicateKey` 策略必须保持数值和外部字符串稳定，核对 Go `engine.go`、IMPORT INTO 的三处选择函数、globalsort/简单 SST writer 的策略映射，并为未知值回退保留兼容行为。修改 `Range` 语义时要同步 region-job 消费方和重复检测解码约束；不得把 `SSTMeta` 的结束键例外泛化到普通范围。

测试应继续放在独立文件，不内嵌到 `engine.rs`。公共契约优先扩展 `pkg/ingestor/engineapi/migration_aster_unit_test.rs`；实际背压、取消与资源释放扩展 `pkg/ingestor/globalsort/engine_api_test.rs`；本地/外部 pipeline 接线则在 `pkg/ingestor/ingestctrl` 或 `pkg/session/runtime` 的相邻独立测试中验证。兼容风险集中在 trait 实现遗漏、统计元组顺序、键是否解码、常量数值/字符串漂移；性能风险集中在通道容量、阻塞策略和批次所有权释放。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/ingestor/engineapi` 确认目标、Go 对照、`ingest_data` 与独立测试均已索引；`node --file pkg/ingestor/engineapi/engine.rs --offset 1 --limit 220` 读取全部 146 行、17 个符号及 42 个文件使用关系；`query ConflictInfo`、`query OnDuplicateKey`、`query DataAndRanges` 定位实现、适配器和测试引用。
- 源与 crate 边界：`pkg/ingestor/engineapi/engine.rs`、`lib.rs`、`Cargo.toml`、`ingest_data.rs`。
- Go 对照：`pkg/ingestor/engineapi/engine.go`；类型、常量值、字符串、区间与冲突合并语义逐项核对。
- 直接实现/调用证据：`pkg/ingestor/globalsort/engine_api.rs`、`pkg/ingestor/ingestctrl/import_pipeline.rs`、`pkg/session/runtime/import_sst.rs`、`pkg/dxf/importinto/task_executor.rs`、`pkg/dxf/importinto/proto.rs`。
- 独立测试证据：`pkg/ingestor/engineapi/migration_aster_unit_test.rs` 覆盖冲突合并溢出、文件顺序、策略字符串、通道批次、统计、范围、取消与关闭；`pkg/ingestor/globalsort/engine_api_test.rs` 覆盖真实适配器的数据读取、背压取消与内存释放。任务为纯文档分析，按计划未运行 Cargo 或测试二进制。
