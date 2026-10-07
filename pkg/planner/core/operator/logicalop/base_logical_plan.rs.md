# `pkg/planner/core/operator/logicalop/base_logical_plan.rs`

## 文件定位

本文件是 `astersql-planner-core-operator-logicalop` crate 的逻辑计划公共基座。crate 入口 `pkg/planner/core/operator/logicalop/lib.rs` 将 `base_logical_plan` 声明为私有模块，再以 `pub use base_logical_plan::*` 对外重导出，因此 planner、Cascades 规则和各逻辑算子通常通过 `logicalop::LogicalPlan`、`logicalop::BaseLogicalPlan` 等名称使用它。

它位于 SQL 优化阶段的逻辑算子层：`pkg/planner/core/optimizer_runtime.rs` 的规则驱动器对计划根调用 `PredicatePushDownPlan`、`PruneColumns`、`BuildKeyInfo` 等契约；具体的 `LogicalJoin`、`LogicalProjection`、`LogicalSelection` 等节点内嵌 `BaseLogicalPlan` 并按需覆盖默认行为。此文件不负责 SQL 解析、物理计划执行或存储访问，而是统一逻辑计划树的身份、子节点、schema、统计、函数依赖和优化遍历协议。

crate 边界由同目录 `Cargo.toml` 定义，包名为 `astersql-planner-core-operator-logicalop`，库入口为 `lib.rs`，并直接依赖 `base`、`expression`、`fd`、`kv`、`property` 等规划基础 crate；`[package.metadata.porting]` 将 Go 对照包指向 `pkg/planner/core/operator/logicalop`。

## 核心职责

1. 用对象安全的 `LogicalPlan` trait 和 `LogicalPlanRef = Box<dyn LogicalPlan>` 统一异构逻辑算子树，并通过 `as_any`/`as_any_mut` 支持运行时向下转型。
2. 用 `BaseLogicalPlan` 保存所有算子共有的上下文、类型、计划 ID、查询块偏移、子节点、输出 schema/列名、统计、`MaxOneRow`、FD、TiFlash 可用性、任务缓存和标志位。
3. 提供单子节点算子的默认优化行为：谓词向子节点传递、残差谓词包装成 `LogicalSelection`、列裁剪、TopN 挂接、统计继承和递归元数据推导。
4. 提供少量跨算子的判定函数：`HasMaxOneRow`、`CanSelfBeingPushedToCopImpl` 和 `CanPushToCopImpl`。
5. 为 Cascades/物理枚举提供可撤销任务缓存、FD 汇总、子树计划 ID 哈希和重分配支持。

这些默认实现只适合其注释和分支明确覆盖的形状。多子节点统计、算子自身表达式改写、Join/聚合等特有语义必须由具体算子覆盖，不能依赖基类“猜测”。

## 主要符号

- `APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG: u64`：第 0 位标志，供 decorrelate 生成的中间 Apply 及后续规则识别；调用证据见 `pkg/planner/cascades/rule/ruleset/rule_set.rs`。
- `TaskRef { cost, plan_id }`：Rust 当前任务缓存的轻量值，记录估算代价和物理计划 ID。
- `LogicalPlanRef`：拥有所有权的 trait object，形成可变、异构且递归拥有子节点的逻辑计划树。
- `PredicatePushDownPlan`：根/子树谓词下推适配器。调用 `PredicatePushDownRoot`，并在具体算子返回替代根时原地更新传入的 `Box`。
- `AttachSelectionToPlan`：将未被子节点消费的谓词物化为 `LogicalSelection`。空条件或零行 `LogicalTableDual` 直接返回；否则要求子节点具有规划上下文，保存其 schema 和输出名，用临时空 dual 取走旧节点，再构造 Selection 包裹旧节点。
- `LogicalPlan` trait：公开逻辑算子契约。基础访问器委托给 `base/base_mut`；`PredicatePushDown`、`PruneColumns`、`BuildKeyInfo`、`PushDownTopN`、`DeriveStats` 等默认委托给基座，具体算子可覆盖。
- `BaseLogicalPlan`：公共状态容器。私有字段限制外部直接破坏不变量，`Flag` 是当前唯一公开字段。
- `NewBaseLogicalPlan`：从 `base::ContextRef` 分配唯一计划 ID，并记录算子类型和查询块偏移；逻辑算子的 `Init` 方法广泛调用它，例如 `logical_join.rs`、`logical_datasource.rs`、`logical_projection.rs`。
- `Hash64`、`Equals`、`HashCode`：分别将 ID 写入 hasher、按 ID 与类型名比较、生成 ID 的大端字节。注意 Rust 的 `Equals` 比 Go 版本多比较了 `tp`。
- `PredicatePushDown`、`PruneColumns`、`PushDownTopN`：基类的单子节点默认变换；均只处理第一个子节点或恰好一个子节点。
- `BuildKeyInfo`、`RecursiveDeriveStats`、`DeriveStats`：自底向上推导基数相关信息。`DeriveStats` 缓存结果；叶子默认一行且每列 NDV 为 1，单子节点继承，多子节点返回错误。
- `PreparePossibleProperties`、`ExtractFD`：前者缓存“所有给定子节点均有 TiFlash 且列表非空”的结果；后者惰性合并全部子节点 FD 并缓存。
- `GetTask`、`StoreTask`、`RollBackTaskMap`、`GetLogicalTS4TaskMap`：按字符串键维护任务缓存和回滚日志。时间戳使用 `wrapping_add`，回滚恢复被覆盖的旧值或删除新插入值。
- `ReAlloc4Cascades`：为 Cascades 派生节点重设类型并分配新 ID，清空任务缓存、回滚日志、`MaxOneRow` 和 FD；子节点、schema、输出名、统计、哈希及标志位不在此函数中重置。
- `CanSelfBeingPushedToCopImpl`、`CanPushToCopImpl`：按类型字符串和目标存储判断自身/全子树是否可下推。
- `HasMaxOneRow`：根据算子类型字符串和子节点标志推导最多一行性质。

