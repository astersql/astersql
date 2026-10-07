# `pkg/planner/cardinality/exponential.rs`

## 文件定位

本文件属于 `astersql-planner-cardinality` crate（见 `pkg/planner/cardinality/Cargo.toml`），提供基数估算子系统共享的指数退避数值组合器。模块在 `pkg/planner/cardinality/lib.rs` 中以私有模块 `mod exponential` 装配，再通过 `pub use exponential::*` 将两个公开符号提升到 crate 根。它不负责收集统计信息或决定排序方向，只对调用方准备好的浮点序列做组合和边界裁剪。

RustCodeGraph 将本文件识别为两个符号，并指出生产使用文件为 `pkg/planner/cardinality/ndv.rs` 与 `pkg/planner/cardinality/row_count_index.rs`。因此它位于“单列统计值已经取得”与“列组 NDV / 多列索引选择率形成最终估计”之间，而不是 SQL 规划入口本身。

## 核心职责

- `ApplyExponentialBackoff` 将预先排序的值按 `v[0] * v[1]^(1/2) * v[2]^(1/4) * v[3]^(1/8)` 组合，让越靠后的证据权重越小。
- `MaxExponentialBackoffCols` 把参与计算的值限制为前四个；第五项及以后不影响结果。
- 最终结果按调用方提供的 `lowerBound` 和 `upperBound` 裁剪，因此同一算法可服务于大于等于 1 的 NDV 和通常位于 `[0, 1]` 的选择率。
- 空输入采用 `lowerBound`，单值输入跳过指数组合但仍执行相同的上下界裁剪。

这里的“预先排序”是接口契约而非内部保证。`ndv.rs::estimateNDVWithExponentialBackoff` 按 NDV 降序排列，使最大 NDV 获得权重 1；`row_count_index.rs::expBackoffEstimation` 按选择率升序排列，使最有选择性的条件获得权重 1。

## 主要符号

- `pub const MaxExponentialBackoffCols: usize = 4`：指数退避最多读取的序列前缀长度。它还被 `row_count_index.rs` 用于计算选择率下界，以及限制追加 handle 列可获得的衰减权重，修改它会影响不止本函数。
- `pub fn ApplyExponentialBackoff(sortedValues: &[f64], lowerBound: f64, upperBound: f64) -> f64`：无分配、无错误返回的纯数值函数。切片采用共享借用；函数不修改或重新排序输入。
- 局部变量 `result`：从第一个值开始累乘，保留全权重项。
- 局部变量 `maxCols`：`4` 与输入长度的较小值，保证遍历不越界并忽略多余列。
- 局部变量 `weighted`：当前值的副本；对索引 `index` 连续开平方 `index` 次，等价于非负输入上的 `value^(1/2^index)`。

文件没有类型、trait、`impl`、条件编译项、堆分配或外部 crate 导入。

## 执行流程

1. 输入为空时直接返回 `lowerBound`；这是无法形成组合统计值时的保守结果。
2. 输入只有一个值时，计算 `sortedValues[0].min(upperBound).max(lowerBound)` 后返回。
3. 多值路径以首项初始化 `result`，并把参与项数限制为 `min(4, len)`。
4. 从索引 1 开始遍历：索引 1 开一次平方根，索引 2 开两次，索引 3 开三次；每个衰减值依次乘入 `result`。
5. 乘积先对 `upperBound` 取最小值，再对 `lowerBound` 取最大值，得到最终结果。

复杂度为 `O(min(n, 4)^2)` 次常量规模操作，额外空间为 `O(1)`；由于上限固定为 4，实际运行时间有常数上界。

## 数据与状态

函数消费 `&[f64]`，只维护栈上的 `usize` 和 `f64` 局部变量，不写全局状态。排序顺序决定哪一项获得哪一级权重，但该顺序不会被编码或校验：相同集合以不同顺序传入可能得到不同结果。

两个已验证的调用语境如下：

- `ndv.rs::estimateNDVWithExponentialBackoff` 收集正的单列 NDV，按降序排序，以最大单列 NDV 为下界、行数为上界；若行数不大于下界则在调用前直接回退。
- `row_count_index.rs::expBackoffEstimation` 收集各索引列的选择率，按升序排序，以索引可用下界和前四个单列选择率中的最小值共同形成下界，以 `1.0` 为上界。

`row_count_index.rs::AdjustRowCountForAppendedHandleColumns` 使用同一常量并手工实现错开一档的平方根权重，但没有调用本函数：已有 prefix 估计占据全权重位置，追加列从 `1/2` 权重开始，不能直接用本函数当前签名替代。

## 依赖与调用关系

上游装配关系是 `lib.rs -> mod exponential -> pub use exponential::*`。RustCodeGraph 的文件节点报告 `ndv.rs` 和 `row_count_index.rs` 使用本文件；源码检索确认直接函数调用边为：

- `EstimateColsNDVWithSessionVars -> estimateNDVWithExponentialBackoff -> ApplyExponentialBackoff`。该结果可按会话变量 `RiskGroupNDVSkewRatio` 与保守 NDV 插值，供 Join 等列组基数估算使用。
- `GetRowCountByIndexRanges` 的索引估算路径 -> `expBackoffEstimation -> ApplyExponentialBackoff`。返回的组合选择率与独立性下界、单列上界一起用于索引行数估算。

