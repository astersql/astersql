# `pkg/util/chunk/chunk.rs`

## 文件定位

本文件实现 `astersql-util-chunk` crate 的核心列式批容器 `Chunk`。它不是独立模块入口，而是由 `pkg/util/chunk/internal/group1/lib.rs` 的 `chunk_impl` 通过 `include!("../../chunk.rs")` 注入，再由 `pkg/util/chunk/lib.rs` 的 `pub use group_1::*` 暴露给其他 crate。`pkg/util/chunk/Cargo.toml` 将 crate 根设为 `lib.rs`，并通过 `types`、`types-field`、`types-scalar` 提供 Datum、字段类型和标量类型依赖。

在完整应用中，`Chunk` 是 SQL 执行链上的批量行载体：执行器按列写入结果，表达式按逻辑行读取或临时设置 selection vector，协议层和游标逐行消费。例如 `pkg/session/runtime/canonical_table_reader.rs` 用 `Reset`、`IsFull`、`AppendDatum` 生成结果批，`pkg/server/internal/resultset/cursor.rs` 用 `NumRows`/`GetRow` 消费批，`pkg/expression/chunk_executor.rs` 用 `SetSel` 临时过滤后恢复 selection。对象池入口则经本文件的 `NewChunkFromPoolWithCapacity`/`Destroy` 转到 `internal/group1/lib.rs` 的桥接函数，再落到 `pkg/util/chunk/pool.rs`。

## 核心职责

- 定义一批数据的元状态：`Chunk { sel, columns, numVirtualRows, capacity, requiredRows, inCompleteChunk }`（`chunk.rs:30`）。实际单元格字节、偏移和 null bitmap 由 `Column` 持有。
- 创建、续用和回收批：`NewEmptyChunk`、`NewChunkWithCapacity`、`New`、`Renew`、`GrowAndReset`、`Reset`、`Destroy`。
- 维护逻辑行与物理行的映射：`NumRows`、`GetRow`、`SetSel`、`appendSel`、`Reconstruct`。
- 以整行、投影列、批区间、Datum 或具体 MySQL 类型追加数据，并保持定长/变长列、offsets、null bitmap 和 `length` 一致。
- 提供列投影、列引用、交换、深拷贝、截断、内存估算和调试格式化等批操作。

本文件遵循 Go `pkg/util/chunk/chunk.go` 的重要前提：这些 API 通常不做完整输入校验，调用者必须保证 schema、列下标、行区间、类型和各列行数一致。

## 主要符号

- `MSG_ERR_SEL_NOT_NIL`：`MakeRefTo`、`swapColumn`、`swapColumnWithin` 在任一相关 Chunk 存在 `sel` 时返回的固定错误消息，防止把逻辑行下标误当成物理行下标。
- `InitialCapacity = 32`、`ZeroCapacity = 0`：容量增长的初始值和零容量常量；`reCalcCapacity` 从零增长时使用前者。
- `IntoFieldType`：让构造函数同时接受 `types::FieldType` 和 `Box<types::FieldType>`，是 Rust 对 Go 指针式字段描述的适配层。
- `Chunk`：核心结构。`sel: Option<Vec<usize>>` 保留 Go 中 nil selection 与已存在 selection 的区别；`columns` 保存列；零列或 `inCompleteChunk` 场景由 `numVirtualRows` 表示逻辑行数；`capacity` 是增长基线；`requiredRows` 是 `IsFull` 阈值；`inCompleteChunk` 支持 join 的非完整列批。
- 构造与续用：`NewEmptyChunk` 创建零预分配列，`NewChunkWithCapacity` 令初始容量和最大批大小相同，`New` 把实际容量限制为 `min(capacity, maxChunkSize)`，`Renew`/`renewWithCapacity` 只复制 schema 与元属性而不复制行，`renewColumns` 按旧列 `typeSize` 建空列。
- 行数与容量：`NumRows` 优先返回 `sel.len()`，否则在不完整/零列批上返回 `numVirtualRows`，普通批返回首列 `length`；`SetRequiredRows` 把 0 或超过上限的值回退到 `maxChunkSize`；`IsFull` 比较逻辑行数与 `requiredRows`。
- 引用与复制：`Prune`、`MakeRef`、`MakeRefTo` 使用 `Column::reference_clone` 保持相同 `reference_id`；`swapColumn`/`swapColumnWithin` 借助 `Column::same_ref` 修复同源引用组；`CopyConstruct` 深拷贝全部物理列，`CopyConstructSel` 只复制已选行。
- 写入：`AppendRow*`/`AppendRows*` 处理 Row 及投影，`Append` 复制 `[begin, end)`，各 `Append<Type>` 写具体类型，`AppendDatum` 按 `Datum::Kind` 分派，`appendCellByCell` 是定长/变长列的共同单元复制原语。
- 整理与读取：`GetRow` 返回映射后的 `Row` 视图，`Reconstruct` 将 selection 物化到每列，`TruncateTo` 先物化再截断数据、offsets 与 null bitmap，`ToString` 按逻辑行格式化。
- 原始内存入口：`AppendRaw` 接收已编码切片；`AppendCellFromRawData` 从裸指针读取定长数据或 `u32` 长度前缀的变长数据并返回新偏移。

