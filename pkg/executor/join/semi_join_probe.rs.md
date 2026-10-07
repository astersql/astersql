# `pkg/executor/join/semi_join_probe.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-executor-join`（`pkg/executor/join/Cargo.toml` 的 `[lib]` 指向 `lib.rs`），并由 `pkg/executor/join/lib.rs` 以 `pub mod semi_join_probe` 暴露。它实现 Hash Join 的 Semi Join（半连接）探测器：只保留左侧中“在另一侧至少存在一个满足连接键及附加条件的匹配”的行，不输出左右行的拼接结果。

文件有两个必须区分的部分：第 24～331 行是整段注释掉的 Go v2 探测路径对照草稿，不参与 Rust 编译；真正生效的 Rust 实现从 `use crate::base_join_probe::{Probe, WorkerResult}` 开始（第 333 行）。因此，注释草稿中的 `SQLKiller`、向量化 `Chunk`、tagged pointer、原子 used flag 和四条专用分支不是当前 Rust 运行时能力。

当前 Rust 入口由 `base_join_probe.rs::new_join_probe` 的 `JoinType::Semi` 分支创建 `SemiJoinProbe`，再通过 `Box<dyn Probe>` 使用。仓库搜索只找到测试代码调用 Rust `new_join_probe`，没有找到非测试 Rust 生产调用点；Cargo workspace 和 `pkg/executor/Cargo.toml` 已声明该 crate，但不能据此断言这条 Rust probe 已接入完整 SQL 执行主链。

## 核心职责

- `SemiJoinProbe` 把 `BaseSemiJoin` 组合成统一的 `Probe` trait 实现。
- 装载普通或 spill 恢复后的 probe 行，并重置每行匹配状态。
- 右侧为 build side 时，逐个 probe 行调用 `BaseSemiJoin::match_probe_row`；一旦 `Joiner` 判定半连接命中，输出该 probe 行，避免因 build 侧重复键重复输出外表行。
- 左侧为 build side 时，probe 阶段不直接输出；它用 `Joiner::try_to_match_outers` 判断每个候选 build 行是否满足附加条件，并设置 `HashJoinContext::build_row_used`。之后通过 row-table 扫描输出 used 为 `true` 的 build 行。
- 把 probe 进度、扫描游标、错误结果和 spill 剩余行交给 `BaseJoinProbe` / `BaseSemiJoin` 管理。

它不负责构建哈希表、序列化连接键、执行 SQL 调度或落盘 I/O。哈希候选在 `BaseJoinProbe::set_chunk_for_probe` 中准备；条件判断和结果形状由 `Joiner` 决定；本文件只协调半连接特有的“存在性”语义。

## 主要符号

- `pub struct SemiJoinProbe { pub semi: BaseSemiJoin }`：文件唯一的生产类型，`#[derive(Clone)]`。公开字段允许 crate 使用者访问共享半连接状态，但正常入口是 `new_join_probe` 返回的 trait object。
- `impl Probe for SemiJoinProbe`：实现十个 trait 方法，没有本文件私有辅助函数、常量、条件编译项或 `unsafe` 代码。
- `set_chunk_for_probe(Vec<Row>) -> Result<(), String>`：先调用 `BaseJoinProbe::set_chunk_for_probe` 校验 probe key 下标、序列化键并查找真实候选，再调用 `BaseSemiJoin::reset_probe_state` 创建与 chunk 等长的 `matched` / `has_null` 数组并清空未完成队列。
- `set_restored_chunk_for_probe`：直接复用普通装载路径。当前 Rust 版本没有为恢复数据设置独立状态。
- `spill_remaining_probe_chunks`：委托 `BaseJoinProbe` 保存当前 probe chunk 尚未处理的尾部并返回 spill 缓冲。
- `probe() -> WorkerResult`：主状态机；受 `context.max_chunk_size` 限制，按 build side 走“标记 build 行”或“输出 probe 行”路径。
- `need_scan_row_table`、`init_for_scan_row_table`、`scan_row_table`、`is_scan_row_table_done`：左 build 的延迟输出协议。
- `is_current_chunk_probe_done`：要求 `unfinished_probe_rows` 为空且底层 probe 游标到末尾。
- `reset_probe`：清理底层 chunk/游标/冲突计数/扫描游标，并重建半连接的空匹配状态；不会清空共享上下文中的 `build_row_used`。

## 执行流程

