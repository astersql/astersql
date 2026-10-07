# `pkg/ingestor/globalsort/engine_api.rs`

## 文件定位

本文件是 globalsort 外部引擎与通用导入接口之间的适配层。底层的
[`Engine`](engine.rs) 负责从外部存储加载、排序和去重 KV，并以
`MemoryIngestData`/`MemoryDataIter` 表示内存批次；本文件把这些具体类型包装为
`astersql_ingestor_engineapi` crate 定义的 `Engine`、`IngestData` 和
`ForwardIter` trait，使通用的 region-job 导入流水线无需依赖 globalsort 的内部类型。

模块入口位于 [`engine.rs`](engine.rs) 的 `#[path = "engine_api.rs"] mod api`，并仅向
crate 使用者重新导出 `ExternalEngineAdapter`。当前可见的生产上游是
[`pkg/dxf/importinto/write_ingest_backend.rs`](../../dxf/importinto/write_ingest_backend.rs)：
`GlobalSortWriteIngestBackend::CloseExternalEngine` 构造并保存适配器，
`ImportEngine` 再把它交给 `local::import_pipeline::do_import`。因此，本文件位于
“外部存储的全局排序结果”到“按 Region 写入并 ingest TiKV”的类型与生命周期边界上。

该文件不是独立存储引擎，也不实现排序、去重、Region 扫描或写入 RPC；这些职责分别
留在 [`engine.rs`](engine.rs)、导入流水线和 transport 中。

## 核心职责

1. `ExternalEngineAdapter` 实现公共 `api::Engine`，把 globalsort `Engine` 的加载、
   统计、键范围、切分键、冲突信息和关闭操作暴露给通用导入流水线。
2. `LoadIngestData` 将内部 `DataAndRanges` 转换成公共 `api::DataAndRanges`，并通过
   有界 `SyncSender` 逐批发送，避免先收集全部批次而使活跃内存翻倍。
3. 在公共 `api::Context` 取消时联动取消底层 `CancellationToken`；在通道满时以短轮询
   保持取消可观察，在接收端断开或批次尚未发送即取消时主动释放批次数据。
4. `DataAdapter` 实现 `api::IngestData`，保留范围查询、时间戳、引用计数和导入统计语义。
5. `IterAdapter` 实现 `api::ForwardIter`，把构造迭代器时发生的错误保存到迭代器状态，
   并在 `First`/`Next`/`Valid` 时观察上下文取消。
6. 在底层引擎被 `Mutex` 独占加载的同时，将不依赖可变引擎的资源控制、静态统计和
   原子导入统计保存到适配器字段，使这些信息仍可并发读取。

## 主要符号

- `MonitorStop<'a>(&'a AtomicBool)`：作用域清理哨兵。其 `Drop` 用 Release 顺序写入
  `true`，保证 `LoadIngestData` 即使在中途返回时也会通知取消监控线程退出。
- `ExternalEngineAdapter`：唯一公开类型。
  - `engine: Mutex<Engine>` 串行化加载、冲突信息、关闭以及依赖引擎可变状态的查询。
  - `controls: Arc<EngineResource>` 是从引擎取得的共享资源控制句柄。
  - `size`/`count`、`range`、`splits` 是构造时快照；读取它们不需要等待引擎锁。
  - `imported_size`/`imported_count` 是与引擎共享的原子计数器。
  - `token` 是底层加载流程使用的取消令牌。
- `ExternalEngineAdapter::new(engine, token)`：在取得 `Engine` 所有权之前提取资源句柄、
  总 KV 统计、导入统计原子量、键范围和 Region 切分键，随后将引擎放入 `Mutex`。
- `ResourceHandle`：返回 `Arc<EngineResource>` 克隆，供框架设置 worker pool 或更新资源。
- `GetTotalLoadedKVsCount`、`RecordedDuplicateSize`：加锁读取底层加载结果；前者用于上游
  校验已加载条数，后者暴露 Record 策略下已记录重复数据的字节数。
- `CloseShared`：为仍以共享引用持有适配器的调用场景提供关闭入口；锁中毒转换成
  `Error::Poisoned`，底层关闭错误原样返回。
