# `pkg/statistics/handle/internal/testutil.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-statistics-handle-internal`（见同目录 `Cargo.toml`），是 statistics handle 子系统的内部测试辅助实现，而不是线上 SQL 执行路径。crate 根 `pkg/statistics/handle/internal/lib.rs` 以私有 `mod testutil` 装载本文件，再通过 `pub use testutil::*` 对外暴露公开断言 `AssertTableEqual`；同一 crate 的测试由独立文件 `testutil_test.rs` 承载。

该辅助函数用于核对两份 `statistics::Table` 快照中测试关心的表级统计是否一致。Go 对照 `pkg/statistics/handle/internal/testutil.go` 的调用场景包括统计信息持久化后清缓存并重新加载（`pkg/statistics/handle/handletest/statstest/stats_test.go`），因此它表达的是“统计核心内容往返后保持一致”的测试契约，不是 `Table` 所有字段的通用 `PartialEq`。

## 核心职责

- `AssertTableEqual` 比较实时行数 `RealtimeCount`、修改计数 `ModifyCount`、列数和索引数。
- 对实际表中的每个列 ID 和索引 ID，在期望表中查找同 ID 项，并比较其 `Histogram`、可选 `CMSketch` 和可选 `TopN`。
- 最后比较 `ColAndIdxExistenceMap`，覆盖列/索引是否存在及是否已分析的标记。
- `top_n_equal` 补足 Rust `Option<TopN>` 与 Go 可 nil 指针之间的语义差异：缺失 TopN 与总计数为零的 TopN 等价。

本文件不会比较 `Table` 的 `Version`、`LastAnalyzeVersion`、`LastStatsHistVersion`、`TblInfoUpdateTS`、`IsPkIsHandle`，也不会比较 `HistColl` 的 `PhysicalID`、`StatsVer`、`Pseudo` 或各种 ID 映射。扩展者不能把函数名中的 “TableEqual” 理解为整对象逐字段相等。

## 主要符号

- `fn top_n_equal(left: Option<&statistics::TopN>, right: Option<&statistics::TopN>) -> bool`：模块私有适配器。先用 `TopN::TotalCount` 将 `None` 映射为 0；两侧总计数都为 0 时直接相等，否则只有两侧均为 `Some` 且 `TopN::Equal` 为真才相等。
- `pub fn AssertTableEqual(actual: &Table, expected: &Table)`：唯一公开 API。参数均为不可变借用；任一检查失败即通过标准 `assert!`、`assert_eq!` 或 `expect` 触发 panic，无返回值。
- `statistics::Table`：被比较的数据载体；`Table` 解引用到 `HistColl`，所以代码可直接访问 `RealtimeCount`、`ModifyCount`、`ColNum` 和 `IdxNum`。具体定义位于 `pkg/statistics/table.rs`。

文件没有模块级常量、struct、enum、trait、`impl` 或条件编译项。

## 执行流程

1. `AssertTableEqual` 先比较两表的 `RealtimeCount`、`ModifyCount` 与 `ColNum()`。前置数量检查确保后续“遍历 actual、在 expected 中逐 ID 查找”同时验证列 ID 集合一致。
2. 遍历 `actual.HistColl.Columns`。每个 ID 必须能由 `expected.GetCol(id)` 找到，否则以 `expected column must exist` panic。
3. 对每列调用 `statistics::HistogramEqual(..., false)`；`false` 表示不忽略直方图 ID。随后以 `Option<CMSketch>` 的结构相等比较 CMSketch，并以 `top_n_equal` 比较 TopN。
4. 比较 `IdxNum()`，再以同样方式遍历 `actual.HistColl.Indices`；缺失 ID 的 panic 文本为 `expected index must exist`。
5. 调用 `statistics::ColAndIdxExistenceMapIsEqual` 比较整张存在性映射。全部断言通过后函数正常返回 `()`。

