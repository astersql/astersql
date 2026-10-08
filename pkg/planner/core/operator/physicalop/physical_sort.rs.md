# `pkg/planner/core/operator/physicalop/physical_sort.rs`

对应源码：[physical_sort.rs](./physical_sort.rs)。

## 文件定位

本文件定义物理排序节点 `PhysicalSort`，并提供从 `LogicalSort` 枚举物理候选、从 `ORDER BY` 表达式推导物理排序属性以及判断排序前缀是否匹配的辅助函数。它位于 `astersql-planner-core-operator-physicalop` crate；crate 入口 `lib.rs` 以 `mod physical_sort` 装入模块，再用 `pub use physical_sort::*` 对外导出。

它处在规划与执行之间的三个边界上：

- 规划入口：`base_physical_plan.rs` 的逻辑算子路由将 `LogicalSort` 交给 `ExhaustPhysicalPlans4LogicalSort`；旧 Cascades 实现规则和排序 enforcer 也直接构造 `PhysicalSort`。
- 物理计划公共接口：`lib.rs` 中的 `impl_sort_operator!(PhysicalSort, ...)` 为该类型接入 `Plan`、`PhysicalPlan`、克隆、Schema、Explain、代价、任务挂接和 PB 编码等统一分派。
- 执行入口：`pkg/executor/physical_plan_runtime.rs` 识别 `PhysicalSort`，先执行唯一孩子，再用 `sort.ByItems` 构造 `SortExec`。因此本文件描述计划节点和规划语义，不实现排序算法本身。

目标包没有 `doc.go`；crate 边界以本目录 `Cargo.toml`、`lib.rs` 和上述路由为准。

## 核心职责

1. 用 `PhysicalSort` 保存通用物理计划状态、排序项 `ByItems` 和部分排序标志 `IsPartialSort`。
2. 维护节点生命周期：`New`/`Init` 初始化，`Clone` 深拷贝排序表达式，`ResolveIndices` 将表达式列索引绑定到孩子 Schema。
3. 向诊断和优化提供 `ExplainInfo`、`ExtractCorrelatedCols`、`MemoryUsage` 与旧版 `GetCost`。
4. 通过 `Attach2Task`、`GetPlanCostVer1` 和 `GetPlanCostVer2` 接入任务树及两套代价模型。
5. 仅为 `IsPartialSort == true` 的节点生成 `tipb::Executor`，形成可下推的 partial-sort PB 子树。
6. 由 `LogicalSort` 和所需 `PhysicalProperty` 枚举真实 `PhysicalSort` 或 `NominalSort` 候选。
7. 用 `GetPropByOrderByItemsContainScalarFunc`、`GetPropByOrderByItems` 和 `MatchItems` 在排序表达式与物理属性之间转换、校验。

这里的“排序”有两层含义：`PhysicalSort` 表示真实排序工作；`NominalSort` 表示把排序要求传给孩子、在条件允许时不新增真实排序。二者由 `ExhaustPhysicalPlans4LogicalSort` 联合枚举。

## 主要符号

### `PhysicalSort`

- `PhysicalSchemaProducer: PhysicalSchemaProducer`：承载 `BasePhysicalPlan`、Schema、统计信息、孩子及孩子所需属性等公共状态。虽然结构体保存了生产者 Schema，`lib.rs` 中针对 Sort 的 `schema_operator` 会优先返回第一个孩子的 Schema，确保 Sort 不改变输出列布局。
- `ByItems: Vec<planner_util::ByItems>`：有序的排序表达式列表；每项包含表达式和 `Desc` 方向。列表次序就是排序键优先级。
- `IsPartialSort: bool`：标识只对一个分区内的数据排序。它是 `ToPB` 的硬前置条件，也参与窗口/TiFlash 细粒度 shuffle 规划。

### 构造、复制与绑定

- `New(ctx)`：创建 `TypeSort` 节点，排序项为空，`IsPartialSort` 为 `false`。
- `Init(self, ctx, stats, offset, props)`：设置上下文、类型、查询块偏移、统计信息和孩子所需物理属性。
- `Clone(&self, new_ctx)`：通过 `CloneWithNewCtx` 复制基础计划，复制可选 Schema，逐项调用 `ByItems::Clone`，并保留 `IsPartialSort`。
- `ResolveIndices(&mut self)`：先解析生产者/基础计划，再以第一个孩子的 Schema 重写每个排序表达式；没有孩子时安全返回 `Ok(())`。

