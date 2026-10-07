# `pkg/executor/join/index_lookup_merge_join.rs`

## 文件定位

本文件属于 `astersql-executor-join` crate；`pkg/executor/join/Cargo.toml` 以 `lib.rs` 为库入口，`lib.rs` 通过公开模块 `index_lookup_merge_join` 暴露这里的 API。当前可执行 Rust 代码从源文件第 565 行附近开始，提供一个基于内存 `Row` 的同步 Index LookUp Merge Join：外表分批生成 lookup key，调用 `IndexJoinExecutorBuilder` 拉取内表行，再按连接键执行归并连接。

文件前半段（约第 16—564 行）是被块注释包住的 Go 对照设计草图，其中出现通道、worker、context、内存 tracker 和 chunk 池等结构；这些内容不参与 Rust 编译，不能当作当前已实现能力。当前真实调用证据主要来自同 crate 单元测试 `pkg/executor/join/index_lookup_merge_join_test.rs`、独立测试 crate `pkg/executor/join/test/indexjoin/index_lookup_merge_join_test.rs` 和 `pkg/executor/benchmark_test.rs`。虽然 `pkg/executor/builder.rs::buildIndexLookUpMergeJoin` 会选择 `ExecutorKind::IndexLookupMergeJoin`，真正的构造由 `ExecutorBuilderDependencies::build_index_join_executor` 间接完成，未发现它直接构造本文件的 `IndexLookUpMergeJoin`。

## 核心职责

1. `OuterMergeWorker` 保存外表行和读取游标；必要时先按 join key 排序，并以指数增长的批大小产生 `LookUpMergeJoinTask`。批次边界会向后扩展到相同 key 组结束，避免同 key 外表行被拆到两个任务。
2. `InnerMergeWorker` 从每批外表行提取非 NULL lookup key，调用 `IndexJoinExecutorBuilder::build` 获取候选内表行，对内表按 key 排序，然后用双指针识别两侧同 key 区间。
3. `Joiner::try_to_match_inners` 负责实际连接条件与结果拼装；没有成功匹配时，`Joiner::on_miss_match` 按 join 类型产生或抑制 miss 结果。
4. `IndexLookUpMergeJoin` 管理 `open`/`next`/`close` 生命周期、一次性完整执行和分页读取输出。

当前实现不是 Go 版的并发流式执行器：它会在第一次有效 `next` 中通过 `execute` 消费所有外表任务，把全部结果累积到 `output`，之后才按 `required_rows` 返回切片。

## 主要符号

- `LookUpMergeJoinTask`：单批工作对象。`outer_rows`、`lookup_contents`、`inner_rows` 分别保存外表批、索引查找内容和内表候选；`output` 保存本批结果；`outer_match` 当前由 `OuterMergeWorker::build_task` 全部初始化为 `true`，但生效代码没有读取它；`done` 由 `handle_task` 成功后置为 `true`。
- `IndexMergeJoinResult`：`next` 的返回包装，包含 `rows` 和 `error`。当前正常路径只构造 `error: None`；真实错误由 `Result<_, String>` 的 `Err` 返回。
- `OuterMergeWorker::{new,new_with_outer_sort,build_task,reset}`：校验批大小、可选排序、按完整 key 组切批、指数扩批，以及重开时复位游标和初始批大小。`OuterMergeWorker` 类型公开，但字段与 `reset` 私有。
- `InnerMergeWorker::{construct_lookup_keys,handle_task,do_merge_join}`：构造 lookup 内容、拉取并排序内表、执行同 key 分组归并。类型公开但字段私有，通常由 `IndexLookUpMergeJoin::execute` 临时创建。
- `IndexLookUpMergeJoin::{new,new_with_outer_sort,open,next,close}`：对外构造和生命周期 API；`execute` 是私有的完整求值入口。
- `compare_keys`：公开的键比较函数。它先检查两侧 key 列数量，再分别调用 `extract_key`，最后交给 `index_lookup_join::compare_row` 做字典序比较。

本文件没有模块级常量、trait、条件编译项或可执行的异步 worker 定义。

## 执行流程

