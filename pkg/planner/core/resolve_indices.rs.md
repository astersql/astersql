# `pkg/planner/core/resolve_indices.rs`

## 文件定位

本文件位于 `astersql-planner-core` crate，提供一套围绕“把表达式列引用的 `unique_id` 解析为子节点输出中的位置 `index`”的简化值模型与辅助入口。`pkg/planner/core/lib.rs` 通过 `mod resolve_indices` 无条件编译该模块，并用 `pub use resolve_indices::*` 公开再导出其中的公有类型和函数；对应独立测试模块只在 `cfg(test)` 下启用。

需要区分模块可见性与主链接线：RustCodeGraph 将本文件标记为仅被 `pkg/planner/core/resolve_indices_test.rs` 使用，仓库精确引用搜索也未发现这些驼峰公有函数的生产调用者。当前优化主链在 `optimizer_runtime.rs` 上调用动态物理计划的 `resolve_indices()`，完整物理算子在 `operator/physicalop/` 下各自实现相关逻辑。因此本文件当前是可公开复用、用于 Go 行为对齐和聚焦测试的简化 API，而不是所有生产物理计划解析工作的唯一实现。

## 核心职责

- 用 `IndexedColumn`、`IndexedExpr` 和 `IndexSchema` 表示索引解析所需的最小列、表达式与有序 Schema 信息。
- 为 Projection、UnionScan、IndexLookUpReader、Selection、TopN 和 Limit 的简化计划结构原地写回列下标。
- 保留三项重要的 Go 语义：表达式树递归解析；inline projection 对有序 Schema 的单调匹配，以区分重复列；相邻 Projection 对同源输出列的代表下标归并。
- 在 Selection 的普通 `unique_id` 解析失败后，支持按 `virtual_expr` 文本回退匹配虚拟列。
- 以 `Result<_, String>` 报告缺列或缺失虚拟列，并保留逐项原地更新造成的“失败前修改不回滚”行为。

本文件不负责构建真实物理计划树、递归解析真实子计划、生成执行器 protobuf、计算成本或执行 SQL；这些职责仍属于完整的 planner/physicalop 体系。

## 主要符号

- `IndexedColumn { unique_id, index, virtual_expr }`：`unique_id` 是匹配键，`index` 是解析结果，`virtual_expr` 是虚拟列回退所需的可选文本。派生 `Clone/Default/Eq/PartialEq` 便于值语义更新和测试比较。
- `IndexedExpr`：只有 `Column` 与递归的 `Scalar { name, args }` 两种分支；标量函数名只被保留，不参与列匹配。
- `IndexSchema = Vec<IndexedColumn>`：顺序即执行期行布局；部分算法不仅看成员是否存在，还依赖这个顺序。
- `resolve_column`：在 Schema 中寻找第一个 `unique_id` 相等的列，克隆输入列并写入位置；缺失时返回包含列 ID 的错误。
- `resolve_expr`：深度优先递归解析表达式树；`collect::<Result<Vec<_>, _>>()` 在首个失败参数处短路。
- `resolve_virtual_expr`：优先按 `unique_id`，否则按完全相等的 `virtual_expr` 文本寻找第一个候选；任一递归参数无法匹配时返回 `None`。
- `ProjectionPlan` 与 `resolveIndicesItself4PhysicalProjection` / `resolveIndices4PhysicalProjection`：前者只解析表达式并细化相邻 Projection，后者先解析输出 Schema 的 inline 映射再调用前者。
- `find_root` / `refine4NeighbourProj`：用路径压缩并查集把子 Projection 中引用同一输入下标的多个输出槽位合并到首个输出槽位。
- `UnionScanPlan` / `resolveIndices4PhysicalUnionScan`：依次解析过滤条件，再解析所有 handle 列。
- `IndexLookUpReaderPlan` / `resolveIndices4PhysicalIndexLookUpReader`：校验输出虚拟列可在表侧 Schema 找到，并解析额外 handle 与 common-handle 列。
- `SelectionPlan` / `resolveIndices4PhysicalSelection`：逐条件普通解析；失败后对整棵条件树尝试虚拟表达式回退。
- `resolveIndexForInlineProjection`：按输出 Schema 与子 Schema 的既有顺序单向前进，保证重复列分别绑定到不同的后续位置。
- `ByItem`、`PartitionByItem`、`TopNPlan`、`LimitPlan`：承载排序表达式、分区列、输出/子 Schema 与可选前缀列；对应入口按固定顺序解析这些字段。