### 可观测信息与代价

- `ExtractCorrelatedCols`：从每个排序表达式收集相关列并克隆结果。
- `MemoryUsage`：累计生产者、`Vec` 头、按 capacity 计算的指针槽、每个 `ByItems` 以及布尔字段的估算占用。
- `ExplainInfo`：用 `ExplainByItems` 格式化排序键；若 `TiFlashFineGrainedShuffleStreamCount > 0`，追加 `stream_count`。
- `GetCost(count, schema)`：旧式局部估算，CPU 为 `n * log2(n) * CPUFactor`，基础内存为 `n * MemoryFactor`；可能根据行宽、内存额度和临时存储开关加入落盘代价。
- `GetPlanCostVer1`/`GetPlanCostVer2`：转发到 `BasePhysicalPlan`。公共宏和 planner core 的路由器负责把具体节点接到规范代价实现。

### 规划与属性辅助函数

- `ExhaustPhysicalPlans4LogicalSort(logical, required)`：为逻辑 Sort 返回当前物理属性下的候选集合。
- `GetPropByOrderByItemsContainScalarFunc(items)`：允许列引用，或能由 `ScalarFunction::GetSingleColumn` 还原成单列的特殊标量函数；返回可选属性和“是否全为列引用”。
- `GetPropByOrderByItems(items)`：只接受全列引用；标量函数即使能还原单列也返回 `None`。
- `MatchItems(required, items)`：验证所需排序项是 `ByItems` 的匹配前缀，逐项比较方向和列恒等性。

## 执行流程

### 从逻辑 Sort 枚举物理候选

1. `base_physical_plan.rs` 识别 `LogicalSort` 后调用 `ExhaustPhysicalPlans4LogicalSort`。
2. 函数先取得逻辑节点上下文；上下文缺失时返回空集合。
3. 对 Root task，只有 `MatchItems(required, logical.ByItems)` 成立才继续：
   - 创建孩子属性，继承 task 类型、CTE 状态和 `NoCopPushDown`，并把 `ExpectedCnt` 设为 `f64::MAX`，表示真实 Sort 需要读取完整孩子输入；
   - 按父属性的 `ExpectedCnt` 缩放逻辑统计；
   - 克隆 `ByItems` 和逻辑 Schema，生成真实 `PhysicalSort`；
   - 再尝试 `NominalSort::FromLogical(..., false)`，若排序键可转成属性，则追加名义排序候选。
4. 对 MPP task，不创建本文件中的真实 Sort 候选，只尝试 `NominalSort::FromLogical(..., true)`；该模式要求排序键最终全为列引用，并保留 MPP 的本质属性。
5. 其他 task 类型或不匹配的 Root 属性返回空集合。

除此之外，当已有候选不能满足强制排序属性时，`base_physical_plan.rs::enforce_canonical_sort` 会从 `required.SortItems` 构造 `ByItems`，设置 `IsPartialSort = required.IsSortItemAllForPartition()`，必要时先补 MPP Exchange，再把 Sort 包装进 `RootTask`。

### 节点进入执行

1. `Attach2Task` 调用公共 `PhysicalPlan::attach_to_task`，由 `lib.rs` 的公共实现克隆孩子任务计划，把当前节点接在孩子之上并构造 `RootTask`。
2. 执行器的 `physical_plan_runtime.rs` 通过 downcast 识别 `PhysicalSort`。
3. 它要求恰有一个孩子，先取得孩子输出行，再将 `ByItems` 转成执行排序键。
4. `SortExec` 对这些行排序，执行侧负责真正的数据比较、缓冲和输出；这些机制不在本文件内。

### partial sort 下推为 PB

1. `ToPB` 先拒绝 `IsPartialSort == false` 的节点。
2. 从 `BuildPBContext` 取得 client 和表达式求值上下文；缺失 client 返回错误。
3. 每个 `ByItems` 经 `SortByItemToPB` 转换；任一表达式不可下推即整体失败。
4. 要求第一个孩子存在，并递归调用孩子的 `to_pb`。
5. 创建 `tipb::Sort`，写入排序项、`is_partial_sort = true` 和孩子；再创建 `TypeSort` executor，写入 executor id、细粒度 shuffle stream count 与 batch size。

### 旧式局部代价计算