1. `base_join_probe.rs::new_join_probe` 接收 `JoinType::Semi`，创建 `BaseJoinProbe`，并把 `!right_as_build_side` 作为 `BaseSemiJoin::is_left_side_build`。这是 `SemiJoinProbe` 的实际工厂入口。
2. 调用者用 `set_chunk_for_probe`（或恢复入口）装载一批 probe 行。`BaseJoinProbe` 为非 NULL 键计算哈希、取出同桶 build 行并再次比较完整序列化键；NULL probe key 得到空候选。成功后半连接状态被重置。
3. `probe` 在 `current_probe_row < probe_chunk.len()` 且当前返回行数小于 `max_chunk_size` 时循环：
   - 左 build：复制当前 probe 行对应的 build 下标和行，调用 `Joiner::try_to_match_outers(build_rows, probe_row, ...)`。仅 `OuterRowStatus::Matched` 对应的 build 下标被标记为 used；本阶段结果为空。
   - 右 build：调用 `BaseSemiJoin::match_probe_row(index, rows, NaajType::Unknown)`。该函数取候选 build 行，经 `Joiner::try_to_match_inners` 执行附加条件并按 Semi Join 语义写入 probe 行，同时累计 `matched`、`has_null` 和可能的未完成行。
   - 成功完成该 probe 行后，`finish_current_lookup_loop` 推进 `current_probe_row` 并清零候选游标。
4. 左 build 的调用者在 probe 完成后检查 `need_scan_row_table`，调用 `init_for_scan_row_table` 将扫描游标置零，再重复调用 `scan_row_table`。`BaseSemiJoin::scan_build_rows(true)` 每批最多返回 `max_chunk_size` 个 used build 行，直至 `is_scan_row_table_done` 为真。
5. 新一轮使用前可调用 `reset_probe`。如果上一 chunk 尚未处理完，直接装载新 chunk 会由 `BaseJoinProbe::set_chunk_for_probe` 返回 `previous chunk is not probed yet`。

当前实现有一项需要扩展者特别复核的状态事实：`match_probe_row` 能把“候选未全部消费”的行加入 `unfinished_probe_rows`，而本文件的 `probe` 仍会推进到底层下一行，且没有显式弹出/重放该队列。现有半连接测试覆盖常见重复键和条件场景，但这部分容量续跑行为不能仅凭字段名推断为完整实现。

## 数据与状态

`SemiJoinProbe` 自身只有 `semi` 字段，主要状态位于两层基类：

- `BaseSemiJoin::is_left_side_build` 决定结果产生位置；`matched` 与 `has_null` 记录右 build 下各 probe 行的匹配/NULL 结果；`unfinished_probe_rows` 记录候选未完全消费的 probe 行。
- `BaseJoinProbe::probe_chunk`、`matched_rows`、`current_probe_row`、`current_candidate` 保存当前批次及推进位置；`spilled_chunks` 保存待恢复尾部；`scan_row_index` 是左 build 事后扫描游标。
- `HashJoinContext::build_rows`、`hash_table` 和 `build_row_used` 是 build 侧数据及使用标记；`joiner` 承担附加条件与 Join 类型语义；`max_chunk_size` 限制单次结果批大小。

关键不变量是：`matched_rows[index]` 中的下标必须指向 `context.build_rows`；`build_row_used` 与 build 行等长；半连接对一个右 build probe 行最多输出一次，即使候选中有重复 build key。`HashJoinContext::new` 不把 NULL build key 放入哈希表，`set_chunk_for_probe` 也让 NULL probe key没有候选，所以普通 Semi Join 的 NULL 等值不成立。

`Clone` 会克隆整个 Rust 状态（包括 `HashJoinContext` 内的向量和布尔 used 标记），不是共享同一份原子 row-table 标记；不能把它视为 Go v2 多 worker 共享内存模型的等价物。

## 依赖与调用关系

上游关系：

- `pkg/executor/join/lib.rs` 声明模块。
- `base_join_probe.rs::new_join_probe` 在 `JoinType::Semi` 分支实例化本类型，并返回 `Box<dyn Probe>`。
- `semi_join_probe_test.rs::semi_probe` 是已找到的直接 Rust 使用者；它以左右 build、附加条件等组合驱动 trait 方法。
- Go 完整执行链由 `base_join_probe.go::newJoinProbe` 创建 `newSemiJoinProbe`，并由 `hash_join_v2.go` 根据 `NeedScanRowTable` 调度事后扫描；这是 Go 对照证据，不是 Rust 调用边。

