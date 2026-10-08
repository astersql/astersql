# `pkg/planner/funcdep/fd_graph.rs`

## 文件定位

`fd_graph.rs` 是 `astersql-planner-funcdep` crate 的函数依赖（Functional Dependency，FD）核心实现。crate 入口 `pkg/planner/funcdep/lib.rs` 以 `funcdep` 模块加载并重新导出本文件；`pkg/planner/funcdep/Cargo.toml` 表明其生产依赖只有 `astersql-util-intset`，列集合统一由 `FastIntSet` 表示。

它位于逻辑优化阶段的属性推导层，而不是表达式求值或物理执行层。逻辑算子把列的 unique id、常量约束、等值条件、非空信息和分组信息写入 `FDSet`；后续算子再合并、投影或查询这些信息。已接线的 Rust 调用点包括 `base_logical_plan.rs` 的子节点合并、`logical_projection.rs` 的重命名/非空/投影、`logical_selection.rs` 的非空提升与投影、`logical_aggregation.rs` 的分组依赖、`logical_join.rs` 的合并与裁剪，以及 `logical_union_all.rs` 的公共等价类求交。

理论背景及 AsterSQL 对 Lax FD、外连接和 Cond-FD 的约定在 `pkg/planner/funcdep/doc.go`。本文件只维护推导状态，不拥有 schema、行数据或执行器资源。

## 核心职责

1. 用 `fdEdge` 表示严格依赖 `X --> Y`、松散依赖 `X ~~> Y`、等价类以及带 null-constraint 的条件依赖。
2. 用 `FDSet` 维护普通可见边、暂时不可见的 Cond-FD、非空列、表达式 hash 到 unique id 的映射，以及聚合相关元数据。
3. 计算严格闭包、Lax 闭包和等价闭包，并据此判断蕴含、约简决定端和寻找候选主键。
4. 在插入常量、等价或普通 FD 时消除平凡边、合并同决定端严格边、删除被更强关系蕴含的边，控制图规模。
5. 对非空推导、笛卡尔积、外连接、最多一行和列投影执行 FD 状态变换。
6. 为 Union All 一类算子计算多个输入共同拥有的等价类（`FindCommonEquivClasses`）。

这些职责都以列 unique id 为身份基础；`FDSet::AddFrom` 的注释和实现均假定不同输入不会错误复用同一个 unique id。

## 主要符号

- `fdEdge`：单条边。`from` 是决定列，`to` 是依赖列；`strict` 区分 Strict/Lax；`equiv` 表示等价边；`conditionNC` 存储 Cond-FD 的 null-constraint。普通调用者不能直接构造它，因为 `fdEdge::new` 是私有函数。
- `fdEdge::implies`：判定一条边是否足以替代另一条。两个 Lax FD 只有依赖端完全相同且当前决定端更小时才构成蕴含；Strict/Equivalence 则同时比较决定端、依赖端和关系强度。
- `FDSet`：图与附属属性的聚合对象。`fdEdges`、`ncEdges` 私有，外部通过方法保持不变量；`NotNullCols`、`HashCodeToUniqueID`、`GroupByCols`、`HasAggBuilt` 是算子推导所需的公开元数据。
- `ClosureOfStrict` / `closureOfStrict`：反复应用严格边；等价边只要与当前结果相交即可扩展，直到集合长度不再变化。
- `ClosureOfLax`：等价关系可以反复扩展，普通 Lax 边按扫描过程应用，再并入输入集合的严格闭包。测试 `lax_dependencies_are_one_step_but_equivalence_extends_the_step` 固化了该语义。
- `ClosureOfEquivalence`：只沿等价边扩展；用于合并等价类、传播非空性和寻找投影替代列。
- `InClosure` / `ReduceCols`：前者判断 `to` 是否属于 `from` 的严格闭包；后者逐列试删决定端中可由剩余列推出的冗余列。
- `AddStrictFunctionalDependency`、`AddLaxFunctionalDependency`、私有 `addFunctionalDependency`：普通 FD 的规范插入路径。
- `AddNCFunctionalDependency`：把尚未满足 null-reject 条件的边放进 `ncEdges`，不立即参与普通闭包。
- `AddEquivalence` / `AddEquivalenceUnion` / 私有 `addEquivalence`：合并等价闭包，并联动常量与非空信息。
- `AddConstants`、`ConstantCols`：维护空决定端的常量边 `{} --> C`，并用常量闭包约简其他边。
- `MakeNotNull` / `MakeNullable`：更新非空列；前者还会激活 Cond-FD，并把决定端已确定非空的 Lax FD 提升为 Strict。
- `MakeCartesianProduct`、`AddFrom`：合并两个集合；前者偏向关系代数笛卡尔积，后者还合并 hash、分组和聚合元数据。
- `MakeOuterJoin` 与 `ArgOpts`：执行外连接专用传播。`SkipFDRule331`、`OnlyInnerFilter`、`InnerIsFalse` 控制特定推导分支。
- `MaxOneRow`：最多一行时丢弃非等价旧边，保留投影范围内等价类并增加 `{} --> cols`。
- `ProjectCols` / `makeEquivMap`：把图裁到输出列，同时保留可经严格闭包或等价替代恢复的依赖。
- `FindPrimaryKey` / `AllCols`：分别寻找能严格决定当前全部列的决定端，以及收集普通边中出现的列。
- `RegisterUniqueID` / `IsHashCodeRegistered`：维护表达式 hash 与分配后 unique id 的首次映射。
- `FindCommonEquivClasses`：依次对多个 `FDSet` 的等价类求交，只保留长度大于 1 的交集。