除计划结构与七个算子入口外，列/表达式解析、并查集和 inline projection 辅助函数均为模块私有。文件没有条件编译项、模块级常量、trait 或 `unsafe`。

## 执行流程

1. 调用方先构造带逻辑列 ID 的简化计划，并提供已经有序的 `child_schema`。
2. 普通表达式路径进入 `resolve_expr`：列节点由 `resolve_column` 找到第一个同 ID 槽位；标量节点保持名称并递归重建参数数组。任何子表达式缺列都会立即返回错误。
3. Projection 完整入口先调用 `resolveIndexForInlineProjection`。该函数为每个输出列从上次匹配位置继续向后扫描，因此子 Schema `[7, 7, 9]` 与输出 Schema `[7, 7, 9]` 会得到 `[0, 1, 2]`，而不会把两个 `7` 都绑定为 `0`。随后解析投影表达式；若存在 `child_projection`，以其克隆快照执行相邻投影细化。
4. `refine4NeighbourProj` 只考察子 Projection 中直接为 `Column` 的表达式。它按输入 `index` 收集输出位置，把同组后续输出并到首个输出，再把父 Projection 中直接列引用的下标替换为并查集根；父层标量表达式不参与这一轮细化。
5. UnionScan 先逐个解析 `conditions`，再逐个解析 `handle_columns`。IndexLookUpReader 先检查每个带 `virtual_expr` 的输出列在 `table_schema` 中是否存在同文本候选，再解析 `extra_handle_col` 和 `common_handle_cols`。
6. Selection 对每个条件先执行严格 ID 解析；若失败，则从原条件出发递归执行 `resolve_virtual_expr`。回退成功时丢弃原错误，回退失败时返回原始缺列错误。
7. TopN 依次解析 `by_items`、`partition_by`、inline 输出 Schema 和 `prefix_col`。Limit 跳过排序项，依次处理 `partition_by`、inline 输出 Schema 和 `prefix_col`。

所有公有入口都是同步的原地修改。它们遇到首个错误即返回，但不会撤销此前已经写入的字段。

## 数据与状态

核心不变量是“`unique_id` 表示逻辑身份，`index` 表示当前直接子节点行布局中的物理偏移”。`resolve_column` 的独立查找采用首个匹配；只有 inline projection 因重复输出列问题采用两个有序序列的单调扫描。调用方必须保证 inline 输出列按子 Schema 的相对顺序构成可匹配子序列，否则即使相同 ID 存在，也会因已经越过匹配位置而失败。

相邻 Projection 的 `BTreeMap<usize, Vec<usize>>` 以输入下标分组，`BTreeMap` 令分组遍历稳定，但每组代表始终由子表达式的枚举顺序决定。并查集大小取 `child.schema.len()`；代码假设子投影直接列表达式的输出位置和父层直接列引用的 `index` 都落在这个范围内。若构造出越界的不一致计划，`find_root` 会索引切片并 panic，而不是返回 `Result` 错误。

`IndexLookUpReaderPlan.index_schema` 当前只作为结构字段保存，本文件入口不读取它；入口也不把输出 Schema 的虚拟列改写为 table-side 下标，只验证同文本虚拟表达式存在。`virtual_expr` 使用字符串完全相等和首个匹配，没有表达式归一化、类型检查或歧义拒绝。

## 依赖与调用关系

直接标准库依赖只有 `std::collections::BTreeMap`；列、表达式、Schema 与简化计划均在本文件内定义。`pkg/planner/core/Cargo.toml` 将模块归入 `astersql-planner-core`，crate 根为 `lib.rs`，默认 feature 为空，`nextgen` feature 只转发配置依赖；本文件没有 feature gate，也没有直接使用 Cargo 中的外部 crate。