1. `count` 至少按 2 计算，避免 `log2` 在小输入上退化。
2. 行宽优先来自统计直方图的磁盘平均行宽，否则按输出 Schema 各列静态类型宽度求和。
3. 若开启 OOM 临时存储、语句内存额度为正，且 `row_size * count` 超额，则按额度比例压低内存成本，并增加 `count * DiskFactor * row_size` 的磁盘成本。
4. 返回 CPU、调整后内存和磁盘三部分之和。

## 数据与状态

- 节点自身可变状态仅为公共计划状态、`ByItems` 和 `IsPartialSort`。排序数据不保存在计划节点中，而在执行侧 `SortExec` 生命周期内处理。
- `ByItems` 的顺序和 `Desc` 是语义数据；`MatchItems` 只允许 `required.SortItems` 匹配它的前缀，不能跳项或忽略方向。
- `GetPropByOrderByItemsContainScalarFunc` 对可还原为单列的标量函数可能翻转方向，采用 `GetSingleColumn` 返回的 `desc`，不能直接沿用原始 `item.Desc`。
- `PhysicalSort` 的输出 Schema 应与孩子一致。宏生成的 `schema_operator` 优先读取孩子 Schema；独立 Rust 测试还验证替换孩子后 Schema 会随之变化，而非继续暴露节点中旧的缓存 Schema。
- `Clone` 对排序项做语义克隆，不共享原 `ByItems` 容器；`IsPartialSort` 是值复制。基础节点和 Schema 的复制规则委托给 `PhysicalSchemaProducer`/`BasePhysicalPlan`。
- `MemoryUsage` 是计划对象的估算，不代表执行排序所需的全部行缓冲内存；后者由执行器和内存跟踪器管理。
- `GetCost` 读取 session 级 CPU、内存、磁盘因子，语句 `MemTracker` 限额以及全局 `EnableTmpStorageOnOOM`。函数不修改这些配置，只在局部变量中调整成本。

## 依赖与调用关系

### 上游

- `base_physical_plan.rs`：逻辑到物理的统一路由调用 `ExhaustPhysicalPlans4LogicalSort`；属性 enforcer 直接构造 `PhysicalSort`。
- `pkg/planner/cascades/old/implementation_rules.rs`：用 `MatchItems`、`GetPropByOrderByItems` 选择实现并创建排序节点。
- `pkg/planner/cascades/old/enforcer_rules.rs`：在所需顺序无法自然满足时插入 `PhysicalSort`。
- `physical_topn.rs`：复用 `MatchItems` 和属性推导函数处理 TopN 排序需求。
- `nominal_sort.rs`：复用允许特殊标量函数的属性推导函数。
- planner core 的优化、代价和执行计划遍历代码通过 downcast 识别 `PhysicalSort`。

### 下游

- `base`：`ContextRef`、`Plan`、`PhysicalPlan`、`Task`、`BuildPBContext` 以及公共计划状态。
- `logicalop` 和 `property`：`LogicalSort`、统计信息、task 类型、排序/分区属性。
- `planner_util` 和 `expression`：排序项、表达式克隆、相关列、索引解析、Explain 格式化及 PB 表达式转换。
- `statistics`、`cardinality`、`chunk`、`vardef`：行宽估算、类型宽度、临时存储开关和内存配额。
- `tipb`、`kv`：下推 executor 的协议结构与存储类型。
- `NominalSort`：与真实 Sort 一起构成逻辑排序的候选集合。

`Cargo.toml` 将本 crate 定义为 `astersql-planner-core-operator-physicalop`，`autotests = false`、`doctest = false`；Rust 测试通过 `lib.rs` 中 `#[cfg(test)] mod physical_sort_test` 编入库测试。与本文件直接相关的依赖均在 crate 清单中声明，包括 `base`、`costusage`、`cardinality`、`chunk`、`expression`、`kv`、`logicalop`、`property`、`planner_util`、`statistics`、`plancodec`、`vardef` 和带 `protobuf-codec` feature 的 `tipb`。清单没有本文件专属 feature gate。

## 错误处理与边界

