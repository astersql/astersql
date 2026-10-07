# `pkg/executor/typed_index_reader.rs`

## 文件定位

[`typed_index_reader.rs`](./typed_index_reader.rs) 属于 `astersql-executor` crate（见 [`Cargo.toml`](./Cargo.toml)），实现 Rust typed physical-plan 执行链中的覆盖索引读取器 `TypedIndexReader`。模块由 [`lib.rs`](./lib.rs) 公开为 `pub mod typed_index_reader`，构建入口是 [`builder.rs`](./builder.rs) 的 `BuildTypedPhysicalPlanWithBindings`：当计划节点可下转为 `PhysicalIndexReader` 时，构建器从其 `IndexPlan` 找到 `PhysicalIndexScan`，核对表、索引、输出列和扫描绑定，然后创建本读取器。

它不是 Go `IndexReaderExecutor` 的逐字段翻译。Go 实现在 `pkg/executor/distsql.go` 中构造 DAG/KV 请求并消费 `distsql.SelectResult`；本文件面向已经注入的 `kv::Retriever` 与编码好的 `KeyRange`，在本地直接遍历索引 KV。其职责集中在覆盖索引的叶子扫描、解码和锁键导出，上层 `Selection`、`Limit` 等由构建器包装。

## 核心职责

- `TypedIndexReader::new` 固化扫描依赖和元数据，并按 `descending` 对范围排序，使多个范围按最终输出方向依次消费。
- `next_inner` 实现分页拉取：重置调用方 chunk 和本页锁键，在一个或多个范围中持续读取，直到 chunk 满或全部范围结束。
- `append_index_row` 从索引键值解出索引列和行句柄，仅从覆盖索引或整型主键句柄生成输出列，不回表读取记录。
- 每成功输出一行就生成相应记录键，供 `ExecExecutor::TakeLockKeys` 的上层加锁流程消费；全局索引的 `PartitionHandle` 使用句柄携带的分区 ID，而不是构造时的表 ID。
- 实现 `ExecExecutor` 的生命周期、schema/chunk 配置、扫描计数和 `Detach`，从而可嵌入 typed 执行器树。

## 主要符号

- `pub struct TypedIndexReader`：唯一公开类型。`retriever` 是共享、可跨线程持有的 KV 读取接口；`table_id`、`index_column_ids`、`output_columns` 和 `schema` 是解码元数据；`ranges`、`range_index`、`cursor` 是扫描进度；`page_keys` 与 `scanned_rows` 是执行观测状态；`opened`、`closed` 管理生命周期。
- `pub fn TypedIndexReader::new(...) -> Self`：唯一公开构造函数。它把 `ColumnInfo.FieldType` 投影成 `SchemaColumn`，升序按 `(start, end)`、降序按 `(end, start)` 的逆序排列范围。该函数不访问 KV，也不会自动打开执行器。
- `fn append_index_row(&self, key, value, output) -> AdapterResult<Key>`：私有单行解码器。调用 `CutIndexKey`、`DecodeIndexHandle`、`DecodeOne` 和 `EncodeRowKeyWithHandle`，向列式 chunk 追加 datum 并返回记录锁键。
- `fn next_inner(&mut self, context, output) -> AdapterResult`：私有公共执行内核，`Next` 传入 `None`，`NextWithContext` 传入真实 `ExecutionContext`，后者会检查 `sql_killer`。
- `impl ExecExecutor for TypedIndexReader`：提供 `Open`、`Close`、`Next`、`NextWithContext`、`ChunkConfig`、`NewChunk`、`Schema`、只读/外键属性、`TakeLockKeys`、`ScannedRows` 和 `Detach`。本文件没有模块级常量、trait 定义或条件编译分支。

## 执行流程

