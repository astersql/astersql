# `pkg/dxf/importinto/conflictedkv/handler.rs`

## 文件定位

本文件是 `astersql-dxf-importinto-conflictedkv` crate 的冲突 KV 处理核心。crate 入口 `pkg/dxf/importinto/conflictedkv/lib.rs` 将 `handler` 声明为内部模块并整体再导出；`Cargo.toml` 的 `package.metadata.porting.go-package` 指向同路径 Go 包，说明它是 `pkg/dxf/importinto/conflictedkv/handler.go` 的 Rust 移植面。

在 IMPORT INTO 的冲突处理阶段，已记录的重复 KV 分为两类：data KV 可以直接解码出一行；唯一索引 KV 只能先得到行 handle，再从集群快照回查 data KV。该文件把这两条路径统一为 `Handler` 生命周期，并把重编码后的整行交给调用方提供的 `EncodedRowHandler`。上游生产构造点是 `collector.rs::NewCollector` 和 `deleter.rs::NewDeleter`：前者收集冲突行及校验信息，后者据重编码结果安排删除。

## 核心职责

1. 用 `ConflictContext`、`ConflictStore`、`ConflictSnapshot`、`ConflictRowCodec` 等 trait 隔离 KV 存储、快照、事务、keyspace 和行编解码实现。
2. 用 `BaseHandler::encodeAndHandleRow` 统一“行数据重编码 → 回调消费”的尾段，并按表是否有聚簇索引决定 `auto_row_id`。
3. `DataKVHandler` 直接去除 keyspace 前缀、解码行键和行值，然后重编码整行。
4. `IndexKVHandler` 从唯一索引 KV 解出物理表 ID 与 handle，按行键过滤、缓冲，并通过惰性快照批量回查实际行；这也覆盖分区表必须沿用索引键中物理表 ID 的要求。
5. `LazyRefreshedSnapshot` 至多复用快照 15 秒，在读取后按返回键和值的字节数记录集群读流量。
6. 在 `Handler::Run` 中消费 `mpsc::Receiver<ConflictKVPair>`、响应取消、记录每条输入 KV 的处理进度，并在 `Close` 中冲刷资源。

## 主要符号

- `snapshotRefreshInterval: Duration`：固定为 15 秒，是快照的最短刷新间隔；目的是避免快照版本长期落后 GC safe point，同时避免每批都取新版本。
- `DataKVGroup: &str`：值为 `"data"`。`collector.rs::NewCollector` 与 `deleter.rs::NewDeleter` 用它区分 data 与 index 处理器。
- `BufferedHandleLimit: AtomicUsize`：默认 256；`IndexKVHandler::HandleOne` 在缓冲达到 `load(Acquire).max(1)` 时刷新。它公开给独立测试调小批次，测试修改时用全局互斥串行化。
- `ConflictContext { KV, ObjectIO, cancelled }`：传递 KV/对象存储上下文。`Cancel` 以 Release 写共享原子标志并取消对象 IO；`IsCancelled` 以 Acquire 读取，同时接受 ObjectIO 已取消状态。
- `ConflictKVPair { Key, Value }`：通道中的单条冲突 KV，拥有键和值。
- `TrafficRecorder`：读写流量接口；本文件只在 `LazyRefreshedSnapshot::BatchGet` 调用 `IncClusterReadBytes`，写流量接口供同 crate 的删除路径使用。
- `ConflictSnapshot`：只读批量获取抽象，返回以原始键字节索引的 `ValueEntry`。
- `ConflictTransaction` 与 `ConflictStore`：存储边界。后者提供 keyspace、当前版本、快照、事务和可重试错误分类；默认的 `IsTxnRetryableError` 保留 `TxnRetryableMark` 字符串判定。这些能力由 `deleter.rs` 共用，handler 的快照路径直接使用 `CurrentVersion`/`GetSnapshot`。
- `ConflictRowCodec`：处理 keyspace 去除、行键/索引 handle/表 ID 解码、行解码和整行重编码，并拥有 `Close` 生命周期。`ConfigureKeyspace` 默认无操作，允许具体实现按存储 keyspace 配置。
- `Handler`：三阶段接口 `PreRun`、`Run`、`Close`。即使预处理或运行失败，上层也应调用 `Close`；Go 对照接口在注释中明确这一契约。
- `KVHandler`：单条 KV 入口，便于直接测试和让两类 handler 共享语义。
- `EncodedRowHandler`：重编码完成后的策略回调，接收行键、解码后的 `Datum` 列和生成的 `Pairs`；collector 与 deleter 以不同实现消费它们。
- `BaseHandler` / `NewBaseHandler`：保存目标 `TableInfo`、KV 组、codec 和进度 `Collector`。没有传 collector 时使用 `NoopCollector`。
- `DataKVHandler` / `NewDataKVHandler`：data KV 的 `Handler + KVHandler` 实现，`PreRun` 无额外准备。
- `HandleOfTable`：索引路径的内部缓冲项，保存生成的 data row key 和可复制的 handle。
- `IndexKVHandler` / `NewIndexKVHandler`：索引 KV 实现，保存目标索引、惰性快照、可选 `KeyFilter` 及待回查 handle。
- `LazyRefreshedSnapshot` / `NewLazyRefreshedSnapshot`：初始没有快照，第一次 `BatchGet` 才读取版本并构造快照。
- `stripKeyspacePrefix`：对 `ConflictRowCodec::StripKeyspacePrefix` 的公开便捷包装。

