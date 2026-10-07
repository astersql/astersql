# `pkg/dxf/importinto/conflictedkv/collector.rs`

## 文件定位

本文件属于 `astersql-dxf-importinto-conflictedkv` crate（见同目录 `Cargo.toml`），实现 IMPORT INTO 冲突解决流程中 **collect-conflicts** 阶段的单 worker 收集器。它位于冲突 KV 的读取/分发与后续元数据汇总之间：直接上游 `pkg/dxf/importinto/collect_conflicts.rs::CollectConflictGroup` 为每个 worker 创建一个 `ConflictCollector`，调用 `Run` 消费该 worker 的冲突 KV 通道，再调用 `Close`、`MergeRowKeysInto` 和 `GetCollectResult`；多个 worker 的结果随后合并为子任务级 `CollectResult`。

crate 入口 `lib.rs` 通过 `mod collector; pub use collector::*;` 导出本文件的公开符号。该 crate 的职责由 `doc.rs` 界定：data KV 和唯一索引 KV 的冲突行会在 collect-conflicts 阶段被还原、重编码、计算 checksum，并以可供用户检查的文本文件写入对象存储。本文件不负责读取 SST、调度 worker、删除冲突 KV 或最终 checksum 比对。

## 核心职责

1. `NewCollector` 根据 `kv_group` 选择 `DataKVHandler` 或 `IndexKVHandler`，并组装对象存储、集群快照、行编解码器、进度收集器、行键去重集合和共享文件大小计数器。
2. `Run` 将输入 `mpsc::Receiver<ConflictKVPair>` 交给所选 Handler。Handler 负责把冲突 KV 还原为逻辑行并重编码；本收集器作为 `EncodedRowHandler` 接收处理完成的行。
3. `HandleEncodedRow` 把 Datum 行序列化为文本、按上限写入分片文件，同时累计成功处理的行数和重编码 KV 的 `KVChecksum`。
4. `CollectResult::Merge` 合并多个 worker/组的行数、实际文件字节数、截断状态、checksum 和文件列表。
5. `MergeRowKeysInto` 将本 worker 在索引路径成功处理的行键合并到外层集合，使同一行因多个唯一索引冲突时可以去重。

关键不变量是“文件记录可被截断，但逻辑统计继续”：超过总文件大小上限后，`stop_recording` 阻止后续对象存储写入，而后续成功回调仍会增加 `RowCount` 和 `Checksum`。这使 checksum 计算不依赖用户可检查文件是否完整；完整性由 `RowRecordingCapped` 单独表达。

## 主要符号

- `MaxConflictRowFileSize: AtomicI64`：单个冲突行文件的轮转阈值，默认 8 GiB。它在写入一行之前检查当前文件已有大小，因此允许最后一行使文件略超阈值，下一行才触发轮转。
- `maxTotalConflictRowFileSize: AtomicI64`：所有共享同一计数器的 collector 的总记录阈值，默认 1 GiB；仅通过测试辅助函数调整。
- `minUploadPartSize`：对象存储 writer 的 part 大小，5 MiB；`switchFile` 同时固定上传并发为 20。
- `CollectResult`：公开汇总结构，包含 `RowCount`、`TotalFileSize`、`RowRecordingCapped`、`Checksum`、`Filenames`。`NewCollectResult` 用集群 keyspace 初始化 checksum；`Merge` 对可选结果做累加、布尔或、checksum 相加和文件名追加。
- `ConflictCollector`：保存对象存储、文件前缀、可暂时取出的 Handler、当前结果、本地 `BoundedKeySet`、跨 worker 的原子文件大小计数、文件序号、当前文件大小、可选 writer 和停止记录标记。
- `NewCollector(...) -> ConflictCollector`：公开构造入口。`kv_group == DataKVGroup` 时创建 `DataKVHandler`；否则创建带 `LazyRefreshedSnapshot` 和 `KeyFilter(global_set, local_set)` 的 `IndexKVHandler`。未传共享文件计数器时会分配一个从零开始的独立计数器。
- `Run`：先从 `Option` 中取出 Handler，执行 `PreRun` 和 `Run`，最后无论结果成功与否都将 Handler 放回。若 Handler 已被取出，返回 `collector handler is already running`。
- `recordRowToFile`、`switchFile`、`onTotalSizeLimitExceeded`：内部文件写入状态机，分别处理序列化/计数、文件轮转和总上限截断。
- `Close`：调用 Handler 的收尾逻辑（索引 Handler 会冲刷剩余批次并关闭 codec），再关闭尚存的 writer。
- `HandleEncodedRow`：`EncodedRowHandler` 实现；文件处理成功后才更新行数和 checksum。
- `getRowFileName`：生成 `data-NNNN.txt`，并手工复现 Go `path.Join` 的 POSIX 路径清理语义。
- `MergeRowKeysInto`：把受 Mutex 保护的本地行键集合合并到调用方集合。
- `SetMaxTotalConflictRowFileSizeForTest`、`RowKeySetLenForTest`：仅在 `cfg(test)` 下存在的验证接口。

