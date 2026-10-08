# `pkg/planner/core/rule/logical_rules.rs`

源文件：[`logical_rules.rs`](logical_rules.rs)

## 文件定位

本文件是 `astersql-planner-core-rule` crate 的逻辑优化规则“选择协议”：它为每条逻辑改写分配一个稳定的 `u64` 位，并提供一个开启谓词下推位的辅助函数。crate 入口 [`lib.rs`](lib.rs) 通过私有 `mod logical_rules` 加载本文件，再用 `pub use logical_rules::*` 将全部常量和函数提升为 crate 公共 API；因此规划器通常以 `rule::FLAG_*` 使用它们，而不会直接引用模块路径。

它不实现任何计划树改写。真正的规则枚举、执行顺序、位到规则的映射和派发循环位于 [`../optimizer_runtime.rs`](../optimizer_runtime.rs) 的 `LogicalRule`、`LOGICAL_RULES`、`LOGICAL_RULE_FLAGS` 和 `logical_optimize_in_place`。计划构建器在 [`../logical_plan_builder_runtime.rs`](../logical_plan_builder_runtime.rs) 中逐步累加所需位，`DoOptimize` 再把最终掩码交给逻辑优化阶段。

crate 边界由 [`Cargo.toml`](Cargo.toml) 声明：包名为 `astersql-planner-core-rule`、库入口为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/planner/core/rule"` 标明 Go 对照包。本文件本身只使用 Rust 原生整数和按位运算，不直接依赖 Cargo 中列出的其他 crate；那些依赖供同一 crate 的具体规则模块和初始化桥接使用。

## 核心职责

1. 为 34 条逻辑规则定义互不重叠的单比特标志，范围从 `1 << 0` 到 `1 << 33`。
2. 保持标志数值稳定。文件注释明确这些值只在进程内使用，不持久化、也不在线路上传输；新增规则应追加新位，避免无意义地重编号旧标志。
3. 将“启用哪些规则”与“规则以什么顺序运行”解耦。位号表达身份，执行顺序由 `optimizer_runtime.rs::LOGICAL_RULES` 与 `LOGICAL_RULE_FLAGS` 的相同下标配对决定。
4. 通过 `set_predicate_push_down_flag` 为规则工具层提供一个保留原掩码、只开启谓词下推的回调实现；[`rule_init.rs`](rule_init.rs) 的 `init` 将它注册给 `astersql_planner_core_rule_util`。

本文件不负责校验未知位、不负责规则依赖闭包，也不负责决定默认启用集合。调用方可以组合任意位；派发器只检查它认识且列入 `LOGICAL_RULE_FLAGS` 的位。

## 主要符号

全部符号都是公开 API，没有模块级可变状态、trait、结构体、`impl` 或条件编译项。