## 执行流程

典型结果批流程如下：

1. 调用方用 `New`/`NewChunkWithCapacity` 按字段类型创建列；每列由 `NewColumn` 选择定长或变长布局，`requiredRows` 初始化为最大批大小。
2. 执行器逐列调用 `AppendDatum` 或具体的 `AppendInt64`、`AppendBytes` 等。每次写第 0 列时，`appendSel` 会在 selection 已启用的情况下追加新物理行号；真正的编码由 `Column` 方法完成。
3. `NumRows` 给出可见逻辑行数，`IsFull` 判断是否达到父执行器需要的行数。消费方用 `GetRow` 获得只读行视图；存在 `sel` 时先将逻辑下标映射到物理下标。
4. 过滤阶段可用 `SetSel(Some(...))` 暂时只暴露部分行。需要连续物理布局时，`Reconstruct` 对每列调用 `Column::reconstruct`，把选中行压紧并清除 `sel`。
5. 批被消费后，`Reset` 清除 selection、列长度和虚拟行数但保留已分配容量；需要扩容时 `GrowAndReset` 调用 `reCalcCapacity`，仅当当前行数达到容量才倍增，且不超过 `maxChunkSize`。
6. 池化场景通过 `Destroy` 把拥有的 `Chunk` 交给 group1 桥接函数；`pkg/util/chunk/pool.rs::PutChunk` 清空各列并按列宽归还全局池。

批复制存在三条不同路径：`Prune`/`MakeRef` 是引用语义，`CopyConstruct` 是全部物理数据的深拷贝，`CopyConstructSel` 是 selection 物化后的深拷贝。扩展代码必须先明确需要哪一种所有权和可见行语义。

## 数据与状态

`Column` 的关键不变量是：所有列代表同一批逻辑行；定长列的 `data.len()` 等于 `length * elemBuf.len()`；变长列的 `offsets` 有 `length + 1` 项，最后一项等于数据尾偏移；null bitmap 每一位描述一个物理行。`appendCellByCell` 同时追加 null 位、数据/offset 和 `length`，`Append` 则对一个行区间批量完成同样更新。

`sel` 只改变可见行和 `GetRow` 映射，不立即删除物理数据。`appendSel` 以第 0 列长度为新物理行号，因此多列追加必须按一致顺序完成；零列批改用 `numVirtualRows`。`TruncateTo` 先调用 `Reconstruct`，随后裁剪每列，并清除 null bitmap 末字节中超出新长度的高位。

`capacity` 与底层 `Vec` 的实际 capacity 不是同一概念：前者是批增长决策的行容量元数据，`MemoryUsage` 则按列内各 `Vec::capacity()` 估算已保留内存；`UsedMemoryUsage` 按 `len()` 估算当前使用量。`Reset` 后二者可能分别保持和下降，这一行为由 `chunk_test.rs::go_merge_11_used_memory_excludes_retained_capacity` 验证。

## 依赖与调用关系

下游依赖主要分为四组：

- 列与行：`Column` 提供创建、追加、重建、引用标识、深拷贝及类型宽度；`Row::view` 提供指向 Chunk 的行视图。二者由 `internal/group1/lib.rs` 和 crate 根统一再导出。
- 类型系统：`types::FieldType` 决定列布局，`types::Datum` 和 `Kind*` 决定动态追加分支，`Time`、`Duration`、`MyDecimal`、`Enum`、`Set`、`BinaryJSON`、`VectorFloat32` 对应专用列编码。
- 错误：列引用/交换的 selection 冲突通过 `group_1::errors::New` 返回 `errors::Error`；crate 根可把它转换为 `ChunkError::Message`。
- 生命周期：`getChunkFromPool`、`putChunkFromPool` 是 group1 中接收拥有值的桥接函数，实际复用由 `pkg/util/chunk/pool.rs` 的全局 `OnceLock<Mutex<HashMap<...>>>` 与分宽度缓存完成。

