# `pkg/planner/core/operator/physicalop/physical_merge_join.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate；crate 入口 `pkg/planner/core/operator/physicalop/lib.rs` 以私有模块 `mod physical_merge_join` 纳入它，再通过 `pub use physical_merge_join::*` 对外重导出公开符号。`Cargo.toml` 的 `package.metadata.porting.go-package` 指向同目录 Go 包，直接对照文件是 `physical_merge_join.go`。

它描述规划阶段的物理 Merge Join 节点，而不是执行器本身：`PhysicalMergeJoin` 保存连接条件、连接键、排序方向和键比较器，接入通用 `PhysicalPlan`/`Plan` 接口，并为 EXPLAIN、代价查询、任务树挂接和列下标解析提供计划期行为。算子的上游生成入口可见于 `base_physical_plan.rs` 的 Merge Join 枚举分支和 `pkg/planner/cascades/old/implementation_rules.rs`；执行计划的通用 trait 接线由 `lib.rs` 中的 `join_operator_core!(PhysicalMergeJoin, BasePhysicalJoin)` 提供。

当前 Rust 文件不是 Go 文件的完整逐函数复刻。Go 的 `GetMergeJoin` 同时负责候选枚举、排序属性匹配、提示冲突和强制 Merge Join；Rust 的同名函数只从 `LogicalJoin` 抽取基类字段并构造单个节点，候选属性处理由上游规划代码承担。阅读或扩展时必须连同这些上游入口核对，不能仅据函数同名推断行为等价。

## 核心职责

- `PhysicalMergeJoin` 表示需要两侧输入按连接键有序的二元物理连接节点；`Desc` 记录归并方向，`CompareFuncs` 为每个已物化左连接键保存一个比较函数槽位。
- `GetMergeJoin` 将 `LogicalJoin` 的左右单侧条件、其他条件和可识别的“列 op 列”等值条件克隆到 `BasePhysicalJoin`，并依据左键类型建立比较函数。
- `BuildMergeJoinPlan` 提供以显式左右键和连接类型创建最小计划骨架的入口；它填入两个整数 `1` 作为默认值，并建立比较器。
- `Init`、`Clone`、`Attach2Task`、`GetPlanCostVer1/2` 把该具体节点接入公共物理计划基础设施。
- `ExplainInfo`/`ExplainNormalizedInfo` 生成稳定的计划说明；`MemoryUsage` 估算节点自身内存；`ResolveIndices` 把连接键、条件和输出列绑定到孩子 schema 的实际下标。
- 四个 crate 内辅助函数分别处理最长有序键前缀、未用等值条件迁移、按偏移重排以及“允许跳过前导常量键”的排序兼容判断。

本文件不负责读取数据、维护运行时游标或执行归并循环。注释所述“线性扫描合并匹配行”是该物理算子的执行语义前提，实际执行由后续执行器消费计划完成。

## 主要符号

- `pub struct PhysicalMergeJoin`：唯一核心类型。
  - `BasePhysicalJoin: BasePhysicalJoin` 保存 `JoinType`、左右连接键、左右单侧条件、其他条件、默认值以及 `PhysicalSchemaProducer`/`BasePhysicalPlan`。
  - `Desc: bool` 表示按连接键降序归并；构造函数默认 `false`，上游属性枚举可再设置。
  - `CompareFuncs: Vec<crate::JoinCompareFunc>` 与物化连接键一一对应，并在克隆及计划缓存快照中保留。
