# `pkg/executor/aggfuncs/func_percentile.rs`

## 文件定位

`result_by` 用调用方提供的比较器执行选择；对实现 `Ord` 的类型由 `result` 提供默认入口，对 `f32`/`f64` 分别由专用方法处理偏序比较。

## 核心职责

- `ordinal_rank` 计算 TiDB 使用的从 1 开始的序数秩：`ceil(row_count * percent / 100)`，上限截断为样本数。
- `Percentile<T>` 持有固定百分比与一组非 NULL 样本，支持构造、重置、追加、合并、查询和选择结果。
- `result_by` 用调用方提供的比较器执行选择；对实现 `Ord` 的类型由 `result` 提供默认入口，对 `f32`/`f64` 分别由专用方法处理偏序比较。
- 五个公开类型别名把 SQL 侧整数、实数、十进制、时间和时长路径映射到统一泛型状态。
- `DEF_SLICE_SIZE` 提供空 `Vec` 头部大小，供与 Go `DefSliceSize` 的内存记账语义对照。

本实现是精确收集全部样本后做 nth-selection，虽然 SQL 函数名是 APPROX_PERCENTILE，但本文件没有近似摘要或采样结构。

## 主要符号

- `pub const DEF_SLICE_SIZE: i64`：`size_of::<Vec<()>>()`，表示 Rust 空向量结构本身的固定开销；元素占用另行计算。
- `pub fn ordinal_rank(row_count: usize, percent: i32) -> usize`：计算并截断序数秩。它只执行数学转换，不验证百分比是否落在 `0..=100`。
- `pub struct Percentile<T>`：核心状态。`percent` 私有且构造后不变；`data` 为 `pub(crate)`，以便同 crate 的 spill 实现访问。
- `Percentile::new`：以目标百分比创建空状态。
- `Percentile::reset`：用新 `Vec` 替换旧缓冲，确保容量归零并释放旧 backing storage。
- `Percentile::update`：接收任意 `IntoIterator<Item = Option<T>>`，通过 `flatten` 丢弃 `None`，返回“新增元素数 × `size_of::<T>()`”的内存增量估算。
- `Percentile::merge_from`：新建足够容量的缓冲，按 destination 在前、source 在后的次序移动元素；结束后 source 为空。
- `Percentile::result_by`：先计算秩，再用 `slice::select_nth_unstable_by(rank - 1, ...)` 原地选择，返回所选元素引用。
- `Percentile::values`：暴露当前样本的只读切片，供 spill 和验证观察状态。
- `Percentile::capacity`：仅在 `cfg(test)` 下可见，用于验证 reset 是否释放容量。
- `Percentile::result`：仅对 `T: Ord` 提供，使用 `Ord::cmp`。
- `result_float32` / `result_float64`：浮点专用入口；`partial_cmp` 返回 `None` 时按 `Ordering::Equal` 处理。
- `PercentileOriginal4Int`、`PercentileOriginal4Real`、`PercentileOriginal4Decimal`、`PercentileOriginal4Time`、`PercentileOriginal4Duration`：与 Go 类型化状态命名对齐的别名；当前真实 builder/spill 接线直接写 `Percentile<T>`，不依赖这些别名。

## 执行流程

1. `builder.rs::build_approx_percentile` 校验聚合模式、读取第二个常量参数作为 `percent`，再按第一个参数的求值类型选择实现；不支持的类型选择 `PercentileNull`。
2. spill 注册时，`aggfuncs.rs::BuiltAggFunc::spill_function` 根据 `ValueKind` 调用 `Percentile::<T>::new(percent)` 创建类型化模板和初始 partial result。
3. 状态更新调用 `Percentile::update`：跳过 `None`，把有效值依次追加到 `data`，并报告按元素静态大小计算的增量。
4. 多个状态汇合时，`merge_from` 分配新缓冲，将目标已有元素置前、源元素置后，并把源缓冲移空。`aggfuncs.rs::merge_spilled_partial_result` 因接收只读 source，会先克隆 source，再调用该方法。
5. 求值时，`result_by` 计算从 1 开始的秩；空集或 0% 得到秩 0，返回 `None`。其他情况转换为零基下标，执行不稳定 nth-selection 并返回引用。
6. spill 时，`spill_serialize_helper.rs` 的 `SpillState for Percentile<T>` 顺序写出所有样本；恢复时替换 `data` 并按 Go 约定报告元素内存，时间/时长固定按 8 字节、十进制按 `MyDecimal` 大小计费。

