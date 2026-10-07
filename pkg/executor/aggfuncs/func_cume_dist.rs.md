# `pkg/executor/aggfuncs/func_cume_dist.rs`

源文件：[`func_cume_dist.rs`](./func_cume_dist.rs)

## 文件定位

该文件属于 `astersql-executor-aggfuncs` crate；crate 入口 [`lib.rs`](./lib.rs) 以 `pub mod func_cume_dist` 暴露模块，[`Cargo.toml`](./Cargo.toml) 则用 `[lib] path = "lib.rs"` 声明 crate 根。它保存 SQL 窗口函数 `CUME_DIST` 的 Rust 状态机：先收集一个已按窗口 `ORDER BY` 排序的分区，再逐行输出“不大于当前 peer group 的行数 / 分区总行数”。

这里需要区分算法实现与生产接线。Rust [`builder.rs`](./builder.rs) 已能把 `FunctionName::CumeDist` 选择为 `AggImplementation::CumeDist`，但仓库内 `.rs` 引用检索只发现 `CumeDist<T>` 本体由 [`func_cume_dist_test.rs`](./func_cume_dist_test.rs) 和 [`window_func_test.rs`](./window_func_test.rs) 实例化；未发现执行器依据该枚举变体创建本状态机的 Rust 生产调用。因此当前证据支持“算法状态机已实现并有单元测试”，不支持声称它已经接入完整 Rust SQL 执行主链。

## 核心职责

- `CumeDist<T>` 缓存当前窗口分区的全部行，并保存下一待输出行及当前 peer group 末端的位置。
- `update` 批量追加行，同时按 Go 版本约定返回估算内存增量，而不是容器真实 capacity 的字节变化。
- `next_by` 接受调用者提供的 peer 比较器，使 peer 判定能够对应多列 `ORDER BY`、排序方向、NULL 与排序规则等上层语义；本文件自身不解释 SQL 排序规则。
- `next` 为 `T: PartialEq` 提供简化入口，只根据值相等与否划分 peer，主要适合简单值和测试。
- `reset` 清除分区状态以复用已经分配的 `Vec` 容量。

## 主要符号

- `pub const DEF_PARTIAL_RESULT_CUME_DIST_SIZE: i64`：取 `size_of::<CumeDist<()>>()`，表示空状态对象的固定体积，对应 Go 的 `DefPartialResult4CumeDistSize`。泛型参数选 `()` 是为了只计结构中的固定字段和 `Vec` 句柄，不包含缓存行。
- `pub struct CumeDist<T>`：公开的泛型状态类型。其字段均为私有：`cur_idx` 是下一待输出行下标，`last_rank` 是当前 peer group 之后的第一个下标，`rows` 保存完整分区。
- `impl<T> Default`：建立 `cur_idx = 0`、`last_rank = 0`、空 `rows` 的初始状态。
- `reset(&mut self)`：把两个游标归零并执行 `rows.clear()`；容量可复用，但逻辑内容被清空。
- `update(&mut self, rows: impl IntoIterator<Item = T>) -> i64`：追加输入并返回新增元素数量乘 `func_rank::DEF_ROW_SIZE`。
- `next_by(&mut self, compare: impl Fn(&T, &T) -> Ordering) -> Option<f64>`：核心求值入口；按比较器扩展 peer group，推进一行并返回累计比例，耗尽时返回 `None`。
- `next(&mut self) -> Option<f64>`：在 `T: PartialEq` 时把相等映射为 `Ordering::Equal`，把不等统一映射为 `Ordering::Less` 后委托给 `next_by`。算法只关心“是否为 Equal”，不会使用 Less/Greater 的方向。

文件没有 trait 定义、宏、条件编译项或自定义错误类型。

## 执行流程

1. 调用者用 `CumeDist::default()` 创建空状态，或在前一分区结束后调用 `reset()`。
2. 调用者通过一次或多次 `update` 追加分区行。正确性前提是同行 peer 连续出现，即输入已经按窗口排序规则排列；本类型不会排序或校验顺序。
3. 每次需要一个输出时调用 `next_by`。它先用 `rows.get(cur_idx)?` 获取当前行；若已耗尽，立即返回 `None`，且不移动任何游标。
4. 从现有 `last_rank` 开始向右比较。只要候选行与当前行的比较结果为 `Ordering::Equal`，就递增 `last_rank`；循环结束后，`last_rank` 恰是当前 peer group 结束后的下标，也等于“小于等于当前组的行数”。
5. `cur_idx` 只增加一，所以一个调用只消费一个输出位置；同一 peer group 的后续行复用已经推进过的 `last_rank`，得到完全相同的比例。
6. 返回 `last_rank as f64 / rows.len() as f64`。例如 `[1, 1, 2, 3, 3]` 依次得到 `2/5、2/5、3/5、5/5、5/5`，这由 `cume_dist_advances_peer_groups_at_their_last_rank` 验证。

