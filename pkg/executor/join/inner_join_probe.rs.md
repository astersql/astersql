# `pkg/executor/join/inner_join_probe.rs`

源文件：[`inner_join_probe.rs`](inner_join_probe.rs)

## 文件定位

本文件属于 `astersql-executor-join` crate，由 [`lib.rs`](lib.rs) 以 `pub mod inner_join_probe` 暴露。它实现 Hash Join 的 Inner Join 探测策略：`InnerJoinProbe` 持有一个 `BaseJoinProbe`，并为它实现统一的 `Probe` trait（`inner_join_probe.rs:121-207`）。

当前接线需要区分两层事实：[`base_join_probe.rs`](base_join_probe.rs) 的 `new_join_probe` 在 `JoinType::Inner` 分支确实构造 `InnerJoinProbe`（`base_join_probe.rs:325-335`），但全仓非测试 Rust 搜索未发现生产代码调用 `new_join_probe`；当前 [`hash_join_v2.rs`](hash_join_v2.rs) 的 `HashJoinV2Exec::fetch_and_probe_hash_table` 直接调用 `ProbeWorkerV2::probe_row`。因此，本文件是已实现、可通过公共工厂构造并有独立测试覆盖的 Probe 策略，不应误述为当前 Rust Hash Join V2 执行器主路径上的实际调用对象。

## 核心职责

- 将装载、恢复、spill、完成判断和重置等通用操作委托给 `BaseJoinProbe`（`set_chunk_for_probe`、`set_restored_chunk_for_probe`、`spill_remaining_probe_chunks`、`is_current_chunk_probe_done`、`reset_probe`）。
- 在 `probe` 中按当前 probe 行和当前候选游标分批取 build 侧候选，使每次返回不超过 `HashJoinContext::max_chunk_size`（`inner_join_probe.rs:143-185`）。
- 交给 `Joiner::try_to_match_inners` 拼接行并计算 other condition；Inner Join 只输出真正满足条件的组合行（`joiner.rs:1395`）。
- 记录候选批次中未形成真实匹配的数量到 `BaseJoinProbe::probe_collision`，并在命中时标记对应 build 行已使用。
- 声明 Inner Join 不需要 probe 后扫描整张 build row table；误入扫描 API 时以 panic 暴露调用方协议错误（`inner_join_probe.rs:187-200`）。

## 主要符号

- `pub struct InnerJoinProbe { pub base: BaseJoinProbe }`：本文件唯一类型。`#[derive(Clone)]` 会连同共享上下文和游标状态做值克隆；字段公开，工厂可直接组装它（`inner_join_probe.rs:123-128`）。
- `impl Probe for InnerJoinProbe`：对外行为边界。装载、恢复、spill、完成判断和重置都是对 `base` 的薄委托；核心专有逻辑集中在 `probe`。
- `probe(&mut self) -> WorkerResult`：从 `current_probe_row/current_candidate` 开始消费候选，返回本批 `rows` 或首个字符串错误。它允许同一 probe chunk 因容量限制被多次调用。
- `need_scan_row_table(&self) -> bool`：恒为 `false`，表达 Inner Join 不输出未命中的 build 行。
- `init_for_scan_row_table`、`scan_row_table`、`is_scan_row_table_done`：均 `panic!("should not reach here")`，与 Go 对照实现保持不可达契约。

本文件没有模块常量、自由函数、条件编译项或固有 `impl InnerJoinProbe`；其公开能力来自公开结构体及 `Probe` trait 实现。

## 执行流程

