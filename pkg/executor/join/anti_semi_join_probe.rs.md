# `pkg/executor/join/anti_semi_join_probe.rs`

## 文件定位

`anti_semi_join_probe.rs` 是 `astersql-executor-join` crate 中 Hash Join probe 阶段的 Anti Semi Join 实现。它实现 `base_join_probe.rs` 定义的 `Probe` trait，并通过 `BaseSemiJoin` 复用 semi/anti-semi 的共享状态。模块由 `pkg/executor/join/lib.rs` 公开声明，具体对象由 `base_join_probe.rs::new_join_probe` 在 `JoinType::AntiSemi` 分支创建。

该文件处理普通 Anti Semi Join：返回左侧中在右侧找不到符合 join key 及 other condition 的行。它不是 `AntiLeftOuterSemi` 的 marker-column 实现，也没有自己实现 null-aware anti join 状态机；本文件向 `match_probe_row` 传入的是 `NaajType::Unknown`。

## 核心职责

- 在装载每个 probe chunk 后同步重置 `BaseJoinProbe` 和 `BaseSemiJoin` 的逐行状态。
- 右侧为 build side 时，逐行调用 `BaseSemiJoin::match_probe_row`，仅把没有命中的 probe 行交给 `Joiner::on_miss_match` 输出。
- 左侧为 build side 时，probe 阶段不产出行，而是将 `Matched` 或 `HasNull` 的 build 候选行标记为 used；probe 完成后由 row-table scan 输出未 used 的 build 行。
- 把 spill、恢复 chunk、输出容量限制、扫描游标和 reset 接入统一 `Probe` 生命周期。

## 主要符号

- `pub struct AntiSemiJoinProbe { pub semi: BaseSemiJoin }`：本文件唯一类型。它本身不另存储匹配数据，所有状态均位于 `semi` 及其 `base`/`context` 字段。
- `set_chunk_for_probe` / `set_restored_chunk_for_probe`：建立当前 chunk 的哈希候选表，然后清空 `matched`、`has_null` 和 `unfinished_probe_rows`。当前 Rust 的 restored 路径与普通路径完全相同。
- `probe`：核心状态机，按 `is_left_side_build` 分流，且在 `max_chunk_size` 限制内逐行推进。
- `need_scan_row_table` / `init_for_scan_row_table` / `scan_row_table` / `is_scan_row_table_done`：左 build 路径的后置扫描协议。`scan_row_table` 传入 `expected_used = false`。
- `is_current_chunk_probe_done`：同时要求未完成队列为空且基础 probe 游标到达 chunk 尾部。
- `reset_probe`：清理当前 probe chunk 的基础缓冲、游标和 semi 共享标记；不重建 Hash Join context。

## 执行流程

1. `new_join_probe` 先用 `HashJoinContext` 创建 `BaseJoinProbe`，再以 `!right_as_build_side` 得到 `is_left_side_build`，包装成 `AntiSemiJoinProbe`。
2. worker 调用 `set_chunk_for_probe`。`BaseJoinProbe::set_chunk_for_probe` 拒绝覆盖尚未处理完的上一 chunk，检查 probe key 下标，序列化 key，并从 context 的哈希表取出键相等的 build 行下标。随后 `reset_probe_state` 为本 chunk 重建 semi 状态。
3. `probe` 在尚有 probe 行且本次输出未达 `max_chunk_size` 时循环。
4. 若左侧是 build side，它从 `matched_rows[index]` 取得候选 build 行，以当前 probe 行作为 inner 调用 `Joiner::try_to_match_outers`。每个返回 `Matched` 或 `HasNull` 的位置都把对应 `build_row_used` 设为 `true`；此分支的 `rows` 保持为空。
5. 若右侧是 build side，它调用 `match_probe_row(index, ..., NaajType::Unknown)`。若 `MatchResult::matched` 为 false，则把累积的 `has_null` 与 probe 行交给 `Joiner::on_miss_match`；对普通 `AntiSemi` 而言，非 null-aware 且 `has_null` 为 true 时不输出。
6. 每行成功处理后，`finish_current_lookup_loop` 推进 `current_probe_row` 并清零候选游标。如果 joiner 返回错误，则立即返回已累积的行和错误，不推进当前行。
7. 左 build 的调用者在 probe 结束后初始化 `scan_row_index`，反复调用 `scan_row_table`；`BaseSemiJoin::scan_build_rows(false)` 按 chunk 容量分批输出所有未 used 的 build 行。