## 执行流程

典型构造流程如下：具体算子的 `Init` 调用 `NewBaseLogicalPlan`；上下文的 `alloc_plan_id()` 生成 ID；算子再设置 schema、输出名与子节点。`LogicalPlan` 默认访问器随后都从该内嵌基座读写共享状态。

谓词下推主链为：

1. `optimizer_runtime.rs` 在 `LogicalRule::PredicatePushDown` 分支以空谓词调用根 `PredicatePushDownPlan`。
2. 适配器调用动态分派的 `PredicatePushDownRoot`；默认实现继续调用该算子的 `PredicatePushDown`，特殊算子可同时返回替代根。
3. 基类 `PredicatePushDown` 只处理第一个子节点：递归调用 `PredicatePushDownPlan`，取得子节点未消费的残差条件。
4. `AttachSelectionToPlan` 保证残差不会丢失：需要时在子节点上方创建 Selection；基类最终向父层返回空残差。
5. Join、Projection、Selection、Aggregation、Window、UnionAll 等具体实现直接复用这两个辅助函数来控制各分支的可下推条件。`optimizer_runtime.rs` 的 decorrelation 路径也用同一组合给 Apply 内侧挂回残差。

统计流程分两层：`RecursiveDeriveStats` 先递归要求所有子节点派生统计，再调用当前基座的 `DeriveStats`。缓存存在且 `reload == false` 时直接返回 `(缓存克隆, false)`；否则叶子构造一行统计、单子节点继承，多子节点明确报错。具体多输入或改变基数的算子必须覆盖 trait 方法，否则会触及这一错误边界。

关键性质流程为：`BuildKeyInfo` 先递归子节点，再用 `HasMaxOneRow(tp, children)` 写入本节点缓存；`ExtractFD` 首次访问时递归合并子节点集合；`PreparePossibleProperties` 接收调用方已计算的子节点 TiFlash 布尔值并缓存合取结果。

任务缓存流程为：调用方取得逻辑时间戳作为回滚点；每次 `StoreTask` 都保存该键的旧值并推进本节点时间戳；`RollBackTaskMap(timestamp)` 从日志尾部弹出所有 `ts > timestamp` 的修改并恢复旧状态。这是单节点、后进先出的回滚，不会在本函数中递归子树。

## 数据与状态

`ctx` 是可选的共享规划上下文。通过 `NewBaseLogicalPlan` 初始化的正常节点持有它；`Default` 和测试桩可能没有上下文。依赖上下文的操作必须处理 `None`：例如 `AttachSelectionToPlan` 会返回 `PlannerError`，而 `ReAlloc4Cascades` 在缺少上下文时保留旧 ID。

`tp`、`id`、`query_block_offset` 构成节点身份和定位信息。`HashCode` 只编码 ID，而 `Equals` 同时比较 ID 与类型；因此两者都依赖 ID 在上下文中的分配约束，不能把默认构造的多个 `id == 0` 节点当成稳定的跨会话身份。

