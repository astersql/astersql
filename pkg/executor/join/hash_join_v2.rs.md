# `pkg/executor/join/hash_join_v2.rs`

## 文件定位

本文件属于 `astersql-executor-join` crate；crate 根由 `pkg/executor/join/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/join/lib.rs` 通过 `pub mod hash_join_v2` 将它公开为连接执行器的一条实现路径。它实现分区式 Hash Join v2：把 build 侧行按连接键散列到分区、建立 `HashTableV2`，再用 probe 侧行查表并交给 `Joiner` 生成各种连接语义的结果；达到内存阈值时，还会把部分分区暂存在 spill helper 中并在本轮末恢复。

需要特别区分文件中的两层内容：第 13–1571 行是用注释保留的 Go 迁移草稿，不参与 Rust 编译；真正的 Rust 实现从导入 `crate::hash_join_base` 的第 1572 行开始。本文描述的是后者，并只把前者作为 Go 设计对照证据，不能把注释中的 goroutine、channel、failpoint 或磁盘临时文件当成当前 Rust 已实现能力。

上层可直接构造 `HashJoinCtxV2`、`Joiner` 和 `HashJoinV2Exec`，再调用 `next` 或 `execute_all`。仓库内的直接用例见 `pkg/executor/join/hash_join_v2_test.rs`、`pkg/executor/join/inner_join_spill_test.rs`、`pkg/executor/join/right_outer_join_probe_test.rs`、`pkg/executor/test/jointest/hashjoin/hash_join_test.rs`、`pkg/executor/join_pkg_test.rs` 与 `pkg/executor/benchmark_test.rs`。当前未发现生产 builder 把 SQL 物理计划接到这个 Rust 类型上的直接引用，因此它已具备可独立执行和测试的实现，但完整 SQL 主链接线不应据此文档宣称已经完成。

## 核心职责

1. `HashJoinCtxV2` 校验并保存 join 类型、两侧键列、build side、null-aware 标志、并发提示、结果 chunk 大小和可选内存上限，同时把并发提示规范化为最多 16 的二次幂分区数。
2. `BuildWorkerV2::split_partition_and_append` 对 build 行计算键哈希，用哈希高位选择分区；`HashTableContext::build` 再将每个分区编码为 `RowTable` 并组合成 `HashTableV2`。
3. `ProbeWorkerV2::probe_row` 在对应分区中先按哈希取候选，再以 `keys_equal` 做真实等值核验，防止哈希碰撞产生假匹配；匹配结果由 `Joiner` 按 inner、outer、semi、anti 及 null-aware 语义处理。
4. `HashJoinV2Exec` 管理 open、惰性 prepare、分批返回、耗尽和 close 状态；首次 `next` 会完成 build、probe、spill 恢复和未匹配 build 行扫描，之后按 `max_chunk_size` 切片输出。
5. `HashJoinSpillHelper` 记录因内存上限被移出的 build 分区、相应 probe 行及字节/恢复统计。当前实现使用内存中的 `Vec`，并非 Go 版 `chunk.DataInDiskByChunks` 的真实磁盘 spill。
6. 可选的 `HashStateRuntimeStats` 记录建表行数，并在 `close` 时注册进 `RuntimeStatsColl`；`HashJoinRuntimeStatsV2` 记录 build/probe 耗时和 spill 统计。

## 主要符号