文件没有 trait、模块级常量或 feature 条件；唯一条件编译项是 `#[cfg(test)]`，它将独立的 `fd_graph_test.rs` 作为嵌套测试模块加载，未把测试逻辑写入生产源文件。

## 执行流程

普通 FD 的插入从 `AddStrictFunctionalDependency` 或 `AddLaxFunctionalDependency` 进入 `addFunctionalDependency`：先拒绝 `to ⊆ from` 的平凡依赖，再移除两端交集，通过 `ReduceCols` 缩小决定端；随后逐条比较 `implies`，用更强新边替换旧边、忽略已被旧边蕴含的新边，或把同决定端的严格非等价边合并，最后才追加未处理的新边。

加入等价关系时，`addEquivalence` 先用 `ClosureOfEquivalence` 得到完整类并追加自反等价边。旧等价子集会被移除；旧普通边依赖端中已经属于等价类的列会被删去。若等价类与常量边相交，则整个类经 `AddConstants` 进入常量闭包；若与非空列相交，则 `MakeNotNull` 把非空性扩到整个类。

加入常量时，`AddConstants` 先用严格闭包扩展常量集合，再追加唯一的空决定端边。对于已有非等价边，严格边的常量决定列可以删除；Strict 和 Lax 边的常量依赖列都可以删除。决定端或依赖端被完全吸收的边会被移除。

`MakeNotNull` 先合并旧非空列并经过等价闭包扩展，然后循环扫描 `ncEdges`。null-constraint 与非空集合相交的边被取出：常量边走 `AddConstants`，等价边走 `AddEquivalence`，其他边走规范 FD 插入；新等价可能扩大非空集合，因此循环到稳定。之后，只要存在决定端已包含于非空集合的 Lax 边，就加入对应 Strict 边，最后保存稳定后的 `NotNullCols`。

`MakeOuterJoin` 以当前集合为外侧、参数 `inner` 为内侧：先记录两侧候选主键；内侧普通边按决定端是否含已知非空列选择保留 Strict 或降为 Lax；过滤产生的常量和等价先存为以全部内侧列为 null-constraint 的 Cond-FD。连接键还可产生内侧到外侧的 Lax FD、规则 3.3.1 的合成严格边，以及双侧主键共同决定全部输出列的边。结束时合并元数据并从 `NotNullCols` 删除被外连接补 NULL 的内侧列。

