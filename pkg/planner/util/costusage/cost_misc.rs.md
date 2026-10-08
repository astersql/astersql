# `pkg/planner/util/costusage/cost_misc.rs`

## 文件定位

本文件属于独立 crate `astersql-planner-util-costusage`；crate 根 `pkg/planner/util/costusage/lib.rs` 通过 `pub mod cost_misc` 声明模块，并用 `pub use cost_misc::*` 对外重导出全部公开项。`pkg/planner/util/costusage/Cargo.toml` 的 `package.metadata.porting.go-package` 指向同目录 Go 包，说明它是 `pkg/planner/util/costusage/cost_misc.go` 的 Rust 移植边界。

该 crate 被 planner core、`core/base`、物理算子和 `utilfuncp` 作为低层依赖引用。例如 `pkg/planner/core/Cargo.toml` 以 `costusage-dependency` 引入它，`pkg/planner/core/operator/physicalop/Cargo.toml` 以 `costusage` 引入它。这样，物理计划接口可以共享成本类型和标志而不反向依赖完整 planner core，避免 Go 文件头注释所描述的 `base`、`util` 与 `core` 循环依赖。

在运行链路中，本文件不读取统计信息，也不决定某种物理算子的成本公式；它提供 Cost Model V2 的通用“值 + 可选追踪”容器及组合运算。具体算子公式主要由 `pkg/planner/core/plan_cost_ver2.rs` 构造，`pkg/planner/core/operator/physicalop/base_physical_plan.rs::GetPlanCostVer2` 则负责递归取得子计划成本、汇总并缓存。

## 核心职责

1. 用 `CostVer2` 保存优化器比较候选计划所需的原始 `f64` 成本，并可选保存 `CostTrace`。
2. 用三个位标志表达强制重算、使用真实基数和记录成本公式；本文件只直接解释 TRACE，其他标志由上层消费。
3. 在 `new_cost_ver2` 中惰性生成公式，避免未开启追踪时承担字符串构造开销。
4. 为成本项提供求和、除法、乘法和“不写入 trace 的附加值”操作，同时让数值、因子贡献和展示公式保持规定的一致关系。
5. 通过 `format_go_float` 与私有的 `format_go_fixed_2` 保持 Rust 追踪文本与 Go 的 `%v`、`strconv.FormatFloat(..., 'f', 2, 64)` 风格一致，包括非有限值和科学计数法。

本文件不是完整的成本模型：它不包含表扫描、网络、CPU、内存等公式，也不处理计划节点遍历产生的错误；这些职责位于上层 `plan_cost_ver2.rs` 和物理算子实现中。

## 主要符号

- `COST_FLAG_RECALCULATE = 1`：要求上层忽略已缓存成本。直接消费者之一是 `base_physical_plan.rs::GetPlanCostVer2`。
- `COST_FLAG_USE_TRUE_CARDINALITY = 2`：通知上层使用真实基数。此文件只定义该位，不解释其业务效果。
- `COST_FLAG_TRACE = 4`：开启 `CostTrace` 及公式生成。
- `CostVer2 { cost, trace }`：两个字段均为私有，调用方通过 `get_cost` 和 `get_trace` 只读访问。类型实现 `Clone`，供计划缓存返回副本及组合运算使用。
- `CostVer2::get_cost(&self) -> f64`：有限负值钳制为 `0`，非负值原样返回；NaN 特判为原样传播。内部组合仍使用未钳制的私有 `cost`，钳制只发生在公开读取边界。
- `CostTrace { factor_costs, formula }`：记录因子名到累计贡献的 `HashMap<String, f64>`，以及用于 EXPLAIN 类展示的公式字符串；getter 返回借用，不暴露可变入口。
- `new_zero_cost_ver2(trace)` 与 `ZERO_COST_VER2`：分别构造可选择追踪的零值，和惰性初始化、无追踪的进程级零值。前者在开启追踪时创建空 map 与空公式。
- `has_cost_flag`、`trace_cost`：执行位掩码判断；`trace_cost(None)` 为 `false`。
- `CostVer2Factor { name, value }`：因子描述；`Display` 输出 `name(value)`。`value` 用于显示因子本身，而 `new_cost_ver2` 写入 map 的贡献值是参数 `cost`。
- `format_go_float`：公开的 Go 风格浮点格式化辅助函数。它固定输出 `NaN`、`+Inf`、`-Inf`，规范指数符号与至少两位指数，并按 Go `%g` 的阈值在指数范围 `[-4, 6)` 外采用科学计数法。
- `new_cost_ver2(option, factor, cost, lazy_formula)`：基础成本项构造器。只有 TRACE 开启时才调用闭包、建立 trace，并以 `factor.name -> cost` 初始化贡献。
- `sum_cost_ver2(&[CostVer2])`：累加原始数值；只要输入中有 trace 就创建结果 trace，同名因子相加，非空公式按 `(formula) + (formula)` 拼接。
- `div_cost_ver2`、`mul_cost_ver2`：按标量缩放原始值与所有因子贡献，并生成带两位小数标量的公式；输入无 trace 时输出也无 trace。
- `add_cost_without_trace`：消费并返回 `CostVer2`，只修改原始数值，特意不改因子 map 和公式，供不应出现在解释输出中的微小 tie-breaker 使用。
- `PlanCostOption { cost_flag }`、`new_default_plan_cost_option`、`with_cost_flag`：承载标志集合。`with_cost_flag` 是按值建造者，直接替换 `cost_flag`，不会与旧值自动按位或；需要组合标志时调用者必须先自行使用 `|`。