- `BuildTask`：描述一个建表区间，包含 `partition_index`、`segment_start_index` 和 `segment_end_index`。辅助函数 `create_build_tasks` 当前按哈希表总行数切段，但把每个任务的 `partition_index` 固定为 0；它尚未接入 `HashJoinV2Exec` 的实际构建流程。
- `HashJoinSpillHelper`：保存 `spilled_build: Vec<Option<Vec<Row>>>`、每分区的 `spilled_probe`、逐轮字节统计、恢复统计、轮次和关闭标志。`spill_build`、`spill_probe`、`restore_partition`、`close` 构成其生命周期。
- `HashTableContext`：同时持有编码后的 `HashTableV2`、可用于产出结果的 `original_rows`、`JoinTableMeta` 和估算内存量。`lookup` 返回 `RowPos`，`original_row` 将位置映射回原始行，`mark_used`/`unmatched_rows` 支撑 outer join，`reset`/`clear_partition` 负责释放内容。
- `HashJoinCtxV2`：执行配置和共享基础状态。`new` 验证键数量非空且相等、并发度和 chunk 大小非零；`setup_partition_info` 可在每次 open 时重算分区参数。
- `BuildWorkerV2`：通过 `BuildWorkerBase::run_guarded` 包裹分区操作；当前执行器只创建 worker 0，`concurrency` 用于分区数和统计，不会实际生成多个线程。
- `ProbeWorkerV2`：通过 `ProbeWorkerBase::run_guarded` 执行探测；`probe_row` 是匹配语义的核心。outer build side 会调用 `try_to_match_outers` 并标记实际匹配行；其他路径调用 `try_to_match_inners`，未匹配时由 `on_miss_match` 决定输出。
- `ProbeSideTupleFetcherV2`：拥有 probe chunks 和游标，`next_chunk` 顺序取块，`reset` 支持重复 open。`can_skip_probe_if_hash_table_is_empty` 由执行器在 probe 前设置。
- `HashJoinV2Exec`：公开构造、统计接线、输入替换、生命周期和取数 API。重要入口为 `new`、`with_runtime_stats`、`set_build_chunks`、`open`、`next`、`execute_all` 和 `close`；核心内部阶段为 `fetch_and_build_hash_table`、`fetch_and_probe_hash_table`、`restore_and_probe`、`start_build_and_probe`。
- 分区/编码辅助：`gen_hash_join_partition_number`、`get_partition_mask_offset`、`generate_partition_index`、`rehash`、`build_row_table`、`infer_field_types`、`hash_row`、`encode_value`、`hash_bytes`、`keys_equal`、`row_size` 和 `grow_add`。

## 执行流程

1. `HashJoinV2Exec::new` 先确认 `context.join_type == joiner.join_type()`，保存两侧输入，创建与分区数等长的 spill 容器，并把状态置为 `ExecutorState::Created`。
2. 显式 `open`，或首次 `next` 隐式调用 `open`。`open` 重置 `HashJoinContextBase`、分区信息、probe 游标、spill helper、输出和统计，将状态改为 `Open`。这也是同一执行器重复运行的复用边界。
3. 首次取数时 `next -> start_build_and_probe -> fetch_and_build_hash_table`。build worker 遍历 build chunks，以 `hash_row` 和 `generate_partition_index` 分区；随后根据首个 build 行推断字段类型，构造 `JoinTableMeta`，把每个分区编码为 `RowTable` 并建立 `HashTableV2`。
4. 若设置了 `memory_limit` 且估算内存超限，`fetch_and_build_hash_table` 按分区内存从大到小选择候选：从 `original_rows` 取出行、清除对应哈希表分区，再交给 `spill_build`，直至估算量不超限。至少 spill 一个非空分区时，`HashJoinContextBase::set_spilled` 被调用。
5. `fetch_and_probe_hash_table` 等待 build 完成。若内存中哈希表为空、当前 join 类型允许空表跳过、且没有 spill build 分区，则完全跳过 probe；否则逐 chunk、逐行计算分区。内存内分区进入 `probe_row`，已 spill 分区的 probe 行则由 `spill_probe` 暂存。
6. `probe_row` 先按哈希取 `RowPos`，再比较 build/probe 键值。SQL NULL 在 `keys_equal` 中永不相等；null-aware 路径还综合 probe/build 是否含 NULL，向 `Joiner` 传递 `NaajType` 或 miss 的 `has_null`。需要后续扫描的 outer build 行在匹配时通过原子 used flag 标记。
7. `restore_and_probe` 逐个取回 spill 分区，单独重建只含该分区的哈希表并重放已缓存的 probe 行。若需要扫描 build 侧未匹配行，会在每个恢复分区完成后输出；内存内主表则在恢复全部结束后扫描。
8. `collect_spill_stats` 汇总轮次、spill 字节和恢复字节；`prepared` 置为 true。`next` 从完整输出向量中最多返回 `max_chunk_size` 行，全部读完后返回默认空结果并把状态置为 `Exhausted`。`execute_all` 循环调用 `next` 直至空批次。
9. `close` 注册可选 hash-state 统计、取消共享上下文、清空哈希表和输出、关闭 spill helper，并把状态置为 `Closed`。关闭后直接 `next` 返回错误；再次显式 `open` 可复用执行器。

## 数据与状态

