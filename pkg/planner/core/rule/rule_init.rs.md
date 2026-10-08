# `pkg/planner/core/rule/rule_init.rs`

## 文件定位

源码入口为 [`rule_init.rs`](./rule_init.rs)。该文件属于 Cargo crate `astersql-planner-core-rule`，crate 根为同目录的 `lib.rs`，并由其中的 `pub mod rule_init` 公开。`Cargo.toml` 的 `package.metadata.porting.go-package` 将整个 crate 对应到 Go 包 `pkg/planner/core/rule`；本文件直接对应 `rule_init.go`，但还承载了 Rust 规则实现共用的一套精简表达式/逻辑计划 IR。

它有两个需要严格区分的角色：

1. `init()` 是生产优化链上的初始化桥。`pkg/planner/core/optimizer_runtime.rs::logical_optimize_in_place` 在遍历真实的 `LOGICAL_RULES` 前调用它，为 `rule/util` 安装谓词简化和谓词下推标志回调。
2. `Value`、`Expr`、`Plan`、`LogicalRule` 等是同 crate 若干已移植规则及独立测试使用的精简 IR。生产主优化器在 `optimizer_runtime.rs` 中实际处理的是 `logicalop::LogicalPlanRef`，因此不能把这里的 `Plan` 当作完整 TiDB/AsterSQL 逻辑计划表示。

目标目录没有 `doc.go`；包级 Go 语义由同路径 `rule_init.go` 及实际调用点核对。

## 核心职责

- 用显式且可重复调用的 `init()` 模拟 Go 包加载时的 `init()`：安装普通谓词简化、Join 谓词简化以及“打开谓词下推位”三类钩子。
- 定义规则侧精简值与表达式模型：`Value`、`FieldType`、`Expr`，并提供列依赖、结果类型和确定性分析。
- 定义精简计划模型：Join、聚合、分区、数据源、投影、选择等节点数据，以及 schema、谓词、键、估算行数和已用统计摘要。
- 通过 `LogicalRule` 统一同目录规则的名称和一次变换契约；成功结果同时返回新计划与“是否发生修改”的布尔值，失败以 `String` 传播。
- 提供 `default_rule_names()` 这一名称顺序清单。仓库搜索未发现其调用者；真实生产调度顺序由 `optimizer_runtime.rs` 的 `LOGICAL_RULES` 与 `LOGICAL_RULE_FLAGS` 决定，故该函数目前只是一份未接线的精简规则名列表。

## 主要符号

### 初始化与规则契约

- `pub fn init()`：调用 `astersql_planner_core_operator_logicalop::InstallPredicateSimplificationPassthrough()`，后者一次性注册普通与 Join 两个谓词简化回调；再调用 `RegisterSetPredicatePushDownFlag(crate::set_predicate_push_down_flag)` 注册第三个回调。两个注册入口都忽略“是否首次注册”的返回值，因此重复调用无副作用。
- `pub trait LogicalRule`：`name() -> &'static str` 返回稳定名称；`optimize(Plan) -> Result<(Plan, bool), String>` 消费计划、返回变换后的所有权和修改标志。
- `pub fn default_rule_names() -> Vec<&'static str>`：返回 14 个精简规则名。它不是生产 `LOGICAL_RULES` 的来源，且当前没有仓库内调用边。

### 表达式和值

- `Value`：覆盖 `Null`、布尔、有符号/无符号整数、浮点和文本；它是规则 IR 的轻量值，不等同于完整 Go `Datum`。
- `FieldType`：覆盖整数、浮点、十进制、文本（含字符集与排序规则）、日期时间和布尔类型。
- `Expr`：支持列、常量、标量函数和显式 Cast。
- `Expr::columns()`：递归收集列 ID 到 `BTreeSet`，天然去重并保持确定顺序；常量返回空集。
- `Expr::field_type()`：列、标量函数和 Cast 返回类型引用；`Constant` 因不携带独立类型信息返回 `None`。
- `Expr::deterministic()`：将函数名恰为小写 `rand`、`uuid`、`now` 的标量函数视为非确定性，并递归检查其参数；Cast 透传内部表达式，其余变体为确定性。这里是精简规则，不负责大小写归一化或完整易变函数目录。

### 计划、分区与统计

