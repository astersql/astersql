# `pkg/executor/join/merge_join.rs`

## 文件定位

本文件属于 `astersql-executor-join` crate。该 crate 由 `pkg/executor/join/Cargo.toml` 定义，入口 `pkg/executor/join/lib.rs` 以 `pub mod merge_join` 公开本模块，并在 `#[cfg(test)]` 下把独立单元测试 `merge_join_test.rs` 接入 crate。

当前有效 Rust 代码提供两个内存行集上的排序归并连接执行器：串行的 `MergeJoinExec`，以及先分 lane、再并行运行多个 `MergeJoinExec` 的 `ShuffleMergeJoinExec`。输入、输出行都是 `joiner::Row = Vec<row_table_builder::Value>`，结果如何拼接则委托给 `joiner::Joiner`。

它目前不是 Go `exec.Executor` 的逐接口替代品：RustCodeGraph 和精确引用检索只发现 `pkg/executor/benchmark_test.rs`、`pkg/executor/join/merge_join_test.rs` 与 `pkg/executor/join/test/mergejoin/merge_join_test.rs` 直接构造这些类型。`pkg/executor/builder.rs::buildMergeJoin` 会调用依赖注入接口 `build_merge_join_executor`，但没有直接引用本文件的具体类型。因此，本文件是可直接调用并由测试覆盖的 crate 级实现；它是否进入完整 SQL 执行主链，现有直接证据不能确认。

文件第 19—387 行保留了一段注释化的 Go 结构/流程草图。它是移植对照资料，不是会被编译的 Rust 实现；判断现状应以第 388 行之后的类型和方法为准。

## 核心职责

- `MergeJoinTable` 保存单侧已排序行、join key 列下标和分组游标，把连续且 key 相等的行暴露为一个 group。
- `MergeJoinExec` 同步推进 outer/inner group：较小的一侧推进；key 相等时让每条 outer 行与完整 inner group 匹配；outer 无匹配时由 `Joiner` 按连接类型决定是否补行。
- `ShuffleMergeJoinExec` 用相同 key 的哈希值把两侧行送入相同 lane，在作用域线程中并行完成每个 lane 的 merge join，再拼接各 lane 输出。
- 通过 `Joiner::try_to_match_inners` 和 `Joiner::on_miss_match` 复用 Inner、Outer、Semi、Anti 等连接类型及 other-condition 的结果语义，而不在本文件重复实现行投影和三值逻辑。

关键前置条件是两侧输入必须按 join key、按 `compare_row` 所定义的升序排列。`MergeJoinTable::new` 只验证每行能否抽取 key，不检查排序；传入乱序数据可能产生错误结果而不会报错。`ShuffleMergeJoinExec` 也不在 lane 内排序，只保留输入在各 lane 中的相对次序，所以调用方仍需先保证全局输入有序。

## 主要符号

- `pub struct MergeJoinTable`：一侧输入的所有权容器。公开字段 `rows`、`join_keys`、`is_inner` 描述数据和角色；私有字段 `cursor`、`group_start`、`group_end`、`finished` 记录分组状态。
- `MergeJoinTable::new(rows, join_keys, is_inner) -> Result<Self, String>`：逐行调用 `extract_key`，拒绝越界等无法抽 key 的输入；不会校验 key 数量非零或行序。
- `MergeJoinTable::init` / `finish`：分别重置游标和把表标为耗尽。`finish` 不释放 `rows`。
- `MergeJoinTable::select_next_group() -> Result<Option<&[Row]>, String>`：从 `cursor` 开始用 `compare_rows` 扩展连续等 key 区间；inner 侧遇到任一 key 为 `Value::Null` 的 group 时整体跳过。
- `MergeJoinTable::current_group` / `current_key`：读取最近选择的 group 或代表 key。它们不是主执行循环的必要入口，主要暴露观察能力。
- `MergeJoinTable::has_null_in_join_key`：把列下标不存在也视作 NULL；正常构造路径已由 `new` 提前拒绝越界，因此这一分支主要是防御性判断。
- `pub struct MergeJoinExec`：持有 outer/inner 表、`Joiner`、完整输出缓冲、分页游标和 `opened`/`closed` 生命周期标志。
- `MergeJoinExec::new`：只额外校验两侧 join key 列数相同。
- `MergeJoinExec::open` / `next` / `close`：实现本文件自己的轻量生命周期；`next` 首次取数时通过私有 `execute` 一次性算完全部结果，再按 `required_rows` 分页复制返回。
- `MergeJoinExec::compare`：抽取两行的 key 后交给 `index_lookup_join::compare_row` 比较。
- `pub struct ShuffleMergeJoinExec`：持有多个串行 lane、合并结果缓冲、分页游标和生命周期标志。
- `ShuffleMergeJoinExec::new`：校验 `concurrency > 0` 和两侧 key 数量一致，随后分区并为每个 lane 构造一对 `MergeJoinTable`。
- `ShuffleMergeJoinExec::partition`：对 key 的 `Debug` 字符串使用 `DefaultHasher`，再对并发数取模。
- `ShuffleMergeJoinExec::execute`：把 `lanes` 移出自身，在 `std::thread::scope` 中每 lane 启动一个线程，循环 `next(1024)` 至耗尽并关闭 lane，最后按 lane 顺序展平输出。