文件没有 trait、枚举、泛型类型定义或条件编译项；唯一的闭包泛型是 `new_cost_ver2` 的 `impl FnOnce() -> String` 参数。

## 执行流程

典型调用从上层成本公式开始。例如 `plan_cost_ver2.rs::canonical_term` 先构造 `CostVer2Factor`，再调用 `new_cost_ver2` 传入数值贡献和公式闭包：

1. `trace_cost(Some(option))` 检查 `COST_FLAG_TRACE`。
2. 未开启时，只保存 `cost`，闭包不执行，`trace` 为 `None`。
3. 开启时，执行一次 `lazy_formula`，创建因子 map，并将当前成本贡献记到因子名下。
4. 上层把多个基础项交给 `sum_cost_ver2`；它按输入顺序累计数值与公式，同名因子按 key 合并。
5. 若某部分成本被并发度、批量比例等摊薄或放大，上层使用 `div_cost_ver2` / `mul_cost_ver2`，数值、因子贡献和公式同步缩放。
6. `base_physical_plan.rs::GetPlanCostVer2` 在无孩子时用 `new_zero_cost_ver2(trace_cost(...))`，有孩子时用 `sum_cost_ver2`；随后缓存克隆值。若未设置 RECALCULATE，后续请求可以直接返回缓存。
7. 最终消费者用 `get_cost` 取得用于比较的非负公开成本，用 `get_trace` 读取因子分解和公式。

格式化流程独立于成本计算：`CostVer2Factor::fmt` 使用 `format_go_float`；除法和乘法公式中的标量使用 `format_go_fixed_2`，所以普通标量固定两位小数，而 NaN/Infinity 仍使用 Go 拼写。

## 数据与状态

`CostVer2` 是拥有所有权的值对象。`Clone` 会复制 `String` 和 `HashMap`，因此上层缓存与返回值之间没有共享可变状态。原始 `cost` 可以是负数、NaN 或无穷大；只有 `get_cost` 钳制有限负数，组合函数遵循 Rust `f64` 的 IEEE 754 运算继续传播特殊值。

`CostTrace.factor_costs` 的 key 是因子名称，value 是该因子在当前组合成本中的数值贡献。`sum_cost_ver2` 的不变量是：每个 key 的结果等于所有 traced 输入中该 key 贡献之和。公式只是展示串，不参与计算；由 `new_zero_cost_ver2(true)` 产生的空公式在求和时跳过，防止出现无意义的空括号。

`ZERO_COST_VER2` 是 `LazyLock<CostVer2>`，首次访问时创建无 trace 零值，之后只通过共享引用读取。普通构造与算术函数均返回新值（`add_cost_without_trace` 接收所有权后原地改局部值），不修改输入或全局零值。

`PlanCostOption.cost_flag` 是一个 `u64` 位集合。三个常量当前分别占最低三位；新增标志必须选择未占用的位，并核对所有上层缓存、基数和追踪分支。

## 依赖与调用关系

本文件只依赖 Rust 标准库：`HashMap`、`LazyLock`、`fmt`、`String` 和 `f64` 格式化；目标 `Cargo.toml` 没有声明第三方依赖或 feature。

