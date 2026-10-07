# `pkg/executor/windows/builder.rs`

## 文件定位

`builder.rs` 是 `astersql-executor-windows` crate 内把窗口执行计划装配为可运行执行器的入口。模块由 [`lib.rs`](./lib.rs) 公开为 `builder` 并整体再导出；crate 边界和 Go 包映射由 [`Cargo.toml`](./Cargo.toml) 声明。它不解析 SQL，也不创建具体窗口函数，而是接收已经包含列下标、窗口函数对象和帧定义的 `PhysicalWindowPlan`，在流水线执行器 `PipelinedWindowExec` 与缓冲式执行器 `WindowExec` 之间选择并初始化状态。

当前接线证据应谨慎理解：RustCodeGraph 将该文件识别为 14 个符号、217 行，并报告直接文件使用者为 `pkg/executor/benchmark_test.rs` 与 `pkg/executor/windows/window_memory_test.rs`；crate 内的 `window_executor_test.rs` 还通过 `super::*` 使用其公开项。仓库搜索没有发现 Rust 生产 SQL 执行主链直接构造 `PhysicalWindowPlan`，所以本文只确认该 crate 的可执行实现与测试/基准入口，不宣称它已经替代 Go 主链中的 `windows.Build`。

## 核心职责

该文件承担四项集中职责：

1. 用 `PhysicalWindowPlan` 定义构建阶段所需的最小物理信息：输出列总数、分区列、排序列、窗口函数、可选帧以及流水线开关。
2. 用 `WindowExecutor` 统一包装两种具体执行器，并把 `open`、`next`、`close` 和 `memory_bytes` 原样分派到底层实现。
3. 在 `build` 中验证输入/输出列数，创建共享的 `WindowMemoryTracker`，再按 `force_pipelined || plan.pipelined_enabled` 选择执行路径。
4. 按帧类型完成关键装配：无帧表示整分区聚合，`ROWS` 使用行偏移处理器，`RANGE` 先绑定 `ORDER BY` 比较列再使用值域处理器；`build_ordered` 则强制得到流水线路径并套上 `OrderedWindowExec`。

它刻意不负责窗口函数本身的计算、分区消费或内存释放算法；这些行为分别位于 [`window.rs`](./window.rs) 和 [`pipelined_window.rs`](./pipelined_window.rs)。

## 主要符号

- `PhysicalWindowPlan`：构建输入。`schema_columns` 是输入列与窗口结果列的总和；`partition_by` 和 `OrderBy::column` 都是行内列下标；`window_functions` 持有可变的 `Box<dyn WindowFunction>`；`frame == None` 表示整分区；`pipelined_enabled` 是普通构建的路径开关。结构体没有 `Clone`，因为窗口函数对象和子执行器在构建时被所有权移动。
- `WindowExecutor::{Pipelined, Buffered}`：两条实现的和类型。其生命周期方法只做枚举分派，不增加重试、转换或错误包装；`memory_bytes` 同样读取对应执行器的窗口内存跟踪值。
- `build_ordered(plan, child) -> Result<OrderedWindowExec>`：调用 `build(plan, child, true)` 强制流水线，然后把 `PipelinedWindowExec` 包装为有序执行器。若内部将来违反该约束返回缓冲式变体，会返回固定错误 `ordered window must be built with pipelined window executor`。
- `build(plan, child, force_pipelined) -> Result<WindowExecutor>`：核心装配函数。它消耗计划和 `Box<dyn ChildExecutor>`，计算输入列数、建立内存跟踪器、准备帧边界，并用显式初值构造具体执行器的状态机。
- 本文件没有模块级常量、trait、条件编译项或私有辅助函数；公开 API 就是上述计划类型、枚举及两个构建函数和枚举方法。

## 执行流程

`build` 的流程如下：

