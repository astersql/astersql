# `pkg/executor/join/index_lookup_hash_join.rs`

## 文件定位

本文件是 `astersql-executor-join` crate 中 Index Nested Loop Hash Join 的 Rust 实现。模块由 `pkg/executor/join/lib.rs` 以 `pub mod index_lookup_hash_join` 公开，核心公开类型是 `IndexNestedLoopHashJoin`。它把外表按批次组织成索引查找请求，通过 `IndexJoinExecutorBuilder` 获取内表行，再借助 `Joiner` 生成不同连接类型的结果。

当前可执行 Rust 实现位于源文件第 637 行之后；此前的大段注释保存了 Go 版本的结构和迁移线索，并不参与编译。当前实现是同步、单线程、全量物化结果的版本。`pkg/executor/benchmark_test.rs` 直接构造并抽干该执行器；`pkg/executor/builder.rs::buildIndexNestedLoopHashJoin` 则通过 `ExecutorBuilderDependencies::wrap_index_nested_loop_hash_join` 做通用执行器接线，现有直接证据不能证明该依赖注入最终必然构造本文件的具体类型。

crate 边界见 `pkg/executor/join/Cargo.toml`：包名为 `astersql-executor-join`，库入口为 `lib.rs`，移植元数据指向 Go 包 `pkg/executor/join`。本文件实际使用同 crate 的 `index_lookup_join`、`joiner`、`row_table_builder` 模块以及标准库 `HashMap`；Cargo 中大量 `cfg(windows)` 依赖属于整个 join crate，并非都被本文件直接调用。

## 核心职责

1. `IndexHashJoinOuterWorker::build_task` 按指数增长批次切分外表行，提取连接键，过滤含 `NULL` 的键，构造外表哈希表和去重后的索引 lookup 内容。
2. `IndexHashJoinInnerWorker::handle_task` 调用 `IndexJoinExecutorBuilder::build` 获取内表行，并按编码后的连接键把内表行挂到对应外表行下标。
3. `IndexNestedLoopHashJoin::execute` 串行驱动 outer worker、inner worker，并根据 `keep_outer_order` 选择保序或乱序连接流程。
4. `join_in_order` 按外表原始顺序调用 `Joiner::try_to_match_inners`；`join_unordered` 以内表行为驱动探测外表哈希桶，并在探测结束后补齐未匹配外表行。
5. `open`、`next`、`close` 提供轻量生命周期和分页读取接口；第一次需要数据的 `next` 会一次性执行完全部任务并缓存结果。

本文件不负责真实存储访问、索引范围构造或 SQL 表达式实现：索引查找由传入的 `IndexJoinExecutorBuilder` 抽象完成，连接类型、附加条件和未匹配行输出由 `Joiner` 完成。

## 主要符号

- `NUM_RESULT_CHUNKS_HELD: usize = 4`：与 Go `numResChkHold` 对应；当前 Rust 可执行路径没有 chunk 资源池，因此该常量仅保留接口/移植语义，未被执行流程使用。
- `MAX_ROWS_PER_FETCH: usize = 4096`：`build_task` 的单批外表行硬上限，也是指数增长的封顶值。
- `IndexHashJoinResult { rows, error }`：`next` 的返回包装。当前成功路径把错误直接作为 `Result::Err(String)` 传播，代码没有构造 `error: Some(...)`，所以 `error` 字段在当前实现中始终为 `None`。
- `IndexHashJoinTask`：单批任务状态，包含 `outer_rows`、去重的 `lookup_contents`、`outer_hash`、拉取到的 `inner_rows`、保序路径使用的 `matched_inner_rows` 和完成标记 `done`。
- `IndexHashJoinOuterWorker`：拥有全部外表行、外键列下标、当前位置和下一批大小。`new` 校验初始批次非零，`build_task` 逐批推进且不会回退。
- `IndexHashJoinInnerWorker<'a>`：短生命周期借用 builder 和内表键列；`handle_task` 拉取并按键关联内表行。
- `IndexNestedLoopHashJoin`：执行器主体，独占 outer worker、boxed builder、内表键列、`Joiner`、输出缓存和生命周期标志。
- `IndexNestedLoopHashJoin::new`：校验内外连接键列数量相等，并构造 outer worker。
- `support_incremental_lookup`：仅当不保序且 join 类型为 `Inner`、`LeftOuter`、`RightOuter` 或 `AntiSemi` 时返回 `true`，与 Go 的门禁条件一致；当前 `execute` 并未根据返回值进行分批 inner lookup，因此它目前是能力判定接口而非实际增量调度开关。
- `open` / `next` / `close`：生命周期入口；`execute`、`join_in_order`、`join_unordered` 是私有执行阶段。