`children: Vec<LogicalPlanRef>` 独占子树；`TakeChildren` 会把向量整体移出并留下空向量。`SetChild` 做边界检查并返回 `PlannerError`，其余按首子节点或精确二子节点读取的辅助方法分别用 `Option`/错误表达形状不满足。

`schema` 与 `output_names` 是当前 Rust 基座自身持有的数据。`AttachSelectionToPlan` 在包裹前克隆/浅拷贝它们，保持替代节点的输出契约。`stats`、`fd_set`、`has_ti_flash`、`max_one_row` 是派生缓存；修改子树、schema 或相关表达式后，调用方必须按对应优化阶段重新派生或显式覆盖，通用 setter 不会联动清空全部缓存。

任务缓存由 `task_map`、`task_map_bak` 和单调包装递增的 `task_map_bak_ts` 组成。日志项同时保存时间戳、键和旧值，使同一键多次覆盖仍能逆序还原。`plan_ids_hash` 由外部设置，当前基座的字符串缓存键不会自动拼接该哈希，调用方需自行构造稳定键。

`Flag` 是位集合；`SetFlag` 只置位，没有清位 API。`APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG` 由规则层识别，新增位必须避免冲突并同步规则测试。

## 依赖与调用关系

上游入口包括：

- `pkg/planner/core/optimizer_runtime.rs`：逻辑规则循环调用 `PredicatePushDownPlan`、trait 上的列裁剪和键推导；decorrelation 代码组合调用 `PredicatePushDownPlan` 与 `AttachSelectionToPlan`。
- `pkg/planner/core/operator/logicalop/*.rs`：各算子 `Init` 调用 `NewBaseLogicalPlan`；Join、Projection、Selection、Aggregation、Window 等复用谓词下推辅助函数；多种算子委托 TiFlash 属性与 FD 推导给基座。
- `pkg/planner/cascades/memo/group_expr.rs`：从逻辑表达式提取 FD，并把子节点 TiFlash 状态汇总给 `PreparePossibleProperties`。
- `pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_simple_apply.rs`：克隆/转换 Apply 为 Join 时调用 `ReAlloc4Cascades`。
- `pkg/planner/cascades/rule/ruleset/rule_set.rs`：读取 decorrelate 标志位以决定规则匹配。

下游依赖包括：

- `base::ContextRef`：分配计划 ID并提供规划上下文。
- `expression::{Column, Expression, Schema}` 与 `types::metadata::NameSlice`：表示谓词、输出列及名称。
- `property::StatsInfo`：保存行数、列 NDV 等统计。
- `fd::FDSet`：合并并缓存函数依赖。
- 同 crate 的 `LogicalSelection`、`LogicalTableDual`：实现残差过滤包裹和临时占位。
- 标准库 `Any`、`HashMap`、`Hasher`：向下转型、任务缓存和节点哈希。

RustCodeGraph 将目标文件标为被 34 个文件使用，并识别 `PredicatePushDownPlan`、`NewBaseLogicalPlan` 等符号；对调用边的源码复核显示，谓词辅助函数的直接 Rust 调用者包括 `logical_join.rs`、`logical_projection.rs`、`logical_selection.rs`、`logical_aggregation.rs`、`logical_window.rs`、`logical_union_all.rs` 和 `optimizer_runtime.rs`。

## 错误处理与边界

公开可失败路径统一返回本 crate 的 `Result<T> = Result<T, PlannerError>`。

- `AttachSelectionToPlan` 在确实需要创建 Selection、但子节点没有 `SCtx` 时返回 `logical child has no plan context`。空条件和零行 dual 会在读取上下文前短路成功。
- `SetChild` 对越界索引返回含索引值的错误，不 panic。
- `DeriveStats` 对两个及以上子节点返回 `multi-child logical operator must implement DeriveStats`，以阻止基类静默给出错误基数。
- 子节点的 `PredicatePushDown`、`PruneColumns` 和 `DeriveStats` 错误都用 `?` 原样向上传播。
- `ExtractFD` 的最终 `expect("initialized above")` 只依赖同一函数刚刚写入的内部不变量；`RollBackTaskMap` 的 `expect("checked above")` 也紧随非空检查。

默认行为的形状边界需要特别注意：谓词下推和列裁剪只访问第一个子节点；`GetChildStatsAndSchema` 要求首子节点及其统计均存在；`GetJoinChildStatsAndSchema` 只接受恰好两个子节点且两侧已有统计。`PushDownTopN` 只有一个子节点时才挂接，但当前实现从基座移走子节点后直接把该子节点挂到传入 TopN 下，调用者应核对这是否是期望的完整算子链。

