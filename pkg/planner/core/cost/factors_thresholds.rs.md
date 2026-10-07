# `pkg/planner/core/cost/factors_thresholds.rs`

## 文件定位

本文件是 `astersql-planner-core-cost` crate 的参数定义文件，由同目录的 [`lib.rs`](lib.rs) 公开为 `factors_thresholds` 模块。crate 自身只依赖 `astersql-parser-ast`，并在 `lib.rs` 中把其聚合函数名模块再导出为 `ast`，因此这里能够用与 Go 版相同的函数名作为成本表键。它位于规划器的“共享估算参数”边界：不执行计划枚举或成本计算，只给基数估算、逻辑算子和实现选择代码提供统一常量。

当前 Rust 接线尚未覆盖 Go 版的全部用途。精确引用搜索显示，生产代码直接读取了 `SelectionFactor` 和 `ToleranceFactor`；`DistinctFactor`、`AggFuncFactor`、`SmallScanThreshold` 目前只有独立迁移测试直接读取，其中流式聚合仍以字面量 `0.8` 注明对应 Go `cost.DistinctFactor`。因此不能把 Go 版完整成本模型视为已经在 Rust 中接通。

## 核心职责

- 用 `SelectionFactor = 0.8` 表示无法精确估算过滤或连接条件时的默认选择率。
- 用 `DistinctFactor = 0.8` 表示 distinct 基数或状态规模的默认折减比例；当前 Rust 生产代码尚未直接使用此符号。
- 用 `ToleranceFactor = 0.00001` 为行数估算比较提供浮点容差，避免微小舍入误差改变边界分支。
- 用惰性只读表 `AggFuncFactor` 保存聚合函数的相对基础成本，并保留 `"default" = 1.5` 作为调用方可显式选择的未知函数回退值。
- 用 `SmallScanThreshold = 10000` 表示带 LIMIT 的小扫描边界；当前 Rust 生产代码尚未直接使用此符号。

这些数值是规划启发式而非用户配置，也不是统计数据本身；修改它们会系统性影响估算或计划选择，应与 Go 对照和独立测试一起审查。

## 主要符号

- `pub const SelectionFactor: f64`：默认选择率。Rust 逻辑选择节点的 `LogicalSelection::DeriveStats`、半连接 NDV 缩放以及 `ApplyImpl::cost_limit` 会读取它。
- `pub const DistinctFactor: f64`：distinct 默认折减系数。定义和测试均存在，但当前 Rust 生产引用搜索没有命中。
- `pub const ToleranceFactor: f64`：比较容差。逻辑数据源在校正 appended-handle 路径行数时读取；cardinality crate 通过其 `cost` 再导出命名空间，在列/索引区间估算中读取。
- `pub static AggFuncFactor: LazyLock<HashMap<&'static str, f64>>`：首次解引用时建立的聚合权重表。键来自 `crate::ast` 的聚合函数名常量，另含字符串键 `"default"`；值覆盖普通聚合（多为 `1.0`）、`first_row`（`0.1`）、位聚合（`0.9`）、平均值（`2.0`）以及方差/标准差（`3.0`）。
- `pub const SmallScanThreshold: i32`：带 LIMIT 时是否仍属小扫描的阈值。定义和测试均存在，但当前 Rust 生产引用搜索没有命中。
- `#![allow(non_upper_case_globals)]`：允许保留 Go 导出符号的 CamelCase 名称，以降低双版本核对成本。

文件没有 trait、结构体、函数、`impl` 或条件编译项；五个数据符号全部是公开 API。

## 执行流程

1. crate 使用者通过 `astersql_planner_core_cost::factors_thresholds`（或下游 crate 的别名/再导出）解析这些公开符号。
2. 四个标量 `const` 在编译期直接内联到调用表达式。例如 `LogicalSelection::DeriveStats` 将子节点统计按 `SelectionFactor` 缩放，并清空 `GroupNDVs`；`LogicalJoin::DeriveStats` 对 semi join 保留列的 NDV 再乘同一系数。
3. `ToleranceFactor` 参与严格不等式的边界修正。例如逻辑数据源只有在 `CountAfterAccess + ToleranceFactor < RowCount` 时才把 appended-handle 路径校正到统计行数；列和索引的区间估算用 `1.0 - ToleranceFactor` 判断结果是否已近似覆盖全集。
4. 首次访问 `AggFuncFactor` 时，`LazyLock` 执行闭包并用 `HashMap::from` 建表；之后所有线程共享同一张不可变表。查找具体聚合名是否回退到 `"default"` 由调用方决定，表本身的 `get` 对未知键返回 `None`。
5. `DistinctFactor` 和 `SmallScanThreshold` 当前只在迁移单元测试中验证数值，尚未进入 Rust 生产执行流；Go 的对应消费点只能作为待迁移语义证据，不能作为 Rust 已执行行为。