| 位 | Rust 常量 | 规则语义 |
| ---: | --- | --- |
| 0 | `FLAG_GC_SUBSTITUTE` | 生成列表达式替换 |
| 1 | `FLAG_PRUNE_COLUMNS` | 首次列裁剪 |
| 2 | `FLAG_STABILIZE_RESULTS` | 稳定结果顺序 |
| 3 | `FLAG_BUILD_KEY_INFO` | 构建唯一键/候选键信息 |
| 4 | `FLAG_DECORRELATE` | 解除相关子查询 |
| 5 | `FLAG_SEMI_JOIN_REWRITE` | 半连接改写 |
| 6 | `FLAG_ELIMINATE_AGG` | 消除冗余聚合 |
| 7 | `FLAG_SKEW_DISTINCT_AGG` | 倾斜 `DISTINCT` 聚合处理 |
| 8 | `FLAG_ELIMINATE_PROJECTION` | 消除冗余 Projection |
| 9 | `FLAG_MAX_MIN_ELIMINATE` | MAX/MIN 聚合消除 |
| 10 | `FLAG_CONSTANT_PROPAGATION` | 常量传播 |
| 11 | `FLAG_PREDICATE_PUSH_DOWN` | 谓词下推 |
| 12 | `FLAG_JOIN_KEY_TYPE_CAST` | Join 键类型转换 |
| 13 | `FLAG_ELIMINATE_OUTER_JOIN` | 外连接消除 |
| 14 | `FLAG_PARTITION_PROCESSOR` | 分区表处理 |
| 15 | `FLAG_COLLECT_PREDICATE_COLUMNS_POINT` | 收集谓词列统计加载点 |
| 16 | `FLAG_PUSH_DOWN_AGG` | 聚合下推 |
| 17 | `FLAG_DERIVE_TOP_N_FROM_WINDOW` | 从窗口函数推导 TopN |
| 18 | `FLAG_PREDICATE_SIMPLIFICATION` | 谓词简化 |
| 19 | `FLAG_PUSH_DOWN_TOP_N` | TopN 下推 |
| 20 | `FLAG_ORDER_AWARE_JOIN_REORDER` | 保序感知 Join 重排 |
| 21 | `FLAG_SYNC_WAIT_STATS_LOAD_POINT` | 同步等待统计加载点 |
| 22 | `FLAG_JOIN_REORDER` | Join 重排 |
| 23 | `FLAG_OUTER_JOIN_TO_SEMI_JOIN` | 外连接转半连接 |
| 24 | `FLAG_CORRELATE` | 相关化 |
| 25 | `FLAG_PRUNE_COLUMNS_AGAIN` | 流水线后段的第二次列裁剪 |
| 26 | `FLAG_PUSH_DOWN_SEQUENCE` | Sequence 下推 |
| 27 | `FLAG_ELIMINATE_UNION_ALL_DUAL_ITEM` | 删除 UnionAll 中的 Dual 分支 |
| 28 | `FLAG_EMPTY_SELECTION_ELIMINATOR` | 空 Selection 消除 |
| 29 | `FLAG_RESOLVE_EXPAND` | 解析 Expand 算子 |
| 30 | `FLAG_FULLTEXT_INDEX_RESOLVE_WHERE` | 全文索引 WHERE 解析 |
| 31 | `FLAG_FULLTEXT_INDEX_RESOLVE_TOP_N` | 全文索引 TopN 解析 |
| 32 | `FLAG_FULLTEXT_INDEX_RESOLVE_PROJECTION` | 全文索引 Projection 解析 |
| 33 | `FLAG_FULLTEXT_INDEX_RESOLVE_REJECT` | 拒绝残余的不支持全文索引路径 |

`set_predicate_push_down_flag(flags: u64) -> u64` 返回 `flags | FLAG_PREDICATE_PUSH_DOWN`。它既不会清除其他位，也不会改变已开启位，因此具有幂等性：重复调用结果不变。

## 执行流程

典型流程如下：

1. [`../logical_plan_builder_runtime.rs`](../logical_plan_builder_runtime.rs) 的各类计划构建路径依据 SQL 结构，把相应 `FLAG_*` 按位或进 `builder.optFlag`。例如聚合构建路径会组合键推导、聚合下推、MAX/MIN 消除、TopN 下推、谓词下推、聚合消除和 Projection 消除等位。
2. [`../optimizer_runtime.rs`](../optimizer_runtime.rs) 的 `DoOptimize`/`do_optimize_with_update_projection_policy` 接收该 `u64`，先调用 `logical_optimize_in_place(flag, plan)`。
3. `logical_optimize_in_place` 先调用 `rule::rule_init::init()` 安装跨 crate 回调，然后对 `LOGICAL_RULES.iter().zip(LOGICAL_RULE_FLAGS)` 迭代。某个 `rule_flag` 若不在输入掩码中即跳过，否则执行同下标的 `LogicalRule` 分支。
4. 执行顺序不是位号顺序。例如全文索引 WHERE 位是第 30 位，却在常量传播之后、谓词下推之前执行；TopN 和 Projection 的全文索引规则也插在对应流水线位置。这样可追加稳定的新位，同时把规则放到语义要求的位置。
5. 当 operator/util 层需要在已有掩码上补开谓词下推时，它调用已注册的 `SetPredicatePushDownFlag` 钩子；[`rule_init.rs`](rule_init.rs) 把钩子绑定到本文件的 `set_predicate_push_down_flag`。

## 数据与状态

核心数据是一个 `u64` 位集合。每个常量恰有一个置位位元，组合使用按位或，检测使用 `flag & rule_flag != 0`，移除某条规则则使用与取反掩码，例如 [`../../optimize.rs`](../../optimize.rs) 的替代优化轮次用 `flag & !FLAG_DECORRELATE` 暂时关闭 decorrelate。

本文件自身没有堆分配、缓存、锁或全局可变对象。唯一函数是值传入、值返回的纯函数。跨模块注册状态位于 [`util/misc.rs`](util/misc.rs) 的 `OnceLock<SetPredicatePushDownFlagHook>`；这不是本文件持有的状态，但决定了辅助函数如何进入运行时调用链。

位值目前连续到第 33 位，尚有 `u64` 的第 34 至 63 位可供追加。未知高位会被 `set_predicate_push_down_flag` 原样保留；[`rule_init_test.rs`](rule_init_test.rs) 用第 63 位明确验证了这一性质。

