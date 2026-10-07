# `pkg/ingestor/engineapi/ingest_data.rs`

## 文件定位

本文件属于 `astersql-ingestor-engineapi` crate，是 TiKV write + ingest 数据路径的公共协议层。它不读取文件、不生成 SST，也不发起 RPC，而是定义数据源、前向迭代器、取消状态和跨线程错误的共同形状。crate 入口 `pkg/ingestor/engineapi/lib.rs` 将这里的 `Context`、`EngineError`、`ForwardIter`、`IngestData` 公开再导出；相邻的 `engine.rs` 用 `DataAndRanges { Data: Box<dyn IngestData>, SortedRanges }` 把数据和待处理范围交给导入流水线。

所属 crate 由 `pkg/ingestor/engineapi/Cargo.toml` 声明，包名为 `astersql-ingestor-engineapi`，库入口是 `lib.rs`，本文件唯一直接的外部 crate 依赖是工作区内的 `astersql-lightning-membuf`。`[package.metadata.porting]` 将其对应到 Go 包 `pkg/ingestor/engineapi`。

## 核心职责

1. `EngineError` 为开放的引擎错误建立可跨工作线程传递的类型擦除边界。
2. `Context` 提供当前 API 实际需要的最小取消语义：克隆共享一个单向、不可复位的取消标志。
3. `IngestData` 统一本地内存数据与全局排序数据的范围查询、迭代、时间戳、引用计数和成功统计接口。
4. `ForwardIter` 规定批量写入侧所需的单向 KV 游标，以及读取错误、关闭 reader 和提前归还批次缓冲的独立收尾点。

这些职责均是协议而非算法实现。具体实现可见 `pkg/ingestor/ingestctrl/import_pipeline.rs` 的 `LocalData`/`DataIter`，以及 `pkg/ingestor/globalsort/engine_api.rs` 的 `DataAdapter`/`IterAdapter`。

## 主要符号

- `pub type EngineError = Box<dyn Error + Send + Sync + 'static>`：保留 Go `error` 的开放性，同时要求错误拥有静态生命周期并可安全跨线程移动、共享。调用方不能依赖某个固定错误枚举，若需分类应沿具体实现保存的错误链处理。
- `pub struct Context { cancelled: Arc<AtomicBool> }`：字段私有，只能通过公开方法操作。`background()` 创建未取消上下文；`cancel()` 以 `SeqCst` 写入；`is_cancelled()` 以 `SeqCst` 读取。`Clone` 复制 `Arc`，不是复制取消值。
- `pub trait IngestData: Send + Sync`：可作为并发任务共享的 trait object。六组能力分别为范围首尾键 `GetFirstAndLastKey`、构造游标 `NewIter`、读取事务时间戳 `GetTS`、成对引用管理 `IncRef`/`DecRef`、成功进度 `Finish`。
- `GetFirstAndLastKey(&self, lowerBound, upperBound)`：查询半开区间 `[lowerBound, upperBound)` 内的第一和最后一个实际键。空切片表示该方向无界；非空上下界由调用方保证有序。无数据用 `(None, None)` 表达，底层失败用 `Err` 表达。
- `NewIter(&self, ctx, lowerBound, upperBound, bufPool)`：返回 `Box<dyn ForwardIter>`。构造本身不返回 `Result`，所以初始化或后续 I/O 错误需由具体迭代器的 `Error()` 暴露；取消也由迭代器在运行期间观察。
- `GetTS()`：返回同一批数据同时使用的 start TS 与 commit TS。接口不生成或校验时间戳。
- `IncRef()`/`DecRef()`：为一份数据被多个 region job 共享提供显式生命周期协议。trait 不内置计数，也不能自行强制成对调用。
- `Finish(totalBytes, totalCount)`：一次成功分段导入的增量通知；同一数据可能多次调用，具体实现负责线程安全累加或转发。
- `pub trait ForwardIter: Send`：游标本身可移动到工作线程，但不要求 `Sync`，因而预期由一个执行流可变地推进。`First` 定位首项，`Valid` 判断当前位置，`Next` 单向前进，`Key`/`Value` 借用当前位置数据，`Close` 关闭资源，`Error` 读取游标状态错误，`ReleaseBuf` 只归还历史 KV 缓冲。