- `JoinType`、`AggKind`、`AggregateExpr`：分别描述连接种类、聚合种类和聚合参数/Distinct 标志。
- `PartitionKind`、`PartitionDefinition`、`PartitionInfo`：表达 Hash/Key/Range/List 分区、分区 ID/名称/边界值，以及分区列与定义列表。
- `PlanKind`：精简逻辑算子枚举。`DataSource` 保存表、索引、分区元数据与已选分区序号；其他变体保存各自必要数据。
- `Plan`：节点主体，集中保存 `kind`、输出 `schema`、子树、谓词、候选键、估算行数和 `used_stats`。
- `UsedStats`：以表 ID 为中心记录已用列、索引、是否全量加载、是否伪统计及版本。
- `Plan::walk_mut()`：对子节点递归后再访问当前节点，是后序可变遍历；回调可修改节点，但无法通过该 API 提前终止或返回错误。
- `Plan::all_columns()`：把当前节点的 `schema` 转为去重有序集合，不递归子树。

## 执行流程

### 生产初始化链

1. `optimizer_runtime.rs::logical_optimize_in_place(flag, plan)` 准备 CTE 与统计相关桥接后，在执行逻辑规则循环之前调用 `rule::rule_init::init()`。
2. `init()` 先进入 `logicalop::InstallPredicateSimplificationPassthrough()`。该函数向 `rule/util` 的两个 `OnceLock` 注册普通谓词简化与 Join 谓词简化闭包；普通路径执行常量传播（按参数决定）、CNF 拆分、常量折叠、真条件删除、矛盾等式检测和吸收化简，Join 路径在需要时调用 Join 专用常量传播，再复用普通简化。
3. `init()` 再把 `logical_rules.rs::set_predicate_push_down_flag` 注册到第三个 `OnceLock`。回调通过 `flags | FLAG_PREDICATE_PUSH_DOWN` 保留已有位并打开谓词下推位。
4. 主优化器按 `LOGICAL_RULES`/`LOGICAL_RULE_FLAGS` 的 zip 顺序检查 flag 并作用于真实 `LogicalPlanRef`。这里的 `default_rule_names()` 不参与该循环。
5. 后续逻辑算子通过 `rule_util::ApplyPredicateSimplification*` 或 `SetPredicatePushDownFlag` 调用已注册回调；若初始化未发生，`rule/util/misc.rs` 中的包装函数会以 `expect(...)` panic，说明初始化是调用顺序不变量。

### 精简 IR 的规则执行链

同目录规则（例如 `ColumnPruner`、`BuildKeySolver`、`JoinKeyTypeCast`、`PredicateSimplification`、`PartitionProcessor`）实现 `LogicalRule`，接收本文件的 `Plan`。规则根据 `PlanKind` 分派，借助 `Expr::columns()`、`field_type()`、`deterministic()` 或计划字段递归改写，并用 `(Plan, bool)` 报告结果。调用者必须自行选择规则及顺序；本文件没有构建或执行一条由 `default_rule_names()` 驱动的流水线。

## 数据与状态

- `init()` 自身没有可变静态变量；全局状态位于 `astersql-planner-core-rule-util` 的三个 `OnceLock`。注册是进程级、只写一次且没有注销/替换接口。
- 精简 IR 全部按值拥有数据：`Plan.children` 形成树，`Expr::Cast` 用 `Box` 打破递归大小，字符串与容器均由节点拥有。
- 列、键、已用列和索引使用 `BTreeSet`，索引表和统计表使用 `BTreeMap`，使遍历顺序稳定；代码没有声明这些顺序具有 SQL 语义。
- `DataSource.selected_partitions` 用分区定义的 `usize` 序号集合，而 `PartitionDefinition.id` 是独立的 `i64` 标识，扩展时不能混用。
- `estimated_rows` 是普通 `f64`，本文件不验证非负、有限或非 NaN；`schema`、`keys`、分区边界等结构一致性也由构造者/规则维护。
- 所有类型均未使用条件编译；只有 crate 根对独立测试模块使用 `#[cfg(test)]`。

## 依赖与调用关系

### 上游

