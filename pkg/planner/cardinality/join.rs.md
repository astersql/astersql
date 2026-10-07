# `pkg/planner/cardinality/join.rs`

## 文件定位

[`join.rs`](./join.rs) 属于 `astersql-planner-cardinality` crate，实现 Join 等值条件的输出行数估算。crate 根 [`lib.rs`](./lib.rs) 以 `mod join` 纳入该模块，再用 `pub use join::*` 对外导出，因此调用者通过 `cardinality::EstimateFullJoinRowCount` 访问，而不需要知道私有的 `join` 模块。

`pkg/planner/cardinality/Cargo.toml` 声明该 crate 的库入口为 `lib.rs`，并将 Go 对照包记录为 `pkg/planner/cardinality`。本文件不是独立的完整 Join 基数推导器：它只计算等值连接对应的“full join row count”中间值；外连接下界、半连接系数、伪统计回退和结果 `StatsInfo` 组装由上层逻辑算子完成（`logical_join.rs::LogicalJoin::DeriveStats`、`logical_apply.rs::LogicalApply::DeriveStats`）。

## 核心职责

- 在笛卡尔积情形下，返回左右输入行数之积。
- 在有连接键时，分别估算两侧键组 NDV（Number of Distinct Values），用较大的一侧作为分母，计算 `left rows * right rows / max(left NDV, right NDV)`。
- 在普通连接键两侧都为空时，切换到 Null-Aware Join 键；只要任意一侧普通键非空，就使用普通键分支。
- 当 `TiDBOptJoinReorderThreshold > 0` 表示启用 DP join reorder 选择时，为未被 GroupNDV 覆盖的剩余键乘以每键 `0.9` 的相关性因子。

## 主要符号

### `pub fn EstimateFullJoinRowCount(...) -> f64`

文件中唯一的函数和唯一的公开 API。参数语义如下：

- `sctx: &dyn planctx::PlanContext`：基数估算需要的 object-safe 规划上下文。此处 `planctx::PlanContext` 是 `lib.rs` 对 `CardinalityContext` 的别名，本函数直接使用其 `GetSessionVars()`，并将它传给 NDV 估算。
- `isCartesian`：是否为笛卡尔积。为 `true` 时立即返回，不读取键、schema 或会话变量。
- `leftProfile` / `rightProfile`：两侧 `property::StatsInfo`，提供 `RowCount`、单列 NDV 和 GroupNDV。
- `leftJoinKeys` / `rightJoinKeys`：普通等值连接键，也用于计算 DP 分支的剩余键数。
- `leftSchema` / `rightSchema`：把键列匹配到各自统计剖面的 schema。
- `leftNAJoinKeys` / `rightNAJoinKeys`：可选的 Null-Aware Join 键切片；`None` 通过 `unwrap_or_default()` 按空切片处理。

本文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项。`use crate::*` 引入 crate 根重导出的 `expression`、`planctx`、`property` 和相邻 `ndv.rs` 的 `EstimateColsNDVWithMatchedLen`。

## 执行流程

1. 检查 `isCartesian`。若为真，返回 `leftProfile.RowCount * rightProfile.RowCount`。
2. 选择键集。当 `leftJoinKeys` 或 `rightJoinKeys` 任一非空时，两侧都使用普通键；否则两侧都使用各自的 NA 键。
3. 对两侧分别调用 `EstimateColsNDVWithMatchedLen(Some(sctx), keys, schema, profile)`，得到 `(NDV, matched column count)`。
4. 计算基础结果 `count = left rows * right rows / max(left NDV, right NDV)`。`ndv.rs` 将空键、无法匹配的键和 GroupNDV 都回退或钳制到至少 `1.0`，从而为这个除法提供非零分母。
5. 若 `TiDBOptJoinReorderThreshold <= 0`，直接返回 `count`。
6. 否则计算 `remained = leftJoinKeys.len() - max(leftColCnt, rightColCnt)` 的有符号形式，返回 `count * 0.9^remained`。这里以左侧普通键数作为键组长度，与 Go 实现一致。

Rust 直接上游是 `LogicalJoin::DeriveStats` 和 `LogicalApply::DeriveStats`。前者先推导子节点统计并修复非正 NDV，再调用本函数，随后按 Join 类型增加外连接下界或半连接衰减。后者仅在 lateral inner/left outer apply 且有明确连接键时调用，避免对已含相关谓词选择率的 inner 统计再次衰减。