## 执行流程

1. `CollectConflictGroup` 为一个 KV group 建立若干有界同步通道和 worker。每个 worker 配置 codec 的 keyspace，创建本地 `BoundedKeySet`，以唯一 UUID 子目录作为文件前缀调用 `NewCollector`。
2. `NewCollector` 创建 `BaseHandler`。data 组直接走 `DataKVHandler`；索引组额外连接集群快照与全局/本地行键过滤器。
3. `Run` 先调用 Handler 的 `PreRun`。data 路径直接逐条解码 row key 与 row value；索引路径先解析索引 ID、解码 handle、按完整 row key 过滤，再缓冲并用快照 `BatchGet` 回查 data row。两条路径最终都重编码整行，并回调本文件的 `HandleEncodedRow`。
4. `HandleEncodedRow` 调用 `recordRowToFile`。若尚未截断，它先用 `DatumsToString(row, true)` 生成文本，令本次字节数为文本长度加换行符，再对共享计数器执行 `fetch_add`。
5. 若原子累加后的总量大于上限，`onTotalSizeLimitExceeded` 立即设置 `stop_recording` 和 `RowRecordingCapped`，取出并关闭现有 writer；本次越界行不写文件。若未越界且 writer 不存在或当前文件已达单文件阈值，`switchFile` 先关闭旧 writer，再创建下一个 `data-NNNN.txt`。
6. 写入成功后才累计 `result.TotalFileSize` 和 `current_file_size`。随后 `HandleEncodedRow` 增加 `RowCount`，并用重编码产生的 KV pairs 更新 checksum。
7. 通道断开后 `Run` 返回。上游调用 `Close`，使索引 Handler 冲刷最后一批 handle 并关闭 codec，同时关闭最后一个文件 writer。索引 Handler 只在行成功回调后把 row key 加入本地过滤集合。
8. worker 调用 `MergeRowKeysInto` 取回本地成功行键，再克隆 `GetCollectResult`。`CollectConflictGroup` 归并全部 worker 的结果和行键；更外层 `RunGroups` 再按 KV group 串行合并，以避免同一行在多个唯一索引组中重复计 checksum。

## 数据与状态

`CollectResult.RowCount` 表示成功走完文件处理（包括已经处于 stop-recording 的无写入路径）并更新 checksum 的逻辑冲突行数；`TotalFileSize` 只统计实际成功写入文件的“文本字节 + 换行”总量。两者在截断后可以有意不对应。`Filenames` 在 writer 成功创建后立即追加，因此记录的是已经打开过的输出对象。

`shared_total_file_size` 是 `Arc<AtomicI64>`，由同一 collect-conflicts 子任务的 worker 共享。它统计每次尝试记录的行大小，包含触发上限的越界行以及截断前其他 worker 已预记账的行，因而可能大于 `CollectResult.TotalFileSize`，也可能略高于全局阈值。这是与 Go 一致的“先记账、后判断”语义。

`handle_set` 是 `Arc<Mutex<BoundedKeySet>>`，只作为索引 Handler 的本地过滤集合和 worker 完成后的移交状态。直接调用 `ConflictCollector::HandleEncodedRow` 不会登记 row key；登记发生在 `IndexKVHandler::handleBufferedHandles` 中，并且只有 `encodeAndHandleRow` 成功后才执行 `filter.addLocal`。

`handler: Option<Box<dyn Handler>>` 用所有权暂移解决 `handler.Run(..., self)` 同时需要可变访问 Handler 和 collector 的借用冲突。`writer: Option<Box<dyn Writer>>` 同样用 `take` 确保即使 `Close` 返回错误也不会重复关闭同一个 writer。

## 依赖与调用关系

直接上游调用边可由 RustCodeGraph 的文件关系和 `CollectConflictGroup` 源码确认：

- `pkg/dxf/importinto/collect_conflicts.rs::CollectConflictGroup -> NewCollector`
- `CollectConflictGroup worker -> ConflictCollector::Run -> Handler::PreRun/Run`
- `CollectConflictGroup worker -> ConflictCollector::Close`
- `CollectConflictGroup worker -> MergeRowKeysInto/GetCollectResult`

