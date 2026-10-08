# `pkg/util/selection/selection.rs`

## 文件定位

本文件是 `astersql-util-selection` crate 的算法实现文件，crate 根由 [`lib.rs`](./lib.rs) 声明 `selection` 模块并重新导出其公共项；crate 清单 [`Cargo.toml`](./Cargo.toml) 指定 `lib.rs` 为库入口，运行时依赖只有 `rand = "0.8"`。根 workspace 又以 `facade_util_selection` 引入该 crate，并在 `pkg/lib.rs` 的 `util::selection` 模块中重新导出，因此 `Interface` 与 `Select` 同时可从独立 crate 和总 facade 到达。

它实现的是“从可比较、可交换的序列中就地找出第 k 小元素”的 introselect，而不是 SQL `SELECT`、执行器筛选算子或完整排序。当前 Rust 仓库搜索只发现 facade/依赖声明和本目录测试对 `Select` 的使用，没有发现生产 Rust 调用点；`pkg/executor/aggfuncs/Cargo.toml` 虽将 `astersql-util-selection` 声明为可选依赖，但 `func_percentile.rs` 当前直接使用切片的 `select_nth_unstable_by`。Go 侧的实际业务入口仍是 `pkg/executor/aggfuncs/func_percentile.go::percentile` 调用 `selection.Select`。

## 核心职责

- 以公开 trait `Interface` 抽象 `Len`、`Less`、`Swap`，保留 Go `sort.Interface` 的操作形状，使算法不需要拥有或复制元素（`selection.rs::Interface`）。
- 以公开函数 `Select` 接受从 1 开始的排名，将其转换为从 0 开始的索引，并对非空序列执行 introselect；空序列返回哨兵 `-1`（`selection.rs::Select`）。
- 前六层使用随机 pivot 的 quickselect；深度预算耗尽后切换到 median-of-medians，限制持续选中劣质 pivot 的风险（`selection.rs::introselect`）。
- 在确定性回退路径中显式聚合与 pivot 相等的元素，使大量重复值时可直接命中整个等值区间（`selection.rs::partitionIntro`）。
- 保留 crate 内可见的纯 quickselect `quickselect`，只作为测试和基准对照，不属于 crate 的公开 API。

算法只保证返回位置上的元素具有第 k 小的值，并会重排输入；它不保证其余元素完全有序，也不保证相等元素的稳定顺序。

## 主要符号

| 符号 | 可见性与签名语义 | 作用 |
| --- | --- | --- |
| `Interface` | `pub trait`；`Len(&self) -> isize`、`Less(&self, i, j) -> bool`、`Swap(&mut self, i, j)` | 调用者提供长度、严格次序比较和原地交换。算法依赖索引合法及比较关系一致。 |
| `Select` | `pub fn Select(&mut dyn Interface, k: isize) -> isize` | 唯一公开算法入口。`k` 是 1-based 排名；非空时调用 `introselect(..., k - 1, 6)`，空输入返回 `-1`。 |
| `introselect` | 私有 | 随机 quickselect 主循环的递归形式；每次分区后只进入包含目标索引的一侧，深度减一。 |
| `quickselect` | `pub(crate)` | 不带深度回退的对照实现，参数 `k` 是 0-based 绝对索引。 |
| `medianOfMedians` | 私有 | 确定性选择入口；取得稳健 pivot 后用 `partitionIntro` 分区并递归目标侧。 |
| `randomPivot` | 私有 | 用 `rand::thread_rng().gen_range(left..=right)` 从闭区间均匀选择 pivot 索引。 |
| `medianOfMediansPivot` | 私有 | 每五个元素排序并把组中位数搬到区间前部，再递归选择这些中位数的中位数。 |
| `partition` | 私有 | 普通 Lomuto 风格分区：严格小于 pivot 的元素移到左侧，最后放置 pivot。 |
| `partitionIntro` | 私有 | 三向语义分区：先收集小于项，再收集与 pivot 等价的项，并按 `k` 返回可缩小递归区间的索引。 |
| `partition5` | 私有 | 对至多五个元素执行原地插入排序，返回小区间中位数索引。 |

文件没有模块级常量、结构体、枚举、`impl` 或条件编译项；条件编译测试模块位于相邻的 `lib.rs`，不在本文件中。

## 执行流程