## 数据与状态

计算主体是纯局部数值变换：`leftKeyNDV`、`rightKeyNDV`、两侧命中列数、`count` 和 `remained` 都是栈上局部值，本文件不持有可变状态、缓存或全局对象。

可观测的间接状态来自会话上下文。`EstimateColsNDVWithMatchedLen` 读取 `RiskGroupNDVSkewRatio`，并可通过 `SessionVars::RecordRelevantOptVar` 记录相关优化变量；本函数自身还读取 `TiDBOptJoinReorderThreshold`。输入的 profile、键和 schema 都是共享借用，不会被修改。

实现假定左右普通键在语义上成对，但不在本函数内检查两侧数量相等。DP 分支也只用 `leftJoinKeys.len()` 计算剩余数；这是调用者必须维持的输入不变量。

## 依赖与调用关系

**上游调用者**

- `pkg/planner/core/operator/logicalop/logical_join.rs::LogicalJoin::DeriveStats`：非笛卡尔积普通 Join 的主要调用点，将结果写入 `EqualCondOutCnt`，并据 Join 类型得出最终 `RowCount`。
- `pkg/planner/core/operator/logicalop/logical_apply.rs::LogicalApply::DeriveStats`：lateral inner/left outer apply 有连接键时的调用点，同样写入内部 `LogicalJoin.EqualCondOutCnt`。

RustCodeGraph 的文件节点也将上述两个文件列为 `join.rs` 的使用者。Go 版另有 `pkg/planner/core/plan_cost_ver1.go` 的两个成本估算调用点；当前 Rust 搜索未发现对应成本路径直接调用本函数，不应把 Go 调用面当成 Rust 已接线事实。

**下游依赖**

- `pkg/planner/cardinality/ndv.rs::EstimateColsNDVWithMatchedLen`：提供键组 NDV 和 GroupNDV 命中列数。
- `astersql-expression`：通过 crate 根 `expression` 命名空间提供 `Column` 和 `Schema`。
- `astersql-planner-property`：通过 `property` 提供 `StatsInfo`。
- `astersql-planner-planctx` 与 `astersql-sessionctx-variable`：由 crate 根的 `CardinalityContext` 适配并暴露会话变量。

`Cargo.toml` 中上述都是 workspace 内 path dependency，本文件没有自己的 feature gate。

## 错误处理与边界

函数返回类型是 `f64` 而非 `Result`，没有显式错误通道。边界处理主要由下游 NDV 函数的安全回退提供：空键返回 `(1.0, 1)`，无法在 schema 中匹配列时保守返回 `1.0`，GroupNDV 也至少为 `1.0`。NA 键的 `None` 被视为空键，不会 panic。

本函数不主动钳制行数为非负数，也不检查 `NaN`/无穷大；正常统计输入应由上游保证有效。`LogicalJoin::DeriveStats` 在收到返回值后用 `.max(0.0)` 做了非负保护，但 `LogicalApply::DeriveStats` 没有同样的通用钳制。

DP 相关性分支的 `remained` 使用 `i32` 差值后转为 `f64`。若上游违反键组与 matched length 的不变量，负指数会放大而非衰减 `count`；当前实现不会报错。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或 I/O 资源。所有输入都在函数调用期间以不可变引用或切片存活，返回值是独立 `f64`，因此本函数不延长任何输入生命周期。

并发安全性取决于借入的 `PlanContext`/`SessionVars` 实现；本文件既不加锁，也不缓存上下文。统计估算处于计划推导的同步调用链中。

## 与 Go 版本的对应关系

Rust `EstimateFullJoinRowCount` 逐分支对应 `pkg/planner/cardinality/join.go::EstimateFullJoinRowCount`：笛卡尔积直接相乘、普通键优先于 NA 键、两侧 NDV 取大值作分母、非 DP 分支直接返回，以及 DP 分支使用 `0.9` 幂，算术语义一致。

类型形状的差异是：Go 使用 profile/schema 指针和可为 `nil` 的键 slice；Rust 使用不可变引用，普通键始终是 slice，只将 NA 键表示为 `Option<&[Column]>`。Go 的 `math.Pow(0.9, ...)` 对应 Rust 的 `0.9_f64.powf(...)`。Rust 用 `CardinalityContext` 窄 trait 代替带关联类型、不能直接作 trait object 的完整 planner context。

