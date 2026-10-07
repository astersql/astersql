# `pkg/executor/windows/window.rs`

## 文件定位

本文件对应源码 [`window.rs`](./window.rs)，属于 `astersql-executor-windows` crate，是窗口执行模块的“缓冲式执行路径 + 共享窗口原语”实现。crate 入口 [`lib.rs`](./lib.rs) 将本模块整体公开再导出；[`builder.rs::build`](./builder.rs) 根据 `PhysicalWindowPlan` 选择缓冲式 `WindowExec` 或 `pipelined_window.rs::PipelinedWindowExec`，两条路径共同使用这里的值、帧边界、分组器、窗口函数和内存跟踪类型。

`pkg/executor/windows/Cargo.toml` 把 Go 对照包标为 `pkg/executor/windows`。目前依赖声明位于 `cfg(any())` 下，不会启用；因此当前 Rust 文件使用的是本 crate 内的简化 `Value`、`Chunk`、`ChildExecutor` 和 `ExecContext`，尚不是 TiDB 完整 executor/chunk/sessionctx 类型的直接接线。

## 核心职责

1. 定义窗口执行所需的基础数据模型：`Decimal`、`Value`、`Row`、`Chunk`、`Error`/`Result`。
2. 用 `FrameBound`、`WindowFrame`、`OrderBy` 表达 ROWS/RANGE 帧，并计算 RANGE 候选行相对边界的位置。
3. 用 `WindowFunction` 统一窗口函数的 `reset`、`update`、`result` 和可选 `slide` 协议；本文件实现 `RowNumber`、`Lag`、`BitXor`、`Average`、`VarSamp`、`DecimalAverage`、`DecimalSum`、`MaxValue`、`MinValue`、`CountRows`、`Sum`。
4. 用 `GroupChecker` 在已经按 PARTITION BY 排序的输入中识别连续分区，并处理分区跨 Chunk 的情况。
5. 用 `AggWindowProcessor`、`RowFrameWindowProcessor`、`RangeFrameWindowProcessor` 将整分区、ROWS 帧、RANGE 帧分别映射为每行结果。
6. 用 `WindowExec` 缓冲完整分区、向原输入块追加窗口结果、按输入 Chunk 边界依次输出，并通过 `WindowMemoryTracker` 将持有内存计入语句级 tracker。

## 主要符号

- `Decimal { coefficient, scale }`：定点十进制。`coefficient_at_scale` 只允许扩大 scale，并对乘法和 `10^scale` 做溢出检查；相等比较先去尾随零。
- `Value`：支持 NULL、有/无符号整数、浮点、Decimal、文本和布尔。`numeric_cmp` 对整数及可对齐的 Decimal 保持精确比较，其他数值组合才回退到 `f64`；`sql_cmp` 规定 NULL 最小，并为同类文本/布尔提供顺序。
- `Chunk`：以 `Vec<Row>` 简化列式块接口；`projected` 保留输入列，`append_results` 追加窗口列，`swap_columns` 将已完成块零拷贝交给调用方，`memory_usage` 估算自身及嵌套缓冲容量。
- `MemoryTracker` / `WindowMemoryTracker`：前者是原子字节计数器；后者在 `Mutex<WindowMemoryState>` 中维护本地、父级和每个窗口函数的 partial-result 用量。
- `ChildExecutor`：`open/next/close` 生命周期边界；`VecChunkExecutor` 是独立测试使用的预置 Chunk 子执行器。
- `FrameBound`：`before_start` 与 `beyond_end` 依据升降序、PRECEDING/FOLLOWING、数值偏移和比较列判断 RANGE 边界；`update_compare_cols` 拒绝没有 ORDER BY 的 RANGE 帧。
- `WindowFunction`：默认 `slide` 返回 `false`，表示调用方需 reset 后重算；`ignores_frame` 供 `RowNumber` 等忽略帧的函数使用；两个 memory-usage 方法向 tracker 报告状态占用。
- `GroupChecker`：`split_into_groups` 生成当前 Chunk 内的半开区间，并返回首组是否延续上一 Chunk 的尾组；`next_group` 顺序消费这些区间。
- `calculate_frames`：ROWS/RANGE processor 的共同计算核心。它将起点大于终点的合法 SQL 帧规范为空半开区间，优先调用 `slide`，不支持滑动时重建 partial result，并逐行收集函数结果。
- `WindowExec`：缓冲式执行器状态机；关键入口为 `open`、`next`、`consume_one_group`、`consume_group_rows`、`fetch_child`、`close`。

