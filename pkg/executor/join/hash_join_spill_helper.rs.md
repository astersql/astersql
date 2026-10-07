# `pkg/executor/join/hash_join_spill_helper.rs`

## 文件定位

该文件属于 `astersql-executor-join` crate。模块由 `pkg/executor/join/lib.rs` 以 `pub mod hash_join_spill_helper` 导出，直接依赖同 crate 的 `join_row_table`、`joiner` 和 `row_table_builder`，因此位于 Hash Join 的分区 row table 与 build/probe 数据之间，负责内存超限后的分区选择、临时存储、恢复队列和状态同步。

当前接线必须与设计意图区分：`pkg/executor/join/hash_join_spill.rs` 的 `HashJoinSpillAction` 持有本文件的 `Arc<HashJoinSpillHelper>`，会调用 `wait_while_spilling`、`status`、`can_spill` 和 `set_need_spill` 完成 OOM 触发状态转换；但 `pkg/executor/join/hash_join_v2.rs` 的活跃 `HashJoinV2Exec::spill_helper` 使用的是该文件内部另行定义的同名简化类型，而不是本模块的类型。RustCodeGraph 未找到本文件 `spill_row_tables`、`spill_probe_chunk` 或 `prepare_for_restoring` 的生产调用者，这些完整落盘/恢复入口目前由独立 Rust 测试直接验证。

源文件第 16--655 行是一段整体注释掉的 Go 风格迁移草稿，不参与编译。实际 Rust 实现从 `use crate::join_row_table::{RowTable, RowTableSegment}` 开始；理解或修改行为时不能把注释草稿中的 `HashJoinV2Exec`、真实磁盘 `DataInDiskByChunks`、SQL killer 或 failpoint 当作当前 Rust 已支持能力。

## 核心职责

- `HashJoinSpillHelper` 维护 `NotSpilled -> NeedSpill -> InSpilling -> NotSpilled` 状态，配合 `Condvar` 让并发 OOM action 等待正在进行的 spill，并用 `can_spill_flag` 表示当前执行阶段是否允许 Hash Join 自己释放内存。
- `choose_partitions_to_spill` 先计入已经标记的分区，再按内存占用降序选择未 spill 分区，直到预计剩余内存不高于触发限额的 `MEM_FACTOR_AFTER_SPILL`（0.5）。
- `spill_build_segments` 把 `RowTableSegment` 转成 `SpilledBuildRow { hash_value, valid_join_key, row_bytes }`；`spill_probe_chunk` 保存探测侧 `Chunk`。两侧按 worker 和 partition 二维索引隔离。
- `spill_row_tables` 与 `spill_remaining_rows` 从 worker row table 取走 segment，写入 `SpillDisk`，并从 `memory_tracker` 扣减调用方提供的预计释放量。
- `prepare_for_restoring` 按分区聚合所有 worker 的 build/probe chunk，压入后进先出的 `RestoreStack`，记录下一轮编号并清空本轮暂存状态。
- `build_spill_bytes`、`probe_spill_bytes`、`spilled_valid_row_num` 及测试标志提供统计或验证信号；`close`、`reset` 管理本轮资源状态。

## 主要符号