1. `pkg/executor/builder.rs` 从 `PhysicalIndexReader.IndexPlan` 定位 `PhysicalIndexScan`，选择物理表 ID，解析索引列 ID，并确保每个输出列要么被索引覆盖、要么是主键句柄列；随后调用 `TypedIndexReader::new`。扫描过滤条件会在外层包装 `TypedSelection`，`wrap_typed_pushdown_plan` 再还原 `Limit`、`Projection` 等下推节点。
2. `Open` 将范围位置、游标、本页锁键和累计行数复位，并设置 `opened = true`、`closed = false`。重复打开等价于从头扫描。
3. 每次 `Next`/`NextWithContext` 进入 `next_inner`，先拒绝未打开或已关闭状态，再清空输出 chunk 和上一页未取走的 `page_keys`。若有 `ExecutionContext.sql_killer`，扫描前、每个范围前和每行前都会调用 `HandleSignal`。
4. 当前范围使用 `Retriever::Iter(start, end)`；降序使用 `IterReverse(end, start)`。续页时边界由 `cursor` 替代范围原始边界。
5. `append_index_row` 切出索引列编码，解出普通或分区句柄，将索引 datum 按列 ID 放入临时 `HashMap`，再严格按 `output_columns` 顺序写入 chunk。索引中没有的整型主键列从 `handle.IntValue()` 补出；其他未覆盖列报错。
6. 行成功追加后，`scanned_rows` 加一、记录键加入 `page_keys`。升序游标在当前 key 后追加零字节以形成排他式后继；降序游标保留当前 key，依赖反向迭代器的上界语义避免重复。
7. chunk 未满时推进迭代器。迭代器耗尽则进入下一范围并清空游标；chunk 满则保留当前范围与游标，下一次调用续扫。每个范围迭代器无论成功失败都调用 `Close`。
8. 所有范围结束后返回空 chunk 表示 EOF。外层执行器可调用 `TakeLockKeys` 取走本页记录键，或用 `ScannedRows` 读取累计成功解码行数。

## 数据与状态

扫描元数据在构造后保持不变：`retriever`、`table_id`、索引列 ID、输出列、schema、方向、范围和 chunk 容量。运行状态由 `range_index` 与 `cursor` 共同描述，前者指出当前范围，后者指出该范围的续扫位置；`Open` 是将这些状态重新建立为初始值的唯一入口。

`page_keys` 只对应最近一次成功 `Next` 产生的页面。下一次读取开始会主动清空它，`TakeLockKeys` 则通过 `std::mem::take` 转移所有权并在读取器内留下空向量。因此调用方若需要锁键，必须在下一次 `Next` 之前取走。`scanned_rows` 是自最近一次 `Open` 起成功写入 chunk 的累计行数，不会因 `TakeLockKeys` 或单页结束而归零。

空 `output_columns` 是显式支持的计数型形态：`append_index_row` 不追加 datum，而调用 `Chunk::SetNumVirtualRows` 增加虚拟行数。非空输出按 `output_columns` 的顺序和字段类型构造 chunk，`schema` 与 `NewChunk` 使用同一组 `ColumnInfo.FieldType`，保持列数与类型一致。

## 依赖与调用关系

上游静态接线为 `BuildTypedPhysicalPlan` → `BuildTypedPhysicalPlanWithBindings` → `TypedIndexReader::new`（`pkg/executor/builder.rs`）。直接测试也可构造读取器（`pkg/executor/typed_index_reader_test.rs`）。运行期通常由实现于 `pkg/executor/adapter.rs` 的 `ExecExecutor` 调度；外层 typed `Selection`、`Limit`、`Projection` 等通过 trait 调用它的 `Open/Next/Close`，而锁读取流程通过 `TakeLockKeys` 获得本页记录键。

下游关键依赖包括：`astersql-kv` 的 `Retriever`、`Iterator`、`Key` 和句柄类型；`astersql-tablecodec` 的索引切分、句柄解码和记录键编码；`astersql-util-codec::DecodeOne` 的 datum 解码；`astersql-util-chunk` 的列式输出；`astersql-meta-model::ColumnInfo` 和 `astersql-parser-mysql::HasPriKeyFlag` 的列元数据判断。这些依赖均由 `pkg/executor/Cargo.toml` 声明为工作区路径依赖。