下游只依赖 Rust 标准浮点运算：`f64::sqrt`、`f64::min`、`f64::max` 和切片迭代。虽然所属 crate 声明了统计、表达式、ranger 等多项路径依赖，本文件自身不直接引用它们，也没有 feature 分支。

## 错误处理与边界

函数不返回 `Result`，合法统计输入下没有可传播错误。已由 `exponential_test.rs::test_apply_exponential_backoff` 覆盖的边界包括：空输入返回下界、单值裁剪、乘积低于下界、乘积高于上界、第五项被忽略，以及 NDV/选择率两类数值。

调用方必须维护以下前置条件：值已按目标语义排序；参与开平方的值非负且最好为有限数；边界表达有效区间，通常应满足 `lowerBound <= upperBound`。函数不验证这些条件。若下界大于上界，当前“先 `min` 后 `max`”顺序会返回下界；负数在后续位置开平方会产生 `NaN`。Rust `f64::min/max` 与 Go `math.Min/Max` 对 `NaN` 的传播规则不同，因此非法或 `NaN` 输入不应被视为跨语言兼容契约。生产调用方目前会筛掉非正 NDV，并只收集 `0 < selectivity < 1` 或来自有效统计的选择率，但本函数本身没有防线。

## 并发与资源生命周期

本文件没有锁、原子量、通道、异步任务、线程局部或 I/O。函数只读取调用期内有效的借用切片，返回前不保存任何引用；所有中间值随栈帧结束释放。常量是不可变编译期数据，因此函数可被多个线程并发调用，是否满足业务不变量完全取决于每次调用传入的独立数值与边界。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/cardinality/exponential.go`。Rust 保留了 Go 的公开名称、四列上限、空/单值分支、连续开平方实现、乘积顺序和最终裁剪顺序。参数从 Go 的 `[]float64` 映射为 Rust 的只读 `&[f64]`，`min(MaxExponentialBackoffCols, l)` 映射为常量方法 `.min(sortedValues.len())`，没有算法简化。

Rust 测试 `pkg/planner/cardinality/exponential_test.rs::test_apply_exponential_backoff` 与 Go 测试 `pkg/planner/cardinality/exponential_test.go::TestApplyExponentialBackoff` 使用同组案例：一至五个 NDV、一至四个选择率、上下界裁剪和空输入。Rust 额外通过独立测试文件接入 `lib.rs` 的 `#[cfg(test)] #[path = "exponential_test.rs"]`，遵守生产逻辑与测试分文件的仓库约定。

可观察差异集中在非法浮点输入：Rust 的 `f64::min/max` 和 Go 的 `math.Min/Max` 对 `NaN` 不同；现有测试与生产调用契约均未把 `NaN` 纳入支持范围。Go 调用方在 schema 列缺失时还会记录日志，而对应 Rust NDV 调用方静默回退；该差异位于 `ndv.rs`，不是本文件算法的差异。

## 扩展指南

- 调整权重或最大列数时，优先修改 `ApplyExponentialBackoff` 与 `MaxExponentialBackoffCols`，并同步审查 `row_count_index.rs::expBackoffEstimation` 的下界循环和 `AdjustRowCountForAppendedHandleColumns` 的手写衰减循环，避免三处权重语义漂移。
- 增加输入校验时必须先决定跨 Go/Rust 的兼容策略，尤其是负数、`NaN`、无穷值和反向边界；若改变合法返回域，应同步修改 Go 实现而非只让 Rust 测试通过。
- 支持不同排序/权重策略时，建议由调用方显式选择或新增清晰命名的入口，不要在本函数内部猜测 NDV 与选择率语义。
- 回归测试应继续放在独立的 `pkg/planner/cardinality/exponential_test.rs`，并同步 Go 的 `exponential_test.go`；修改常量还应覆盖 `row_count_index_test.rs` 和相关 NDV 测试，因为调用者的边界与估值都会变化。
- 性能风险主要来自把固定四列上限改为无界遍历或高成本幂运算；兼容风险来自改变浮点运算顺序，因为估算结果及容差可能发生可观察变化。

## 验证依据

- RustCodeGraph：`status` 确认本仓库索引包含目标；`files --filter pkg/planner/cardinality` 确认源、Go 对照和独立测试；`node --file pkg/planner/cardinality/exponential.rs --offset 1 --limit 240` 返回完整 53 行源码、两个符号及 `ndv.rs`/`row_count_index.rs` 使用关系；`query ApplyExponentialBackoff --kind function --json` 区分 Rust、Go 与 Go 测试符号。精确 `callers` 查询在 30 秒内未返回，调用边改由下列源码检索核实。
- Rust 源与装配：`pkg/planner/cardinality/exponential.rs`、`pkg/planner/cardinality/lib.rs`、`pkg/planner/cardinality/Cargo.toml`。
- 直接调用证据：`pkg/planner/cardinality/ndv.rs::estimateNDVWithExponentialBackoff`、`pkg/planner/cardinality/row_count_index.rs::expBackoffEstimation`；同一常量的关联逻辑见 `AdjustRowCountForAppendedHandleColumns`。
- Go 对照：`pkg/planner/cardinality/exponential.go`、`ndv.go`、`row_count_index.go`。
- 测试证据：`pkg/planner/cardinality/exponential_test.rs` 与 `pkg/planner/cardinality/exponential_test.go`。本任务是纯文档分析，按计划不运行 Cargo。
- 结构验收使用任务指定命令，要求目标文件存在且恰有上述 11 个固定二级标题。