- `impl api::Engine for ExternalEngineAdapter`：公共引擎契约实现，其中主入口是
  `LoadIngestData`；`ID` 恒为 `"external"`。
- `DataAdapter(MemoryIngestData)`：私有新类型适配器。它保留底层共享批次的 clone 语义，
  并把“空 `Vec<u8>` 表示不存在”转换成公共接口的 `Option<Vec<u8>>`。
- `IterAdapter`：持有 `Option<MemoryDataIter>`、延迟错误 `Option<api::EngineError>` 和
  克隆的 `api::Context`。`check` 是移动操作前的统一取消/错误门禁。

## 执行流程

生产主链如下：

1. `GlobalSortWriteIngestBackend::CloseExternalEngine` 根据请求构造 globalsort `Engine`，
   再调用 `ExternalEngineAdapter::new`，以 `Arc` 保存到 subtask ID 对应的 map。
2. `GlobalSortWriteIngestBackend::ImportEngine` 取出该 `Arc`，连同 region-job generator 和
   worker factory 传给 `local::import_pipeline::do_import`。通用流水线只依赖
   `api::Engine`/`api::IngestData` 契约。
3. 流水线调用 `ExternalEngineAdapter::LoadIngestData`。方法在 scoped thread 中启动一个
   监控线程；它每 1 ms 检查公共 `api::Context`，取消后调用底层 token 的 `cancel()`。
4. 当前线程锁住底层 `Engine`，调用 `Engine::LoadIngestDataWith`。底层按 worker 并发度
   加载 job-key 范围、处理重复键，并通过回调一次交出一个内部批次。
5. 回调把 `MemoryIngestData` clone 包装为 `DataAdapter`，逐个把内部 `KeyRange` 转成
   公共 `api::Range`，然后用 `try_send` 投递：
   - 成功即允许底层继续下一批；
   - 通道满则保留原值，短暂休眠后重试；
   - 公共 context 或底层 token 已取消，则释放尚未发送的批次并返回 `Cancelled`；
   - 接收端断开，则释放批次并返回 `Closed`。
6. 加载结束后设置 `done`，等待 scoped 监控线程退出，再把本 crate 的 `Error` 装箱成
   `api::EngineError` 返回。`MonitorStop` 也保证提前退出路径能够停止监控。
7. 下游为每个 `SortedRanges` 生成 region job，调用 `DataAdapter::IncRef`/`DecRef` 管理共享
   批次，并通过 `NewIter` 取得 `IterAdapter` 扫描对应半开区间。成功 ingest 后调用
   `Finish` 累加字节数和条数。
8. `ImportEngine` 在导入完成后比较 `do_import` 返回统计、`ImportedStatistics` 和
   `GetTotalLoadedKVsCount`；清理阶段要求 `Arc::try_unwrap` 成功后调用 trait `Close`。

迭代器分支中，`DataAdapter::NewIter` 不能通过返回值直接报告构造错误，因此会把底层
`NewIter` 的错误存入 `IterAdapter.error`。之后 `First`/`Next` 返回 `false`，调用方从
`Error()` 取出原因。构造成功时，`First` 定位首项，`Next` 单向推进，`Valid` 同时检查
取消、延迟错误和底层位置有效性。

## 数据与状态

- `size`、`count`、`range`、`splits` 是创建适配器时的不可变快照。globalsort `Engine`
  创建后这些属性本就表示一次导入的固定元数据；适配器返回 clone，避免暴露内部容器。
- `imported_size`、`imported_count` 与每个 `MemoryIngestData` 共享同一组 `AtomicI64`。
  `DataAdapter::Finish` 最终调用底层 `MemoryIngestData::Finish` 做 Relaxed 累加，因此
  `ImportedStatistics` 可在不取得引擎锁的情况下看到累计值。
- `MemoryIngestData` 的 clone 共享 `Arc<MemoryIngestDataInner>`，不是复制 KV payload。
  `DataAdapter(batch.data.clone())` 因而只建立新的所有权句柄。
