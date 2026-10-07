# `pkg/planner/core/base/plan_base.rs`

## 文件定位

本文件是 `astersql-planner-core-base` crate 的计划抽象边界。crate 入口
`pkg/planner/core/base/lib.rs` 以私有模块 `mod plan_base` 装入本文件，再通过
`pub use plan_base::*` 对外暴露公共类型。它位于 AST/会话上下文与具体逻辑、物理算子之间：
规划阶段用 `PlanContext` 获取会话与表达式服务，以 `LogicalPlan` 表示可改写的算子树，最终以
`PhysicalPlan` 表示可计价、可下接任务并可编码为 `tipb::Executor` 的执行树。

`pkg/planner/core/base/Cargo.toml` 将该边界声明为独立库 `astersql-planner-core-base`，并直接依赖
`planctx`、`expression`、`property`、`costusage`、`fd`、`kv`、`tipb`、`execdetails`、
`cascades-base` 与任务相关的本 crate 类型。具体算子不在这里实现；例如逻辑算子实现在
`pkg/planner/core/operator/logicalop/`，基础物理实现在
`pkg/planner/core/operator/physicalop/base_physical_plan.rs`。会话、规划器、执行器和 Cascades
相关 crate 均通过 Cargo 路径依赖消费该接口。

源码第 23 行所说“本轮不进行 mod.rs 连线”是历史说明；当前事实以 `lib.rs` 为准，本文件已经由
crate 入口接线并再导出。

## 核心职责

1. 用对象安全的 `PlanContext`/`ContextRef` 抹去具体 infoschema 关联类型，让计划节点能保存
   `Arc<dyn PlanContext>`，同时保留计划 ID、会话变量、表达式上下文、range 构造、PB 构建和
   内置函数用量等服务。
2. 定义所有计划共有的 `Plan` 契约，包括 schema、ID、EXPLAIN、统计、输出名、查询块偏移、
   计划缓存克隆和不可缓存原因。
3. 将计划树分成 `LogicalPlan` 与 `PhysicalPlan` 两条能力边界：前者承载逻辑规则、统计和属性
   推导，后者承载成本、任务装配、索引解析、PB 编码及运行时行数展示。
4. 为 Cascades memo 提供 `GroupExpression` 和 `get_ge_and_logical_op` 的包装/解包协议。
5. 固化跨模块共享的 `JoinType` 判别值及展示文字，并用 `PhysicalJoin` 标识真正的物理 join；
   `PhysicalApply` 被有意排除。
6. 保存并比较逻辑子树可能提供的有序列集合 `PossiblePropertiesInfo`，其稳定哈希和相等语义
   与 Go 版本一致。

本文件主要是接口与小型值逻辑，不负责实现具体优化算法、执行网络请求或存储访问。

## 主要符号

- `RangerContext<'a>`：`planctx::rangerctx::RangerContext<'a>` 的公开别名。
- `StatsLoadWaiter: Send + Sync`：统计异步加载的同步等待边界；
  `SyncWaitStatsLoad(&SessionVars) -> Result<(), String>` 把失败作为字符串返回。
- `PlanContext`：对象安全规划上下文。必须实现 `alloc_plan_id`、EXPLAIN 后缀策略、会话/表达式/
  ranger/PB 上下文与 `BuiltinFunctionUsageInc`；checkpoint、prepared parameter、allocator reset 和
  stats waiter 均有保守默认实现。
- `BuiltinFunctionUsageCounter`：`Mutex<HashMap<String, u64>>` 封装的线程安全计数器；`Inc`
  自增，`Get` 在键不存在时返回 `0`。
- `ContextRef = Arc<dyn PlanContext>`：计划节点共享上下文的统一所有权类型。
- `BuildPBContext`：直接再导出 `planctx::BuildPBContext`。
- `Plan: Any`：公共计划契约；`as_any`/`as_any_mut` 支持 Rust 运行时下转，
  `as_physical_plan` 默认返回 `None`。