- 生命周期状态由 `ExecutorState::{Created, Open, Exhausted, Closed}` 表示。`prepared` 表示完整 build/probe 流水线已经物化到 `output`，`cursor` 表示下一个结果位置，`in_restore` 只在恢复阶段为 true。
- 分区数由 `gen_hash_join_partition_number(concurrency)` 计算：从 1 倍增到不小于 hint，最高停在 16，因此始终为二次幂。`get_partition_mask_offset` 返回 `64 - trailing_zeros(partition_number)`，`generate_partition_index` 取哈希高位；单分区偏移为 64，函数显式返回分区 0。
- build/probe 键分别由 `build_key_indices` 与 `probe_key_indices` 指定。`hash_row` 对所有键序列化后做 FNV-1a 形状的 wrapping 哈希；可变长键存在时，它还在每段前插入长度。`keys_equal` 仍做逐值比较，因此哈希只用于缩小候选集。
- `infer_field_types` 仅根据首个 build 行推断类型；空 build 输入会得到空类型表，而非从上层 schema 获取。键索引越界会在类型提取或哈希时返回字符串错误。
- `HashTableContext` 同时保留编码行与 `original_rows`。outer join 的 used 标记存储在编码行中，`unmatched_rows` 通过哈希表迭代器扫描并映射回原始行。
- spill 数据当前仍驻留内存：`spilled_build[partition]` 至多一个 build 行向量，`spilled_probe[partition]` 收集该分区所有 probe 行。`spilled_bytes` 与 `restored_bytes` 当前都累计在 round 0；`rounds` 记录每次建表选择的 spill 分区数量，并未真正执行 Go 版多轮 rehash。
- `HashJoinRuntimeStatsV2` 记录 `fetch_and_build`、`fetch_and_probe`、`probe`、最大 build 时长、并发值及 spill 统计；`HashStateRuntimeStats` 通过内部原子计数累计建表行数，并在 close 时合并进集合。

## 依赖与调用关系

上游方面，`pkg/executor/join/lib.rs` 公开本模块。已核实的 Rust 调用者以测试和基准为主：`hash_join_v2_test.rs` 构造执行器检查空表规则；`inner_join_spill_test.rs` 调用 `execute_all/open/close/set_build_chunks/with_runtime_stats`；`right_outer_join_probe_test.rs` 验证 outer 语义；`pkg/executor/test/jointest/hashjoin/hash_join_test.rs` 用同一输入比较 v1/v2；`pkg/executor/join_pkg_test.rs` 与 `pkg/executor/benchmark_test.rs` 提供包级和性能用例。RustCodeGraph 的文件关系也把基准和 join 测试列为使用者，但其 `callers/callees` 命令在本次检查中超时，未能提供更细的图边。

下游方面：

- `hash_join_base` 提供 `HashJoinContextBase`、worker 基类和 `HashJoinWorkerResult`，负责 build 完成/失败/取消状态以及受保护执行。
- `hash_table_v2`、`join_row_table`、`row_table_builder` 和 `join_table_meta` 共同完成行编码、分区哈希索引、位置迭代、内存估算和 used flag。
- `joiner` 决定连接类型的匹配、other condition、缺失行和 null-aware 输出语义。
- `hash_join_stats` 与 `astersql-util-execdetails` 提供执行时间、spill 和 typed hash-state 统计；`RuntimeStatsColl` 是 `Cargo.toml` 中直接依赖 `astersql-util-execdetails` 的使用点。
- 标准库的 `Arc<Mutex<RuntimeStatsColl>>` 只用于统计集合共享；`Instant` 用于阶段计时。

`Cargo.toml` 将 Go 对照包声明为 `pkg/executor/join`。大量 Windows 条件依赖用于整个 join crate 的其他模块；本文件实际编译部分直接使用的跨 crate 依赖仅见 `astersql-util-execdetails`，其余主要依赖都是同 crate 模块。

## 错误处理与边界

