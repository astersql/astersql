# `pkg/executor/aggfuncs/func_varsamp.rs`

## 文件定位

本文件位于 `astersql-executor-aggfuncs` crate；`pkg/executor/aggfuncs/Cargo.toml` 将该 crate 的入口设为 `lib.rs`，而 `lib.rs` 以 `pub mod func_varsamp` 暴露本模块。它是 SQL 聚合函数 `VAR_SAMP` 的 Rust 命名门面，不自行实现聚合算法，而是把 `func_varpop.rs` 中两个可复用状态类型以 Go 实现所使用的阶段化名称重新导出。

在完整执行链中，`builder.rs::build` 识别 `FunctionName::VarSamp`，再由 `build_variance` 按 `DISTINCT` 和聚合阶段选择 `AggImplementation::VarSamp`、`VarSampOriginalDistinct` 或 `VarSampPartialDistinct`。运行时状态分配当前直接在 `aggfuncs.rs` 中绑定到 `func_varpop::VarianceState` 或 `func_varpop::DistinctVariance`；因此本文件的别名主要承担公开类型命名、Go/Rust 语义对应以及独立测试入口，而不是运行时工厂的唯一接线点。

## 核心职责

- 以 `VarSamp4Float64` 表示普通 float64 样本方差的部分结果，并明确最终值应调用 `VarianceState::sample_variance`。
- 分别以 `VarSampOriginal4DistinctFloat64` 和 `VarSampPartial4DistinctFloat64` 表示 `DISTINCT` 原始输入阶段与部分结果阶段；两者目前都别名到同一个 `DistinctVariance`，类型层面没有阶段隔离。
- 保留兼容名称 `VarSampDistinctFloat64`，供不区分 original/partial 阶段的早期 Rust 调用方使用。
- 复用 `func_varpop.rs` 的在线更新、分区合并和去重算法，避免为 `VAR_SAMP` 复制一套状态维护；与 `VAR_POP` 的差异只发生在最终分母：样本方差使用 `n-1`。

## 主要符号

- `pub use crate::func_varpop::VarianceState as VarSamp4Float64`：普通 `VAR_SAMP` 的公开别名。真实状态字段为 `count: i64`、`sum: f64` 和 `variance: f64`，定义在 `func_varpop.rs::VarianceState`，字段只在 crate 内可见，外部通过 `update`、`merge`、`count`、`reset` 和 `sample_variance` 操作。
- `pub use crate::func_varpop::DistinctVariance as VarSampOriginal4DistinctFloat64`：Complete/Partial1 阶段的 `DISTINCT` 命名。`DistinctVariance` 以 `HashMap<u64, f64>` 保存去重后的值。
- `pub use crate::func_varpop::DistinctVariance as VarSampPartial4DistinctFloat64`：Final/Partial2 阶段的 `DISTINCT` 命名。当前与 original 类型完全相同，允许调用相同的 `update`、`merge` 和 `sample_variance`。
- `pub use crate::func_varpop::DistinctVariance as VarSampDistinctFloat64`：无阶段兼容别名。它未被本文件的独立测试使用，变更或移除前需要搜索仓库外部调用方。

本文件没有常量、自有结构体、trait、函数、`impl` 或条件编译项；四个符号都是公开再导出。

## 执行流程

1. `builder.rs::build` 收到 `FunctionName::VarSamp` 后调用 `build_variance(desc, VarianceKind::VarSamp)`。`AggMode::Dedup` 被拒绝；非 `DISTINCT` 统一选择 `AggImplementation::VarSamp`；`DISTINCT` 的 Complete/Partial1 选择 original 变体，Final/Partial2 选择 partial 变体。
2. `aggfuncs.rs` 为非 `DISTINCT` 变体分配 `func_varpop::VarianceState::default()`；为两个 `DISTINCT` 变体分配 `func_varpop::DistinctVariance::default()`。这一步直接使用底层类型而非本文件别名，但状态语义相同。
3. 普通路径调用 `VarianceState::update`：跳过 `None`，累加 `count` 和 `sum`，从第二个有效值起通过 `calculate_intermediate` 在线更新中间方差量。并行或分阶段聚合通过 `VarianceState::merge` 使用两组均值差补偿公式合并状态。
4. `DISTINCT` 路径调用 `DistinctVariance::update` 将非空浮点值按 Go map 兼容键语义去重；合并时将源集合的值重新灌入目标集合；`state()` 再把去重值转换为普通 `VarianceState`。
5. 最终求值调用 `sample_variance`。有效计数小于等于 1 时返回 `None`（对应 SQL `NULL`），否则返回中间方差除以 `count - 1`。本文件只通过别名暴露这一能力，不负责把 `Option<f64>` 写入输出 chunk。
6. spill 恢复后的部分结果在 `aggfuncs.rs::merge_spilled_partial_result` 中仍按底层 `VarianceState` 或 `DistinctVariance` 合并；普通状态内存增量为 0，去重状态按 `HashMap` 容量增长估算内存增量。