列和索引存放于 `HashMap`，遍历顺序不固定；不过函数在第一个不一致处终止，因此失败项的先后次序不应被测试依赖。

## 数据与状态

输入是两份共享借用的 `Table`，函数不克隆、不修改、也不持久化它们。比较涉及：

- 表级可变统计量：实时行数和修改计数。
- `HistColl.Columns` / `HistColl.Indices`：以 `i64` ID 为键的列/索引统计映射。
- 每个统计项的直方图、`Option<CMSketch>` 与 `Option<TopN>`。
- `ColAndIdxExistenceMap`：列、索引存在与 analyzed 状态的映射。

`top_n_equal` 每次调用都会通过 `TotalCount()` 汇总 TopN 条目计数；当前 `TopN::TotalCount` 对向量做线性求和。因此整个断言除遍历列/索引及比较其内部结构外，还会对每个 TopN 做一次计数扫描。它不缓存状态，也没有全局可变数据。

## 依赖与调用关系

直接依赖只有 `statistics` crate；`pkg/statistics/handle/internal/Cargo.toml` 将其映射到本仓库 `pkg/statistics`。测试期额外依赖 `astersql-statistics-handle-cache-internal-testutil`，用于构造 mock 表，但该依赖只出现在 `testutil_test.rs`，不进入本文件实现。

RustCodeGraph 对 `AssertTableEqual` 给出的直接调用者为：

- `assert_table_equal_accepts_equivalent_statistics`
- `assert_table_equal_rejects_different_table_counts`
- `assert_table_equal_accepts_zero_count_topn_as_empty`

三者均位于 `pkg/statistics/handle/internal/testutil_test.rs`。仓库搜索还显示 `pkg/statistics/handle/handletest/statstest/Cargo.toml` 声明了本 crate 依赖，但当前没有发现该 Rust 测试 crate 调用 `AssertTableEqual`；因此不能声称它已经接入更广的 Rust handle 测试主链。

下游调用包括 `Table::GetCol`、`Table::GetIdx`、`HistogramEqual`、`TopN::TotalCount`、`TopN::Equal` 和 `ColAndIdxExistenceMapIsEqual`。其中 RustCodeGraph 明确解析了 `AssertTableEqual -> GetCol/GetIdx` 以及 `AssertTableEqual -> top_n_equal` 的边；其余自由函数/方法由目标源码和调用表达式核验。

## 错误处理与边界

本工具专为测试失败而设计，不返回 `Result`。计数、直方图、CMSketch、TopN 或存在图不相等都会 panic；期望侧缺少 actual 中的列/索引也会 panic。因为先检查映射数量，再逐个验证 actual 的键都存在于 expected，相同数量下不会漏掉 expected 独有的键。

CMSketch 使用 `Option` 的严格相等：`None` 只与 `None` 相等，`Some` 内容由 `CMSketch` 的 `PartialEq` 判定。这与 Go 版本先区分 nil、非 nil 再调用 `Equal` 的分支意图一致。TopN 则刻意不同于普通 `Option` 相等：`None`、空 TopN、以及仅含零计数条目的 TopN，只要总计数均为零就视为等价；若任一侧总计数非零，则要求两侧都存在且 `TopN::Equal` 成立。

直方图比较传入 `ignore_id = false`，所以 ID 是契约的一部分。函数没有为整数溢出、畸形统计结构或 panic 恢复提供额外处理；它假定输入已是可供测试比较的内存统计快照。

## 并发与资源生命周期

函数只持有调用期间有效的共享引用，不获取锁、不启动任务或线程、不使用通道、事务、文件或网络资源。它本身没有可观察的资源清理阶段。能否并发调用取决于 `Table` 及其成员的线程安全约束，但本函数不会引入额外共享状态。

唯一临时工作是迭代 HashMap、借用期望项，以及 `TopN::TotalCount` 的计数累加；所有借用和栈上临时值都在同步调用返回或 panic 展开时结束。