RustCodeGraph 显示 `NewChunkWithCapacity -> New`，`CopyConstructSel -> CopyConstruct/renewWithCapacity/appendCellByCell`，`GrowAndReset -> reCalcCapacity/renewColumns/Reset`，`AppendDatum -> 各 Append<Type>`，`Destroy -> putChunkFromPool`。跨 crate 上游调用边在本次索引查询中不完整，源码搜索补充确认了 `pkg/executor/cte_table_reader.rs` 调用 `CopyConstructSel`，`pkg/executor/{operate_ddl_jobs,explain,load_stats,expand,show}.rs` 调用 `GrowAndReset`，`pkg/table/column.rs`、`pkg/planner/cardinality/selectivity.rs` 和多处执行器/表达式测试调用 `NewChunkWithCapacity`。

## 错误处理与边界

本文件显式返回错误的主要路径只有 `MakeRefTo`、`swapColumn`、`swapColumnWithin`：selection 非空时返回 `MSG_ERR_SEL_NOT_NIL`。其余多数边界依赖 Rust 下标检查、断言或调用约定：列下标越界、`begin > end`、区间超过源行、schema 宽度不匹配、变长 offsets 缺失都会 panic；`AppendRaw` 对定长值强制 `raw.len() == elemBuf.len()`。

`AppendDatum` 对已列出的 Datum kind 分派，兜底分支不写入任何值；增加新 Kind 时若不更新该 match，可能造成列间行数失衡。`SetRequiredRows` 只接受 `usize`，因此对应 Go 的“非正数”边界在 Rust 中表现为 0。`TruncateTo(0)` 不会进入末字节清位分支，但大于当前物理行数会因切片/offset 索引越界而 panic。

`AppendCellFromRawData` 是 `unsafe` 边界：调用者必须保证 `rowData` 非空、对读取 `u32` 足够对齐、从 `currentOffset` 起至少有声明长度的数据，并保证数据编码与目标列宽一致。函数不追加 null bitmap，适用于已经由外层协议管理有效性位的原始行路径，不能直接替代普通 Append API。

## 并发与资源生命周期

`Chunk` 本身没有内部锁或异步任务；所有修改都要求 `&mut self`，正常 Rust 借用规则禁止同一 Chunk 的并发写。`Row` 是指向 Chunk 的视图，因此调用 `Reset`、`GrowAndReset`、`Reconstruct`、`TruncateTo` 或列交换前，调用方必须保证旧视图和旧数据不再被使用；这也与 Go `Reset` 的注释约束一致。

本文件的列引用通过 `reference_clone` 和 `reference_id` 表达 Go 的列指针同源关系。交换逻辑先定位同源组最左列，交换主列后重新建立组内引用；selection 存在时禁止操作。对象池的并发同步不在本文件中，而在 `pool.rs` 的 `OnceLock` 和多个 `Mutex<Vec<Column>>` 中；`Destroy(self, ...)` 消耗 Chunk 所有权，防止归还后继续使用原值。