## 数据与状态

普通状态的不变量来自 `func_varpop.rs::VarianceState`：`count` 只统计非 `NULL` 输入；`sum` 是这些值的累计和；`variance` 是可合并的方差中间量，尚未除以最终分母。默认值和 `reset` 都恢复为三项全零。首个有效值不更新中间方差，因此一个值的状态可参与 merge，但 `sample_variance` 仍返回 `None`。

`DistinctVariance` 保存 `values: HashMap<u64, f64>` 与 `next_nan_payload`。普通数使用位模式作为键，`+0.0` 与 `-0.0` 统一为键 0；每次 NaN 输入分配不同的合成键，以匹配 Go 的浮点 map 行为。由于 `HashMap` 迭代顺序不稳定，去重值灌入 `VarianceState` 的累加顺序也不保证固定；测试使用能得到精确期望值的小数据集，不能据此推导任意浮点输入都具有位级确定性。

三个阶段化 `DISTINCT` 名称以及兼容名称都是同一 Rust 类型的别名，不携带额外标志、所有权或运行时标签。别名之间可互换，这与 Go 通过嵌入定义不同具体结构体的做法不同。

## 依赖与调用关系

- 模块依赖只有 `crate::func_varpop::{VarianceState, DistinctVariance}`；本文件不直接依赖 Cargo 外部 crate。
- crate 边界由 `pkg/executor/aggfuncs/Cargo.toml` 与 `lib.rs` 确认；计算状态自身只使用标准库 `HashMap`，更上层聚合运行时还依赖表达式、类型和序列化相关 workspace crate。
- 上游选择链为 `builder.rs::build` → `builder.rs::build_variance` → `AggImplementation`；状态实例化和 spill 合并位于 `aggfuncs.rs`。
- 下游计算链为别名 → `func_varpop.rs::VarianceState`/`DistinctVariance` → `calculate_intermediate`、`calculate_merge`、`sample_variance`。
- `func_varsamp_test.rs` 直接导入三个阶段化别名，覆盖普通更新、部分结果合并、`DISTINCT` 阶段合并和 `n-1` 分母。RustCodeGraph 对目标文件报告 `used by 0 files` 且只识别一个文件级符号，而精确查询能定位测试中的 `crate` 导入；这说明当前索引没有为 `pub use ... as ...` 建立完整别名调用边，不能把 `used by 0 files` 解读为模块未使用。

## 错误处理与边界

本文件没有 `Result`、显式错误分支或 I/O。SQL `NULL` 输入在底层 `update` 的 `flatten` 中被忽略；空集合和只有一个有效值的集合由 `sample_variance` 返回 `None`。`func_varsamp_test.rs::test_varsamp` 明确覆盖这两个最终结果为空的边界。

普通 merge 对空源不操作、空目标直接复制源状态；两边非空才执行合并公式。`DISTINCT` 合并通过重新插入源值完成去重。NaN 键计数超过 52 位 payload 掩码时，`DistinctVariance::update` 会触发断言 `too many NaN keys`；这是底层实现的极端资源边界，本别名层不捕获它。浮点溢出、无穷值和舍入误差也没有在本文件单独规范化。

`builder.rs::build_variance` 对 `AggMode::Dedup` 返回 `None`，这是构建阶段的不支持状态；不要将它误写成运行时聚合错误。目标文件也不验证输入类型，类型选择和表达式转换属于 builder/执行框架职责。

## 并发与资源生命周期

四个别名本身不创建线程、锁、通道、异步任务或外部资源。`VarianceState` 是 `Copy` 值状态；每个聚合组/部分结果应由执行框架独立持有，再通过显式 `merge` 汇合，文件没有共享可变全局状态。

`DistinctVariance` 拥有自己的 `HashMap`，克隆会复制集合；`reset` 清空集合并把 NaN payload 计数恢复为 1。它的内存随不同值数量增长，spill 合并路径会基于容量变化报告估算增量。安全扩展时应维持“更新目标状态、只读源状态”的 merge 方向，不应让多个工作线程无同步地共同修改同一状态。

资源结束不需要显式 close：状态离开作用域时由 Rust 自动释放。输出 chunk、表达式上下文以及 spill 文件的生命周期由 `AggFunc` 执行框架管理，不属于这个别名模块。

## 与 Go 版本的对应关系