## 数据与状态

四个标量常量没有运行时可变状态。`AggFuncFactor` 是唯一需要初始化的状态：键是生命周期为 `'static` 的字符串切片，值是 `f64`，初始化完成后本文件不暴露可变访问。表共有 18 个条目：17 个明确聚合名和一个 `"default"`；`max_count`、`min_count` 均为 `1.0`，由测试单独覆盖。

核心不变量包括：选择率与 distinct 系数均为 `0.8`；容差为正且远小于正常行数尺度；小扫描阈值为正整数；聚合函数名必须与 `parser-ast` 的常量值一致；默认键必须存在。由于数值使用 `f64`，调用者仍须明确比较方向，不能把容差误作通用近似相等函数。

## 依赖与调用关系

下游依赖只有标准库的 `HashMap`、`LazyLock` 和本 crate 从 `astersql-parser-ast` 再导出的 `ast`。[`Cargo.toml`](Cargo.toml) 将该 crate 声明为 `astersql-planner-core-cost`，`[lib]` 指向 `lib.rs`，并以本地路径依赖 `../../../parser/ast`；没有 feature 条件。

已核实的 Rust 生产读取关系如下：

- `logical_selection.rs::LogicalSelection::DeriveStats` → `SelectionFactor`，用于无精确选择率时缩放子统计。
- `logical_join.rs::LogicalJoin::DeriveStats` → `SelectionFactor`，用于 semi/anti-semi 类估算中的 NDV 折减（其中输出行数附近仍有一个字面量 `0.8`）。
- `logical_datasource.rs::LogicalDataSource::DeriveStats` → `ToleranceFactor`，用于 appended-handle 路径的行数校正门槛；其独立测试也构造了半个容差的边界值。
- `implementation/simple_plans.rs::ApplyImpl::cost_limit` → `SelectionFactor`，左侧存在过滤条件时缩放有效左行数，再计算右子计划成本上限。
- `cardinality/row_count_column.rs` 与 `row_count_index.rs` 经 `cardinality::cost` 再导出 → `ToleranceFactor`，用于抑制已近似全范围时的重复 out-of-range 补偿。

RustCodeGraph 的文件节点还报告逻辑选择、连接、数据源及其测试为使用者；精确工作区搜索补出了 implementation 与 cardinality 的当前引用。`AggFuncFactor`、`DistinctFactor`、`SmallScanThreshold` 未发现 Rust 生产读取者，不能据 Go 调用点虚构 Rust 调用边。

## 错误处理与边界

本文件不返回 `Result`、不触发显式错误，也不做输入校验。主要边界由使用方式形成：

- `AggFuncFactor.get(unknown)` 返回 `None`，不会自动读取 `"default"`；调用方必须显式执行回退。迁移测试正面断言了未知键行为。
- `LazyLock` 初始化闭包没有外部输入和可恢复错误；正常情况下只分配固定大小的映射。若初始化期间发生 panic，标准库的惰性初始化语义决定后续行为，本文件没有额外恢复层。
- `SelectionFactor` 和 `DistinctFactor` 不是概率类型，Rust 类型系统不会阻止写入负数或大于一的未来改动；兼容性依赖代码审查和测试。
- `ToleranceFactor` 只适用于源码中明确写出的比较式。改变其数量级可能同时影响访问路径校正和 out-of-range 补偿边界。
- `SmallScanThreshold` 是 `i32`，而具体成本计算常使用浮点行数；未来接线时应在调用点明确且安全地转换类型。

## 并发与资源生命周期

标量常量没有生命周期管理或同步成本。`AggFuncFactor` 由 `std::sync::LazyLock` 保证并发首次访问只完成一次初始化；初始化后的 `HashMap` 通过共享静态引用读取，没有锁住整个查表过程，也没有清理阶段，生命周期覆盖进程全程。

本文件不开线程、不持有锁守卫、不创建任务、通道、事务或 I/O 资源。固定表的内存只分配一次，条目数量很小；规划热路径的标量读取可视为常量访问，表查询则具有普通 `HashMap` 查找成本。