- `EXCEED_MAX_SPILL_ROUND_ERROR`：恢复轮次越界的稳定错误文本；`prepare_for_restoring` 在 `last_round + 1 > max_spill_round` 时返回它。
- `MEM_FACTOR_AFTER_SPILL`：分区选择的目标比例 0.5；它不是触发阈值，而是一次 spill 后期望达到的剩余内存比例。
- `SPILL_CHUNK_SIZE`：保留的默认 chunk 行容量 1024；当前可执行实现没有引用它，实际 `SpillDisk` 每次追加由传入 segment/chunk 决定。
- `SpillStatus`：三态枚举。`NeedSpill` 由 OOM action 置位，`InSpilling` 包围实际写出过程，结束后无论成功失败都回到 `NotSpilled`。
- `MemoryTracker`：基于原子整数的轻量计数器，支持正负 `consume`、更新限额与超限判断；负限额表示无限制。它是本 crate 内的迁移辅助类型，不等同于 Go 的完整 `memory.Tracker`。
- `SpilledBuildRow`：恢复 build row 所需的最小表示，保留原 hash、有效 join key 标记和编码行字节。
- `SpillDisk<T>`：内存中的 chunk 容器，同时累计调用方给出的字节数和关闭标志。名称表达 spill 抽象，但当前实现不会创建磁盘文件。
- `RestorePartition` / `RestoreStack`：同一分区跨 worker 聚合后的 build/probe 数据和轮次，以及其 LIFO 容器。
- `SpillState`：由单个 `Mutex` 保护的复合可变状态，包括状态枚举、分区位图、二维暂存区、恢复栈和测试标志。
- `HashJoinSpillHelper`：公共总控类型。尺寸和生命周期不绑定 `HashJoinV2Exec`，构造时显式接收分区数、并发数、最大轮次和内存限额。
- `row_size`：探测侧字节估算函数；`Bytes`/`Text` 按实际长度，其余值统一估算为 8 字节。

## 执行流程

1. 调用方以 `HashJoinSpillHelper::new(partition_num, concurrency, max_spill_round, memory_limit)` 初始化固定形状的 worker×partition 槽位。分区数或并发数为零会立即失败。
2. `HashJoinSpillAction::action_impl` 在自己的 `action_lock` 下先调用 `wait_while_spilling`；当外部 tracker 已超限、helper 仍为 `NotSpilled`、数据量值得 spill 且 `can_spill` 为真时，调用 `set_need_spill` 记录触发时的 consumed/limit 快照。该阶段只发出请求，不写数据。
3. 执行 spill 时，`spill_row_tables` 先置 `InSpilling`，调用 `choose_partitions_to_spill`。算法将 `partition_memory_usage` 与可选 `hash_table_memory_usage` 逐项相加，先复用已 spill 分区的释放量，再稳定地按“占用降序、分区号升序”补选分区，目标为 `memory_tracker.bytes_consumed() - released <= bytes_limit * 0.5`。
4. `spill_selected_partitions` 标记分区，然后逐 worker、逐目标分区以 `std::mem::take` 取空 row table 的 segment。`spill_build_segments` 为每个 segment 生成有效键标记，组合 hash 与编码字节，按一次调用一个 chunk 追加到对应 `SpillDisk`，更新 `disk_tracker`。
5. 上一步完成后从 `memory_tracker` 扣除预计释放量；`spill_row_tables` 随即调用 `set_not_spilled` 并唤醒所有等待者。闭包结果被保留，因此写出失败也会恢复状态并通知等待线程。
6. 已 spill 分区在 build 阶段又积累行时，`spill_remaining_rows` 只处理当前位图中已经标记的分区；`hash_join_spill_helper_test.rs` 验证未标记分区保持原样。
7. probe 阶段由 `spill_probe_chunk` 将 chunk 追加到相同 worker/partition 槽位。它不会自行检查该分区是否已被标记，正确配对由调用者负责。
8. 一轮结束时，`prepare_for_restoring(last_round)` 先检查轮次上限，再对每个已 spill 分区汇总所有 worker 的 chunk。只有 build 侧非空才压栈；之后清空二维暂存区和分区位图。调用者以 `pop_restore_partition` 按 LIFO 顺序消费恢复任务，恢复期间可再次 spill 并形成更高轮次。

## 数据与状态

`HashJoinSpillHelper` 的结构参数 `partition_num`、`concurrency` 和 `max_spill_round` 构造后不变。`SpillState::build_rows_in_disk` 与 `probe_rows_in_disk` 始终按 `[worker][partition]` 寻址，每个槽位惰性创建 `SpillDisk`；`reset` 和成功的 `prepare_for_restoring` 会用同样形状的新空矩阵替换它们。

