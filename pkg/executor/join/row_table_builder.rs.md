# `pkg/executor/join/row_table_builder.rs` 逻辑说明

## 文件定位

`row_table_builder.rs` 属于 Cargo crate `astersql-executor-join`。crate 入口 `pkg/executor/join/lib.rs` 以 `pub mod row_table_builder` 导出它，并在 `#[cfg(test)]` 下装配独立测试 `row_table_builder_test.rs`。`pkg/executor/join/Cargo.toml` 将库入口设为同目录 `lib.rs`，没有为本模块声明专用 feature；现行实现只直接使用标准库以及同 crate 的 `join_row_table`、`join_table_meta`。

它位于 Hash Join v2 的 build 侧：`HashTableContext::build`（`hash_join_v2.rs`）逐个处理已经分区的 `Vec<Row>`，经私有函数 `build_row_table` 创建 `RowTableBuilder`，再由 `process_chunk` 生成 `RowTable`，交给 `HashTableV2` 建索引和后续 probe。当前生产调用把构建器的 `partition_number` 固定为 1，因此文件虽提供多分区计算，主调用点是在上游已分区之后为每个分区各建一张单分区行表。

文件必须分成两部分理解：第 1–609 行是被块注释包住的 Go 风格移植草稿，不参与编译；第 610 行之后才是现行 Rust 实现。草稿列出了更完整的目标形态，但不能作为当前已支持行为的证据。

## 核心职责

现行实现承担四项职责：

1. 用 `Value` 和 `Chunk = Vec<Vec<Value>>` 表示 join crate 内共享的简化行数据；`Value` 也被 `hash_join_v1.rs`、`hash_join_v2.rs`、各 probe/joiner 和多份测试复用。
2. `RowTableBuilder::process_chunk` 判定过滤结果与 NULL join key、序列化 join key、计算 hash 和分区，并把保留行编码成 `RowTableSegment`。
3. `encode_row`、`serialize_key`、`encode_value` 建立 `EncodedRow` 的 null map、key、列数据和 8 字节对齐布局。
4. 提供恢复/分区、最大长度估算、冲突链 next 指针和对齐长度等辅助 API。

这里的 `RowTable` 同时保留 `segment.raw_data` 和结构化的 `segment.rows: Vec<EncodedRow>`。现行 probe/hash-table 代码主要通过结构化行访问；`raw_data` 是连续编码载荷与 Go 布局语义的保留。它不是 Go `rowTableBuilder` 的完整等价移植，尤其不负责表达式求值、SQL 取消检查、内存 tracker 或真正的 spill 重建。

## 主要符号

- `Value`：公开枚举，支持 `Null`、布尔、有/无符号整数、浮点、字节串和文本。浮点以 `to_bits()` 编码；字节串和文本以 `u32` 小端长度加内容编码。
- `Chunk`：公开类型别名 `Vec<Vec<Value>>`。外层是行，内层是列；它不是 Go 的列式 `chunk.Chunk`。
- `RowTableBuilder`：公开、可克隆的构建状态。配置字段包括 `build_key_indices`、`partition_number`、`has_nullable_key`、`has_filter`、`keep_filtered_rows`、`null_map_length`；逐批次观测缓冲包括 `hash_values`、`partition_indices`、`valid_keys`、`serialized_keys`。
- `RowTableBuilder::new`：唯一构造入口，拒绝 0 或非 2 的幂的分区数。
- `reset_buffer`：按当前行数清空并重建四个观测缓冲，不保留旧元素；容量可能由 `Vec` 复用。
- `process_chunk`：普通 build 主入口，也是当前唯一生产使用的构建方法。
- `process_restored_chunk`：检查传入分区数等于构建器配置后，以 `filter_result=None`、`partition_mask_offset=0` 调回 `process_chunk`。
- `regenerate_hash_and_partition`：保持传入 hash 不变，只按 offset 和分区掩码重新算分区。
- `check_max_element_size`：估算 chunk 中最大的“行数据长度 + 固定 key 长度 + next 指针宽度”，返回 `(是否不超过上限, 最大估值)`。
- `encode_row`、`serialize_key`、`encode_value`、`hash_bytes`：私有编码与 FNV-1a 风格 hash 实现。
- `fill_next_row_pointer`：公开薄封装，转调 `RowTableSegment::set_next_row_address`；仓库精确搜索未发现现行调用者。
- `calculate_row_data_length`、`calculate_fake_length`：公开估算辅助函数；前者被 `check_max_element_size` 使用，后者目前仅由独立测试直接验证。

