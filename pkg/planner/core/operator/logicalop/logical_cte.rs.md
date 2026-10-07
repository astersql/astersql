# `pkg/planner/core/operator/logicalop/logical_cte.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-logicalop` crate，是 SQL `WITH` 公用表表达式在逻辑计划层的消费者算子及共享定义状态。crate 入口 `pkg/planner/core/operator/logicalop/lib.rs` 通过 `mod logical_cte` 装载并 `pub use logical_cte::*` 导出其类型；`Cargo.toml` 的 `package.metadata.porting.go-package` 指向同目录 Go 包，表明直接对照文件是 `logical_cte.go`。

规划器构建表源时，`pkg/planner/core/logical_plan_builder_runtime.rs` 的 CTE 分支会在非内联、非递归引用处构造 `LogicalCTE`；递归引用则构造相邻文件中的 `LogicalCTETable`。因此本文件位于“AST/CTE 绑定已经建立”之后、“逻辑规则优化和物理计划选择”之前，不负责解析 `WITH` 语法，也不负责执行 CTE 工作表。

## 核心职责

- 用 `CTEClassRef = Rc<RefCell<CTEClass>>` 让同一个 CTE 的多个引用共享种子/递归计划、优化标志、Limit 元数据、列映射和谓词缓冲。
- 用 `LogicalCTE` 表示一次非递归引用，保存该引用的名称、输出 schema 基类、共享统计句柄以及共享 CTE 定义。
- 在满足“非递归且是最外层引用”时收集可下推谓词，并将消费者列替换为种子列；原谓词仍返回给调用方，因而这里是跨引用信息收集而不是删除消费者侧过滤。
- 在统计推导时将各引用收集的谓词合并到种子计划、通过回调运行逻辑优化、把种子/递归计划的统计映射到消费者 schema，并更新共享的 `SeedStat`。
- 提供 CTE 的 TopN 边界、保守列裁剪、TiFlash 可用性提示和关联列收集，并通过 `LogicalPlan` trait 接入通用逻辑优化流水线。

## 主要符号

- `OptimizeCTESeed = fn(u64, &mut LogicalPlanRef) -> Result<()>`：解耦 logicalop crate 与上层核心规则流水线的函数指针边界。
- `OPTIMIZE_CTE_SEED: OnceLock<OptimizeCTESeed>` / `InstallOptimizeCTESeed`：全进程只能首次成功安装种子优化器。实际安装点是 `pkg/planner/core/optimizer_runtime.rs::logical_optimize_in_place`，回调转入 `LogicalOptimizeForMpp`。
- `CTECardinalityContext`：把 `base::PlanContext` 的会话变量、表达式上下文和 ranger 上下文适配成 `cardinality::CardinalityContext`，供递归 DISTINCT 的基数估算使用。
- `CTEClass`：共享定义。`SeedPartLogicalPlan`/`RecursivePartLogicalPlan` 保存两部分逻辑子树；`OptFlag` 和 `SeedPartLogicalOptimized` 控制种子优化；`PushDownPredicates` 与 `ColumnMap` 支撑跨引用谓词下推；`HasLimit`、`LimitBeg`、`LimitEnd`、`IDForStorage` 等字段由 CTE 构建/执行链消费，本文件只保存它们。
- `CTEClass::MemoryUsage`：返回结构体静态大小，加上缓冲表达式和映射值 `Column` 的估算；不会递归统计逻辑计划，也未额外计算 `HashMap` 桶和整数键容量，因而是近似值。
- `LogicalCTE`：逻辑算子本体。`LogicalSchemaProducer` 提供基类/schema；`Cte` 指向共享定义；`SeedStat: Arc<RwLock<StatsInfo>>` 与 CTE 表引用共享种子统计。
- `resolve_cte_expression`：递归处理 `Column`、`CorrelatedColumn` 和 `ScalarFunction`。按 `UniqueID` 替换列，克隆标量函数参数并清理函数哈希缓存；其他表达式种类原样返回。
- `Init`、`PredicatePushDown`、`PruneColumns`、`PushDownTopN`、`DeriveStats`、`PreparePossibleProperties`、`ExtractCorrelatedCols`：算子的主要生命周期方法。文件末尾的 `impl LogicalPlan` 将通用 trait 调用转发到前四个优化方法。

## 执行流程

