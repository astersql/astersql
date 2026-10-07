# `pkg/executor/join/hash_join_stats.rs`

## 文件定位

本文件属于 `astersql-executor-join` crate，由 `pkg/executor/join/lib.rs` 以 `pub mod hash_join_stats` 暴露。它位于 Hash Join 执行链的观测层：`hash_join_v1.rs` 和 `hash_join_v2.rs` 执行 build、probe 与 spill，本文对应的类型保存耗时、冲突、并发度和落盘统计，并通过 `Display` 生成与 Go 版运行时统计相容的诊断文本。

文件第 16～422 行是块注释中的旧迁移草稿，不参与编译；实际 Rust 实现从 `use std::sync::atomic::{AtomicI64, Ordering}` 开始。理解或修改行为时应以 `HashJoinRuntimeStats`、`HashJoinRuntimeStatsV2` 等编译态符号为准，不能把注释中的 Go 风格伪代码视为已接线实现。

`pkg/executor/join/Cargo.toml` 声明该 crate 的入口为 `lib.rs`，普通依赖只有执行器内部接口和 `astersql-util-execdetails`；本文件自身只直接使用标准库的 `Duration`、`AtomicI64` 与格式化能力，没有 I/O、网络或存储依赖。

## 核心职责

1. 用 `HashJoinRuntimeStats` 保存 v1 Hash Join 的 build/probe 总耗时、纯执行耗时、冲突数、并发度以及最慢 worker 耗时。
2. 用 `HashJoinRuntimeStatsV2` 保存 v2 更细的 partition/build/probe、当前 spill 轮与跨轮最大值，以及 `SpillStats`。
3. 用 `merge`、`reset`、`reset_round` 和原子最大值更新维护统计聚合语义。
4. 用两个 `Display` 实现生成 `EXPLAIN ANALYZE` 风格的文本；v2 还根据 `is_hash_join_ga` 选择详细或兼容格式。
5. 用 `write_spilled_partition_num_stats`、`write_bytes_stats` 和时长格式化辅助函数稳定输出比例、GiB 与 Go 风格 duration。

本文件只定义统计容器和格式化规则，不负责调度 Hash Join、注册统计到 statement context，也不直接触发 spill。生产采集发生在 `hash_join_v1.rs`、`hash_join_v2.rs`；本文件的 `tp()` 目前只是返回本地常量，没有在这里实现 `astersql-util-execdetails` 的运行时统计 trait。

## 主要符号

- `write_spilled_partition_num_stats(output, partition_num, rounds)`：输出 `[本轮落盘分区数/本轮候选分区总数 ...]`。首轮分母是 `partition_num`，后续分母是“上一轮落盘分区数 × partition_num”；空切片输出 `[]`。
- `write_bytes_stats(output, bytes)`：把每个字节数除以 `1_073_741_824.0`，以两位小数输出 GiB 列表；它只格式化，不检查负数或溢出来源。
- `HashJoinRuntimeStats`：v1 公共统计结构。`TYPE = 4`，`tp()` 返回该值；自定义 `Clone` 负责读取原子字段，`merge()` 累加计数并保留最大 `max_fetch_and_probe_ns`。
- `HashStatistic`：独立的 build 耗时与 probe 冲突计数，供 `hash_join_v1.rs` 中 `NestedLoopApplyExec`、`JoinRuntimeStats` 等复用；`reset()` 恢复默认值。
- `SpillStats`：同时含 Go 对齐字段（`round`、`partition_num`、各类 `*_per_round`）和 Rust 执行器当前采集字段（`spilled_partition_num`、`spilled_bytes`、`restored_bytes`）；`reset()` 清空全部向量及轮次。
- `HashJoinRuntimeStatsV2`：v2 公共统计结构。`TYPE = 5`；`set_max_worker_fetch_and_probe()` 原子更新最大 worker 时间；`reset()` 清零耗时但保留 `spill`、`concurrency` 和 `is_hash_join_ga`；`reset_round()` 把当前轮最大耗时累加进跨轮字段；`merge()` 对总量求和、对最大值取 max。
- `set_max_value(value, candidate)`：使用 `compare_exchange_weak` 的 CAS 循环，只有候选值更大时才更新 `AtomicI64`。
- `duration_to_nanos` / `nanos_to_duration`：在 `Duration` 与有符号纳秒之间转换；前者钳制到 `i64::MAX`，后者把负值钳制为零。
- `format_duration` / `format_go_duration` / `format_decimal_duration`：内部格式化链，按秒、毫秒、微秒量级舍入，并输出 Go duration 风格的 `h/m/s/ms/µs/ns` 文本。