文件没有 trait、模块级运行时常量或条件编译项。`SIZE_OF_NEXT_PTR` 来自 `join_row_table.rs`；`AtomicBool` 用于初始化每个 `EncodedRow.used`。

## 执行流程

现行生产路径如下：

1. `HashTableContext::build` 遍历 `partitioned_rows`，调用 `hash_join_v2.rs::build_row_table`。
2. 空输入直接返回空 `RowTable`；非空输入用一个分区、可空 key、无过滤、保留无效行的参数创建 `RowTableBuilder`。
3. `process_chunk` 先以 `chunk.len()` 重置逐行缓冲，并为配置的每个分区创建空 `RowTableSegment`。
4. 对每行先检查 `row.len() >= meta.build_types.len()`。过滤结果由 `filter_result[row_index]` 提供；没有切片或索引越界时默认通过。随后扫描 `build_key_indices` 判断是否存在 `Value::Null`。只有 `has_nullable_key` 为真时 NULL key 才使行无效。
5. 若行无效且 `keep_filtered_rows` 为假，立即跳过；对应缓冲位置仍保留 reset 后的默认值，`valid_keys` 已记为 false。
6. `serialize_key` 按 key 下标调用 `encode_value`。`VariableSerialized` 模式还会在每个字段编码前插入该字段编码长度；其他模式不加这一层字段长度。
7. 有效行对 key 做 `hash_bytes`，再用 `((hash >> partition_mask_offset) & (partition_number - 1))` 取分区。保留的无效行使用递增假 hash，并轮询分区，避免集中到一个 segment。
8. `encode_row` 依次写可选 used 占位字节、null map、序列化 key、`row_columns_order` 指定的列数据，最后补零到 8 字节边界；同时保存 key/row-data 偏移并创建 `AtomicBool(false)`。
9. 目标 segment 同步追加 `raw_data`、hash、分区号、空 next 指针和结构化 `EncodedRow`。有效行还记录 segment 内位置并增加 `valid_key_count`。
10. 只把非空 segment 放入结果；加入前调用 `init_tagged_bits`，由 `segment.rows` 的基址计算可用于 tagged pointer 的位数。

成功返回后，调用者把若干 `RowTable` 交给 `HashTableV2::new`。本文件只构建行表，不在这里建立 hash 桶或冲突链。

## 数据与状态

`RowTableBuilder` 的配置与批次状态混合在一个结构中。`new` 后配置保持不变；每次 `process_chunk` 都覆盖四个逐行向量，因此这些向量只描述最近一次调用。构建失败也不提供事务式回滚：例如短行错误发生在 reset 之后，测试明确确认 `hash_values` 和 `valid_keys` 已调整到输入行数；但局部 `partitions` 尚未返回，调用方不会得到部分 `RowTable`。

有效性不变量是 `passed_filter && !(has_nullable_key && has_null_key)`。`has_filter` 字段在现行编译实现中没有参与此判定：是否过滤实际只取决于调用者是否传入 `filter_result`。同样，构建器的 `null_map_length` 字段不参与编码，`encode_row` 使用的是 `meta.null_map_length`。扩展者不能假设这两个构造参数会主动校验或驱动行为。

`EncodedRow.bytes` 的现行布局是 `[可选 used 字节][null map][key][row data][零填充]`。`key_offset`、`key_length`、`row_data_offset` 是结构化边界；null map 按 `row_columns_order` 中的保存位置编号，并以低位优先 `1 << (saved_index % 8)` 置位。`segment.raw_data` 只是逐行 bytes 的串接，不另写 Go 版本位于行首的 next-pointer 占位。

分区数必须是 2 的幂，因而掩码 `partition_number - 1` 有效。普通生产调用配置为 1；多分区行为主要由 `row_table_builder_test.rs` 验证。

## 依赖与调用关系

上游直接调用边（RustCodeGraph 符号查询并以 `rg` 补核）：

- `hash_join_v2.rs::HashTableContext::build` → `build_row_table` → `RowTableBuilder::new` → `process_chunk`。
- `RowTableBuilder::process_restored_chunk` → `process_chunk`；生产代码未找到 `process_restored_chunk` 的调用者。
- `row_table_builder_test.rs` 直接覆盖构造、普通构建、恢复入口、重新分区和长度辅助函数。

下游直接依赖：

- `join_table_meta.rs::{JoinTableMeta, KeyMode, EncodedRow}` 决定保存列、key 模式、null map 和偏移语义。
- `join_row_table.rs::{RowTable, RowTableSegment, SIZE_OF_NEXT_PTR}` 接收构建结果，并提供 segment 统计、tagged bits 与 next pointer 操作。
- `RowTableSegment::init_tagged_bits` 依赖 `tagged_ptr.rs` 的地址 tag 计算；`set_next_row_address` 接受 tagged pointer（该 crate 中底层类型与 `usize` 兼容）。
- 标准库 `AtomicBool` 初始化外连接等场景使用的行命中标志；后续由 `JoinTableMeta` 的 used-flag API 以原子操作读写。