1. `pkg/planner/core/logical_plan_builder_runtime.rs` 的 CTE 表源分支从 `CteEnvironment` 取得 `CteBinding`，为本次引用生成新的可见 schema，并把“可见列 `UniqueID` -> 种子列”写入共享 `CTEClass::ColumnMap`。
2. 对非内联且非递归引用，构建器创建 `LogicalCTE { Cte, CteAsName, CteName, SeedStat, .. }`，调用 `Init` 写入类型为 `CTE` 的 `BaseLogicalPlan`，随后设置输出 schema 和列名。
3. 通用谓词下推调用 `PredicatePushDown`。递归 CTE或非最外层引用不收集；Apply 外的引用还会排除含关联列的谓词。无可收集谓词时写入恒真表达式，使该引用仍参与稍后的跨引用 DNF 汇总；否则先通过 `ColumnMap` 将消费者列改写为种子列，再把 CNF 表达式追加到共享缓冲。方法始终返回原始 `predicates`。
4. `DeriveStats` 若允许复用且节点已有统计，直接返回缓存并标记未重算。否则暂时从共享状态取出种子计划，避免持有 `RefCell` 借用跨越对子计划的可变调用。
5. 若存在收集谓词，将各引用条件组合为 DNF，再用 `ExtractFiltersFromDNFs` 提取公共过滤条件；有公共条件时创建 `LogicalSelection` 包裹种子计划，保留原种子统计作为该 Selection 的初始统计，并在 `OptFlag` 中设置谓词下推标志。
6. 若种子尚未优化且回调已安装，调用 `OPTIMIZE_CTE_SEED`。成功后置 `SeedPartLogicalOptimized = true`；若未安装回调，当前代码继续用现有计划推导统计，不报错。
7. 从种子计划取得或推导统计，把行数及按 schema 位置对应的 NDV 写到消费者可见列 `UniqueID`，然后把种子计划放回共享定义。
8. 若有递归计划，推导其统计并按列位置累加 NDV。`IsDistinct` 为真时以合并后的 NDV 估算行数；否则把递归行数直接加到种子行数。
9. 将种子统计写入共享 `SeedStat`，将合并统计缓存到当前节点并返回。之后 `PreparePossibleProperties` 从种子计划缓存的属性值给出 `HasTiFlash`，`ExtractCorrelatedCols` 则合并种子与递归子树的关联列。

## 数据与状态

`CTEClassRef` 使用 `Rc<RefCell<_>>`，说明共享仅限单线程规划上下文；多个 `LogicalCTE` 引用观察同一 `PushDownPredicates`、`ColumnMap` 和种子计划。`PredicatePushDown` 追加缓冲，`DeriveStats` 用 `mem::take` 一次性消费缓冲，因此调用顺序会影响种子首次优化时看到的条件。`SeedPartLogicalOptimized` 防止同一共享种子重复经过回调。

`ColumnMap` 以消费者列的 `UniqueID` 为键。构建器在每次引用生成 schema 后追加映射，因此表达式改写不能改用列的显示名或位置。统计重映射则依赖 `visible_schema.Columns` 与种子/递归 schema 的相同列序；`zip` 会在较短一侧结束，文件本身不检查长度一致性。

`SeedStat` 使用 `Arc<RwLock<_>>`，以便 `LogicalCTE` 和 `LogicalCTETable` 共享统计。它保存的是种子统计，而当前 `LogicalCTE` 的 `StatsInfo` 是种子与可选递归部分合并后的消费者统计。`CteAsName`、`CteName` 和 `OnlyUsedAsStorage` 在本文件中仅存储；尤其 Rust `DeriveStats` 没有依据 `OnlyUsedAsStorage` 改写 children。

## 依赖与调用关系

上游直接证据包括：

- `pkg/planner/core/logical_plan_builder_runtime.rs::CteBinding` 持有并克隆 `CTEClassRef`、schema、统计和存储 ID；同文件的 CTE 表源分支写 `ColumnMap` 并创建 `LogicalCTE`/`LogicalCTETable`。
- `pkg/planner/core/optimizer_runtime.rs::logical_optimize_in_place` 安装 `OptimizeCTESeed`，并在规则流水线中通过 `LogicalPlan` trait 调用列裁剪等操作。
- `pkg/planner/core/operator/logicalop/lib.rs` 对外再导出本文件符号，并以独立 `logical_cte_test.rs` 注册单元测试。

