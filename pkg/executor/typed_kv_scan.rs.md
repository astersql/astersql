# [`pkg/executor/typed_kv_scan.rs`](typed_kv_scan.rs)

## 文件定位

本文件属于 `astersql-executor` crate；crate 入口 `pkg/executor/lib.rs` 以 `pub mod typed_kv_scan` 导出生产模块，并仅在 `cfg(test)` 下装配独立测试 `typed_kv_scan_test.rs`。它实现 typed executor 路径中的本地 KV 表扫描叶子：输入已经由规划/构建层编码好的半开键区间和一个真实 `kv::Retriever`，按需把表记录解码到 `chunk::Chunk`。直接构建入口是 `pkg/executor/builder.rs::BuildTypedTableScan`，更大的物理计划也通过 `BuildTypedPhysicalPlan*` 分发到该实现。

这里不是 Go `TableReaderExecutor` 的逐行翻译，也不负责 ranger 到 KV range、DAG protobuf、TiKV/TiFlash 请求或虚拟列计算。范围编码由调用者承担；`TypedKVScan` 只扫描给定的编码区间。因此它是 typed adapter 的本地快照执行边界，而非完整的分布式 TableReader 替代品。

## 核心职责

- `KeyRange` 表示 `[start, end)` 编码 KV 区间。`TypedKVScan::new` 按扫描方向稳定确定各区间的访问顺序，但不合并、裁剪或重新编码区间。
- `TypedKVScan` 延迟持有 `Arc<dyn kv::Retriever + Send + Sync>`；`new` 和 `Open` 都不读 KV，第一次 `Next`/`NextWithContext` 才创建迭代器。
- `next_inner` 在 chunk 容量允许时跨区间读取，通过 `cursor` 保存页间位置，保证正向和反向分页不重复当前键。
- `append_decoded_row_for_table` 校验记录键所属物理表，使用 `tablecodec` 解码列值，并在 `PKIsHandle` 模式下从记录 handle 补回未存入 value 的主键列。
- 实现 `adapter::ExecExecutor` 的生命周期、schema/chunk 构造、扫描计数、悲观锁键传递和 detach 能力；该扫描是只读执行器，不产生外键检查或级联任务。

## 主要符号

- `pub struct KeyRange { start: kv::Key, end: kv::Key }`：规划层交给执行层的编码半开区间。字段公开，区间合法性和互不重叠不在本文件内验证。
- `pub struct TypedKVScan`：扫描状态机。配置字段包括 retriever、`table_id`、`pk_is_handle`、方向、列元数据、schema、ranges 和 chunk 容量；运行字段包括 `range_index`、`cursor`、`page_keys`、`scanned_rows`、`opened`、`closed`。
- `TypedKVScan::new(...) -> Self`：构造并按方向排序 ranges；由 `ColumnInfo.FieldType` 生成 `SchemaColumn`。它不打开快照迭代器。
- `RebindRetriever`：crate 内部替换 retriever，当前由 `typed_point_get.rs::TypedPointGet::RebindRetriever` 用于缓存 PointGet 在新语句上绑定当前快照；调用者负责只在可复用状态调用。
- `append_decoded_row` / `append_decoded_row_for_table`：前者使用扫描自身 `table_id`，后者接受显式物理表 ID，供分区/二次读取场景复用。`typed_index_lookup.rs` 和 `typed_point_get.rs` 直接调用后者。
- `ColumnIDs`：返回当前输出列 ID 顺序，供缓存 PointGet 的计划形状校验。
- `clone_detached`：复制配置和当前位置，共享 retriever，但清空 `page_keys`；由 trait 方法 `Detach` 装箱返回。
- `next_inner`：`Next` 与 `NextWithContext` 的共同状态机；后者额外检查 `ExecutionContext.sql_killer`。
- `ExecExecutor` 实现：`Open` 重置游标、计数和生命周期标志；`Close` 标记关闭；`ChunkConfig`、`NewChunk`、`Schema` 描述输出；`TakeLockKeys` 一次性移交本页记录键；`ScannedRows` 返回累计成功解码行数。

## 执行流程

