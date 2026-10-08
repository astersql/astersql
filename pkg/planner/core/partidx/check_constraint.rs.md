# `pkg/planner/core/partidx/check_constraint.rs`

## 文件定位

本文件是 `astersql-planner-core-partidx` crate 的部分索引（partial index）约束蕴含判定实现。crate 入口 `pkg/planner/core/partidx/lib.rs` 将本模块的公开类型和函数全部再导出；根 `Cargo.toml` 以 `facade_planner_core_partidx` 引入该 crate，`pkg/lib.rs` 再把它暴露为 `planner::core::partidx`。

它要回答两个与索引选择有关的问题：查询过滤条件 `filters` 是否蕴含索引元数据中的前置谓词 `pre_predicates`；以及在计划缓存场景下，过滤条件是否必然拒绝目标列上的 `NULL`。Go 生产链在 `pkg/planner/core/operator/logicalop/logical_datasource.go` 的 `DataSource.CheckPartialIndexes` 中调用同路径 Go 实现来删除不可用访问路径，并为不能稳定满足条件的缓存计划设置 `NoncacheableReason`。当前仓库的 RustCodeGraph 和文本检索没有发现 Rust 生产代码调用本文件两个公开入口，只有 crate/facade 再导出和独立单元测试，因此 Rust 版本目前应视为“算法已移植、生产接线未验证”，不能把 Go 调用链当作 Rust 已接线事实。

## 核心职责

- `CheckConstraints` 先尝试表达式一对一精确匹配，再对单个前置谓词尝试有限的蕴含证明。比较谓词通过 ranger 区间集合包含关系证明；规范形式 `NOT(IS NULL(column))` 通过生成区间是否排除 `NULL` 证明。
- `AlwaysMeetConstraints` 专门处理单个 `NOT(IS NULL(column))` 前置谓词，并递归分析过滤表达式的 `AND`/`OR` 结构，判断至少一条过滤条件是否必然拒绝该列的 `NULL`。该路径对应 Go 计划缓存的特殊处理，不是一般布尔定理证明器。
- `ConstraintContext` 把表达式相等、列访问条件提取、区间构造和区间合并交给调用方，使排序规则、类型转换、告警、内存配额与计划缓存策略仍由真实规划器上下文负责。
- `StructuralContext` 只提供不依赖会话语义的结构判等和按列抽取；其区间构造、区间并集会明确返回 `RangeError`，因此它只适合精确匹配及 `AlwaysMeetConstraints` 的结构化路径，不足以验证 ranger 蕴含路径。

## 主要符号

- `Column { id, unique_id, field_type }`：规划器列引用。`structural_expression_equal` 和 `containsColumn` 都以 `unique_id` 判断逻辑列身份，物理 `id` 和字段类型不参与结构判等。
- `Literal`、`CompareOp`、`FunctionName`、`ScalarFunction`、`Expression`：本 crate 内的轻量表达式模型。`Expression::column`/`scalar` 是安全的节点类型访问器；比较算子包括普通大小/等值、不等、NULL-safe 等值和 `IN`。
- `BoundValue` 与 `Range`：表达 ranger 结果所需的区间端点、低高边界向量及开闭标志。`implCompareExpr` 依赖完整 `Range` 相等来判断并集是否改变前置谓词区间。
- `RangeError`：上下文区间操作失败的字符串错误包装，实现 `Display` 和 `Error`。
- `ConstraintContext`：算法与真实表达式/ranger 服务之间的接口；四个方法分别负责表达式相等、构造列区间、抽取目标列访问条件、合并区间。
- `StructuralContext`：轻量实现。`expressions_equal` 递归比较树；`extract_access_conditions_for_column` 保留任何包含目标 `unique_id` 的表达式；另外两个方法返回“不具备 planner ranger adapter”的错误。
- `CheckConstraints`、`AlwaysMeetConstraints`：两个公开业务入口。其余 `exactMatch`、`canBeImpliedFromExprs`、`implCompareExpr`、`implIsNotNull`、`checkIsNullRejected`、`containsColumn` 都是模块内部辅助函数。
- 文件没有模块级可变状态、宏、条件编译项或异步函数；测试条件编译发生在相邻 `lib.rs`，测试代码独立位于 `check_constraint_aster_unit_test.rs`。

## 执行流程

`CheckConstraints(context, pre_predicates, filters)` 的流程如下：

1. 前置谓词为空时直接返回 `true`；数量不是一个时返回 `false`。这使后续只需处理单谓词元数据。
2. `exactMatch` 为每个过滤项维护一次性 `matched` 标记，逐个寻找 `context.expressions_equal` 的过滤表达式，避免一个过滤项重复满足多个前置谓词；当前入口限制为单谓词，但辅助函数仍保留一般的一对一算法。
3. 精确匹配失败后，`canBeImpliedFromExprs` 要求前置谓词为标量函数。若形式为 `UnaryNot(IsNull(Column))`，进入 `implIsNotNull`；若为 `Compare(_)`，进入 `implCompareExpr`；其他形态返回 `false`。
4. `implCompareExpr` 从比较函数前两个参数中找列，分别构造前置谓词区间和过滤条件在该列上的区间。它合并“过滤区间 + 前置谓词区间”，调用 `union_ranges(..., false)`；仅当合并结果与原前置谓词区间完全相等时返回 `true`，即过滤区间是前置谓词区间的子集。
5. `implIsNotNull` 抽取目标列条件并构造区间；只有区间非空，且每个区间都不以“包含的 `Null` 下界”开始，才证明过滤条件排除了 `NULL`。