RustCodeGraph 能定位 `TypedIndexReader`、`append_index_row`、`next_inner` 及测试侧 import，但当前索引没有产出这些函数的有效 callers/callees 边；因此构建器入口、trait 调度和 codec 调用关系由相邻源码直接核验，而不是把缺失图边解释为“无调用”。

## 错误处理与边界

所有执行错误通过 `AdapterResult<T> = Result<T, errors::SharedError>` 传播。明确的本地错误包括：未 `Open` 或已 `Close` 时读取；索引 KV 无行句柄；请求输出列既不在索引 datum 中、也不是可从整型句柄恢复的主键列。构建阶段还会提前拒绝缺失 `IndexPlan`/`PhysicalIndexScan`/表/索引、非法索引列偏移、输出列不属于 scan，以及非覆盖输出列。

`CutIndexKey`、`DecodeIndexHandle`、`DecodeOne`、KV 迭代器创建/推进和 kill signal 的错误均用 `?` 原样上抛。若迭代期间出现解码、推进或 kill 错误，`next_inner` 会在返回前重置输出 chunk 和本页锁键，避免调用方看到半页结果；已增加的 `scanned_rows` 和已经推进的游标不回滚，因此错误后不能把该实例视为事务性重试点。迭代器的 `Close` 返回值不参与结果，当前接口调用也未检查关闭错误。

范围边界完全由 `KeyRange` 和 `Retriever` 的半开/反向迭代约定决定；本文件只排序并续接，不合并重叠范围，也不去重。构建器必须提供与索引、方向和绑定一致的范围。复合/非整型主键只有在索引编码提供相应 handle 时可形成记录键；只有 `handle.IsInt()` 的主键列才会被作为缺失输出 datum 补出。

## 并发与资源生命周期

读取器需要 `&mut self` 执行，单个实例本身不支持并发推进。`retriever` 使用 `Arc<dyn Retriever + Send + Sync>`，使原实例与 detached 实例可共享底层读取源；范围、元数据、游标等值状态则在 `Detach` 时复制。`Detach` 保留当前 `range_index`、`cursor`、累计行数和开关状态，但刻意创建空 `page_keys`，避免原实例与副本重复交付同一页锁键。测试 `typed_index_reader_detaches_its_cursor_and_preserves_descending_record_keys` 证明原实例关闭后，副本仍可从降序游标继续。

每次范围扫描创建一个短生命周期 KV 迭代器，并在本次范围处理结束或失败后显式 `Close`。`Close` 对读取器只设置标志，不清理 `Arc`、范围、chunk 配置或游标，也不直接关闭底层共享 retriever；实际内存由 Rust 所有权和引用计数释放。`NextWithContext` 仅借用执行上下文，并在多个边界检查共享 `SQLKiller`，不会在读取器内保存会话上下文。

## 与 Go 版本的对应关系

最接近的 Go 生产实现是 `pkg/executor/distsql.go` 的 `IndexReaderExecutor`：两者都实现执行器生命周期、按方向读取索引结果、向上游分批返回 chunk，并支持 detach。`pkg/executor/detach.go` 中 Go `IndexReaderExecutor.Detach` 采用浅拷贝加静态化执行上下文；Rust `Detach` 复制自有扫描状态、共享 `Arc<Retriever>`，且不持有 session 表达式上下文，满足“原执行器和返回执行器都仍可使用”的同一目标。

两者运行边界不同。Go `Open` 负责重建 ranger 范围、按分区生成 KV ranges、构建 DAG 请求、建立 DistSQL result 和内存跟踪器，`Next` 主要委托 `SelectResult.Next`；Rust 构建器已收到编码范围与 retriever，`TypedIndexReader` 自己迭代原始 KV 并解码覆盖列。Go 还具有 grouped ranges、分区表列表、分页、stale read、replica scope、runtime stats、index usage report 和 dummy reader 等字段，本文件均未实现，不能据 Go 能力推断 Rust 已支持。