本文件没有 trait、模块级常量或条件编译项；公开 API 均为上述类型和方法，`execute`、`partition`、`compare_rows` 以及状态字段是内部实现。

## 执行流程

普通 Merge Join 的完整流程如下。

1. 调用方先构造 outer/inner `MergeJoinTable` 和配置好的 `Joiner`，再用 `MergeJoinExec::new` 组合。构造阶段验证 key 可抽取、两侧 key 数相同，但不验证排序。
2. `open` 调用两侧 `init`，清空旧输出并把输出游标归零。若已 `close`，返回 `cannot reopen closed merge join`。
3. `next(required_rows)` 可隐式调用 `open`。当输出尚未生成时，它调用 `execute`；`required_rows == 0` 虽不返回行，但当前普通执行器会先完成一次 `execute`。
4. `execute` 分别取得第一组 outer 和 inner。`select_next_group` 以连续相等 key 划组；inner 的 NULL-key group 被跳过，outer 的 NULL-key group则逐行走 `on_miss_match(false, ...)`。
5. 若 inner 已耗尽，所有剩余 outer group 都按未匹配处理。否则比较两组代表行：outer key 较小时输出/忽略未匹配 outer 并推进 outer；inner key 较小时跳过该 inner group并推进 inner；相等时对每条 outer 调用 `try_to_match_inners` 扫描整个 inner group。
6. 相等 key 下，若 `Joiner` 报告没有匹配，则把其 `has_null` 传给 `on_miss_match`，保留 other-condition 的 SQL 三值语义。处理完相等组后两侧同时推进。
7. `next` 从 `output[cursor..end]` 克隆至返回值；到达末尾或请求零行时返回空向量。`close` 结束两侧游标、清空结果，并永久设置 `closed`。

Shuffle 路径先在 `new` 中按 key 分 lane，同 key 的两侧行因使用同一分区函数而进入同一 lane。首次非零 `next` 调用 `execute`，每个线程打开并耗尽自己的串行执行器；线程 panic 被转换为 `shuffle merge join worker panicked`。最终结果按 lane 建立顺序展平，不保证全局按 key 排序。

## 数据与状态

`MergeJoinTable` 拥有 `Vec<Row>`，group 是 `rows[group_start..group_end]` 的借用切片。`cursor` 总是指向下一未分组行；`group_start/group_end` 指向最近一次已选择组；`finished` 仅由 `init`/`finish` 更新，当前没有公开读取者，也不参与 `select_next_group` 的控制流。

`is_inner` 决定 NULL-key 组的处理：inner NULL key 永不参与等值匹配；outer NULL key仍需进入 miss 路径，让外连接、反连接和外半连接得到正确形状。普通比较使用完整复合 key；第一个差异由 `compare_row` 的值顺序决定。

`MergeJoinExec::output` 缓存完整物化结果，`cursor` 只负责分页。这意味着 `required_rows` 限制单次返回量而不限制实际计算量和峰值内存。若连接结果为空，`output.is_empty() && cursor == 0` 在后续 `next` 仍成立，会重复执行归并；结果正确，但会重复工作。

`ShuffleMergeJoinExec::lanes` 在执行时通过 `std::mem::take` 变为空；执行完成后实际 lane 对象在线程返回前已关闭并被丢弃。`output` 同样完整物化所有 lane 结果。分区依赖 `format!("{key:?}")`，因此分区稳定性绑定到 `Value` 的 `Debug` 表示与标准库 hasher；这里只要求同一次执行中两侧等 key 同 lane，不应把 lane 编号当作持久协议。

## 依赖与调用关系

直接下游依赖只有同 crate 的三个模块：

- `index_lookup_join::{extract_key, compare_row}`：抽取指定列组成的 key，并提供 key/行值的排序比较。
- `joiner::{Row, Joiner, NaajType}`：定义行、连接类型行为、other-condition 求值、匹配与 miss 输出。本文件调用 `try_to_match_inners(..., NaajType::Unknown)`，没有细分 NAAJ key 状态。
- `row_table_builder::Value`：用于识别 `Value::Null`。

