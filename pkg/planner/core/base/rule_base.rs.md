# `pkg/planner/core/base/rule_base.rs`

## 文件定位

该文件位于 `astersql-planner-core-base` crate 的抽象边界层，只定义逻辑优化规则的公共契约 `LogicalOptRule`，不包含任何具体改写算法。crate 入口 `pkg/planner/core/base/lib.rs` 以私有模块 `mod rule_base` 纳入本文件，再通过 `pub use rule_base::*` 向依赖该 crate 的规划器组件公开 trait。

`pkg/planner/core/base/Cargo.toml` 声明 crate 名为 `astersql-planner-core-base`，并以同目录 `lib.rs` 为库入口；本接口直接使用该 crate 的 `LogicalPlan` 和 `Error`，以及 `tokio-util` 的 `CancellationToken`。同目录 `doc.go` / `doc.rs` 给出的 base 层约束同样适用于此 trait：公共接口应保持抽象和精简，不应依赖具体规则类型或反向引入 core 实现，以免形成依赖环。

需要特别区分契约与现状：RustCodeGraph 能定位本文件及 `LogicalOptRule`，但没有返回 Rust 调用者或被调用者；仓库搜索也未发现 `impl LogicalOptRule`、`dyn LogicalOptRule` 或 Rust 侧直接导入该 trait。当前 Rust 生产优化流水线实际由 `pkg/planner/core/optimizer_runtime.rs::logical_optimize_in_place` 按 `LogicalRule` 枚举分派。因此本文件是已公开但尚未接入当前 Rust 优化主链的 Go 对齐接口，而不是现行规则调度器。

## 核心职责

`LogicalOptRule` 把一条逻辑优化规则压缩为两个能力：

1. `optimize` 接收取消信号和一棵拥有所有权的逻辑计划树，返回优化后的树、是否发生需要上层关注的变化，以及可能的表达式错误。
2. `name` 返回稳定的规则名称，供编排、禁用配置、诊断或跟踪使用。

该抽象让调用方理论上可以用统一的 trait object 编排去关联、谓词下推、列裁剪等异构规则，而无需知道具体规则类型。它只规定一次规则调用的边界，不规定规则顺序、启用位、重复执行、交互规则选择或统计刷新；这些属于优化器编排层职责。当前 Rust 编排层的真实实现可见 `pkg/planner/core/optimizer_runtime.rs::{LOGICAL_RULES, LOGICAL_RULE_FLAGS, logical_optimize_in_place}`。

## 主要符号

- `pub trait LogicalOptRule`：文件内唯一的顶层类型，也是唯一公开 API。trait 没有泛型方法或返回 `Self`，签名可用于 `dyn LogicalOptRule`；但它没有 `Send`、`Sync` 或 `'static` 超 trait 约束，不能仅凭本接口推断实现可跨线程共享。
- `LogicalOptRule::optimize(&self, ctx: &CancellationToken, plan: Box<dyn LogicalPlan>) -> Result<(Box<dyn LogicalPlan>, bool), Error>`：消费计划根的 `Box`，成功时归还新的或等价的计划根与 `changed` 标志，失败时返回 crate 的统一错误类型。
- `LogicalOptRule::name(&self) -> &str`：借用规则自身所持有的名称，不分配新字符串；调用方不得假定返回值一定是字符串字面量，但应把它当作规则生命周期内稳定的标识。
- `crate::LogicalPlan`：定义于 `pkg/planner/core/base/plan_base.rs`，继承 `Plan` 与 `cascades_base::HashEquals`，提供谓词下推、列裁剪、统计推导、子节点替换等逻辑计划操作。
- `crate::Error`：在 `pkg/planner/core/base/lib.rs` 中是 `expression::Error` 的类型别名，因此规则错误与表达式/规划错误使用同一错误通道。

本文件没有模块常量、结构体、枚举、自由函数、`impl` 块或条件编译项。

## 执行流程

若未来由 Rust 编排器接入该 trait，一次调用按接口可观察到的流程是：