下游关系：

- `BaseJoinProbe::{set_chunk_for_probe, spill_remaining_probe_chunks, finish_current_lookup_loop, is_current_chunk_probe_done, reset_probe}` 管理通用 probe 生命周期。
- `BaseSemiJoin::{reset_probe_state, match_probe_row, scan_build_rows}` 管理半连接匹配与扫描。
- 左 build 路径直接调用 `Joiner::try_to_match_outers` 并读取 `OuterRowStatus::Matched`；右 build 路径经 `match_probe_row` 调用 `Joiner::try_to_match_inners`。
- `NaajType::Unknown` 明确表示这里不是 NULL-aware anti join 路径。

`pkg/executor/join/Cargo.toml` 定义 crate 名并通过 workspace 被 `pkg/executor/Cargo.toml` 等引用。当前文件只直接使用同 crate 模块和 `std`；Cargo 中大量 join 依赖位于 `target.'cfg(windows)'.dependencies`，不能据此为本文件虚构直接外部调用。

## 错误处理与边界

- 所有可恢复错误以 `String` 表示。装载阶段的 key 下标越界或“上一 chunk 未完成”由 `set_chunk_for_probe` 原样返回。
- `probe` 中 Joiner 条件计算失败时立即返回 `WorkerResult { rows, error: Some(error) }`；此前已经生成的 `rows` 会随错误一起保留，而当前 probe 行不会执行末尾的游标推进。
- `scan_row_table` 当前只委托内存扫描，总是返回 `error: None`；与 Go 版不同，没有 SQL killer 检查。
- 右 build 调用 `init_for_scan_row_table`、`scan_row_table` 或 `is_scan_row_table_done` 都会以 `should not reach here` panic。Rust 独立测试逐项固定了这一约束。
- `scan_row_table` 未显式要求先调用初始化；`scan_row_index` 初值为 0，因此当前 Rust 行为不会像 Go 版那样以 `scanRowTable before init` panic。调用协议仍应先初始化，以免复用对象时从旧游标继续。
- 空 probe chunk、无候选、NULL key 都产生空结果而非错误。重复 build key 由 Semi Join/Joiner 语义折叠，不应重复输出右 build 下的外表 probe 行。
- 每次 `probe` 的循环容量按本次新增的 `rows.len()` 计算；左 build 阶段不产出行，因此会扫描完整 probe chunk，不受返回行容量提前截断。

## 并发与资源生命周期

本文件不创建线程、锁、channel、事务、文件或网络资源。所有输入输出都是拥有所有权的 `Vec<Row>` / `WorkerResult`，资源随 Rust 值生命周期释放；spill 方法在这里仅移动内存中的剩余行，并不执行磁盘 I/O。

状态机生命周期为“构造 → 装载 chunk → 一次或多次 probe →（左 build）初始化并分批扫描 build rows → reset”。`max_chunk_size` 控制右 build 的 probe 输出及左 build 的扫描输出。`build_row_used` 跨 `reset_probe` 保留，这是多批 probe 后统一扫描左 build 结果所必需的；若要把同一实例复用于逻辑上全新的 join，必须重建上下文，而不能只调用 `reset_probe`。

Go 原版的 used flag 使用 row-table 元数据和原子检查，且长循环响应 SQL kill；当前 Rust 用可变 `Vec<bool>`，没有同步原语或取消点。因此它适合当前单实例可变借用模型，不能在没有额外同步与共享状态设计的情况下由多个 worker 并发修改同一个逻辑 build 表。

## 与 Go 版本的对应关系

结构对应关系是 `SemiJoinProbe` ↔ `semiJoinProbe`，`BaseSemiJoin` ↔ `baseSemiJoin`，Rust 工厂 `new_join_probe` 的 Semi 分支 ↔ Go `newSemiJoinProbe`。普通/恢复 chunk 设置、左 build 需要 row-table 扫描、右 build 输出左侧 probe 行，以及右 build 禁止扫描的 panic 约束均保持同一总体语义。

但当前 Rust 是行向量级的简化实现，并非 Go 文件的逐函数完整移植：