- 构造期拒绝空键列表、两侧键数不一致、零并发、零 chunk 大小，以及 context/joiner 的 join 类型不一致。
- build 类型推断、行哈希和编码会把键列越界、row table 构造失败等问题作为 `Result<_, String>` 向上传播。`next` 捕获准备阶段错误后调用 `context.base.fail(error.clone())`，使共享 build 状态进入失败态。
- `fetch_and_probe_hash_table` 在 build 未完成时通过 `wait_for_build_side` 取失败或取消信息；哈希表上下文缺失会返回 `"V2 hash table missing"`。
- `BuildWorkerBase::run_guarded` 和 `ProbeWorkerBase::run_guarded` 是当前 worker 阶段的保护边界；具体 panic/错误转换语义由 `hash_join_base.rs` 定义。本文件自身没有 Go 版的显式 `recover`、错误 channel 和 failpoint。
- `next` 在 `Closed` 状态返回 `"hash join V2 is closed"`；在 `Created` 状态会自动 open。`execute_all` 以空 rows 作为结束信号，因此调用者不应把合法非终止批次表示为空。
- SQL NULL 的等值行为由 `keys_equal` 明确为不匹配；null-aware anti 路径在 `probe_row` 中单独计算 build/probe NULL 状态。测试覆盖了部分语义，但不是 Go 测试矩阵的完整等价移植。
- 内存限制是对 `HashTableV2::total_memory_usage/partition_memory_usage` 的估算控制；从大分区依次移除后使用 `saturating_sub`，不会出现负数，但 spill 容器自身仍占内存，故它不是严格的进程内存上限。
- `close` 对运行时统计锁使用 `expect("runtime stats lock poisoned")`；锁中毒会 panic，而不是返回可处理错误。

## 并发与资源生命周期

当前 Rust 数据路径是同步且顺序的：`start_build_and_probe` 在调用 `next` 的线程上依次 build、probe、restore；执行器只实例化 worker 0，没有线程池、channel、wait group 或异步取数。`concurrency` 目前影响分区数、`max_spill_round` 和统计字段，而不是实际并行度。文档或扩展代码不能因类型名中含 `Worker` 就假定并行执行已经接线。

共享/并发安全的局部机制有两处：编码行的 used flag 由 `JoinTableMeta::set_used_flag` 与 `is_current_row_used_atomic` 原子访问，以保留未来并行 probe/scan 的基础；runtime stats 集合通过 `Arc<Mutex<_>>` 共享。执行器本体需要 `&mut self` 推进，未提供跨线程并发调用协议。

资源生命周期为 `new -> open -> next/execute_all -> close`。`open` 丢弃上轮表、spill 和输出，重置 fetcher/统计；`close` 取消 base 状态、清表、清空输出并关闭 spill helper。测试证明显式 `close` 后 `open` 可重复执行；若要替换 build 输入，应在下一次 open 前调用 `set_build_chunks`。`HashTableContext::reset` 和 `HashJoinSpillHelper::close` 是主要释放点，但 Rust 容器真正内存回收仍依赖所有权离开或容量重分配。

## 与 Go 版本的对应关系

`pkg/executor/join/hash_join_v2.go` 是直接语义对照。Rust 保留了 `HashJoinCtxV2`、`BuildWorkerV2`、`ProbeWorkerV2`、`ProbeSideTupleFetcherV2`、`HashJoinV2Exec`、分区辅助函数，以及 open/build/probe/restore/close 的主阶段；空哈希表跳过矩阵与 Go `canSkipProbeIfHashTableIsEmpty` 一致，`hash_join_v2_test.rs` 对该矩阵和单分区 mask 进行了显式回归。

当前 Rust 与 Go 的重要差异如下：

- Go 使用真实 executor children、并行 build/probe worker、goroutine、channel、wait group、cancel channel 和 panic recovery；Rust 当前输入是预先装入的 `Vec<Chunk>`，阶段顺序执行，worker 只提供逻辑边界。
- Go 的 spill helper 使用临时存储、磁盘 tracker、OOM action、多轮恢复与 rehash；Rust 当前把 spill 行继续保存在内存 Vec 中，单次恢复，不使用 `max_spill_round` 和 `rehash` 驱动后续轮次。
- Go 从 executor schema/FieldType、build/probe filter、other condition 和列使用信息构造 table meta；Rust 从首个 build 行推断字段类型，过滤和投影主要委托给已经构造好的 `Joiner`，context 中没有对应的完整字段集合。
- Go `Next` 首次调用启动异步流水线并从结果 channel 消费；Rust 首次 `next` 会先物化全部结果到 `output`，再按 chunk 大小返回，因此延迟、峰值内存和背压行为不同。
- Go 有 failpoint、SQL killer、trace region、磁盘释放和更完整的 runtime stats；Rust 当前只有字符串错误、base guard、阶段耗时和 hash-state 行数统计。
- Go 的 `buildTask` 真正驱动并行哈希表构建；Rust 的 `BuildTask/create_build_tasks/check_balance` 是公开辅助能力，但尚未连入执行器主流程。

所以本文件可视为保持 Go 核心结果语义的可运行 Rust 实现，而不是 Go v2 执行/资源模型的完整等价移植。新增能力时应优先对照 Go 同名方法，但不得把 Go 代码直接视为 Rust 当前行为。

## 扩展指南