## 执行流程

### 公共生命周期

1. `collector.rs::NewCollector` 或 `deleter.rs::NewDeleter` 调用 `NewBaseHandler`。KV 组等于 `DataKVGroup` 时构造 `DataKVHandler`，否则构造 `IndexKVHandler`；索引路径同时构造 `LazyRefreshedSnapshot`，collector 还传入 `KeyFilter`。
2. 上层先调用 `PreRun`，再把冲突 KV 发送到通道并调用 `Run`，最后调用 `Close`。
3. 两类 `Run` 都阻塞在 `pairs.recv()`；通道断开表示输入正常结束。每取到一项先检查 `ConflictContext::IsCancelled`，成功处理后才调用 `collector.Processed(1, 0)`。
4. 任一解码、读取、重编码或回调错误立即通过 `Result<_, String>` 返回，当前输入不会计为已处理。

### data KV 路径

1. `DataKVHandler::Handle` 调用 `StripKeyspacePrefix`，得到事务访问所需的不带 keyspace 前缀的键。
2. `DecodeRowKey` 提取 handle，`DecodeRow` 把 KV value 还原为 `Vec<Datum>`。
3. `BaseHandler::encodeAndHandleRow` 检查 `TableInfo::HasClusteredIndex`：聚簇表传 `auto_row_id = 0`，非聚簇表传 `handle.IntValue()`，以便 codec 重建隐式行 ID 相关 KV。
4. `EncodeRow` 生成整行 `Pairs`，随后 `EncodedRowHandler::HandleEncodedRow` 决定收集还是删除。
5. `Close` 只关闭 codec。

### index KV 路径

1. `IndexKVHandler::PreRun` 把 `base.kv_group` 解析为索引 ID，在 `target_table.Indices` 中找到并克隆对应 `IndexInfo`；解析失败或索引不存在即失败。
2. `HandleOne` 去 keyspace 前缀并从该索引键解码物理 `table_id`。ID 为 0 被视为非法；随后用目标索引的列数调用 `DecodeIndexHandle`。
3. `EncodeRowKey(table_id, handle)` 生成 data row key。若可选过滤器的全局集合已含该键，直接跳过，不加入缓冲。
4. 未跳过的 `{ row_key, handle }` 加入 `buffered_handles`；达到 `BufferedHandleLimit` 后调用 `handleBufferedHandles`。
5. 批处理先生成 `row_keys`，并建立“原始行键字节 → handle 副本”的映射，然后调用 `LazyRefreshedSnapshot::BatchGet`。快照未返回的键代表集群中没有对应 data KV，因而不会进入后续处理。
6. 对每条返回行，先检查本 worker 的本地过滤集合以去除跨批次重复（例如多值索引的多个 index entry 指向同一行），再从映射取 handle、解码行、重编码并调用行处理回调。
7. 只有回调成功后才 `KeyFilter::addLocal`；全部返回行完成后清空缓冲。`Close` 会先冲刷未满一批的剩余 handle，再无条件尝试关闭 codec，并优先返回批处理错误。

### 惰性快照

1. `LazyRefreshedSnapshot::refreshAsNeeded` 在已有快照且距上次刷新不足 15 秒时复用它。
2. 否则先取 `ConflictStore::CurrentVersion`，再以该版本 `GetSnapshot`，成功后记录 `Instant::now()`。
3. `BatchGet` 确保刷新后调用底层快照；若配置了流量 recorder，则累加实际返回项的 `key.len() + value.Value.len()`，不是请求键总量。