## 执行流程

构造阶段，`IndexNestedLoopHashJoin::new` 先比较 `outer_key_columns.len()` 与 `inner_key_columns.len()`，再调用 `IndexHashJoinOuterWorker::new` 验证 `initial_batch_size > 0`。成功后执行器尚未打开，输出为空，游标为零。

调用 `open` 时，执行器清空输出、重置结果游标并置 `opened = true`。已经 `close` 的实例会被拒绝重新打开。`next(required_rows)` 在未显式打开时会自动调用 `open`；`required_rows == 0` 会立即返回空结果，不消费 outer worker，也不触发 builder。

第一次非零 `next` 在 `output` 为空且 `cursor == 0` 时调用 `execute`：

1. `build_task` 计算 `end = min(cursor + min(batch_size, 4096), rows.len())`，复制当前批次，推进 cursor，并把下一批大小翻倍且封顶 4096。
2. 对每条外表行调用 `extract_key`。任一键值为 `Value::Null` 时，该行不进入 `outer_hash` 和 `lookup_contents`，但仍保留在 `outer_rows`，后续会走未匹配处理。
3. 非空键经 `encode_key` 成为 `HashMap<Vec<u8>, Vec<usize>>` 的键；同键的多个外表行下标都保留。lookup 内容按 `compare_row` 排序，再按完整 `keys` 相等去重，以减少重复查找。
4. `IndexHashJoinInnerWorker::handle_task` 一次调用 `builder.build(&lookup_contents)` 得到该批全部内表行；随后为每条内表行提取并编码键，把它克隆到所有同键外表行对应的 `matched_inner_rows`，最后置 `done = true`。
5. 保序路径按 `outer_rows` 顺序逐行处理其匹配内表集合；乱序路径按 `inner_rows` 顺序探测 `outer_hash`。两条路径都由 `Joiner` 判定附加条件和连接类型，并对最终未匹配行调用 `on_miss_match`。
6. 所有批次执行完毕后，结果已全部写入 `output`。`next` 仅按 `required_rows` 从该向量复制一段到返回值，后续调用继续推进 `cursor`；耗尽后返回空结果。

`close` 清空输出并设置 `closed = true`、`opened = false`。它不重置 outer worker 的 cursor，因此即使放宽当前的禁止重开校验，也不能自然重放已经消费的输入。

## 数据与状态

`IndexHashJoinOuterWorker` 持有外表输入的所有权。`cursor` 是外表消费进度，`batch_size` 每成功构建一批后翻倍；两者使任务分批单向推进。每个任务复制当前批次的 `Row`，因此任务与 worker 的原始行存储互不借用。

`outer_hash` 的键来自 `encode_key(extract_key(row, key_columns))`，值是当前任务内的外表行下标列表。它保留重复外键对应的全部行；`lookup_contents` 则排序去重，因此一个唯一连接键只发起一个逻辑 lookup。`matched_inner_rows` 长度始终初始化为 `outer_rows.len()`，可按相同下标直接访问；`done` 仅由 inner worker 在成功完成后置真，当前执行器没有单独读取该标志。

保序路径直接使用 `matched_inner_rows`。乱序路径另建与外表行等长的 `matched` 和 `has_null` 向量：每次 `try_to_match_inners` 的结果分别用逻辑或累积，确保同一外表行经过多个内表候选后仍能正确决定是否需要 `on_miss_match`。