- `PhysicalPlan: Plan`：物理树契约。主要入口为 `get_plan_cost_ver1/ver2`、`attach_to_task`、
  `to_pb`、`resolve_indices`、children/setters、克隆、内存估算和 probe 行数换算。
- `LogicalPlan: Plan + cascades_base::HashEquals`：逻辑树契约。包括谓词下推、列裁剪、TopN、
  常量传播、统计/NDV/排序属性/函数依赖推导以及 children/setters。
- `GroupExpression: LogicalPlan`：Cascades 包装接口，额外暴露规则探索位和输入组 schema。
- `get_ge_and_logical_op<T>`：先尝试把普通逻辑计划直接下转为 `T`；若是 group expression，
  则解包 `get_wrapped_logical_plan()` 后再下转；无法识别时返回两个 `None`。
- `JoinType`：`#[repr(i32)]` 的八值枚举，判别值固定为 `0..=7`；提供 outer/semi/inner 分类和
  稳定英文 `Display`。
- `PhysicalJoin`：物理 join 的公共能力，返回 inner child 下标与 `JoinType`。
- `PossiblePropertiesInfo`：`orders: Option<Vec<Vec<Option<Column>>>>` 保存 nil/非 nil 及列顺序，
  `has_tiflash` 是运行时裁剪信号；后者明确不参与 `hash64` 和 `equals`。

本文件没有条件编译项；测试条件编译位于 `lib.rs`，把独立文件 `plan_base_test.rs` 和
`base_test.rs` 接入测试构建。

## 执行流程

典型主链可按接口职责理解为：

1. 计划构造器通过 `PlanContext::alloc_plan_id` 分配节点 ID；直接证据包括
   `operator/baseimpl/plan.rs`、`operator/logicalop/base_logical_plan.rs`、`initialize.rs` 和
   `expression_rewriter.rs`。
2. 逻辑优化在 `Box<dyn LogicalPlan>` 树上进行谓词下推、列裁剪、TopN/常量传播、统计与函数
   依赖推导。各具体算子在 `operator/logicalop/*.rs` 实现该 trait；例如
   `LogicalJoin`、`LogicalSelection`、`LogicalAggregation`、`DataSource` 均有明确实现。
3. `prepare_possible_properties` 自底向上汇集可用排序。当前集中入口
   `pkg/planner/core/property_cols_prune.rs::preparePossibleProperties` 消费该接口，叶子扫描与
   join/aggregation 等实现生成或组合 `PossiblePropertiesInfo`。
4. 物理选择后，通过 `get_plan_cost_ver1` 或 `get_plan_cost_ver2` 计算成本，
   `attach_to_task` 将父算子挂到子任务上。`optimizer_runtime.rs` 调用成本与
   `resolve_indices`；`physical_hash_join.rs`、`physical_limit.rs`、`physical_window.rs` 等具体
   算子实现任务装配。
5. 在执行前调用 `resolve_indices`，把表达式中的列引用解析为行下标；随后 `to_pb` 按
   `StoreType` 为下推部分生成 `tipb::Executor`。例如 `session/runtime/relational_scan.rs` 对
   TiKV 路径调用 `to_pb`，而 reader/join/window 等物理算子递归编码子计划。
6. EXPLAIN/运行时统计路径通过 `get_est_row_count_for_display` 和
   `get_actual_probe_count` 处理 Index Join/Apply 内侧“单次 probe 估算”与“全部 probe 实测”
   的口径差异。

辅助流程中，`expression_rewriter.rs` 通过 `BuiltinFunctionUsageInc` 记录 PB 标量函数签名；
`optimizer_runtime.rs::sync_wait_stats_load_point` 经 `GetStatsLoadWaiter` 等待统计加载；
`logical_plan_builder_runtime.rs` 与 `physicalop/cache_snapshot.rs` 使用 prepared LIMIT 接口。

## 数据与状态

