# `pkg/bindinfo/binding_plan_evolution.rs`

## 文件定位

本文件属于 `astersql-bindinfo` crate 的计划演进层，定义“如何给一组候选绑定计划打分”的抽象和默认实现。模块由 `pkg/bindinfo/lib.rs` 声明并公开重导出；真正组织候选收集、计划生成和推荐回退的是相邻的 `binding_auto.rs`。因此本文件不生成计划、不查询统计、不写绑定，只消费 `BindingPlanInfo` 中已经汇总的计划文本与执行指标。

`pkg/bindinfo/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/bindinfo`，且本文件只使用 crate 内的 `BindingPlanInfo`、`IsSimplePointPlan` 和统一 `Result`，没有直接引入新的外部依赖或 feature 条件。文件中没有条件编译项。

## 核心职责

- `PlanPerfPredictor` 规定统一打分协议：输入可变候选切片，输出与候选等长的分数和解释向量；约定分数位于 `0.0..=1.0`，`1.0` 表示被判定为最佳。
- `ruleBasedPlanPerfPredictor` 按固定优先级执行三条启发式规则：简单点查优先；单位返回行扫描量显著占优；多项延迟/扫描指标同时显著占优。
- `llmBasedPlanPerfPredictor` 保留 LLM 预测器接口位置，但当前只返回全零分和空解释，明确表示“无法推荐”，不是已接入模型的实现。
- 本文件通过全零结果表达“没有足够证据推荐”。`binding_auto.rs::fillRecommendation` 据此决定是否回退到下一个预测器。

## 主要符号

- `pub trait PlanPerfPredictor: Send + Sync`：公开的对象安全预测器接口。`PerfPredicate(&self, plans: &mut [BindingPlanInfo]) -> Result<(Vec<f64>, Vec<String>)>` 允许实现原地调整候选顺序，并允许将预测错误向上传播。
- `pub struct ruleBasedPlanPerfPredictor`：无字段的规则预测器。公开类型名沿用 Go 命名，crate 根通过 `#![allow(non_camel_case_types, non_snake_case)]` 接受该形式。
- `ruleBasedPlanPerfPredictor::PerfPredicate`：主要算法入口。它预先按候选数分配两个结果向量，短路处理边界，再对非点查候选排序和比较。
- `pub struct llmBasedPlanPerfPredictor`：无字段的 LLM 占位预测器。
- `llmBasedPlanPerfPredictor::PerfPredicate`：只按输入长度构造全零分数和空字符串解释，不读取或改写候选内容。

两个实现类型和 trait 都经 `pkg/bindinfo/lib.rs` 的 `pub use binding_plan_evolution::*` 重导出；默认实例由 `binding_auto.rs::newBindingAuto` 装入 `Box<dyn PlanPerfPredictor>`。

## 执行流程

`ruleBasedPlanPerfPredictor::PerfPredicate` 的执行顺序如下：

1. 创建与 `plans.len()` 等长的零分向量和空解释向量。空输入直接返回二者。
2. 单候选无需比较，直接令第 0 项为 `1.0`；此分支不要求执行统计，也不填写解释。
3. 用 `IsSimplePointPlan(&plan.Plan)` 从原顺序中寻找第一个简单 `PointGet`/`BatchPointGet` 计划。命中后只给该位置 `1.0` 和固定解释并返回。
4. 若任一候选的 `ExecTimes == 0`，说明比较所需的运行统计不完整，返回全零结果，并保持候选原顺序。
5. 原地按 `ScanRowsPerReturnRow`、`AvgLatency`、`AvgScanRows`、`LatencyPerReturnRow` 依次升序排序；浮点比较使用 `f64::total_cmp`，因而即使出现 NaN 也有确定顺序。
6. 若排序后第一项的 `ScanRowsPerReturnRow` 严格小于第二项的一半，第一项得 `1.0` 并立即返回。
7. 否则要求第一项的 `AvgLatency`、`AvgScanRows` 和 `LatencyPerReturnRow` 对其余每项都满足“小于或等于一半”；全部满足时第一项得 `1.0`。未满足任何规则则保持全零。