- `pub fn GetMergeJoin(&LogicalJoin, PhysicalSchemaProducer) -> PhysicalMergeJoin`：公开构造入口。它克隆表达式；只接受能下转为 `ScalarFunction` 且 `ExtractColumnsFromColOpCol` 能提取出左右列的等值条件。
- `pub fn ShouldSkipHashJoin(bool, bool) -> bool`：两个开关做逻辑或；分别代表“偏好不使用 Hash Join”和“会话禁用 Hash Join”。
- `pub fn BuildMergeJoinPlan(ContextRef, JoinType, Vec<Column>, Vec<Column>) -> PhysicalMergeJoin`：显式键构造入口；Go 注释说明其当前主要供测试使用，Rust 本身未限制调用可见性。
- `PhysicalMergeJoin::Init`：用 `NewBasePhysicalPlan(ctx, "MergeJoin", offset)` 替换节点基计划并写入统计信息。
- `Clone`：通过 `BasePhysicalJoin::CloneWithSelf` 克隆基类，原样复制 `Desc`，共享克隆 `CompareFuncs` 中的 `Arc` 比较函数；错误以 `expression::Error` 返回。
- `Attach2Task`：委托 `base::PhysicalPlan::attach_to_task`。
- `GetCost`：当前本地简化值为非负化后的左右数值之和。级联规划的 `BinaryJoinCostPlan` 会调用它；主计划代价路径另在 `pkg/planner/core/plan_cost_ver1.rs` 对 `PlanKind::MergeJoin` 使用按行数与 CPU 因子计算的模型。
- `GetPlanCostVer1` / `GetPlanCostVer2`：转发到嵌入的 `BasePhysicalPlan` 缓存/计算接口，而非在本文件复刻 Go 的专用 utilfuncp 回调。
- `ExplainInfo` / `ExplainNormalizedInfo` / 私有 `explain`：输出连接类型、非内连接的左侧节点、左右键及三类条件；归一化版本避免具体列显示。
- `MemoryUsage`：基类占用加 `CompareFuncs` 的 Vec 头部、容量对应的函数槽位以及一个布尔量。
- `ResolveIndices`：本文件最重要的可失败变换，详见执行流程。
- `find_max_prefix_len`：求多个候选键序列与目标键序列的最大共同前缀长度。
- `move_equal_to_other_conditions`：保留原 `other` 顺序，再追加未被偏移集合使用的 `equal` 条件，且对所有表达式调用 `CloneExpr`。
- `reorder_by_offsets<T: Clone>`：先按 `offsets` 指定顺序复制元素，再按原顺序追加未选元素。
- `is_sort_prop_compatible_with_join_keys`：顺序消费所需排序项；连接键中只有位于下一个匹配项之前且已证明为常量的键可以跳过。

本文件没有模块级常量、trait 定义或条件编译项；测试模块的 `#[cfg(test)]` 声明位于 `lib.rs`，测试逻辑独立保存在 `physical_merge_join_test.rs`。

## 执行流程

典型的计划构造链如下：

1. 上游从逻辑连接和所需物理属性枚举 Merge Join。`base_physical_plan.rs` 在 Merge Join 分支创建 `PhysicalSchemaProducer`、复制逻辑输出 schema，然后调用 `GetMergeJoin(...).Init(...)`；旧 cascades 实现规则也直接调用 `GetMergeJoin`。
2. `GetMergeJoin` 用逻辑连接类型创建 `BasePhysicalJoin`，深克隆左右单侧条件和其他条件，再逐个检查等值条件。只有可识别为“列与列运算”的标量函数才生成左右键；未识别条件不会在此自动转入 `OtherConditions`。
3. 对每个左连接键，从 `RetType` 通过 `ranger::chunk::GetCompareFunc` 取得比较器并包装为 `Arc`。随后节点以升序默认值返回。
4. `Init` 安装带上下文、类型名、查询块偏移及统计信息的 `BasePhysicalPlan`。上游再设置孩子、孩子所需属性或 `Desc`。
5. 代价选择阶段可以经 `GetCost` 或通用 `GetPlanCostVer1/2` 查询节点代价；任务构造阶段由 `Attach2Task` 把节点挂到两个子任务之上。`pkg/planner/core/task.rs` 另有按 `PlanKind::MergeJoin` 走 `root_binary` 的计划表示路径。
6. 计划定型后调用 `ResolveIndices`：
   - 先解析 `PhysicalSchemaProducer` 自身；若失败立即返回。
   - 要求至少两个孩子，分别取得左右 schema；不足时返回 `merge join requires exactly two children`。错误文案写“exactly”，当前检查实际只拒绝少于两个，额外孩子不会在此处被拒绝。
   - 左键和左条件只对左 schema 解析，右键和右条件只对右 schema 解析。
   - 合并左右 schema 后解析 `OtherConditions`；合并失败返回 `merge join schemas are unavailable`。
   - 克隆输出 schema。对 `LeftOuterSemiJoin`/`AntiLeftOuterSemiJoin` 跳过末尾的匹配标志列，其余输出列通过单调前进方式映射到合并 schema，从而正确处理重复列引用。
   - 若未找到全部应解析输出列，返回包含计划 explain id 的错误；成功则用独立的输出 schema 替换旧 schema。
7. EXPLAIN 阶段按固定顺序追加连接类型、非内连接左侧、左右键、左右条件和其他条件；条件列表通过排序后的 explain 辅助函数渲染，降低无关顺序差异。

辅助属性流程中，`find_max_prefix_len` 用于选择最完整的有序键前缀；`reorder_by_offsets` 保证优先键与剩余键均保持确定顺序；`move_equal_to_other_conditions` 防止未被归并键吸收的等值条件丢失；`is_sort_prop_compatible_with_join_keys` 只允许跳过已证明常量的前导键，不能跨越普通键满足后续排序项。