内存复用的生命周期分三档：`Reset` 保留列缓冲；`GrowAndReset` 可能重建更大的列缓冲；`Destroy` 把列移入全局池。`CopyConstruct`/`CopyConstructSel` 创建独立数据，适合需要跨越源 Chunk 复用周期的消费者。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/util/chunk/chunk.go` 的 `Chunk`、构造函数、容量控制、selection、追加、复制、截断、引用和对象池 API。关键一致性包括：`requiredRows` 默认取最大批大小；`NumRows` 的 selection/虚拟行/首列优先级；容量仅在当前批已满时倍增；`CopyConstructSel` 忽略未选物理行；`TruncateTo` 清理 null bitmap 尾部无效位；`AppendDatum` 覆盖相同 MySQL Datum 类别。

Rust 特有适配包括：Go 的 nil slice 用 `Option<Vec<usize>>` 和 `Vec` 的“长度/容量”状态近似；`IntoFieldType` 兼容值与 `Box`；`Box<Chunk>` 表达堆所有权；同一 Chunk 内列交换另设 `swapColumnWithin` 以避免制造两个别名 `&mut Chunk`；裸指针入口使用 `std::slice::from_raw_parts`。Rust 还额外提供 `AppendRaw`，用于行落盘解码时追加已编码单元格。

当前 Rust 测试 `pkg/util/chunk/chunk_test.rs` 是独立测试文件，覆盖普通值/NULL、容量与已用内存、Append/Truncate/Copy/selection、requiredRows、投影/虚拟行、零列批增长、引用交换和格式化。Go 的更大测试集位于 `pkg/util/chunk/chunk_test.go`；后续移植功能时应先找到其中对应测试意图，再在独立 Rust 测试文件中保持相同分支，而不是把测试嵌入生产文件。

## 扩展指南

- 新增一种 Datum 类型：同步修改 `types::Datum`/Kind、`Column` 的编码与读取方法、`Chunk::AppendDatum` 分派和具体 `Append<Type>`；在 `pkg/util/chunk/chunk_test.rs` 增加 NULL、普通值、复制、selection、截断覆盖，并核对 `chunk.go`/`chunk_test.go` 对应语义。
- 修改 selection：重点审查 `NumRows`、`GetRow`、`appendSel`、`CopyConstructSel`、`Reconstruct`、`TruncateTo`，并保持“逻辑下标到物理下标”不变量。列引用或交换功能不得绕过 selection 检查。
- 修改列共享/交换：同时核对 `Column::reference_clone`、`same_ref` 和 `reference_id`，验证同源多列在跨 Chunk 与同 Chunk 交换后仍组成正确引用组。
- 修改容量/内存策略：同步检查 `New`、`Renew`、`GrowAndReset`、`reCalcCapacity`、`MemoryUsage`、`UsedMemoryUsage` 和池化路径，警惕把行容量与底层字节容量混为一谈。
- 修改原始数据格式：`AppendCellFromRawData`、磁盘 codec/row decoder 和 `sizeUint32` 必须一起审查；这是 unsafe 与兼容性风险最高的入口，应增加独立边界测试。
- 修改对象池：保持 `NewChunkFromPoolWithCapacity` 与 `Destroy` 的字段列表、列数和列宽对称，测试放在 `pkg/util/chunk/pool_test.rs`，Chunk 行为测试仍放在 `chunk_test.rs`。

性能风险主要来自逐单元复制、selection 物化、无意深拷贝和频繁重建列；兼容风险集中在 Go nil/空状态、Datum kind、null bitmap、变长 offsets 与 raw row 编码。所有新 API 应明确是引用、视图还是深拷贝。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/chunk` 确认该目录 Rust/Go 文件均已索引。
- `rustcodegraph node --file pkg/util/chunk/chunk.rs --symbols-only`：确认本文件有 `Chunk`、3 个常量/trait 项及构造、状态、追加、复制、选择、池化等共 78 个符号；分段 `node --file` 阅读了全部 924 行。
- RustCodeGraph 调用证据：`NewChunkWithCapacity -> New`；`CopyConstructSel -> CopyConstruct, renewWithCapacity, appendCellByCell`；`GrowAndReset -> Reset, renewColumns, reCalcCapacity`；`AppendDatum -> 13 个具体追加函数`；`Destroy -> putChunkFromPool`。精确 caller 查询未返回部分跨 crate 使用，因此用源码搜索补充，未把缺失图边解释为“没有调用者”。
- crate 与装配：`pkg/util/chunk/Cargo.toml`、`pkg/util/chunk/lib.rs`、`pkg/util/chunk/internal/group1/lib.rs`；后者确认 `include!("../../chunk.rs")`、`Row` 再导出、`sizeUint32` 及对象池桥接。
- 直接实现依赖：`pkg/util/chunk/column.rs` 的 `same_ref`/`reference_clone`；`pkg/util/chunk/pool.rs` 的全局池、`GetChunk`、`PutChunk`。
- Go 对照：完整核对 `pkg/util/chunk/chunk.go` 的同名结构和方法；相关回归意图来自 `pkg/util/chunk/chunk_test.go`。
- Rust 测试：完整读取 `pkg/util/chunk/chunk_test.rs`；并通过调用搜索确认 `pool_test.rs`、`chunk_in_disk_test.rs`、`row_container_test.rs` 等相邻独立测试使用核心构造/回收 API。
- 本任务是纯文档分析，按任务要求未运行 Cargo；结构验证命令及退出码在交付前单独执行并报告。