- 计划树状态由具体实现保存，本文件只规定访问与修改协议。`Plan` 暴露 ID、schema、统计、输出
  名、上下文、query block offset 和不可缓存原因；children 修改必须经对应逻辑/物理 setter。
- `ContextRef` 使用 `Arc` 共享，不要求计划节点复制会话上下文。trait 本身未声明 `Send + Sync`，
  因而不能仅凭 `Arc` 推断整个计划树可跨线程发送；只有 `StatsLoadWaiter` 明确要求两者。
- `BuiltinFunctionUsageCounter.counts` 由 `Mutex` 串行保护。锁中毒时 `Inc`/`Get` 都通过
  `PoisonError::into_inner` 继续访问已有 map，而不是 panic；计数是进程内 `u64` 累加，源码未
  实现溢出策略或持久化。
- `JoinType` 的数值是兼容协议的一部分。`#[repr(i32)]` 加显式判别值保证 Rust 布局和 Go
  `iota` 顺序一致，尤其 `FullOuterJoin == 7`；不能重排、插入或复用既有值。
- `PossiblePropertiesInfo.orders` 的三层结构分别表达“整体 nil”“若干候选顺序”和“顺序中的
  nullable 列”。`None` 与 `Some(vec![])` 必须不相等且哈希不同；候选与列的排列顺序参与比较。
- `has_tiflash` 只影响运行时裁剪，不能进入 memo 身份：两份对象仅该字段不同仍相等且哈希相同。

## 依赖与调用关系

下游依赖按职责划分如下：

- `planctx` 提供 `SessionVars`、`ExprContext`、`RangerContext`、`BuildPBContext`；本文件通过
  `PlanContext` 形成对象安全适配层。
- `expression` 提供 schema、列、关联列和表达式；`property` 提供统计、物理属性与任务类型；
  `fd` 提供函数依赖集合。
- `costusage` 提供两代成本结果/选项；`kv::StoreType` 选择下推目标；`tipb::Executor` 是 PB
  编码结果；`execdetails::RuntimeStatsColl` 支撑实际 probe 次数换算。
- `cascades_base::HashEquals/Hasher` 定义 memo 所需稳定身份协议；`Task` 来自同一 base crate 的
  `task_base.rs`。

上游直接证据包括：

- `pkg/planner/core/operator/baseimpl/plan.rs` 和逻辑/物理 operator 文件实现、持有或调用这些
  trait；`base_physical_plan.rs` 明确实现 `Plan` 与 `PhysicalPlan`。
- `pkg/planner/cascades/memo/group_expr.rs` 为 `GroupExpression` 实现 `LogicalPlan`；旧 Cascades
  优化器和 transformation rules 消费 `LogicalPlan`、`PhysicalPlan`、`JoinType` 与属性信息。
- `pkg/planner/core/optimizer_runtime.rs` 调用 `resolve_indices`、成本入口及统计等待器。
- `pkg/session/runtime/planning.rs` 持有 `BuiltinFunctionUsageCounter`，
  `typed_adapter_bridge.rs` 在会话与 typed physical plan 间传递 trait 对象。
- `pkg/executor/builder.rs`、`physical_plan_runtime.rs` 与 MPP coordinator 消费物理计划；
  `statement_ru_plan_walk.rs` 还以 `&dyn Plan` 遍历计划并识别 `FullOuterJoin`。

RustCodeGraph 将本文件标记为被 50 个文件使用。由于 trait 对象是动态分派，图上的
`callers/callees` 对 trait 方法和泛型 `get_ge_and_logical_op` 没有给出完整静态边；上述动态关系
均由具体实现与调用点的仓库搜索补证，不将“无静态边”解释成“未使用”。

## 错误处理与边界

- `PlanContext::prepared_limit_value` 默认返回 `Err("Incorrect arguments to LIMIT")`；只有具体
  context 能提供合法参数值。`prepared_param_index` 默认 `None`，调用方必须处理未绑定参数。
