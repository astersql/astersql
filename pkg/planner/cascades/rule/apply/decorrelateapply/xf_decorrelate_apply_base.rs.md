# `pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_apply_base.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-rule-apply-decorrelateapply` 子 crate；crate 入口 `pkg/planner/cascades/rule/apply/decorrelateapply/lib.rs` 将它声明为公开模块并整体再导出。它位于 Cascades 逻辑变换规则层，服务于 Apply 解相关规则，但当前职责不等同于完整优化器运行时：文件头明确称其为“机械迁移阶段的自包含桩”，用最小的 Pattern、Rule、逻辑计划和 memo 组表达式模型支撑共享预检以及独立单元测试。

直接使用者是相邻的 `xf_decorrelate_simple_apply.rs`。后者把 `XFDeCorrelateApplyBase` 组合进 `XFDeCorrelateSimpleApply`，复用本文件的轻量模型执行测试路径；与此同时，它另行实现 `cascades_rule::Rule`，通过真实的 `BoundPlan`、`logicalop::LogicalApply` 和 `coreusage` 接入 Cascades。因而，本文件是移植语义的共享底座和测试模型，不应被误写为生产优化器中全部 Apply 解相关能力的实现。

## 核心职责

1. 用 `XFDeCorrelateApplyBase` 保存规则元数据，并在 `pre_check`/`PreCheck` 中拒绝设置了 `no_decorrelate` 的 Apply。
2. 定义 `XF_DECORRELATE_SIMPLE_APPLY_ID` 和 `APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG`，分别表达简单解相关规则 ID 与“由解相关规则生成的中间 Apply”标志。
3. 提供轻量规则模型：`Operand`、`Engine`、`Pattern`、`BaseRule`、`Rule`。
4. 提供轻量逻辑计划模型：`Schema`、`LogicalNode`、`LogicalJoin`、`LogicalApply`、`LogicalPlan`、`GroupExpression`，让相邻规则可以在不依赖完整 Memo 运行时的路径上验证变换语义。
5. 通过 `extract_correlated_columns` 从内层节点的相关列中筛出属于外层 schema 的列，作为“Apply 是否仍相关”的判定输入。
6. 通过 `LogicalJoin::realloc_for_cascades` 模拟 Apply 改写为 Join 后必须发生的元数据重置：分配新 `plan_id`、把类型改成 `Join`、清除旧统计。

## 主要符号

- `XF_DECORRELATE_SIMPLE_APPLY_ID: usize = 2`：轻量规则模型使用的固定 ID；相邻真实实现则以 `cascades_rule::XFDeCorrelateSimpleApply` 作为规则类型。
- `APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG: u64`：位 0 标志。`LogicalApply::has_flag` 用按位与检测它；相邻 `xform` 据此决定成功变换后是否从 memo 移除旧中间 Apply。
- `Error` 与 `Result<T>`：仅包装字符串的轻量错误类型。当前本文件不主动产生错误；相邻轻量 `xform` 在 Apply 孩子数不是 2 时用它返回错误。
- `Operand::{Apply, Any}`、`Engine::TiDbOnly`、`Pattern`：描述 `Apply(Any, Any)` 且只适用于 TiDB 引擎的最小模式树。`Pattern::set_children` 直接替换孩子列表。
- `BaseRule { id, pattern }`：轻量规则元数据；`XFDeCorrelateApplyBase.base_rule` 持有它。
- `Schema::contains`：以字符串精确相等判断输出列是否存在；该模型没有列 ID、限定名或类型系统。
- `LogicalNode`：唯一直接携带 `correlated_columns` 的轻量计划变体。
- `LogicalJoin`：保存计划 ID、类型名、schema、孩子和可选统计。`shallow_ref` 当前实际调用 `clone`，因此在此轻量模型中是完整值克隆，而不是共享引用。
- `LogicalJoin::realloc_for_cascades`：用函数内静态 `AtomicU64` 分配 ID，随后设置 `plan_type = "Join"` 并清空 `statistics`。
- `LogicalApply`：在 `LogicalJoin` 外增加 `no_decorrelate` 与 `flags`。
- `LogicalPlan`：`Apply`、`Join`、`Node` 三类枚举；`schema` 对三类统一取输出 schema，`correlated_columns` 只对 `Node` 返回实际数据，`as_apply` 执行安全枚举判别。
- `GroupExpression`：包装一个 `LogicalPlan` 和递归孩子；`wrapped_logical_plan` 只返回计划引用。
- `Rule` trait：约定 `id`、`base_rule`、`pre_check`、`xform`。本文件只声明接口；`xf_decorrelate_simple_apply.rs` 为轻量规则实现它。
- `XFDeCorrelateApplyBase::pre_check`/`PreCheck`：要求根计划是 Apply，并返回 `!apply.no_decorrelate`；大写版本只是 Go 风格兼容别名。
- `extract_correlated_columns`：保持内层相关列的原顺序和重复项，只克隆那些被外层 schema 精确包含的字符串。