`spilled_partitions` 是当前轮的位图。`spill_triggered` 表示本轮曾标记分区；`prepare_for_restoring` 或 `reset` 将其清零，而 `spill_triggered_for_test` 等累计测试标志不会随 `reset` 清零。`round` 只在 `prepare_for_restoring` 成功后更新，因此 `is_respill_triggered_for_test` 的条件是已准备过第二轮或更高轮次。

内存相关存在三组数值：`memory_tracker` 是当前 Hash Join 消耗，`bytes_consumed`/`bytes_limit` 是 `set_need_spill` 捕获的触发快照，`disk_tracker` 累计当前进程生命周期内成功追加的估算 spill 字节。分区选择读取实时 `memory_tracker.bytes_consumed()` 和快照 `bytes_limit`；当前实现不读取 `bytes_consumed` 参与选择。`reset`、`prepare_for_restoring` 和 `close` 都不会回退 `disk_tracker`，因此它更接近累计写出量而非当前暂存量。

`generate_spilled_valid_join_key` 根据 `RowTableSegment::valid_key_count` 将前 N 行标为有效，并将 N 累加到 `spilled_valid_row_num`。这与 Go 依据 `validJoinKeyPos` 标记任意位置不同，是当前 Rust `RowTableSegment` 数据模型下的简化假设；扩展 segment 表示时必须同步审视。

## 依赖与调用关系

上游模块关系如下：

- `pkg/executor/join/lib.rs` 导出模块，并仅在 `cfg(test)` 下装配 `hash_join_spill_helper_test.rs`。
- `pkg/executor/join/hash_join_spill.rs::HashJoinSpillAction` 是当前明确的生产上游，使用状态、等待、限额和开关 API；RustCodeGraph 对 `spill_row_tables`、`spill_probe_chunk`、`prepare_for_restoring` 没有返回生产 caller。
- `pkg/executor/join/inner_join_spill_test.rs`、`outer_join_spill_test.rs` 和 `hash_join_spill_helper_test.rs` 直接串联 build spill、probe spill、恢复、reset 与轮次上限，构成完整数据路径的当前可执行证据。
- `pkg/executor/join/hash_join_v2.rs::HashJoinV2Exec` 使用它自身在约 1598 行定义的 `HashJoinSpillHelper`；两个类型同名但模块不同，不能据此声称本文件已接入 v2 执行主链。

下游依赖全部在同 crate 内：`RowTable`/`RowTableSegment` 提供 build 侧 segment，`EncodedRow::bytes` 被复制进 `SpilledBuildRow`；`joiner::Row` 和 `row_table_builder::Chunk` 表示 probe 侧行与批次。同步原语来自标准库 `Mutex`、`Condvar` 与原子类型。`pkg/executor/join/Cargo.toml` 声明 crate 入口为 `lib.rs`，一般依赖只有两个内部 crate；大量 executor 依赖位于 `cfg(windows)` 表中，而本文件自身没有第三方磁盘、异步运行时或序列化依赖。

## 错误处理与边界

- `new` 拒绝零分区或零并发；`set_partition_spilled`、`spill_build_segments`、`spill_probe_chunk` 拒绝越界索引；分区内存数组长度不匹配也返回 `Err(String)`。
- 可恢复的锁中毒在部分写入路径映射为 `"spill helper poisoned"`，但多个只读/清理接口用 `expect`，会直接 panic。调用方不能把所有错误都视为统一的 `Result` 路径。
- `spill_build_segments` 假定每个 segment 的 `hash_values` 至少与 `rows` 等长，使用相同下标读取；若上游破坏不变量会 panic。`valid_key_count` 超过行数时被截断。
- `spill_probe_chunk` 接受空 chunk并创建一个零字节 chunk；但 `prepare_for_restoring` 只在 build 侧非空时建恢复项，所以仅有空 probe 的分区不会产生伪恢复任务，`outer_join_spill_test.rs::outer_spill_empty_probe_chunk_round_trips` 覆盖此边界。
- `prepare_for_restoring` 在轮次越界时不改变任何状态。成功时只把有 build 数据的分区压栈；若调用者只写 probe 而没有 build，probe 数据会在本轮矩阵重置时丢弃，这是接口前置条件而非自动修复。
- 负内存限额在 `MemoryTracker::check_exceed` 中表示无限制；但分区选择直接使用 `bytes_limit * 0.5`，正常流程应保证 OOM action 仅在有限正配额下触发。
- `SpillDisk::close` 清空内容并禁止后续追加；`HashJoinSpillHelper::close` 不重建槽位，也不将 tracker 归零，因此 helper 关闭后不应复用。

