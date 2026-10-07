# [`pkg/bindinfo/binding_auto.rs`](./binding_auto.rs)

## 文件定位

本文件位于 `astersql-bindinfo` crate（见 `pkg/bindinfo/Cargo.toml`），实现 SQL 绑定子系统中的“候选计划探索与推荐”编排层。crate 根 `pkg/bindinfo/lib.rs` 将 `binding_auto` 声明为私有模块并重新导出其公开项，因此外部调用者看到的是 `BindingPlanEvolution`、`ExploreContext`、`BindingPlanInfo`、`PlanRuntime`、`newBindingAuto` 和 `IsSimplePointPlan`，而不是模块路径本身。

它不直接解析 SQL、访问绑定系统表、调用优化器或执行查询；这些环境相关操作被压缩到 `PlanRuntime` trait。默认对象由 `pkg/bindinfo/binding_handle.rs` 的 `NewBindingHandle` 调用 `newBindingAuto` 组装，并作为 `bindingHandle.BindingPlanEvolution` 暴露。RustCodeGraph 给出的直接上游边是 `NewBindingHandle → newBindingAuto`，目前对 `ExplorePlansForSQL` 的已索引 Rust 调用者主要是同目录独立测试；这意味着运行时的完整 SQL 语句接线仍取决于更上层对 `BindingPlanEvolution` trait 对象的使用，不能仅凭本文件断言已经覆盖 Go 的 `EXPLAIN EXPLORE` 执行链。

## 核心职责

1. 用 `BindingPlanEvolution::ExplorePlansForSQL` 定义并实现一次探索的总流程：取历史候选、生成新候选、可选试跑新候选、合并候选并推荐一个计划。
2. 用 `PlanRuntime` 隔离绑定存储、语句统计、实际执行和优化器状态枚举，使本模块可由测试替身驱动。
3. 用 `PlanExecInfo` 与 `apply_exec_info` 把累计执行数据换算为推荐器使用的平均延迟、平均扫描量和单位返回行成本。
4. 用 `fillRecommendation` 实现规则预测器优先、LLM 预测器兜底的选择协议，并保证并列最高分时只推荐第一个候选。
5. 用 `IsSimplePointPlan` 对 EXPLAIN 文本执行轻量白名单分类；该函数被 `pkg/bindinfo/binding_plan_evolution.rs` 的规则预测器用于优先选择 Point Get 类计划。

本文件负责流程编排而非各算法的全部细节：候选搜索在 `pkg/bindinfo/binding_plan_generation.rs`，规则及 LLM 预测器在 `pkg/bindinfo/binding_plan_evolution.rs`，绑定实体在 `pkg/bindinfo/binding.rs`。

## 主要符号

- `PlanExecInfo`：一次或多次执行的累计统计载体。`Plan` 保存文本计划；`ResultRows`、`ExecCount`、`ProcessedKeys`、`TotalTime` 是派生平均指标的原始量。`planExecInfo` 只是保留 Go 小写类型名的公开别名。
- `PlanRuntime: Send + Sync`：运行时适配接口。`historical_bindings` 读取 SQL 或摘要对应的历史绑定；`plan_exec_info` 按计划摘要取统计；`execute_binding` 试跑绑定；`generation_spec` 和 `plan_under_state` 为 `planGenerator` 提供搜索空间与按状态产出计划的能力。
- `ExploreContext`：携带 `CurrentDB`、`Charset`、`Collation`，这些值同时传给历史绑定查询和候选生成，避免未限定表名、解析字符集和排序规则脱离会话环境。
- `BindingPlanInfo`：探索结果单元，持有共享的 `Arc<Binding>`、计划文本、五项执行派生指标及 `Recommend`/`Reason`。默认值表示“尚无统计、尚未推荐”，不是一次零成本执行。
- `BindingPlanEvolution`：面向调用者的线程安全接口；核心方法 `ExplorePlansForSQL(&ExploreContext, &str, bool)` 返回完整候选列表，而不是只返回获胜者。
- `bindingAuto`：crate 内默认实现，组合一个共享 `PlanRuntime`、一个 `PlanGenerator` 以及规则和 LLM 两个 `PlanPerfPredictor` trait 对象。
- `newBindingAuto`：公开构造器。它复用同一个 `Arc<dyn PlanRuntime>` 构造默认 `planGenerator`，并安装 `ruleBasedPlanPerfPredictor` 与 `llmBasedPlanPerfPredictor`。
- `runToGetExecInfo`：只对 `ExecTimes == 0` 的输入候选调用 `PlanRuntime::execute_binding`，随后用 `apply_exec_info` 写入指标。
- `getBindingPlanInfo` / `getPlanExecInfo`：私有历史候选装配函数；前者过滤删除态绑定，后者处理空摘要并委托运行时查询统计。
- `fillRecommendation`：调用预测器、找最高分、写入唯一 `YES (from <name>)` 和其解释，并把其余项置为 `NO`。
- `apply_exec_info`：集中执行累计值到平均值的换算，保护执行次数和返回行数两个除数。
- `IsSimplePointPlan`：公开的文本分类器。非空行的首个空白分隔 token 必须是表头 `id`，或包含 `Point_Get`、`Batch_Point_Get`、`Selection`、`Projection`；空/全空白计划返回 `false`。