执行器把所有结果物化到 `output: Vec<Row>`，`cursor` 只控制对调用者的分页。`opened` 表示已打开，`closed` 是永久关闭门禁。没有“执行完成”独立标志；当前代码用 `output.is_empty() && cursor == 0` 判断是否需要执行，这意味着合法的零结果查询在每次后续非零 `next` 都会再次进入 `execute`，但 outer worker 已耗尽，所以只会做一次空循环。

## 依赖与调用关系

上游关系：

- `pkg/executor/join/lib.rs` 公开本模块，并在 `cfg(test)` 下装配独立测试 `index_lookup_hash_join_test.rs`。
- `pkg/executor/benchmark_test.rs::run_index_nested_loop_hash_join_case` 与 `run_index_join_lane` 直接调用 `IndexNestedLoopHashJoin::new -> open -> next* -> close`，验证可抽干 10,000 行规模结果及基准矩阵中的 OuterHash 路径。
- `pkg/executor/join/index_lookup_hash_join_test.rs` 直接构造执行器并验证 `support_incremental_lookup` 的 join 类型与保序门禁。
- `pkg/executor/builder.rs` 把 `Plan::IndexHashJoin` 分派给 `buildIndexNestedLoopHashJoin`，该函数先构造 lookup join，再调用依赖接口 `wrap_index_nested_loop_hash_join`。这说明 Index Hash Join 位于计划到执行器的构建链上，但具体包装实现不在本文件，不能仅凭该调用边认定生产构建链已使用本类型。

下游关系：

- `index_lookup_join::{extract_key, encode_key, compare_row}` 定义键提取、规范编码和 lookup 内容排序；`IndexJoinLookupContent` 携带键和原始外表行；`IndexJoinExecutorBuilder::build` 是获取内表行的唯一外部数据入口。
- `joiner::{Joiner, JoinType, NaajType, Row}` 提供连接语义。两条 join 路径调用 `try_to_match_inners`，未匹配时调用 `on_miss_match`；本文件固定传入 `NaajType::Unknown`。
- `row_table_builder::Value::Null` 用于识别包含 NULL 的外表连接键。
- 标准库 `HashMap` 保存编码键到外表行下标的多值映射。

RustCodeGraph 对目标文件给出的文件级使用关系是 `pkg/executor/benchmark_test.rs`；独立单测通过 `lib.rs` 的条件模块装配，不一定表现为普通文件导入边。对 `build_task -> extract_key/encode_key/compare_row`、`handle_task -> builder.build/extract_key/encode_key`、`next -> open/execute` 和 `execute -> build_task/handle_task/join_*` 的关系，可由目标文件对应符号直接复核。

## 错误处理与边界

所有可失败的当前 Rust 路径统一使用 `Result<_, String>`，并以 `?` 原样向上传播：构造错误来自键列数量不匹配或零初始批次；执行错误可来自 `extract_key`、builder、键比较相关调用或 `Joiner::try_to_match_inners`。错误没有额外上下文包装，也没有恢复/重试逻辑。

明确边界如下：

- 内外 key 列数量必须相等，但 `new` 不验证列下标是否落在每行范围内；越界行为由 `extract_key` 返回错误。
- 外表含 NULL 的连接键不参与 lookup 或哈希匹配，最终交给 `on_miss_match`；当前代码没有 Go `HashIsNullEQ` 的 null-safe equality 分支。
- inner builder 返回的行若无法按 `inner_key_columns` 提取键，整个执行终止。
- `required_rows == 0` 保证不触发执行或推进状态。
- `close` 后调用 `next` 会因自动 `open` 而返回 `cannot reopen closed index hash join`。
- `IndexHashJoinResult.error` 没有承载当前运行错误；调用者必须检查外层 `Result`。
- `batch_size * 2` 在极端 `usize` 输入下理论上可先溢出再执行 `.min`；正常构造值和 4096 封顶下不会触及该风险，但扩展输入校验时应考虑使用饱和乘法。

Go 版本包含 context 取消、channel 关闭、panic 恢复、failpoint、inner executor 关闭和 memory tracker 清理；这些只存在于文件前半段注释与同路径 Go 源码中，当前 Rust 可执行实现没有等价错误通道或清理分支。

## 并发与资源生命周期