- 生产调用者：`pkg/planner/core/optimizer_runtime.rs::logical_optimize_in_place` 调用 `rule::rule_init::init()`。
- 初始化回归测试：`pkg/planner/core/rule/rule_init_test.rs::init_registers_all_go_rule_hooks_idempotently` 连续调用两次 `init()`。
- 精简 IR 使用者：同目录的列裁剪、键构建、常量传播、Join 键类型转换、MAX/MIN 消除、Join 重排、外连接转半连接、分区处理、谓词简化、计划统计收集等 Rust 文件及其独立测试导入本文件类型或 trait。

### 下游

- `init()` 依赖 crate `astersql-planner-core-operator-logicalop` 的 `InstallPredicateSimplificationPassthrough`，并依赖 `astersql-planner-core-rule-util` 的 `RegisterSetPredicatePushDownFlag`；两者均是 `Cargo.toml` 的普通路径依赖。
- 标志回调指向 crate 根再导出的 `logical_rules.rs::set_predicate_push_down_flag`。
- `Expr` 和 `Plan` 的辅助方法只依赖标准库 `BTreeMap`/`BTreeSet`；其余 IR 类型没有外部 crate 数据成员。

RustCodeGraph 将本文件识别为包含 68 个符号并被 107 个文件使用；精确搜索进一步确认 `init()` 的生产调用点只有 `optimizer_runtime.rs`，测试调用点为 `rule_init_test.rs`，而 `default_rule_names()` 仅有定义。这类“文件被使用”统计包含类型引用，不应等同于每个符号都有 107 个直接调用者。

## 错误处理与边界

- `init()` 不返回错误。`OnceLock::set` 的布尔结果被丢弃，使重复初始化被明确视为成功的幂等操作；这也意味着若槽位已被其他实现占用，本函数不会覆盖它，也不会报告冲突。
- `rule_util::ApplyPredicateSimplification`、`ApplyPredicateSimplificationForJoin`、`SetPredicatePushDownFlag` 在对应钩子未注册时 panic。安全调用边界是：必须先经过 `logical_optimize_in_place` 或显式调用 `rule_init::init()`。
- `LogicalRule::optimize` 的错误是无结构的 `String`，本文件不定义错误分类、回滚或部分修改语义；具体规则必须保证返回 `Err` 时的约定。
- `Expr::field_type()` 对常量返回 `None`，使用者必须显式处理；例如 Join 键类型改写通过 `?` 放弃无法取得类型的候选。
- `Expr::deterministic()` 仅识别三个精确小写名称，是精简模型的已知边界，不能外推为生产表达式系统的完整确定性判定。
- `walk_mut()` 递归深度与计划树深度相同，没有迭代化、深度限制、错误返回或提前停止机制；极深的人造树可能造成栈压力。
- `default_rule_names()` 与生产规则枚举/flag 没有静态一致性检查，新增或重排生产规则不会自动更新此列表。

## 并发与资源生命周期

- 三个初始化槽位使用标准库 `OnceLock`，可在多线程竞争下只接受一次写入；`init()` 忽略后续注册失败，因此多次及并发调用保持已安装值。
- 回调类型是普通 `fn` 指针或无捕获闭包可转换出的函数指针，不持有请求级上下文；运行时上下文、谓词和 schema 在每次调用时传入。
- 注册状态持续到进程退出，没有释放资源、后台任务、通道、锁守卫或显式 shutdown 流程。
- `Plan`/`Expr` 本身没有内部同步、共享引用计数或异步生命周期；规则通常独占消费或可变借用计划树，并发隔离由上层所有权安排。
- `walk_mut()` 在访问父节点前结束对子节点的可变借用，符合后序处理“父节点观察已处理子节点”的意图。

## 与 Go 版本的对应关系

- Go `rule_init.go::init()` 在包加载时直接把 `applyPredicateSimplification`、`applyPredicateSimplificationForJoin`、`setPredicatePushDownFlag` 赋给 `rule/util` 的三个函数变量。
- Rust 没有依赖 Go 式包初始化副作用，改为由 `logical_optimize_in_place` 显式调用 `rule_init::init()`。其中两个谓词简化钩子的具体注册被集中到 `logicalop::InstallPredicateSimplificationPassthrough()`，第三个仍从 rule crate 注册；最终仍满足 `RuleInitHooksRegistered()` 对三槽齐备的检查。
- Go 使用可重新赋值的包变量；Rust 使用 `OnceLock`，因此生命周期更严格：只允许首次注册、不可覆盖，调用前未注册则明确 panic。
- Go 文件只负责 3 个回调接线，不包含本 Rust 文件的精简 `Value`/`Expr`/`Plan`/`LogicalRule`。这些类型是 Rust 移植层为同目录规则与测试增加的本地抽象，不应声称为 Go `rule_init.go` 的逐项翻译。
- Go 同路径没有 `rule_init_test.go`。Rust 独立测试补充验证注册完整性、幂等性和谓词下推位的 OR 语义；其他规则测试间接覆盖精简 IR。