## 执行流程

v1 的生产路径是：`HashJoinV1Exec::new` 或 `new_full_outer` 创建默认 `HashJoinRuntimeStats`；`open()` 完成 build 后累加 `fetch_and_build` 并写入 `concurrency`；`produce_all()` 完成 probe 后累加 `fetch_and_probe` 和 `probe`。消费者调用 `to_string()` 时，`Display` 先在 `fetch_and_build > 0` 时输出 build 段，再在 `probe > 0` 时输出 probe 段，并仅在冲突数大于零时追加 `probe_collision`。

v2 的生产路径是：`HashJoinV2Exec::new` 创建默认统计；每次 `open()` 调用 `reset()`，随后恢复执行器的并发度；`fetch_and_build_hash_table()` 累加 build 总时间并更新 `max_build_hash_table`；`fetch_and_probe_hash_table()` 累加 probe 时间；准备完成时 `collect_spill_stats()` 把 helper 的 `rounds`、`spilled_bytes`、`restored_bytes` 复制进 Rust 自有的 spill 向量。格式化时，build/probe 段分别受其计数是否大于零控制，GA 模式显示总量和最大值的细分，非 GA 模式显示兼容摘要；只有 `spill.round > 0` 才输出 Go 风格的按轮 spill 段。

聚合路径不修改接收者的 `concurrency`。v1 `merge()` 累加五个总量字段并对 worker 时间取最大值；v2 `merge()` 累加 build、partition、probe 和冲突总量，对相应 max 字段取最大值，但不合并 `worker_fetch_and_probe`、`spill`、当前轮字段、并发度或 GA 标志。这一选择由 `hash_join_stats_test.rs` 和 `join_stats_test.rs` 明确覆盖。

## 数据与状态

所有时间在公开结构中用 `Duration` 表示，跨线程更新的最大 worker 时间单独保存为纳秒制 `AtomicI64`。普通计数和 `Duration` 字段不是原子类型，因此调用方若跨线程共享并写入整个统计对象，必须在外层串行化；本文件仅为“最大值”这一窄操作提供无锁更新。

`reset()` 的保留边界很重要。`HashJoinRuntimeStatsV2::reset()` 清空本次执行的计时、冲突和当前轮字段，却不清空 `spill`、`concurrency` 或 `is_hash_join_ga`；执行器 `open()` 会在 reset 后覆盖 `concurrency`，但当前 Rust 生产路径没有在同一点重设 GA 标志或全部 Go 风格 spill 字段。`SpillStats::reset()` 才会彻底清空 spill 状态，但生产路径未在本文件外直接调用它。

`reset_round()` 名称中的 “reset” 不是丢弃：它把四个 `*_for_current_round` 加到跨轮累计最大耗时后再清零。因而 `max_partition_data`、`max_build_hash_table`、`max_probe` 在多轮 spill 情况下表示各轮最大耗时之和，而不是所有 worker 的总时间，也不是全局单一最大值。

`Clone for HashJoinRuntimeStatsV2` 有意复刻 Go 的选择性复制：它把 `worker_fetch_and_probe` 置零，把 `spill` 置默认值，把 `is_hash_join_ga` 置 `false`，其余已列字段按实现复制。调用方不能把 clone 当成完整快照。

## 依赖与调用关系