`CanSelfBeingPushedToCopImpl` 使用类型字符串白名单/黑名单，无法检查 DataSource 的实际访问路径、缓存表、聚合禁止下推标志或 CTE 递归性；它只能表达当前 Rust 文件中的粗粒度判定。未知 TiFlash 类型默认允许，而未知 TiKV 类型默认拒绝，因此不能把返回值解释为完整执行可行性证明。

## 并发与资源生命周期

本文件没有线程、异步任务、锁或通道。逻辑计划通过 `Box` 独占子节点，并以 `&mut self` 串行执行绝大多数变换；类型本身没有声明跨线程共享保证。

`ContextRef` 的具体共享/同步策略由 `base` crate 决定，本文件只克隆引用，不管理其销毁。子树替换通过 `std::mem::replace` 和所有权移动完成：旧 child 移入新 Selection，新 Selection 再写回原槽位，失败发生在移动之前的上下文检查阶段，因此不会因错误丢失原计划。

任务缓存与 FD 缓存都属于节点局部可变状态，没有内部同步。`StoreTask`/`RollBackTaskMap` 必须在同一可变节点访问序列中使用；`wrapping_add` 允许极端情况下时间戳回绕，代码未处理跨回绕的顺序语义。`ReAlloc4Cascades` 清理派生状态但保留子树所有权，适合在外层已完成深拷贝/转换后重设身份，不应被视为通用 reset。

## 与 Go 版本的对应关系

主要 Go 对照是 `pkg/planner/core/operator/logicalop/base_logical_plan.go`，下推判定与 `HasMaxOneRow` 的 Go 实现在同目录 `logical_plans_misc.go`。Rust 保留了公共基座、默认优化契约、任务缓存、FD、TiFlash 属性、标志位和 Cascades 重分配等概念，但当前语义并非完整逐项移植：

- Go 的 `BaseLogicalPlan` 嵌入 `baseimpl.Plan`，schema/输出名默认转发到首子节点，并用 `self base.LogicalPlan` 保留具体动态类型；Rust 将身份、schema、输出名直接存进基座，以 trait object 动态分派，不保存 `self` 指针。
- Go `AddSelection` 会先做谓词化简，并能把恒假/NULL 条件折叠成 `LogicalTableDual`；Rust `AttachSelectionToPlan` 只处理空条件、已有零行 dual 和普通 Selection 包裹。文档使用者不能假设 Rust 已具备 Go 的常量折叠路径。
- Go `DeriveTopN` 受会话变量 `AllowDeriveTopN` 控制，并把每个子节点返回的新根写回；Rust 基座的 `DeriveTopN` 无开关、只递归调用基座方法。两者当前行为边界不同。
- Go `RecursiveDeriveStats` 将列组、全部子统计/schema 和 reload 列表传给动态分派的当前算子；Rust 版本先对子节点调用 trait `DeriveStats`，再调用基座自身 `DeriveStats`。具体算子若需要专有递归语义，应显式覆盖/由外层驱动。
- Go 任务键由 `planIDsHash + PhysicalProperty::HashCode` 生成，回滚是否记录受 statement hint 控制，并递归回滚子节点；Rust API 接收已生成的字符串键，每次都记录旧值，回滚仅限当前节点，且能够恢复被覆盖的旧任务。两边数据结构和时间戳比较条件也不同。
- Go `ExtractFD` 在没有缓存时组合子 FD 但片段中不写回 `p.fdSet`；Rust 会缓存合并结果。子树变化后尤其要注意缓存失效责任。
- Go `CanSelfBeingPushedToCopImpl`/`CanPushToCopImpl` 按具体动态类型、访问路径、缓存表、算子选项和 CTE 状态判断，并对不支持的 MPP 算子发警告；Rust 仅按 `tp` 字符串做简化判定。因此 Rust 当前结果是粗粒度兼容门面，不等同 Go 的完整可下推判定。
- Go `HasMaxOneRow` 对半连接族只要求左子最多一行，普通 Join 才要求两侧；Rust 对所有 `"Join"` 都要求两个子节点同时最多一行，无法从 `tp` 区分 JoinType。
- Go `Hash64`/`Equals` 只允许少数动态类型直接使用并带断言，`Equals` 比较 ID；Rust 无对应动态类型限制，且 `Equals` 比较 ID 与类型名。
- Go `ReAlloc4Cascades` 还重设嵌入 Plan 和 `self`；Rust 重分配 ID/类型并清理部分缓存。两者都保留 children，但其余被保留字段集合不完全相同。