1. 读取 `plan.window_functions.len()` 作为结果列数，并收集每个函数的 `initial_partial_result_memory_usage()` 创建 `WindowMemoryTracker`。
2. 用 `schema_columns.checked_sub(function_count)` 计算 `input_columns`。这保证后续 `Chunk::projected(input_columns)` 不会把窗口结果列误当成子执行器输入；若结果函数数超过 schema 列数则立即失败。
3. 计算 `pipelined = force_pipelined || plan.pipelined_enabled`。
4. 若走流水线：取走 `plan.frame`。无帧时合成为 `UNBOUNDED PRECEDING .. UNBOUNDED FOLLOWING`；有帧时保留起止边界，并记录是否为 `RANGE`。`RANGE` 会对起止边界调用 `FrameBound::update_compare_cols(&plan.order_by)`。随后构造 `PipelinedWindowExec`，把分区检查器、排序键、函数、子执行器和内存跟踪器移入，并将水位、队列、分区及滑动窗口状态全部初始化为空/零/`false`。
5. 若走缓冲路径：`frame == None` 选择 `AggWindowProcessor`；`FrameType::Rows` 选择 `RowFrameWindowProcessor`；其余（当前枚举即 `Range`）先绑定两端比较列，再选择 `RangeFrameWindowProcessor`。最后构造 `WindowExec`，初始结果队列为空、`executed == false`。
6. 调用方随后通过 `WindowExecutor::open/next/close` 驱动实际状态机。缓冲路径在 `window.rs::WindowExec` 中跨 Chunk 拼齐分区后计算；流水线路径在 `pipelined_window.rs::PipelinedWindowExec` 中边读取、边判断帧是否足够、边产出并丢弃不再需要的前缀。

`build_ordered` 的额外不变量是 `force_pipelined == true`，因此即使计划自身关闭流水线也应得到 `OrderedWindowExec`；`window_executor_test.rs::go_test_build_ordered_window_exec_returns_ordered_pipeline` 验证了这一点及分区内输出顺序。

## 数据与状态

构建阶段最重要的数据不变量是 `schema_columns = input_columns + window_functions.len()`。本文件只检查不会下溢，并不验证每个分区列、排序列或窗口函数参数列是否落在输入列范围内；这些下标在实际分组、比较或函数更新时才可能报错。

`WindowMemoryTracker` 由两条路径及其处理器共享同一个克隆句柄。它在构建时记录每个窗口函数声明的初始部分结果内存；真正与 `ExecContext::statement_memory_tracker` 建立父子关系发生在执行器 `open`，缓冲队列、流水线行缓冲及动态部分结果随后增减该计数，`close` 清零并解绑。`window_memory_test.rs` 同时覆盖了两条路径在打开、产出、关闭时的计费与释放，以及输出 Chunk 移交后不继续计入窗口执行器的约定。

两条执行器都持有并独占 `child` 与窗口函数对象。缓冲式处理器的状态集中在 `current_row`、上一 RANGE 起止偏移和 `initialized_sliding_window`；流水线执行器还维护待输出 `VecDeque`、已读/已丢弃水位、当前分区行缓冲和 staged 边界。这里的显式初值必须与各自 `open`/分区重置逻辑保持一致，否则复用执行器时可能泄漏上次执行状态。

## 依赖与调用关系

上游入口与验证调用包括：

- `lib.rs` 通过 `pub mod builder` 暴露模块，并用 `pub use builder::*` 再导出 API。
- `window_executor_test.rs` 直接调用 `build`、`build_ordered`，并统一经 `WindowExecutor::open/next/close` 拉取输出。
- `window_memory_test.rs` 调用 `build` 后检查两种变体的 `memory_bytes` 与语句级计数。
- `pkg/executor/benchmark_test.rs` 以 `astersql_executor_windows::builder::{PhysicalWindowPlan, build as build_window}` 使用该 crate，覆盖多种窗口基准场景；这是跨 crate 的直接调用证据，但仍属于测试/基准代码。

下游依赖包括：

- `window.rs` 提供计划字段使用的 `OrderBy`、`WindowFrame`、`WindowFunction`，以及 `WindowExec`、三种 `WindowProcessor`、`GroupChecker`、`FrameBound` 和 `WindowMemoryTracker`。
- `pipelined_window.rs` 提供 `PipelinedWindowExec` 与仅做委托包装的 `OrderedWindowExec`。
- `std::collections::VecDeque` 用来初始化两类执行器的待处理/待输出队列。