## 执行流程

构建阶段由 `builder.rs::build` 完成：输出列数减窗口函数数得到 `input_columns`；无显式 frame 时选择 `AggWindowProcessor`，ROWS 选择 `RowFrameWindowProcessor`，RANGE 先把 ORDER BY 列写入两端 `FrameBound`，再选择 `RangeFrameWindowProcessor`；最后把 processor、`GroupChecker`、子执行器和共享 tracker 装入 `WindowExec`。

运行时流程如下：

1. `WindowExec::open` 把 tracker 挂到 `ExecContext::statement_memory_tracker`，清空队列、分组器和 EOF 状态，再打开 child。
2. `next` 先清空输出；只要尚未耗尽且队首结果块还未补齐窗口列，就调用 `consume_one_group`。
3. `fetch_child` 从 child 取下一 Chunk；非空块的原始版本保存到 `child_result`，前 `input_columns` 列的投影进入 `result_chunks`，对应行数进入 `remaining_rows_in_chunk`。
4. `consume_one_group` 用 `GroupChecker` 取得一个分区。若当前分区延续到 Chunk 尾部，则继续拉取后续 Chunk，直到分区键变化或 EOF，因而 processor 始终看到完整分区。
5. `consume_group_rows` 按结果块的剩余槽位分段填充。processor 先接收完整分区行，再由 `append_result` 产生当前段每行的窗口值，`Chunk::append_results` 把这些值附在输入列之后；分区完成后重置 partial result。
6. 队首块的 remaining 变为零后，`next` 将它弹出并通过 `swap_columns` 交给调用方。child 耗尽后，连续调用 `next` 最终返回空 Chunk。
7. `close` 清空所有缓冲和分组状态、解除并归零内存计费，再关闭 child。

三种 processor 的差异是：`AggWindowProcessor` 对整分区只更新一次状态并为每行读取结果；`RowFrameWindowProcessor` 用当前行和行偏移计算 `[start,end)`；`RangeFrameWindowProcessor` 依靠已排序 ORDER BY 值单调推进 `last_start_offset`/`last_end_offset`。后两者都委托 `calculate_frames` 使用滑动优化或完整重算。

## 数据与状态

`WindowExec` 同时持有原始 `child_result`、待返回的输入列投影 `result_chunks`、每块尚未追加结果的计数以及 processor 状态。这里的重要不变量是：两个 VecDeque 按下标一一对应；只有队首 remaining 为零时才能输出；传入 processor 的 `rows` 是一个完整且连续的 PARTITION BY 分区。

ROWS processor 的 `current_row` 每产出一行递增，起止位置均为分区内半开区间。RANGE processor 除 `current_row` 外还保存两个单调水位；此算法依赖上游已经按窗口 PARTITION BY/ORDER BY 要求排序。`RowNumber` 和 `Lag` 自行维护分区内当前位置；滑动聚合保存上一次 `[start,end)`，只增删差集，若边界倒退则重建。

内存状态分三类计费：函数固定 partial-result 大小在 `WindowMemoryTracker::open` 时计入；动态 partial-result 在 update/slide 后按差值更新；结果队列、临时完整分区和嵌套字符串容量在进入/离开所有权范围时增减。`Arc` 让本地 tracker 与 statement tracker 共享引用，`AtomicI64` 提供计数，`Mutex` 保护父级绑定和函数用量数组。

## 依赖与调用关系

