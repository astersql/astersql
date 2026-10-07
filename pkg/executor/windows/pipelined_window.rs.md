# `pkg/executor/windows/pipelined_window.rs`

## 文件定位

本文件实现 `astersql-executor-windows` crate 中的流水线窗口执行器，源文件是 [`pipelined_window.rs`](pipelined_window.rs)，crate 边界由 [`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs) 确定。`lib.rs` 公开 `pipelined_window` 模块并再导出其符号；`builder.rs::build` 在 `force_pipelined || plan.pipelined_enabled` 时构造 `PipelinedWindowExec`，`builder.rs::build_ordered` 则强制走流水线路径并包装成 `OrderedWindowExec`。

它的输入契约是：子执行器已按 `PARTITION BY`/窗口 `ORDER BY` 所需顺序产生 `Chunk`。本文件不负责排序、解析 SQL 或构造窗口函数，而是在按分区读取行的同时，判定每行的帧、维护窗口函数部分状态，并将结果追加到保留了输入列的输出块。

`Cargo.toml` 将 crate 命名为 `astersql-executor-windows`，并用 `package.metadata.porting.go-package = "pkg/executor/windows"` 指向 Go 对照包。其列出的跨 crate 依赖位于 `target.'cfg(any())'.dependencies`；`cfg(any())` 恒为 false，所以当前 Rust 文件实际使用的是本 crate `window.rs` 中的本地执行、帧、函数和内存跟踪抽象，不能把该依赖清单解读为当前已直接接入所有外部 crate。

## 核心职责

1. **按需拉取并切分分区**：`get_rows_in_partition` 通过 `GroupChecker` 将子 `Chunk` 分成连续的分区组，使跨 Chunk 的同一分区可继续处理，并用 `new_partition`/`done` 表示分区或数据源边界。
2. **在数据足够时尽早产出**：`enough_to_produce` 只在当前行帧边界已能确定，或整个分区已读完 (`whole`) 时允许 `produce`，因此不必无条件缓冲整个分区。
3. **计算 ROWS/RANGE 半开帧**：`start_row` 和 `end_row` 把帧表示为 `[start, end)`。ROWS 按行号偏移并用饱和运算避免下溢/上溢；RANGE 从上次探测位置单调向后扫描，通过 `FrameBound::before_start`/`beyond_end` 比较排序键。
4. **滑动更新或安全重建函数状态**：`produce` 先尝试 `WindowFunction::slide`；未初始化或函数不支持滑动时，设置绝对帧起点、reset，再对当前帧执行全量 `update`。
5. **管理行与输出块的生命周期**：`data` 保留待回填/返回的投影 Chunk，`rows` 保留计算帧尚可能需要的行。`accumulated` 和 `dropped` 构成水位条件，保证队首 Chunk 在结果全部填充、且内部行缓冲不再需要它对应的前缀后才移交给上游。
6. **精确跟踪本地与语句级内存**：输出 Chunk、`VecDeque<DataInfo>` 容量、`rows` 及文本堆内存、窗口函数部分结果都计入 `WindowMemoryTracker`；对外移交 Chunk、重置帧/分区或 `close` 时对应释放。

## 主要符号

- `DataInfo { chunk, remaining, accumulated }`：一个待产出 Chunk 的元数据。`remaining` 是尚未填充窗口结果的行数；`accumulated` 是读到该 Chunk 末尾时的全局累计行水位。
- `PipelinedWindowExec`：主状态机。公开的生命周期入口是 `open(&ExecContext)`、`next(&ExecContext, &mut Chunk)` 和 `close()`；`memory_bytes()` 提供可观测的本地记账值。
- `OrderedWindowExec { inner }`：薄包装，`open`/`next`/`close` 原样委托给 `PipelinedWindowExec`。“Ordered”是构建器和子计划已满足排序属性的契约，本包装自身不执行排序检查。
- `open_self()`：重置所有分区、水位、滑动帧和缓冲状态，调用每个 `WindowFunction::reset`。它不单独打开 child；`open` 先打开 child 和内存跟踪器，再调用它。
- `first_result_chunk_not_ready()`：队首输出块的门禁。必须同时满足 `remaining == 0` 与 `accumulated <= dropped` 才能弹出，对应“结果已填完”和“内部不再依赖该前缀行”。
- `get_rows_in_partition()` / `fetch_child()`：前者消费 `GroupChecker` 的下一组并追加到 `rows`；后者拉取 child Chunk，投影前 `input_columns` 列形成待输出块，记录水位和内存。
- `start_row()` / `end_row()`：计算当前行帧的绝对起点与半开终点。`last_*` 是上一已产出帧，`staged_*` 是 RANGE 探测到但尚可因数据未读全而未完成的候选边界。
- `produce(data_index, remained)`：为指定 `data` Chunk 最多产出 `remained` 行结果，处理空帧、忽略帧的函数、滑动/重建、结果追加和前缀裁剪。
- `enough_to_produce()`：数据就绪判定。分区完整时，只要还有当前行就能产出；未完整时，起止边界都必须落在已缓冲水位前。
- `reset_partition()`：切换分区，丢弃当前分区剩余缓冲，重置帧游标并释放函数部分结果内存。
- `refresh_rows_memory()` / `value_heap_memory_usage()`：重算 `rows` 向量、每行 `Value` 向量和 `Value::Text` 容量的持有内存，将差额记入跟踪器。

## 执行流程

1. `builder.rs::build` 根据计划函数数量求得输入列数，构造 `WindowMemoryTracker`、`FrameBound`、`GroupChecker` 及窗口函数集合。RANGE 帧在此通过 `FrameBound::update_compare_cols` 绑定 `ORDER BY` 比较列。
2. `open` 先调用 `ChildExecutor::open`，再把 `WindowMemoryTracker` 挂到 `ExecContext::statement_memory_tracker`，最后由 `open_self` 建立空的状态机。任一步返回 `Err` 都立即传播。
3. 每次 `next` 先 `output.reset()`，然后在队首块尚未安全返回时循环。它先调 `enough_to_produce`；若数据不足且 child 未耗尽，则由 `get_rows_in_partition` 拉取/切分下一组行。
4. `fetch_child` 对非空 child Chunk 更新 `accumulated`，创建仅含输入列的拥有型投影 Chunk，以 `DataInfo` 入队，保存原 Chunk 供分区行拷贝，并记账 Chunk 及队列容量。`None` 或空 Chunk 均表示读尽。
5. `GroupChecker::split_into_groups` 根据分区列构造连续 `[begin, end)` 组，并告知首组是否延续上一 Chunk 的末组。`get_rows_in_partition` 将下一组拷贝进 `rows`，先记入 `rows_to_consume`；`next` 在处理完新分区边界后再将其并入 `row_count`。
6. 当 child 耗尽或遇到新分区，`finish` 将当前分区标为完整，使尾部行即使帧终点超出现有行也可按分区末尾截断后产出。若已无可产出行，`reset_partition` 清空旧分区，然后再接纳 `rows_to_consume` 中已预取的新分区行。
7. `produce` 对每个当前行求 `[start, end)` 并截断到 `row_count`。空帧 (`start >= end`) 对依赖帧的函数只在首次进入连续空帧时 reset，再取函数的默认结果；`ignores_frame()` 的函数直接取结果，保留如行号这类按分区位置推进的语义。
8. 非空帧将绝对下标减去 `row_start` 转为 `rows` 相对下标。若滑动状态已初始化，先调 `slide`；返回 `false` 时则 reset 并对完整帧 `update`。结果由 `Chunk::append_results` 追加到正确输出行，然后推进 `current_row` 与上一帧边界。
9. 一批产出结束后，`min(current_row, last_end_row, last_start_row)` 给出帧与当前行都不再使用的前缀。该前缀从 `rows` 排出，`dropped`/`row_start` 前移；因为 `slide` 参数是相对当前 `rows` 的偏移，裁剪后必须将 `initialized_sliding_window` 置假，下一帧全量重建。
10. 队首 `DataInfo` 就绪后，`next` 先从窗口内存跟踪器扣除该 Chunk，再用 `swap_columns` 把所有权移给 `output`。队列变空时还扣除 `VecDeque` 容量记账。后续一次 `next` 会继续拉取；完全结束时返回空 Chunk。

## 数据与状态

| 状态组 | 字段 | 不变量/用途 |
| --- | --- | --- |
| 构建期依赖 | `child`, `input_columns`, `window_functions`, `start`, `end`, `group_checker`, `order_by`, `range_frame` | 由 `builder.rs::build` 一次组装；执行期不更换计划形状。 |
| 输入/输出块 | `child_result`, `data`, `data_index` | `child_result` 是最近一批分区切分来源；`data_index` 指向当前回填块，队首弹出后减一。 |
| 全局水位 | `accumulated`, `dropped` | `accumulated` 只随 child 行增加，`dropped` 只随前缀裁剪/分区重置增加。`DataInfo.accumulated <= dropped` 说明内部已不依赖该块覆盖的行。 |
| 分区边界 | `done`, `new_partition`, `rows_to_consume`, `whole` | `rows_to_consume` 可暂存已预取的新分区行；先 finish 旧分区，reset 后才将其并入新分区 `row_count`。 |
| 行缓冲 | `rows`, `row_start`, `row_count` | `row_count` 是当前分区的逻辑累计行数；`rows[absolute - row_start]` 是对绝对分区行号的映射。仅前缀可裁剪。 |
| 帧游标 | `current_row`, `last_start_row`, `last_end_row`, `staged_start_row`, `staged_end_row` | 在有序输入前提下单调推进；RANGE 扫描借此避免每行从分区起点重扫。 |
| 函数状态 | `empty_frame`, `initialized_sliding_window` | 连续空帧不重复 reset；只有前一非空帧的相对偏移仍有效时才可 slide。 |
| 内存记账 | `memory_tracker`, `data_memory`, `rows_memory` | `data_memory`/`rows_memory` 是上次容量计算的快照，每次只向 tracker 提交差额；函数状态由 tracker 的按索引记账管理。 |

`Value` 是 `window.rs` 中的拥有型枚举，`Row = Vec<Value>`，`Chunk` 也持有 `Vec<Row>`。因此 Rust `fetch_child` 的 `projected` 会 clone 输入值，`get_rows_in_partition` 会 clone 当前分区切片；这与 Go 版依赖 `chunk.Row`/列引用的内存所有权细节不同，但也使 Rust 内部不会持有指向上游可复用 Chunk 的借用。

## 依赖与调用关系

**上游调用者**

- `builder.rs::build` 是 `PipelinedWindowExec` 的直接构造者，返回 `WindowExecutor::Pipelined`。`WindowExecutor::{open,next,close,memory_bytes}` 将枚举调用分派到本执行器。
- `builder.rs::build_ordered` 以 `force_pipelined = true` 调用 `build`，再封装为 `OrderedWindowExec`。本文件的 `OrderedWindowExec::{open,next,close}` 委托给 `inner`。
- `lib.rs` 声明并再导出本模块；本文件则从 `window.rs` 导入共享类型和辅助函数。精确的构造入口位于 `builder.rs`，不应根据文件级索引摘要推导更多运行时调用者。

**下游依赖与关键边**

- `open -> ChildExecutor::open -> WindowMemoryTracker::open -> open_self`；`close -> clear/reset -> WindowMemoryTracker::close -> ChildExecutor::close`。
- `next -> first_result_chunk_not_ready/enough_to_produce/get_rows_in_partition/finish/reset_partition/produce`，是状态机核心调度边。
- `get_rows_in_partition -> fetch_child -> ChildExecutor::next`，以及 `get_rows_in_partition -> GroupChecker::{split_into_groups,next_group}`。
- `start_row -> FrameBound::before_start`，`end_row -> FrameBound::beyond_end`；两者都经 `row()` 从前缀裁剪后的缓冲取绝对行。
- `produce -> WindowFunction::{ignores_frame,slide,set_window_start,result}`；重建路径经 `reset_partial_result_and_release_memory` 和 `update_partial_result_and_track_memory`，结果经 `Chunk::append_results` 写入。
- `reset_partition -> reset_partial_results_and_release_memory`，确保分区间不共享聚合状态。

RustCodeGraph 精确符号查询确认：`open_self` 的调用者是本文件 `open`；`get_rows_in_partition` 的调用者是 `next`；`start_row`/`end_row` 均由 `produce` 和 `enough_to_produce` 调用；`produce`、`reset_partition`、`enough_to_produce` 的直接上游都是 `next`（`produce` 还自行重查 `enough_to_produce`）。

## 错误处理与边界

- 本文件所有可失败执行路径统一使用 `window.rs::Result<T>`，`?` 传播 child 打开/拉取/关闭、分区键读取、RANGE 比较、窗口函数更新/滑动/取结果和输出行索引错误。文件不捕获或吞掉这些错误。
- `builder.rs::build` 在窗口函数数量超过 schema 列数时先拒绝构建；RANGE 帧缺少 `ORDER BY` 列也在构建期返回错误，从而避免本文件带着不完整边界元数据运行。
- ROWS 起点用 `saturating_sub`/`saturating_add`，终点 FOLLOWING 连续使用饱和加；`produce` 再把起止截断到 `row_count`。这保证分区首尾超界帧能转换为合法的半开范围。
- `start >= end` 是 SQL 允许的空帧，不是错误。依赖帧的函数在 reset 状态上返回自身空输入语义，例如已有测试约束 `SUM -> NULL`、`COUNT -> 0`；`ignores_frame` 的行号函数仍继续推进。
- `fetch_child` 把 `None` 和零行 Chunk 都当作 EOF。因此 `ChildExecutor` 不应用“中间空 Chunk”表示暂无数据；否则会被视为永久耗尽。
- `get_rows_in_partition` 对 `child_result.as_ref().unwrap()` 的使用依赖内部不变量：只有 `fetch_child` 成功取得非空 Chunk 后才调用 `split_into_groups`/`next_group`。`row()` 的索引也依赖 `row_start <= absolute < row_count` 且对应行未被裁剪。这些是内部状态机契约，若未来新增公开调用顺序或修改裁剪公式，需避免将其变成 panic 路径。
- `enough_to_produce` 在分区未完整时要求 `end < row_count` 而非 `<=`，与 Go `enoughToProduce` 一致；这会保留额外的前瞻行，直到后续行或分区边界证明帧已完整，不应在未证明等价时改成宽松条件。

## 并发与资源生命周期

`PipelinedWindowExec` 是单调用者、可变借用驱动的状态机：其方法使用 `&mut self`，没有内部任务、线程、channel 或异步边界。`ChildExecutor: Send` 与 `WindowFunction: Send` 允许拥有它们的执行器整体被移动到其他线程，但不表示同一实例可并发调用 `next`。

`WindowMemoryTracker` 内部用 `Arc<Mutex<WindowMemoryState>>` 和原子计数器以支持 clone 后的共享记账，但本执行器对行、帧和函数状态的推进仍是串行的。`open` 将 tracker 连接到语句级 parent 并计入函数初始状态；`close` 清空所有缓冲，再由 tracker 一次性从 parent 扣回尚存的本地字节，最后关闭 child。

输出 Chunk 的资源移交是生命周期的关键：在 `swap_columns` 之前从 tracker 扣除它的 `memory_usage`，输出后由调用者持有，不再归窗口执行器记账。相反，未弹出 `data` 的 Chunk、`rows` 和窗口函数部分状态始终由本执行器负责。`window_memory_test.rs` 验证运行时本地值与语句 parent 一致、输出移交后不重复记账，以及 `close` 后两者均归零。

## 与 Go 版本的对应关系

直接对照文件是 [`pipelined_window.go`](pipelined_window.go)。主要一一对应如下：

| Rust | Go | 对应语义 |
| --- | --- | --- |
| `DataInfo` | `dataInfo` | 待回填 Chunk、剩余行数、累计水位 |
| `PipelinedWindowExec` | `PipelinedWindowExec` | 分区/帧/水位/缓冲状态机 |
| `OrderedWindowExec { inner }` | `OrderedWindowExec { *PipelinedWindowExec }` | 有序流水线薄包装 |
| `open_self`, `first_result_chunk_not_ready`, `next` | `OpenSelf`, `firstResultChunkNotReady`, `Next` | 初始化、队首水位门禁、主调度循环 |
| `get_rows_in_partition`, `fetch_child` | `getRowsInPartition`, `fetchChild` | 按分区切片拉取并建立待输出 Chunk |
| `start_row`, `end_row` | `getStart`, `getEnd` | ROWS/RANGE 帧边界与 staged 单调扫描 |
| `produce`, `enough_to_produce` | `produce`, `enoughToProduce` | 帧计算、滑动/重建、产出就绪判断 |
| `reset_partition` | `reset` | 分区边界清理与部分结果释放 |

两版保留的核心不变量包括：队首块必须结果填完且内部不再引用才可返回；新分区行可预取但须先结束/重置旧分区；未读完分区时不提前产出未能确定的帧；空帧只在进入时 reset；滑动不可用时全量重建；通过前缀裁剪控制内存。

当前 Rust 适配层的主要差异也必须保持可见：

- Go 结合 `BaseExecutor`、TiDB `chunk.Chunk`、`aggfuncs.PartialResult`、表达式比较函数和 `sessionctx`；Rust 使用 `window.rs` 中的 `ChildExecutor`、拥有型 `Chunk/Row/Value`、`WindowFunction` 和简化 `ExecContext`。
- Go `copyChk` 为输入列建立列引用，并因上游可复用 Chunk 而需要水位保护；Rust `Chunk::projected` clone 值，但仍保留同样的队首水位算法与输出顺序。
- Go 的 RANGE 边界由 planner 生成的 `CmpFuncs`/`CalcFuncs` 和 `expectedCmpResult` 驱动；Rust `FrameBound::candidate_cmp_boundary` 在本地 `Value` 上执行数值/排序比较。因此新增复杂 SQL 类型、collation 或表达式偏移时，不能只修本文件，还需核对 `FrameBound` 的语义覆盖。
- Rust `WindowFunction::ignores_frame` 显式让按分区位置计算的函数跳过帧 reset/update；Go 由其聚合/窗口函数实现与构建装配维持等价语义。
- Rust 在前缀 `drain` 后强制取消滑动初始化，因为它的 `slide` 接口接收相对 `rows` 的下标；Go 滑动接口使用绝对帧位置和 `getRow` 回调，不需要这一 Rust 适配步骤。

Go 回归面主要在 `window_executor_test.go` 与 `window_sql_test.go`；Rust 对应的独立测试文件是 `window_executor_test.rs`、`window_sql_test.rs`、`window_memory_test.rs` 和 `window_test.rs`。Rust 测试同时跑 buffered/pipelined 并比较分区行号、ROWS/RANGE、滑动函数、非对称/空帧、跨 Chunk `LAG`、有序构建和内存释放，但它基于本地简化执行模型，不等同于 Go SQL 层所有类型、planner 与 session 集成已由 Rust 端完整覆盖。

## 扩展指南

- **改变调度或预取策略**：首先检查 `next`、`first_result_chunk_not_ready`、`enough_to_produce`、`get_rows_in_partition` 四者的联动。必须保持 `remaining` 与 `accumulated/dropped` 双门禁，否则可能返回未填完块、提前遗忘帧行或让队列无法前进。
- **新增帧类型或边界语义**：主要接入点是 `start_row`/`end_row`，但构建期元数据在 `builder.rs::build`，值比较在 `window.rs::FrameBound`。必须同步 Go `getStart`/`getEnd` 与 planner 语义，并覆盖 ASC/DESC、NULL、多 ORDER BY 列、分区首尾和超界偏移。
- **新增窗口函数或滑动优化**：在 `window.rs::WindowFunction` 实现 `reset/update/result`，可选实现 `slide`、`set_window_start`、`ignores_frame` 和内存用量方法。`produce` 的原则是 slide 返回 `false` 必须可全量重建；任何持有堆内存的新状态都必须正确报告 `partial_result_memory_usage`。
- **扩展 `Value` 或改变 Chunk 所有权**：同步审查 `Chunk::memory_usage`、`refresh_rows_memory` 和 `value_heap_memory_usage`。当前本文件只额外计算 `Value::Text` 的 heap capacity；新增拥有堆数据的变体时若遗漏，语句级限额会低估。
- **改变前缀裁剪**：只能丢弃同时早于 `current_row`、`last_start_row` 和 `last_end_row` 的前缀，且需同步 `dropped`、`row_start`、`rows_memory` 和滑动状态失效。性能优化不应牺牲队首 Chunk 的安全水位。
- **改变生命周期**：`open`/`close` 必须保持 child 与 statement tracker 成对。错误路径若新增持有资源，要明确由调用者仍然调用 `close` 清理，或在返回错误前就地回滚。
- **测试放置**：不要在本生产文件内内嵌测试。执行器状态机/有序构建回归放在 `window_executor_test.rs`；ROWS/RANGE、函数与空帧语义放在 `window_sql_test.rs`；内存归属放在 `window_memory_test.rs`；底层帧比较可放在 `window_test.rs`。同时核对并必要时扩展对应 Go 独立测试，避免 Rust 适配逻辑偏离 Go 行为。
- **风险重点**：正确性风险集中在分区跨 Chunk、空帧、RANGE 比较和裁剪后偏移；兼容性风险集中在 Go/TiDB 表达式与简化 Rust `Value` 语义差异；性能风险集中在 `projected`/`extend_from_slice` clone、`Vec::drain` 前缀移动以及频繁全量重建。

## 验证依据

**RustCodeGraph 证据**

- `rustcodegraph status`：索引有效，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/windows` 列出本文件及相关 Rust/Go/测试文件。
- `node --file pkg/executor/windows/pipelined_window.rs --offset 1 --limit 420` 与 `--offset 421 --limit 180`：覆盖目标文件全部 518 行、两个结构体与所有方法。
- `query PipelinedWindowExec`、`query open_self/get_rows_in_partition/start_row/end_row/reset_partition/memory_bytes`：确认 Rust 定义及 Go 同名对照。
- `node open_self`：确认其由 `open` 调用；`node get_rows_in_partition`：确认由 `next` 调用，下游含 `fetch_child`、`GroupChecker::split_into_groups/next_group`、`refresh_rows_memory`。
- `node start_row` 与 `node end_row`：确认上游均为 `produce`/`enough_to_produce`，下游分别为 `FrameBound::before_start`/`beyond_end`。
- `node pipelined_window.rs::next`、`node produce`、`node enough_to_produce`、`node reset_partition`、`node pipelined_window.rs::fetch_child`：确认主调度边、帧计算与函数调用边、分区重置边和 child 拉取边。
- `node --file builder.rs`、`node --file window.rs`：核对构建条件、RANGE 元数据绑定、`Chunk`、`FrameBound`、`WindowFunction`、`WindowMemoryTracker`、`GroupChecker` 与部分结果辅助函数。

**直接读取的配置、Go 和测试证据**

- `pkg/executor/windows/Cargo.toml`：crate 名、`lib.rs` 入口、Go 对照包和 `cfg(any())` 下的移植依赖清单。
- `pkg/executor/windows/lib.rs`：模块公开/再导出关系以及四个独立 Rust 测试模块的声明。
- `pkg/executor/windows/pipelined_window.go`：Go 实现的完整状态机，包括队首引用安全注释、拉取/分区、ROWS/RANGE、滑动、空帧、裁剪与 reset。
- `pkg/executor/windows/window_executor_test.rs` 与 `.go`：分区 `ROW_NUMBER`、滑动 `SUM`、buffered/pipelined 等价、强制 ordered pipeline 以及空帧可空性/默认结果。
- `pkg/executor/windows/window_sql_test.rs` 与 `.go`：跨 Chunk `LAG`、ROWS/RANGE 滑动函数、非对称与反向边界、ASC/DESC 及 issue 45964/46050 的空帧回归。
- `pkg/executor/windows/window_memory_test.rs`：流水线与缓冲执行器对 statement tracker 的记账、输出所有权移交和 close 释放。
- `pkg/executor/windows/window_test.rs`：RANGE `CURRENT ROW` 对相邻大整数 peer group 的比较回归，支撑边界比较不应通过损失精度的简化路径改写。

本任务是纯文档分析，按计划不运行 Cargo。结构验证要求本文件存在且恰好含有上述 11 个固定二级标题；验证命令与退出码记录于任务交付。