- `GetFirstAndLastKey(lower, upper)` 使用半开区间 `[lower, upper)`；空 bound 表示无界。
  底层无匹配项时返回两个空 vector，适配层将其转换为 `(None, None)`，与 Go 的
  `nil, nil, nil` 约定一致。
- `IterAdapter` 自身拥有底层迭代器。底层 `MemoryDataIter` 持有 KV vector 的 `Arc`，
  关闭时换成空 vector；`ReleaseBuf` 对当前纯内存实现是无操作。
- `ConflictInfo` 将内部小写字段 `count/files` 拷贝到公共结构；只有记录过重复键时底层
  才返回重复文件路径。

重要不变量包括：发送失败时未交付批次必须被释放；每次 `IncRef` 必须有对应
`DecRef`；`Key`/`Value` 只能在有效位置且迭代器未关闭时调用；引擎可变加载和关闭操作
由同一把 `Mutex` 串行化。

## 依赖与调用关系

直接依赖如下：

- `super::*` 来自 [`engine.rs`](engine.rs) 及 globalsort crate 上下文，提供 `Engine`、
  `EngineResource`、`MemoryIngestData`、`MemoryDataIter`、`CancellationToken`、`Error`、
  原子类型和同步类型。
- `astersql_ingestor_engineapi as api` 提供公共 `Engine`、`IngestData`、`ForwardIter`、
  `Context`、`DataAndRanges`、`Range`、`ConflictInfo` 和开放错误类型。
- `astersql_lightning_membuf::Pool` 仅为满足公共 `NewIter` 签名而传入；当前
  `MemoryIngestData` 迭代器共享已有 KV，不从该 pool 分配。
- 标准库 `SyncSender`/`TrySendError` 实现有界批次传递，`Duration` 实现 1 ms 轮询退让。

[`Cargo.toml`](Cargo.toml) 将本目录定义为 `astersql-ingestor-globalsort` crate，并以路径
依赖引入 `astersql-ingestor-engineapi`、`astersql-lightning-membuf`、
`astersql-ingestor-simplesst`、ingestor 错误定义和 worker pool。manifest 的
`package.metadata.porting.go-package` 指向 `pkg/ingestor/globalsort`，确认其 Go 对照边界。

RustCodeGraph 将本文件标记为被 8 个文件使用；精确 impl 方法的静态 callers/callees
查询未产生边。源码引用核验到：`engine.rs` 声明并重新导出适配器，
`write_ingest_backend.rs` 负责生产创建和消费，`engine_api_test.rs` 直接验证公共契约；
其余下游通过 trait object 或通用流水线动态分发，不能仅从静态调用图恢复。

## 错误处理与边界

- 引擎锁中毒：`LoadIngestData`、`Close` 和 `CloseShared` 映射为 `Error::Poisoned`；
  `GetTotalLoadedKVsCount`、`RecordedDuplicateSize` 和 `ConflictInfo` 使用 `unwrap`，若此前
  持锁线程 panic，则这些只读辅助入口会继续 panic。扩展时应明确是否保持这种差异。
- 取消：公共 context 取消由监控线程传播到底层 token；发送循环同时检查两种取消源，
  所以即便通道永久满也能退出。迭代器取消只设置本地 `Error::Cancelled`，不会反向取消
  整个引擎。
- 通道关闭：`TrySendError::Disconnected` 转换为 `Error::Closed`。错误返回前显式调用
  `batch.data.release()`，防止内存预算一直被在途批次占用。
- 底层加载、读取、重复键处理或 consumer 回调错误统一保留为 globalsort `Error`，在
  trait 边界装箱为 `api::EngineError`；调用者可 downcast 恢复具体错误。
- `monitor.join().unwrap()` 假设监控闭包不会 panic；若未来在监控线程加入可失败逻辑，
  这里的 panic 行为需要重新设计。
- `Key`/`Value` 对已关闭或不存在的迭代器使用 `expect`，并依赖调用者先检查
  `First`/`Valid`。这是接口使用前置条件，而非可恢复错误路径。
- `Close(&mut self)` 可直接通过 `Mutex::get_mut` 访问底层引擎，但仍会报告 poison；
  `CloseShared(&self)` 则取得运行时锁。生产 cleanup 使用前者，并要求没有其他 `Arc`。