在满足“peer 连续”和“消费期间不再追加破坏顺序”的契约时，`last_rank >= cur_idx` 且两个游标都单调不减；整个分区的扫描总计为线性时间，每次输出的摊销复杂度为 `O(1)`。

## 数据与状态

`rows: Vec<T>` 拥有分区行，因此状态的主要资源成本随分区大小线性增长。`cur_idx` 与 `last_rank` 都是 `usize` 下标：前者标记已输出行数，后者缓存已确认的累计 peer 边界。`last_rank` 不会在同一分区中回退，这正是避免每行从头扫描的关键。

内存计量分成两部分：`DEF_PARTIAL_RESULT_CUME_DIST_SIZE` 只报告空状态的固定大小；`update` 对每个新行报告 `DEF_ROW_SIZE`。这与 Go 的固定 partial result 大小加 `len(rowsInGroup) * DefRowSize` 约定一致，但它是协议化估值，不包括 `T` 的真实堆内存、`Vec` 扩容余量或比较器捕获的数据。

`#[derive(Clone, Debug, PartialEq)]` 允许复制、调试和比较整个状态，复制会克隆 `rows`。生产热路径若使用 `clone`，成本同样随缓存行数增长；当前文件内没有主动克隆。

## 依赖与调用关系

直接依赖只有标准库的 `std::cmp::Ordering`、`std::mem::size_of`，以及同 crate [`func_rank.rs`](./func_rank.rs) 的 `DEF_ROW_SIZE`。目标文件没有直接使用 `Cargo.toml` 所列的其他工作区依赖，说明核心状态机刻意保持为与具体 chunk、表达式和排序规则解耦的泛型实现。

已验证的 Rust 关系如下：

- [`lib.rs`](./lib.rs) 声明并公开 `func_cume_dist` 模块。
- [`builder.rs`](./builder.rs) 的 `build_window_function` 将 `FunctionName::CumeDist` 映射为 `AggImplementation::CumeDist`，保留结果列序号和描述符元数据。
- [`func_cume_dist_test.rs`](./func_cume_dist_test.rs) 直接覆盖固定大小、内存增量、重置、peer 推进和自定义比较器。
- [`window_func_test.rs`](./window_func_test.rs) 的 `collect_cume_dist` 直接构造状态机，覆盖无 `ORDER BY` 时所有行同组以及有序唯一值时逐行增长的 Go 场景。

RustCodeGraph 对 `CumeDist` 给出的文件使用者也是上述两份测试；补充的全仓 `.rs` 引用检索仅找到 builder 枚举映射和这两份测试，未找到从 `AggImplementation::CumeDist` 到 `CumeDist<T>` 的生产实例化边。扩展或集成时不能把枚举映射误当成完整运行时接线。

## 错误处理与边界

本 API 不返回 `Result`，也不产生自定义错误。正常结束以 `next_by`/`next` 的 `None` 表示；空分区第一次求值就返回 `None`，因此不会发生除以零。`update` 接受空迭代器时增量为零。

调用契约没有由类型系统强制：

- 输入必须让 peer 行连续；若相等行被其他行隔开，算法会把它们当成不同组，结果不再符合 SQL `CUME_DIST`。
- 同一 peer group 内，比较器对任意当前行与候选行必须稳定、对称地返回 `Equal`。比较器 panic 会直接向上传播。
- 开始消费后继续追加虽不会被 API 禁止，但调用者必须保证追加仍维持已消费前缀和 peer 边界的一致性；稳妥的生命周期是先收齐一个分区，再逐行消费。
- `next` 只能表达 Rust `PartialEq` 的相等关系，不能自行处理 SQL NULL、collation、多列或表达式排序；这些情况应使用与上层 `ORDER BY` 语义一致的 `next_by` 比较器。
- 行数和估算内存增量的整数转换没有显式溢出处理；这只会在超出可实际容纳的极端分区规模附近成为风险。

## 并发与资源生命周期

`CumeDist<T>` 是普通拥有型状态，没有锁、原子变量、任务、通道、事务或 I/O。`update`、`reset` 和求值方法都要求 `&mut self`，所以一个状态实例的修改天然需要独占访问；是否可跨线程传递由泛型 `T` 的自动 `Send`/`Sync` 性质决定，本文件没有额外并发保证。

建议生命周期是“每个窗口分区一个可复用状态”：创建或重置 → 收集全部行 → 每行调用一次求值 → 重置后进入下一分区。`reset` 的 `Vec::clear` 会析构已有元素但保留 allocation；状态析构时 `Vec` 最终释放缓存。大分区后复用可能长期保留较大 capacity，本实现没有 `shrink_to_fit` 或溢写磁盘策略。

## 与 Go 版本的对应关系