关键下游关系是：

- `NewCollector -> NewBaseHandler -> NewDataKVHandler`，或 `NewIndexKVHandler -> NewLazyRefreshedSnapshot/NewKeyFilter`。
- `DataKVHandler/IndexKVHandler -> BaseHandler::encodeAndHandleRow -> EncodedRowHandler::HandleEncodedRow`。
- `recordRowToFile -> DatumsToString` 完成可读文本序列化；`switchFile -> Storage::Create -> Writer::Write/Close` 管理对象存储文件。
- `HandleEncodedRow -> KVChecksum::Update`，`CollectResult::Merge -> KVChecksum::Add`。

`Cargo.toml` 显示本文件的直接类型边界分别来自本地 crate：`astersql-kv`、`astersql-lightning-backend-kv`、`astersql-lightning-verification`、`astersql-meta-model`、`astersql-objstore-objectio`、`astersql-objstore-storeapi`、`astersql-types` 和 DXF task executor。该 manifest 没有为 collector 声明条件 feature；条件编译只用于本文件的测试辅助 API。

## 错误处理与边界

所有运行时失败以 `Result<(), String>` 向上返回，对 codec、Handler、对象存储创建/写入/关闭错误使用 `to_string` 保留底层文本。主要边界如下：

- `Run` 的 Handler 不可重入；Handler 已被取出时立即报错。Handler 在 `PreRun` 或 `Run` 失败后仍会放回，允许上游继续调用 `Close` 做资源收尾。
- `DatumsToString`、创建 writer 或写入失败时，本行不会增加 `RowCount`、checksum 或成功写入大小；共享总计数在序列化之后、真实写入之前增加，因此后续写失败不会回滚预记账。
- 文件轮转时先从 `Option` 取出旧 writer；即使旧 writer 关闭失败，它也不会被二次关闭，触发轮转的行不计入结果。
- 总上限路径先设置 capped/stop 状态再关闭 writer；关闭失败会向上传播，但截断状态保留、writer 已清空，后续直接回调可继续统计而不会再次写文件或重复关闭。
- `Close` 先获得 Handler 结果，再获得 writer 结果，最终使用 `handler_result.and(writer_result)`。因此 writer 仍会尝试关闭，但若 Handler 和 writer 都失败，返回的是 Handler 错误。
- `getRowFileName` 清理空段、`.` 和可消解的 `..`，绝对路径不会越过根；调用方仍须提供符合对象存储命名约束的前缀。
- 总大小判断是 `total > limit`，等于上限仍允许写入；单文件判断发生在写下一行之前，故不是严格的单文件硬截断。

## 并发与资源生命周期

一个 `ConflictCollector` 由一个 worker 可变独占，不在多个线程间并发调用；并发发生在上游创建的多个 collector 之间。跨 worker 共享的文件大小通过 `AtomicI64` 管理：累加使用 `AcqRel`，读取上限使用 `Acquire`，测试覆盖值使用 `Release`。这提供全局截断判定所需的原子顺序，但并不承诺把所有 worker 精确停在阈值处。

索引行键集合由 `Arc<Mutex<BoundedKeySet>>` 保护。`NewKeyFilter` 与 collector 共享同一个本地集合，Handler 成功处理时写入；worker 结束后 `MergeRowKeysInto` 在持锁期间把它复制/合并到外层集合。全局集合通过 `Arc<BoundedKeySet>` 只用于跨组过滤。

writer 生命周期是“首次成功行时惰性创建—达到阈值时轮转—截断或 `Close` 时关闭”。`switchFile` 和截断路径都先 `take` 再关闭，避免关闭失败留下可重复使用的资源句柄。Handler 生命周期覆盖整个输入通道；`Close` 对索引 Handler 尤其必要，因为它会处理尚未达到批量阈值的剩余 handle，并关闭 codec。上游 `CollectConflictGroup` 明确按 `Run` 后 `Close` 的顺序执行，并在任一失败时取消/汇总 worker 错误。

## 与 Go 版本的对应关系

Go 对照实现是同目录 `collector.go`，测试是 `collector_test.go`。Rust 保留了以下核心语义：按 data/index 组选择 Handler；Datum 文本每行追加换行；单文件达到阈值后在下一行前轮转；writer 选项为并发 20、part 5 MiB；跨 collector 共享总大小；先原子加再比较总上限；截断后仍统计行数与 checksum；`CollectResult.Merge` 累加所有字段；文件名等价于 `path.Join(prefix, data-NNNN.txt)`。