## 数据与状态

节点的持久状态主要位于 `BasePhysicalJoin`、`Desc` 和 `CompareFuncs`。条件表达式在 `GetMergeJoin` 中通过 `CloneExpr` 建立所有权隔离；连接键从提取结果克隆。`Clone` 继续通过基类专用克隆路径隔离计划上下文和表达式状态，同时 `CompareFuncs` 的 `Arc` 可安全共享不可变比较逻辑。

`CompareFuncs` 的长度由构造当时的左键数量决定。两个公开构造入口都要求每个左键有 `RetType` 且 `ranger::chunk::GetCompareFunc` 支持该类型，否则使用 `expect` 触发 panic。该不变量意味着后续若直接修改连接键，必须同步重建比较函数；本文件没有公开的 `initCompareFuncs` 修复入口。

`ResolveIndices` 是原地状态转换：键和条件会被替换为带孩子下标的表达式，输出 schema 的列也被克隆后写入 `Index`。单调扫描依赖输出列顺序是左右合并 schema 的有序子序列；它特意不使用一般解析算法，以免重复列都绑定到第一次出现的位置。

`reorder_by_offsets` 假设所有偏移合法且通常无重复；越界会 panic，重复偏移会在前缀中重复元素。`move_equal_to_other_conditions` 用 `HashSet<usize>` 去重已使用偏移，越界偏移只会成为无匹配集合成员，不会访问数组。`find_max_prefix_len` 对空候选返回 `0`。

## 依赖与调用关系

直接 crate 依赖由 `Cargo.toml` 声明：

- `base` 提供 `ContextRef`、`JoinType`、`Plan`、`PhysicalPlan` 和 `Task`，形成公共计划接口。
- `logicalop` 提供 `LogicalJoin`，是 `GetMergeJoin` 的逻辑输入。
- `expression` 提供列、表达式克隆/解析、schema 合并和 EXPLAIN 工具，是本文件最主要的数据操作依赖。
- `property` 提供统计信息、任务类型和排序项；`costusage` 提供两版代价接口类型。
- `ranger` 的 `chunk::GetCompareFunc` 根据字段类型选择比较器；`types` 用于构造默认 Datum。
- crate 内部的 `BasePhysicalJoin`、`PhysicalSchemaProducer`、`NewBasePhysicalPlan` 和 `JoinCompareFunc` 提供具体节点骨架。

已核实的上游引用包括：

- `base_physical_plan.rs` 调用 `GetMergeJoin(...).Init(...)` 枚举物理 Merge Join。
- `pkg/planner/cascades/old/implementation_rules.rs` 在 Hash Join/Apply 构造时复用 `GetMergeJoin` 抽出的 `BasePhysicalJoin`，并在 Merge Join 实现规则中构造真正的 `PhysicalMergeJoin`；其中 `BinaryJoinCostPlan for PlanAdapter<PhysicalMergeJoin>` 调用 `GetCost`。
- `physical_merge_join_test.rs` 直接覆盖四个 crate 内辅助函数；`joins_agg_aster_unit_test.rs` 覆盖 `ShouldSkipHashJoin` 的四种布尔组合。

已核实的下游消费者还包括：

- `lib.rs` 的宏接线使该类型实现通用具体物理算子能力，并在缓存快照契约表中登记 Go/Rust 类型映射。
- `cache_snapshot.rs` 对该节点进行动态下转，捕获并恢复 `BasePhysicalJoin`、`Desc` 和 `CompareFuncs`。
- `pkg/planner/core/flat_plan.rs` 根据连接类型为 Merge Join 的两个孩子标注 build/probe 侧。
- `pkg/planner/core/task.rs` 和 `pkg/planner/core/plan_cost_ver1.rs` 在另一套 `PlanKind` 表示中分别负责二元 root task 挂接与专用代价计算。

RustCodeGraph 的 `node --file` 报告该文件被六个文件使用，并明确列出 `statement_ru_plan_walk.rs`、旧 cascades 规则、`flat_plan.rs`、`base_physical_plan.rs`、`cache_snapshot.rs` 等；精确符号的 `callers/callees` 命令未返回边，因此上述具体调用边以直接引用搜索和源文件上下文复核。

## 错误处理与边界