## 扩展指南

- 新增 Go 初始化钩子的 Rust 对应物时，应同时修改 `rule_init::init()`、`rule/util` 的钩子类型/`OnceLock`/注册与调用 API，并扩展 `RuleInitHooksRegistered()` 和独立的 `rule_init_test.rs`。若钩子实现在 operator crate，还需保持 Cargo 依赖方向，避免让 `rule/util` 反向依赖 rule crate。
- 新增精简表达式变体时，必须审查 `Expr::columns()`、`field_type()`、`deterministic()` 的穷举语义，并同步依赖这些不变量的规则与独立测试；非确定函数扩展尤其要验证大小写、别名和嵌套参数。
- 新增 `PlanKind` 时，应审查所有匹配该枚举的规则，明确 schema、谓词、子节点、键和统计传播；测试继续放在同目录独立 `*_test.rs`，不要内嵌到生产源文件。
- 修改 `Plan::walk_mut()` 时要保留或明确改变后序顺序，因为键构建、统计汇总一类父节点逻辑可能依赖已处理子节点；若需要错误传播或提前停止，宜新增返回 `Result`/控制流的遍历 API，避免破坏现有简单回调。
- 新增生产规则应修改 `optimizer_runtime.rs` 的真实规则枚举、顺序和 flag 映射，并按需更新 `default_rule_names()`；在后者被真正接线前，不应仅修改名称列表就声称规则已加入优化流水线。
- 兼容性风险集中在初始化时序、规则顺序、flag 位保留以及 IR 枚举匹配；性能风险集中在表达式递归集合分配和计划树递归遍历。扩展后应优先增加针对这些边界的独立回归测试。

## 验证依据

- RustCodeGraph 索引状态：项目含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/core/rule/rule_init.rs` 确认目标已索引，`node --file ...` 显示 272 行、68 个符号及文件级使用关系。
- RustCodeGraph 符号查询：`query default_rule_names --kind function`、`query LogicalRule --kind trait --json`；`callers/callees` 对这些 Rust 符号未返回边，因此又用精确仓库搜索核对实际引用，未把图的空结果解释为不存在行为。
- 目标源码：`pkg/planner/core/rule/rule_init.rs`，核对 `init`、全部枚举/结构、三个 `Expr` 方法、两个 `Plan` 方法、`LogicalRule` 与 `default_rule_names`。
- crate 边界：`pkg/planner/core/rule/Cargo.toml` 与 `pkg/planner/core/rule/lib.rs`，核对 crate 名、Go 包映射、路径依赖、公开模块及独立测试装配。
- 生产入口与下游：`pkg/planner/core/optimizer_runtime.rs::logical_optimize_in_place`、`pkg/planner/core/operator/logicalop/logical_datasource.rs::InstallPredicateSimplificationPassthrough`、`pkg/planner/core/rule/util/misc.rs` 的三个 `OnceLock`/注册/调用函数、`pkg/planner/core/rule/logical_rules.rs::set_predicate_push_down_flag`。
- Go 对照：`pkg/planner/core/rule/rule_init.go::init`，核对三项函数变量赋值与包依赖方向说明。
- 独立 Rust 测试：`pkg/planner/core/rule/rule_init_test.rs::init_registers_all_go_rule_hooks_idempotently`；同目录其他规则的 `*_test.rs` 证明精简 IR 和 `LogicalRule` 是共享测试表面。未发现同路径 Go 初始化专用测试。
- 本任务为纯文档分析，按任务约束未运行 Cargo；交付前以任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核文档区分了生产 `LogicalPlanRef` 与精简 `Plan`、真实调度表与未接线名称列表。