上游 `binding_auto.rs::BindingPlanEvolution for bindingAuto::ExplorePlansForSQL` 收集历史和生成候选，必要时执行新候选补齐指标，然后调用 `fillRecommendation`。`fillRecommendation` 调用 `PerfPredicate`，将最高非零分对应的第一个候选标为 `YES`；规则结果全零时再调用 LLM 预测器。由于规则 2/3 会原地排序，分数位置与排序后的 `plans` 位置保持一致。

## 数据与状态

本文件没有全局变量、缓存或持久状态。两个预测器都是零大小类型，所有工作状态均局限于单次调用中的 `scores`、`explanations` 和借入的 `plans`。

规则读取 `BindingPlanInfo` 的 `Plan`、`ExecTimes`、`ScanRowsPerReturnRow`、`AvgLatency`、`AvgScanRows`、`LatencyPerReturnRow`。这些派生指标由 `binding_auto.rs::apply_exec_info` 从累计执行统计计算；本文件不校验指标来源、单位、负值或有限性。

重要可观察副作用是：只有通过简单点查和执行次数检查后，规则 2/3 的路径才会原地重排候选。结果向量按调用返回时的候选顺序对齐，而不是按传入前的稳定身份对齐。排序不是稳定排序语义契约；扩展方若需要保留身份，应使用 `Binding`/摘要等字段关联，而不要缓存旧下标。

## 依赖与调用关系

直接下游依赖为：

- `ruleBasedPlanPerfPredictor::PerfPredicate` → `binding_auto.rs::IsSimplePointPlan`，用于识别只含点查、选择和投影算子的简单计划；
- 两个实现 → `binding_auto.rs::BindingPlanInfo`，读取计划和统计字段；
- trait 与实现 → `lib.rs::Result`，统一使用 `BindError` 错误通道。

直接上游关系为：

- `binding_auto.rs::newBindingAuto` 构造规则与 LLM 两个 trait 对象；
- `binding_auto.rs::bindingAuto::fillRecommendation` 调用 `PlanPerfPredictor::PerfPredicate`；
- `binding_auto.rs::ExplorePlansForSQL` 先走规则预测器，规则无法推荐时走 LLM；
- `binding_plan_evolution_test.rs` 直接实例化两个默认实现验证其契约，`binding_auto_test.rs` 另有 `ZeroPredictor` 验证预测器回退链。

RustCodeGraph 的文件节点显示目标文件被 `binding_auto.rs`、`binding_auto_test.rs` 和 `binding_plan_evolution_test.rs` 使用；精确 `query` 也定位了 trait 声明及两个实现。图的 `callers/callees` 精确查询未在本次时限内返回内容，因此上述动态调用边进一步由对应源码调用点和文本引用核验，而不是推测。

## 错误处理与边界

- 默认两个实现当前不会主动构造 `Err`；它们仍返回 `Result`，以便未来预测器（例如外部 LLM）报告失败。`binding_auto.rs::fillRecommendation` 使用 `?` 原样传播该错误，不会静默回退。
- 空候选返回两个空向量；单候选总是得 `1.0`，即使没有执行统计；任一多候选缺少执行次数则全体不推荐。
- 简单点查规则先于执行次数检查，所以简单点查即便 `ExecTimes == 0` 仍可获推荐；多个简单点查只推荐输入顺序中第一个。
- 规则 2 使用严格 `< 50%`：恰好等于对手一半不命中。规则 3 使用 `<= 50%`：恰好一半可以命中，但三个指标必须对所有其他候选同时成立。
- trait 仅在文档中约定结果向量等长，默认实现满足该约定；`fillRecommendation` 对解释和分数使用安全索引，但没有拒绝长度不匹配、负数、NaN 或无穷分数的第三方实现。
- LLM 实现是明确占位：全零意味着当前回退链最终仍可能没有推荐，不能描述为已具备模型推理能力。

## 并发与资源生命周期