1. 调用者用实现了 `Interface` 的可变序列调用 `Select(data, k)`。`Select` 读取一次 `Len`；长度为零时立即返回 `-1`，否则把排名换算成 `k - 1`，以全区间 `[0, Len()-1]` 和深度预算 `6` 进入 `introselect`。
2. `introselect` 遇到单元素区间直接返回该索引。预算尚未耗尽时，`randomPivot` 选择 pivot，`partition` 把所有严格小于 pivot 的元素放到左边，并把 pivot 放到最终分区位置。
3. 若 pivot 索引等于目标索引则完成；否则只递归左侧或右侧，因而不会排序无关的另一半。每次递归消耗一层预算。
4. 预算耗尽时，`introselect` 转入 `medianOfMedians`。`medianOfMediansPivot` 将当前区间按最多五项分组，使用 `partition5` 对各组做插入排序，把组中位数紧凑搬到左端，再在中位数区域递归选择中位数的中位数。
5. `medianOfMedians` 使用 `partitionIntro`。后者先形成“小于 pivot”区间，再通过 `!Less(a,b) && !Less(b,a)` 判断比较意义上的等价，把等值项聚集起来。目标落在等值区间时直接返回 `k`；否则返回相应边界，让下一次递归严格缩小到目标所在一侧。
6. 返回值是当前已重排序列中的索引。调用者必须在算法返回后、同一份数据上读取该索引处的元素；测试 `selection_test.rs::test_selection_with_random_case` 正是先保存选中值，再排序副本状态并比较期望值。

## 数据与状态

本文件没有持久结构或全局可变状态。全部算法状态都由递归参数 `left`、`right`、`k`、`depth` 和局部索引组成；元素存储仍由调用者持有，算法仅经 `Interface::Less` 观察、经 `Interface::Swap` 原地修改。

核心分区不变量是：`partition` 返回时，返回索引左边的元素均严格小于 pivot，pivot 位于返回索引；右边元素不严格小于 pivot。`partitionIntro` 进一步维护 `[left, storeIndex)` 小于 pivot、`[storeIndex, storeIndexEq]` 与 pivot 比较等价，并根据目标落点返回左边界、目标本身或等值区间右边界。这里的“相等”是比较器等价，不要求元素实现 `Eq`。

`Select` 的 `k` 是 1-based，而所有内部选择函数的 `k` 都是当前原始序列坐标系中的 0-based 绝对索引。各递归调用不会把 `k` 改成子切片相对索引，这是修改边界逻辑时必须保持的不变量。

## 依赖与调用关系

RustCodeGraph 对精确符号给出的内部调用链为：

```text
Select -> Interface::Len
       -> introselect -> randomPivot -> rand::thread_rng / Rng::gen_range
                      -> partition -> Interface::{Less, Swap}
                      -> medianOfMedians
                           -> medianOfMediansPivot -> partition5
                           -> partitionIntro -> Interface::{Less, Swap}
```

`medianOfMediansPivot` 还会调用 `medianOfMedians` 在已搬到左侧的组中位数区域选 pivot，`introselect`、`quickselect` 和 `medianOfMedians` 各自会递归调用自身。`quickselect` 与主入口不相连，只由 `selection_test.rs` 的基准辅助路径调用。

上游装配关系是 `pkg/util/selection/lib.rs` 重新导出本模块公共项，根 `Cargo.toml` 以 `facade_util_selection` 依赖该 crate，`pkg/lib.rs::util::selection` 再次重新导出。RustCodeGraph 的宽泛同名调用方结果包含其他模块的 `Select`，不能视为本函数调用证据；结合精确仓库搜索，当前可靠的 Rust 使用点仅为本目录测试，未验证到生产 Rust 调用。Go 对照主链则是 `pkg/executor/aggfuncs/func_percentile.go::percentile -> pkg/util/selection/selection.go::Select`。

## 错误处理与边界

该 API 不返回 `Result`，也不主动校验非空输入的排名范围。已验证的正常契约是空输入返回 `-1`，非空输入要求 `1 <= k <= Len()`；若传入 `k <= 0` 或 `k > Len()`，递归可能构造反向区间，随后在随机闭区间或调用者的索引实现中 panic。新增调用者不能把 `-1` 转为 `usize` 后索引，也不能依赖越界排名有稳定返回值。

`Interface` 实现必须满足以下前置条件：`Len()` 在一次选择期间保持一致且能由 `isize` 表示；`Less`/`Swap` 接受算法给出的所有合法索引；`Less` 提供一致的严格弱序；`Swap` 确实交换同一底层序列。实现违反这些条件时，算法可能返回错误元素或 panic，本文件不会包装错误。

重复值通过双向 `Less` 都为假判定为等价。对浮点 NaN 等偏序数据，这一判定可能把不可比较值当作等价值；是否允许这种数据应由具体 `Interface` 实现和调用者决定。随机数只影响分区路径和性能，不影响满足比较契约时的选择结果。

## 并发与资源生命周期

算法是同步、阻塞、原地执行的，不创建线程、异步任务、锁、通道、事务或文件/网络资源。`&mut dyn Interface` 保证一次调用期间对该接口对象的独占可变借用；是否能在多个线程上分别操作不同对象，取决于对象自身，`Interface` 没有 `Send`/`Sync` 约束。

本文件不分配元素缓冲区；median-of-medians 通过交换复用调用者存储。主要额外资源是递归栈和 `rand::thread_rng()` 使用的线程本地随机数生成器。前六层随机选择之后进入确定性回退；回退仍使用递归，因此极端大输入需关注栈深，但不存在需要显式关闭或回滚的资源。调用结束后，线程局部 RNG 由 `rand` 管理，输入序列保持被部分重排后的状态。

