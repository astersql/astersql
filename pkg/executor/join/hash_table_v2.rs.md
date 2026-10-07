# `pkg/executor/join/hash_table_v2.rs`

## 文件定位

本文件属于 `astersql-executor-join` crate；crate 入口 `pkg/executor/join/lib.rs` 以 `pub mod hash_table_v2` 暴露它，并由同 crate 的 `hash_join_v2.rs` 直接使用。它位于 Hash Join V2 的构建侧数据链中：`HashTableContext::build` 先把各分区的构建行编码为 `RowTable`，再交给 `HashTableV2::new` 建立按 hash 分桶的索引；probe 阶段经 `HashTableContext::lookup` 返回 `RowPos` 候选，随后仍由 `hash_join_v2.rs` 比较真实 join key，避免把 hash 相等误当成键相等。

`pkg/executor/join/Cargo.toml` 将库入口设为 `lib.rs`，并声明 Go 包来源为 `pkg/executor/join`。本文件自身只直接依赖同 crate 的 `join_row_table::RowTable` 与 `join_table_meta::EncodedRow`，不直接引入外部 crate。当前 Cargo 清单的大部分移植依赖位于 `target.'cfg(windows)'.dependencies`；这属于整个 join crate 的构建配置，而不是本哈希表独有依赖。

## 核心职责

- `SubTable` 为一个分区保存构建侧 `RowTable`，并将每个有效 join key 的完整 `u64` hash 与三维行坐标 `RowPos` 写入桶冲突链（`SubTable::new`、`SubTable::build`）。
- `HashTableV2` 管理全部可选分区，提供建表、分区替换、按分区 lookup、行读取、内存估算、spill 后清空及全局行号遍历（`HashTableV2` 的各方法）。
- `RowIter` 把跨分区、跨 segment 的物理布局投影为连续的全局区间 `[start, end)`，供未匹配构建行扫描等上游逻辑使用。
- `next_power_of_two`、`get_hash_table_length_by_row_len` 和 `get_hash_table_memory_usage` 保留 Go 版本的桶容量与账面内存计算规则。

该实现不是独立执行器，也不负责 join key 相等比较、分区 hash 计算、输出行拼装或落盘 I/O；这些职责分别留在 `hash_join_v2.rs`、`join_table_meta.rs`、`joiner.rs` 及 spill 组件中。

## 主要符号

- `MINIMAL_HASH_TABLE_LEN = 32`：桶数组的最小逻辑长度。由于 `next_power_of_two` 返回“严格大于”输入的幂，32 个有效 key 会分配 64 桶。
- `TAGGED_POINTER_LEN`：取当前目标平台 `usize` 宽度，供与 Go `taggedPointerLen` 对齐的账面估算使用；Rust 桶的真实容器是 `Vec<Vec<(u64, RowPos)>>`，因此该值不是 Rust 实际堆占用测量。
- `RowPos { sub_table_index, row_segment_index, row_index }`：定位一条编码构建行的稳定逻辑坐标；它实现 `Copy`、比较与 hash，供 lookup、迭代、取行和已匹配标记传递。
- `SubTable`：公开 `row_data` 和两个构造时空状态标志，内部持有桶链 `hash_table` 与二次幂掩码 `pos_mask`。`lookup` 先以 `hash & pos_mask` 选桶，再以完整 hash 过滤桶内冲突。
- `HashTableV2`：内部 `tables: Vec<Option<SubTable>>` 允许分区尚未填充；`partition_number` 固定记录槽位数。`new` 一次性填充，`empty` 配合 `replace_partition` 延迟填充。
- `RowIter<'a>`：借用 `HashTableV2`，以两个全局行号 `current`、`end` 表示半开区间；标准 `Iterator` 的 item 是 `RowPos`，`get_value` 可读取当前 `EncodedRow`。
- 容量辅助函数：`next_power_of_two`、`get_hash_table_length_by_row_table`、`get_hash_table_length_by_row_len`、`get_hash_table_memory_usage` 均为公开 API。

## 执行流程

1. `HashTableV2::new` 接收按分区排列的 `Vec<RowTable>`，逐项调用 `SubTable::new(table, partition)`；分区号被写进后续产生的每个 `RowPos`。
2. `SubTable::new` 读取 `valid_key_count`，计算至少 32 且为二次幂的桶长，初始化空标志、空桶和 `pos_mask`，再由 `with_built_rows` 对全部 segment 调用 `build`。
3. `SubTable::build` 将请求区间裁到实际 segment 数。正常路径严格遍历 `valid_join_key_positions` 并以该下标读取 hash；只有旧夹具没有保存与 `valid_key_count` 等长的位置数组时，才兼容性地取 `hash_values` 前 N 项。每个有效项通过 `update_hash_value` 追加到对应桶。
4. probe 时，`HashJoinExecutorV2::probe_row` 在 `hash_join_v2.rs` 中计算 probe hash 与分区，然后经 `HashTableContext::lookup`、`HashTableV2::lookup`、`SubTable::lookup` 获得完整 hash 相等的候选坐标。上游随后调用 `keys_equal` 做真实键比较，再读取 `original_rows` 形成 join 结果。
5. 需要扫描全部构建行时，`create_row_iter` 把反向区间夹成空区间（`start.min(end)`），并验证 `end` 不超过总行数。每次 `RowIter::next` 调用 `create_row_pos`，后者依次扣减非空分区及 segment 的行数，将全局序号解析为 `RowPos`。
6. spill/reset 路径由 `HashTableContext::reset` 调用 `clear_partition_segments`；该操作清除该分区的行 segment 与桶内容，使行数和内存估算下降，但不改写构造时缓存的两个空标志。