## 数据与状态

- `BaseJoinProbe::probe_chunk`、`matched_rows`、`current_probe_row` 表示当前 chunk、每行候选 build 下标及行级游标。`spill_remaining_probe_chunks` 仅取 `current_probe_row..` 的尾部，因此已处理行不会重复 spill。
- `HashJoinContext::build_row_used` 与 build rows 等长。左 build 时它是跨 probe chunk 的累积结果；`reset_probe` 不清除它，否则最终扫描会丢失先前 chunk 的命中信息。
- `BaseSemiJoin::matched` 和 `has_null` 是当前 probe chunk 的逐行标记；`unfinished_probe_rows` 用于表示因候选消费不完而延后的行。在当前 `AntiSemiJoinProbe::probe` 路径中，joiner 对 semi 族首次命中就将 `consumed` 设为全部候选数，通常不会留下未完成行；完成判定仍保留该不变量。
- `scan_row_index` 是左 build 后置扫描的独立游标，可在输出 chunk 满时暂停并于下次调用续跑。

## 依赖与调用关系

- 上游构造：`base_join_probe.rs::new_join_probe` 是直接工厂入口；`pkg/executor/join/anti_semi_join_probe_test.rs::anti_probe` 也通过该工厂获得 `Box<dyn Probe>`。RustCodeGraph 的文件关系还标识 `base_join_probe.rs` 和 `hash_table_v1.rs` 为目标文件的使用者。
- 下游共享状态：`base_semi_join.rs::BaseSemiJoin` 提供 `reset_probe_state`、`match_probe_row` 和 `scan_build_rows`；`base_join_probe.rs::BaseJoinProbe` 提供 chunk 预处理、spill 和游标推进。
- 下游语义：`joiner.rs::Joiner::try_to_match_outers`、`try_to_match_inners` 和 `on_miss_match` 决定 other condition 的 true/false/NULL 语义与输出投影。
- crate 边界：`pkg/executor/join/Cargo.toml` 将该包定义为 `astersql-executor-join`，`[lib]` 入口为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/executor/join"` 标记 Go 对照包。目标文件只直接依赖 crate 内部模块和标准库容器，没有自己的 feature 或条件编译分支。

## 错误处理与边界

- 新 chunk 的 probe key 列越界时，`BaseJoinProbe::set_chunk_for_probe` 返回包含行号的 `String` 错误；上一 chunk 未完成时返回 `previous chunk is not probed yet`。
- other condition 求值错误由 `Joiner` 原样向上传播。`probe` 会在 `WorkerResult.error` 中返回错误，并保留错误前已产出的 `rows`。
- build 或 probe join key 为 NULL 时，`HashJoinContext`/`BaseJoinProbe` 不为该 key 建立等值候选。普通 anti join 因此会把 NULL probe key 当作未命中行输出；这与 `NOT IN` 的 null-aware 语义不能直接等同。
- 左 build 分支把 `OuterRowStatus::HasNull` 也标记为 used，避免 other condition 为 SQL NULL 时错误输出该 build 行；独立 Rust 测试专门锁定了此行为。
- `max_chunk_size` 必须在 `Joiner::new` 时为正数。本类型假定 context 中 `matched_rows` 与 build 下标有效；这些不变量由构造和 chunk 预处理维护。

## 并发与资源生命周期

`AntiSemiJoinProbe` 没有内部锁、原子操作、通道或异步任务；其 `Probe` API 需要 `&mut self`，表明单个实例的 chunk、游标和 used 标记由单一可变访问者推进。类型的 `Clone` 是状态深拷贝，并不提供共享同一 `build_row_used` 的并发协调。