1. `builder.rs::BuildTypedTableScan` 从 `PhysicalTableScan` 选择 `PhysicalTableID`（非零时优先）或逻辑表 ID，并传入 `PKIsHandle`、`Desc`、输出列、已编码 ranges 及 chunk 容量。
2. `new` 在升序时按 `(start, end)` 递增排序，在降序时按 `(end, start)` 递减排序；schema 保持 `columns` 的顺序。此阶段没有 KV I/O。
3. `Open` 把 `range_index` 置零、清空 `cursor`/`page_keys`、归零 `scanned_rows`，并进入 opened 且未 closed 状态。
4. `Next` 进入 `next_inner(None, ...)`；`NextWithContext` 传入执行上下文。函数先拒绝未打开或已关闭实例，再重置输出 chunk 和本页锁键，并在建立迭代器前检查 kill signal。
5. 对当前 range，升序调用 `Retriever::Iter(cursor 或 start, Some(end))`；降序调用 `IterReverse(Some(cursor 或 end), Some(start))`。每个 range 仍按半开边界交给 KV 接口。
6. 迭代有效且 chunk 未满时，读取 key/value，调用 `append_decoded_row`。解码先用 `DecodeRecordKey` 取得表 ID 与 handle；表 ID 不符立即报错。随后按列 ID 构造类型映射并调用 `DecodeRowToDatumMap`，必要时从 handle 补 PK，最后按 `columns` 顺序逐列 `AppendDatum`。零列 schema 用虚拟行数表示一行。
7. 每成功追加一行，增加 `scanned_rows` 并把记录 key 加入 `page_keys`。降序把当前 key 作为下页上界；升序在当前 key 后追加一个零字节作为严格后继起点，避免下一页重复该键。
8. chunk 尚未满时才在当前迭代器上 `Next`。迭代器失效表示 range 完成，推进 `range_index` 并清空 cursor。无论成功或错误，每轮 range 的迭代器都在离开前显式 `Close`。
9. 解码、KV 或 kill 错误会清空本次 chunk 与 `page_keys` 后向上传播，避免调用者看到半页数据或锁住未返回的行。全部 range 耗尽后返回空 chunk；后续调用不再创建迭代器。

## 数据与状态

`columns` 同时定义解码类型、输出顺序和 chunk 列布局；`schema` 是从这些字段类型派生的 adapter 视图。value 中缺失的普通列用 `Datum::default()` 补位，而 `pk_is_handle` 且列带 `HasPriKeyFlag` 时优先从记录 handle 合成整数 Datum。当前实现因此明确面向整数 handle；它没有在本文件中实现 common handle 的主键重建。

`ranges` 在构造后只读。`range_index + cursor` 是可分页状态：`range_index` 指向当前区间，`cursor` 是该区间下次扫描边界。`page_keys` 只记录当前一次成功 `Next` 返回的行键，`TakeLockKeys` 通过 `mem::take` 消费后立即为空。`scanned_rows` 从最近一次 `Open` 起累计，不因 `TakeLockKeys` 或普通 `Next` 清零。

`initial_capacity` 与 `maximum_chunk_size` 原样进入 `ChunkConfig` 和 `chunk::New`，实际停止条件由 `Chunk::IsFull` 决定。空列输出不追加 Datum，而增加 virtual row 数，仍保留正确行数语义。

## 依赖与调用关系

上游生产调用以 `pkg/executor/builder.rs` 为主：`BuildTypedTableScan` 直接创建本扫描；typed physical-plan 构建也创建一个空 ranges 的 `TypedKVScan` 作为 `TypedIndexLookUp` 的记录解码器。`pkg/executor/typed_point_get.rs` 同样内嵌扫描作为共享记录解码器，并使用 `RebindRetriever`、`ColumnIDs` 和 `append_decoded_row_for_table`。`pkg/executor/typed_index_lookup.rs` 在索引键解析出记录键后复用显式表 ID 解码入口。

下游依赖来自 `pkg/executor/Cargo.toml` 中的 workspace path crate：`astersql-kv` 提供 Retriever/Iterator/Key，`astersql-tablecodec` 提供记录键和行解码，`astersql-meta-model` 提供列元数据，`astersql-parser-mysql` 提供主键标志判断，`astersql-types` 提供 Datum，`astersql-util-chunk` 提供列式输出，`astersql-errors` 统一错误。文件没有条件编译项，也不依赖 `nextgen` feature。