- 增加真实并行 build/probe：接入点是 `BuildWorkerV2`、`ProbeWorkerV2` 与 `HashJoinV2Exec::start_build_and_probe`。必须定义 output 汇合顺序、错误/取消传播、used flag 并发安全和 close 等待规则，并同步独立测试而不是把测试写进源文件。
- 增加真实磁盘 spill/多轮恢复：替换或扩展 `HashJoinSpillHelper`，让 `memory_limit` 触发磁盘介质、round 上限和 `rehash`；需保持每轮统计、清理失败路径与重复 open/close 一致。重点同步 `inner_join_spill_test.rs`，并参考 Go 的 `inner_join_spill_test.go`、`outer_join_spill_test.go` 与 `hash_join_spill_helper_test.go`。
- 扩展类型/排序规则：修改 `infer_field_types`、`encode_value`、`hash_row` 与 `keys_equal` 时，哈希编码和等值比较必须同契约，尤其要覆盖复合可变长键、NULL、浮点、文本 collation、不同数值类型兼容和碰撞。
- 扩展 join 类型或 null-aware 行为：主要修改 `ProbeWorkerV2::probe_row`、`can_skip_probe_if_hash_table_is_empty` 和可能的未匹配扫描条件；应同步 `hash_join_v2_test.rs`、各 probe 独立测试以及 `pkg/executor/test/jointest/hashjoin/hash_join_test.rs` 的 v1/v2 对照用例。
- 改变 outer join：必须同时审查 `HashTableContext::mark_used/unmatched_rows/original_row` 和恢复分区扫描，避免重复或遗漏未匹配 build 行。`right_outer_join_probe_test.rs` 已提供直接回归入口。
- 改变生命周期或统计：审查 `open`、`next`、`close`、`with_runtime_stats` 和失败后的 `context.base.fail`。重复 open 的累计行数、失败使统计 invalid、锁注册时机由 `inner_join_spill_test.rs` 固定。
- 将实现接入完整 SQL executor builder 时，应新增明确的生产调用证据，并验证 schema、左右 build side、filters、session tracker、取消和物理计划版本选择；当前仅有测试/基准构造，不能仅复用 `new` 就认为接线完成。
- 性能风险主要是 `original_row` 为计算 segment 偏移反复遍历全表、每行 probe 都扫描 build 表判断是否有 NULL、首次取数物化全部输出，以及内存 spill 并不释放进程占用。优化这些位置必须先固定结果语义和统计契约。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/join/hash_join_v2.rs` 确认目标文件已索引并含 77 个符号；`query HashJoin`、`query HashJoinV2`、`query Probe` 找到本文件的 `HashJoinCtxV2`、`HashJoinSpillHelper`、`HashJoinV2Exec` 与 `probe_row`。按文件 `node --file ... --offset/--limit` 阅读了全文件。精确 `callers/callees` 查询两次在 30 秒内无返回，因此调用边另由下列源码引用核验。
- 源码与装配：`pkg/executor/join/hash_join_v2.rs`（尤其实际 Rust 的 1572–2494 行）、`pkg/executor/join/lib.rs`、`pkg/executor/join/Cargo.toml`。
- 下游直接实现：`pkg/executor/join/hash_join_base.rs`、`hash_table_v2.rs`、`join_row_table.rs`、`join_table_meta.rs`、`row_table_builder.rs`、`joiner.rs`、`hash_join_stats.rs`（由本文件导入和调用符号确认）。
- Rust 测试与调用者：`pkg/executor/join/hash_join_v2_test.rs`、`pkg/executor/join/inner_join_spill_test.rs`、`pkg/executor/join/right_outer_join_probe_test.rs`、`pkg/executor/test/jointest/hashjoin/hash_join_test.rs`、`pkg/executor/join_pkg_test.rs`、`pkg/executor/benchmark_test.rs`。它们分别提供空表/分区边界、spill 与复用、outer 未匹配、v1/v2 对照、包级及基准构造证据。
- Go 对照：`pkg/executor/join/hash_join_v2.go` 的同名类型、`Open/Close/Next`、build/probe/restore、分区和 task 函数；相关 Go 回归入口包括 `inner_join_spill_test.go`、`outer_join_spill_test.go`、各 join probe 测试和 `hash_table_v2_test.go`。
- 本任务是只新增说明文档的静态分析，按计划不运行 Cargo。结构验收使用任务指定的 `test -f` 加 11 个固定二级标题计数命令；最终退出码另行记录。