## 与 Go 版本的对应关系

Rust `selection.rs` 与同目录 `selection.go` 基本逐函数对应：`Interface` 保留 `sort.Interface` 的三方法形状；`Select` 同样把 1-based 排名减一、空输入返回 `-1`、以 `6` 为 introselect 深度预算；`introselect`、`quickselect`、median-of-medians、两种 partition 以及五元素插入排序的分支和索引计算一致。

可见差异主要是语言适配：Go 使用 `type Interface = sort.Interface`，Rust 定义独立 trait；Go 的 `int` 映射为 Rust `isize`；Go 的 `rand.Int() % width` 映射为 `rand 0.8` 的闭区间 `gen_range`；Rust 借用系统把可变数据访问表达为 `&mut dyn Interface`。这些差异没有改变正常排名语义。

测试也保持对照：`selection_test.go` 与 `selection_test.rs` 都覆盖基本、重复值、百万随机数据和百万逆序数据，并保留 introselect/quickselect/全排序的基准辅助结构；`migration_aster_unit_test.rs` 额外明确覆盖空输入 `-1`、首尾排名、全重复值和较大确定性数据。Go 的 `main_test.go` 负责通用测试环境及 goroutine 泄漏检查，Rust 算法无后台任务，因此没有对应生命周期夹具。

业务接线目前并非逐行对应：Go 百分位聚合仍调用本包 `Select`；Rust `pkg/executor/aggfuncs/func_percentile.rs::Percentile::result_by` 使用标准库 `select_nth_unstable_by`，所以本 crate 当前是已移植并经独立测试覆盖、但未接入该 Rust 百分位路径的公共工具。

## 扩展指南

- 新增公共选择能力时，优先在 `Select` 周围保持 1-based 外部契约，内部继续统一使用 0-based 绝对索引；若要支持越界错误，应设计新的返回类型或兼容入口，不应悄悄改变现有 `-1`/panic 行为。
- 修改 pivot 或分区策略时，重点维护 `partition`/`partitionIntro` 的区间不变量、重复值等价判定，以及每次递归区间严格缩小。性能优化不能退化为完整排序，也不能假设元素可复制或实现 `Eq`。
- 扩展 `Interface` 会影响所有实现者，通常没有必要；算法所需的最小能力就是长度、比较和交换。若引入泛型切片 API，可在保留 trait API 的前提下增加适配层，避免破坏 facade 使用者。
- 生产接线若要让 Rust 百分位聚合复用本 crate，需同时审查 `pkg/executor/aggfuncs/func_percentile.rs` 的 NaN 比较、空集/百分比为零语义与 Cargo feature 接线，不能仅替换函数名。Go 行为基线仍应以 `func_percentile.go::percentile` 和 `selection.go` 为准。
- 测试必须继续放在独立文件：算法变更同步更新 `pkg/util/selection/selection_test.rs`；Go 对齐或迁移契约变化同步更新 `migration_aster_unit_test.rs`，必要时对照 `selection_test.go`。建议补充的边界包括最小/最大合法排名、所有元素相等和刻意触发深度回退；若决定定义非法排名行为，应先写对应回归测试。
- 随机 pivot 会造成执行路径不确定；正确性测试应比较选中值与完整排序后的第 `k-1` 项，而不应断言最终排列或 pivot 序列。性能评估应在独立 benchmark harness 中进行，不把大规模基准嵌入普通单元测试。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/selection` 确认实现、模块入口和 Rust/Go 测试均在索引中。
- RustCodeGraph 文件节点：`node --file pkg/util/selection/selection.rs --offset 1 --limit 260` 读取了完整 210 行源文件并列出 14 个符号；精确 `node` 查询核对了 `Select`、`introselect`、`medianOfMedians`、`medianOfMediansPivot`、`partitionIntro`、`quickselect` 的源码和内部调用边。
- 实现与装配：`pkg/util/selection/selection.rs`、`pkg/util/selection/lib.rs`、`pkg/util/selection/Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`。
- Go 对照与业务入口：`pkg/util/selection/selection.go`、`pkg/executor/aggfuncs/func_percentile.go`。
- 独立测试：`pkg/util/selection/selection_test.rs`、`pkg/util/selection/migration_aster_unit_test.rs`、`pkg/util/selection/selection_test.go`、`pkg/util/selection/main_test.go`。
- Rust 现状交叉核验：`pkg/executor/aggfuncs/func_percentile.rs` 与其 `Cargo.toml`，以及对 `facade_util_selection`、`astersql_util_selection`、`selection::Select`、`Select(` 的限定仓库搜索；未发现本目录测试以外的可靠生产 Rust 调用。
- 本任务是纯文档分析，依计划未运行 Cargo 或代码测试；交付验证仅检查固定十一章节、链接/路径和事实一致性。