- `New` 可产生尚无排序项和孩子的节点；调用者必须在计划进入执行或下推前完成接线。
- `ResolveIndices` 会传播基础计划或表达式索引解析错误；没有孩子时返回成功，不会索引越界。
- `ToPB` 明确拒绝非 partial sort、缺失 PB client、不可下推表达式和缺失孩子，并传播孩子 PB 转换错误。
- 常规 Root `PhysicalSort` 不能借 `ToPB` 下推；它应由 Root 执行器执行。只有被标为分区内 partial sort 的节点允许形成 tipb Sort。
- `ExhaustPhysicalPlans4LogicalSort` 用空候选表达“不适用/上下文缺失/属性不匹配”，其 Rust 签名不携带 Go 版本的 handled 布尔值或 error；调用路由负责把候选集合纳入更大的优化流程。
- `GetPropByOrderByItemsContainScalarFunc` 遇到不能还原为单列的标量函数或其他表达式类型时返回 `(None, false)`；调用者不能把失败误解成无排序要求。
- `MatchItems` 在所需项比实际 `ByItems` 长时立即失败；空的所需排序项按前缀规则匹配任何列表。
- `Attach2Task` 及宏生成的公共挂接路径包含对物理节点/孩子克隆成功的 `expect`，表示规划阶段的不变量；违反时会 panic，而非返回可恢复错误。
- 本文件不验证 `NaN`、负统计值等异常成本输入；只对 `count` 做下限截断，并把估算行宽截断到非负。

## 并发与资源生命周期

`PhysicalSort` 本身不创建线程、异步任务、锁、通道或事务，也不持有执行数据资源。计划节点通过拥有的 `Vec<ByItems>` 和 boxed 孩子形成普通 Rust 所有权树；克隆到新 context 时由 `Clone` 复制必要状态。

并发相关配置均为只读依赖：`GetCost` 读取 session variables、语句内存 tracker 和全局原子开关 `EnableTmpStorageOnOOM`；`ExplainInfo`/`ToPB` 读取细粒度 shuffle 参数。实际排序缓冲、可能的落盘和执行器释放由执行模块负责，不能从本文件推断其具体线程模型或 spill 实现。

partial sort 与分布式资源边界有关但不自行管理资源：规划器先保证 MPP 分区属性，再把 `IsPartialSort` 编入 PB；存储侧 executor 按分区执行。`ToPB` 借用 `BuildPBContext`，递归取得拥有的孩子 executor，返回后不保留对 context/client 的借用。

## 与 Go 版本的对应关系

同目录 `physical_sort.go` 是直接语义对照，核心字段及大部分方法一一对应：`PhysicalSort`、`Init`、`Clone`、`ExtractCorrelatedCols`、`MemoryUsage`、`ExplainInfo`、`Attach2Task`、代价入口、`ToPB`、`ResolveIndices`、物理候选枚举和三个属性辅助函数均有对应实现。

已核实的等价点：

- Root 匹配时同时考虑真实 Sort 和可用的 NominalSort；MPP 只接受能通过孩子属性满足的仅列排序。
- 真实 Sort 的孩子 `ExpectedCnt` 为最大值，统计按父期望行数缩放。
- 排序前缀同时比较列和升降序；特殊标量函数通过 `GetSingleColumn` 抽取列与方向。
- 非 partial sort 不允许转换为 PB；PB 携带孩子、executor id 和细粒度 shuffle 参数。
- Go 测试 `pkg/planner/core/physical_plan_test.go::TestPhysicalPlanMemoryTrace` 与 Rust 测试共同验证 `ByItems` 会增加内存估算；Go `optimizer_test.go` 的窗口/shuffle 场景证明只有 partial sort 传播细粒度 shuffle stream count，普通 Sort 会禁用该路径。

需要注意的 Rust 形态差异：

- Go 通过嵌入 `BasePhysicalPlan` 表达继承，Rust 通过 `PhysicalSchemaProducer` 组合并由 `lib.rs` 宏实现 traits。
- Go 的 `GetCost` 和 `Attach2Task` 委托 `utilfuncp`；Rust 本文件保留局部旧式成本公式，任务挂接走公共 trait 默认实现，同时规范 v1/v2 代价仍经 planner core 路由。
- Go `ExhaustPhysicalPlans4LogicalSort` 返回“候选、是否处理、错误”，Rust 返回候选向量；当前 Rust 路由把空向量作为无候选。
- Rust `ToPB` 显式检查 client、表达式转换和孩子存在，错误边界比 Go 版本的直接取值更防御性。
- Go 对 nil receiver 的 `MemoryUsage` 返回零；Rust 引用方法不存在 nil receiver 情形。
- Go 文件还实现 `CloneForPlanCache`；本 Rust 文件没有同名方法，不能据此宣称 Rust Sort 已在本处提供等价的 plan-cache 克隆入口。