`ProjectCols` 分三阶段：先记录常量、被删决定列和相关等价列，并把严格边依赖端扩成完整传递闭包；再为被删决定列建立仍在投影中的等价替代映射；最后裁剪依赖端、替换决定端并通过正规插入 API 重建替换后的边。Strict FD 可直接分解；Lax FD 只有在被删依赖列全为常量或已知非空时才能保留。Cond-FD 仅在其 null-constraint 与投影列相交时裁剪边本身；无交集时按 Go 当前行为继续保留隐藏边。

`FindCommonEquivClasses` 从第一个集合的等价类开始，与后续集合的每个等价类逐层求交；单列交集不构成有意义的二元等价，因此被过滤，结果为空时提前结束。

## 数据与状态

`FastIntSet` 是核心值类型，方法调用中经常使用 `Copy`、`Union`、`Intersection` 或原地 `*With` 操作区分“派生集合”和“更新当前状态”。扩展代码必须特别留意所有权：例如闭包入口接收值并返回新集合，内部插入路径则会消费或原地修改传入集合。

`fdEdges` 只保存当前可见的普通 FD、常量边和等价边。其关键不变量是：平凡普通 FD 不入图；等价边的 `from` 与 `to` 相同且为 Strict；常量边 `from` 为空；规范插入后尽量不存在可被另一条边直接蕴含的冗余边。测试可以因验证内部算法而直接构造边，但 `fd_graph_test.rs` 明确说明生产路径应使用公开插入方法。

`ncEdges` 是不可见边队列，只有 `MakeNotNull` 证明 null-constraint 命中后才转入 `fdEdges`。投影不能无条件丢弃条件列已离开输出的隐藏边，因为 Go 语义要求它仍可能被后续嵌套外连接场景使用；`TestProjectColsKeepsCondFDWithUnprojectedCondition` 专门覆盖这一点。

`NotNullCols` 既是对外属性，也是 Lax 提升和投影合法性的证明条件。`HashCodeToUniqueID` 使用字符串保存任意表达式 hash 字节的 Rust 表示，`RegisterUniqueID` 采用首次写入生效。`GroupByCols` 和 `HasAggBuilt` 随 `AddFrom`/`MakeOuterJoin` 合并，供聚合相关算子避免重复推导。

## 依赖与调用关系

下游依赖只有 `crate::intset::{FastIntSet, NewFastIntSet}` 和标准库 `HashMap`。`intset` 在 `lib.rs` 中重新导出 `astersql-util-intset`，Cargo 清单没有可选 feature；测试额外依赖 `astersql-testkit-testsetup`。

RustCodeGraph 对目标文件的查询显示内部主链为：公开 Add 方法进入 `addFunctionalDependency`，后者调用 `ReduceCols`，再由 `ReduceCols` 调用 `InClosure`/严格闭包；`AddEquivalence`、`AddConstants`、`MakeNotNull` 互相协作维持等价、常量与非空闭包；`ProjectCols` 调用严格闭包、`makeEquivMap` 和正规插入方法重建边；`MakeOuterJoin` 调用主键查找、闭包、约简以及 Strict/Lax/Cond-FD 插入。

直接上游证据包括：

- `pkg/planner/core/operator/logicalop/base_logical_plan.rs`：默认属性推导用 `AddFrom` 合并孩子。
- `logical_projection.rs`：加入重命名列等价和严格依赖，调用 `MakeNotNull` 后 `ProjectCols`。
- `logical_selection.rs`：从谓词提取非空信息并投影到输出列。
- `logical_aggregation.rs`：分组列严格决定非 first-row 聚合输出。
- `logical_join.rs`：合并孩子 FD 并裁到 join 输出。
- `logical_union_all.rs`：调用 `FindCommonEquivClasses`，只保留所有分支共同等价关系。
- `pkg/planner/cascades/old/implementation_rules.rs`：使用 `MaxOneRow` 表达最多一行的实现属性。