Go 对照实现位于 [`func_cume_dist.go`](./func_cume_dist.go)：`partialResult4CumeDist.curIdx/lastRank/rows` 分别对应 Rust 的 `cur_idx/last_rank/rows`；`ResetPartialResult` 对应 `reset`；`UpdatePartialResult` 对应 `update`；`AppendFinalResult2Chunk` 的 peer 扫描、游标推进和 `lastRank/numRows` 对应 `next_by`。

两者保留了相同的关键语义和内存估算：缓存整个分区、peer 组共享末秩、每批新增行按固定 `DefRowSize` 计量。Rust 的差异主要是接口拆分：

- Go 的 `cumeDist` 同时嵌入 `baseAggFunc` 和 `rowComparer`，直接向结果 `chunk` 的目标列追加 `float64`；Rust 状态机只返回 `Option<f64>`，结果落列由尚未在本文件中出现的上层负责。
- Go 的 `buildCumeDist` 通过 `buildRowComparer(orderByCols)` 注入真实 SQL 比较规则；Rust 用调用时参数 `compare` 保持泛型，`next` 只是简单等值便捷路径。
- Go `AllocPartialResult` 同时返回新状态和固定内存增量；Rust 用 `Default` 创建状态，把固定大小公开为独立常量。
- Go 生产路径由 `BuildWindowFunctions` 明确调用 `buildCumeDist`；当前 Rust 证据只确认 builder 选出实现枚举，未确认状态机的生产实例化与结果写出。

Go [`func_cume_dist_test.go`](./func_cume_dist_test.go) 验证 partial result 固定大小和按行更新的内存变化；Go [`window_func_test.go`](./window_func_test.go) 验证单行、无排序 peer 组和四个唯一排序值的输出。Rust 两份独立测试覆盖这些意图，并额外覆盖重复 peer 序列、自定义比较器、耗尽返回 `None` 和 reset 复用。

## 扩展指南

若只调整累积分布状态算法，应修改 `CumeDist::update`、`next_by` 或 `reset` 中最小必要的符号，并同步 [`func_cume_dist_test.rs`](./func_cume_dist_test.rs)；不要把测试内嵌回生产源文件。涉及 SQL peer 语义时优先增加 `next_by` 的多字段、NULL/collation 等上层比较器测试，而不是增强 `next` 去猜测数据库排序规则。

若完成 Rust 生产接线，至少需要从 `builder.rs` 的 `AggImplementation::CumeDist` 追到执行期实例工厂、将真实 `ORDER BY` 比较器传给 `next_by`，并把 `f64` 写入正确结果列；同时新增独立集成测试证明这条链路，而不能仅以 builder 枚举或零测试编译作为完成证据。应特别核对 Go `BuildWindowFunctions/buildCumeDist` 的 ordinal、无 `ORDER BY` 时全分区同 peer、NULL 和多列排序行为。

性能方面应保留单调 `last_rank`，避免退化为每行重新扫描整个分区的 `O(n^2)`；内存方面若改变 `T` 的所有权或计量协议，必须同步 `DEF_PARTIAL_RESULT_CUME_DIST_SIZE`、`DEF_ROW_SIZE` 约定及内存测试。若引入流式或 spill 方案，需要证明仍能在输出当前行前得知整个 peer group 的末端和分区总行数。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `node --file pkg/executor/aggfuncs/func_cume_dist.rs`：读取全部 88 行，并报告使用者为 `func_cume_dist_test.rs`、`window_func_test.rs`。
- RustCodeGraph `query CumeDist`、`query DEF_PARTIAL_RESULT_CUME_DIST_SIZE`、`node CumeDist`：核对目标结构、常量、builder 枚举及直接测试导入；精确方法名 `next_by` 未被当前索引单独建成可查询 method 节点，因此以目标文件源码和直接测试补证。
- RustCodeGraph 文件节点：读取 [`lib.rs`](./lib.rs)、[`builder.rs`](./builder.rs)、[`func_cume_dist_test.rs`](./func_cume_dist_test.rs)、[`window_func_test.rs`](./window_func_test.rs)、[`func_cume_dist.go`](./func_cume_dist.go)、[`builder.go`](./builder.go)、[`func_cume_dist_test.go`](./func_cume_dist_test.go) 和 [`window_func_test.go`](./window_func_test.go) 的相关段落。
- [`Cargo.toml`](./Cargo.toml)：核对 crate 名称 `astersql-executor-aggfuncs`、crate 根和包移植元数据；目标文件自身只使用标准库及同 crate 的 `DEF_ROW_SIZE`。
- `rg` 补充检索：核对所有 Rust `CumeDist`/常量/实现枚举引用，以及 Go 的 builder、实现和测试位置；该检索用于补足 RustCodeGraph 未生成的方法调用边。
- 本任务是纯文档分析，按计划不运行 Cargo。最终结构校验要求本文恰有十一个约定的二级标题，并人工复核“文件为何存在、如何运行、如何安全扩展”均能由上述源码和测试追溯。