1. 调用方先经 `set_chunk_for_probe` 装载 probe 行。实际工作由 `BaseJoinProbe::set_chunk_for_probe` 完成：校验 probe key 列下标、序列化 key、计算哈希、排除 NULL key，并在同哈希桶内再次比较完整序列化 key，形成每行的 `matched_rows`（`base_join_probe.rs:188-231`）。
2. `probe` 创建空输出 `rows`，在当前 chunk 尚未完成且输出未达到 `max_chunk_size` 时循环（`inner_join_probe.rs:143-148`）。
3. 它读取 `current_probe_row` 对应的 outer/probe 行，以 `current_candidate` 为起点截取最多为剩余输出容量的候选下标，再从 `context.build_rows` 克隆成 `inners`（`inner_join_probe.rs:149-158`）。
4. `Joiner::try_to_match_inners(&outer, &inners, &mut rows, NaajType::Unknown)` 对每个候选拼接行、计算谓词并投影结果。Inner Join 不使用 NAAJ 分类，所以固定传 `Unknown`（`inner_join_probe.rs:159-164`；`joiner.rs:1395-1444`）。
5. 成功时，`current_candidate` 增加 `result.consumed`；若该批存在匹配则调用 `mark_build_rows_used(index)`；候选耗尽时 `finish_current_lookup_loop` 推进到下一 probe 行并把候选游标清零（`inner_join_probe.rs:165-175`；`base_join_probe.rs:252-255`）。
6. 若 Joiner 返回错误，立即返回已构造的部分 `rows` 和 `Some(error)`，游标保持在本次错误发生前的位置；否则循环结束后返回 `WorkerResult { rows, error: None }`。
7. 当容量使一次调用无法消费全部候选时，下一次 `probe` 从保留的 `current_candidate` 继续；独立测试以 `max_chunk_size = 1` 验证重复 key 跨两次调用输出。

## 数据与状态

`InnerJoinProbe` 自身只保存 `base`。关键可变状态均位于 `BaseJoinProbe`：

- `probe_chunk` 是当前探测批；`matched_rows[probe_row]` 保存已通过哈希和完整 key 比较的 build 行下标。
- `current_probe_row` 指向当前外表行，`current_candidate` 指向该行的下一批候选；二者组成可暂停、可续跑的游标。
- `context.build_rows` 提供候选行实体；`context.joiner` 决定拼接方向、other condition 与列投影；`context.max_chunk_size` 限制单次返回行数。
- `context.build_row_used` 在 `mark_build_rows_used` 中更新。此标记主要服务需要后扫 build 表的其它 join 类型；Inner Join 自己声明无需后扫，但仍沿用共享状态更新。
- `probe_collision` 是累计计数。本实现用 `inners.len() - usize::from(result.matched)` 增量；由于 `matched` 只是“这一批是否至少命中一次”的布尔值，该指标不是逐候选精确的 key 冲突数，尤其在一批含多个成功匹配时会把其余候选计入。扩展或消费该统计前必须确认这一当前语义。
- `spilled_chunks` 位于基类。spill 时保存从 `current_probe_row` 起的整行尾部，而不是从 `current_candidate` 精确保存当前行的剩余候选（`base_join_probe.rs:237-246`）；当前测试只覆盖完成首行后 spill 后续 probe 行。

## 依赖与调用关系

上游关系：

- `base_join_probe::new_join_probe` 是直接构造入口；`JoinType::Inner` 返回 `Box<dyn Probe>` 包装的 `InnerJoinProbe`。
- [`inner_join_probe_test.rs`](inner_join_probe_test.rs) 通过该工厂构造并调用所有核心接口。
- RustCodeGraph 将目标识别为 `inner_join_probe.rs::InnerJoinProbe`，并识别其 `Probe` 方法；其宽泛 callers 查询未在合理时间内返回。全仓窄范围搜索进一步确认，非测试 Rust 中只有工厂构造点，没有该工厂的生产调用点。

下游关系：

- 委托 `BaseJoinProbe::{set_chunk_for_probe,set_restored_chunk_for_probe,spill_remaining_probe_chunks,is_current_chunk_probe_done,finish_current_lookup_loop,mark_build_rows_used,reset_probe}`。
- 读取 `HashJoinContext::{max_chunk_size,build_rows,joiner}` 与基类游标/候选集合。
- 调用 `Joiner::try_to_match_inners`，并传入 `NaajType::Unknown`；错误通过 `WorkerResult.error` 返回。