Go 的 `func_varsamp.go` 定义三个具体结构体：`varSamp4Float64` 嵌入 `varPop4Float64`，两个 `DISTINCT` 结构体分别嵌入对应的 var-pop 阶段类型；各自覆盖 `AppendFinalResult2Chunk`，在计数不超过 1 时追加 `NULL`，否则追加 `variance / (count-1)`。Rust 将这部分压缩为对 `VarianceState`/`DistinctVariance` 的公开别名，并把最终公式放在共享的 `sample_variance` 方法中。

Go `builder.go::buildVarSamp` 与 Rust `builder.rs::build_variance` 的阶段选择一致：Complete/Partial1 使用 original distinct，Final/Partial2 使用 partial distinct，DedupMode 不构建实现，非 distinct 的其他模式使用普通样本方差。区别在于 Go 返回实现了 `AggFunc` 的不同对象，Rust builder 返回枚举变体，而运行时再绑定共享状态。

Go `func_varsamp_test.go` 的核心样例是输入 `0..5` 得到 `2.5`，并验证 partial merge；Rust `func_varsamp_test.rs` 保留这两项，还补充空输入、单个非空值夹杂 `NULL`、original/partial distinct 合并和重复值去重后除以 `n-1`。Rust 当前没有在本文件内复刻 Go 的 chunk 追加方法，因此“返回 `None` 等同追加 SQL NULL”仍依赖上层输出适配，不能仅凭别名文件声称完整 chunk 写出行为由此实现。

## 扩展指南

- 若只调整样本方差公式或数值稳定性，应修改真实实现 `func_varpop.rs::VarianceState::sample_variance`、在线/merge 公式及 `DistinctVariance` 转换，并同步独立测试 `func_varsamp_test.rs`；同时确认 `VAR_POP` 和两个 `STDDEV` 复用途径未被意外改变。
- 若新增聚合阶段或让 original/partial distinct 使用不同序列化布局，不能继续用同一 `DistinctVariance` 的简单别名；需要同时调整本文件公开类型、`builder.rs::AggImplementation`/`build_variance`、`aggfuncs.rs` 的分配与 spill merge，以及 Go 阶段语义对照。
- 若改变公开别名名称，先搜索 crate 外使用者并考虑保留 `VarSampDistinctFloat64` 兼容导出。类型别名不是新类型，不能用于编译期阻止阶段混用；需要阶段安全时应采用 newtype 并补齐 trait/序列化接线。
- 新测试必须继续放在独立的 `pkg/executor/aggfuncs/func_varsamp_test.rs`，不要内嵌到生产源文件。至少覆盖普通、merge、空/单值、`NULL`、`DISTINCT`、正负零、NaN 以及大数值精度风险；若触及 builder，再同步 `builder_test.rs` 的阶段选择断言。
- 兼容风险主要是 SQL `NULL` 边界、Go 浮点 map 的 `±0`/NaN 语义和阶段化 partial state；性能风险主要是 `DISTINCT` 集合增长与重新遍历构造 `VarianceState`，普通路径应继续保持单遍在线累计和常数状态大小。

## 验证依据

- RustCodeGraph：`status` 确认索引包含目标文件；`files --filter pkg/executor/aggfuncs/func_varsamp.rs` 确认文件及符号计数；`node --file ...func_varsamp.rs` 读取全部 18 行；精确查询 `VarSamp4Float64`、`DistinctVariance`、`VarianceState`、`sample_variance`；`explore 'VarSamp4Float64 VarSampDistinctFloat64 builder.rs func_varpop.rs func_varsamp_test.rs'` 核对更新、merge、最终求值与测试关系。宽泛查询会混入仓库其他同名 `update/merge/result`，本文未采用那些跨模块结果作为证据。
- Rust 源与入口：`pkg/executor/aggfuncs/func_varsamp.rs`、`func_varpop.rs`、`builder.rs`、`aggfuncs.rs`、`lib.rs`；目标包没有 `pkg/executor/doc.go`。
- crate 声明：`pkg/executor/aggfuncs/Cargo.toml`，确认包名、`lib.rs` 入口、workspace 内依赖与 porting 元数据。
- 独立 Rust 测试：`pkg/executor/aggfuncs/func_varsamp_test.rs`；相关 spill 状态证据还由 RustCodeGraph 定位到 `spill_helper_test.rs`。
- Go 对照：`pkg/executor/aggfuncs/func_varsamp.go`、`func_varsamp_test.go`、`builder.go::buildVarSamp`、`aggfuncs.go` 的 `AggFunc` 编译期实现清单。
- 本任务是纯文档分析，没有运行 Cargo 或代码测试；结构验收使用任务文件指定命令，要求本文恰好包含上述 11 个固定二级标题。