本文件没有条件编译项、模块级常量或固有可变全局状态。

## 执行流程

`ExplorePlansForSQL` 的顺序具有行为意义：

1. 调用 `getBindingPlanInfo(CurrentDB, sqlOrDigest, Charset, Collation)`。输入先 `trim`；空输入立即返回 `BindError("SQL or digest is empty")`。运行时返回的每个 `Binding` 中，`StatusDeleted` 被丢弃。
2. 对非删除绑定，只有 `PlanDigest` 非空时才调用 `getPlanExecInfo`。统计缺失或 `ExecCount <= 0` 时仍保留候选，但 `Plan` 和派生指标保持默认值；正执行次数才交给 `apply_exec_info`。
3. 调用默认或注入的 `PlanGenerator::Generate` 生成新候选。默认实现进一步经同一 `PlanRuntime` 的 `generation_spec` 与 `plan_under_state` 枚举搜索状态，并把计划提示写入绑定 SQL；细节属于 `binding_plan_generation.rs`。
4. `analyze == true` 时，仅把“新生成候选”切片交给 `runToGetExecInfo`。历史候选不会被再次执行；其中新候选若已带 `ExecTimes > 0` 也会跳过。
5. 将新候选追加在历史候选之后。这个稳定顺序决定同分时谁先获得推荐，同时可能被规则预测器自身的排序改变。
6. 先调用规则预测器。`fillRecommendation` 在候选为空或最高分为 `0.0` 时返回 `false`，此时再调用 LLM 预测器；规则层产生非零最高分后不再调用 LLM。
7. 返回所有候选。若两个预测器都给全零分，则没有候选被写成 `YES`；由于第一次全零调用不会重写字段，调用者不应把默认空字符串等同于显式 `NO`。

RustCodeGraph 对实现方法确认的下游边包括：`ExplorePlansForSQL → getBindingPlanInfo/runToGetExecInfo/Generate`，`runToGetExecInfo → execute_binding/apply_exec_info`，以及 `getBindingPlanInfo → historical_bindings/getPlanExecInfo/apply_exec_info`。

## 数据与状态

- 所有绑定都通过 `Arc<Binding>` 共享。本文件只替换 `BindingPlanInfo` 自身的文本、数值和推荐字段，不修改 `Binding` 内部；`Arc` 也使历史列表、生成列表及外部持有者可共享同一绑定。
- `PlanExecInfo` 表示累计量；`apply_exec_info` 计算 `AvgLatency = TotalTime / ExecCount`、`AvgScanRows = ProcessedKeys / ExecCount`、`AvgReturnedRows = ResultRows / ExecCount`。只有平均返回行数大于零时，才计算两个“每返回行”指标。
- `ExecCount <= 0` 的历史统计不会进入 `apply_exec_info`，因此不会把无效累计量或负计数传播到结果。`runToGetExecInfo` 则信任运行时返回内容并总是复制 `Plan`/`ExecCount`，仅由 `apply_exec_info` 的除零保护决定派生值。
- `fillRecommendation` 的胜者是 `scores` 中第一个等于最高分的索引。`Reason` 只从同索引的解释向量取得；解释缺项时使用空字符串。非胜者会清空旧理由，避免复用候选时遗留推荐说明。
- 预测器接收可变切片。当前规则预测器可能原地排序候选，因此返回列表的最终顺序不应被当作严格的“历史在前、生成在后”协议；简单点查或统计不完整的早退分支则不会排序。

## 依赖与调用关系

上游装配关系为：

`binding_handle.rs::NewBindingHandle` → `newBindingAuto` → `bindingAuto` → 以 `Arc<dyn BindingPlanEvolution>` 存入 `bindingHandle.BindingPlanEvolution`。

核心下游关系为：

- `bindingAuto::getBindingPlanInfo` → `PlanRuntime::historical_bindings` / `PlanRuntime::plan_exec_info`；
- `bindingAuto.planGenerator.Generate` → `binding_plan_generation.rs::planGenerator` → `PlanRuntime::generation_spec` / `PlanRuntime::plan_under_state`；
- `bindingAuto::runToGetExecInfo` → `PlanRuntime::execute_binding`；
- `bindingAuto::fillRecommendation` → `PlanPerfPredictor::PerfPredicate`；
- `ruleBasedPlanPerfPredictor::PerfPredicate` → `IsSimplePointPlan`。