- `GetMergeJoin` 对不能下转为 `ScalarFunction` 或不能提取左右列的等值条件静默跳过；这与“把未用等值条件转入其他条件”的辅助函数不是自动衔接关系，上游必须保证条件不会因此丢失。
- 两个构造入口在键缺少返回类型或类型无比较器时通过 `expect` panic，而不是返回 `Result`。任何新增键类型支持都必须先确保 `GetCompareFunc` 覆盖。
- `Clone`、两版计划代价函数和 `ResolveIndices` 传播 `expression::Error`；前两类主要委托基类，`ResolveIndices` 还产生孩子不足、schema 不可用和输出列不可解析三类本地错误。
- `ResolveIndices` 使用 `children[0]`、`children[1]` 前先检查数量，但不验证恰好为二；调用者仍应维持二元算子不变量。
- `ExplainInfo` 对非内连接读取第一个孩子时使用 `first()`，缺少孩子不会 panic，只是不输出 `left side`；归一化版本输出孩子类型，普通版本输出 explain id。
- `reorder_by_offsets` 对越界偏移没有防御，调用前必须验证偏移来自对应 `values`；这是扩展候选属性逻辑时的显式前置条件。
- 排序兼容函数只比较列身份和常量集合，不检查 `SortItem.Desc`。升降序一致性应由上游对整个物理属性的检查和 `Desc` 设置保证。
- 当前 Rust 文件未实现 Go `GetMergeJoin` 中的 Enum/Set 排除、NULL-safe 等值键拒绝、字符集/排序规则冲突检查、hint 冲突告警、outer join 输出排序限制和强制 Merge Join 枚举。除非上游另有等价检查，否则不能宣称这些 Go 边界在本文件内已覆盖。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、channel、事务或 I/O；所有构造和索引解析均同步发生在规划阶段。`PhysicalMergeJoin` 的可变方法要求 `&mut self`，没有内部可变性或本文件自建的并发协议。

主要资源生命周期是计划对象和表达式所有权：构造时克隆条件/键；`Clone` 创建绑定到 `new_ctx` 的新基类；缓存快照在 `cache_snapshot.rs` 中捕获并恢复比较器；`CompareFuncs` 通过 `Arc` 克隆共享比较实现。孩子计划由 `BasePhysicalPlan` 持有，`Attach2Task` 把节点交给通用任务树，生命周期规则由 `base::PhysicalPlan`/`Task` 管理。

`MemoryUsage` 只估算节点基类、比较器 Vec 容器及布尔字段；它没有单独追踪 `Arc` 指向的共享函数对象，也不表示执行时输入缓冲、排序或结果行内存。性能判断需把上游排序代价和执行器资源一并纳入。

## 与 Go 版本的对应关系

字段层面，Rust `PhysicalMergeJoin` 对应 Go 同名结构：都嵌入/持有 `BasePhysicalJoin`，并有 `Desc`、`CompareFuncs`。Rust 比较器槽位使用 `Arc` 封装的 `JoinCompareFunc`，Go 使用 `expression.CompareFunc`。

已保持的主要语义包括：

- `Clone` 保留基类、比较器和降序标志。
- EXPLAIN 均按连接类型、左侧、左右键、左右条件、其他条件的顺序输出，且有归一化版本。
- `MemoryUsage` 均按基类、比较器切片容量和布尔字段计量。
- `ResolveIndices` 均分别用左右 schema 解析左右键/条件，以合并 schema 解析其他条件，并用单调扫描避免重复列错误；两种 outer-semi join 都跳过末尾生成列。
- `find_max_prefix_len`、未用等值条件追加、偏移重排、前导常量键排序兼容与 Go 辅助函数的核心顺序语义一致。

明确差异如下：

- Go `GetMergeJoin` 返回多个候选 `[]base.PhysicalPlan`，处理属性、hint、NULL-safe 键、Enum/Set、collation、期望行数与强制排序；Rust `GetMergeJoin` 只返回一个尚待上游完善的节点。
- Go `ShouldSkipHashJoin` 自行读取逻辑计划 hint 和 session 变量；Rust 接受两个已计算布尔值，因此调用者承担取值职责。
- Go `BuildMergeJoinPlan` 在 `Init` 后返回指针且当时未初始化比较器；Rust 返回值对象并立即按左键初始化比较器。
- Go `GetCost`/`Attach2Task`/两版计划代价委托 `utilfuncp` 中的专用回调；Rust 本文件分别使用简化和基类路径，另有 `PlanKind` 级专用函数存在于 core 文件中。
- Go `initCompareFuncs` 同时根据成对的左右键和表达式上下文选择比较器；Rust 只依据左键 `RetType` 选择 chunk 比较器。左右键类型/排序规则兼容必须由上游维持。
- Go 另有 `tryToGetChildReqProp`、`checkJoinKeyCollation`、`getNewJoinKeysByOffsets` 和 `getNewNullEQByOffsets`；Rust 本文件只有通用 `reorder_by_offsets`，未保留 NULL-equality 数组处理或完整属性传递方法。