已核对的上游调用关系包括：

- `pkg/planner/core/plan_cost_ver2.rs::canonical_term`、`canonical_net_cost` 及各算子成本分支调用 `new_cost_ver2` 创建原子成本项，并广泛调用 `sum_cost_ver2`、`div_cost_ver2`、`mul_cost_ver2` 组合公式。
- `pkg/planner/core/plan_cost_ver2.rs` 在需要决定宽度计算或零项是否保留追踪时调用 `trace_cost` 和 `new_zero_cost_ver2`。
- `pkg/planner/core/operator/physicalop/base_physical_plan.rs::GetPlanCostVer2` 使用 `COST_FLAG_RECALCULATE` 控制缓存，递归取得孩子成本后调用零值或求和辅助函数。
- 多个物理算子文件通过 `costusage::{CostVer2, PlanCostOption}` 暴露统一的 `GetPlanCostVer2` 接口；它们把计算委托给 core 路由或子计划，因此本文件构成接口层和具体公式层之间的共享数据契约。

RustCodeGraph 的文件节点报告 `cost_misc.rs` 被 10 个已索引文件使用，并列出 `base_physical_plan.rs` 及其测试等直接使用者。精确 `query` 能定位 `new_cost_ver2`、`sum_cost_ver2`、`div_cost_ver2`、`mul_cost_ver2`；当前索引的 `callers/callees` 子命令未输出边文本，因此上述具体边又通过对应调用点源码与 `rg` 交叉核对，不能把“无输出”解释为无调用者。

## 错误处理与边界

本文件所有公开函数都是非 `Result` API，不主动产生业务错误。边界行为如下：

- `get_cost` 对有限负值返回 `0`，对 NaN 原样返回；正无穷也原样返回。
- `sum_cost_ver2(&[])` 返回无 trace 零值；只有 traced 输入才会使结果携带 trace。
- 求和时空公式不拼接，但其因子 map（若有）仍会合并。
- 除数为零、NaN 或无穷时不报错，按 `f64` 规则得到 Infinity/NaN/零等结果，因子值同样缩放；公式仍记录该操作。调用者必须保证这些结果符合上层成本模型预期。
- `format_go_float` 中对 Rust 已生成的指数执行 `parse::<i32>().expect(...)`，其不 panic 的前提是标准库 `f64::to_string` 始终生成合法数字指数；这是内部格式契约，不是用户输入解析。
- 因子名为空、重复或公式为空均不会被拒绝。重复名称在求和时归并；名称与公式语义正确性由上层公式提供者负责。
- `with_cost_flag` 覆盖而非累加旧标志。连续调用不会保留前一次值，这是与 Go 实现一致但容易误用的 API 边界。

## 并发与资源生命周期

文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。`CostVer2` / `CostTrace` 的生命周期完全由所有权和借用管理；getter 返回的引用不能超过所属成本对象。

唯一全局状态 `ZERO_COST_VER2` 使用标准库 `LazyLock` 做线程安全的一次初始化，初始化后没有可变访问。组合运算分配新的 map/字符串并克隆 key，因此不同计划成本对象不会共享可变 trace。代价是开启 TRACE 时会产生字符串和 map 分配；惰性闭包确保关闭 TRACE 后避开这部分资源开销。

## 与 Go 版本的对应关系

Rust 符号基本逐项对应 `pkg/planner/util/costusage/cost_misc.go`：三个 `CostFlag*` 常量、`CostVer2`、`CostTrace`、零值构造、标志判断、追踪判断、基础项构造、因子、求和、乘除、无追踪附加值以及 `PlanCostOption` 均保留 Go 语义。

需要注意的语言适配：

- Go 用 `*CostTrace == nil`，Rust 用 `Option<CostTrace>`；Go 的可变指针 getter 在 Rust 中变为只读借用。
- Go 的可变接收者 `WithCostFlag` 返回原指针，并对 nil 接收者返回 nil；Rust 使用按值建造者，不存在 nil 接收者。两者都替换标志字段而非自动 OR。
- Go 的变参 `SumCostVer2(costs ...CostVer2)` 在 Rust 中是切片参数 `&[CostVer2]`。
- Go 传值的乘除和附加函数在 Rust 中分别使用不可变借用或所有权，以避免不必要地要求调用方共享可变对象。
- Rust 显式实现 `format_go_float` 和固定两位格式，补偿 Rust 默认格式与 Go `%v` / `FormatFloat` 在 Infinity、指数形式等细节上的差异。
- Rust `get_cost` 显式保留 NaN 后再对普通值执行 `max(0.0)`，使移植意图和测试边界清晰。