Rust 标准库层面只直接依赖 `std::sync::Arc`。本文件引用的其余类型全部来自当前 crate。`pkg/bindinfo/Cargo.toml` 声明 crate 名为 `astersql-bindinfo`、入口为 `lib.rs`、关闭自动测试发现，并依赖本地 `astersql-parser`、`astersql-util-hint`、`astersql-util-parser` 及 `serde`/`serde_json`；不过这些 parser/serde 依赖不是由本文件直接调用，而由 crate 内其他模块使用。`[package.metadata.porting] go-package = "pkg/bindinfo"` 明确了 Go 对照目录。

## 错误处理与边界

- `ExplorePlansForSQL` 和所有运行时/生成器/预测器调用使用 `Result` 与 `?` 短路传播；任一步失败都不会返回部分候选。错误统一为 crate 根定义的 `BindError(String)`。
- 明确的本地输入错误只有空或全空白 `sqlOrDigest`。非空字符串究竟是 SQL 还是摘要、是否能解析，由 `PlanRuntime::historical_bindings` 和生成器负责。
- 删除态绑定被静默过滤。与 Go 版本不同，Rust 不会在绑定缺少计划摘要时现场计算摘要，也不会在单条统计查询失败时记录日志后跳过该绑定；Rust 对空摘要保留无统计候选，对运行时错误则整体返回错误。
- `fillRecommendation` 假设预测器得分与候选语义上按索引对齐。Rust 对短 `scores`/`explanations` 使用 `get`，不会像直接索引那样崩溃，但长度不一致会导致候选无法命中或理由为空；新增预测器仍应返回与候选等长的两个向量。
- 最高分从 `0.0` 起算，因此全零和全负分都不能产生推荐；`NaN`、无穷值及非 `[0,1]` 分数未显式拒绝。预测器实现应遵守 `PlanPerfPredictor` 文档中的 `0.0..=1.0` 约定。
- `IsSimplePointPlan` 是文本启发式，不解析计划树。它依赖首 token 命名并用 `contains` 匹配；格式或算子命名变化可能造成误判。空文本被明确判为非简单点查。

## 并发与资源生命周期

`PlanRuntime`、`BindingPlanEvolution`、`PlanGenerator` 和 `PlanPerfPredictor` 都要求 `Send + Sync`，默认演进对象可通过 `Arc` 跨线程共享。`newBindingAuto` 让演进器与计划生成器共享同一运行时 `Arc`；对象销毁时由引用计数自动回收，不存在本文件管理的裸资源或后台任务。

一次 `ExplorePlansForSQL` 调用内部是同步串行的：历史查询、候选生成、逐个执行缺统计候选、两级预测依序发生。`runToGetExecInfo` 没有并发执行，也没有本地超时、取消或回滚机制；执行到第一个错误即停止，之前已写入切片的统计仍存在于栈上但不会随 `Err` 返回给调用者。超时、会话恢复和查询取消必须由 `PlanRuntime::execute_binding` 的实现保证。