## 并发与资源生命周期

所有跨线程复合状态集中在一个 `Mutex<SpillState>` 中；布尔开关、计数和限额使用原子变量。`set_in_spilling` 与 `set_not_spilled` 在锁内改变状态，后者调用 `notify_all`；`wait_while_spilling` 使用 `while` 循环抵抗虚假唤醒。外部 `HashJoinSpillAction::action_lock` 保证多个并发 OOM action 中只有一个能从 `NotSpilled` 成功置为 `NeedSpill`，对应测试会让 16 个线程同时竞争并断言仅一次成功。

写 build/probe 数据时整个转换和追加过程最终在 `SpillState` 锁下完成，当前 Rust 实现没有像 Go `spillRowTableImpl` 那样按 worker 并行写盘；因此没有 worker 间的数据竞争，但锁持有时间会随 chunk 大小增长。`MemoryTracker` 和 `disk_tracker` 在状态锁外以原子方式更新。

资源阶段为：构造空矩阵 -> 多次惰性追加 -> `prepare_for_restoring` 将拥有的数据克隆进恢复栈并重置本轮矩阵 -> `pop_restore_partition` 转移恢复项 -> `close` 清空当前矩阵和栈。由于 `RestorePartition` 保存的是内存 `Vec` 而非文件句柄，`close` 主要释放容器数据；当前没有临时文件删除、I/O flush 或异步任务 join。

## 与 Go 版本的对应关系

Go 权威对照为 `pkg/executor/join/hash_join_spill_helper.go`。Rust 保留了三态状态机、0.5 释放目标、优先 spill 大分区、build 三元组语义、worker/partition 二维布局、probe 配对、restore stack、最大轮次限制以及测试观察接口。名称按 Rust 风格改为 PascalCase/snake_case，并将依赖执行器内部字段的构造改为显式参数。

当前 Rust 与 Go 的重要差异如下：

- Go 使用 `chunk.DataInDiskByChunks` 和 disk tracker 处理真实临时文件；Rust `SpillDisk` 只是内存 `Vec<Vec<T>>`，所以本文只能称其为落盘抽象或暂存，不能宣称已完成真实磁盘 I/O。
- Go helper 持有 `HashJoinV2Exec`，从执行器获取 worker、partition 内存、SQL killer、字段类型、tracker 与 `maxSpillRound`；Rust helper 与执行器解耦，这些信息由参数和调用方提供，也未实现 SQL kill 检查、failpoint、字段类型 chunk、rehash buffer 或 panic recovery channel。
- Go `spillRowTableImpl` 为每个 build worker 启动并发任务；Rust 顺序遍历 worker。Go 写入固定容量临时 chunk，Rust 每个 `spill_build_segments` 调用汇成一个 `Vec<SpilledBuildRow>`，`SPILL_CHUNK_SIZE` 尚未接线。
- Go 有精确 `validJoinKeyPos`；Rust 仅有 `valid_key_count`，把前 N 行视为有效。Go restore stack 保存磁盘对象所有权，Rust聚合并克隆内存 chunk。
- Go 的 `setNotSpilled` 依赖调用方 defer 的 `cond.Broadcast`；Rust 把 `notify_all` 内聚到 `set_not_spilled`，并让 `spill_row_tables`/`spill_remaining_rows` 在成功和错误后都显式调用它。
- Go 的完整 helper 由 `HashJoinV2Exec` 构造并参与主流程；当前 Rust v2 主执行器仍使用另一同名简化 helper。因此这是部分语义移植和可测试基础设施，不是 Go 路径的一比一运行时替换。

