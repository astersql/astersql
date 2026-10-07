# `pkg/executor/join/left_outer_semi_join_probe.rs`

## 文件定位

本文件属于 `astersql-executor-join` crate；crate 根由 `pkg/executor/join/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/join/lib.rs` 以公开模块 `left_outer_semi_join_probe` 装配它。它实现 Hash Join probe 阶段的 Left Outer Semi 与 Anti Left Outer Semi 变体：对每条左侧（probe 侧）行保留原行并追加布尔或 `NULL` 标记，而不展开右侧（build 侧）列。

直接构造入口是 `base_join_probe.rs::new_join_probe`。当 `JoinType` 为 `LeftOuterSemi` 或 `AntiLeftOuterSemi` 且 `right_as_build_side == true` 时，工厂创建 `LeftOuterSemiJoinProbe` 并以 `Box<dyn Probe>` 返回；左侧作为 build 侧会直接返回错误。因此，该实现位于“build 侧哈希表已建立、worker 为当前 probe chunk 查找候选行”之后和 `Joiner` 生成最终标记行之前。

## 核心职责

- 通过 `Probe` trait 将当前 probe chunk 的装载、spill/恢复、分批 probe、完成判定和重置统一接入 Hash Join probe 状态机。
- 对每条 probe 行调用 `BaseSemiJoin::match_probe_row`，让共享基座取得候选 build 行，并由 `Joiner::try_to_match_inners` 求值 other condition 和生成命中标记。
- 在没有命中时调用 `Joiner::on_miss_match`，把 Left Outer Semi 的未命中写成 `false`、Anti 变体写成 `true`，把需要三值逻辑表达的未知结果写成 `NULL`。
- 在 `null_aware` 模式下根据两侧 join key 是否含 `NULL` 选择 `NaajType`，并保证未命中行的标记保留 NAAJ 的未知语义。
- 明确声明此连接形态不需要 probe 完成后扫描整张 build row table；两个扫描生命周期方法是不可达防线。

## 主要符号

- `pub struct LeftOuterSemiJoinProbe { semi, anti, null_aware }`：唯一生产类型。`semi: BaseSemiJoin` 持有候选、游标、匹配与未完成队列；`anti` 区分普通/反向标记语义；`null_aware` 开启空值感知路径。类型实现 `Clone`，但文件没有共享锁或全局状态。
- `impl Probe for LeftOuterSemiJoinProbe`：公开行为通过 trait object 暴露，方法本身不是固有公开方法。
- `set_chunk_for_probe`：委托 `BaseJoinProbe::set_chunk_for_probe` 校验 probe key 下标、序列化键并查哈希桶，成功后调用 `BaseSemiJoin::reset_probe_state` 重建每行状态。
- `set_restored_chunk_for_probe`：恢复路径复用普通装载路径，因而具有相同校验与状态初始化语义。
- `spill_remaining_probe_chunks`：委托 base 把尚未处理的 probe 尾部行移入 spill 缓冲并取出。
- `probe`：本文件的主执行入口；受 `max_chunk_size` 限制逐行推进、计算 NULL 状态、匹配候选，并输出 `WorkerResult`。
- `need_scan_row_table` / `init_for_scan_row_table` / `scan_row_table` / `is_scan_row_table_done`：描述 build 表扫描协议；本实现返回“不需要扫描”，初始化与完成查询会 panic，`scan_row_table` 仅给出默认空结果。
- `is_current_chunk_probe_done`：只有未完成队列为空且 base 游标越过当前 chunk 时才完成。
- `reset_probe`：先清空 base 的 chunk、键、候选、游标、冲突计数和扫描游标，再清空 semi 的匹配、NULL 与未完成队列。

本文件没有模块级常量、条件编译项或私有辅助函数。

## 执行流程