1. 上层持有一个具体规则实现、借用的 `CancellationToken` 和 `Box<dyn LogicalPlan>` 根节点。
2. 上层调用 `optimize`，将计划所有权移入规则；规则可以原地改写、递归处理子树，或构造并返回替代根。
3. 规则在成功路径返回 `(plan, changed)`。`changed` 表示该规则报告的交互变化信号，并不等价于“返回对象地址不同”；Go 注释明确默认值为 `false`，且若为 `true` 可触发后续交互规则。
4. 规则失败时返回 `crate::Error`，调用方应立即停止或按上层策略处理，不能从 `Result::Err` 中取回已移入的计划。
5. 上层可调用 `name` 做规则禁用判断、跟踪或诊断。

上述是 trait 规定的调用模型，不是当前 Rust 主链的已接线事实。当前 `logical_optimize_in_place` 遍历 `LOGICAL_RULES` 与 `LOGICAL_RULE_FLAGS`，直接 `match` 每个枚举成员调用具体函数，并在末尾刷新统计；它没有构造或调用 `LogicalOptRule` trait object。Go 主链 `pkg/planner/core/optimizer.go::{normalizeOptimize, logicalOptimize}` 则真实遍历 `[]base.LogicalOptRule` 并调用 `Optimize`。

## 数据与状态

- 计划状态：`Box<dyn crate::LogicalPlan>` 表示唯一拥有的动态逻辑计划根。所有权传入、成功后再传出，允许实现替换根节点，同时避免接口层暴露具体算子类型。
- 变化状态：返回元组中的 `bool` 是规则显式报告的信号。它是否为 `true` 由实现语义决定，不能通过比较输入输出指针推导。Go 的 `logicalOptimize` 使用该值决定是否收集交互规则；当前 Go 的 `optInteractionRuleList` 初始为空映射。
- 取消状态：`&CancellationToken` 只是共享借用，trait 不取得令牌所有权，也不负责创建、取消或销毁令牌。签名本身不会自动中断计算；具体实现必须主动检查或等待该令牌，调用者也不能仅因传入令牌就假定规则支持及时取消。
- 规则身份：`name` 返回借用字符串，接口不缓存、不全局注册，也不强制唯一性。若上层以名称做配置键，实现者必须保持名称稳定并避免冲突。
- 错误状态：没有局部错误枚举或恢复状态；全部失败统一经 `Result::Err(crate::Error)` 传播。

## 依赖与调用关系

直接下游依赖如下：

- `tokio_util::sync::CancellationToken`：来自 `pkg/planner/core/base/Cargo.toml` 中启用 `rt` feature 的 `tokio-util = 0.7`，为 `optimize` 提供取消/截止协作载体。
- `crate::LogicalPlan`：由 `lib.rs` 再导出的 `plan_base.rs::LogicalPlan`，是规则输入输出的动态计划边界。
- `crate::Error`：`lib.rs` 对 `expression::Error` 的别名，是优化失败的统一错误类型。

公开路径为 `rule_base.rs -> lib.rs::pub use rule_base::* -> astersql_planner_core_base::LogicalOptRule`。多个规划器、执行器和会话 crate 依赖 `astersql-planner-core-base`，但搜索结果只证明它们依赖同一 base crate；没有证据表明它们使用本 trait。

上游调用关系分两类：

- Rust 当前状态：RustCodeGraph 对 `LogicalOptRule` 的 callers/callees 查询没有产生边，`rg` 也没有找到 Rust 实现或直接使用；因此不能声称本 trait 已被 `optimizer_runtime.rs` 调用。
- Go 对照链：`pkg/planner/core/optimizer.go` 的 `optRuleList`、`normalizeRuleList` 和 `logicalRuleList` 保存 `base.LogicalOptRule`；`normalizeOptimize` / `logicalOptimize` 按位标志筛选后调用 `Optimize`，`isLogicalRuleDisabled` 调用 `Name`。

仓库另有 `pkg/planner/core/rule/rule_init.rs::LogicalRule`，其计划类型是该文件自有的 `Plan`，错误类型是 `String`，且没有取消参数；它与本 trait 名义相近但不是实现关系。当前生产流水线又使用 `optimizer_runtime.rs::LogicalRule` 枚举，三者不应混为一谈。