直接上游是 `pkg/executor/windows/builder.rs::build`：它构造本文件的 `WindowExec`、processor、`GroupChecker` 和 `WindowMemoryTracker`；`WindowExecutor::{open,next,close}` 再把统一生命周期调用分派到缓冲或流水实现。`lib.rs` 将这些类型公开给 crate 用户。`pipelined_window.rs` 复用 `ChildExecutor`、`Chunk`、`FrameBound`、`GroupChecker`、`WindowFunction` 以及三个 partial-result 内存辅助函数，因此本文件也承担两条执行路径的共享语义层。

缓冲路径的内部调用链为 `WindowExec::next -> consume_one_group -> fetch_child / GroupChecker::{split_into_groups,next_group} -> consume_tracked_group_rows -> consume_group_rows -> WindowProcessor::{consume_group_rows,append_result}`；ROWS/RANGE 的 `append_result` 再进入 `calculate_frames -> WindowFunction::{slide,reset,update,result}`。

RustCodeGraph 索引显示目标文件由 `builder.rs`/`pipelined_window.rs` 的模块关系使用，并识别 `WindowExec` 的直接构建符号 `builder.rs::build`；对同名 `WindowExec`（Go/Rust）执行通用 callers/callees 查询时存在歧义，因此调用边以精确文件节点和构建源码复核为准。

## 错误处理与边界

所有可恢复错误统一为带消息的 `Error`。主要失败点包括：Decimal scale/系数和求和溢出；列下标、结果行下标越界；RANGE 缺少 ORDER BY；`GroupChecker` 接收空 Chunk 或被过度消费；BIT_XOR/Decimal 聚合遇到不支持的值类型；AVG 滑动计数下溢；MIN/MAX 滑动状态找不到应移除值；child 生命周期或窗口函数返回错误。

NULL 语义由各函数明确处理：数值聚合跳过非数值/NULL，空 SUM/AVG/MIN/MAX/VAR_SAMP 返回 NULL，`CountRows` 返回帧行数，`RowNumber` 忽略帧。`calculate_frames` 对 start > end 的帧强制使用空区间，避免非法切片且保留 SQL 空帧语义。`fetch_child` 当前把空 Chunk 当作 EOF，这是当前实现事实，扩展 child 协议时必须保持或有意识改变该约定。

`Value::sql_cmp` 是简化 SQL 比较，不包含完整 TiDB 类型、collation、时区及 Datum 比较规则；混合的非数值异类最终按 Debug 字符串排序。`Average`/`Sum` 使用 `f64`，而 Decimal 路径用 `i128`；Decimal 除不尽时追加 12 位 scale 并截断。这些都是与完整 Go 引擎接线时需要重点核对的兼容边界。

## 并发与资源生命周期

`ChildExecutor`、`WindowFunction` 和 `WindowProcessor` 都要求 `Send`，允许执行器所有权跨线程移动，但单个 `WindowExec::next` 仍是 `&mut self` 的串行状态机，没有在文件内创建线程或异步任务。内存计数用 relaxed 原子操作；复合状态通过 `Mutex` 序列化。锁只包围短小的记账操作，更新 parent counter 时仍持有 state 锁，但没有反向获取本 tracker 锁的路径。