尽管类型沿用 outer/inner worker 命名，当前 Rust 实现没有线程、异步任务、channel、锁或原子状态。`execute` 在调用线程内依次构建任务、同步调用 builder、执行 join；`IndexJoinExecutorBuilder` 要求 `Send + Sync`，但本文件不会并发调用它。`NUM_RESULT_CHUNKS_HELD` 在当前路径中未参与资源控制。

资源所有权由 Rust 值生命周期管理：执行器拥有 boxed builder、joiner、outer worker 和输出向量；临时 inner worker 只在单个任务处理期间借用 builder 与键列。`close` 只清理 `output` 并切换标志，没有显式关闭 builder 的接口，也没有 `Drop` 实现。任务对外表行、内表行和匹配行存在多次克隆，内存峰值可同时包含原始 outer 输入、任务批次、inner 结果、逐外表匹配副本和最终 output。

Go `IndexNestedLoopHashJoin` 的真实模型是一条 outer goroutine 加 N 条 inner goroutine，以 task/result channel、WaitGroup、取消 context、chunk 资源池和内存跟踪器协调；哈希表构建和内表 lookup 还可并行。当前 Rust 版本没有移植这些并发与资源生命周期保证，因此不能依据 Go 设计描述其吞吐、取消或 panic 行为。

## 与 Go 版本的对应关系

同路径 `pkg/executor/join/index_lookup_hash_join.go` 是直接对照实现。名称和主要概念的映射包括：`numResChkHold -> NUM_RESULT_CHUNKS_HELD`、`maxRowsPerFetch -> MAX_ROWS_PER_FETCH`、`indexHashJoinTask -> IndexHashJoinTask`、outer/inner worker、`IndexNestedLoopHashJoin`、保序/乱序 join 以及 `supportIncrementalLookUp` 门禁。

已保持的主要语义：

- outer 侧建哈希、inner 侧索引 lookup 后探测的算法方向；
- 保序模式按外表行组织匹配结果，乱序模式以内表行驱动探测；
- lookup key 去重、重复 outer key 映射到多行；
- 未匹配行统一交给 Joiner，并累计 join 条件的 NULL 结果；
- 增量 lookup 仅允许乱序的 Inner、LeftOuter、RightOuter、AntiSemi 四类 join；
- 批次最大值 4096，以及保序资源常量值 4。

尚未等价或明显简化的部分：

- Go 用一个 outer worker 和多个 inner worker 并发执行，并让建哈希与 inner fetch 并行；Rust 当前完全串行。
- Go 使用 chunk、task/result channel 和逐 worker joiner；Rust 使用 `Vec<Row>` 全量物化和单个 `Joiner`。
- Go 支持 context 取消、panic 恢复、failpoint、运行时统计、内存跟踪、chunk 回收和 inner executor 生命周期；Rust 当前均未实现。
- Go 可真正按 `maxRowsPerFetch` 对 inner 结果做增量拉取；Rust 的 `support_incremental_lookup` 只报告门禁，builder 每个任务仍一次返回全部 inner 行。
- Go 通过编码哈希后再用 `EqualChunkRow` 排除哈希碰撞并支持 `HashIsNullEQ`；Rust 直接以完整编码字节作为 `HashMap` 键，且 NULL outer key 一律跳过。
- Go 的保序路径保存 inner row pointer，避免为每个 outer 匹配复制行；Rust 在 `matched_inner_rows` 中克隆 inner 行。
- Go `Open/Close` 可重置更完整的 worker 状态；Rust `close` 后明确禁止重开，且不会重置 outer cursor。

因此，本文件应被描述为保持核心连接语义的当前 Rust 实现和 Go 迁移落点，而不是 Go 并发执行器的完整等价移植。

## 扩展指南

