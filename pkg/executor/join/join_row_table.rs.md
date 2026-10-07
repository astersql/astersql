# `pkg/executor/join/join_row_table.rs`

## 文件定位

本文件属于 `astersql-executor-join` crate；crate 入口 `pkg/executor/join/lib.rs` 以 `pub mod join_row_table` 导出它。它位于 Hash Join V2 的构建侧数据链路中：`row_table_builder.rs::RowTableBuilder::process_chunk` 把输入行编码并按分区组成 `RowTableSegment`，`hash_table_v2.rs::SubTable` 再持有 `RowTable`、按有效 join key 建桶，并在 probe 或扫描阶段通过行位置取回 `EncodedRow`。`pkg/executor/join/Cargo.toml` 的 `[package.metadata.porting]` 将整个 crate 对应到 Go 包 `pkg/executor/join`。

源码第 16—227 行是一段全部被 `//` 注释掉的早期 unsafe 移植草稿，不参与编译。实际可执行定义从 `use crate::join_table_meta::EncodedRow` 开始，采用拥有所有权的 `Vec<EncodedRow>`、索引和 `Option<TaggedPtr>`；理解当前行为时不能把注释稿中的裸指针 API 当成已启用实现。

## 核心职责

- `RowTableSegment` 保存一批 build-side 编码行及与这些行按下标对齐的 hash、分区、有效键位置和冲突链 next 信息。
- `RowTable` 把多个 segment 组合成一张逻辑行表，提供跨段计数、全局行访问、有效键序号到全局行号的换算、合并和清空。
- `SIZE_OF_NEXT_PTR`、`SIZE_OF_ELEMENT_SIZE` 保留行布局中指针槽与元素长度槽的平台宽度语义；构建器的最大行长估算直接使用 `SIZE_OF_NEXT_PTR`。
- 内存统计按容器 `capacity` 估算，供 `hash_table_v2.rs::SubTable::total_memory_usage` 及 spill/内存控制路径使用；它不是分配器精确账单。

## 主要符号

- `SIZE_OF_NEXT_PTR: usize`：当前平台 `usize` 宽度；`row_table_builder.rs::check_max_element_size` 用它计入每行 next 槽。
- `SIZE_OF_ELEMENT_SIZE: usize`：`u32` 宽度，对应 Go 的变长元素长度字段。当前文件内不消费它，但 `join_row_table_test.rs` 固定验证其平台语义。
- `RowTableSegment`：公开字段包括 `raw_data`、`rows`、`hash_values`、`partition_indices`、`valid_key_count`、`valid_join_key_positions`、`tagged_bits`、`next_rows`。核心不变量是与行有关的向量应按相同行下标对应；有效位置必须指向 `rows`/`hash_values` 中存在的行。
- `RowTableSegment::total_used_bytes`：累计 `raw_data` capacity、每个 `EncodedRow` 的 `bytes`/`null_map` capacity，以及 hash、分区、有效位置、next 向量的 capacity 成本。结构体本身、`Vec` 头部与分配器开销未计入。
- `row_count`、`valid_key_count`、`get_row`、`get_row_bytes`：分别返回段内行数、缓存的有效键数，以及安全的可选行/字节视图。
- `init_tagged_bits`：取 `rows` 向量基址，交给 `tagged_ptr.rs::get_tagged_bits_from_uintptr` 计算可作 tag 的高位数。它不扫描首尾行地址，与文件前半注释稿不同。
- `set_next_row_address`：必要时扩展 `next_rows`，再写入指定下标；因此可写尚无对应 `rows` 的槽位，调用者必须维护行对齐不变量。
- `get_next_row_address`：安全读取 next；槽不存在或为 `None` 返回 `None`，存在时仅在 pointer 的 tag 位等于 `TagPtrHelper::get_tagged_value(hash_value)` 时返回。
- `RowTable`：`segments` 私有，只能经 `segments`/`segments_mut` 访问；其余方法提供跨段聚合与定位。
- `RowTable::valid_join_key_position`：按各段 `valid_key_count` 定位第 N 个有效键，再以 `valid_join_key_positions[N]` 得到段内真实行号；仅为旧夹具兼容，在位置表为空时把有效行视作从零开始的连续前缀。

## 执行流程

1. `RowTableBuilder::process_chunk` 为每个分区创建默认 `RowTableSegment`，逐行判定过滤结果和 NULL key。需要保留但不能匹配的行仍进入 segment，但不会写入有效位置表。
2. 构建器编码出 `EncodedRow`，把字节追加到 `raw_data`，并同步 push `hash_values`、`partition_indices`、`next_rows` 与 `rows`；有效行还追加 `valid_join_key_positions` 并递增 `valid_key_count`。
3. 非空 segment 调用 `init_tagged_bits` 后，经 `RowTable::segments_mut` 加入结果表。`RowTable::merge` 可按原顺序把另一张表的全部 segment 移入当前表。
4. `HashTableV2::new` 为每个分区的 `RowTable` 创建 `SubTable`。`SubTable::build` 优先严格使用 `valid_join_key_positions` 和对应 `hash_values` 生成 `RowPos`；只有旧夹具的计数与位置表不一致时才退回“前 N 行有效”的兼容路径。
5. probe 得到 `RowPos` 后，`HashTableV2::get_row` 经 subtable、segment、row 三层边界检查，最终调用 `RowTableSegment::get_row`。需要线性扫描整张逻辑表时，`RowTable::get_row` 逐段扣减行数。
6. spill 或清理分区时，`SubTable::clear_segments` 调用 `RowTable::clear_segments` 释放其 segment 所拥有的行缓冲；`RowTable` 随后计数和内存估算均归零。