`AlwaysMeetConstraints(context, pre_predicates, filters)` 只接受单个 `UnaryNot(IsNull(Column))`。随后遍历顶层过滤项：`checkIsNullRejected` 对 `OR` 要求所有标量分支都拒绝 `NULL`，对 `AND` 只要求至少一个标量分支拒绝 `NULL`；目标列上的 `IS NULL` 和 `NullEqual` 明确不构成拒绝；其他比较只要任一前两个参数是同一逻辑列即构成拒绝。任一顶层过滤项通过便返回 `true`。

## 数据与状态

表达式、区间和错误值均为调用栈上的拥有型 Rust 数据；算法通过切片借用输入。只有在需要传给 ranger 或拼接区间时才克隆表达式、列或区间。`exactMatch` 分配一个长度等于 `filters.len()` 的布尔向量；`implCompareExpr` 把过滤区间移动到 `combined` 后追加前置谓词区间克隆；`StructuralContext::extract_access_conditions_for_column` 会克隆命中的过滤表达式。

关键不变量是逻辑列身份使用 `Column.unique_id`。`Column.id` 与 `field_type` 只是携带的元数据；独立测试特意让它们不同而保持 `unique_id` 相同，以验证结构判等仍成立。区间包含证明还依赖调用方返回规范化、可按 `PartialEq` 比较的 `Vec<Range>`；本文件不自行排序或规范化区间。

## 依赖与调用关系

RustCodeGraph 对 `CheckConstraints` 的 callees 给出 Rust `exactMatch` 和 `canBeImpliedFromExprs`，对 `AlwaysMeetConstraints` 给出 Rust `checkIsNullRejected`（图结果同时包含同名 Go 节点，文档只把同文件 Rust 节点视为 Rust 调用边）。两个公开入口的 Rust callers 查询均为空；`rg` 只找到 `check_constraint_aster_unit_test.rs` 的直接调用。因此已验证的 Rust 关系是：

`lib.rs` 再导出 → `CheckConstraints` → `exactMatch` / `canBeImpliedFromExprs` → `implCompareExpr` / `implIsNotNull` → `ConstraintContext` 服务；以及 `lib.rs` 再导出 → `AlwaysMeetConstraints` → `checkIsNullRejected` → `ConstraintContext::expressions_equal`。

`pkg/planner/core/partidx/Cargo.toml` 声明了 expression、parser AST、plan context、intest、ranger、ranger context 五个可选依赖，但当前文件没有直接引用这些 crate；它通过本地轻量类型和 `ConstraintContext` 隔离真实服务。仓库中也只有 `StructuralContext` 一个 `ConstraintContext` 实现，尚未找到连接这些可选依赖的生产适配器。

Go 对照的生产上游是 `DataSource.CheckPartialIndexes`：它解析索引 `ConditionExprString`、拆分 CNF，再调用 `partidx.CheckConstraints`；使用计划缓存时继续调用 `partidx.AlwaysMeetConstraints`。这说明本算法在完整应用中的目标位置，但不证明 Rust 规划器已采用该实现。

## 错误处理与边界

公开接口用保守的 `bool` 表示“已证明/未证明”，不会把无法处理的形式当作满足约束。前置谓词数量错误、节点类型不符、缺少列参数、没有目标列访问条件、空区间、区间构造失败或区间合并失败，都会返回 `false`。`RangeError` 的具体信息在这些路径中被丢弃；调用者只能知道证明失败，不能区分逻辑不蕴含与适配器故障。

`StructuralContext` 的 ranger 方法总是报错，所以使用它调用非精确匹配的 `CheckConstraints` 会保守返回 `false`。`checkIsNullRejected` 只检查比较表达式前两个参数；空参数通过 `first`/`get` 安全失败。`OR` 中出现非标量分支会使该 `OR` 证明失败，`AND` 中的非标量分支则被忽略，只要另一个标量分支能拒绝 `NULL` 即可。

此实现不是完整 SQL 逻辑蕴含器：只支持单个前置谓词、单列比较区间及规范的 `NOT(IS NULL(column))`；普通 `IS NULL`、任意函数组合或多谓词只能依靠精确匹配。比较值的排序规则、类型转换和 SQL 警告语义完全取决于未来的真实 `ConstraintContext` 适配器。

## 并发与资源生命周期

文件不创建线程、异步任务、锁、通道、事务或外部句柄；所有临时集合在函数返回时释放。公开函数只共享不可变 `&C` 和输入切片，因此本文件自身没有并发写入或竞态状态。它也没有给 `ConstraintContext` 增加 `Send`/`Sync` 约束；若调用方跨线程共享上下文，线程安全责任属于上下文实现和上层规划器。