## 执行流程

典型生产链路由目标文件之外的实现组织：

1. `Engine::LoadIngestData` 通过同步通道发送 `DataAndRanges`（`pkg/ingestor/engineapi/engine.rs`）。
2. `pkg/ingestor/ingestctrl/import_pipeline.rs` 的加载线程接收批次，把 `Box<dyn IngestData>` 转成 `Arc<dyn IngestData>`，再按 `SortedRanges` 生成 region jobs。
3. 每个 job 交给 worker 前，`RegionJob::ref` 经 `JobResources::reference` 先增加 pending 数，再调用 `IngestData::IncRef`。这一顺序保证任务已经计入流水线生命周期后才对数据建立实现级引用。
4. worker 将任务推进到 `Ingested` 时，`RegionJob::convertStageTo` 调用 `JobResources::finish`，进而调用 `IngestData::Finish(bytes, count)` 并累加组级统计。
5. 成功、终止、重试清理或未发送任务的 `Drop` 路径最终调用 `RegionJob::done`；`JobResources::done` 先执行可能阻塞的 `DecRef`，再减少 pending 并通知等待者。重扫产生多个子任务时先补充引用，再共享同一资源对象。
6. 需要实际扫描数据时，实现通过 `NewIter` 创建游标，遵循 `First → (Valid → Key/Value → Next)* → Error → Close`；若一批旧键值不再需要，可在关闭前调用 `ReleaseBuf` 归还缓冲。目标 trait 只规定顺序和生命周期，具体读取发生在实现/适配层。

取消流程与数据流程正交：持有同一 `Context` 的各方可用 `cancel()` 广播取消，迭代器在 `First`、`Valid` 或 `Next` 等实现定义的检查点停止。取消不会自动调用 `Close`、`ReleaseBuf` 或 `DecRef`，调用流水线仍须走资源清理路径。

## 数据与状态

目标文件自身只有一个运行时状态：`Context.cancelled`。其 `Arc<AtomicBool>` 让所有克隆观察同一状态，默认值为 `false`，一旦置为 `true` 没有恢复 API。`SeqCst` 提供全序可见性，但这里不携带取消原因、截止时间或父子上下文关系。

`IngestData` 不保存状态，只规定实现必须管理的几类状态：有序 KV 及其范围、批次 TS、region job 引用数、已成功导入的字节数与 KV 数。`ForwardIter` 的隐含状态包括“尚未 First”“当前项有效”“已到末尾”“发生错误”“已关闭/已释放缓冲”。调用 `Key`/`Value` 前必须确保 `Valid()`；接口返回借用切片，Rust 借用检查阻止在持有该引用时可变推进游标，但实现仍须保证当前位置数据在借用期内稳定。

范围使用字节序的半开区间。空 bound 是无界哨兵，不等同于空键值的普通边界。空范围返回两个 `None`；仅有一个匹配键时首键和尾键应是同一个键。`pkg/ingestor/engineapi/migration_aster_unit_test.rs` 与 globalsort 的 Rust/Go 对照测试覆盖了这些情况。

## 依赖与调用关系

- 上游声明：`pkg/ingestor/engineapi/lib.rs` 公开本模块；`engine.rs::DataAndRanges` 持有 `Box<dyn IngestData>`。
- 上游生产者：`pkg/ingestor/globalsort/engine_api.rs::ExternalEngineAdapter::LoadIngestData` 产生由 `DataAdapter` 包装的数据；`pkg/ingestor/ingestctrl/import_pipeline.rs` 也用 `LocalData` 实现本 trait。
- 上游生命周期调用：`import_pipeline.rs::JobResources::{reference, done, finish}` 分别调用 `IncRef`、`DecRef`、`Finish`；`job_worker.rs::RegionJob::{ref, done, convertStageTo}` 将它们接到任务状态机。
- 下游依赖：`NewIter` 接受 `membuf::Pool`，允许实现从共享池取得可跨多次推进保留的批次存储。目标文件不分配池内存，也不决定池是否线程安全。
- 具体全局排序适配：`globalsort/engine_api.rs::DataAdapter` 转换空 `Vec<u8>` 与 `Option` 的差异，并把构造错误保存在 `IterAdapter.error`；`IterAdapter` 在取消时记录 `Error::Cancelled`，关闭时取走并关闭内部游标。
- 具体本地适配：`import_pipeline.rs::LocalData` 按范围复制 KV 到 `DataIter`，用 `AtomicUsize` 实现引用计数；`DataIter::Valid` 检查共享取消状态。其 `Finish` 和 `ReleaseBuf` 当前为空实现，这是该实现的现状，不是接口允许所有实现忽略统计或缓冲管理的证明。