## 数据与状态

桶长始终由 `get_hash_table_length_by_row_len` 生成，因此非空桶数组长度至少为 32 且是 2 的幂，`pos_mask = len - 1` 才能用位与代替取模。桶元素保存完整 hash 而不只保存低位桶号；不同 hash 即使落入同一桶也会被 `lookup` 过滤，同一完整 hash 可返回多个构建行坐标。

`HashTableV2::total_row_count` 只统计当前仍存在的 `Some(SubTable)` 的 `RowTable` 行数。`create_row_pos(total)` 返回 `sub_table_index == partition_number` 的结束哨兵；大于 total 返回 `Err`。空分区或 `None` 分区在坐标解析时被跳过。`get_row` 对分区、segment 和行下标逐层使用安全索引，无效坐标返回 `None`。

内存值是兼容性账面值：`SubTable::total_memory_usage` 等于 `RowTable::total_memory_usage()` 加“桶数 × 指针宽度”。清空后桶长度变为 0、行段也被释放，所以该公式返回 0；它没有计算 Rust 内层 `Vec` 容量及 `(u64, RowPos)` 元素的真实分配成本。

## 依赖与调用关系

上游直接调用证据位于 `pkg/executor/join/hash_join_v2.rs`：`HashTableContext::build` 调用 `HashTableV2::new` 并记录 `total_memory_usage`；`HashTableContext::lookup` 转发分区 lookup；`original_row`、`hash_table_row_count`、`unmatched_rows` 使用 `create_row_iter`；`mark_used` 使用 `get_row`；`reset` 使用 `clear_partition_segments`。`create_build_tasks` 读取 `total_row_count` 来划分逻辑任务，但并不调用本文件的 `SubTable::build`。

下游数据依赖是 `pkg/executor/join/join_row_table.rs` 的 `RowTable`/segment API（行数、有效 key 数、segment 列表、清空和内存估算），以及 `pkg/executor/join/join_table_meta.rs` 的 `EncodedRow`。模块注册与测试注册分别见 `pkg/executor/join/lib.rs` 的 `pub mod hash_table_v2` 和 `#[cfg(test)] mod hash_table_v2_test`。

RustCodeGraph 能定位 `HashTableV2`、`SubTable` 及文件源码，并显示 `hash_join_v2.rs` 的直接引用；其方法级 `query/callers/callees` 对这些 impl 方法没有返回稳定节点，因此方法调用边又用限定在 `pkg/executor/join/*.rs` 的文本引用核验，未据此扩大到间接或推测调用者。

## 错误处理与边界

- `HashTableV2::replace_partition` 对越界分区返回包含分区号的 `Err(String)`；`lookup`、`partition_memory_usage`、`clear_partition_segments` 对越界或 `None` 分区分别退化为空结果、0 或无操作。
- `create_row_pos` 对 `position > total` 返回错误，对恰好等于 total 返回结束哨兵；若内部行数与 segment 无法对应，则返回“could not be resolved”。`RowIter::new` 拒绝超过总行数的上界。
- `RowIter::get_value` 将坐标解析错误转为 `None`；调用者应先检查 `is_end`，因为在结束哨兵处没有实际行。
- `next_power_of_two` 在 `value >= 2^63` 时 panic，以避免左移溢出；其余输入返回严格大于输入的最小 2 的幂。
- `build` 会忽略超过实际 segment 数的结束下标；`valid_join_key_positions` 中越过 `hash_values` 的项由 `filter_map` 跳过。兼容前缀路径仅用于旧夹具，生产数据若缺失精确位置会改变可索引行集合，扩展时不可把它误当成正常格式。

## 并发与资源生命周期

`RowIter` 持有共享借用，迭代期间哈希表不能被可变清空或替换；返回的 `EncodedRow` 引用也受哈希表借用生命周期约束。`HashTableV2::replace_partition` 以新 `SubTable` 整体替换旧分区，旧桶和行数据随后按 Rust 所有权规则释放；`clear_partition_segments` 就地释放行段和桶元素，但保留 `SubTable` 对象及空状态缓存。

必须注意：Rust 的 `SubTable::atomic_update_hash_value` 当前只是调用 `update_hash_value`，并依赖 `&mut self` 独占访问；`build` 也接收 `&mut self`。因此它没有实现 Go 版本以 `atomic.CompareAndSwapUintptr` 支持多个 worker 同时写同一子表的能力。现有 `SubTable::new` 是构造期单线程整表 build，当前接线是安全的；若未来真的共享并行 build，必须先引入锁、分片桶或原子化表示，并增加并发测试，不能仅调用这个同名方法。