1. `new` 默认把 `need_outer_sort` 设为 `true`；`new_with_outer_sort` 先检查外、内 key 数量一致，再创建 `OuterMergeWorker`。后者拒绝零 `batch_size` 或零 `max_batch_size`，并在需要时按外表 key 排序。
2. `open` 清空历史输出、将结果游标归零、调用 `OuterMergeWorker::reset`，再设置 `opened = true`。直接调用 `next` 也会隐式 `open`。
3. 首次 `next` 发现 `output` 为空且 `cursor == 0` 时调用 `execute`。`execute` 循环调用 `build_task`，直到外表耗尽。
4. `build_task` 以当前 `batch_size` 取行；若批尾 key 与下一行相同，继续扩展批尾，从而保持同 key 组完整。任务生成后批大小翻倍，但不超过 `max_batch_size`。
5. `InnerMergeWorker::handle_task` 调用 `construct_lookup_keys`。每个外表行由 `extract_key` 提取 key；含 `Value::Null` 的 key 被跳过，其余内容按相邻相等 key 去重。由于默认路径已排序，这等价于对相同 key 去重；若调用方选择 `need_outer_sort = false`，必须保证外表已经按 key 有序，否则非相邻重复 key 不会被去重，归并顺序也不成立。
6. builder 根据 lookup 内容返回内表候选；`handle_task` 对候选按内表 key 排序，再进入 `do_merge_join`。
7. `do_merge_join` 使用 outer/inner 两个游标。外 key 较小时对该外行执行 miss；内 key 较小时推进 inner；相等时分别扩展两侧完整同 key 区间，并让 `Joiner` 对每个外行匹配整个内区间。未匹配的外行由 `on_miss_match` 处理。
8. 每个任务的 `output` 追加到执行器总 `output`。`next(required_rows)` 从 `cursor` 起最多复制指定行数；结果耗尽或 `required_rows == 0` 时返回空结果。`close` 清空输出并复位 worker，使同一实例可重新执行。

## 数据与状态

`OuterMergeWorker` 的持久状态是 `rows`、`key_columns`、`cursor`、当前/初始/最大批大小。每次 `build_task` 都复制该批外表行；同 key 组可能使实际批大小超过 `max_batch_size`，这是保持归并正确性的有意取舍。指数扩批仅影响下一批的目标大小。

`IndexLookUpMergeJoin` 拥有 boxed builder、两侧 key 列、`Joiner`、完整输出缓冲、输出游标和 `opened` 标志。`execute` 将各任务结果移动到总输出中，因此峰值内存至少包含输入外表、当前内表候选和全部未释放输出；没有 Go 版的 chunk 回收或 memory tracker。

排序和分组的不变量是：外表必须按 `outer_key_columns` 有序，内表必须按 `inner_key_columns` 有序，且两组列数量相同。`new_with_outer_sort(..., false)` 把第一个不变量交给调用方；内表则总在 `handle_task` 中排序。键值类型、NULL 顺序和字典序最终由 `row_table_builder::Value`、`extract_key` 与 `compare_row` 决定。

## 依赖与调用关系

上游关系：

- `pkg/executor/join/lib.rs` 公开导出模块。
- `pkg/executor/join/index_lookup_merge_join_test.rs` 直接调用 `IndexLookUpMergeJoin::new`，验证 close 后 reopen。
- `pkg/executor/join/test/indexjoin/index_lookup_merge_join_test.rs` 调用 `new`，覆盖小批量重复执行和多列 key 顺序。
- `pkg/executor/benchmark_test.rs::{run_index_merge_join_case,run_index_join_lane}` 调用 `new_with_outer_sort`，并由 `drain_index_merge_join` 执行 `open`/循环 `next`/`close`。
- RustCodeGraph 将本文件标为被 `pkg/executor/benchmark_test.rs` 与 `pkg/executor/join/index_lookup_merge_join_test.rs` 使用；跨 crate 测试由源码导入核对补充。

下游关系：