注意 `select_nth_unstable_by` 会重排 `data`；只保证选中位置满足分区关系，不保证其余元素保持输入顺序或完全排序。

## 数据与状态

`Percentile<T>` 的持久状态只有 `percent: i32` 与 `data: Vec<T>`。百分比由 `new` 写入后没有 setter；样本由 `update`、`merge_from`、spill 恢复或 `reset` 改变。

内存记账有三个层次：`DEF_SLICE_SIZE` 是空向量头部；`update` 只按新增长度和 `size_of::<T>()` 返回逻辑元素增量，不依据容量增长；spill 恢复则由相邻模块按 Go 的类型计费规则计算。因而这些值是兼容性记账，不等同于 Rust allocator 的精确实时分配量。

合并的状态不变量是：目标包含“原目标 + 原源”的全部元素，源的 `values()` 为空。重置的不变量是数据为空且容量为零。结果选择可能改变样本排列，但不增删样本。

五个 SQL 对应类型来自本 crate 的本地值模型：整数为 `i64`，实数别名为 `f64`，十进制为 `func_sum::Decimal`，时间和时长分别为 `func_max_min::TimeValue` 与 `DurationValue`。

## 依赖与调用关系

上游直接关系如下：

- `lib.rs` 声明并公开模块，测试模式下装配独立文件 `func_percentile_test.rs`。
- `builder.rs::build_approx_percentile` 产生携带 `percent` 和 `ValueKind` 的构建结果。
- `aggfuncs.rs::BuiltAggFunc::spill_function` 创建五种 `Percentile<T>`；`merge_spilled_partial_result` 对恢复后的五种状态调用 `merge_from`。
- `spill_serialize_helper.rs::SpillState for Percentile<T>` 使用 crate 可见的 `data` 完成复制、写出和恢复。
- `export_test.rs` 将 `ordinal_rank` 重导出为测试名 `PercentileForTesting`，验证测试导出复用生产算法。

本文件的直接下游只有标准库的 `Ordering`、`size_of`、`Vec` 和切片 `select_nth_unstable_by`，以及相邻模块定义的 `Decimal`、`TimeValue`、`DurationValue`。Cargo 中这些本地类型最终由 `astersql-types`、`astersql-util-serialization` 等 crate 支撑，但本文件没有直接导入外部 crate。

RustCodeGraph 将目标文件标为被 `aggfuncs.rs`、`func_sum.rs`、`spill_serialize_helper.rs` 等 21 个文件使用；精确的泛型方法 `callers/callees` 查询未生成调用边，因此上述生产接线又用模块内定向符号搜索核实。

## 错误处理与边界

- 本文件所有 API 都不返回 `Result`，也不执行表达式求值，因此没有可传播的求值错误；Go `UpdatePartialResult` 中的表达式错误路径不在本文件内。
- 空样本和 `percent == 0` 都令 `result_by` 返回 `None`。上层若要产生 SQL NULL，必须把 `None` 写成 NULL；该写出逻辑不在本文件。
- `ordinal_rank` 不验证负数或大于 100 的百分比。大于 100 时结果被 `.min(row_count)` 截断；负数经浮点转无符号整数的 Rust 饱和转换得到 0。合法性应由描述符构建或更上游保证，扩展时不能依赖本函数报错。
- 对空集，100% 仍返回秩 0；`func_percentile_test.rs` 明确覆盖此边界。
- 浮点 NaN 导致 `partial_cmp` 返回 `None` 时被当作相等。由于 nth-selection 需要一致的比较关系，若 SQL 层允许 NaN，其具体选中项可能依赖输入排列，文档不能宣称全序语义。
- `result_by` 的比较器若不满足稳定的一致次序，选择结果由调用方承担；函数本身没有校验。