主要下游依赖由该 crate 的 `Cargo.toml` 声明：`base` 提供计划 trait/context，`expression` 提供表达式组合、关联列提取和 schema，`property` 提供 `StatsInfo`，`cardinality` 估算递归 DISTINCT 行数，`rule_util` 设置谓词下推 flag，`planctx` 提供统计估算所需上下文，`parser_ast` 提供大小写不敏感名称。`std` 的 `Rc/RefCell`、`Arc/RwLock` 和 `OnceLock` 分别承担单线程共享可变定义、跨对象共享统计及全局一次性回调安装。

RustCodeGraph 对目标文件报告 22 个使用文件，并确认 `InstallOptimizeCTESeed` 的安装点以及 `LogicalCTE` 的构建器入口。由于 `PredicatePushDown`、`DeriveStats` 等是 trait 方法，同名符号很多，调用关系的一部分属于动态分发边界；不能只把静态同名调用计数当成完整运行时调用集。

## 错误处理与边界

- `PredicatePushDown` 的正常路径没有业务错误，但在要求上下文的最外层下推中使用 `expect`；若未 `Init` 且确实组合出条件，会 panic。共享 `RefCell` 若发生重叠可变借用也会在运行时 panic。
- `DeriveStats` 用 `?` 传播 Selection/种子/递归子计划统计错误；优化回调错误被转成 `PlannerError(error.to_string())`，会丢失具体错误类型但保留文本。
- `RwLock` 读写遭遇 poison 时用 `PoisonError::into_inner` 继续访问，而不是把锁中毒作为规划错误返回。
- 未安装 `OPTIMIZE_CTE_SEED` 不是错误，代码会对未经过核心规则流水线的现有种子计划继续取/推导统计；调用方若需要与完整规划器一致的结果，必须保证先经过安装入口。
- 表达式列映射缺项时保留原表达式；不属于 `Column`、`CorrelatedColumn` 或 `ScalarFunction` 的表达式也不深入遍历。新增复合表达式实现时需要确认该保守行为是否仍正确。
- `PushDownTopN` 通过 `mem::replace` 把当前节点移入 TopN 子节点，留下默认值；调用者必须使用返回的新根，不能再依赖原变量保持完整算子状态。

## 并发与资源生命周期

逻辑计划定义采用 `Rc<RefCell<_>>` 而非 `Arc<Mutex<_>>`，因此 `CTEClass` 不是用于跨线程并发修改的结构。其动态借用范围在 `DeriveStats` 中被刻意缩短：种子计划先 `take` 出来，完成可能递归进入优化器的工作后再放回，避免在回调期间持有共享可变借用。

`SeedStat` 使用 `Arc<RwLock<_>>`，读取时克隆快照，推导结束后整体覆盖；这提供锁级内存安全，但本文件没有版本号或比较交换语义，不表达多个线程并发推导同一 CTE 的一致性协议。实际规划主状态仍受 `Rc` 限制。

`OPTIMIZE_CTE_SEED` 生命周期为进程全局且只能安装一次。`logical_optimize_in_place` 忽略重复安装的返回值，首次安装的函数指针持续有效。谓词缓冲在首次统计推导时被清空，种子计划在方法执行期间短暂离开 `CTEClass`，但所有正常 `Result` 返回路径都会在递归统计之前放回；优化/统计错误发生在放回之前时，当前实现会提前返回，使共享定义中的种子计划保持为 `None`，这是扩展错误恢复时必须注意的状态边界。

## 与 Go 版本的对应关系

直接对照 `pkg/planner/core/operator/logicalop/logical_cte.go`：两侧都只对最外层非递归 CTE 收集谓词，Apply 外排除关联谓词，以 DNF 汇聚各引用条件并提取公共过滤，按 schema 位置重映射 NDV，递归 `UNION ALL` 累加行数、递归 DISTINCT 以 NDV 估算行数，并合并种子/递归关联列。

已验证的实现差异如下：

- Go `CTEClass` 同时持有种子/递归物理计划；Rust 结构只有逻辑计划，以 `OptimizeCTESeed` 回调就地优化逻辑树并从逻辑节点读取统计。
- Go 每次在缺少种子物理计划时调用 `DoOptimize`；Rust 用 `SeedPartLogicalOptimized` 防止重复优化，并允许回调尚未安装时继续。
- Go 在 `OnlyUsedAsStorage` 时把 seed 设为当前节点 child；Rust 当前只保留该字段，没有对应分支。
- Go 会独立优化递归逻辑计划，并在优化期间临时关闭 parallel apply；Rust 当前仅调用递归计划的 `DeriveStats`，没有这段优化和会话变量保护。不能据此声称 Rust 已具备 Go 的递归执行并发保护。
- Go `PreparePossibleProperties` 可合并调用方传入的所有有效 child 属性；Rust 无 child 参数，只读取共享种子计划缓存的 `PreparePossiblePropertiesValue`，忽略递归计划。现有 Rust 测试明确验证这种 seed fallback 行为。
- Go 的 `ColumnMap` 类型为 `map[string]*Column` 并通过规则工具替换；Rust 使用 `HashMap<i64, Column>`，键为 `UniqueID`，通过本文件的递归函数替换。
- Go 内存估算计入物理计划、字符串键和指针成本；Rust 没有物理计划字段，仅估算表达式和映射值，因此数值口径不可直接比较。