## 与 Go 版本的对应关系

Go 基准实现是同目录 `testutil.go::AssertTableEqual(t, a, b)`。Rust 版本保留了相同的比较顺序和范围：实时/修改计数、列数量、逐列 histogram/CMSketch/TopN、索引数量、逐索引三类统计，最后是存在性映射。

主要语言适配如下：

- Go 通过 `testing.T` 与 `require` 报告断言；Rust 不接收测试上下文，直接使用标准断言并 panic。
- Go 用 `ForEachColumnImmutable` / `ForEachIndexImmutable`；Rust直接借用公开的 `Columns` / `Indices` HashMap。两者都只读遍历所有条目。
- Go 的 CMSketch 指针需要显式 nil 分支；Rust 的 `Option<CMSketch>` 直接使用派生相等，维持 nil/非 nil 区分。
- Go `(*TopN).Equal` 可处理 nil 接收者。Rust `TopN::Equal` 只接受具体引用，因此由 `top_n_equal` 把 `Option` 和零总计数语义补齐；独立 Rust 测试明确覆盖 `None` 与含零计数条目的 `Some(TopN)` 等价。
- Go 函数在真实 handle 测试中比较存储前后的统计表；当前 Rust 图查询只发现本 crate 的三个单元测试调用者，说明 API 已移植并验证核心契约，但更广测试接线仍有限。

## 扩展指南

若统计快照新增且应参与往返一致性验证的字段，应先确认 Go `AssertTableEqual` 是否同步比较，再在 Rust `AssertTableEqual` 对应阶段加入断言；不要未经契约确认就把所有 `Table` 元数据纳入比较，否则可能把允许变化的版本/运行状态误判为失败。

新增列或索引内部统计结构时，最可能修改两个遍历体。若两者规则相同，可提取私有辅助函数，但仍应保持列/索引缺失时的明确诊断。涉及可选值时必须分别定义 `None` 与“空但存在”的等价规则，不能默认依赖派生 `PartialEq`。

测试必须继续放在独立的 `pkg/statistics/handle/internal/testutil_test.rs`，不要内嵌进生产源文件。至少应覆盖：新增字段相等路径、每类不相等的 panic 路径、缺失列/索引、`None`/空/非空统计结构组合；若改变 Go 对齐语义，还应同步审阅 `pkg/statistics/handle/internal/testutil.go` 及其 `statstest` 调用场景。性能上应避免在每个条目中引入重复的全表扫描。

## 验证依据

- 源文件：`pkg/statistics/handle/internal/testutil.rs`，确认两个函数、可见性、断言顺序和全部字段访问。
- crate 边界：`pkg/statistics/handle/internal/Cargo.toml` 与 `lib.rs`，确认包名、`statistics` 路径依赖、公开再导出和独立测试模块。
- Rust 测试：`pkg/statistics/handle/internal/testutil_test.rs`，确认等价表成功、实时行数差异 panic，以及零计数 TopN 与缺失 TopN 等价。
- 下游实现：`pkg/statistics/table.rs`、`cmsketch.rs`、`histogram.rs`，确认表/映射结构、查找与数量方法、TopN 总计数/相等语义及直方图比较参数。
- Go 对照：`pkg/statistics/handle/internal/testutil.go`；Go 使用证据：`pkg/statistics/handle/handletest/statstest/stats_test.go`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件；`query/node` 定位 Go/Rust 两个 `AssertTableEqual` 和私有 `top_n_equal`；节点调用轨迹确认三个 Rust 测试调用者、`GetCol`/`GetIdx` 被调用关系；`explore` 再确认 `top_n_equal` 与 `AssertTableEqual` 的局部调用关系。
- 仓库搜索：确认 Rust 侧无上述三个单元测试之外的直接调用，并确认 `handletest/statstest/Cargo.toml` 对本 crate 的依赖声明。