生命周期要求是 `open -> 多次 next -> close`。`open` 会先调用 tracker 的 `close` 清理旧父级和旧计费，因此支持重新打开；`close` 释放队列和 partial-result 的全部计费。`next` 弹出结果块时将该块内存从窗口 tracker 转移出去，调用方随后拥有输出。`window_memory_test.rs` 同时验证缓冲式与流水式路径会把内存计入 statement tracker、输出后不遗留错误计费、close 后两级计数均归零。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/windows/window.go`。Rust `WindowExec::{open,next,close,consume_one_group,consume_group_rows,fetch_child}` 分别对应 Go `(*WindowExec).Open/Next/Close/consumeOneGroup/consumeGroupRows/fetchChild`；Rust `WindowProcessor` 及三种 processor 对应 Go `windowProcessor`、`aggWindowProcessor`、`rowFrameWindowProcessor`、`rangeFrameWindowProcessor`。两边都以连续分区为计算单位、支持跨 Chunk 拼接，并把结果追加回与输入块边界对应的输出块。

Rust 版本保留了 Go 的处理器分层、ROWS/RANGE 半开边界、滑动 partial result 和内存跟踪意图，但当前是独立简化模型：Go 使用真实 `exec.Executor`、`chunk.Chunk/Row`、`aggfuncs.WindowFunc`、session context 和表达式比较；Rust 使用本文件定义的容器和函数实现。Cargo 中真实子系统依赖被放在永假 `cfg(any())` 下也印证了尚未完成正式接线，不能把当前 crate 描述为完整替代 Go SQL 执行链。

`window_executor_test.rs` 对照 `window_executor_test.go` 的核心场景，验证缓冲/流水执行器的 row_number 与滑动 sum 一致、有序构建固定走流水路径，以及空帧时 SUM/COUNT/ROW_NUMBER 的返回约定。Go `window_sql_test.go` 覆盖更多函数、类型与 SQL 集成情形；其中未在 Rust 独立测试出现的能力不能仅凭 Go 测试宣称 Rust 已支持。

## 扩展指南

- 新增窗口函数：实现 `WindowFunction`，明确空帧/NULL/类型错误语义；若可增量维护则实现 `slide` 和 `set_window_start`，并准确报告 fixed/dynamic partial-result 内存。测试应放在同目录独立 `*_test.rs`，不要内嵌到 `window.rs`。
- 修改 ROWS/RANGE 规则：分别从 `RowFrameWindowProcessor::{start_offset,end_offset}` 或 `FrameBound::{before_start,beyond_end}`、`RangeFrameWindowProcessor` 接入；同时覆盖升降序、无界、CURRENT ROW、start > end、NULL、跨 Chunk 分区和大整数精确 peer group。
- 修改缓冲生命周期：保持 `result_chunks` 与 `remaining_rows_in_chunk` 同步、只输出已补齐块、错误后停止继续消费、close 归零父子 tracker。应同步 `window_executor_test.rs` 与 `window_memory_test.rs`。
- 扩充值类型或迁移到真实 TiDB 类型：优先替换 `Value::{numeric_cmp,sql_cmp}` 和各聚合的类型分派，并逐项对照 Go expression/types/collation 语义；性能上警惕完整分区复制、`Lag::update` 的行克隆、MIN/MAX 的线性删除与每行线性极值扫描。
- 任何改变都应同时核对 `builder.rs` 和 `pipelined_window.rs`，因为它们共享这里的 frame/function/tracker 协议；若行为只在一条路径更新，会破坏两执行器结果一致性。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/executor/windows` 定位 13 个 Go/Rust 文件；对 `window.rs` 分段执行 `node --file` 阅读 1–1884 行；查询 `WindowExec`、`GroupChecker` 并读取 `builder.rs`、`pipelined_window.rs` 精确文件节点，核对构建入口和共享调用关系。
- Rust 源与边界：`pkg/executor/windows/window.rs`、`builder.rs`、`pipelined_window.rs`、`lib.rs`、`Cargo.toml`。
- Rust 独立测试：`window_executor_test.rs`（缓冲/流水一致性、ordered pipeline、空帧），`window_test.rs`（超过 `f64` 精确整数范围的 RANGE peer 分离），`window_memory_test.rs`（statement/local 内存计费与 close 释放）。
- Go 对照：`pkg/executor/windows/window.go`、`window_executor_test.go`、`window_sql_test.go`；只将与当前 Rust 符号和测试能互相印证的行为写为已实现。
- 人工复核结论：文件存在是为了提供缓冲式窗口状态机及两路径共享原语；运行依赖有序输入、完整分区收集和 processor 逐行产值；安全扩展必须同步函数协议、帧边界、内存记账、builder/流水路径和独立测试。