## 扩展指南

要接入真实 Hash Join v2，首先消除或清晰命名 `hash_join_v2.rs` 中的同名 helper，并在执行器生命周期中统一拥有本文件类型；接线点至少包括 OOM action 注册、build worker row table 转移、probe 分区写出、restore 循环和 `close`。修改必须保持 `NeedSpill/InSpilling/NotSpilled` 的原子转换与失败后唤醒不变量，并在独立测试文件中覆盖生产调用链。

若把 `SpillDisk` 替换为真实磁盘实现，应在 `spill_build_segments`、`spill_probe_chunk`、`prepare_for_restoring` 和 `close` 中明确所有权转移、flush/close 错误、临时文件删除与 tracker 回退；避免在状态锁内执行长时间 I/O。必须保留 Go 的三列 build 编码语义，并同步 `inner_join_spill_test.rs`、`outer_join_spill_test.rs`，新增 I/O 失败和清理回归测试。

若调整分区选择，修改入口是 `choose_partitions_to_spill`。需维持“已 spill 分区优先计入”“占用降序”“相等时确定性排序”和“达到 0.5 目标即停止”，并补充长度错误、已有分区已足够、全部分区仍不足、零/负用量等测试。若 `RowTableSegment` 获得有效键位置集合，应同步改写 `generate_spilled_valid_join_key`，不可继续用前 N 行近似。

轮次或恢复策略变更集中在 `prepare_for_restoring`、`RestorePartition` 和 `RestoreStack`；需验证越界不改状态、空 build 不建任务、多 worker 同分区聚合、LIFO 次序和 respill 轮次。测试逻辑继续放在独立的 `*_test.rs` 文件，不能内嵌到本生产源文件。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标文件被索引为 1296 行、72 个符号。
- RustCodeGraph 源码与符号：`HashJoinSpillHelper`（本文件约 814 行）、`spill_row_tables`（1128 行）、`prepare_for_restoring`（1199 行）、`spill_probe_chunk`（1086 行）、`choose_partitions_to_spill`（990 行）。`callees spill_row_tables` 返回 `set_in_spilling`、`choose_partitions_to_spill`、`spill_selected_partitions`、`set_not_spilled`；`callees prepare_for_restoring` 返回轮次错误常量、`RestorePartition`、`RestoreStack::push` 和 chunk 读取；这些入口未返回生产 caller。
- 已读生产与装配文件：`pkg/executor/join/hash_join_spill_helper.rs`、`pkg/executor/join/hash_join_spill.rs`、`pkg/executor/join/hash_join_v2.rs`、`pkg/executor/join/lib.rs`、`pkg/executor/join/Cargo.toml`。
- Go 对照：完整读取 `pkg/executor/join/hash_join_spill_helper.go`，核对构造、状态、选择算法、build/probe 写出、并发 worker、reset、restore stack、轮次和关闭语义。
- Rust 独立测试：`pkg/executor/join/hash_join_spill_helper_test.rs` 验证只 spill 已标记的残留分区；`hash_join_spill_test.rs` 验证阈值、fallback 和 16 线程单次置位；`inner_join_spill_test.rs` 验证最大分区选择、build/probe 写出、恢复与 round 限制；`outer_join_spill_test.rs` 验证两侧内容完整、空 probe 和 reset 边界。
- 人工复核结论：本文明确区分注释迁移草稿与可执行实现、内存模拟与真实磁盘、状态机已接线部分与完整 spill/restore 尚未接入 v2 主执行器的部分；没有把 Go 独有的 SQL killer、failpoint、rehash 或并行写盘描述为 Rust 已支持。