RustCodeGraph 的文件索引显示 `ingest_data.rs` 被 13 个文件引用；精确符号查询还显示 Go 侧 `IngestData` 被 globalsort、ingestctrl 的 engine/job worker/local 等路径使用。由于 trait 方法动态分发，图索引未为每个 Rust trait 调用生成完整静态调用边，因此实现与调用点另用局部 `rg` 补齐。

## 错误处理与边界

`GetFirstAndLastKey` 和 `Close` 可直接返回 `EngineError`；`NewIter` 不能直接返回错误，迭代器须通过 `Error()` 保存并暴露错误。布尔返回值只表示位置是否有效，不能单独区分正常结束、取消和 I/O 失败，调用者结束循环后仍应检查 `Error()`。

`Key`/`Value` 不返回 `Option` 或 `Result`，错误顺序由调用协议约束：在未成功 `First`、`Valid == false`、`Close` 后或实现已使位置失效时调用，具体实现可能 panic。`ReleaseBuf` 会使之前取得的数据失效；当前 Rust 签名通过借用规则覆盖同一调用栈内的基本安全性，但跨批持有数据应复制为自有 `Vec<u8>` 或由具体实现提供等价 guard。

接口不会验证 `lowerBound < upperBound`、引用计数下溢、`Finish` 参数非负或 `DecRef` 次数。实现和调用者必须共同维持这些不变量。`EngineError` 也不规定取消的统一错误类型：例如 globalsort 适配器保存具体 `Error::Cancelled`，而本地 `DataIter` 仅让 `Valid` 变为 false 且 `Error` 仍为 `None`；消费者若需要一致的取消诊断，应在接口演进时显式定义。

## 并发与资源生命周期

`IngestData: Send + Sync` 明确允许多个 region job 并发共享同一实现，所以引用计数、完成统计、reader/文件释放等内部可变状态必须同步。目标文件不提供锁；现有实现分别使用原子量或把操作转发到底层并发安全对象。`Finish` 可重复乃至并发发生，不能按“一份数据只调用一次”设计。

`ForwardIter: Send` 而非 `Send + Sync`，表示游标可转移线程所有权，但不应被多个线程同时推进。`Close` 与 `ReleaseBuf` 是不同资源层级：前者关闭 reader 并释放所持资源，后者只提前释放承载历史键值的缓冲；正常路径应确保最终 `Close`，即使已调用 `ReleaseBuf`。

生产流水线特意让 `DecRef` 发生在 pending 计数递减之前（`JobResources::done`），因此等待 pending 归零也意味着数据实现的清理已经完成。`OwnedJob::Drop` 为取消和通道发送竞争提供兜底，避免未成功提交的任务泄漏引用。`Context` 的 `Arc` 生命周期与 `IngestData` 引用计数相互独立，丢弃或取消上下文不会替代 `DecRef`。

## 与 Go 版本的对应关系

权威对照是同目录 `pkg/ingestor/engineapi/ingest_data.go`。Rust 保留了 Go 接口的方法集合、PascalCase 名称、半开范围、空 bound、同一 TS 用作 start/commit、显式引用计数、可多次 `Finish`，以及 `ForwardIter` 的读取和缓冲生命周期。

主要表示差异如下：

- Go 的 `error` 映射为 `EngineError`，额外增加 `Send + Sync + 'static` 约束。
- Go 的 `context.Context` 映射为本地最小 `Context`，目前只有取消布尔值，没有 deadline、value、Done channel 或取消原因。
- Go 的 `nil, nil, nil` 映射为 `Ok((None, None))`；非空键由拥有所有权的 `Vec<u8>` 返回。
- Go `ForwardIter.Error() error` 的 nil 映射为 `Option<&EngineError>`；Rust 只借用迭代器保存的错误。
- Go 接口值映射为 `Box<dyn IngestData>`，进入多任务流水线后再转为 `Arc<dyn IngestData>`。Rust 的 `Arc` 只管理 trait object 自身的内存，不能替代 Go 语义中的 `IncRef`/`DecRef`，后者可能管理底层 reader、文件或缓存。
- Go 切片的失效约定由注释规定；Rust 返回切片借用并要求实现维持其底层存储，但 `ReleaseBuf` 后仍需调用方遵守协议，不能保留自有指针绕过借用系统。