- Go `Probe` 按“左/右 build × 有/无 other condition”分成四条专用路径；Rust 合并为两条 build-side 分支，把条件计算交给 `Joiner`。
- Go 使用 `chunk.Chunk`、row table 指针、tagged pointer、碰撞链、批量构造和向量化表达式；Rust 使用 `Vec<Row>`、build 行下标和预先过滤的 `matched_rows`。
- Go 左 build 无条件路径会避免重复原子写 used flag，并每 2000 次循环检查 `SQLKiller`；Rust 没有原子标记和取消检查。
- Go 带 other condition 时维护中间 joined chunk 与未完成队列；Rust `BaseSemiJoin::match_probe_row` 保留简化队列状态，但本文件没有 Go `produceResult` 等续跑分支。
- Go `ScanRowTable` 要求显式初始化并可返回 kill 错误；Rust 扫描基于整数游标，无未初始化状态。
- Go spill 测试覆盖临时文件清理与执行器级恢复；Rust 本文件只把未处理的行保存在内存向量中。

因此，Go 文件可以用来核对目标语义和尚待迁移的能力，但当前 Rust 行为必须以第 333～443 行及其 Rust 基类和测试为准。

## 扩展指南

- 若修改 Semi Join 产出语义，首先检查 `SemiJoinProbe::probe`，并同步审查 `BaseSemiJoin::match_probe_row`、`Joiner::{try_to_match_inners, try_to_match_outers}`；测试应放在独立的 `semi_join_probe_test.rs`，不要内嵌到生产源文件。
- 若增加新的 build-side 或 other-condition 分支，必须覆盖四种组合（左右 build × 有无条件）、重复键、NULL key、空候选、输出容量分页和错误中断后的游标状态。
- 若完善 unfinished queue，应定义候选消费位置、何时入队/出队、`is_current_chunk_probe_done` 与 `set_chunk_for_probe` 的关系，并新增“单行候选数超过一次容量”的回归测试；不要仅清空队列来让状态检查通过。
- 若实现真实 spill，需保持普通和恢复入口的状态等价，并验证剩余行顺序、错误传播和资源清理。当前 `spill_remaining_probe_chunks` 只有内存语义。
- 若接入并行 worker，需要先决定 `build_row_used` 的共享所有权及同步策略；简单克隆 `SemiJoinProbe` 会得到独立标记，无法汇总左 build 命中。
- 若追齐 Go v2，需把 SQL killer、chunk 容量、向量化过滤、原子 used flag、碰撞统计和 row-table 扫描作为明确的独立迁移项，不能把文件上半部注释直接视为实现。
- 性能风险集中在左 build 每行克隆所有候选 build rows、右 build `candidate_rows` 克隆候选，以及 `Vec<bool>` 扫描。优化时必须保留“重复 build 候选不重复输出 probe 行”和附加条件三值逻辑。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `query SemiJoinProbe` / `node SemiJoinProbe`：定位生产结构体到 `semi_join_probe.rs:338`，并给出 `new_join_probe` 的实例化调用边。
- RustCodeGraph `node new_join_probe`：确认 `JoinType::Semi` 构造 `SemiJoinProbe { semi: BaseSemiJoin::new(base, !right_as_build_side) }`。
- RustCodeGraph `query match_probe_row` / `node match_probe_row`：确认其调用 `Joiner::try_to_match_inners`，并更新 `matched`、`has_null` 与未完成队列。图索引没有识别本文件 trait 实现内到该方法的调用边，因此该直接调用另由 `semi_join_probe.rs:395-404` 核对。
- 已读生产文件：`semi_join_probe.rs`、`base_join_probe.rs`、`base_semi_join.rs`、`lib.rs`；已读 crate 配置：`pkg/executor/join/Cargo.toml`，并搜索 workspace/上层 Cargo 引用。
- 已读 Go 对照：`semi_join_probe.go`、`base_join_probe.go` 的工厂调用点、`hash_join_v2.go` 的扫描调度调用点。
- 已读独立测试：`semi_join_probe_test.rs`；其覆盖右 build 命中、重复 build key 去重、附加条件 false/NULL、左 build 延迟扫描、NULL key、重复左 build 行和三项非法扫描 panic。已读 `semi_join_probe_test.go`，其覆盖左右 build、有无附加条件、重复键、spill 及 probe 基础场景。
- 全仓 Rust 调用搜索：`new_join_probe(` 的非定义命中均位于 `*_test.rs`，所以文档把当前生产接线状态标记为“未发现”，而不是声称完整 SQL 主链已使用。
- 本任务是纯文档分析，按计划不运行 Cargo；最终仅运行任务指定的 11 章节结构检查，并人工复核没有把注释代码描述成已支持能力。