本文件本身没有锁、通道、事务或会话池。独立测试中的 `RecordingRuntime` 使用 `Mutex<Vec<String>>` 只是为了跨共享引用记录执行顺序，不代表生产实现的同步策略。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/bindinfo/binding_auto.go`，总体骨架一致：`BindingPlanInfo`、`BindingPlanEvolution`、`bindingAuto`、`newBindingAuto`、`ExplorePlansForSQL`、`runToGetExecInfo`、`getBindingPlanInfo`、`fillRecommendation`、`getPlanExecInfo`、`planExecInfo` 和 `IsSimplePointPlan` 均有对应物。两版都先取历史候选、再生成候选；analyze 只试跑生成候选；推荐先规则后 LLM；简单点查白名单及空计划为 false 的语义一致。

关键迁移差异如下：

- Go 直接持有 `DestroyableSessionPool`，在本文件中解析/规范化 SQL、读系统表、查询 statements stats、切换并恢复会话变量及执行绑定；Rust 把这些能力全部委托给 `PlanRuntime`。所以 Rust 的正确性取决于适配器是否复现 Go 的 SQL/digest 判别、默认库规范化、统计表选择和会话恢复。
- Go 遇到缺失 `PlanDigest` 会调用优化器补算；补算或统计查询失败时记录日志并跳过该绑定。Rust 对缺摘要保留候选但不查统计，其他运行时错误整体向上传播。
- Go 的 analyze 执行临时关闭 plan baselines，并用 defer 恢复当前库和开关；Rust 本文件没有可见的等价操作，它属于 `execute_binding` 实现必须承担的契约风险。
- Go 在生产路径直接从会话收集耗时、扫描键和返回行；Rust 由运行时返回 `PlanExecInfo` 后统一换算。
- Go 用数组直接索引预测器输出，默认要求等长；Rust 对缺项更防御，但仍没有显式验证长度。

`pkg/bindinfo/binding_auto_test.rs` 说明迁移状态不是“所有 Go 集成测试均已原生 Rust 化”：文件前半保留了一大段不执行的 Go 测试草稿，实际可执行测试包括简单点查冒烟测试、`analyze_executes_only_generated_candidates_like_go` 和 `non_positive_historical_exec_count_leaves_candidate_stats_empty_like_go`。Go 的 `binding_auto_test.go` 仍提供更完整的 `EXPLAIN EXPLORE`、提示生成、analyze、verify-and-bind 和多种计划文本边界证据。

## 扩展指南

- 新增运行时数据源或改变 SQL/digest 解析时，优先扩展 `PlanRuntime` 实现而非把会话、存储或优化器依赖塞回 `binding_auto.rs`；同步核对 Go `getBindingPlanInfo` 的规范化、缺摘要与容错语义。
- 新增候选生成策略应接入 `PlanGenerator::Generate` 或 `binding_plan_generation.rs`，保持本文件“先历史、后生成、仅试跑生成候选”的编排不变量；若要改变该不变量，必须同步更新 `binding_auto_test.rs` 中 analyze 执行范围测试。
- 新增预测器时应返回与候选等长的分数和解释，定义零分、负分、`NaN` 和并列行为，并决定它在规则/LLM 回退链中的位置。注意预测器可重排可变切片，分数必须与重排后的索引一致。
- 扩充简单计划白名单时修改 `IsSimplePointPlan`，并把 Go `TestIsSimplePointPlan` 的完整正反计划形状移植到独立的 `binding_auto_test.rs`；不要把测试内嵌进生产文件。对树形前缀、表头变化和名字包含关系应增加回归用例。
- 修改执行统计时集中调整 `PlanExecInfo` 与 `apply_exec_info`，覆盖 `ExecCount <= 0`、`ResultRows == 0`、大整数转 `f64` 的精度及运行时返回异常值；同步检查规则预测器的排序和 50% 阈值。
- 若补齐完整应用接线，需要从 `bindingHandle.BindingPlanEvolution` 的上层语句执行入口建立并验证调用边；当前索引只证明构造接线和测试调用，不能把 Go 的 `EXPLAIN EXPLORE` 端到端覆盖视为 Rust 已验证事实。
- 兼容风险主要在 Go/Rust 的错误粒度、缺摘要处理和会话状态恢复；性能风险主要在 analyze 串行执行全部无统计新候选、候选搜索规模及同步预测延迟。

## 验证依据

- 源码：`pkg/bindinfo/binding_auto.rs`（328 行），逐项核对全部结构体、trait、函数、impl、别名及无条件编译事实。
- crate 与装配：`pkg/bindinfo/Cargo.toml`、`pkg/bindinfo/lib.rs`、`pkg/bindinfo/binding_handle.rs`。
- 直接下游：`pkg/bindinfo/binding_plan_generation.rs`、`pkg/bindinfo/binding_plan_evolution.rs`。
- Rust 独立测试：`pkg/bindinfo/binding_auto_test.rs`；可执行用例验证简单点查正反例、analyze 只执行生成候选、非正历史执行次数保持空统计。该文件也明确标注其大部分 Go 迁移草稿不参与编译。
- Go 对照与测试：`pkg/bindinfo/binding_auto.go`、`pkg/bindinfo/binding_auto_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/bindinfo/binding_auto.rs` 列出完整源码及 5 个使用文件；`query/node` 核对了 `ExplorePlansForSQL`、`newBindingAuto`、`IsSimplePointPlan`、`fillRecommendation`、`runToGetExecInfo`、`getBindingPlanInfo`。关键图边包括 `NewBindingHandle → newBindingAuto`、`ExplorePlansForSQL → getBindingPlanInfo/runToGetExecInfo/Generate`、`runToGetExecInfo → execute_binding/apply_exec_info`，以及两个可执行测试对 `ExplorePlansForSQL` 的调用。
- 验证限制：本任务按计划不运行 Cargo；因此这里只验证源码结构、图索引、Go 对照和现有测试意图，不声称运行时适配器或 `EXPLAIN EXPLORE` 端到端行为已在本任务中执行。仓库说明提到的 `.agents/skills/tidb-verify-profile` 在当前工作区不存在，无法额外执行其 Ready 配置；任务文件指定的精确结构验证作为交付门槛。