## 与 Go 版本的对应关系

Rust 文件与 `pkg/executor/join/hash_table_v2.go` 对齐了分区子表、最小桶长、严格上取 2 的幂、全局行坐标/迭代、内存账面公式、分区清空以及“只索引 `validJoinKeyPos`”的核心语义。`hash_table_v2_test.rs` 还专门验证非前缀有效位置（只索引第 3 行）和清空后不重写空标志。

主要实现差异如下：

- Go 桶保存带 hash tag 的行地址并通过行内 next pointer 串链；Rust 不保存裸指针，改用 `Vec<(完整 hash, RowPos)>` 冲突链，生命周期更安全，但真实内存布局与性能成本不同。
- Go `lookup` 只以 tag 快速排除并返回链头，调用方沿链继续检查；Rust 直接扫描单桶并返回所有完整 hash 相等的 `RowPos`，上游仍须比较 join key。
- Go 局部 segment build 使用 CAS 并发插入；Rust 尚无等价并发写入，如上一节所述。
- Go 的 `createRowPos` 对越界 panic；Rust 改为 `Result`。Go `createRowIter` 最终也可能因非法 end 在坐标创建处 panic；Rust 显式校验 end。两者都把 `start > end` 变为空区间。
- Rust 新增 `empty`、`replace_partition`、`get_row` 与跨分区总内存等便利 API；它们服务当前 Rust 上游，不是 Go 同文件逐函数翻译。

Go 测试 `hash_table_v2_test.go` 还覆盖百万级 build、CAS 并发 build、tagged pointer lookup 与多并发区间扫描；Rust 独立测试覆盖容量、lookup、分区替换/清空、空分区迭代、拆分区间及有效 key 位置，但没有证明 Go CAS 并发场景或 tagged pointer 布局，因为 Rust 当前采用不同表示且未移植并发写入能力。

## 扩展指南

- 改动分桶或 lookup 时，优先修改 `SubTable::{new, build, update_hash_value, lookup}`，保持桶长二次幂、`pos_mask` 与完整 hash 过滤三个不变量；同步扩展 `pkg/executor/join/hash_table_v2_test.rs` 的冲突、重复 hash、精确有效位置测试，并核对 `hash_join_v2.rs::probe_row` 的真实键复核流程。
- 改动分区生命周期时，关注 `HashTableV2::{empty, replace_partition, clear_partition_segments}` 与 `HashTableContext::{build, reset}`；需要明确清空后空标志是否仍应保持 Go 兼容语义。
- 改动全局遍历时，集中处理 `create_row_pos`、`create_row_iter` 和 `RowIter`，测试空/`None` 分区、多 segment、`0..0`、反向区间、恰好 total 和超过 total。测试必须继续放在独立的 `hash_table_v2_test.rs`，不要内嵌到生产文件。
- 若要求真实并行建表，应先定义同步策略和内存模型，再改造 `atomic_update_hash_value`/`build` 及上游任务接线；应新增 Rust 并发压力测试，并以 Go `TestConcurrentBuild` 的“每行恰好出现一次”为最低行为基线。
- 改动内存追踪时，不应继续把 tagged-pointer 兼容公式称为 Rust 实际内存；可保留兼容指标，同时另增明确命名的真实容量估算，并同步 `HashTableContext::memory_bytes` 的使用者。

## 验证依据

- 生产源码：`pkg/executor/join/hash_table_v2.rs`（全部 619 行）；直接入口与调用：`pkg/executor/join/hash_join_v2.rs` 的 `HashTableContext`、`HashJoinExecutorV2::probe_row`、`create_build_tasks`；模块入口：`pkg/executor/join/lib.rs`。
- crate 边界：`pkg/executor/join/Cargo.toml`；目标目录没有 `doc.go`，包职责由 `lib.rs` 模块文档与真实调用链核验。
- Rust 独立测试：`pkg/executor/join/hash_table_v2_test.rs`，覆盖容量/内存公式、build/lookup、分区替换与清空、跨空分区迭代、拆分区间、精确有效 key 位置及缓存空标志。
- Go 对照：`pkg/executor/join/hash_table_v2.go`；Go 测试：`pkg/executor/join/hash_table_v2_test.go`，其中 `TestHashTableSize`、`TestBuild`、`TestConcurrentBuild`、`TestLookup`、`TestRowIter` 提供原始语义和并发差异证据。
- RustCodeGraph：执行了 `status`（索引含 11,467 文件、307,296 节点、1,848,419 边）、`query HashTableV2`、`query SubTable`、按文件 `node` 读取目标源码与直接调用片段；方法级图查询无稳定结果后，以限定目录的 `rg` 补足直接引用。
- 本任务是纯文档分析，按计划不运行 Cargo、Rust/Go 单元测试或构建；最终以固定十一章节结构命令、链接/事实人工复核和 diff 范围检查交付。