`Cargo.toml` 将库入口设为 `lib.rs`，并记录 Go 包为 `pkg/executor/windows`。其中迁移依赖和开发依赖都位于 `target.'cfg(any())'` 下；`cfg(any())` 恒为假，因此这些声明当前不会参与正常 Rust 构建。`builder.rs` 的实际实现只依赖同 crate 模块和标准库，这也说明它目前是自包含的简化执行模型，而不是直接绑定列于清单中的 planner/session/expression crates。

## 错误处理与边界

本文件返回 `window.rs::Result<T>`，错误不会吞掉或降级：

- schema 列数少于窗口函数数时，`checked_sub` 返回 `window function count exceeds schema columns`。
- 任一 `RANGE` 帧缺少 `ORDER BY` 时，起点第一次调用 `update_compare_cols` 就返回 `RANGE frame requires an ORDER BY column`；起点成功而终点失败时也会原样传播。
- `build_ordered` 对意外的缓冲式结果返回专用错误，而不把它伪装成有序执行器。
- 构建成功不代表所有列下标有效。越界的 `partition_by` 会在 `GroupChecker::key` 中报错，越界排序列会在 RANGE 比较中报错，窗口函数输入列错误由具体函数在执行时报告。
- `frame == None` 的含义不是“空帧”，而是整分区窗口：流水线侧显式改写为双无界边界，缓冲侧选择聚合处理器。真正的空帧由运行期计算出的起点不早于终点表示；`window.rs::calculate_frames` 会把终点提升到至少等于起点，避免无效切片。

窗口函数列表可以为空：当前代码会得到 `input_columns == schema_columns` 和空的内存明细，并仍构造执行器；本文件没有额外拒绝该计划。是否允许这种物理计划属于上游契约，当前生产主链接线未验证。

## 并发与资源生命周期

该构建器本身不创建线程、异步任务、通道、锁或事务。所有权通过 `Box<dyn ChildExecutor>`、`Vec<Box<dyn WindowFunction>>` 和枚举变体单向移入执行器，因此单个执行器由调用方以 `&mut self` 串行驱动；文件中没有声明执行器可跨线程共享。

唯一共享状态是 `WindowMemoryTracker` 内部的 `Arc<Mutex<...>>`，它使执行器与处理器的多个句柄更新同一份局部/语句级内存账本；底层 `MemoryTracker` 使用 `Arc<AtomicI64>`。这提供计数更新的同步能力，不等于窗口状态机支持并发调用。

正常生命周期为 `build -> open -> next* -> close`。`open` 打开子执行器并把内存跟踪接到语句上下文；`next` 逐块产出；`close` 清空缓冲、重置分组器、释放窗口内存并关闭子执行器。错误由底层方法返回，`WindowExecutor` 不自动调用 `close`，因此调用方仍应在结束或错误处理路径显式关闭资源。`window_memory_test.rs` 提供了关闭后两级计数归零的直接证据。

## 与 Go 版本的对应关系

同路径 [`builder.go`](./builder.go) 是结构和分支选择的主要语义来源：Go `BuildOrdered` 同样强制 `Build(..., true)` 并要求得到 `PipelinedWindowExec`；Go `Build` 同样按强制/会话开关选择流水线，并按无帧、`ROWS`、`RANGE` 选择处理器，且 RANGE 两端都调用比较列初始化。

Rust 版本不是 Go 函数的逐参数翻译。Go `Build` 接收 `sessionctx.Context`、`physicalop.PhysicalWindow` 与通用 `exec.Executor`，并在构建器内完成以下工作：创建 `BaseExecutor`，从表达式提取分区/排序列，把窗口描述符构造成 `aggfuncs.AggFunc`，分配 partial result，根据会话变量读取流水线开关，并配置 Go RANGE 比较方向/类型信息。Rust 把这些结果前移到 `PhysicalWindowPlan` 和已构造的 `WindowFunction` trait object，只保留列下标与 `OrderBy::descending`，内存初值也由函数方法提供。