Go 测试 `pkg/ingestor/globalsort/engine_test.go::TestMemoryIngestData` 验证无界、闭左开右、空结果、单键、重复键和前向遍历；Rust 的 `pkg/ingestor/globalsort/engine_test.rs::test_memory_ingest_data` 保留相同用例。`pkg/ingestor/engineapi/migration_aster_unit_test.rs::migration_ingest_data_range_iteration_ref_and_finish_match_go` 另以独立假实现验证接口级范围、释放、引用归零和 `Finish` 累加。

## 扩展指南

- 新增 `IngestData` 实现时，应先决定范围数据是否有序、空 bound 如何解释、初始化错误如何装入迭代器，并为 `IncRef`/`DecRef` 的归零释放与并发 `Finish` 选择原子或锁策略。
- 新增 `ForwardIter` 实现时，应定义所有状态迁移，确保 `First`/`Next` 的 false 与 `Error` 配合，`Close` 幂等或明确非幂等，并让 `ReleaseBuf` 不隐式关闭 reader。若从 `membuf::Pool` 分配，必须证明 `Key`/`Value` 数据直到下一次明确释放前有效。
- 若扩充 `Context`（如 deadline 或取消原因），必须同步 `LocalData::NewIter`、`DataAdapter::NewIter` 及所有 engine/mock 实现，避免不同数据源呈现不同取消结果。
- 若修改 trait 方法签名或错误模型，需同步 `engine.rs::DataAndRanges` 的 trait object 使用、`ingestctrl/import_pipeline.rs`、`globalsort/engine_api.rs` 和独立测试；不要把单元测试嵌入本生产文件，继续放在 `migration_aster_unit_test.rs` 或对应实现的 `*_test.rs`。
- 兼容风险集中在 Go 对齐、动态 trait object 的对象安全和取消语义；正确性风险集中在半开范围、重复键顺序、引用不配对和结束后错误检查；性能风险集中在为延长 KV 生命周期而复制数据、锁竞争及过强原子顺序。修改前后应同时跑接口迁移测试和具体实现的范围/生命周期测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录列出 `ingest_data.rs`、Go 对照、模块入口与独立迁移测试。
- RustCodeGraph `node --file pkg/ingestor/engineapi/ingest_data.rs`：核对完整 128 行及 21 个符号；`query IngestData --kind trait` 定位本 trait；`query MemoryIngestData --kind struct` 定位具体 Go/Rust 实现与相关测试。
- RustCodeGraph 源码节点：`pkg/ingestor/engineapi/engine.rs`、`pkg/ingestor/ingestctrl/import_pipeline.rs`、`pkg/ingestor/ingestctrl/job_worker.rs`、`pkg/ingestor/globalsort/engine_api.rs`，用于核对批次所有权、动态分发、引用/完成顺序、取消和关闭行为。
- crate 与包契约：`pkg/ingestor/engineapi/Cargo.toml`、`pkg/ingestor/engineapi/lib.rs`、`pkg/ingestor/doc.go`。
- Go 对照：`pkg/ingestor/engineapi/ingest_data.go`；Go 测试证据：`pkg/ingestor/globalsort/engine_test.go::TestMemoryIngestData` 及 `pkg/ingestor/ingestctrl/local_test.go` 的 mock 迭代器和阻塞 `DecRef` 实现。
- Rust 测试证据：`pkg/ingestor/engineapi/migration_aster_unit_test.rs::migration_ingest_data_range_iteration_ref_and_finish_match_go`、`migration_engine_channel_context_statistics_and_ranges_match_go`，以及 `pkg/ingestor/globalsort/engine_test.rs::test_memory_ingest_data`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰好包含上述 11 个固定二级章节，并人工复核只新增本文档、未修改 Rust/Go/Cargo/`plan.md`。