RustCodeGraph 的文件使用关系还包含测试及若干同名通用符号产生的噪声；文档只把源码或精确路径搜索能确认的上述位置视为真实业务调用。当前 Rust 算子的常规 Join 推导没有直接调用 `FDSet::MakeOuterJoin`，该方法仍是与 Go 对齐的完整公开变换能力，不能据此宣称它已覆盖所有 Rust join 入口。

## 错误处理与边界

该模块没有 `Result`、显式错误类型或 panic 分支；非法、平凡或重复输入主要通过忽略、规范化或返回哨兵表达。`fdEdge::String` 对“Lax 等价”这一不支持组合返回字符串 `Wrong functional dependency`；`IsHashCodeRegistered` 未命中返回 `(-1, false)`；空 hash 不注册；重复 hash 保留第一次的 unique id。

必须注意以下边界：空 `FindCommonEquivClasses` 输入返回空向量；单列等价交集被丢弃；`FindPrimaryKey` 只考察严格非等价边，返回第一条闭包覆盖全部已知列的决定端，而不是枚举或保证最优候选键；`AllCols` 不读取 `ncEdges`，等价边只需读取一侧；`MakeNullable` 只移除非空标记，不逆向拆除此前已提升的 Strict 边。

闭包和插入算法依赖 `fdEdges` 的规范状态。测试为了构造特定图可直接写私有字段，但外部扩展若绕过 Add API，可能产生互相蕴含的边或不满足等价边形状的状态，使约简结果失去保证。

`String` 主要用于诊断和测试，顺序来自当前向量顺序，不应作为跨重排的稳定序列化格式。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务、文件句柄或网络资源。`FDSet` 是普通可克隆的内存状态，通过 `&mut self` 串行变换；并发共享必须由上层自行同步。

资源生命周期等同于 Rust 值生命周期。闭包计算创建临时集合；`addFunctionalDependency`、`addEquivalence`、`AddConstants`、`ProjectCols` 和 `MakeNotNull` 会使用 `drain` 或重建 `Vec` 来避免在遍历时别名修改。`MakeOuterJoin`、`ProjectCols` 会克隆集合或整个 `FDSet` 作为变换前快照，成本随边数和列集合大小增长。循环均以集合不再增长、待提升边耗尽或输入集合处理完毕为终止条件。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/funcdep/fd_graph.go`，理论约定在同目录 `doc.go`，Go 单元测试在 `fd_graph_test.go`。Rust 保留了 Go 的公开方法命名、`fdEdge`/`FDSet`/`ArgOpts` 结构、Strict/Lax/Equivalence/Cond-FD 分类，以及闭包、约简、常量、非空、外连接、投影和公共等价类算法。`fd_graph_test.rs` 逐项复刻 Go 的严格边合并、闭包、决定端约简、常量传播、Lax 蕴含、等价合并和公共等价类案例。

语言层差异主要是：Go 边使用指针切片，Rust 使用拥有所有权的 `Vec<fdEdge>`；Go 的 `*FastIntSet`/可变集合语义在 Rust 中通过值传递、`Copy` 和显式克隆实现；Go 的 `nil` map/切片对应 Rust 的默认空 `HashMap`/`Vec`；Rust `conditionNC` 用 `Option<FastIntSet>` 表达是否存在条件。

可观察差异是告警：Go 在空 hash、重复 hash 或 `AddFrom` 遇到重复表达式映射时写日志；Rust `RegisterUniqueID` 对空 hash 静默忽略，对重复 key 通过 `or_insert` 保留旧值，`AddFrom` 也静默保留当前集合的旧值。两者的数据结果一致，但 Rust 不产生 Go 的诊断日志。Rust `String` 对不支持的 Lax 等价只返回错误文本，同样没有 Go 版本的日志副作用。

Rust 的 `ProjectCols` 明确保留 null-constraint 与投影列无交集的 Cond-FD；`fd_graph_test.rs::TestProjectColsKeepsCondFDWithUnprojectedCondition` 记录这是对 Go 当前 `continue` 分支行为的兼容，而不是文档推测。

## 扩展指南

新增 FD 类型或改变蕴含规则时，优先修改 `fdEdge::implies`、`addFunctionalDependency` 和闭包函数，并同步独立测试 `pkg/planner/funcdep/fd_graph_test.rs`；不得直接从算子向 `fdEdges` 写边。任何 Strict/Lax 规则变化都要同时核对 `doc.go` 的本项目定义，尤其不要直接套用论文中不同的 Lax 定义。

扩展外连接推导应集中在 `MakeOuterJoin`、`ArgOpts`、`MakeNotNull` 和 Cond-FD 投影逻辑，验证嵌套外连接、null-reject、内侧补 NULL、恒假内侧过滤与规则 3.3.1 开关。还应检查真实算子入口是否调用该方法；仅补 API 测试不能证明逻辑计划主链已经接线。

扩展投影规则时应覆盖三类风险：中间列被删后传递 Strict FD 不丢失、Lax 分解只有在被删列 definite 时成立、被删决定列只能由投影内等价列替代。对应现有证据在 `doc_1_aster_unit_test.rs::projection_preserves_transitive_dependencies`、`fd_graph_test.rs::TestProjectColsKeepsCondFDWithUnprojectedCondition` 和 `extract_fd_test.rs` 的 Join/Projection API 序列测试。

新增元数据字段时，需要决定其在 `Default`、`Clone`、`AddFrom`、`MakeCartesianProduct`、`MakeOuterJoin`、`MaxOneRow` 和 `ProjectCols` 中的传播或清理规则。新增表达式映射行为时，应明确重复 key 与空 key 是否仍保持 Go 数据语义，以及是否需要补足 Rust 的诊断机制。

性能方面，闭包是按边反复扫描，投影和外连接还会克隆图；若边规模显著增长，应先用现有蕴含消除保持图最小，再考虑索引化，且必须验证结果顺序变化不会破坏依赖字符串测试。正确性方面，应继续把测试放在独立 `*_test.rs` 文件，并通过 `#[cfg(test)] #[path = ...]` 接入。