## 数据与状态

`EncodedRow` 定义于 `join_table_meta.rs`，持有完整编码字节、null map、key/row-data 偏移以及原子 `used` 标志。`RowTableSegment::rows` 是当前活跃实现的权威逐行载荷；`raw_data` 是构建器同步追加的连续字节副本，当前文件的读取 API 不用它反向切行。

`hash_values[i]`、`partition_indices[i]`、`next_rows[i]` 应描述 `rows[i]`。`valid_key_count` 是缓存计数，`valid_join_key_positions` 列出真正可建桶的行下标；两者在正常构建器输出中相等。若手工构造时让计数、位置或 hash 数组失配，当前类型系统不会阻止，部分消费者会走兼容分支或忽略缺失 hash。

`RowTable` 保持 segment 顺序。`merge` 不重编码、不重分区，只追加；因此全局行序为当前表各段之后接上 `other` 的各段。`valid_join_key_position` 的 `row_offset` 按所有行数累计，而输入 `row_index` 按有效键数扣减，这正是“有效键序号”到“全局物理行号”的换算。

## 依赖与调用关系

- 上游生产者：`row_table_builder.rs::RowTableBuilder::process_chunk` 创建 segment、维护所有并行向量和有效位置；`fill_next_row_pointer` 转调 `RowTableSegment::set_next_row_address`。
- 主要消费者：`hash_table_v2.rs::SubTable::{new,build,total_memory_usage,clear_segments}` 使用计数、位置、hash、内存和清理 API；`HashTableV2::get_row` 读取编码行，`get_hash_table_length_by_row_table` 依据有效键数确定桶长。
- spill 消费者：`hash_join_spill_helper.rs` 接收 `RowTable`/`RowTableSegment`，序列化有效键并清理或恢复分区；`hash_join_v2.rs` 在 build、restore 与内存统计路径持有 `RowTable`。
- 本文件的直接类型依赖仅为同 crate 的 `join_table_meta::EncodedRow` 和 `tagged_ptr::{TagPtrHelper, TaggedPtr}`，没有引入新的外部 crate。`Cargo.toml` 将库入口设为 `lib.rs`，而这些兄弟模块由该入口统一装配。
- RustCodeGraph 将该文件列为被 `hash_join_v2.rs`、多份 hash-table/spill 测试等使用；精确源码链同时确认 `process_chunk -> RowTableSegment/RowTable` 与 `SubTable::build -> valid_join_key_positions/hash_values`。

## 错误处理与边界

本文件没有 `Result` 错误通道，查询边界主要用 `Option` 表达：`get_row`、`get_row_bytes`、`get_next_row_address` 和 `valid_join_key_position` 在找不到目标时返回 `None`。`RowTable::get_row` 的跨段减法只在前段容纳不下该索引时执行，不会因正常越界产生下溢。

仍需调用者维护以下前置条件：`init_tagged_bits` 应在 `rows` 存储稳定后调用；之后若 `rows` 扩容并迁移，先前基址推导的 `tagged_bits` 可能失效。`valid_key_count` 应与位置表一致且位置有效；`hash_values` 应覆盖所有建桶位置。`set_next_row_address` 会自动扩容 next 数组但不验证相应行存在。`total_used_bytes` 使用普通 `usize` 加法后转换为 `i64`，面向正常进程内分配，不提供极端容量溢出的防护。

`get_next_row_address` 只检查 tag，不验证 pointer/索引是否确实指向本 segment；当前 Rust 哈希表的活跃实现实际以 `Vec<Vec<(u64, RowPos)>>` 表达桶冲突，next API 主要保留布局/移植边界。不能据此宣称已经启用 Go 的裸指针冲突链。

## 并发与资源生命周期

`RowTable` 与 `RowTableSegment` 自身不含锁或通道；修改 segment、合并和清理都要求 `&mut self`，共享并发写入必须由更高层串行化或加锁。`hash_table_v2.rs::atomic_update_hash_value` 当前也只是转调可变借用的普通更新，不是原子桶更新。

逐行匹配状态由 `EncodedRow::used: AtomicBool` 承担，相关测试以 Release/Acquire 验证读写；它与本文件的容器并发语义是两层概念。`RowTableSegment`/`RowTable` 派生 `Clone`，而 `EncodedRow` 的克隆会建立独立的 `AtomicBool` 状态（由 `join_row_table_test.rs` 验证），不是共享匹配标记。