- 模块入口：`pkg/executor/join/lib.rs` 公开 `hash_join_stats`，并在 `cfg(test)` 下装配 `hash_join_stats_test.rs` 和 `join_stats_test.rs`。
- v1 上游：`pkg/executor/join/hash_join_v1.rs` 导入 `HashJoinRuntimeStats`、`HashStatistic`；`HashJoinV1Exec.stats` 在构造、build 和 probe 生命周期中被初始化和更新。
- v2 上游：`pkg/executor/join/hash_join_v2.rs` 导入 `HashJoinRuntimeStatsV2`；`HashJoinV2Exec.stats` 在 open、build、probe 和 spill 汇总阶段被更新。
- 下游依赖：实际实现只调用标准库原子操作、`Duration` 运算和字符串格式化；其类型标识在语义上对应 `pkg/util/execdetails/runtime_stats.rs` 中的 Hash Join 类型常量，但本文件使用自己的 `u8` 常量。
- Go 生产对照：`hash_join_v1.go`、`hash_join_v2.go` 和 `hash_table_v1.go` 负责同类统计的采集；`hash_join_stats.go` 定义 Go 容器、合并与字符串格式。

RustCodeGraph 将目标文件标记为被 `hash_join_v1.rs`、`hash_join_v2.rs`、相关 join 测试等文件使用；精确 `callers` 命令未返回边，因此上述具体接线又由 `rg` 对导入、字段和方法调用进行了核验。

## 错误处理与边界

本文件的公开方法均不返回业务错误。格式化写入 `String`，`fmt()` 只把最终字符串交给 formatter；内存分配失败等不可恢复情况不在显式错误契约内。

为避免不可信计数导致减法下溢，v1/v2 的 fetch 派生时间使用 `Duration::saturating_sub`。负的原子纳秒值通过 `nanos_to_duration` 映射为零，超大的 `Duration` 通过 `duration_to_nanos` 钳制为 `i64::MAX`。`write_spilled_partition_num_stats` 对空轮次安全输出 `[]`，但当轮次非空且 `partition_num == 0` 时只是输出零分母文本，并不做除法或报错。

`rounds[index - 1] * partition_num` 使用 `usize` 乘法；正常分区计数应远低于上限，但本函数没有显式溢出保护。字节统计接受 `i64`，负值会按负 GiB 输出。扩展采集接口时应在数据来源处维护非负、分区数有效和轮次数组一致等不变量。

格式化是否显示某段由 `fetch_and_build`、`probe` 和 `spill.round` 控制，而不是由其他字段是否非零控制；例如仅设置 `fetch_and_probe` 而 `probe == 0` 不会显示 probe 段。开头没有 build 段而有 probe 段时，输出仍以 `", probe:{...}"` 开始，这是 Go 对齐行为的一部分。

## 并发与资源生命周期

`set_max_value` 先以 `Acquire` 读取，再以 `AcqRel/Acquire` 的弱 CAS 循环竞争更新，可容忍伪失败并重读观察值。`Clone` 和 `Display` 对原子最大值使用 `Acquire`；`reset()` 使用 `Release` 清零；`reset_round()` 用 `fetch_add(AcqRel)` 汇入当前轮。这些原子操作只保护 `max_*_ns` 自身，不为同一结构中的其他字段建立整体一致快照。

统计对象由执行器拥有，生命周期与执行器实例一致。v2 每次 `open()` 重置执行计时，v1 当前活动实现则在 `open()` 中用 `+=` 累加 build 时间而没有先调用统计 reset；重复打开时的累计语义由执行器负责。`SpillStats` 中的 `Vec` 拥有数据，clone 会复制向量；没有文件句柄、任务、通道、锁或临时文件需要由本文件释放。