1. `new_join_probe` 验证右侧为 build 侧，用 `BaseJoinProbe::new` 与 `BaseSemiJoin::new(base, false)` 构造本类型；`anti` 来自具体 `JoinType`，`null_aware` 由调用方传入。
2. worker 调用 `set_chunk_for_probe`。base 拒绝覆盖尚未探测完的旧 chunk，逐行校验 key 下标；含 `NULL` 的 probe key 不查哈希表，普通 key 在哈希桶内以完整序列化键二次过滤。随后 semi 将 `matched`、`has_null` 调整为 chunk 行数并全部置为 `false`，清空未完成队列。
3. `probe` 创建本批输出 `rows`，在 probe 游标未到末尾且输出行数小于 `max_chunk_size` 时循环。每轮先检查当前 probe key 是否含 `NULL`，再检查任一 build 行的任一 build key 是否含 `NULL`。
4. 普通模式把 `NaajType::Unknown` 传给 `match_probe_row`；null-aware 模式按 probe key 是否含 NULL 传 `LeftHasNullRightNotNull` 或 `LeftNotNullRightNotNull`。build 侧是否含 NULL 不编码进这里的枚举，而在未命中分支单独合并处理。
5. `BaseSemiJoin::match_probe_row` 克隆当前 outer 行和候选 build 行，调用 `Joiner::try_to_match_inners`。后者逐个求值条件；Left Outer Semi 命中追加 `true`，普通 Anti Left Outer Semi 命中追加 `false`，null-aware Anti 命中由 `naaj_match_row` 决定；半连接族首次命中后停止继续扫描重复候选，因此一条 outer 行最多输出一个标记结果。
6. 若返回 `matched == false`，普通模式使用 `MatchResult::has_null`；null-aware 模式使用 `probe_key_has_null || build_has_null_key`。`on_miss_match` 据此追加普通/anti 的 `false`、`true` 或 `NULL` 标记。
7. 匹配或补行完成后，`finish_current_lookup_loop` 将 probe 行游标加一并清零当前候选游标。发生条件求值错误则立即返回当前已生成的行以及 `WorkerResult.error = Some(error)`，且不会推进出错行。
8. 调用方可依据 `is_current_chunk_probe_done` 决定是否继续调用 `probe`；完成后用 `reset_probe` 释放本批状态，或通过 spill/恢复方法转移未处理尾部。

## 数据与状态

`LeftOuterSemiJoinProbe` 自身只有策略字段，绝大多数可变状态在两层基座中：

- `BaseJoinProbe` 保存 `HashJoinContext`、`probe_chunk`、各行哈希值与序列化键、每行候选 build 下标、`current_probe_row`/`current_candidate`、spill 缓冲和扫描游标。
- `BaseSemiJoin` 保存逐 probe 行的 `matched` 与 `has_null`，以及容量受限时使用的 `unfinished_probe_rows`。
- `HashJoinContext` 保存 build 行、键下标、哈希桶、build 行使用标记、`Joiner` 和 `max_chunk_size`。其构造阶段不会把含 NULL key 的 build 行加入哈希表，但原始 `build_rows` 仍保留，故本文件可扫描它们来判断 NAAJ 未命中是否未知。
- `WorkerResult` 同时携带已生成的 `Vec<Row>` 与可选字符串错误；错误发生前的部分输出不会被丢弃。

关键不变量是：`matched`/`has_null` 的长度与当前 `probe_chunk` 相同；`current_probe_row` 指向下一条待处理行；每处理完一行恰好调用一次 `finish_current_lookup_loop`；Left Outer Semi/Anti Left Outer Semi 只支持 `right_as_build_side == true`。`anti` 字段在本文件的控制流中未直接读取，实际输出取反由构造时已配置相同 `JoinType` 的 `Joiner` 完成；它仍保留了具体 probe 的配置身份。

## 依赖与调用关系

上游直接证据：

- `pkg/executor/join/lib.rs` 导出本模块，并在 `#[cfg(test)]` 下登记独立测试模块。
- `pkg/executor/join/base_join_probe.rs::new_join_probe` 是生产构造入口，返回 `Box<dyn Probe>`；同文件的 `Probe` trait 定义本实现必须满足的生命周期接口。
- RustCodeGraph 将目标文件列为被 `base_join_probe.rs` 和 `outer_join_probe.rs` 使用；对 `probe`/`LeftOuterSemiJoinProbe` 的 `callers` 查询为空，这与 trait object 动态分派无法形成直接静态边一致。因此这里不声称某个具体 worker 是已由图确认的 Rust 调用者。

下游直接依赖：

- `BaseJoinProbe::{set_chunk_for_probe, spill_remaining_probe_chunks, finish_current_lookup_loop, is_current_chunk_probe_done, reset_probe}` 管理键查找、游标与批次资源。
- `BaseSemiJoin::{reset_probe_state, match_probe_row}` 管理半连接逐行状态，并调用 `Joiner::try_to_match_inners`。
- `Joiner::{try_to_match_inners, on_miss_match}` 实施条件求值、投影和 `true`/`false`/`NULL` 标记规则。
- `NaajType` 描述 null-aware anti join 的左右 NULL 组合；`Row` 与 `Value::Null` 是本实现实际处理的数据表示。