资源成本主要来自表达式/区间克隆和 ranger 调用。传给 `build_column_ranges` 的 `column_length` 固定为 `-1`、`range_memory_quota` 固定为 `0`，传给 `union_ranges` 的 `merge_consecutive` 固定为 `false`，其真实资源含义与限制必须由适配器保持与 Go ranger 一致。本文件不持有 ranger 返回值到调用之外，也不缓存证明结果。

## 与 Go 版本的对应关系

同路径 `check_constraint.go` 是直接语义基线。Rust 的 `CheckConstraints`/`exactMatch`/`canBeImpliedFromExprs`/`implCompareExpr`/`implIsNotNull` 和 `AlwaysMeetConstraints`/`checkIsNullRejected` 与 Go 函数分层对应；区间并集相等、`OR` 全分支拒绝、`AND` 任一分支拒绝以及排除 `NullEQ` 的规则保持一致。

已确认的实现差异：

- Go 在 intest 构建中断言非空 `prePredicates` 恰有一个，随后直接索引首项；Rust 对数量不为一直接返回 `false`，并通过 `Option` 模式匹配避免类型断言或参数越界崩溃。
- Go 使用真实 `expression.Expression`、`planctx.PlanContext` 和 ranger 类型；Rust 当前定义轻量镜像类型，并要求 `ConstraintContext` 适配服务。仓库中未发现生产适配器，因此 ranger 路径的生产可用性尚未验证。
- Go 的表达式相等由 eval context 决定；Rust `StructuralContext` 对列只比较 `unique_id`，对标量树递归比较函数名和参数，对字面量直接比较枚举值。该行为由独立 Rust 测试验证，但不能替代带排序规则和类型转换的真实上下文。
- Go 已由 `logical_datasource.go` 接入部分索引访问路径筛选；Rust 当前只有 workspace/facade 暴露和单元测试调用，未找到等价生产上游。

## 扩展指南

新增支持时应优先保持 `CheckConstraints` 的“不能证明即 false”契约。增加新的前置谓词形态，应在 `canBeImpliedFromExprs` 分流并以独立私有函数实现；扩展空值拒绝逻辑，应修改 `checkIsNullRejected`，同时明确 `AND`/`OR` 三值逻辑和 NULL-safe 运算符语义。支持多列区间时，需重新审查列选择、`Range.low_values/high_values` 的逐列顺序以及并集相等条件，不能沿用当前只观察首个低端点的 `implIsNotNull`。

要真正接入 Rust 规划器，应新增位于适当规划器 crate 的 `ConstraintContext` 实现，把真实表达式相等、ranger 构造和区间并集映射进来；不要把排序规则、转换、告警或内存策略复制到本文件。接线后还应从 Rust 数据源/访问路径选择处调用两个公开入口，并验证 facade 之外确有生产调用边。

测试必须继续放在独立的 `pkg/planner/core/partidx/check_constraint_aster_unit_test.rs`，不要嵌入生产源文件。新增 ranger 适配器时至少补充：过滤区间严格包含/等于/不相交、开闭端点、NULL 区间、`IN`、类型转换和排序规则、构造/合并错误、多个前置谓词的保守行为；计划缓存接线还应覆盖混合 `AND`/`OR` 与 `NullEqual`。同时对照 `check_constraint.go` 及 Go 部分索引集成测试，防止 Rust 逻辑被简化。

## 验证依据

- 源码全貌：`pkg/planner/core/partidx/check_constraint.rs`（454 行），核对全部公开类型、trait、公开入口及私有辅助函数。
- crate/装配：`pkg/planner/core/partidx/Cargo.toml`、`pkg/planner/core/partidx/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`；确认 crate 名称、可选依赖、测试文件独立装配及 facade 暴露。
- RustCodeGraph：`status` 显示目标仓库已索引；`node --file ...` 读取目标文件；`query CheckConstraints` 与 `query AlwaysMeetConstraints` 定位 Rust/Go 同名符号；带 `--file` 的 callers 对两个 Rust公开入口均返回空数组；callees 分别返回 `exactMatch`/`canBeImpliedFromExprs` 与 `checkIsNullRejected`。
- 文本调用检索：`rg` 找到 Rust 直接调用仅位于 `pkg/planner/core/partidx/check_constraint_aster_unit_test.rs`；`impl ConstraintContext` 检索只找到 `StructuralContext`；未找到 Rust 生产适配器或规划器调用点。
- Go 对照：`pkg/planner/core/partidx/check_constraint.go`；生产上游：`pkg/planner/core/operator/logicalop/logical_datasource.go` 的 `DataSource.CheckPartialIndexes`。
- 独立 Rust 测试：`pkg/planner/core/partidx/check_constraint_aster_unit_test.rs`，覆盖按 `unique_id` 判等、空约束与精确匹配、普通比较和 `NullEqual`、`AND`/`OR` 空值拒绝及显式 `IS NULL` 反例。未发现同包专门的 Go `check_constraint_test.go`；仓库中的 Go 部分索引集成覆盖位于 planner case/index 等测试面，但本纯文档任务未运行测试或 Cargo。