- `GetStatsLoadWaiter` 默认 `None`。`optimizer_runtime.rs` 会在没有 waiter 时跳过等待；有 waiter
  时把 `SyncWaitStatsLoad` 的错误纳入优化错误路径。
- 成本计算、PB 编码、索引解析、物理克隆、谓词下推、列裁剪和统计推导都显式返回 `Result`，
  具体实现不得用成功桩吞掉底层错误。
- children setter 与 `get_child_req_props` 以索引工作，但 trait 不做边界检查；越界行为由实现
  负责。新增实现必须与同类算子既有策略一致。
- `get_ge_and_logical_op` 使用安全的 `downcast_ref`，类型不匹配时返回 `None`，不会 panic；这比
  Go 版本在 group expression 分支中的 `.(T)` 更保守，调用者必须检查第二个返回值。
- Rust 的 `JoinType::fmt` 对所有八个枚举值穷尽匹配，因此没有 Go `String()` 的
  `"unsupported join type"` 分支；安全 Rust 中不能构造未声明枚举值。
- Rust `PossiblePropertiesInfo` 本身不能是 nil，所以 `hash64` 总先写对象 `NotNilFlag`；
  `orders` 和内部列仍完整保留 Go nil 语义。

## 并发与资源生命周期

`ContextRef` 的 `Arc` 让多个计划节点共享同一规划上下文，并以引用计数管理生命周期；计划缓存
克隆和物理克隆可接收新的 `ContextRef`，避免旧会话上下文被隐式沿用。树节点本身使用
`Box<dyn ...>` 表达单一所有权，`set_children`/`set_child` 明确转移子树所有权；只读遍历返回借用，
避免在遍历期释放节点。

本文件没有生成异步任务、通道、事务、文件句柄或网络连接。唯一内部同步原语是
`BuiltinFunctionUsageCounter` 的 `Mutex`；锁作用域局限于单次 map 访问。统计加载的实际等待和
取消行为由 `StatsLoadWaiter` 实现负责，本文件仅规定 `Send + Sync` 及同步返回协议。

PB 构建上下文以 `&mut BuildPBContext` 传入 `to_pb`，保证一次编码链中对构建状态的独占可变访问；
运行时统计以共享引用传入，不在该接口层转移所有权。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/base/plan_base.go`：

- Rust `Plan`、`LogicalPlan`、`PhysicalPlan`、`GroupExpression`、`PhysicalJoin` 的方法集合与 Go
  同名接口保持同一职责；Rust 采用 snake_case，并补充 `Any` 下转与对象安全入口。
- Go `PlanContext` 只是 `planctx.PlanContext` 类型别名；Rust 因关联类型无法直接作为 trait object，
  新建对象安全的 `PlanContext` 边界。这是语言适配，不是删减规划期服务。
- Go 可变参数映射为 Rust 切片或 `Vec`：例如 `TP(...bool)` 对应 `tp(&[bool])`，
  `Attach2Task(...Task)` 对应 `attach_to_task(Vec<Box<dyn Task>>)`。
- Go 指针/接口 nil 由 Rust 的 `Option`、借用、`Box` 或 `Arc` 明确表达。
  `PossiblePropertiesInfo` 的 receiver nil 无 Rust 等价值，但 `Orders == nil` 与 nil column 仍保留。
- `GetGEAndLogicalOp` 的普通计划与 group wrapper 两分支一致；Rust 类型不匹配返回 `None`，而 Go
  group 分支强制类型断言可能 panic。
- `JoinType` 八个数值、分类与展示文字一致。Go `init` 用断言守护数值，Rust用显式判别值和
  `#[repr(i32)]` 固化；独立 Rust 测试再次覆盖 `FullOuterJoin == 7`。
- `PossiblePropertiesInfo::hash64/equals` 与 Go 顺序编码、nil 区分以及忽略 `HasTiFlash` 的语义
  一致。Rust 额外允许内部 `Option<Column>`，并在哈希时对 nil column 写 `NilFlag`，对应 Go
  column 指针的 nil 概念。