## 扩展指南

- 新增谓词可下推表达式种类时，优先扩展 `resolve_cte_expression`，确保递归复制、列替换及哈希缓存失效语义完整；同步在独立的 `logical_cte_test.rs` 增加 Column、CorrelatedColumn、嵌套 ScalarFunction 和缺失映射用例。
- 修改 CTE 构建/列映射时，应同时检查 `logical_plan_builder_runtime.rs` 写入 `ColumnMap` 的位置和 `LogicalCTETable` 的递归引用分支，保持每个可见列与种子列的一一对应及顺序不变量。
- 改动统计算法时，应覆盖缓存命中、谓词 Selection 注入、普通递归累加、DISTINCT NDV 估算、schema 长度或列 ID 不一致等边界；现有 `recursive_distinct_cte_estimates_union_cardinality_from_combined_ndv` 是最直接的回归基线。
- 若补齐 Go 的递归优化、`OnlyUsedAsStorage` 或并发保护，修改范围会跨到优化器和执行器，不能只在本文件内增加字段或桩。应依据 Go 的实际生命周期接线，并在独立测试文件验证错误路径恢复共享 seed、会话变量恢复和多引用行为。
- 修改 `PreparePossibleProperties` 时需要决定 Rust API 是继续读 seed 缓存，还是恢复 Go 的多 child 合并输入；同步更新 `logical_cte_test.rs` 和 `logicalop_test/logical_operator_test.rs`，并注意 TiFlash 路径选择兼容性。
- 性能风险主要来自跨引用谓词 DNF 增长、表达式深克隆、重复统计推导及共享计划的再次优化；正确性风险集中在列 ID/列序映射、错误提前返回遗失 seed、递归 DISTINCT 行数口径和关联谓词误下推。

## 验证依据

- 目标源码：`pkg/planner/core/operator/logicalop/logical_cte.rs`（397 行），核对全部类型、函数、impl 和无条件编译项；该文件没有 `cfg` 条件项。
- crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml` 与 `lib.rs`，核对包名、直接依赖、Go 包映射、模块装载、再导出和独立测试注册。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标已索引；`explore "pkg/planner/core/operator/logicalop/logical_cte.rs LogicalCTE LogicalCTETableSource"` 定位目标、Go 对照及调用边；`node --file` 分段核对目标全文件、构建器、优化器安装点和测试；`query InstallOptimizeCTESeed`、`query CTEClassRef`、`query PredicatePushDown --kind function` 用于消歧。
- 上游源码：`pkg/planner/core/logical_plan_builder_runtime.rs` 的 `CteBinding` 和 CTE 表源构建分支；`pkg/planner/core/optimizer_runtime.rs::logical_optimize_in_place`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_cte.go`，重点核对 `PredicatePushDown`、`DeriveStats`、`PreparePossibleProperties`、`ExtractCorrelatedCols` 及 `CTEClass` 字段。
- Rust 测试：`pkg/planner/core/operator/logicalop/logical_cte_test.rs` 验证递归 DISTINCT 的 NDV/行数与 seed TiFlash fallback；`pkg/planner/core/operator/logicalop/logicalop_test/logical_operator_test.rs::TestLogicalCTEPreparePossiblePropertiesSkipNilChild` 验证无 seed 时返回空 order 且无 TiFlash。测试与源文件分离。
- Go 测试：`pkg/planner/core/operator/logicalop/logicalop_test/logical_operator_test.go::TestLogicalCTEPreparePossiblePropertiesSkipNilChild` 验证 Go 对 nil child 的过滤与有效 child 的 TiFlash 属性；它同时说明 Go 的属性 API 与 Rust seed-only API 不完全相同。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证恰有 11 个固定二级标题，并人工复查所有“已支持”陈述均来自上述源码、调用边或测试。