Go 同目录没有 `*_test.go` 专门测试这些辅助函数；本仓库的直接移植回归位于独立 Rust 文件 `cost_misc_test.rs` 和 `migration_aster_unit_test.rs`。更高层 Go 行为由 `pkg/planner/core/plan_cost_ver2_test.go` 等成本模型测试覆盖，但它们验证的是整体公式与 SQL/计划效果，不是本文件每个辅助函数的逐项单测。

## 扩展指南

- 新增成本标志：在三个 `COST_FLAG_*` 后选择独立位；同步 Go 常量、`PlanCostOption` 消费点和独立 Rust 测试。若影响缓存，重点检查 `base_physical_plan.rs::GetPlanCostVer2`；若影响公式构造，检查 `plan_cost_ver2.rs`。
- 新增基础因子或公式：通常不需要改本文件，应在 `plan_cost_ver2.rs` 的具体算子函数中用 `CostVer2Factor` 和 `new_cost_ver2` 接入，并验证因子名与 Go 一致。
- 修改组合语法：同时维护数值、`factor_costs` 与 `formula` 三层语义，尤其保留空公式跳过、同名因子累加和括号优先级；更新 `migration_aster_unit_test.rs`。
- 修改浮点展示：同步验证普通数、大小科学计数边界、NaN、正负 Infinity；直接测试放在 `cost_misc_test.rs`，不要内嵌回生产文件。
- 新增无追踪修正：先确认它是否应影响校准因子与 EXPLAIN。若不应展示，可沿用 `add_cost_without_trace`；若应展示，必须使用 traced 基础项和组合函数，避免数值与 trace 不一致。
- 性能风险集中在 TRACE 开启后的 map 合并、key 克隆和字符串拼接；新增热路径组合时应避免在未追踪分支提前构造公式。
- 兼容性风险集中在公开公式文本、因子名称、特殊浮点格式以及 `with_cost_flag` 的替换语义；这些可能被解释输出或成本校准工具依赖。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标 Rust/Go 文件；`files --filter pkg/planner/util/costusage` 确认目标、Go 对照和两个独立测试；`node --file` 完整读取 `cost_misc.rs`、`cost_misc.go`、`cost_misc_test.rs` 与 `lib.rs`；`query --kind function --json` 精确定位四个组合入口。文件节点还报告目标被 10 个索引文件使用。`callers/callees` 本次未返回文本，调用边因此使用下列源码调用点交叉验证。
- 源文件：`pkg/planner/util/costusage/cost_misc.rs`，核对全部 268 行公开/私有符号、分支和格式逻辑。
- crate 边界：`pkg/planner/util/costusage/Cargo.toml` 与 `pkg/planner/util/costusage/lib.rs`；依赖入口还核对了 `pkg/planner/core/Cargo.toml`、`pkg/planner/core/base/Cargo.toml`、`pkg/planner/core/operator/physicalop/Cargo.toml`、`pkg/planner/util/utilfuncp/Cargo.toml`。
- 上游调用：`pkg/planner/core/plan_cost_ver2.rs` 的 `canonical_term`、`canonical_net_cost` 及组合调用；`pkg/planner/core/operator/physicalop/base_physical_plan.rs::GetPlanCostVer2` 的缓存、递归汇总与重算分支。
- Go 对照：`pkg/planner/util/costusage/cost_misc.go` 全文件，并检索 `pkg/planner/core/plan_cost_ver2_test.go` 等上层 Go 成本模型测试。
- Rust 测试：`pkg/planner/util/costusage/cost_misc_test.rs` 验证非有限值和科学计数法格式；`pkg/planner/util/costusage/migration_aster_unit_test.rs` 验证标志位、公式惰性、零值、因子合并、乘除缩放、负值/NaN 和无追踪 tie-breaker。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试；交付结构使用任务指定命令验证。仓库提到的 `.agents/skills/tidb-verify-profile` 在当前工作区不存在，无法额外执行该技能入口。