因此两者保持的是执行路径与帧装配意图，而不是完整类型体系或接线等价。Go 测试 `window_executor_test.go::TestWindowExecutorsBasic`、`TestBuildOrderedWindowExec` 和 `TestWindowReturnColumnNullableAttribute` 是对应行为来源；Rust 的 `window_executor_test.rs` 用简化 Chunk/计划重建了缓冲与流水线结果、有序强制路径和空帧结果约定。完整 SQL、表达式上下文、nullable 元数据以及 Go session/planner 集成不由本文件实现，不能从这些局部测试推导为 Rust 主链已支持。

## 扩展指南

- 新增构建级计划字段时，先扩展 `PhysicalWindowPlan`，再同步所有构造点：`window_executor_test.rs::build_plan`、`pkg/executor/benchmark_test.rs` 中的各个计划字面量，以及任何新生产调用者。避免在执行器内部重复推导上游已经确定的信息。
- 新增第三种执行路径时，需要同时扩展 `WindowExecutor` 四个分派方法、`build` 的选择规则、生命周期/内存测试和有序路径约束。遗漏 `close` 或 `memory_bytes` 分派会造成资源或观测不一致。
- 新增帧类型时，不应继续依赖缓冲分支的 `Some(mut frame)` 兜底为 RANGE；应改为显式匹配，并同步流水线侧 `range_frame` 判定、对应处理器与独立测试。
- 修改 RANGE 装配时，必须保留起止两端 `update_compare_cols` 的错误传播，并同时检查升序、降序、无 `ORDER BY`、多排序键和列越界场景。相关独立测试宜放在同目录 `window_executor_test.rs` 或 `window_test.rs`，不要嵌入生产源文件。
- 修改内存初值或状态字段时，要与 `WindowMemoryTracker::open/close`、两种执行器的重置逻辑以及 `window_memory_test.rs` 同步；重点风险是重复计费、漏释放和输出缓冲移交后仍计费。
- 若要接入真实 Rust planner/session 主链，应新增明确的适配层，把物理计划和表达式描述转换成此处的简化计划，而不是让本文件悄悄承担 SQL 解析或 planner 类型转换；同时需要补充生产调用边和端到端 SQL 测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录文件列表确认 Rust/Go 实现及独立测试均已索引。
- RustCodeGraph `node --file pkg/executor/windows/builder.rs --offset 1 --limit 400`：读取目标文件完整 217 行，并报告 14 个符号以及 `pkg/executor/benchmark_test.rs`、`pkg/executor/windows/window_memory_test.rs` 两个直接文件使用者。
- RustCodeGraph `query build_ordered --kind function`：定位 `builder.rs::build_ordered` 与其 Rust 回归测试；针对精确符号的 callers/callees 查询未返回可靠边，因此调用点以索引的 `explore` 结果和仓库 `rg` 交叉核对，不臆造缺失图边。
- RustCodeGraph `explore "pkg/executor/windows FrameBound update_compare_cols WindowMemoryTracker GroupChecker PipelinedWindowExec WindowExec"`：核对 `build -> build_ordered`、内存测试调用、执行器生命周期、RANGE 比较列和处理器下游关系。
- RustCodeGraph 文件节点读取：`window.rs` 的 `FrameBound`、`GroupChecker`、三种处理器、`WindowExec`、内存跟踪；`pipelined_window.rs` 的执行器字段及 `open/close` 生命周期。
- 直接读取的非图/对照证据：`pkg/executor/windows/Cargo.toml`、`lib.rs`、`builder.go`、`window_executor_test.go`；Rust 独立测试 `window_executor_test.rs`、`window_memory_test.rs`，以及 `rg` 得到的 `pkg/executor/benchmark_test.rs` 构建调用点。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰好包含上述 11 个固定二级标题；内容人工复核重点是：不把测试接线写成生产主链接线，不把 Go 中尚未迁入本构建器的 planner/session/表达式职责写成 Rust 已支持。