`Cargo.toml` 表明该文件与上述模块在同一 crate 内，所用三个 `use crate::...` 均为 crate 内依赖。本文件自身没有直接引用 Cargo 中的外部 crate，也没有 feature gate；清单中的大批执行器依赖只在 Windows target 下声明，不能据此推断本文件直接调用了它们。

## 错误处理与边界

- `set_chunk_for_probe` 传播 base 的字符串错误：旧 chunk 未完成时返回 `previous chunk is not probed yet`；probe key 下标越界时返回带行号的错误。只有装载成功后才重置 semi 状态。
- `probe` 传播 predicate/Joiner 的字符串错误，保留错误前已写入的输出行；出错行的游标不推进，调用方必须先处理错误，不能把该结果当成完整 chunk。
- `max_chunk_size` 控制单次输出循环。构造 `Joiner` 时要求该值大于零；若绕过构造约束令其为零，`probe` 不会推进。
- 普通等值连接中，任一侧 join key 为 NULL 都不匹配。普通 Left Outer Semi 对 NULL key 输出 `false`；启用 null-aware Anti 时，只要 probe key 或任意 build key 含 NULL，未命中标记为 `NULL`。
- 多个重复 build key 首次成功后即停止，避免同一 outer 行重复输出。
- `init_for_scan_row_table` 和 `is_scan_row_table_done` 是故意 panic 的不可达接口；正确调用方必须先检查 `need_scan_row_table() == false`。`scan_row_table` 虽返回空默认值，也不应作为正常执行阶段调用。
- 当前 null-aware 分支只区分 probe key 是否含 NULL来选择两个 `NaajType`，而 build NULL 通过全表布尔值参与未命中判断；若扩展多键或更细的 NAAJ 组合，必须重新核对这一简化是否仍满足 Go 语义。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或文件句柄。每个 `LeftOuterSemiJoinProbe` 是带独立可变游标和缓冲的 worker 状态，所有 `Probe` 方法都要求 `&mut self`；并行执行应由上层为每个 worker 创建独立实例，而不是并发修改同一实例。