文件内主要调用边为：

- `resolveIndices4PhysicalProjection` → `resolveIndexForInlineProjection` → `resolveIndicesItself4PhysicalProjection`。
- `resolveIndicesItself4PhysicalProjection` → `resolve_expr`，并在有子 Projection 时 → `refine4NeighbourProj` → `find_root`。
- UnionScan、IndexLookUpReader、TopN、Limit 入口 → `resolve_column`；Projection、UnionScan、Selection、TopN → `resolve_expr`。
- Selection 严格解析失败后 → `resolve_virtual_expr`。

RustCodeGraph 的文件边显示直接使用者为 `resolve_indices_test.rs`。模块通过 `lib.rs` 再导出后可被 crate 外调用，但当前仓库搜索没有发现这些公有驼峰入口的生产调用。完整生产路径的直接对照是 `base::PhysicalPlan::resolve_indices`、`operator/physicalop` 各算子方法以及 `optimizer_runtime.rs` 的最终解析调用，不能把两套类型或调用边混为一体。

## 错误处理与边界

- 普通缺列错误为 `column {unique_id} cannot find the reference from its child`；inline projection 缺列使用较笼统的 `some columns cannot find the reference from its child(ren)`；虚拟列缺失使用 `virtual column {unique_id} is missing`。
- 错误是普通 `String`，没有 Go 版本的结构化 planner 错误、算子 Explain ID 或错误码，调用方只能按成功/失败或文本处理。
- `resolve_expr` 和各顺序循环在首错处短路。由于外层把每个成功结果立即写回，后续失败会留下之前已解析的表达式或 handle；独立测试分别固定了 Projection、UnionScan 和 IndexLookUpReader 的这一行为。
- Selection 的回退对整棵原条件重新解析，只有所有递归节点都可匹配才成功；失败时返回严格 ID 路径的原错误，而不是虚拟匹配失败的额外诊断。
- 空表达式、空 handle 或空分区列表自然成功。没有前缀列时不会额外解析。空输出 Schema 的 inline projection 也成功。
- `refine4NeighbourProj` 对标量表达式不做重定向，且对越界下标没有显式保护；扩展构造路径时必须维持 Schema 与表达式下标一致。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄，也没有全局可变状态。所有入口在调用线程同步执行，并借助 `&mut` 独占修改计划。

表达式解析通过克隆构造新值，再逐项替换原字段；临时 `Vec`、`BTreeMap` 和并查集向量在函数返回时释放。Projection 的 `child_projection.as_deref().cloned()` 会生成完整子 Projection 快照，使细化期间可以同时独占父计划而不持有指向其字段的借用；代价是一次与子 Projection 大小相关的克隆。错误返回不会泄漏资源，但也不会事务式回滚先前写回。

## 与 Go 版本的对应关系

直接 Go 对照为 `pkg/planner/core/resolve_indices.go`。公有 Rust 入口保留了 Go 私有函数的名称和总体顺序：Projection 的 producer/inline Schema 后接表达式解析，UnionScan 的条件后接 handle，Selection 的虚拟表达式回退，TopN/Limit 的分区、inline Schema 与前缀列，以及相邻 Projection 的并查集合并。

关键差异如下：

- Go 函数接收真实 `base.PhysicalPlan` 并向下转型，调用真实孩子、Schema、表达式上下文和子计划 `ResolveIndices`；Rust 本文件接收自包含的简化结构，不递归真实计划树。完整 Rust 物理算子另有 trait 实现。
- Go Projection 从真实第一个孩子识别相邻 Projection；这里使用可选的 `child_projection` 克隆。Go inline projection 会克隆 Schema 切片和匹配列以避免共享列对象；这里的列本来就是拥有值，直接在 `Vec` 中改写。
- Go UnionScan 与 Selection 会先解析 `BasePhysicalPlan`，IndexLookUpReader 会依次解析 table/index 子计划；简化 Rust 入口不包含这些步骤，且 `index_schema` 未消费。
- Go 虚拟表达式回退使用求值上下文和表达式语义；这里用 `virtual_expr: String` 完全相等近似。Go 的虚拟列 helper 还负责真实 Schema 列关系，这里只检查存在性。
- Go `HandleCols` 是组合抽象；这里是普通 `Vec<IndexedColumn>`。Go common handle 使用 `TablePlans[0].Schema()`，这里统一使用 `table_schema`。
- Go 错误带 planner/Explain 上下文；这里简化为字符串。Rust 独立测试覆盖部分更新和递归虚拟列回退，但未找到同目录专门针对 `resolve_indices.go` 的 Go 测试；Go 语义主要由生产实现以及相关完整算子测试间接约束。