`PlanPerfPredictor: Send + Sync` 允许预测器 trait 对象随 `bindingAuto` 跨线程共享；默认实现无内部可变状态，多个调用之间不存在共享数据竞争。每次调用独占借用 `&mut [BindingPlanInfo]`，Rust 借用规则保证同一候选切片不会被并发修改。

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络连接。临时向量在返回后由调用者拥有，候选切片的借用在调用结束时释放；预测器本身由 `bindingAuto` 内的 `Box` 管理并随宿主释放。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/bindinfo/binding_plan_evolution.go`。Rust 保留了 `PlanPerfPredictor`、`ruleBasedPlanPerfPredictor`、`llmBasedPlanPerfPredictor`、`PerfPredicate` 的命名、三条规则的优先级、固定解释文本，以及 LLM TODO 的全零占位行为。`pkg/bindinfo/binding_plan_evolution_test.go::TestRuleBasedPlanPerfPredictor` 覆盖的点查、批量点查、规则 2、规则 3 和无推荐场景均在 Rust 独立测试中复现。

需要注意的实现差异：Go 使用 `sort.Slice`，当首指标相等时以三个次级指标同时更小作为比较条件；Rust 使用四个字段的确定性字典序 `total_cmp`。常规测试场景语义一致，但次级指标互有优劣或含特殊浮点值时，Rust 的排序更全序化，不能宣称逐比较器完全等价。Rust 还用值切片代替 Go 指针切片，并通过 `Result`/`Vec` 明确表达错误和所有权。

Rust 测试 `binding_plan_evolution_test.rs` 在 Go 测试之外明确验证空输入、单候选、缺执行统计时不排序、50% 严格边界和 LLM 不改写计划文本。这些是当前 Rust 行为契约，不应在扩展时无意破坏。

## 扩展指南

- 新增或调整启发式规则时，修改 `ruleBasedPlanPerfPredictor::PerfPredicate`，明确规则相对点查、缺统计检查和排序的优先级；同步更新独立的 `pkg/bindinfo/binding_plan_evolution_test.rs`，并核对 Go 对照实现与 `binding_plan_evolution_test.go`。
- 接入真实 LLM 时，在 `llmBasedPlanPerfPredictor::PerfPredicate` 内实现评分，保持结果与候选等长、分数范围合法、解释下标对齐，并通过 `Result` 报告传输/解析错误。网络客户端、凭据和超时策略应由上层注入，避免在无状态预测器中隐藏全局资源。
- 若需要避免候选重排，可改为排序索引或带身份的临时视图，但必须同时调整分数映射和 `bindingAuto::fillRecommendation`，并增加多候选顺序回归测试。
- 若允许第三方预测器，应在 `PlanPerfPredictor` 契约或 `fillRecommendation` 中决定是否强制校验向量长度、NaN、无穷和范围；否则调用者对异常实现只有部分防护。
- 性能方面，点查和缺统计扫描为 `O(n)`，规则排序为 `O(n log n)`，规则 3 再做一次 `O(n)` 比较。候选量扩大时应优先评估是否需要无需整体排序的最小值选择。
- 测试逻辑应继续保留在独立 `binding_plan_evolution_test.rs`，不要嵌入生产源文件。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标包内目标文件有 7 个符号；`files --filter pkg/bindinfo` 确认源文件、Go 对照和独立测试均已索引。
- RustCodeGraph `node --file pkg/bindinfo/binding_plan_evolution.rs`：核对完整 112 行源码、trait、两个实现、排序字段、阈值及文件使用者。
- RustCodeGraph `query PerfPredicate`、`query ruleBasedPlanPerfPredictor`、`query llmBasedPlanPerfPredictor`：定位 Rust/Go 声明与测试实现；精确 `callers/callees` 查询超时无输出，调用边改由下列源码位置交叉核验。
- `pkg/bindinfo/binding_auto.rs`：核对 `BindingPlanInfo` 字段来源、`newBindingAuto` 组装、`ExplorePlansForSQL` 回退顺序、`fillRecommendation` 调用及错误传播。
- `pkg/bindinfo/Cargo.toml` 与 `pkg/bindinfo/lib.rs`：核对 crate 名、Go 包映射、依赖边界、模块声明、公开重导出及统一错误类型。
- `pkg/bindinfo/binding_plan_evolution.go` 与 `pkg/bindinfo/binding_plan_evolution_test.go`：核对 Go 三条规则、解释文本、排序逻辑和原始测试意图。
- `pkg/bindinfo/binding_plan_evolution_test.rs`：核对 Rust 推荐规则、输入重排、边界条件和 LLM 占位契约；`pkg/bindinfo/binding_auto_test.rs::ZeroPredictor` 作为回退链的直接测试证据。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另运行固定十一章节结构检查并人工复核所有“当前支持”结论均有上述源码或测试依据。