## 扩展指南

- 新增排序节点字段时，应同步修改 `New`、`Clone`、`MemoryUsage`，并判断 `ExplainInfo`、PB、代价、计划缓存和宏接线是否需要承载该字段；同时更新独立的 `physical_sort_test.rs`，不要把测试写进生产源文件。
- 改变排序表达式支持范围时，优先审查 `GetPropByOrderByItemsContainScalarFunc`、`GetPropByOrderByItems` 和 `MatchItems`，并同步检查 `nominal_sort.rs`、`physical_topn.rs` 与旧 Cascades implementation rules。风险在于错误地把不能由孩子保证的顺序当成物理属性，导致漏排或结果顺序错误。
- 改变逻辑 Sort 枚举时，应覆盖 Root 匹配/不匹配、MPP、标量函数、缺失 context 和 NominalSort 可/不可生成的分支。现有 Rust 测试只直接覆盖 Root 同时生成两个候选，其他分支需要新增独立测试。
- 扩展 PB 下推时，应验证 partial-only 不变量、表达式转换失败、零/多孩子处理、store 类型以及 TiFlash shuffle 参数；兼容性风险包括生成旧存储节点不理解的 tipb 字段。
- 调整成本时必须区分本文件的旧式 `GetCost` 与 planner core v1/v2 路由实现，并用同一行数、行宽、内存额度、临时存储开关对照 Go。只改一条路径会造成候选选择不一致。
- 调整 Schema 行为时保留“优先孩子 Schema”的宏覆盖，并扩展 `sort_replacement_preserves_child_output_schema`；Sort 不应改变列集合或顺序。
- 与执行行为有关的变更还需同步 `pkg/executor/physical_plan_runtime.rs` 和 sort executor 的独立测试。本文件只定义计划，不应在这里复刻执行排序算法。
- 性能审查重点是 `ByItems` 深克隆、属性枚举候选数、`n log n` 成本及 spill 判断；正确性审查重点是方向、列恒等、表达式索引和分区内 partial 语义。

## 验证依据

事实核对使用了以下本地证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `node --file pkg/planner/core/operator/physicalop/physical_sort.rs --offset 1 --limit 400`：读取目标文件全部 385 行，并报告它被 13 个文件使用。
- RustCodeGraph `query PhysicalSort --kind struct`、`query ExhaustPhysicalPlans4LogicalSort --kind function`、`query GetPropByOrderByItemsContainScalarFunc --kind function`、`query MatchItems --kind function`：确认 Rust/Go 同名定义及位置。
- RustCodeGraph callers/callees 查询：确认 `ExhaustPhysicalPlans4LogicalSort` 调用 `MatchItems`；工具对同名 Go/Rust 符号未完全消歧，因此上游引用又以精确路径搜索和下列文件交叉核对。
- `pkg/planner/core/operator/physicalop/lib.rs`：模块装配、公开再导出、`impl_sort_operator!` 以及 Sort 的孩子优先 Schema 规则。
- `pkg/planner/core/operator/physicalop/base_physical_plan.rs`：逻辑 Sort 路由、强制排序和 MPP 分区接线。
- `pkg/planner/core/operator/physicalop/nominal_sort.rs`、`physical_topn.rs`：属性推导函数的直接消费者。
- `pkg/executor/physical_plan_runtime.rs`：`PhysicalSort` 到 `SortExec` 的直接执行入口。
- `pkg/planner/core/operator/physicalop/Cargo.toml`：crate 名称、模块依赖、tipb feature、测试装配模式和 Go 包映射。
- `pkg/planner/core/operator/physicalop/physical_sort.go`：逐符号 Go 对照。
- `pkg/planner/core/operator/physicalop/physical_sort_test.rs`：Rust 独立测试覆盖内存 capacity、Root 真实/名义候选和替换孩子后的 Schema。
- `pkg/planner/core/physical_plan_test.go`、`optimizer_test.go`、`find_best_task_test.go`：Go 侧内存估算、partial sort/shuffle 和属性 enforcer 行为。

人工复核结论：文档区分了规划节点、名义排序与执行排序，明确了 partial-only PB 边界、候选枚举、状态所有权、Go 差异及安全扩展位置；没有把未在本文件中实现的执行 spill、并发模型或 plan-cache 克隆声明为已支持。