一个 probe chunk 的生命周期是“set → 反复 probe 直到 done → 可选 spill/reset”。左 build 时还有“所有 probe chunk 完成 → init scan → 反复 scan 直到 done”。`Vec<Row>` 和字符串错误均由 Rust 所有权管理，没有手工释放路径。Spill 在这一层只移出未处理行的内存向量，文件 I/O 由更高层 spill 组件负责。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/join/anti_semi_join_probe.go`。Rust `AntiSemiJoinProbe`/`BaseSemiJoin` 对应 Go `antiSemiJoinProbe`/`baseSemiJoin`；`set_chunk_for_probe`、`probe`、scan 组方法、`is_current_chunk_probe_done` 和 `reset_probe` 均能找到同名语义。两者都覆盖左/右 build，other condition 的 true/false/NULL，容量限制续跑，以及未命中行的输出。

当前 Rust 实现是可运行的行向量模型，不是 Go 实现的逐句机械翻译：

- Go 按“有/无 other condition × 左/右 build”分为四个专用 probe 函数，使用 `chunk.Chunk`、行地址链、向量表达式求值和中间 joined chunk；Rust 用 `Joiner` 逐行评估并统一了分支。
- Go 在 probe/scan 循环内检查 `SQLKiller`，并在左 build 无 other condition 路径使用原子 used flag；Rust 此文件没有对应的取消检查或原子标记。
- Go 的 spill 状态会把已 spill 行视为 matched，restored chunk 走专用 base 方法；Rust 的 restored 路径复用普通 set，spill 路径仅保留当前未处理尾部。
- Go `InitForScanRowTable`/`ScanRowTable` 在非左 build 时 panic，且 scan 要求先 init；Rust 方法本身不做这些 panic 检查，依赖 `need_scan_row_table` 协议的正确调用顺序。

Go 回归 `anti_semi_join_probe_test.go` 还覆盖更多 SQL 类型、复合 key、nullable key、重复 key 和 `NOT IN` 场景；这些证明 Go 实现的行为意图，但不能单独作为 Rust 已具备同等类型系统与 null-aware 执行能力的证据。

## 扩展指南

- 改变 anti 输出或 NULL 语义时，首先检查 `probe`、`BaseSemiJoin::match_probe_row` 与 `Joiner::on_miss_match`的边界，同时覆盖左 build 的 `OuterRowStatus::HasNull` 标记逻辑。
- 改变容量或续跑机制时，必须保持“行只输出一次”、`current_probe_row` 单调前进、spill 不带走已处理行，以及 `is_current_chunk_probe_done` 等待未完成队列清空。
- 扩展左 build 时，不要在 chunk reset 中清除 `build_row_used`；该标记必须保留到全部 probe 完成后的 scan。若引入多 worker 共享 context，需要先设计 Go 版原子 used flag 的 Rust 对等机制。
- 若要对齐 Go 的取消、向量化或 spill/restored 语义，这不只是本文件的局部修改；需连同 `base_join_probe.rs`、`base_semi_join.rs`、`joiner.rs` 及上层 worker 协议一并评估。
- 所有 Rust 回归都应放在独立的 `pkg/executor/join/anti_semi_join_probe_test.rs`，不要内嵌到生产文件。最少同时跑右 build、左 build + scan、other condition NULL、多次容量续跑与 spill 用例。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/executor/join/anti_semi_join_probe.rs` 确认目标文件已索引且有 15 个符号；`node --file ... --offset 1 --limit 500` 读取了全部 126 行并报告直接使用文件；`query AntiSemiJoinProbe` 核对了 Rust 类型与 Go 对照符号。`callers/callees` 在当前索引上未在 30 秒内返回，调用关系因此又用工厂和模块源文本直接核验。
- Rust 源码：`anti_semi_join_probe.rs`、`base_join_probe.rs`、`base_semi_join.rs`、`joiner.rs`、`lib.rs`。
- crate 声明：`pkg/executor/join/Cargo.toml`。其移植元数据明确指向 Go 包 `pkg/executor/join`。
- Rust 独立测试：`anti_semi_join_probe_test.rs` 覆盖重复 build key 只输出未命中行、restored/reset、NULL key、左 build 后置扫描、NULL other condition 抑制 build 行、spill 尾部与容量续跑。
- Go 对照：`anti_semi_join_probe.go` 提供分支、scan、spill 和取消的原始实现；`anti_semi_join_probe_test.go` 提供左/右 build、other condition、nullable/复合 key、重复 key 与 `NOT IN` 的回归意图。
- 本任务是纯文档分析，按计划不运行 Cargo；交付检查为固定十一章节的结构验证与人工事实复核。