## 并发与资源生命周期

状态使用普通 `Vec<T>` 和 `&mut self` 修改，没有内部锁、原子、任务或通道；它不是共享并发容器。执行器若并行聚合，应让每个 worker/分组独占自己的 partial result，再通过合并阶段汇总。

`update` 复用现有容量；`reset` 主动换成新空向量并释放旧缓冲；`merge_from` 分配一个恰能容纳两侧样本的新缓冲，并移动而非克隆元素；`result_by` 在现有缓冲上原地工作，不创建完整排序副本。`merge_spilled_partial_result` 为适配只读 source 会在外层克隆源状态，这一额外成本来自调用点，不是 `merge_from` 本身。

时间复杂度方面，更新与合并均与处理元素数线性相关；nth-selection 通常是线性选择而非 `O(n log n)` 全排序，但会破坏输入顺序。空间规模始终与全部非 NULL 样本数线性相关，因此大分组的主要风险是内存，而非摘要误差。

## 与 Go 版本的对应关系

`ordinal_rank` 对应其 `percentile` 中的 `ceil(P/100*N)` 与样本数截断；Rust 返回从 1 开始的 rank，调用点再减 1，而 Go 的 `selection.Select` 负责其索引约定。

Go 为 int、real、decimal、time、duration 分别定义 partial-result slice 和执行器类型；Rust 用一个 `Percentile<T>` 及五个别名复用状态逻辑。

重要差异是 Go 文件同时实现 `AggFunc` 生命周期：分配、表达式求值、合并、最终写入 chunk、spill 序列化/恢复；Rust 本文件只实现状态与选择算法，spill 接线位于相邻文件，且当前没有 `AggFunc for Percentile<T>` 的直接实现证据。Go 的 real 路径只有 `float64`；Rust 额外提供未见生产 builder 使用的 `f32` 结果方法。Go 的 decimal 注释保留“大值复制”的 TODO，Rust 同样把 `Decimal` 值直接存进向量，没有在本文件采用指针间接层。

Go `func_percentile_test.go::TestPercentile` 经完整聚合测试器覆盖 SQL 类型；Rust 独立测试目前直接测试状态和 spill，而非本文件到 `Chunk` 的完整聚合接口。

## 扩展指南

- 新增受支持 SQL 类型时，至少需要同步：本文件的必要类型别名/比较入口、`builder.rs::build_approx_percentile` 的 `ValueKind` 映射、`aggfuncs.rs::spill_function` 与 `merge_spilled_partial_result`、`spill_serialize_helper.rs` 的 `SpillElement`/记账语义，以及独立的 `func_percentile_test.rs` 和 spill 测试。
- 若新增类型没有 `Ord`，应像浮点路径一样在本文件提供明确比较器，并记录 NULL、NaN、排序规则或时区语义；不要通过任意 fallback 隐藏不可比情况。
- 若调整百分位定义，必须同时核对 `ordinal_rank`、零基下标转换、0/100/空集行为以及 Go `percentile`，并扩展 `export_test.rs` 的规范边界测试。
- 若接入完整执行路径，应在独立生产模块实现 `AggFunc` 所需的表达式求值、错误传播和 `Chunk` 写出，并在独立测试文件验证；不要把测试嵌入本源文件。
- 若改变内存报告，必须区分向量头、元素逻辑大小、容量和 allocator 实际分配，并同步 spill 恢复对 Time/Duration/Decimal 的 Go 兼容计费。
- 若为大分组改成近似摘要，属于算法和状态格式变更：需要定义精度、合并律、spill 兼容及旧状态迁移，不能只替换 `result_by`。

兼容风险主要集中在序数秩的 1/0 基转换、NULL/空组输出、浮点 NaN、时间/时长比较及 spill 格式；性能风险集中在保存全部样本、合并时重新分配以及外层为只读 source 做克隆。