## 数据与状态

- `BaseHandler` 独占可变 codec，因此解码/编码/关闭在一个 handler 的顺序调用中发生；目标表通过 `Arc<TableInfo>` 共享，进度 collector 通过 `Arc<dyn Collector>` 共享。
- `IndexKVHandler::target_index` 初始为 `None`，只能由成功的 `PreRun` 填充。绕过 `PreRun` 调用 `Handle` 会得到 `index handler was not prepared`。
- `buffered_handles` 是尚未完成快照回查的状态。成功批处理后才整体清空；中途读取、解码或回调失败时不会执行清空，因此后续 `Close` 或重试可再次处理该批。
- `key_to_handle` 以完整 data row key 字节为键，保留物理表 ID，避免分区表中相同逻辑 handle 混淆。相同 row key 的多个索引项在单批映射中合并，而跨批重复由 `KeyFilter` 的本地集合排除。
- `KeyFilter` 定义在 `row_handle.rs`：全局集合表示更早 KV 组已处理的行，本地 `Mutex<BoundedKeySet>` 表示当前 worker 已成功处理的行。集合达到共享内存上限后不再新增，因此去重是有界内存下的尽力行为。
- `LazyRefreshedSnapshot` 保存 `Option<Box<dyn ConflictSnapshot>>` 和上次刷新时刻；构造不访问集群。每个 handler 实例独立维护快照，不存在本文件级全局快照。
- `BufferedHandleLimit` 是 crate 级原子全局配置，影响所有同时运行的索引 handler；生产代码只读，测试修改后必须恢复。

## 依赖与调用关系

上游调用链（由 RustCodeGraph 确认）：

- `collector.rs::NewCollector` → `NewBaseHandler` → `NewDataKVHandler`，或 `NewIndexKVHandler(NewLazyRefreshedSnapshot, NewKeyFilter)`。collector 用它把冲突行记录到对象存储并累计后续校验所需信息。
- `deleter.rs::NewDeleter` → 同样的 data/index 分派；index handler 本身不带 filter，而 deleter 另持有惰性快照。它用重编码结果生成需要从集群删除的键。
- `lib.rs` 再导出本文件符号；crate 的更上层入口位于 `collect_conflicts.rs` 和 `conflict_resolution.rs`，分别构造 collector 与 deleter 完成两阶段冲突解决。

主要下游依赖：

- `astersql-kv`：`Key`、`Handle`、`Version`、`ValueEntry` 与事务重试标记。
- `astersql-meta-model`：`TableInfo` 和 `IndexInfo`，决定聚簇属性及索引列数。
- `astersql-lightning-backend-kv::Pairs` 与 `astersql-types::datum::Datum`：重编码结果与解码行表示。
- `astersql-dxf-framework-taskexecutor-execute::{Collector, NoopCollector}`：每条输入冲突 KV 的进度统计。
- `astersql-objstore-objectio::Context`：与 KV 上下文共同承载取消状态。
- crate 内 `row_handle.rs::KeyFilter`：索引回查路径的全局/本地行键去重。

`Cargo.toml` 表明该 crate 还依赖 importer、globalsort、simplesst、table/tablecodec 等实现 crate；本文件通过 `ConflictRowCodec`/`ConflictStore` 把这些具体实现隔在接口之后，便于 collector、deleter 和独立测试共享调度逻辑。

## 错误处理与边界

- 所有业务错误统一为 `String`，使用 `?` 原样传播；缺少结构化错误类型意味着调用者只能依赖文本或 store 提供的重试分类。
- 通道断开不是错误，`Run` 返回成功；取消只在每次收到 KV 后、处理前检查。阻塞等待空但未关闭的通道时，设置取消标志本身不会唤醒 `recv()`。
- data 路径会传播 keyspace 去除、行键解码、行解码、重编码和回调错误。
- index `PreRun` 拒绝非整数 KV 组或表上不存在的索引；`HandleOne` 还拒绝表 ID 0、未准备索引、索引 handle 解码失败。
- `BufferedHandleLimit.max(1)` 防止测试或配置把阈值设为 0 后出现无意义行为。
- `BatchGet` 可以少返回键；缺失行被正常跳过，符合唯一索引冲突对应 data KV 可能已在前一步被记录/移除的流程。若快照返回未请求的键，则以 `snapshot returned an unrequested row key` 拒绝，避免用错误 handle 解码。
- 若同一批处理在部分行成功后失败，缓冲仍保留，而已成功行只有配置 `KeyFilter` 时会被登记为本地已处理；无 filter 的 deleter 路径重试可能再次把已成功行交给回调，所以下游操作需要符合其重试语义。
- `Close` 先计算批处理结果，再调用 codec 的 `Close`，因此即使冲刷失败也会释放 codec；两者都失败时 `buffered_result.and(close_result)` 返回前者。
- `LazyRefreshedSnapshot` 只有在成功设置 `snapshot` 后才执行 `expect("snapshot initialized")`，正常路径不会触发 panic；store 版本或 BatchGet 错误均作为 `Err` 返回。
- 流量只统计实际读回的字节，错误结果不计；`TrafficRecorder` 自身没有返回值，属于尽力记账。