- 修改外表批处理、NULL 键规则、lookup 排序或去重时，主要接入点是 `IndexHashJoinOuterWorker::build_task`。应在独立文件 `pkg/executor/join/index_lookup_hash_join_test.rs` 增加批次增长/4096 封顶、重复键、NULL 键、无效列下标测试，不能把测试内嵌回生产源文件。
- 修改 inner lookup 或匹配关联时，接入点是 `IndexHashJoinInnerWorker::handle_task` 和 `IndexJoinExecutorBuilder`。需覆盖 builder 错误、inner key 提取错误、一个 key 对多个 outer/inner 行以及匹配行克隆带来的内存风险。
- 增加连接类型或 null-aware 语义时，应同时审查 `support_incremental_lookup`、`join_in_order`、`join_unordered` 与 `pkg/executor/join/joiner.rs`。保序与乱序路径必须对 `matched`/`has_null` 和 `on_miss_match` 保持一致语义，并与 Go `supportIncrementalLookUp`、`doJoinInOrder`、`doJoinUnordered` 对照。
- 若实现真正增量 lookup，不能只修改门禁函数；还需让 builder/inner worker 暴露可续拉状态，并确保只有 inner 数据耗尽后才为 outer 行补未匹配结果，这一点对应 Go `handleTask` 中围绕 `innerExec` 的循环。
- 若移植 Go 并发模型，应先设计任务所有权、取消、worker 错误优先级、结果背压和资源回收，不应把注释中的 channel/goroutine 机械翻译为共享可变状态。特别要保持 Go 的“panic 原因优先于 context cancelled”、每 worker 独立 joiner、范围/比较器深拷贝和任务内存 detach 语义。
- 修改生命周期时，应补充 `open -> next -> close`、隐式 open、零 required rows、空结果重复 next、close 后 next，以及是否允许重新 open 的独立回归测试。
- 若要接入生产计划构建链，应沿 `pkg/executor/builder.rs::buildIndexNestedLoopHashJoin` 检查 `wrap_index_nested_loop_hash_join` 的具体实现，验证返回对象确实包装或构造本类型；不要仅凭 plan 分派名称假定已经接线。
- 性能重点是当前全量 `output`、任务/匹配行克隆和单线程 builder 调用。任何减少克隆或引入流式输出的改动都必须同时验证结果顺序、未匹配行时机和分页边界。

## 验证依据

本说明基于以下直接证据完成事实核对：

- RustCodeGraph 索引状态：项目索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- 目标源码与符号：`pkg/executor/join/index_lookup_hash_join.rs`；查询确认 22 个符号，并逐段读取全部 908 行。主要符号为 `IndexHashJoinResult`、`IndexHashJoinTask`、`IndexHashJoinOuterWorker::{new, build_task}`、`IndexHashJoinInnerWorker::handle_task`、`IndexNestedLoopHashJoin::{new, support_incremental_lookup, open, execute, join_in_order, join_unordered, next, close}`。
- 下游定义：RustCodeGraph `node IndexJoinExecutorBuilder` / `node IndexJoinLookupContent` 定位到 `pkg/executor/join/index_lookup_join.rs`；`node Joiner`、`query try_to_match_inners`、`query on_miss_match` 定位到 `pkg/executor/join/joiner.rs`。
- 上游与装配：`pkg/executor/join/lib.rs`、`pkg/executor/builder.rs::build` 和 `buildIndexNestedLoopHashJoin`、`pkg/executor/benchmark_test.rs::run_index_nested_loop_hash_join_case` / `run_index_join_lane`。
- crate 边界：`pkg/executor/join/Cargo.toml` 的 package、lib、porting metadata 和条件依赖声明。
- Go 对照：RustCodeGraph 逐段读取 `pkg/executor/join/index_lookup_hash_join.go`，重点核对 `Open`、`startWorkers`、`buildTask`、`supportIncrementalLookUp`、`newInnerWorker`、`handleTask`、`doJoinUnordered`、`doJoinInOrder` 及 worker/channel 生命周期。
- 独立 Rust 测试：`pkg/executor/join/index_lookup_hash_join_test.rs::incremental_lookup_matches_go_join_type_and_order_gate`，确认四个允许类型、三个拒绝类型及 `keep_outer_order` 门禁。
- 人工复核结论：当前 Rust 可执行部分的同步物化实现与前半段注释化 Go 迁移线索已明确分离；未把 Go 并发、取消、内存跟踪或增量 inner fetch 描述为 Rust 已支持。

本任务按计划属于纯文档分析，未运行 Cargo。结构验收应使用任务指定命令，确认文件存在且恰有上述 11 个固定二级标题。