因此迁移状态应描述为“核心计划节点与若干辅助算法已实现，完整 Go 候选枚举语义分散在上游或尚需逐项核实”，而不是完整等价。

## 扩展指南

- 新增或改变逻辑连接到 Merge Join 的转换时，优先检查 `GetMergeJoin` 以及真实上游候选枚举点 `base_physical_plan.rs`、旧 cascades `implementation_rules.rs`。若涉及未使用等值条件，必须明确调用或等价实现 `move_equal_to_other_conditions`，防止谓词丢失。
- 新增连接键类型时，先保证 `ranger::chunk::GetCompareFunc` 可返回比较器；若比较语义依赖左右两种类型、collation 或表达式上下文，应评估当前“仅看左键类型”的实现是否足够，并补充错误返回而非 panic 的设计。
- 改变键顺序或排序属性时，应成组维护左右键、可能存在的 NULL-equality 元数据、`Desc` 和 `CompareFuncs`；`reorder_by_offsets` 的偏移必须验证合法且不重复。同步扩展 `physical_merge_join_test.rs` 的前缀、重排、条件迁移与常量键测试。
- 改动输出 schema 或 join 类型时，应重点复核 `ResolveIndices` 对重复列和 outer-semi 末尾匹配标志的处理。回归测试必须放在独立测试文件，覆盖孩子不足、左右/合并 schema 解析失败和输出列不存在；不要把测试内嵌进生产 `.rs`。
- 改动缓存可克隆字段时，同时更新 `cache_snapshot.rs` 的 `CachedMergeJoin::capture/restore` 和 `lib.rs` 的缓存快照契约/生成器配置，确保 `Desc`、比较器及新字段往返一致。
- 改动代价或 task 挂接时，先判定使用的是具体 `PhysicalMergeJoin` 方法还是 `PlanKind::MergeJoin` 路径，并同步检查 `plan_cost_ver1.rs`、`task.rs`、`utilfuncp` 接口及旧 cascades 的 `BinaryJoinCostPlan`，避免两套表示漂移。
- 追求 Go 对齐时，按功能逐项核实 Enum/Set、NULL-safe 等值、collation、hint 冲突、outer join 排序和强制 Merge Join；不应仅复制大段 Go 代码或把未接线 helper 当成已生效行为。
- 性能风险集中在不完整排序兼容判断导致额外排序或错误选择、比较器与键不一致、以及输出 schema 解析退化。当前辅助算法均为线性扫描；不要无依据改成集合化处理而破坏顺序语义。

## 验证依据

本说明基于以下直接材料：

- 目标源码：`pkg/planner/core/operator/physicalop/physical_merge_join.rs`（410 行），核对全部 import、结构体、三个公开自由函数、`impl` 方法及四个 crate 内辅助函数。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml`；模块声明、重导出、缓存契约及 trait 宏接线：同目录 `lib.rs`。
- Go 对照：同目录 `physical_merge_join.go`（571 行），逐项比较结构、候选枚举、构造、代价、EXPLAIN、比较器、内存、索引解析和排序 helper。
- 独立 Rust 测试：`physical_merge_join_test.rs` 验证最长前缀、稳定重排、未用等值条件追加以及只能跳过前导常量键；`joins_agg_aster_unit_test.rs` 验证 `ShouldSkipHashJoin` 的四种输入组合。目录内未检索到直接针对 Go `PhysicalMergeJoin` helper 的同名 Go 单元测试引用。
- 上下游直接引用：`base_physical_plan.rs`、`pkg/planner/cascades/old/implementation_rules.rs`、`pkg/planner/core/flat_plan.rs`、`cache_snapshot.rs`、`pkg/planner/core/task.rs`、`pkg/planner/core/plan_cost_ver1.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query PhysicalMergeJoin/GetMergeJoin/BuildMergeJoinPlan/ShouldSkipHashJoin` 定位了 Rust/Go 对照符号；`node --file ... --offset 1 --limit 500` 返回目标文件全貌并报告六个使用文件。路径 `files --filter` 和精确 Rust 节点 `callers/callees` 未产生可用边，故调用关系又由直接引用搜索复核。

本任务是纯文档分析，未运行 Cargo 或代码测试。结构验证要求是目标文档存在且恰有本文的十一个固定二级标题；行为结论以源码、模块配置、Go 对照和既有独立测试交叉验证。