## 并发与资源生命周期

`ExternalEngineAdapter` 必须是 `Send + Sync` 才能实现公共 `api::Engine`。可变的底层
`Engine` 放在 `Mutex` 中，因此同一适配器不会并发执行两次加载或在加载中关闭；资源句柄
和原子统计则可在加载持锁期间独立访问。

`LoadIngestData` 使用 `std::thread::scope`，监控线程借用 `self`、`context` 和局部
`done`，作用域结束前强制 join，不会产生脱离调用栈的后台线程。`done` 的 Release/Acquire
配对仅负责停止信号；真正的业务取消由共享 token/context 各自的同步原语保证。

有界 `SyncSender` 提供背压。实现用 `try_send + 1 ms sleep` 而不是阻塞 `send`，是为了在
没有通道容量时仍周期性观察取消。发送成功后批次所有权进入通道；发送失败返回的
`outgoing` 被继续重试，避免丢失或重复包装。

底层 `MemoryIngestDataInner` 在引用归零时清空 KV 并执行一次性 release callback；若批次
从未被 region job `IncRef`，最后一个 Rust owner drop 也会走等价清理。本文件在取消或
断连时额外显式 `release`，使加载器无需等待 wrapper drop 才归还内存配额。独立测试验证
了这两种未消费路径的 in-flight 计数/字节均归零。

## 与 Go 版本的对应关系

Go 中 [`engine.go`](engine.go) 的 `*Engine` 直接实现
[`pkg/ingestor/engineapi/engine.go`](../engineapi/engine.go) 的 `Engine` interface，
`*MemoryIngestData` 和 `*memoryDataIter` 则直接实现
[`pkg/ingestor/engineapi/ingest_data.go`](../engineapi/ingest_data.go) 的接口。Rust 因为
globalsort 内部类型和公共 trait 分属不同 crate/所有权模型，增加了本文件这层显式包装。

主要语义对应关系：

- Go `Engine.LoadIngestData(ctx, chan<- DataAndRanges)` 对应适配器的同名 trait 方法；Rust
  底层 `Engine::LoadIngestDataWith` 以 consumer 回调流式交付批次，再由适配器发送到
  `SyncSender`。
- Go 的 context 可直接传入加载和通道 select；Rust 公共 `Context` 只暴露取消标志，
  因而由 scoped 监控线程桥接到底层 `CancellationToken`，并在满通道时轮询。
- Go 的 `nil` 首尾键对应 Rust `Option::None`；globalsort 内部仍使用空 vector 表达，
  `DataAdapter` 在 trait 边界转换表示法。
- Go interface error 对应 Rust `Box<dyn Error + Send + Sync>`；`DataAdapter::NewIter`
  不能直接返回 `Result`，所以与 Go iterator 的 `Error()` 模式一致地延迟保存错误。
- Go 的原子 imported 统计、引用计数、`Finish` 累加和最后引用释放语义均被保留；Rust
  另外用 `Drop` 兜底未被引用的批次，以适配所有权析构路径。
- Go 的 `GetRegionSplitKeys` 显式 clone 每个键；Rust 在 `ExternalEngineAdapter::new` 时取得
  owned `Vec<Vec<u8>>`，每次公共查询再 clone 整体，调用者同样不能修改内部状态。
- Go 的 `memoryDataIter.Close`/`ReleaseBuf` 都是空操作；Rust `MemoryDataIter::Close` 会
  丢弃其 `Arc` 和位置状态，`ReleaseBuf` 仍为空操作，关闭后的访问会 panic。

不能把适配层理解为新的业务实现：排序、重复键策略和批次划分仍应与同路径 Go
`Engine` 以及 Rust [`engine.rs`](engine.rs) 对照；本文件只处理公共接口差异和生命周期
接线。

## 扩展指南

- 扩展公共 `api::Engine` trait 时，应在 `impl api::Engine for ExternalEngineAdapter` 增加
  对应映射，并判断数据是构造时快照、共享原子状态还是必须锁住底层引擎；不要无条件把
  新查询放进长时间持有的加载锁。