这些差异是从当前源码得到的迁移状态，不代表目标设计。若后续要求与 Go 行为对齐，应逐项补测试和实现，而不应在本基座中用更宽泛的桩逻辑绕过。

## 扩展指南

新增逻辑算子时，应内嵌 `BaseLogicalPlan`，实现 `as_any`、`as_any_mut`、`base`、`base_mut`，并在初始化方法中调用 `NewBaseLogicalPlan`。只有“单子节点且不改变该性质”的算子才可直接采用谓词、列裁剪、统计或 `MaxOneRow` 默认实现；多子节点、复制/过滤/聚合行、改变列映射或持有自身表达式的算子应覆盖对应 trait 方法。

扩展谓词下推时，必须保持“未消费谓词不可丢失”的不变量：对子节点调用 `PredicatePushDownPlan` 后，将残差返回父层或调用 `AttachSelectionToPlan`。若要对齐 Go `AddSelection` 的化简与恒假折叠，应在独立 Rust 测试文件中覆盖空条件、零行 dual、恒真、恒假/NULL、缺少上下文以及替换根等分支。

新增统计逻辑时，需同时考虑缓存命中、`reload`、输出列 UniqueID 到 NDV 的映射、零/一/多子节点形状，并避免直接落入多子节点错误。相关现有测试入口包括 `logical_relational_aster_unit_test.rs`、`logical_table_dual_test.rs` 以及各具体算子的独立 `*_test.rs`；本文件当前没有同名独立测试，若直接扩展基座，建议新建独立测试模块并在 `lib.rs` 的 `#[cfg(test)]` 区登记，遵守生产代码与测试代码分文件约束。

修改任务缓存时，应把键生成、时间戳边界、同键覆盖、嵌套回滚、回滚后再写入和是否递归子树作为一组设计；若目标是 Go 对齐，还要决定 statement hint 开关和 `planIDsHash`/物理属性哈希由哪一层负责。修改 `ReAlloc4Cascades` 时要明确列出应保留与应失效的每个缓存字段。

扩展 Coprocessor 判定或 `MaxOneRow` 时，不宜继续只增加模糊类型字符串：JoinType、DataSource 路径、缓存表、聚合禁用标志和 CTE 递归状态都是 Go 侧真实语义输入。应优先让具体算子暴露/覆盖自身判定，并添加 TiKV/TiFlash、半连接族、普通 Join 和未知算子的表驱动测试。

任何修改都应同步核对 `base_logical_plan.go`、`logical_plans_misc.go` 和直接调用者，且保持 PingCAP 许可证。Rust 生产逻辑修复后按仓库约定保留顶部 `// Copyright 2026 AsterSQL.`；测试继续放在独立文件。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter` 确认目标文件已索引，文件节点报告 667 行、100 个符号并被 34 个文件使用。
- RustCodeGraph 源码/符号查询：读取 `base_logical_plan.rs` 全部 667 行；`query PredicatePushDownPlan --json` 定位到 `base_logical_plan.rs::PredicatePushDownPlan` 第 47 行，`query NewBaseLogicalPlan` 同时定位 Rust 第 266 行与 Go 第 434 行。`callers`/`callees` 查询未产生可用文本，因此调用边又由已索引调用文件和精确源码搜索交叉核验，未据此臆造边。
- 读取的生产文件：`pkg/planner/core/operator/logicalop/base_logical_plan.rs`、同目录 `lib.rs`、`Cargo.toml`、`logical_window.rs` 等直接调用片段，以及 `pkg/planner/core/optimizer_runtime.rs` 的规则入口和 decorrelation 路径。
- Go 对照：`pkg/planner/core/operator/logicalop/base_logical_plan.go`、`pkg/planner/core/operator/logicalop/logical_plans_misc.go`；crate 元数据也明确声明该 Go 包为 porting 来源。
- 测试证据：同名独立 Rust 测试不存在；读取了 `logical_plans_misc_test.rs`（TiFlash 缓存行为）、`logical_relational_aster_unit_test.rs`（trait 动态分派、谓词下推、统计和 `MaxOneRow` 的相关行为），并定位 `logical_schema_producer_test.rs`、`logical_table_dual_test.rs`、`logical_union_all_test.rs`、`logical_cte_test.rs`、Cascades rule/memo 测试等间接覆盖面。Go 侧读取 `logicalop_test/logical_operator_test.go` 的基座克隆/子节点共享测试。
- 本任务只新增说明文档，按计划不运行 Cargo。结构验证要求是目标文件存在且恰好出现十一个规定的二级标题；交付前另行执行该精确命令并记录退出码。