若未来真正由多个 worker 直接写 `Duration` 或 `u64` 字段，现有结构不足以保证数据竞争安全；应选择 worker 私有统计后汇总，或把共享字段改成适当的原子/锁，而不能因一个最大值字段是原子的就认为整个结构可并发变更。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/join/hash_join_stats.go`。Rust 的两个格式化 helper、v1/v2 结构、`set_max_value`、reset、逐轮 reset、clone、merge 和字符串字段顺序都以该文件为主要语义来源。`pkg/executor/join/join_stats_test.go::TestHashJoinRuntimeStats` 的期望字符串也被 Rust 测试复现。

主要类型映射为：Go `hashJoinRuntimeStats` ↔ Rust `HashJoinRuntimeStats`，Go `hashStatistic` ↔ Rust `HashStatistic`，Go `spillStats` ↔ Rust `SpillStats`，Go `hashJoinRuntimeStatsV2` ↔ Rust `HashJoinRuntimeStatsV2`。Go 的 `int64` 纳秒计数在 Rust 中多数提升为 `Duration`，需要并发更新的最大 worker 值仍保留为原子纳秒。

当前迁移并非完全等价接线：Rust `SpillStats` 额外包含 `spilled_partition_num`、`spilled_bytes`、`restored_bytes`，`HashJoinV2Exec::collect_spill_stats()` 只填这三个字段；`Display` 却读取 Go 对齐的 `round` 和四个 `*_per_round` 字段。因此生产执行器能够采集 Rust 自有 spill 数据，但除非其他调用方显式填充 Go 对齐字段，字符串不会出现完整 spill 段。另一个差异是 Rust 用 `saturating_sub` 和纳秒钳制避免 Go 整数式计算可能产生的异常表示。

Go v2 `Clone()` 同样没有复制 `workerFetchAndProbe`、`spill` 和 `isHashJoinGA`；Rust 测试明确固定了这一选择性 clone 语义。Go `Merge()` 也不合并这些字段，Rust 保持相同边界。

## 扩展指南

新增统计字段时，至少同步检查四处：对应结构的默认值/类型、自定义 `Clone`、`merge`/`reset`/`reset_round`、`Display`。若字段来自生产执行阶段，还要在 `hash_join_v1.rs` 或 `hash_join_v2.rs` 的具体 build/probe/spill 生命周期接线；只在结构中加字段不会产生真实观测数据。

修改格式字符串时，应同时更新独立测试 `pkg/executor/join/hash_join_stats_test.rs` 和更完整的 `pkg/executor/join/join_stats_test.rs`，并与 `pkg/executor/join/hash_join_stats.go` 及 `join_stats_test.go` 核对字段名、顺序、空段、舍入和 clone/merge 选择。Rust 单元测试应继续放在这些独立测试文件中，不要内嵌到生产源文件。

若要补全 v2 spill 展示，最可能的接入点是 `HashJoinV2Exec::collect_spill_stats()`：需要明确如何从 `HashJoinSpillHelper` 映射到 `round`、`partition_num` 和每轮四组 Go 字段，并验证 restore 多轮的分母语义。不要简单把 Rust 自有三组向量改名，因为其含义与 Go 的 build row table/hash table 分类并不一一对应。

若要让这些类型进入统一 RuntimeStats 收集链，应先核对 `astersql-util-execdetails` 的 trait、类型编号和注册所有权，避免仅凭本地 `TYPE` 常量假定已经实现全局接口。性能上应避免在热路径频繁构造展示字符串；并发上新增共享计数必须延续 worker 私有汇总或明确的同步策略。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/executor/join` 确认目标、Go 对照和独立测试均已索引；`node --file pkg/executor/join/hash_join_stats.rs` 读取实际实现；`query` 定位 `HashJoinRuntimeStats`、`HashJoinRuntimeStatsV2`、`set_max_value`、`write_spilled_partition_num_stats`。精确 `callers` 查询没有产生可用输出，因此调用边以源码搜索补证。
- 生产源码：`pkg/executor/join/hash_join_stats.rs`、`hash_join_v1.rs`、`hash_join_v2.rs`、`lib.rs`。
- crate 边界：`pkg/executor/join/Cargo.toml`。
- Go 对照：`pkg/executor/join/hash_join_stats.go`、`hash_join_v1.go`、`hash_join_v2.go`、`hash_table_v1.go`。
- 独立测试：`pkg/executor/join/hash_join_stats_test.rs` 覆盖 v1/v2 文本、选择性 clone/merge、逐轮 reset 和 spill helper；`pkg/executor/join/join_stats_test.rs` 覆盖 v1 clone/merge、v2 max 与不合并 spill 的边界；`pkg/executor/join/join_stats_test.go` 提供 Go v1 基准期望。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 个二级标题的结构命令和人工事实复核作为验收。