调用面并非完全相同。Go 的 `LogicalJoin::DeriveStats` 直接把 `len(EqualConditions) == 0` 作为 `isCartesian` 传入；Rust 调用者在笛卡尔积分支直接相乘，只在非笛卡尔积时以 `false` 调用本函数。Go 还在 `plan_cost_ver1.go` 中为 index/hash join 成本估算调用它；本次 Rust 直接引用搜索只找到 logical join/apply。

## 扩展指南

- 修改 Join 基数公式时，首先修改 `EstimateFullJoinRowCount`，并逐项与 `pkg/planner/cardinality/join.go` 核对；不要在本文件复制 `ndv.rs` 的 GroupNDV/指数退避逻辑。
- 增加键组规则时，同时检查 `EstimateColsNDVWithMatchedLen` 返回的 matched length 语义和 DP `remained` 公式，并保证分母不为零。
- 增加新会话开关时，需要在 `CardinalityContext` 的 object-safe 边界上暴露所需能力，同时考虑 relevant optimizer variable 记录。
- 调整返回值对各 Join 类型的影响时，必须同步复核 `logical_join.rs::DeriveStats` 的外连接下界和半连接分支，以及 `logical_apply.rs::DeriveStats` 避免重复应用相关选择率的契约。
- 测试不应内嵌在 `join.rs`。当前 crate 根没有声明 `join_test.rs` 模块；新增直接单元测试时，应建立同目录独立 `join_test.rs`，并在 `lib.rs` 以 `#[cfg(test)]`/`#[path = "join_test.rs"]` 接入。至少覆盖笛卡尔积、普通键、NA 键、GroupNDV、DP 开关和多键剩余因子。
- 若改动调用者，同步扩展 `pkg/planner/core/operator/logicalop/logicalop_test/plan_execute_test.rs` 中的统计推导回归；对 NDV 边界则扩展独立的 `pkg/planner/cardinality/ndv_test.rs`。

主要兼容性风险是与 Go 公式或会话开关语义漂移；正确性风险是空键、不对称键和 matched length 不变量被破坏；性能风险主要来自更复杂的 NDV 估算或在规划热路径上增加分配。

## 验证依据

- RustCodeGraph `status`：索引有效，项目包含 7,032 个 Rust 文件。
- RustCodeGraph `files --filter pkg/planner/cardinality`：确认 `join.rs`、`join.go`、`ndv.rs`、独立测试和 crate 根的实际文件集。
- RustCodeGraph `node --file pkg/planner/cardinality/join.rs --offset 1 --limit 260`：读取全部 78 行，并报告两个使用文件 `logical_apply.rs` 和 `logical_join.rs`。
- RustCodeGraph `query EstimateFullJoinRowCount --kind function --limit 20 --json`：定位 Rust 与 Go 两个对应符号。精确 `callers`/`callees` 查询在 30 秒内未返回，因此调用边另用文件节点的 `used by` 结果和直接引用搜索核对。
- 源码与配置：`pkg/planner/cardinality/join.rs`、`ndv.rs`、`lib.rs`、`Cargo.toml`、`pkg/planner/core/operator/logicalop/logical_join.rs`、`logical_apply.rs`。目标路径下没有 `doc.go`，因此无额外包契约文件可读。
- Go 对照：`pkg/planner/cardinality/join.go`、`pkg/planner/core/operator/logicalop/logical_join.go`、`logical_apply.go`、`pkg/planner/core/plan_cost_ver1.go`。
- 测试证据：`pkg/planner/cardinality/ndv_test.rs` 直接覆盖下游 NDV 的单列、GroupNDV、多列指数退避和空键；`pkg/planner/core/operator/logicalop/logicalop_test/plan_execute_test.rs` 通过 `LogicalJoin::DeriveStats` 验证空 CTE Join 的 `RowCount` 与 `EqualCondOutCnt` 为零。未找到直接调用 `EstimateFullJoinRowCount` 的 Rust 或 Go 测试，因此 DP/NA 分支的直接回归覆盖仍是未验证项。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认文档存在且恰有 11 个固定二级标题，并人工复核唯一生产物、链接路径与事实限定。