## 与 Go 版本的对应关系

直接对照文件是 [`factors_thresholds.go`](factors_thresholds.go)。五个公开名称、四个标量值及聚合权重条目与 Go 定义对齐；Rust 额外用 `LazyLock<HashMap<&'static str, f64>>` 代替 Go 的包级 `map[string]float64`，并保留 Go 风格名称。

语义差异主要在接线完整度而非定义值：Go 的 `plan_cost_ver1.go` 使用 `SmallScanThreshold`、`DistinctFactor`、`SelectionFactor`，物理聚合用 `AggFuncFactor`，多个规划入口也使用这些参数；当前 Rust 只有部分 `SelectionFactor`/`ToleranceFactor` 调用边。`physical_stream_agg.rs` 仍写死 `0.8 // Go cost.DistinctFactor`，而 `AggFuncFactor` 与 `SmallScanThreshold` 没有生产读取者。因此本文件是对齐后的参数来源，但不是 Go 成本消费链的完整移植证明。

Go 的 map 可在包内被修改，而 Rust 静态值只通过不可变共享引用暴露；这收紧了运行时可变性。两版对未知聚合键的底层查找都会产生“未命中”，但默认权重是否应用仍属于调用方逻辑。

## 扩展指南

- 调整现有系数或阈值时，先定位所有 Rust 精确引用，再核对 Go 同名符号的消费点；至少同步 `migration_aster_unit_test.rs` 中的数值断言，并为受影响的规划分支更新其同目录独立测试。
- 新增聚合函数权重时，使用 `ast` 中的正式函数名常量，不要另写可能漂移的字符串；同时更新 `aggregation_factors_match_go_entries` 的期望数组和条目总数。当前该数组漏列已经存在的 `max_count`/`min_count`，导致其 16 项长度断言与实际 18 项表不一致；扩展前应先在独立测试文件修正这一既有缺口，并保留随后针对这两个特殊条目的断言。
- 将 `AggFuncFactor` 接入物理聚合时，应明确“具体键 → `"default"`”的回退流程，并测试未知聚合名；不能依赖 `HashMap::get` 自动回退。
- 将 `DistinctFactor` 或 `SmallScanThreshold` 接入生产路径时，应从对应 Go 成本函数逐分支迁移，并在物理算子或成本模型的独立 `*_test.rs` 中覆盖阈值两侧、等于阈值和 LIMIT/无 LIMIT 情况。不要为方便而把测试嵌入本源文件。
- 变更 `ToleranceFactor` 时，应同步验证 `logical_datasource_test.rs` 的容差边界，并为 cardinality 的全范围/out-of-range 分支选择现有独立测试文件扩展用例；风险是微小数值变化导致计划路径翻转。
- 若需要可配置因子，应新建显式配置/会话变量读取层，而不是把本静态表改成可变全局；还需评估跨线程一致性和计划缓存兼容性。

## 验证依据

- RustCodeGraph：`status` 确认本仓库索引包含 11,467 个文件；`node --file pkg/planner/core/cost/factors_thresholds.rs --offset 1 --limit 240` 读取全部 69 行并报告直接使用文件；`query` 分别确认五个公开符号及其 Go/测试对应项。符号级 `callers/callees` 查询超时且没有返回可用边，故调用关系以精确源码引用搜索补足。
- 源码与装配：[`factors_thresholds.rs`](factors_thresholds.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。
- Go 对照：[`factors_thresholds.go`](factors_thresholds.go)，并通过 `rg` 核对 `plan_cost_ver1.go`、物理聚合和其他规划调用点。
- 独立 Rust 测试：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 覆盖四个标量、常见聚合权重、未知键返回 `None`，并单独覆盖 `max_count`/`min_count`。但 `aggregation_factors_match_go_entries` 的 `expected` 只有 16 项、实际表有 18 项，其长度断言按当前源码不成立；本任务遵守纯文档范围，未修改或运行该测试。相关消费边界还见 `pkg/planner/core/operator/logicalop/logical_datasource_test.rs`。
- Rust 生产调用证据：`logical_selection.rs`、`logical_join.rs`、`logical_datasource.rs`、`implementation/simple_plans.rs`、`cardinality/row_count_column.rs`、`cardinality/row_count_index.rs`；`physical_stream_agg.rs` 证明 `DistinctFactor` 仍是字面量替代状态。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构命令，并人工复核公开符号、数值、调用边、未接线状态、错误边界及独立测试建议。