- `crate::index_lookup_join::{IndexJoinExecutorBuilder,IndexJoinLookupContent,extract_key,compare_row}` 定义索引拉取抽象、lookup 内容和 key 操作。
- `crate::joiner::{Joiner,NaajType,Row}` 提供连接类型语义、条件匹配、miss 处理和结果行表示；当前归并路径向 `try_to_match_inners` 传入 `NaajType::Unknown`。
- `crate::row_table_builder::Value` 用于识别 NULL lookup key。

`pkg/executor/join/Cargo.toml` 声明 crate 名为 `astersql-executor-join`，库入口是 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/executor/join"` 记录 Go 来源。当前生效代码只使用 crate 内模块；Cargo 中大量 executor/expression/chunk 等依赖位于 `cfg(windows)` 目标依赖，主要对应同目录其他移植代码及注释草图所描述的完整执行器边界。

## 错误处理与边界

- 构造期：两侧 key 数量不一致返回 `"index merge join key count mismatch"`；批大小或上限为零返回 `"merge lookup batch size must be positive"`。
- 比较期：`compare_keys` 的列数不一致返回 `"merge key count mismatch"`；key 列越界等问题由 `extract_key` 以 `String` 错误传播。
- lookup/连接期：builder、`construct_lookup_keys`、`do_merge_join` 和 `Joiner` 错误沿 `Result<_, String>` 逐层传播，失败任务不会设置 `done = true`，也不会把部分任务输出追加到执行器总输出。
- NULL：任一 lookup key 分量为 `Value::Null` 时不访问内表；该外行仍留在 `outer_rows`，最后是否产生结果由归并 miss 和 `Joiner` 的 join 类型决定。
- 空输入或结果耗尽：`next` 返回默认空结果，不用 `IndexMergeJoinResult.error` 表示 EOF。
- 排序比较闭包对 `compare_keys` 错误使用 `unwrap_or(Ordering::Equal)`。因此异常 key 在排序阶段会被当成相等，错误可能直到后续归并比较才暴露；扩展 key 表示或排序逻辑时应避免依赖这种降级行为。
- 当前实现没有检查 `batch_size <= max_batch_size`；若初始值更大，首批目标仍可能超过上限，生成首个任务后才通过 `.min(max_batch_size)` 收敛。

## 并发与资源生命周期

当前生效 Rust 路径没有线程、异步任务、锁、原子变量、通道或取消上下文。`OuterMergeWorker` 和临时 `InnerMergeWorker` 都在调用 `next` 的线程中顺序执行；`IndexJoinExecutorBuilder::build` 也是同步调用。资源以普通所有权管理：执行器拥有外表、builder、joiner 和输出，任务拥有各自的行向量，局部 worker 只借用执行器字段。

生命周期为 `new -> open（可由 next 隐式触发）-> next* -> close`。`close` 清空输出、归零游标、恢复 outer worker 初始批大小并标记未打开；单元测试 `index_lookup_merge_join_can_reopen_after_close_like_go` 证明同一实例能再次 `open` 并得到相同结果。若调用者在结果尚未取完时 `close`，剩余缓冲结果会被丢弃。

Go 版 `IndexLookUpMergeJoin` 则启动一个 outer worker 和多个 inner worker，使用 task/result channel、context cancellation、WaitGroup、chunk 资源池、memory tracker 和 runtime stats；这些结构只存在于本文件注释草图及 Go 源码中，尚不能用来描述当前 Rust 的并发保证。

## 与 Go 版本的对应关系

Rust 的 `IndexLookUpMergeJoin`、`OuterMergeWorker`、`InnerMergeWorker`、`LookUpMergeJoinTask` 和 `IndexMergeJoinResult` 分别对应 `pkg/executor/join/index_lookup_merge_join.go` 的同类概念。两版共享的核心意图是：外表按 key 有序、由外表 key 驱动索引 lookup、内表有序后按相同 key 组归并、通过 joiner 处理匹配和 miss；Rust 独立测试也对应 Go 的 `TestIssue18068` 与 `TestIssue54064`。

但当前语义覆盖并不等价：