生命周期从 `new_join_probe` 构造开始，经 `set_chunk_for_probe` 装载一批行，再由一次或多次 `probe` 推进直到 `is_current_chunk_probe_done`。内存压力路径可用 `spill_remaining_probe_chunks` 取走尚未处理的尾部，随后以 `set_restored_chunk_for_probe` 重新装载；该恢复入口完整重走键构建和状态重置。`reset_probe` 清空本批缓存供实例复用，但 build 侧上下文仍随 `BaseJoinProbe` 保留。`Clone` 会复制当前状态和上下文，不代表两个 clone 共享进度或 build 使用标记。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/join/left_outer_semi_join_probe.go`，两者保持了这些意图：右侧作为 build 侧；每个 probe chunk 重置匹配状态；不扫描 build row table；按是否 anti 生成相反的 0/1（Rust 为 `Bool`）标记；条件为 NULL 时保留 NULL；重复候选只决定一个 outer 标记；spill 恢复后重新初始化状态；扫描初始化/完成查询为不可达 panic。

Rust 当前实现不是 Go 文件的逐语句等价实现：

- Go 直接操作 `chunk.Chunk`、row-table 地址链、tagged pointer、`remainCap`、selection vector、`SQLKiller` 和 probe collision；Rust 活跃实现使用拥有所有权的 `Vec<Row>` 与预先建立的候选下标，相关底层工作集中在 `BaseJoinProbe`/`Joiner`。
- Go 把 other condition 分成 `probeWithOtherCondition`、`produceResult` 和无条件快路径，并可复制列；Rust 统一通过 `BaseSemiJoin::match_probe_row` 与逐行 `Joiner` 求值，没有该列复制快路径，也没有在本文件中检查 SQL killer。
- Go 类型以 `isNullRows` 单独记录未知结果；Rust 使用 `BaseSemiJoin::has_null`，并在本文件额外计算 NAAJ 的 key NULL 状态。Rust 结构含 `null_aware` 字段，Go 对照结构本身没有该字段。
- Go `ScanRowTable` 原样返回传入结果；Rust trait 无输入参数，所以返回 `WorkerResult::default()`。两者都由 `NeedScanRowTable == false` 保证不进入正常流程。

因此，扩展或纠错时应保持 SQL 可观察语义与 Go 测试意图一致，但不能把注释区保留的旧 Go 形状误认为 Rust 活跃代码已经具有相同的 chunk、取消或性能机制。

## 扩展指南

- 修改标记或三值逻辑：首先检查本文件 `probe` 中 `NaajType` 与 `has_null` 计算，再同步检查 `joiner.rs::{try_to_match_inners,on_miss_match,naaj_match_row}`；至少扩展 `left_outer_semi_join_probe_test.rs`，Anti/NAAJ 行为还应同步 `left_outer_anti_semi_join_probe_test.rs`。
- 修改候选查找、NULL key 或 spill：接入点在 `base_join_probe.rs::{set_chunk_for_probe,candidate_rows,spill_remaining_probe_chunks}`，本文件仍须确保状态重置顺序不变。回归应覆盖普通装载、恢复、旧 chunk 未完成和 key 下标越界。
- 修改 other condition 的批处理/容量语义：接入点是 `BaseSemiJoin::match_probe_row` 和 `Joiner::try_to_match_inners`。必须验证单个 outer 多候选、条件 `true/false/NULL/error`、小 `max_chunk_size` 下多次 `probe` 以及 `unfinished_probe_rows` 的完成判定。
- 若新增 build 侧方向支持，不能只移除 `new_join_probe` 的错误分支；还需设计事后 row-table 扫描、outer 行来源和 used 标记，并实现目前故意不可达的三个扫描方法。
- 若追求 Go 的性能/取消等价，应分别评估 selection/列式批处理、候选链增量消费、SQL killer 与 collision 统计，不能仅在本文件加局部快路径。性能风险集中在每行扫描所有 `build_rows` 计算 `build_has_null_key`、候选行克隆以及逐行 predicate 求值。
- Rust 单元测试必须继续放在独立的 `pkg/executor/join/left_outer_semi_join_probe_test.rs`（以及 anti 对应测试文件），不要内嵌进生产源文件。

兼容性重点是标记类型及 NULL 语义、输出行顺序、每个 outer 恰好一行、只支持右 build；正确性重点是错误时的游标与部分结果；性能重点是 NAAJ 每行全 build 扫描及行克隆。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被识别为 15 个符号。
- RustCodeGraph `node --file pkg/executor/join/left_outer_semi_join_probe.rs`：核对 `LeftOuterSemiJoinProbe`、完整 `Probe` 实现及 359 行源文件；`query LeftOuterSemiJoinProbe` 核对结构和 9 个 trait 方法。
- RustCodeGraph `callers`/`callees`：对目标 `probe` 的精确节点查询返回空数组；文件关系显示被 `base_join_probe.rs`、`outer_join_probe.rs` 使用。动态分派缺口由 `base_join_probe.rs::new_join_probe` 的直接构造代码补证，不把空图结果解释为“无人调用”。
- 已读生产与装配证据：`pkg/executor/join/base_join_probe.rs`（`Probe`、`HashJoinContext`、`BaseJoinProbe`、`new_join_probe`）、`base_semi_join.rs`（`reset_probe_state`、`match_probe_row`）、`joiner.rs`（`Joiner::try_to_match_inners`、`on_miss_match`）、`lib.rs`、`Cargo.toml`。
- 已读 Go 对照：`pkg/executor/join/left_outer_semi_join_probe.go`，逐项核对构造约束、状态重置、两类 probe、标记构造、扫描不可达和完成判定。
- 已读独立 Rust 测试：`pkg/executor/join/left_outer_semi_join_probe_test.rs`，覆盖 true/false 标记、条件 NULL、重复 build key、spill/恢复、普通 NULL key、null-aware anti 的未知结果，以及两个不可达 panic。Go 测试 `left_outer_semi_join_probe_test.go` 的索引符号另显示 basic、全 join key、other condition、selection、fast path 与 spill 测试面；本任务未运行测试，因为计划明确为纯文档且禁止 Cargo。
- 结构验证应确认目标文档存在且恰有本页 11 个固定二级标题；最终交付前另人工复核本文只描述可由上述符号和文件支持的当前事实。