Rust 的依赖以 trait/`Arc` 注入代替 Go 的具体 storage、table、encoder 和 logger，并使用 `String` 错误而非 Go error wrapping。Rust 还显式保存本地 `handle_set` 并提供 `MergeRowKeysInto`，以适配 `CollectConflictGroup` 的 scoped worker 返回值；Go 版本由外层持有传入的 `localSet`，无需该移交方法。Rust 的 `Run` 暂时取出 Handler 是所有权实现细节，行为上仍对应 Go 的 `PreRun` 后 `Run`。

Rust 测试额外固定了几项容易漂移的兼容语义：`test_get_row_file_name_cleans_prefix_like_go_path_join` 验证 POSIX 清理；两个 close-failure 测试验证 writer 即使关闭失败也会被清空；`test_collector_direct_callback_leaves_index_filter_to_handler` 验证过滤登记属于 Index Handler 而非 collector 回调。Go 与 Rust 的共享上限测试都证明后写 collector 会被截断，而逻辑行数仍完整。

## 扩展指南

- 若修改结果字段或合并规则，应同步 `CollectResult`、`NewCollectResult`、`Merge`、外层 `applyCollectResult` 及独立测试 `collector_test.rs::test_collect_result_merge`，同时核对 Go 的 `CollectResult` 与 `TestCollectResultMerge`。
- 若调整文件格式、命名或轮转策略，应修改 `recordRowToFile`、`switchFile` 或 `getRowFileName`，并扩展现有文件内容/数量/路径测试；注意 `TotalFileSize`、共享预记账和对象存储实际字节数三者的定义不能混淆。
- 若调整总大小策略，应保留多 worker 原子性，并明确决定是否继续采用“越界行已记账、但不写出”的 Go 兼容行为；至少同步单 collector、共享 collector、总上限关闭失败三个 Rust 回归测试。
- 若新增 Handler 类型或 KV group 分类，接入点是 `NewCollector` 的 Handler 选择分支；同时核对 `collect_conflicts.rs::getKVGroupIndexInfo`、worker 分发规则和 Handler 的独立测试，避免把非 data 组一概视为普通唯一索引。
- 若改变索引去重，应优先修改 `handler.rs::IndexKVHandler`/`KeyFilter`，而不是在 `HandleEncodedRow` 中登记 row key。必须验证“回调失败不登记”“多唯一索引组不重复 checksum”“BoundedKeySet 超限”的行为。
- 若增加异步或更高并发，不可直接共享同一个 `ConflictCollector`；应继续保持每 worker 独占 collector，仅共享经过同步封装的计数器/集合，并验证 writer 关闭、取消和错误汇总顺序。
- Rust 单元测试继续放在独立的 `collector_test.rs`，不要内嵌到生产源文件；Go 对照行为变化时同步检查 `collector_test.go`。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件、307,296 个节点；`files --filter pkg/dxf/importinto/conflictedkv` 确认本 crate 的 Rust/Go 源文件和独立测试均已索引。
- RustCodeGraph `node --file pkg/dxf/importinto/conflictedkv/collector.rs`：核对本文件 313 行的全部常量、类型、函数、trait impl 与条件编译项；图同时报告本文件由 `pkg/dxf/importinto/collect_conflicts.rs` 使用。
- RustCodeGraph `query`：核对 `ConflictCollector`、本文件的 `NewCollector`、`HandleEncodedRow`、`MergeRowKeysInto` 和 `getRowFileName` 的符号位置。泛化的 `explore/callers/callees` 查询存在同名噪声，因此调用边最终以精确文件节点和直接源码为准，没有据此推断额外调用者。
- RustCodeGraph 文件节点：读取 `pkg/dxf/importinto/collect_conflicts.rs::CollectConflictGroup`，确认构造、`Run -> Close -> MergeRowKeysInto -> GetCollectResult`、worker join 与结果合并；读取 `handler.rs`，确认 data/index 解码、快照回查、成功后登记行键和 Close 冲刷行为。
- crate/模块证据：`pkg/dxf/importinto/conflictedkv/Cargo.toml`、`lib.rs`、`doc.rs`；Go 对照：`collector.go`；独立测试：`collector_test.rs`、`collector_test.go`。
- Rust 回归证据覆盖结果合并、Go 路径清理、data/index 文件切分、直接回调不登记过滤集合、总大小截断、两类关闭失败和跨 collector 共享上限。按任务约束这是纯文档分析，未运行 Cargo；交付结构检查要求目标文件存在且恰含上述 11 个固定二级标题。