- 修改批次发送逻辑时，必须保留三项性质：有界背压、满通道时可取消、未成功交付的
  `MemoryIngestData` 会释放。应在独立的 [`engine_api_test.rs`](engine_api_test.rs) 增加
  回归，而不是把测试写进本文件。
- 增加新的 `IngestData` 能力时，应同步检查 `DataAdapter`、底层 `MemoryIngestData`、
  公共 engineapi trait 及 Go `IngestData`。涉及引用或缓冲区的改动尤其要覆盖多 region
  共享、零引用丢弃、部分成功后 `Finish` 和取消竞态。
- 修改 `IterAdapter` 时，应保持错误通过 `Error()` 可观察，且 `First`、`Next`、
  `Valid` 对取消的行为一致。若底层迭代器未来真正使用 `membuf::Pool`，还需实现并测试
  `ReleaseBuf` 后旧 key/value 失效、后续读取可继续的契约。
- 若需要更低延迟或避免 1 ms 轮询，可将公共 context/channel 设计为可等待的取消选择；
  这会跨越 `astersql-ingestor-engineapi` 和导入流水线边界，应做独立设计与并发测试，不能
  只替换本文件的 sleep。
- 修改关闭方式时需同时检查 `CleanupEngine` 的 `Arc::try_unwrap` 约束以及仍需共享关闭的
  `CloseShared` 调用者。重复关闭、加载与关闭竞争、锁中毒行为都应明确测试。
- 兼容风险主要是 Go/Rust 接口语义漂移；性能风险主要是持有 `engine` 锁的整个加载周期、
  满通道轮询和不必要的 key/range clone；正确性风险集中在取消时批次释放与引用计数配对。

## 验证依据

本说明基于以下直接证据核对：

- 目标源码 [`engine_api.rs`](engine_api.rs)：`MonitorStop`、`ExternalEngineAdapter`、
  `api::Engine` impl、`DataAdapter`、`IterAdapter` 及条件编译的独立测试模块。
- 内部实现 [`engine.rs`](engine.rs)：`LoadIngestDataWith` 的流式 consumer、
  `MemoryIngestData` 的范围/引用/释放/统计、`MemoryDataIter` 和 `EngineResource`。
- 公共契约 [`pkg/ingestor/engineapi/engine.rs`](../engineapi/engine.rs) 与
  [`pkg/ingestor/engineapi/ingest_data.rs`](../engineapi/ingest_data.rs)。
- 生产上游 [`pkg/dxf/importinto/write_ingest_backend.rs`](../../dxf/importinto/write_ingest_backend.rs)：
  适配器创建、`do_import` 调用、统计一致性检查、冲突信息和 cleanup。
- crate 声明 [`Cargo.toml`](Cargo.toml)：crate 名称、Go package 迁移元数据和直接路径依赖。
- Go 对照：[`engine.go`](engine.go)、
  [`pkg/ingestor/engineapi/engine.go`](../engineapi/engine.go) 和
  [`pkg/ingestor/engineapi/ingest_data.go`](../engineapi/ingest_data.go)。
- 独立 Rust 测试 [`engine_api_test.rs`](engine_api_test.rs)：验证有界批次按范围消费、
  时间戳和导入统计共享；零容量满通道取消后未发送数据释放；接收端丢弃未引用批次后
  in-flight count/bytes 归零。底层范围、批次和重复键行为另由
  [`engine_test.rs`](engine_test.rs) 覆盖，并与 [`engine_test.go`](engine_test.go) 对照。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；目标文件识别出 39 个符号并
  标记为被 8 个文件使用。`query` 定位到 `engine_api.rs::ExternalEngineAdapter`、
  `engine_api.rs::LoadIngestData`、`DataAdapter` 和 `IterAdapter`；精确
  `callers/callees` 未返回动态 trait/impl 调用边，因此用上述源码引用补充验证。

本任务是纯文档分析，未运行 Cargo 或代码测试。结构检查应确认文档存在且固定的十一个
二级章节各出现一次；人工复核重点是所有“已支持”结论均能回溯到上述符号、调用点或测试。