## 错误处理与边界

`optimize` 的唯一失败出口是 `crate::Error`。接口不吞错、不定义重试，也不把部分结果与错误同时返回；具体规则应在发现无效计划、表达式处理失败或下游逻辑错误时立即返回 `Err`，由编排层决定是否终止整个逻辑优化。Go 对照的第三个返回值 `error` 被 Rust 的 `Result` 外层编码，成功值只保留计划与变化标志。

边界条件包括：

- 接口没有空计划表示法；调用者必须提供有效的 `Box<dyn LogicalPlan>`。
- `changed == false` 不表示计划必然逐字节不变。已有 Rust 独立规则测试（例如 `rule_column_pruning_test.rs`）明确覆盖“发生规则处理但按 Go 契约返回 false”的情况，因此上层不能把该标志当作一般变更检测器。
- 接口没有规定 panic 策略、回滚协议或失败后计划可恢复性。计划已经按值移入，若实现先修改再报错，上层无法通过此签名取回原根；需要原子语义的实现应自行延迟提交或预先克隆所需状态。
- `name` 没有错误通道，必须始终返回有效借用字符串。
- `CancellationToken` 没有在 trait 中绑定具体检查点；忽略它在类型上仍然合法，但会削弱取消语义，应由实现测试约束。

## 并发与资源生命周期

本 trait 的方法都只借用 `&self`，允许实现内部使用不可变状态或内部可变性；不过 trait 不要求 `Send + Sync`，所以不能默认把 `Box<dyn LogicalOptRule>` 送入其他线程或在多线程间共享。接口也没有异步方法，不会自行启动 Tokio 任务、线程、通道或存储 I/O。

`plan` 的所有权在调用 `optimize` 时从调用方转移给实现，成功后随返回值转回调用方；未被返回的旧节点按 Rust 所有权规则释放。`ctx` 仅在调用期间借用，规则不得在没有额外所有权安排的情况下把该引用留到返回之后。取消令牌自身通常可克隆并共享，但本签名只提供借用，是否派生子令牌或克隆应由实现明确决定。

该文件没有锁、静态可变状态、事务、文件句柄或连接池。任何规则实现若引入缓存、锁或后台工作，都必须在实现所在的独立文件中说明其同步与清理协议，不能把这些生命周期责任归给本接口。

## 与 Go 版本的对应关系

Rust `LogicalOptRule` 直接对应 `pkg/planner/core/base/rule_base.go::LogicalOptRule`：

| Go | Rust | 语义 |
| --- | --- | --- |
| `Optimize(context.Context, LogicalPlan) (LogicalPlan, bool, error)` | `optimize(&CancellationToken, Box<dyn LogicalPlan>) -> Result<(Box<dyn LogicalPlan>, bool), Error>` | 计划与变化标志进入成功值，Go `error` 映射为 Rust `Result::Err` |
| `Name() string` | `name(&self) -> &str` | 返回规则稳定名称；Rust 使用借用避免分配 |
| Go interface 值 | Rust trait / 潜在的 `dyn LogicalOptRule` | 都用于隐藏具体规则类型 |

两侧并非完全等价。Go `context.Context` 可携带取消、截止时间和值；Rust 参数只有 `CancellationToken`，没有通用 context value 或独立 deadline API 的同等契约。Go 返回接口值可为 `nil`，Rust 的 `Box<dyn LogicalPlan>` 在安全代码中非空。Go 方法名导出且使用大写，Rust 遵循 snake_case。

更关键的迁移差异是接线状态：Go 的 `optimizer.go` 已用该接口维护规则列表、调用 `Optimize` / `Name` 并处理交互规则；Rust 当前生产编排使用枚举和直接函数分派，具体规则 crate 中的若干实现则实现另一套 `rule_init.rs::LogicalRule`。因此 Rust trait 当前保留的是目标抽象和类型契约，不能把 Go 调度行为视作 Rust 已实现行为。

## 扩展指南

新增或修改规则能力时应遵循以下顺序：