RustCodeGraph 索引能定位 `RowTableBuilder` 和上述函数，但本次对 `callers/callees` 的命令在 30 秒窗口内未返回；因此调用边又用精确符号搜索和相邻源码逐处核对。图的 `RowTableBuilder` trail 也显示 `hash_join_v2.rs` 与独立测试的导入关系。

## 错误处理与边界

显式 `Result<_, String>` 错误只有三类：`new` 拒绝非法分区数；`process_chunk` 拒绝列数少于 `meta.build_types.len()` 的行；`process_restored_chunk` 拒绝恢复分区数不匹配。`regenerate_hash_and_partition` 还保留“分区数为 0”错误，但通过公开构造器创建的实例无法触发。

需要额外注意以下未显式报错边界：

- `build_key_indices` 和 `meta.row_columns_order` 通过 `row[index]` 直接索引。代码只检查行宽是否覆盖 `meta.build_types`，并不单独验证构建器 key 下标与 metadata 的一致性；非法手工组合可能 panic。正常路径依赖 `new_table_meta` 和调用者提供一致元数据。
- 过短的 `filter_result` 不报错，缺失位置按 `true` 处理；`has_filter` 也不强制要求过滤切片。
- `Bytes`/`Text` 长度以及 `VariableSerialized` 字段长度用 `as u32` 转换，现行实现没有 Go 版的“大于 4GB”显式保护。
- `check_max_element_size` 的布尔含义是“fits”，与 Go `checkMaxElementSize` 返回“是否超限”相反；它还是估算整行最大长度，不是逐列检查超大元素。调用者需避免按 Go 返回语义解释。
- `calculate_row_data_length` 对 metadata 标为变长但值不是 `Bytes/Text` 的情况只计 1 字节；其用途是估算而非复现 `encode_row` 的全部载荷。`fixed_key_length` 也可能与实际 `serialize_key` 字节数不同。
- 对空 chunk，`process_chunk` 成功返回无 segment 的表；`check_max_element_size` 返回 `(true, 0)`（只要上限不小于 0）。

## 并发与资源生命周期

`RowTableBuilder::process_chunk` 需要 `&mut self`，单个实例不能同时处理两个 chunk；文件本身不创建线程、锁、任务、通道或异步资源。每次调用创建临时 partition 向量，成功时把非空 segment 移入返回的 `RowTable`，错误时临时 segment 随栈释放。

并发相关状态位于产出的 `EncodedRow.used: AtomicBool`，供后续 probe/未匹配扫描并发标记，而不是由 builder 修改。`RowTableSegment::init_tagged_bits` 在 rows 已填充、即将移入表之前执行；之后若扩展代码会导致 `rows` 重新分配，必须重新审视由基址推导的 tagged bits 和指针稳定性。

与 Go 版本不同，现行 Rust 没有预分配 helper、memory tracker/global arbitrator 记账、SQL killer 周期检查、worker ID、共享 hash-table context 的“全部成功后挂接”协议，也没有 spill 文件或其关闭生命周期。`RowTable` 所有权由值返回，资源释放依靠 Rust 容器析构。

## 与 Go 版本的对应关系

同路径 `row_table_builder.go` 是权威对照。Rust 的 `RowTableBuilder::new/reset_buffer/process_chunk` 大致对应 Go 的 `createRowTableBuilder/ResetBuffer/processOneChunk`；`process_restored_chunk`、`regenerate_hash_and_partition`、`calculate_row_data_length`、`calculate_fake_length` 也有同名意图。无效行采用假 hash 轮询分区、保留/丢弃过滤行、8 字节对齐等核心意图由 Rust 测试和 Go 测试共同佐证。

但当前语义差异显著：