在 statement adapter 中，`pkg/executor/adapter.rs::runPessimisticSelectForUpdate` 每次取到非空 chunk 后调用 `TakeLockKeys`，先锁住对应记录键再缓存/暴露行。`TypedLimit`、`TypedProjection`、`TypedSelection` 等包装器也会转移子执行器的键；聚合和部分 join 会有意消费但不继续暴露明细键。这使 `page_keys` 与“当前返回页”保持同一生命周期。

## 错误处理与边界

- 生命周期错误：未 `Open` 或已经 `Close` 后调用 Next，返回 `table scan executor is not open`。`Close` 本身是幂等标志更新，不触碰共享 retriever。
- 键归属错误：`DecodeRecordKey` 得到的 table ID 与期望物理表不同，返回包含实际和期望 ID 的错误，防止跨表 range 静默解码。
- 编码错误：记录键、行 value 或列类型解码错误直接通过 `AdapterResult` 传播；当前页输出和锁键被清空。
- KV 错误：建立 Iter/IterReverse 或推进 Iterator 的错误向上传播。已成功创建的迭代器在闭包结束后总会调用 `Close`；若建立迭代器本身失败，则没有可关闭对象。
- 取消：仅 `NextWithContext` 能观察 `sql_killer`；检查点位于建迭代器前、外层 range 循环、逐行读取及返回前。取消同样回滚当前页。
- 输入边界由上游保证：本文件不检查 `start <= end`、区间是否重叠、是否属于表前缀，也不去重重叠 ranges；错误或重叠输入可能产生空读或重复行。
- EOF 用成功的空 chunk 表示，不是错误。一个恰好填满末页的升序扫描可能在下一次调用中建立空迭代器才确认 EOF；测试把该行为固定为可接受契约。

## 并发与资源生命周期

`TypedKVScan` 自身没有锁、线程或异步任务，预期由一个执行流以 `&mut self` 串行驱动。Retriever 放在 `Arc` 中并要求 `Send + Sync`，因此扫描对象可跨执行边界持有共享快照；但每个 KV Iterator 只存在于一次 `next_inner` 调用内，返回前显式关闭，不把可能非 `Send` 的游标保存在 struct 中。

`Detach` 共享同一个 Retriever，并复制当前位置、打开/关闭状态和扫描计数，所以 detached 实例可从当前游标继续读；它刻意不复制 `page_keys`，避免同一批行在原实例与 detached 实例中重复参与锁记账。`Open` 可重新开始全范围扫描并清零统计；`Close` 不关闭 Retriever，因为它是共享所有权，实际迭代器已逐次关闭。

发生中断或解码错误时，当前 iterator 仍关闭，当前页数据与锁键同时丢弃。已经提交给更早 `Next` 调用的行及累计 `scanned_rows` 不回滚；调用者若要重试完整扫描，应重新 `Open` 或重新构造执行器。

## 与 Go 版本的对应关系

仓库中不存在 `pkg/executor/typed_kv_scan.go`。crate 元数据把整个 Rust crate 对应到 Go package `pkg/executor`，职责上最接近的是 `pkg/executor/table_reader.go::TableReaderExecutor`：两者都有 Open/Next/Close 生命周期、扫描方向/range 状态并向 chunk 填行；Go 构建侧位于 `pkg/executor/builder.go::buildTableReader` 一带，Rust typed 构建侧是 `builder.rs::BuildTypedTableScan`。

两者不能视为行为完全等价。Go TableReader 把逻辑 ranger ranges 转成请求，构造 DAG，处理 TiKV/TiFlash、分区、signed/unsigned range 分段、correlated column、虚拟列、内存跟踪和 `SelectResult` 生命周期；Rust `TypedKVScan` 接受已编码 range，直接遍历调用者提供的 Retriever，并在本地用 tablecodec 解码。Rust 的 kill、锁键移交、detach 和分页行为由 typed adapter 协议显式实现；Go 对应能力分散在 executor、DistSQL result 和会话执行链中。