并发只使用标准库 `std::thread::scope`，哈希分区使用 `std::hash` 和 `DefaultHasher`。`Cargo.toml` 没有为本文件声明专属第三方依赖；crate 的两个无条件 workspace 依赖也未被本文件直接引用，大量其他依赖位于 `cfg(windows)` 区段。

直接上游证据如下：`lib.rs` 对外公开模块；`merge_join_test.rs` 直接测试 inner NULL 组跳过；独立 crate 测试 `test/mergejoin/merge_join_test.rs` 构造串行和 shuffle 执行器；`benchmark_test.rs` 的 `run_merge_join_lane` 与 `run_merge_join_sort_shuffle_case` 用于基准矩阵。仓库中的 `builder.rs::buildMergeJoin` 只依赖抽象的 `ExecutorBuilderDependencies::build_merge_join_executor`，精确搜索没有发现该接口直接落到本文件类型，故不能宣称这里已连接到 SQL planner/executor 主链。

## 错误处理与边界

所有可预期错误都以 `Result<_, String>` 传播：key 列抽取错误来自 `extract_key`；key 数不一致、零并发和关闭后重开使用本文件固定字符串；Joiner 的 other-condition 错误由 `try_to_match_inners` 原样上抛；worker panic 被归一为固定错误。

重要边界包括：空 outer 直接产生空结果；空 inner 使 outer 全部走 miss；复合 key 任一 inner 列为 NULL 会跳过整组；`required_rows == 0` 返回空；构造时已关闭标志为 false，但 `close` 后不可再次打开。`Joiner` 可以表达多种 join type，不过本执行策略只追踪 outer 行的未匹配状态，没有为 full outer/right outer 额外输出“未被任何 outer 匹配的 inner 行”；因此不得仅凭 `JoinType` 枚举就宣称本文件完整支持这些连接语义，扩展或启用前需补专门验证。

本实现不会检测未排序输入，也不校验 shuffle lane 的排序。在错误输入下，行为可能是静默漏配而不是显式失败。空结果的重复执行、全量物化及返回时克隆也属于调用方需要了解的性能边界。

## 并发与资源生命周期

普通 `MergeJoinExec` 是同步、单线程执行器，没有锁、通道、异步任务或共享可变状态。它取得两侧行和 `Joiner` 的所有权，结果保存在内存中；`close` 清空输出但保留两侧原始 `rows`，且关闭不可逆。

Shuffle 执行器以 lane 为所有权隔离单元。`std::thread::scope` 保证所有 worker 在线程作用域结束前 join；每个 worker 独占一个 `MergeJoinExec`，所以无需锁。主线程按句柄顺序收集结果；任何 worker 返回错误会终止收集并向上传播，任何 panic 会转换为字符串错误。当前代码没有取消其余 worker、内存预算或背压机制。

与 Go 版不同，有效 Rust 实现没有 `memory::Tracker`、`disk::Tracker`、`RowContainer`、临时文件或 OOM spill action。顶部注释块虽记录了这些 Go 机制，但不会编译执行。独立 Rust 测试文件也明确说明 spill/tracker 暂未移植。

## 与 Go 版本的对应关系

Go 对照实现是 `pkg/executor/join/merge_join.go`。两版共同保留的核心语义是：输入按 key 有序、连续等 key 行组成 group、inner NULL-key group 跳过、比较后推进落后一侧、等 key 时 outer 对 inner group 做交叉匹配，以及无匹配 outer 委托 Joiner 产出。

Rust 当前实现是内存化简版，而非结构等价移植：

- Go `MergeJoinTable` 从 child executor 逐 chunk 拉取，借助 `VecGroupChecker` 处理分组，并用 `RowContainer` 合并跨 chunk 的 inner group；Rust 在构造前已拥有全部 `Vec<Row>`，直接用切片划组。
- Go `Next` 按目标 chunk 容量流式推进，并能在输出 chunk 满时保存 inner iterator 位置；Rust `execute` 一次性物化全部结果，`next` 只分页复制。
- Go 支持 `Desc`、outer 侧 vectorized filters、`requiredRows` 下推、statement memory/disk tracker、OOM spill failpoint 与 child executor 生命周期；有效 Rust 代码均未实现这些机制。
- Go key 比较使用每列的 `expression.CompareFunc` 和求值上下文；Rust 使用 `Value`/`compare_row`，没有 collation、类型求值上下文或每列比较函数入口。
- Rust 额外定义 `ShuffleMergeJoinExec` 并直接管理线程/lane；Go 的 shuffle 在更高层执行计划中表现为 `Shuffle` + lane 内 `MergeJoin`，`merge_join.go` 本身只定义串行 executor。