1. 若只是某个规则的私有能力，不要扩展 `LogicalOptRule`；在具体规则模块中实现即可。只有多数规则和编排器都需要的抽象操作才适合加入 base trait，这与 `doc.go` / `doc.rs` 的接口约束一致。
2. 若新增 trait 方法，需在 `rule_base.rs` 末尾追加，并同步所有实现者、对象安全检查以及 Go `rule_base.go` 的语义对照；还要检查是否会迫使 base crate 依赖具体 core/rule 类型并形成循环依赖。
3. 若要真正接入当前 Rust 主链，不能只实现本 trait。必须先决定是把 `optimizer_runtime.rs::{LogicalRule, LOGICAL_RULES, logical_optimize_in_place}` 迁移为 trait-object 编排，还是增加明确适配层；同时保持 `LOGICAL_RULE_FLAGS` 的顺序、禁用规则名称、交互规则和统计刷新语义。
4. 规则实现应明确何时检查 `CancellationToken`、`changed` 的精确定义、失败后是否可能留下部分修改，以及 `name` 的稳定值。性能上应避免仅为判断变化深拷贝整棵计划树。
5. 测试必须放在独立 Rust 测试文件，不能内嵌到 `rule_base.rs`。当前同目录没有 `rule_base_test.rs`；若为接口新增契约测试，建议新建同目录 `rule_base_test.rs` 并由 `lib.rs` 的 `#[cfg(test)] #[path = "rule_base_test.rs"] mod rule_base_test;` 接入。具体规则行为继续放在 `pkg/planner/core/rule/*_test.rs`，优化顺序与生产接线则在 `pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs` 等编排测试中覆盖。

兼容风险主要是 trait 方法变化导致所有实现编译失败、对象安全被破坏、错误类型或计划所有权语义改变；行为风险主要是 `changed` 误报使交互规则漏跑或重复执行、规则名称漂移使禁用配置失效；性能风险主要来自计划克隆、重复遍历和过密取消检查。

## 验证依据

- RustCodeGraph `status`：项目索引可用，包含 11,467 个文件；`files --filter pkg/planner/core/base/rule_base.rs` 识别目标文件及 4 个图节点。
- RustCodeGraph `node --file pkg/planner/core/base/rule_base.rs --offset 1 --limit 240`：核对文件全貌、`LogicalOptRule`、`optimize` 与 `name` 的实际签名。
- RustCodeGraph `query LogicalOptRule --json`：定位 Go interface 与 Rust trait 两个定义；对 Rust qualified name 的 callers/callees 查询未返回调用边。
- RustCodeGraph `node --file pkg/planner/core/base/plan_base.rs --offset 210 --limit 150`：核对 `LogicalPlan` 的继承关系、计划改写能力和错误边界。
- 源码与配置：`pkg/planner/core/base/{rule_base.rs,lib.rs,doc.rs,doc.go,Cargo.toml,rule_base.go}`，用于核对公开路径、crate 边界、设计约束、依赖和 Go 签名。
- 当前 Rust 编排证据：`pkg/planner/core/optimizer_runtime.rs::{LogicalRule, LOGICAL_RULES, LOGICAL_RULE_FLAGS, logical_optimize_in_place}`；替代规则抽象证据：`pkg/planner/core/rule/rule_init.rs::LogicalRule`。
- Go 调用链证据：`pkg/planner/core/optimizer.go::{optRuleList, normalizeOptimize, logicalOptimize, isLogicalRuleDisabled}`；相关 Go 回归入口包括 `pkg/planner/core/logical_plans_test.go` 与 `pkg/planner/core/optimizer_test.go`。
- Rust 独立规则测试证据：`pkg/planner/core/rule/rule_column_pruning_test.rs`、`rule_partition_processor_test.rs` 及同目录其他 `*_test.rs`；同目录 base 测试未引用 `LogicalOptRule`，仓库内也未发现专门的 `rule_base_test.rs`。
- 仓库搜索：`rg` 未发现 Rust 的 `impl LogicalOptRule`、`dyn LogicalOptRule` 或直接导入；该负面证据限定为本次检出的工作树状态。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 章节结构检查和人工事实复核为验证手段。