- Go `Open/startWorkers/Next/Close` 是并发、流式、chunk 化且可取消的；Rust 首次 `next` 同步计算并缓存全部结果。
- Go 根据 session 配置控制并发和批大小，outer join 还使用请求的 required rows；Rust 构造时直接接收批大小，`required_rows` 只控制已缓存结果的分页。
- Go 处理 outer filter、类型转换、collation、desc 排序、order-by key 映射、last-column compare helper、index range、内存跟踪、runtime stats、panic/failpoint 和 chunk 回收；当前生效 Rust 未实现这些能力。
- Go 构造 lookup key 时会把溢出、截断或转换后不相等视为无需 lookup；Rust 的 `Value` key 没有对应类型转换步骤，只跳过 NULL。
- Go 的 `TestIssue54064` 当前断言优化器不应选择 `IndexMergeJoin`（`NotRegexp`），同时核对 SQL 结果；Rust 测试只直接实例化本文件执行器，验证多列 key 映射所得行，不能证明 planner 选择行为。

因此本文件应视为可直接测试和 benchmark 的 Rust 核心模型，而不是 Go 生产执行器全部能力的完整替代。

## 扩展指南

- 增加外表过滤、collation、降序或额外比较列时，应优先扩展明确的上下文结构和 `compare_keys`/排序入口，确保 lookup 去重顺序与 merge 顺序使用同一比较规则；同步增加独立测试文件中的 NULL、多列、重复 key、升降序和不排序前置条件用例。
- 接入真实 executor 主链时，需要在 `pkg/executor/builder.rs` 的依赖实现中明确构造本类型或新的生产适配器，并补齐 chunk/流式返回、取消、内存统计和 runtime stats。不要仅依赖当前 `ExecutorKind::IndexLookupMergeJoin` 枚举分支宣称已接线。
- 若引入并发 worker，应把 Go 版 channel 关闭顺序、取消传播、结果资源回收和 reopen 行为作为兼容约束，并在独立 `*_test.rs` 中测试错误、取消与重复执行；测试逻辑不要放回生产 `.rs` 文件。
- 若修改 `OuterMergeWorker::build_task`，必须保持“同 key 组不跨批”的不变量，否则同一 inner key 的推进状态可能导致漏匹配。还应覆盖超大重复 key 组可能突破批上限的内存风险。
- 若修改 `InnerMergeWorker::do_merge_join` 或 `Joiner` 调用，需覆盖 inner、outer/semi/anti 及 NULL-aware 行为；当前只显式传递 `NaajType::Unknown`，不能据此假设所有 join 类型已与 Go 对齐。
- 若要让 `IndexMergeJoinResult.error` 承载错误，应统一 `next` 的 `Result` 与字段语义，避免双重错误通道；否则可考虑移除未使用字段，但这属于行为/API 变更，不是本文档任务范围。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query IndexLookUpMergeJoin` 定位 Rust 结构于本文件及 Go 对照结构，`query handle_task` 定位 `InnerMergeWorker::handle_task`，其图中 callees 为 `construct_lookup_keys`、`compare_keys`、`do_merge_join`；文件节点报告直接使用者为 `pkg/executor/benchmark_test.rs` 和 `pkg/executor/join/index_lookup_merge_join_test.rs`。
- 生产源码：`pkg/executor/join/index_lookup_merge_join.rs` 的生效符号区（`LookUpMergeJoinTask` 至 `compare_keys`）；`pkg/executor/join/index_lookup_join.rs` 的 builder/key 抽象；`pkg/executor/join/joiner.rs` 的匹配与 miss 语义；`pkg/executor/builder.rs::{buildIndexLookUpMergeJoin,build_index_join_kind}` 的计划构建入口。
- crate/模块：`pkg/executor/join/Cargo.toml` 与 `pkg/executor/join/lib.rs`。
- Rust 测试：`pkg/executor/join/index_lookup_merge_join_test.rs`；`pkg/executor/join/test/indexjoin/index_lookup_merge_join_test.rs`；性能/路径使用证据 `pkg/executor/benchmark_test.rs`。
- Go 对照：`pkg/executor/join/index_lookup_merge_join.go`；`pkg/executor/join/test/indexjoin/index_lookup_merge_join_test.go::{TestIssue18068,TestIssue54064}`。
- 按任务约束未运行 Cargo；本文档交付仅执行章节结构、文件存在性和链接路径检查。