行解码语义也只做局部对齐：Rust 依据 `ColumnInfo.ID/FieldType` 解码并补 `PKIsHandle` 整数主键，这与 Go 表行解码的元数据规则一致，但本文件没有覆盖 Go TableReader 的虚拟列、common handle、extra handle、分区请求和下推表达式等完整路径。扩展时应以这些差异为兼容边界，不能仅因名称相近推断已支持。

## 扩展指南

- 新增扫描模式或 range 语义时，优先修改 `builder.rs` 的编码/绑定边界，并在 `TypedKVScan::new` 与 `next_inner` 明确排序、半开边界和分页不重复不变量；不要在扫描器内悄悄修正规划层错误。
- 修改行解码时，以 `append_decoded_row_for_table` 为单一入口，同时评估 TypedKVScan、TypedIndexLookUp 和 TypedPointGet。增加 common handle、生成列或特殊默认值前，应先核对 tablecodec 与 Go 行解码规则，而不是继续使用当前整数 handle 补列假设。
- 修改 cursor 算法必须同时覆盖升序、降序、chunk 恰好满、跨多个 range、相邻/重叠 range 和 EOF；正向的 `key + 0x00` 与反向的“当前 key 作为上界”依赖 KV Iterator 的边界契约。
- 修改错误或取消路径必须保持“关闭 iterator、清空当前 chunk、清空 page_keys”三项原子观察效果，否则悲观锁路径可能暴露半页或错锁键。
- 修改 Detach/Rebind 时要明确 Retriever 快照所有权和 page_keys 是否可转移；当前约定是共享快照、复制游标、丢弃未消费锁键。缓存 PointGet 的形状检查还依赖 `ColumnIDs`。
- 测试应继续放在独立的 `pkg/executor/typed_kv_scan_test.rs`，不要嵌入源文件。若影响包装器或共享解码器，还应同步 `typed_index_lookup_test.rs`、`typed_point_get_test.rs`，以及按键传递相关的 typed limit/projection/selection 测试。
- 性能关注点包括每行重建 `column_types` HashMap、每页重建 KV Iterator、range 排序和记录 value 全量解码；任何缓存优化都必须保持列元数据不可变、物理表校验和错误页回滚语义。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引含 `pkg/executor` Rust 源码；`node --file pkg/executor/typed_kv_scan.rs --offset 1 --limit 500` 返回完整 334 行，并报告该文件被 typed tests、`typed_point_get.rs` 等 7 个文件使用；`query TypedKVScan --kind struct` 定位结构体于第 32 行。精确 `callers` 查询未返回结果并被终止，因此调用边又以局部源码搜索交叉核验，没有把缺失图边写成结论。
- 主实现：`pkg/executor/typed_kv_scan.rs` 的 `KeyRange`、`TypedKVScan::{new,next_inner,append_decoded_row_for_table,clone_detached}` 及 `impl ExecExecutor`。
- crate/装配：`pkg/executor/Cargo.toml` 的 package、lib、feature 与直接 workspace 依赖；`pkg/executor/lib.rs` 的生产模块和独立测试模块声明。
- 上游与复用：`pkg/executor/builder.rs::{BuildTypedTableScan,BuildTypedPhysicalPlanWithBindings}`，以及该文件构建 TypedIndexLookUp 解码器的位置；`pkg/executor/typed_index_lookup.rs`、`pkg/executor/typed_point_get.rs` 对共享解码 API 的调用；`pkg/executor/adapter.rs::runPessimisticSelectForUpdate` 对 `TakeLockKeys` 的消费。
- Rust 独立测试：`pkg/executor/typed_kv_scan_test.rs` 验证建迭代器前取消、迭代中取消时关闭游标且不泄漏半页/锁键、真正的 codec 行懒读取、升降序分页无重复、多 range 降序、Detach 后继续共享快照，以及物理 builder 返回 typed lazy executor。
- Go 对照：确认无同路径 `typed_kv_scan.go`；读取 `pkg/executor/table_reader.go::{TableReaderExecutor,Open,Next,Close}` 和 `pkg/executor/builder.go` 的 TableReader 构建片段，只据此描述职责映射与已验证差异。
- 本任务是纯文档分析，按计划不运行 Cargo；最终仅运行任务指定的 11 章节结构验证，并人工复核本说明未把 Go DistSQL 能力误写成 Rust 当前能力。