## 依赖与调用关系

- 导出：[`lib.rs`](lib.rs) 通过 `pub use logical_rules::*` 再导出全部标志和辅助函数。
- 主要消费者：[`../optimizer_runtime.rs`](../optimizer_runtime.rs) 的 `LOGICAL_RULE_FLAGS` 消费全部 34 个标志，并与 `LOGICAL_RULES` 一一配对；`logical_optimize_in_place` 按掩码筛选规则。
- 掩码生产者：[`../logical_plan_builder_runtime.rs`](../logical_plan_builder_runtime.rs)、[`../expression_rewriter.rs`](../expression_rewriter.rs) 和 [`../planbuilder_runtime.rs`](../planbuilder_runtime.rs) 根据计划形态设置部分标志。
- 替代轮次调整：[`../../optimize.rs`](../../optimize.rs) 的 `AlternativeRoundKind` 会移除 `FLAG_DECORRELATE`，或补上 `FLAG_ORDER_AWARE_JOIN_REORDER`、`FLAG_CORRELATE`。
- 初始化桥接：[`rule_init.rs`](rule_init.rs) 的 `init` 把 `set_predicate_push_down_flag` 传给 `util::RegisterSetPredicatePushDownFlag`；[`util/misc.rs`](util/misc.rs) 的 `SetPredicatePushDownFlag` 再从 `OnceLock` 取出并调用它。
- 下游改写：本文件不直接调用任何规则实现。RustCodeGraph 的 `callees set_predicate_push_down_flag` 只得到对 `FLAG_PREDICATE_PUSH_DOWN` 的引用；调用者查询没有静态边，因为实际调用经过函数指针和 `OnceLock` 动态分派。

## 错误处理与边界

本文件没有 `Result`、`Option`、panic 或 I/O，按位或对所有 `u64` 输入均有定义。边界主要是协议一致性，而不是运行时错误：

- 标志必须是非零、单比特且互不重复，否则组合和筛选语义会混淆。
- `LOGICAL_RULES` 与 `LOGICAL_RULE_FLAGS` 必须等长且下标对齐，否则规则身份与实际实现会错配。Go 的 `TestOptRuleListFlagAlignment` 显式检查长度、单比特、唯一性和标志总数；Rust 的 `logical_optimizer_dispatches_rules_in_go_order_and_isolates_flags` 检查代表性位值、单规则隔离和执行顺序。
- 本文件不拒绝未映射位。派发循环只遍历已知映射，所以额外位被忽略；钩子还会保留这些位。这符合进程内组合掩码的宽松边界，但调用方不能把未知位误认为已实现规则。
- 注册前调用 `util::SetPredicatePushDownFlag` 会在工具层以“hook is not registered”触发编程错误；正常优化入口先调用 `rule_init::init`，且 `OnceLock` 使重复初始化安全。
- 位值虽声明为非持久化协议，测试和 Go/Rust 对齐仍依赖其稳定性；在中间插入新位会改变后续数值，应避免。

## 并发与资源生命周期

常量和纯函数天然可并发读取/调用，没有借用跨越调用、资源所有权转移、后台任务、通道、事务或清理过程。

唯一相关生命周期发生在外部注册层：`rule_init::init` 在逻辑优化开始前调用，`util/misc.rs` 用进程级 `OnceLock` 保存函数指针；首次注册后持续到进程结束。重复 `init` 不会替换已注册回调，`rule_init_test.rs::init_registers_all_go_rule_hooks_idempotently` 连续调用两次并验证钩子仍可用。此设计允许多个优化请求并发共享只读函数指针，不引入本文件级锁竞争。

## 与 Go 版本的对应关系

直接对照文件是 [`logical_rules.go`](logical_rules.go)。Go 用 `const` 与 `iota` 顺序产生 `FlagGcSubstitute` 至 `FlagFullTextIndexResolveReject`；Rust 把每个数值显式写成 `1 << n`，名称改为大写蛇形，但 34 个名字、位号和语义逐项对应。Go 的 `setPredicatePushDownFlag` 原地执行 `u |= FlagPredicatePushDown` 后返回，Rust 的 `set_predicate_push_down_flag` 直接返回按位或结果，行为等价。

Go 的 [`../optimizer.go`](../optimizer.go) 将 `optRuleList` 与 `optRuleFlags` 分成两个等长数组；Rust 的 `LOGICAL_RULES`/`LOGICAL_RULE_FLAGS` 保留相同设计和相同执行顺序。尤其需要区分“声明位号顺序”和“执行顺序”：四个全文索引标志在 Go 常量块末尾追加以稳定旧数值，但其规则被插入优化流水线中的语义位置；Rust 完整复刻了这种映射。