## 并发与资源生命周期

- `ConflictContext` 的取消标志是 `Arc<AtomicBool>`；克隆的上下文共享该标志。Release/Acquire 保证取消写入对检查方可见，`Cancel` 同时下推给对象 IO。
- `Handler::Run` 使用标准库同步 `mpsc::Receiver`，单个 handler 以可变借用顺序处理输入；本文件不创建线程。发送、关闭通道和调用生命周期方法由上层负责。
- `ConflictStore`、`TrafficRecorder` 要求 `Send + Sync`，事务要求 `Send`；快照 trait 本身没有并发约束，并被独占保存在 `LazyRefreshedSnapshot` 中。
- `BufferedHandleLimit` 以 Acquire 读取；测试以 Release/AcqRel 写入并用 `handler_test.rs::BUFFERED_HANDLE_LIMIT_LOCK` 防止多个测试并发修改。
- `KeyFilter` 的本地集合用 `Mutex` 串行访问，全局集合只读共享。锁仅覆盖一次 Contains/Add，不跨越快照 IO、解码或回调。
- codec 由 `BaseHandler` 独占，必须通过 `Handler::Close` 关闭。`IndexKVHandler::Close` 还负责处理最后一个未达阈值的批次；漏调会丢失这批逻辑工作并跳过 codec 清理。
- Rust 的 `Pairs` 在回调返回后按所有权自动释放；Go 对照版显式 `kvPairs.Clear()` 复用/归还内部存储。

## 与 Go 版本的对应关系

总体流程与 `handler.go` 保持一致：相同的 15 秒快照刷新间隔、默认 256 个 handle 的批次、data/index 分流、物理表 ID、聚簇与非聚簇 `autoRowID` 差异、跨批行键去重、关闭时冲刷、快照实际返回字节计量。

主要结构差异如下：

- Go 的 `BaseHandler` 通过嵌入 `KVHandler` 和 `EncodedRowHandler` 实现动态派发；Rust 把 data/index 各自实现完整 `Handler`，并把行回调作为 `Run`/`Close` 参数传入。
- Go 直接持有 `table.Table`、TiKV `Storage/Snapshot/Codec` 和 `TableKVEncoder`；Rust 将其拆为 `TableInfo` 与 `ConflictRowCodec`、`ConflictStore`、`ConflictSnapshot` trait，便于在当前 crate 边界移植与测试。
- Go 用 `globalsort.KVGroup2IndexID` 解析索引组；Rust 当前直接 `parse::<i64>()`，因此前提是 Rust 上游传入数值形式的 index KV group。`handler_test.rs` 使用 `IndexID2KVGroup` 产生该输入验证当前接线。
- Go 的 `BaseHandler::Run` 依赖 `context.Context` 传递取消；Rust 额外显式检查 `ConflictContext::IsCancelled`。但同步通道等待不会被取消标志直接唤醒。
- Go 的 `rowKeyFilter` 在索引路径直接调用；Rust 使用 `Option<KeyFilter>`，允许 deleter 不启用去重，collector 才启用全局/本地过滤。
- Go 通过 `errors.Trace` 保留错误栈并用 `common.OnceError` 取首个 Close 错误；Rust 使用字符串错误，`buffered_result.and(close_result)` 达到“仍关闭且优先冲刷错误”的结果顺序。
- Rust `ConflictStore` 同时容纳 deleter 所需事务接口，因此比本文件的只读快照需求更宽；这是 crate 内共享边界，不表示 handler 的 data/index 运行都会开启事务。