- Go 接收列式 `chunk.Chunk`、选择向量、真实 `FieldType` 与 `codec.SerializeKeys`；Rust 使用简化 `Vec<Vec<Value>>` 和本地编码，未实现 collation、符号兼容等完整 SQL key 规范。
- Go 通过 `expression.VectorizedFilter` 自己求过滤条件，Rust 只消费调用者给出的布尔切片。
- Go 在 build 与 restore 循环检查 SQL killer，限制单元素/key 不超过 `uint32`，并传播表达式/序列化错误；Rust 没有这些路径。
- Go 先精确预估每个 segment 容量、记入 memory tracker，并在全流程成功后追加到共享 hash-table context；Rust 逐步增长 `Vec` 并返回独占 `RowTable`。
- Go 的原始行布局含 next-pointer 占位、可能的 key-length/假 key、带 `colOffsetInNullMap` 且高位优先的 null map；Rust 使用结构化 `next_rows`、无 key-length字段，并采用紧凑保存列位置和低位优先 null bit。
- Go `fnv.New64` 使用其既定序列化 hash 流程；Rust `hash_bytes` 是文件内 FNV-1a 风格实现。跨实现 hash 值不能假定一致。
- Go restore 会对旧 hash 做 `rehash`、按 spill 行格式重建 segment、更新内存统计；Rust restore 只是重新执行普通构建，`regenerate_hash_and_partition` 不改变 hash。
- Go builder 是完整 Hash Join v2 worker 流程的一部分；Rust 当前生产接线只在上游分区后以 `partition_number=1` 使用，恢复辅助函数没有生产调用者。

文件前部注释草稿更接近 Go 结构，但其中类型和函数均不可执行，不能用来弥补上述差异。

## 扩展指南

若扩展普通 build 行为，首要修改点是 `process_chunk`，并同步检查 `serialize_key`、`encode_row` 与 `join_table_meta.rs` 的读取契约。任何布局变化都要同时核对 `join_row_table.rs`、`hash_table_v2.rs` 和 probe/joiner 对 `EncodedRow` 偏移、null map、used 标志及 next 链的使用，不能只修改 `raw_data`。

若接入真正多分区，需先决定上游 `partitioned_rows` 与 builder 内部分区的职责，避免双重分区；同时明确 `partition_mask_offset` 与 `partition_number` 的约束。新增过滤必须消除 `has_filter` 与 `filter_result` 之间当前松散关系，至少校验切片长度。新增字段类型或 SQL 等值语义时，应在 `Value`、`encode_value`、`serialize_key` 和 probe 侧序列化逻辑中保持同一规范，特别关注文本 collation、整数符号、浮点/NULL 和端序。

若实现 Go 等价 spill/respill，不能在现有 `process_restored_chunk` 上只做局部补丁：需要定义恢复 chunk 格式、真正 rehash、内存记账、取消检查和“失败不发布 segment”的提交边界，并在生产 spill 主链接线。若引入共享并发构建，要保留 builder 临时状态的 worker 私有性，并验证 `EncodedRow.used` 的原子顺序及 segment 地址稳定性。

应同步扩展独立的 `row_table_builder_test.rs`，优先覆盖非法下标和过滤长度、各 `KeyMode` 字节契约、多分区高位 mask、超大长度保护、恢复 rehash、错误后状态以及 `raw_data`/结构化行一致性。与 Go 对齐时还要对照 `row_table_builder_test.go` 的 `TestKey`、`TestColumnsBasic`、`TestColumnsAllDataTypes`、`TestBalanceOfFilteredRows` 和对齐检查；不能用现有简化 Rust 测试代替完整 SQL 类型与选择向量验证。

## 验证依据

- 目标实现：`pkg/executor/join/row_table_builder.rs`；RustCodeGraph 定位了 `RowTableBuilder`（第 631 行）、`process_chunk`（第 692 行）、恢复/长度/指针辅助函数及其签名，并按文件读取了完整 909 行。
- crate/模块边界：`pkg/executor/join/Cargo.toml`、`pkg/executor/join/lib.rs`。
- 生产调用链：`pkg/executor/join/hash_join_v2.rs` 中 `HashTableContext::build` 与 `build_row_table`；精确搜索确认 `process_chunk` 的生产调用只在该处，恢复和 rehash API 无生产调用者。
- 直接数据依赖：`pkg/executor/join/join_row_table.rs` 的 `RowTableSegment`/`RowTable`，以及 `pkg/executor/join/join_table_meta.rs` 的 `JoinTableMeta`/`EncodedRow`/`KeyMode`/`new_table_meta`。
- Rust 独立测试：`pkg/executor/join/row_table_builder_test.rs`，覆盖 key/分区/对齐、过滤行保留与丢弃、无效行均衡、紧凑 null map、恢复分区形状、最大长度、非法分区数和短行错误。
- Go 对照：`pkg/executor/join/row_table_builder.go`、`pkg/executor/join/row_table_builder_test.go`；用于核对完整 build/spill 语义、内存/取消机制、布局和测试意图。
- RustCodeGraph `callers/callees` 查询在本地索引的 30 秒执行窗口内未返回；相关边均以精确 `rg` 和源码读取补证。按任务约束未运行 Cargo，本文只做静态事实与结构验证。