Rust 特有的直接证据位于 `pkg/executor/typed_index_reader_test.rs`：三项测试分别验证降序分页与 detach、全局索引分区记录锁键、以及 `PhysicalIndexReader → Limit → PhysicalIndexScan` 构建后不会在 Limit EOF 后额外扫描。Go `pkg/executor/distsql.go`、`pkg/executor/detach.go` 只能作为执行意图对照，不是本文件逐行等价测试。

## 扩展指南

- 增加新的覆盖列来源或 handle 形态时，优先修改 `append_index_row`，并在 `pkg/executor/typed_index_reader_test.rs` 增加独立测试；同时检查 `pkg/executor/builder.rs` 的预检规则，避免构建阶段和运行阶段对“覆盖”的定义分叉。
- 改变范围排序、游标边界或分页行为时，修改 `new`/`next_inner`，必须覆盖升序、降序、多范围、chunk 恰好填满、空范围和重叠范围。尤其要验证不会重复/漏读，也不会在上层 Limit 已结束后额外调用 retriever。
- 扩展全局索引或分区能力时，保持 `PartitionHandle.PartitionID` 生成物理记录键的不变量，并同步全局索引测试；错误使用逻辑表 ID 会把锁施加到错误分区。
- 改变锁键交付时序时，联动 `ExecExecutor::TakeLockKeys` 的上层消费者，保留“一页行与一页键一一对应、下一页前取走”的约定。不要把测试逻辑内嵌进生产文件，继续使用同目录独立的 `typed_index_reader_test.rs`。
- 引入并行预取、后台任务或可失败的关闭操作时，需要明确任务取消、iterator/retriever 所有权和错误优先级；当前同步 `&mut self` 模型没有后台资源。若要靠近 Go DistSQL reader 的能力，应在计划/构建/请求层另行接线，而不是在此叶子读取器中默默模拟完整子系统。
- 性能敏感点是每行创建 datum `HashMap`、复制 key/value/handle、升序游标追加字节以及每个范围重建迭代器；优化时必须先用等价输出、锁键和错误清理测试守住行为。

## 验证依据

- 源码与模块边界：`pkg/executor/typed_index_reader.rs`、`pkg/executor/lib.rs`、`pkg/executor/adapter.rs`、`pkg/executor/Cargo.toml`。
- 上游构建与包装：`pkg/executor/builder.rs` 中 `BuildTypedPhysicalPlan`、`BuildTypedPhysicalPlanWithBindings`、`find_typed_index_scan` 和 `TypedIndexReader::new` 调用点。
- Rust 独立测试：`pkg/executor/typed_index_reader_test.rs` 的 `typed_index_reader_detaches_its_cursor_and_preserves_descending_record_keys`、`typed_index_reader_uses_global_index_partition_record_lock_key`、`typed_index_reader_builder_executes_nested_limit_without_extra_scan`。
- Go 对照：`pkg/executor/distsql.go` 的 `IndexReaderExecutor.Open/Next/Close` 与 KV 请求流程，`pkg/executor/detach.go` 的 `IndexReaderExecutor.Detach`。
- RustCodeGraph：`status` 显示项目索引包含 Rust/Go 文件；`query TypedIndexReader --kind struct`、`query append_index_row --kind function`、`query next_inner --kind function` 和 `node TypedIndexReader` 定位了目标符号。`files --filter pkg/executor/typed_index_reader` 及宽泛 `explore` 未提供有效目标调用图，精确 callers/callees 也未产出可用边，因此相关调用关系改由上述源码路径核验。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务给定的 `rg` 结构命令验证恰有十一个固定二级标题，并人工复核文档区分了已验证行为、Go 对照差异和未由调用图覆盖的部分。