crate 边界：[`Cargo.toml`](Cargo.toml) 将本目录声明为 `astersql-executor-join`，`lib.rs` 为 crate 根。本文件只使用同 crate 模块，没有直接引用 Cargo 外部依赖；crate 的大部分移植依赖被限定在 `cfg(windows)`，但本模块本身没有平台条件。

## 错误处理与边界

- probe key 下标越界在装载阶段由基类返回 `Err("probe row ... key index out of range")`；本文件原样传播。测试 `inner_join_probe_rejects_out_of_range_probe_key` 固化该边界。
- 上一个 chunk 未处理完时再次普通装载会由基类返回 `previous chunk is not probed yet`。恢复装载当前直接复用普通装载，遵守同一约束。
- NULL join key 在基类建立候选时直接得到空候选，符合等值 Inner Join 不匹配 NULL 的行为；本文件不会为其输出行。
- other condition 的错误由 `Joiner::try_to_match_inners` 返回。本文件保留本次调用已经产生的行并设置 `WorkerResult.error`；调用方必须先处理错误，不能仅消费部分结果。
- `max_chunk_size` 由 `Joiner::new` 保证大于零；若绕过正常构造形成零容量上下文，本循环不会推进，因此应保持构造不变量。
- 三个 row-table 扫描方法故意 panic；只有先检查 `need_scan_row_table == false` 的调度方才可安全使用统一 trait。
- 本文件没有 SQL killer 检查。Go `Probe` 每轮显式调用 `checkSQLKiller`，这是当前 Rust 移植与 Go 行为的明确差异，而非可假定已由本文件覆盖的功能。

## 并发与资源生命周期

`Probe` 的所有操作都要求 `&mut self`，所以单个 `InnerJoinProbe` 实例没有内部并发执行；并行度应由上层为 worker 分配独立实例提供。类型没有锁、通道、异步任务或显式文件句柄，也没有自定义 `Drop`。

生命周期从 `new_join_probe` 构造开始，经 `set_chunk_for_probe`/`set_restored_chunk_for_probe` 装载一批，零次或多次 `probe` 直至 `is_current_chunk_probe_done`，必要时 `spill_remaining_probe_chunks` 取走未处理行，最后可用 `reset_probe` 清理批内缓冲。`reset_probe` 不清空 `build_row_used` 或 build 哈希表，因为它们属于共享连接上下文的构建侧生命周期。

虽然 `InnerJoinProbe` 与 `BaseJoinProbe` 均可 `Clone`，当前字段是普通所有权容器的深层值克隆，并非通过锁共享游标；克隆后两份实例的进度与 used 标记会各自演化。新增并行调用不能把 `Clone` 当作共享一致性机制。

## 与 Go 版本的对应关系

Go 对照文件是 [`inner_join_probe.go`](inner_join_probe.go)，同样以 `innerJoinProbe` 嵌入/持有公共 probe 状态，`NeedScanRowTable` 返回 false，三个扫描方法 panic。两者都按容量分批、处理 other condition，并保留续跑游标这一总体意图。

主要实现差异如下：

- Go 在紧凑 row table 的同哈希链上逐个取 tagged pointer，并用 `isKeyMatched` 二次确认 key；Rust 基类在 `set_chunk_for_probe` 时已经把候选解析成 `Vec<usize>` 并过滤完整 key，本文件消费行下标。
- Go 借助临时 `joinedChk`、virtual-row/incomplete-chunk 标志和 `VectorizedFilter` 批量计算 other condition；Rust 逐候选调用 `Joiner::try_to_match_inners`，谓词与投影都在 Joiner 内完成。
- Go `Probe` 接收 `SQLKiller` 并检查 `killedDuringProbe`；Rust trait 签名没有 killer 参数，本文件也没有等价检查。
- Go 仅在无错误时返回 `ok=true` 并把错误挂到 worker result；Rust 直接返回单个 `WorkerResult`，其中包含行与可选字符串错误。
- Go 的碰撞计数发生在 key 二次比较失败时；Rust 候选已提前通过完整 key 比较，本文件当前用批大小和布尔 `matched` 推算，语义并不等价。