## 执行流程

轻量测试路径的完整流程跨本文件与 `xf_decorrelate_simple_apply.rs`：

1. `new_xf_decorrelate_simple_apply` 构造根为 `Operand::Apply`、两个孩子均为 `Operand::Any`、引擎均为 `TiDbOnly` 的 `Pattern`，再把它连同规则 ID 放进本文件的 `BaseRule` 和 `XFDeCorrelateApplyBase`。
2. 规则匹配后，`Rule::pre_check` 委托给 `XFDeCorrelateApplyBase::pre_check`。该函数经 `wrapped_logical_plan().as_apply()` 取得 Apply；若并非 Apply 会立即 panic，若是 Apply 则仅在 `no_decorrelate == false` 时放行。
3. 相邻轻量 `xform` 要求 `GroupExpression.children` 恰有两个，依次作为 outer 与 inner；数量错误返回 `Error`。
4. 它读取 Apply 的中间节点标志，然后调用本文件的 `extract_correlated_columns(inner, outer.schema())`。筛选结果非空表示内层仍引用外层输出，规则不产出替代计划，也不请求删除原式。
5. 筛选结果为空时，规则通过 `LogicalJoin::shallow_ref` 复制 Apply 内嵌的 Join，再调用 `realloc_for_cascades` 生成新计划身份、改成 Join 并丢弃旧统计，最终输出一个 `LogicalPlan::Join`。若原 Apply 带中间标志，返回的 `remove` 为真。
6. 真实优化器路径不使用这些轻量计划类型。相邻文件的 `impl cascades_rule::Rule` 在 `BoundPlan` 上重复同一语义：检查 `NoDecorrelate`，用 `coreusage::ExtractCorColumnsBySchema4LogicalPlan` 判断相关性，克隆真实 `LogicalJoin`，恢复 schema/output names，并调用真实 `ReAlloc4Cascades`。

## 数据与状态

轻量模型全部以拥有所有权的 Rust 值组成。`Pattern.children`、`LogicalJoin.children` 和 `GroupExpression.children` 使用 `Vec`；schema 与相关列用 `Vec<String>`。克隆计划会克隆字符串与孩子树，因此不存在共享可变计划节点。

`LogicalApply.logical_join` 是 Apply 的公共 Join 骨架，承载输出 schema 与两个孩子；`no_decorrelate` 是独立布尔门禁；`flags` 是可扩展位集合。当前只有位 0 被定义。本文件没有维护相关列缓存，`extract_correlated_columns` 每次线性扫描内层相关列，并对每列在线性 schema 列表中查找，复杂度为 `O(inner_correlated × outer_columns)`。

唯一进程级可变状态是 `realloc_for_cascades` 内的 `NEXT_PLAN_ID: AtomicU64`。它从 1 开始、跨所有调用单调取值；使用 `fetch_add(1, Ordering::Relaxed)`，只保证原子唯一分配所需的修改顺序，不承诺与其他内存操作建立同步关系。整数溢出没有显式处理。

## 依赖与调用关系

crate 边界由同目录 `Cargo.toml` 定义，库入口是 `lib.rs`。该 manifest 的正常依赖包括 `cascades-pattern`、`cascades-rule`、`cascades-util`、`coreusage` 与 `logicalop`，但本文件自身仅使用标准库 `fmt` 和原子类型；这些工作区依赖主要由相邻真实规则桥接使用。`[target.'cfg(any())'.dev-dependencies]` 条件恒假，说明列出的旧测试依赖没有被当前 Cargo 测试目标启用。