- `BuiltinFunctionUsageCounter`、prepared parameter checkpoint 等是 Rust 对象安全上下文为当前
  接线补充的适配能力，不出现在该 Go 文件的接口正文中；其消费者可在 Rust 会话/规划代码定位。

## 扩展指南

- 给公共计划接口新增方法前，先遵守 `doc.go` 的约束：确认它对多数实现者都是真正抽象能力，
  避免依赖具体 operator 造成 crate/import 环，并把方法追加在接口末尾。随后搜索并同步所有实现，
  重点包括 `baseimpl/plan.rs`、`logicalop/*.rs`、`physicalop/*.rs`、Cascades group wrapper 与测试 mock。
- 扩展 `PlanContext` 时优先提供安全默认值，若必须方法不可默认，则同步会话真实实现及大量测试
  context；对 allocator/checkpoint 变更要验证 plan cache 重建与 ID 稳定性。
- 修改逻辑优化能力时，应在相应 `LogicalPlan` 方法及最近的独立测试文件中扩展覆盖，不要把测试
  内嵌到 `plan_base.rs`。属性变更至少同步 `property_cols_prune_test.rs` 和相关逻辑算子测试。
- 修改物理能力时，同步 `BasePhysicalPlan`、特殊 reader/join/window 实现及执行器/MPP 消费点；
  错误传播、task 类型和 `StoreType` 分支是兼容风险，成本或行数换算还可能带来性能/EXPLAIN 漂移。
- 新增 `JoinType` 必须同时更新 Go/Rust 判别值约束、分类方法、`Display`、冲突检测消费者、join
  builder/执行器分支与独立回归测试；禁止在现有值中间插入。
- 修改 `PossiblePropertiesInfo` 身份语义时同时修改 `hash64` 与 `equals`，保证“相等则哈希相同”；
  若字段只是运行时裁剪信号，应像 `has_tiflash` 一样明确排除。同步测试至少包括 nil/空、顺序、
  nullable column 和被排除字段。
- 性能方面，避免在高频 getter 中无谓克隆 schema/统计/children；评估 `BuiltinFunctionUsageCounter`
  的全局 mutex 竞争与逻辑树递归中 `Vec` 分配。兼容方面，保持 trait 对象安全和 Cargo 依赖方向。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；
  `files --filter pkg/planner/core/base` 确认目标源、crate 入口与独立测试均被索引；
  `node --file pkg/planner/core/base/plan_base.rs --offset 1 --limit 520` 完整返回 514 行源码，并显示
  该文件被 50 个文件使用。
- RustCodeGraph 精确查询：`query get_ge_and_logical_op --json` 定位函数至第 366 行；
  `query prepared_limit_value --json` 同时定位 trait 默认方法与会话实现；
  `query get_est_row_count_for_display --json` 定位 trait 方法及基础物理实现。对动态 trait 调用执行
  callers/callees 未返回完整边，故使用实现/调用点搜索补充，而未臆造静态调用图。
- 已读生产与边界文件：`plan_base.rs`、`lib.rs`、`Cargo.toml`、`doc.go`、Go 对照
  `plan_base.go`；已搜索 `operator/logicalop`、`operator/physicalop`、`optimizer_runtime.rs`、
  `property_cols_prune.rs`、Cascades、session 与 executor 中的直接实现和调用点。
- 已读独立 Rust 测试 `plan_base_test.rs`：覆盖 `FullOuterJoin` 判别值/outer 分类/显示文字，以及
  `PossiblePropertiesInfo { orders: None }` 的对象非 nil + orders nil 哈希前缀。
  `base_test.rs` 的仓库搜索结果还显示其覆盖对象安全 context、nil/空属性和忽略 `has_tiflash`；
  Go 同目录没有独立 `plan_base_test.go`，因此 Go 行为依据来自生产对照文件本身。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验证应确认文件存在并且固定的十一个二级标题
  各出现一次；人工复核还需确认没有建议把 Rust 单元测试放回生产源文件。