Go 还从 `core_init.go` 把若干入口安装到 `utilfuncp` 函数变量，并在 `rule_inject_extra_projection.go` 直接复用 `refine4NeighbourProj`。本文件在 Rust 侧没有对应的函数变量注册或额外投影规则调用证据，不能据名称相同推断接线完全等价。

## 扩展指南

- 新增表达式种类时，必须同步审查 `resolve_expr` 与 `resolve_virtual_expr` 的递归规则，并在独立的 `pkg/planner/core/resolve_indices_test.rs` 增加普通成功、深层失败及虚拟列回退用例；测试不要内嵌到生产 `.rs`。
- 新增算子字段时，按真实执行读取顺序决定解析时机，并为“前项已写回、后项失败”的状态增加断言。若需要原子性，应显式先在副本上完成所有解析再整体提交，这属于行为变化，需同时核对 Go。
- 修改 inline projection 时必须保持有序子序列扫描，尤其覆盖重复 ID、输出重排、缺列和空 Schema；不要退化为对每列独立查找首个匹配。完整算子的直接回归面还包括 `operator/physicalop/physical_limit_test.rs::resolve_indices_keeps_duplicate_inline_projection_columns_distinct`。
- 修改相邻 Projection 细化时，应增加同一输入映射多个输出、标量表达式跳过、父下标代表重定向和越界保护测试，并对照 Go `refine4NeighbourProj` 以及 `rule_inject_extra_projection.go` 的实际用途。
- 加强虚拟列行为时，应明确字符串匹配是否需要规范化、重复候选如何处理、输出 Schema 是否要改写，并与完整 physicalop 的表达式上下文实现保持一致。
- 若目标是把本简化 API 接入生产主链，需先证明它能承载真实计划递归、表达式类型/上下文与结构化错误；不能只增加一个调用点便宣称替代完整实现。

## 验证依据

- RustCodeGraph：`status` 显示项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/core/resolve_indices.rs` 确认目标文件已索引；`node --file ... --offset 1 --limit 500` 读取到完整 332 行、29 个符号，并报告直接使用者为 `pkg/planner/core/resolve_indices_test.rs`。
- 精确符号查询：`query` 对七个公有入口以及 `resolveIndexForInlineProjection`、`refine4NeighbourProj` 均同时定位到本 Rust 文件和 `pkg/planner/core/resolve_indices.go`；精确 `callers` 查询在本地超时，因此调用者结论另用模块入口与仓库引用搜索交叉核验，没有把不完整图结果当成肯定证据。
- Rust 源码：`pkg/planner/core/resolve_indices.rs`；模块接线：`pkg/planner/core/lib.rs`；crate 边界与 feature：`pkg/planner/core/Cargo.toml`。
- 独立 Rust 测试：`pkg/planner/core/resolve_indices_test.rs`，覆盖递归虚拟表达式回退，以及 Projection、UnionScan、IndexLookUpReader 在后续失败时保留先前写回。
- Go 对照：`pkg/planner/core/resolve_indices.go`；Go 注册与额外投影调用证据：`pkg/planner/core/core_init.go`、`pkg/planner/core/rule_inject_extra_projection.go`。
- 完整 Rust 主链接线核验：`pkg/planner/core/base/plan_base.rs`、`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/core/operator/physicalop/` 中的算子实现与 `physical_limit_test.rs`。这些文件只用于确认本简化模块和完整生产实现的边界。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定的结构命令确认文档存在且恰好包含十一个固定二级标题，并人工复核所有“当前已接线/未接线”陈述都有上述源码或搜索证据。