Go 测试 `pkg/executor/join/test/mergejoin/merge_join_test.go` 覆盖：spill 后 tracker 归零、left outer 结果、chunk-size 边界矩阵、SMJ 与 Hash Join 结果对照，以及 `tidb_merge_join_concurrency=4` 的 shuffle 计划。Rust 对照测试复用了相同数据矩阵并直接调用本文件类型，但明确没有伪造当前会话层不支持的 JOIN 计划和 spill/tracker 验证。

## 扩展指南

- 接入完整 SQL 执行主链时，首先确认 `ExecutorBuilderDependencies::build_merge_join_executor` 的实现应否构造本文件类型，并为 child executor、schema/type/collation、排序方向和执行生命周期设计适配层；不要仅在测试中直接构造后就视为已接线。
- 若增加降序或 NULL-safe equality，修改点集中在 `MergeJoinTable::select_next_group`、`compare_rows`、`MergeJoinExec::compare/execute`；必须分别测试复合 key、两侧 NULL、升降序与 outer/anti/semi 语义。
- 若要真正流式化，应拆分 `execute` 的状态机，保存当前 outer/inner group和组内位置，避免 `output` 全量物化；同时测试不同 `required_rows` 序列、空结果重复 `next` 和超大重复 key group。
- 若扩展 shuffle，必须保持“两侧等 key 同 lane”和“每 lane 输入仍按比较器有序”两个不变量。改变哈希编码时要避免依赖 `Debug` 作为稳定协议，并测试 worker error/panic、零并发、结果顺序和大 lane 偏斜。
- 若对齐 Go 的资源管理，需引入 tracker、跨 chunk 容器、spill 和可恢复的 close/error 路径；这不是在顶部注释块取消注释即可完成的工作，应同步独立测试中的 spill/failpoint 与消费量断言。
- 生产测试逻辑应继续放在独立文件：小范围分组回归放入 `pkg/executor/join/merge_join_test.rs`；端到端数据矩阵放入 `pkg/executor/join/test/mergejoin/merge_join_test.rs`；性能变化更新 `pkg/executor/benchmark_test.rs`。不要把 `#[cfg(test)]` 测试内嵌进本源文件。

兼容风险主要是排序/NULL/连接类型语义；性能风险主要是全量克隆、全量物化和 lane 偏斜；资源风险主要是当前缺少 Go 版 tracker 与 spill。任何扩展都应同时检查相应 Go 路径，不能以简化实现替代 Go 已有行为。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标 `pkg/executor/join/merge_join.rs` 已索引并识别 29 个符号。
- RustCodeGraph `node --file pkg/executor/join/merge_join.rs`：核对了全部 769 行有效实现和顶部注释化 Go 草图，主要定义位于 `MergeJoinTable`（394）、`MergeJoinExec`（491）、`ShuffleMergeJoinExec`（629）。
- RustCodeGraph 精确 `query/node/callers/callees`：消歧了 Go/Rust 同名 `MergeJoinExec`；类型级 callers/callees 未给出可用直接边，因此用精确引用检索补证，直接构造者仅见测试与基准路径。
- crate/模块证据：`pkg/executor/join/Cargo.toml`、`pkg/executor/join/lib.rs`。
- 下游语义证据：`pkg/executor/join/joiner.rs` 的 `Row`、`JoinType`、`Joiner::try_to_match_inners`、`Joiner::on_miss_match`；`pkg/executor/join/index_lookup_join.rs` 的 `extract_key`/`compare_row` 引用关系。
- 主链边界证据：`pkg/executor/builder.rs::buildMergeJoin` 与 `ExecutorBuilderDependencies::build_merge_join_executor`；未发现它们直接引用本文件的具体类型。
- Go 对照：`pkg/executor/join/merge_join.go`；Go 测试：`pkg/executor/join/test/mergejoin/merge_join_test.go`。
- Rust 测试：`pkg/executor/join/merge_join_test.rs` 验证 inner NULL group 跳过；`pkg/executor/join/test/mergejoin/merge_join_test.rs` 验证 outer/shuffle 结果和 Go chunk-size 矩阵；`pkg/executor/benchmark_test.rs` 验证串行、shuffle 与投影组合的直接调用形状。
- 本任务是纯文档分析，按任务约束未运行 Cargo。结构验证应确认本文存在且恰含上述 11 个固定二级标题。