资源由 `Vec` 所有权管理：`merge` 消耗 `other` 并移动 segments；`clear_segments` 丢弃所有 segment，使其行、字节和辅助数组随所有权释放。`segments_mut` 暴露整个向量，扩展代码可高效装配，但也必须负责上述对齐不变量。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/join/join_row_table.go`。类型层次与聚合语义一致：Go `rowTableSegment`/`rowTable` 对应 Rust `RowTableSegment`/`RowTable`，两端都提供内存估算、行数、有效键数、跨段定位、merge 与 clear。

关键差异如下：

- Go 把整行存入 `rawData`，用 `rowStartOffset` 和 `unsafe.Pointer` 切片/定位；Rust 活跃实现额外保存 `Vec<EncodedRow>`，以安全索引返回 `Option`，并没有活跃的 `rowStartOffset`。
- Go 直接把 tagged next pointer 写在行起始地址；Rust 把它保存在独立的 `next_rows: Vec<Option<TaggedPtr>>`。Go 的 `getNextRowAddress` 返回零哨兵，Rust 返回 `None`。
- Go 的有效键数来自 `validJoinKeyPos.len()`；Rust 同时保存 `valid_key_count` 与 `valid_join_key_positions`，并为缺少位置表的旧测试夹具保留“有效行是前缀”的兼容逻辑。
- Go 初始化与端序有关的 `usedFlagMask`/`bitMaskInUint32`，并链接运行时检查堆对象能否移动；这些只存在于 Rust 文件前半的注释稿，当前活跃 Rust API 没有对应全局状态。Rust 把 used 状态放在 `EncodedRow::AtomicBool` 中。
- Go `initTaggedBits` 合并首尾原始行地址；Rust 仅取 `rows` 向量基址。两者目标相近但证据不支持宣称算法完全等价。

Go 测试 `join_row_table_test.go` 覆盖堆对象不移动、固定宽度、端序 bit mask 和 uintptr 容量；Rust 独立测试只覆盖平台字段宽度、segment 计数/内存/used、跨段 merge/有效位置/clear，以及 clone 后 used 状态独立。因此端序全局量和运行时堆移动检查属于当前 Rust 未实现/未验证的 Go 行为。

## 扩展指南

- 新增段级字段时，应在 `RowTableBuilder::process_chunk` 的每行 push 路径同步维护，并更新 `total_used_bytes`；若字段与行一一对应，补充长度对齐和越界测试。
- 修改有效键规则时，要同时维护 `valid_key_count` 与 `valid_join_key_positions`，并验证 `hash_table_v2.rs::SubTable::build` 对非连续有效行仍使用真实位置；不要依赖旧夹具兼容分支。
- 修改 tagged pointer 语义时，应联动 `tagged_ptr.rs`、`row_table_builder.rs::fill_next_row_pointer` 和本文件的 set/get 方法，特别验证空槽、错误 tag、向量迁移后重新初始化等边界。
- 改变行布局或内存统计时，应同步 `join_table_meta.rs::EncodedRow`、`row_table_builder.rs::encode_row/check_max_element_size`、spill 序列化路径及 Go 对照语义，并说明统计是 capacity 估算还是精确账单。
- 回归测试应放在独立文件 `pkg/executor/join/join_row_table_test.rs`；涉及建桶位置的行为还应扩展 `hash_table_v2_test.rs`，涉及构建编码则扩展 `row_table_builder_test.rs`。不要把测试嵌回生产源文件。

## 验证依据

- RustCodeGraph：`status` 确认本仓库索引包含 11,467 个文件；`files --filter pkg/executor/join` 确认 Rust/Go 对照、模块与测试集合；`explore "pkg/executor/join/join_row_table.rs JoinRowTable JoinRowTableBuilder"` 与后续包含 `RowTableSegment`、`row_table_builder`、`hash_table_v2` 的查询用于核对文件主体、构建/建桶链和使用面。
- RustCodeGraph 精确查询：`query RowTableSegment --kind struct`、`query RowTable --kind struct`、`query EncodedRow --kind struct`，以及目标文件、`row_table_builder.rs`、`hash_table_v2.rs` 的 `node --file` 输出；已核对 `EncodedRow` 定义和 `process_chunk -> SubTable::build -> get_row` 的数据流。
- 读取路径：`pkg/executor/join/join_row_table.rs`、`lib.rs`、`Cargo.toml`、`join_row_table.go`、`join_row_table_test.rs`、`join_row_table_test.go`；直接证据还包括 `row_table_builder.rs`、`hash_table_v2.rs`、`join_table_meta.rs` 与 `hash_join_spill_helper.rs` 的相关符号/引用。
- 测试证据：`join_row_table_test.rs` 验证字段宽度、段计数、内存非零、原子 used、merge 后全局定位、有效位置和 clear；`hash_table_v2_test.rs` 的引用与调用面验证非连续有效位置、分区清理和内存关系。任务是纯文档分析，按计划未运行 Cargo。
- 人工复核结论：本文分别描述了可执行实现和注释化旧稿，所有“已支持”陈述均可回指上述符号；未把 Go 的端序全局量、运行时堆移动检查或裸指针链误写为当前 Rust 已启用行为。