上游关系：

- `lib.rs` 公开声明并再导出本模块。
- `xf_decorrelate_simple_apply.rs::new_xf_decorrelate_simple_apply` 构造 `Pattern`、`BaseRule` 与 `XFDeCorrelateApplyBase`。
- `impl Rule for XFDeCorrelateSimpleApply` 把 `base_rule`、`pre_check` 和轻量 `xform` 接到本文件的接口和类型上。
- `xf_decorrelate_apply_test.rs` 直接构造 `LogicalNode`、`LogicalJoin`、`LogicalApply` 和 `GroupExpression`，并通过 `rule.apply_base.PreCheck` 与 `rule.XForm` 验证行为。

下游关系：本文件没有调用外部 crate；内部调用链主要是 `pre_check → wrapped_logical_plan → as_apply`、`extract_correlated_columns → correlated_columns/Schema::contains`，以及相邻规则的 `xform → shallow_ref → realloc_for_cascades`。

RustCodeGraph 已索引本文件并识别 34 个符号，也识别相邻 Rust/Go 实现与测试文件；对 `extract_correlated_columns` 的精确查询返回本文件函数及真实 logicalop 中的同名语义函数。图工具的 callers/callees 查询在本次会话中超时，因此直接调用边以同目录源码引用搜索交叉核验，没有据此推断仓库外调用者。

## 错误处理与边界

- `XFDeCorrelateApplyBase::pre_check` 对非 Apply 根使用 `expect`，会 panic。这依赖上游 Pattern 已保证根 operand 是 Apply；它不是可恢复的输入校验 API。
- `LogicalPlan::correlated_columns` 对 Apply 和 Join 返回空切片。故轻量 `extract_correlated_columns` 只适用于把相关列放在 `LogicalNode` 的测试模型；它不能递归遍历任意计划树，也不能替代真实的 `coreusage` 提取器。
- 列匹配为区分大小写的字符串相等；同名、别名、作用域、列 ID 和类型均未建模。重复相关列不会去重。
- `Schema::contains` 与提取函数不返回错误；空 schema 或空相关列自然得到空结果，并使简单规则认为 Apply 可转为 Join。
- `shallow_ref` 名称沿用 Go 语义，但当前实现是深度 `clone`。扩展者不能假定克隆计划与原计划共享 schema/孩子身份。
- `realloc_for_cascades` 只模拟 ID、类型和统计重置。Go/真实 Rust 的 `ReAlloc4Cascades` 还涉及 self、task map 等完整计划因子；不要把轻量函数用于生产计划对象。
- 相邻轻量 `xform` 对孩子数错误返回 `Error`，对复制后无孩子使用 `assert!`；真实桥接则返回 `CascadesRuleError` 处理根类型或孩子数错误。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。所有 Pattern、计划和组表达式随普通 Rust 所有权与 `Vec` 生命周期释放，无显式清理步骤。

并发相关点仅为计划 ID 分配器：`AtomicU64` 让并发调用 `realloc_for_cascades` 不会因数据竞争取得同一次 fetch-add 的旧值；`Relaxed` 合理地表明 ID 分配不承担发布计划内容的同步职责。轻量 `LogicalPlan` 中没有 `Arc`/`Mutex`，计划树复制后相互独立。真实 memo 的插入、删除与生命周期由相邻 `cascades_rule::Rule` 的调用方管理，不在本文件内。

## 与 Go 版本的对应关系

Go 同路径 `xf_decorrelate_apply_base.go` 的内容很小：`XFDeCorrelateApplyBase` 嵌入 `*rule.BaseRule`，并以 `PreCheck` 强制转换根计划为 `*logicalop.LogicalApply` 后返回 `!apply.NoDecorrelate`。Rust 的 `XFDeCorrelateApplyBase { base_rule }` 和 `pre_check`/`PreCheck` 与此语义直接对应；Go 的失败方式是类型断言 panic，Rust 的失败方式是 `expect` panic。