Go 测试 [`inner_join_probe_test.go`](inner_join_probe_test.go) 覆盖不同 key 类型、NULL/非 NULL、build side 方向、used 列、other condition、selection 与多分区组合。Rust 独立测试覆盖重复 key/hash miss、容量续跑、other condition、restore/spill、扫描 panic 与 key 越界，但尚未达到 Go 测试矩阵的类型和布局广度。

## 扩展指南

- 修改 Inner Join 的候选消费、容量或游标推进时，优先改 `probe`，并同步 [`inner_join_probe_test.rs`](inner_join_probe_test.rs)。至少保留重复 key 跨批、空候选、谓词过滤和错误后游标行为的回归用例。
- 修改 key 生成、NULL 或候选集合时，职责实际位于 `BaseJoinProbe::set_chunk_for_probe`；应同步独立的 [`base_join_probe_test.rs`](base_join_probe_test.rs)，不要把公共逻辑复制进本文件。
- 修改拼接方向、other condition、列裁剪或错误语义时，职责实际位于 `Joiner::try_to_match_inners`；应同步 [`joiner_test.rs`](joiner_test.rs) 和本文件的端到端 probe 测试。
- 若要把该策略接入当前 `HashJoinV2Exec` 主路径，应先决定替换/统一 `ProbeWorkerV2::probe_row` 的边界，并补生产执行器级测试；只让工厂可构造不等于完成主路径接线。
- 若追求 Go 对齐，应重点补 SQL killer、碰撞统计定义、类型/NULL/selection/左右 build side 测试矩阵，以及 spill 当前行部分消费的精确恢复语义。不要在本文件中内嵌测试；仓库约定继续使用同目录独立 `*_test.rs`。
- 性能敏感点是候选行 `clone`、临时 `inners: Vec<_>` 分配及逐行谓词计算。优化时必须保留 `current_candidate` 的可续跑不变量，并证明单次输出不超过 `max_chunk_size`。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件在索引中，共 207 行。
- RustCodeGraph `node --file pkg/executor/join/inner_join_probe.rs --offset 1 --limit 500`：核对 `InnerJoinProbe` 及完整 `Probe` 实现。
- RustCodeGraph `query InnerJoinProbe --json --limit 20`：定位 Rust/Go 类型和 Go 的四个直接测试入口。
- RustCodeGraph `callees 'inner_join_probe.rs::InnerJoinProbe' --limit 30`：识别本文件 trait 方法；图只解析出 `probe -> is_current_chunk_probe_done`，因此其余委托关系由已索引源码节点核对。宽泛 `callers` 查询长时间无输出后被中止，未把它当作否定证据。
- 已读生产源码/配置：`pkg/executor/join/inner_join_probe.rs`、`base_join_probe.rs`、`joiner.rs`、`hash_join_v2.rs`、`lib.rs`、`Cargo.toml`。
- 已读对照与测试：`pkg/executor/join/inner_join_probe.go`、`inner_join_probe_test.rs`、`inner_join_probe_test.go`；Rust 测试具体覆盖见“与 Go 版本的对应关系”。
- 全仓非测试 Rust 搜索 `new_join_probe|InnerJoinProbe|ProbeFlavor`：只发现本文件、基类工厂及类型映射，未发现生产调用工厂；这一结果与 `hash_join_v2.rs` 的直接 worker 主链共同支撑“尚未接入当前主路径”的结论。
- 本任务为纯文档分析，按计划不运行 Cargo。交付结构检查要求文档存在且恰有十一个固定二级标题。