测试语义保持对齐：Go `handler_test.go::TestHandler` 使用真实 mock TiDB store/table 生成 KV；Rust `handler_test.rs` 用 fake codec/store 驱动真实调度逻辑。两边都验证 data KV 重复 10 次（聚簇 30 对、非聚簇 40 对）、index 仅处理 handle 2/4/5（聚簇 9 对、非聚簇 12 对）、读流量非零和进度计数。Go 还以真实表覆盖函数索引后的可见列重编码与多值索引跨批去重；Rust 的直接 handler 测试覆盖跨批重复输入及“回调成功后才能加入本地集合”，而完整可见列回归位于 `pkg/dxf/importinto/conflict_resolution_test.rs::data_handler_reencodes_visible_columns_after_functional_index`。

## 扩展指南

- 新增一种冲突 KV 组时，不要把分支硬塞进现有 `HandleOne`；应实现新的 `Handler`/`KVHandler`，并同步修改 `collector.rs::NewCollector`、`deleter.rs::NewDeleter` 的构造分派。若 KV 组编码改变，还要核对 Go 的 `globalsort.KVGroup2IndexID` 语义。
- 修改行编解码时优先扩展 `ConflictRowCodec` 的具体实现；必须维持 keyspace 去除发生在事务键解码前、分区物理表 ID来自索引键、非聚簇表传真实 handle 整数作为 `auto_row_id` 三个不变量。
- 修改批处理或去重策略时，重点维护：缺失快照行可跳过；未请求返回键必须拒绝；只有回调成功后才能 `addLocal`；`Close` 必须冲刷尾批且无论冲刷成功与否都关闭 codec。
- 若把同步通道改为异步/可取消等待，应同时明确通道关闭、取消和未刷缓冲的优先级，避免取消后静默丢弃已接收行。
- 若结构化错误或重试策略扩展，应让 `ConflictStore::IsRetryableError`/`IsTxnRetryableError` 与 deleter 的事务重试共同演进，不能只改本文件错误文本。
- 性能调优应关注 `BufferedHandleLimit`、handle/key 的复制、HashMap 容量和快照返回量；增大批次降低 RPC 次数但提高内存及失败重试范围。全局原子阈值会同时影响所有 handler，若需租户/任务隔离应改为实例配置。
- 测试必须放在独立文件 `handler_test.rs`，不要嵌入生产源。至少同步覆盖 data/index 两类表、聚簇/非聚簇、未知索引、零表 ID、快照缺失行、跨批重复、回调失败后重试、取消及 codec Close 错误；涉及真实表编解码时还应更新 `conflict_resolution_test.rs` 和 Go 对照测试。

## 验证依据

- RustCodeGraph 索引状态：项目索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/dxf/importinto/conflictedkv` 找到 Rust/Go 的 handler、collector、deleter、row_handle 及独立测试。
- RustCodeGraph `node --file pkg/dxf/importinto/conflictedkv/handler.rs`：核对本文件 506 行全部源码、公开 trait/构造函数、两类 handler、缓冲回查和快照刷新逻辑。
- RustCodeGraph `node NewDataKVHandler`、`node NewIndexKVHandler`、`node NewLazyRefreshedSnapshot`：Rust 调用边均指向 `collector.rs::NewCollector` 与 `deleter.rs::NewDeleter`；data handler 另由 `conflict_resolution_test.rs` 的函数索引回归直接构造。
- RustCodeGraph `node NewCollector`、`node NewDeleter`：核对 data/index KV 组的生产分派，以及 collector 配置 filter、deleter 不配置 filter 的差异。
- RustCodeGraph `node --file pkg/dxf/importinto/conflictedkv/row_handle.rs`：核对完整 row key、全局/本地集合、共享内存界限和 Mutex 生命周期。
- 已读 crate/模块边界：`pkg/dxf/importinto/conflictedkv/Cargo.toml`、`lib.rs`；已按仓库协议先读 `doc.go`，核对冲突收集、解决和校验在完整 IMPORT INTO 流程中的位置。
- 已读 Go 对照：`pkg/dxf/importinto/conflictedkv/handler.go`；已读测试：`handler_test.rs`、`handler_test.go`，并从 RustCodeGraph 调用边核对 `conflict_resolution_test.rs` 的可见列回归入口。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前使用任务指定命令验证本文恰有 11 个固定二级章节，并人工复核源码链接、符号名、错误边界、Go 差异和扩展建议均有上述直接证据。