Rust 文件明显比 Go 基类更宽：`Error`、Pattern、Rule、Schema、轻量计划枚举、组表达式、原子 ID 分配以及相关列提取都不是 Go 基类文件的逐项翻译，而是机械迁移阶段为自包含测试引入的桩。Go 的真实类型分别来自 `cascades/rule`、`cascades/pattern`、`core/base`、`logicalop` 与 `coreusage`；Rust 的真实对应依赖也已在 Cargo manifest 中声明，并由 `xf_decorrelate_simple_apply.rs` 的 `impl cascades_rule::Rule` 使用。

相邻 Go `xf_decorrelate_simple_apply.go` 提供行为对照：模式为 `Apply(Any, Any)`；检查内层相对外层的相关列；无相关列时浅拷贝 Join 并重新分配 Cascades 因子；根据中间 Apply 标志决定是否删除源 memo 表达式。当前 Rust 轻量路径保留这一控制流，但其字符串列模型和简化重分配不具有 Go 生产类型的全部语义。Go 回归测试 `xf_decorrelate_apply_test.go::TestXFDeCorrelateShouldDeleteIntermediaryApply` 当前开头即 `t.Skip`；Rust 独立测试另有可执行的轻量断言，同时保留了 Go 测试参考文本。

## 扩展指南

- 若只增加 Apply 解相关规则，共享门禁仍应集中在 `XFDeCorrelateApplyBase::pre_check`；规则专有的 Pattern 和 `xform` 应放在独立规则文件，并同时实现或更新真实 `cascades_rule::Rule` 路径，避免只让轻量测试通过。
- 若新增标志，在 `LogicalApply::flags` 上分配不冲突的位，补充 `has_flag` 使用场景，并与 `logicalop` 中真实标志保持一致；测试应覆盖标志存在和不存在两条路径。
- 若扩充相关列语义，不宜继续堆叠字符串特例。应优先复用或对齐 `coreusage::ExtractCorColumnsBySchema4LogicalPlan` 与真实 schema/列 ID 行为，并明确递归、去重、顺序和作用域规则。
- 若修改计划复制或重分配，必须同时检查轻量 `LogicalJoin::{shallow_ref,realloc_for_cascades}` 和相邻真实桥接中的 `LogicalJoinShallowRef`、schema/output names 恢复及 `ReAlloc4Cascades`，防止测试模型与生产路径漂移。
- 单元测试继续放在独立的 `xf_decorrelate_apply_test.rs`，不要内嵌到生产源文件。至少同步覆盖：`no_decorrelate` 门禁、错误根类型/孩子数、相关列存在与不存在、孩子与 schema 保留、统计清空、计划 ID 变化、中间 Apply 删除标志。Go 语义变化时也应核对同目录 Go 实现和测试。
- 性能风险集中在字符串克隆与二重线性查找；若轻量模型用于更大输入，可考虑集合化 schema，但必须保留输出顺序和重复项语义，或先明确改变契约。

## 验证依据

- 目标源码：`pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_apply_base.rs`，核对了全部 282 行以及常量、类型、trait、函数和 impl。
- crate 与模块边界：同目录 `Cargo.toml`、`lib.rs`。
- Rust 直接入口与真实桥接：`xf_decorrelate_simple_apply.rs`，重点核对构造函数、轻量 `Rule` 实现和真实 `cascades_rule::Rule` 实现。
- Rust 独立测试：`xf_decorrelate_apply_test.rs`，核对无相关列 Apply→Join、孩子保留、统计清空、中间 Apply 删除、相关 Apply 保留和规则字符串断言。
- Go 对照：`xf_decorrelate_apply_base.go`、`xf_decorrelate_simple_apply.go`、`xf_decorrelate_apply_test.go`；后者的主回归当前被 `t.Skip` 跳过。
- RustCodeGraph：`status` 显示索引包含本文件；`files --filter .../decorrelateapply` 定位相邻 Rust/Go 文件；`query XFDecorrelateApplyBase`（实际命中大小写为 `XFDeCorrelateApplyBase`）与 `query extract_correlated_columns` 核对主要符号；`node --file ... --offset 1 --limit 420` 读取完整源码。callers/callees 查询超时后，使用 `rg` 对同目录直接引用作补充核验。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证文档存在且固定二级标题恰为 11 个，并人工复核没有把轻量桩描述成完整生产实现。