## 验证依据

- RustCodeGraph：`status` 确认仓库索引含 11,467 个文件，目标目录和 `fd_graph.rs` 已索引；`files --filter pkg/planner/funcdep` 确认源、Go 对照与测试集合；`explore 'pkg/planner/funcdep/fd_graph.rs ...'` 确认目标符号、内部调用链和精确上游候选；`node --file pkg/planner/funcdep/fd_graph.rs --offset 1 --limit 900` 完整读取 795 行实现。方法级 `callers/callees` 未返回内容，因此对该图缺口使用精确路径 `rg` 补证。
- 源与 crate 边界：`pkg/planner/funcdep/fd_graph.rs`、`pkg/planner/funcdep/lib.rs`、`pkg/planner/funcdep/Cargo.toml`、包契约 `pkg/planner/funcdep/doc.go`。
- Go 对照：`pkg/planner/funcdep/fd_graph.go` 与 `pkg/planner/funcdep/fd_graph_test.go`；类型、公开方法、算法分支和测试意图逐项比对。
- Rust 独立测试：`pkg/planner/funcdep/fd_graph_test.rs`、`pkg/planner/funcdep/doc_1_aster_unit_test.rs`、`pkg/planner/funcdep/extract_fd_test.rs`。它们覆盖基本图不变量、Go 对齐补充案例和算子级 API 调用序列；本纯文档任务按计划未运行 Cargo。
- 真实调用点通过 `rg` 核对：`pkg/planner/core/operator/logicalop/{base_logical_plan,logical_projection,logical_selection,logical_aggregation,logical_join,logical_union_all}.rs` 与 `pkg/planner/cascades/old/implementation_rules.rs`。
- 人工复核结论：本文件存在是为了把 SQL 逻辑属性压缩为可组合的列级 FD 图；运行方式是由逻辑算子增量写入约束，再通过闭包、规范插入、连接和投影变换传播；安全扩展必须守住边规范、Lax/NULL 语义、unique id 身份和独立测试同步四个边界。