初始化方式存在语言层差异：Go 的 [`rule_init.go`](rule_init.go) 通过包级 `init()` 自动设置三个工具层函数变量；Rust 没有包初始化器，改由优化入口显式、幂等地调用 `rule_init::init()`，并使用 `OnceLock` 注册。本文件只提供其中的谓词下推标志回调；另两个谓词简化回调由 operator 层安装。

迁移状态不是桩：这些位已经被 Rust 计划构建器和优化派发器消费，相关 Rust 测试也覆盖位隔离与实际计划变化。不过，某一具体规则是否完整对齐 Go 属于各规则实现文件的职责，不能仅凭本位掩码文件推断。

## 扩展指南

新增逻辑规则时，安全的最小改动链为：

1. 在本文件现有末尾追加新的 `FLAG_*: u64 = 1 << n`，不要为了匹配执行顺序而移动或重编号旧位；先确认 `u64` 尚有容量。
2. 在 [`../optimizer_runtime.rs`](../optimizer_runtime.rs) 增加 `LogicalRule` 变体，并在 `LOGICAL_RULES` 与 `LOGICAL_RULE_FLAGS` 的相同位置分别插入规则和新标志；位置应由规则依赖决定，不必等于位号顺序。
3. 在计划构建/重写入口中只在确有需要的路径上组合新位，并补充对应的 `match` 实现。若规则之间有先后或重复运行约束，应在实现与测试中明确表达。
4. 同步 [`logical_rules.go`](logical_rules.go)、Go `optRuleList`/`optRuleFlags` 和相关 Go 测试，避免双语言协议漂移。
5. Rust 测试应放在独立测试文件，不要内嵌到本生产文件。至少扩展 [`../optimizer_logical_entry_aster_unit_test.rs`](../optimizer_logical_entry_aster_unit_test.rs) 的位唯一性/派发顺序测试和规则行为用例；若修改初始化钩子，则同步 [`rule_init_test.rs`](rule_init_test.rs)。Go 侧同步 [`../optimizer_test.go`](../optimizer_test.go) 的对齐检查及最接近的行为测试。

主要兼容风险是重编号导致调用方选择错误规则，正确性风险是映射错位或漏映射，性能风险是默认/构建路径误开高成本规则或改变规则先后。若未来超过 64 个规则，不能直接继续移位；应先设计新的掩码表示并审计所有位运算边界。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件已索引。
- RustCodeGraph `node --file pkg/planner/core/rule/logical_rules.rs`：确认文件共 97 行，仅有 34 个公开常量和 `set_predicate_push_down_flag` 一个函数，无条件编译项。
- RustCodeGraph `query set_predicate_push_down_flag`、`callers`、`callees`：定位本函数及 util 层静态槽；无直接静态调用边，唯一被调用符号为 `FLAG_PREDICATE_PUSH_DOWN`，与函数指针动态分派一致。
- RustCodeGraph 对 [`../optimizer_runtime.rs`](../optimizer_runtime.rs) 的文件节点查询：确认 `LogicalRule`、`LOGICAL_RULES`、`LOGICAL_RULE_FLAGS`、`logical_optimize_in_place` 和 `DoOptimize` 的选择与派发链。
- 读取 [`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`rule_init.rs`](rule_init.rs) 和 [`util/misc.rs`](util/misc.rs)：确认 crate 边界、再导出方式及 `OnceLock` 注册生命周期。
- 读取 Go [`logical_rules.go`](logical_rules.go)、[`rule_init.go`](rule_init.go)、[`../optimizer.go`](../optimizer.go) 与 [`../optimizer_test.go`](../optimizer_test.go)：确认位号、回调语义、规则列表映射和 `TestOptRuleListFlagAlignment` 不变量。
- 读取 Rust [`rule_init_test.rs`](rule_init_test.rs)、[`../optimizer_logical_entry_aster_unit_test.rs`](../optimizer_logical_entry_aster_unit_test.rs)、[`../logical_plans_test.rs`](../logical_plans_test.rs) 及 [`../lateral_join_test.rs`](../lateral_join_test.rs)：确认钩子幂等性、未知高位保留、位隔离、Go 顺序派发，以及实际规划行为按组合掩码选择。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证使用任务文件规定的 11 章节结构命令，并人工检查所有本地链接与结论的源码落点。
