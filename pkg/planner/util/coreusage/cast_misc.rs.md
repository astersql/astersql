# `pkg/planner/util/coreusage/cast_misc.rs`

## 文件定位

本文件属于 `astersql-planner-util-coreusage` crate，是 planner 在物理聚合计划上统一处理聚合参数类型的轻量入口。crate 根 `pkg/planner/util/coreusage/lib.rs` 以私有模块 `cast_misc` 装载它，再通过 `pub use cast_misc::*` 导出 `WrapCastForAggFuncs`。生产侧 `pkg/planner/core/Cargo.toml` 将该 crate 命名为 `coreusage-dependency`，`pkg/planner/core/optimizer_runtime.rs` 在聚合下推 projection 的处理流程中调用此入口。

它不负责推导聚合返回类型，也不直接选择或构造具体 CAST 表达式；这些职责位于 `aggregation::AggFuncDesc::WrapCastForAggArgs`（`pkg/expression/aggregation/base_func.rs`）。本文件只在一组聚合描述符上执行阶段模式门控与批量委托。

## 核心职责

`WrapCastForAggFuncs` 保证仍接收原始输入值的聚合阶段，其参数先转换为聚合描述符要求的输入/返回求值类型；而接收中间聚合结果的 `FinalMode`、`Partial2Mode` 不重复包 CAST。这个约束使后续 projection 注入能够看到新产生的标量 CAST 表达式，并把它们物化到聚合算子下方。

职责边界有两层：本文件只判断 `AggFuncDesc.Mode`；具体函数是否完全不需要 CAST、空参数如何处理、返回类型到 CAST 种类的映射、特殊窗口函数第二参数以及 NULL 参数的跳过逻辑，都由 `AggFuncDesc::WrapCastForAggArgs` 处理。

## 主要符号

- `pub fn WrapCastForAggFuncs(context: &dyn expression::BuildContext, aggregate_functions: &mut [aggregation::AggFuncDesc])`：文件内唯一生产符号。它接收只读构建上下文和可变切片，因此可同时处理 `Vec<AggFuncDesc>`、数组或其他连续切片，而不取得集合所有权。
- `expression::BuildContext`：向下游 CAST 构造提供求值上下文；本函数本身不读取其中状态。
- `aggregation::AggFuncDesc`：每项携带 `Mode`、`Args`、`RetTp` 等聚合描述信息。本函数只直接读取 `Mode`，并可能通过方法调用原地改写 `Args`。
- `aggregation::FinalMode` / `aggregation::Partial2Mode`：两个跳过值；其参数已经是前一阶段产生的中间结果类型。

文件没有模块级常量、自定义类型、trait、`impl`、条件编译项或私有辅助函数。

## 执行流程

1. 按切片顺序逐个取得聚合描述符的可变引用。
2. 比较当前描述符的 `Mode`：若为 `FinalMode` 或 `Partial2Mode`，保持描述符不变并继续下一项。
3. 对其他模式（当前测试明确覆盖 `CompleteMode` 与 `Partial1Mode`）调用 `aggregate_function.WrapCastForAggArgs(context)`。
4. 下游方法先处理空参数和无需转换的聚合函数集合，再依据 `RetTp.EvalType()` 选择整数、实数、字符串、十进制、时间、时长、JSON 或向量 CAST，并原地替换适用的 `Args`。
5. 在生产调用点 `optimizer_runtime.rs` 中，调用之后会重新扫描聚合参数中的 `ScalarFunction`；新包装的 CAST 因而可以触发并参与聚合下方 projection 的构建。

整个批处理不返回新集合，也不短路：某个描述符被跳过不影响后续描述符。

## 数据与状态

输入集合的长度、元素顺序、聚合名称、模式、返回类型、`DISTINCT` 标志及 `OrderByItems` 均不由本函数改变。可观察变更限于非 `FinalMode`/`Partial2Mode` 描述符经下游方法改写其 `Args`；每个适用参数可能从原表达式变为以该表达式为子节点的 CAST 表达式。

本文件没有全局或静态可变状态，没有缓存，也不保留 `context` 或描述符引用。批处理的额外空间由下游创建的新表达式决定；本层遍历为聚合描述符数量的线性复杂度，下游还会线性遍历每个描述符的参数。

关键不变量是阶段幂等边界：`FinalMode` 与 `Partial2Mode` 必须原样保留参数类型，因为它们消费的是 partial 结果；其他模式是否最终产生 CAST，还要服从下游的空参数、函数白名单、特殊位置和 NULL 类型规则。

## 依赖与调用关系

上游生产调用边为 `pkg/planner/core/optimizer_runtime.rs` 中的 `coreusage_dependency::WrapCastForAggFuncs(context.GetExprCtx(), &mut aggregate.AggFuncs)`。对应 Go 主链位于 `pkg/planner/core/rule_inject_extra_projection.go` 的 `InjectProjBelowAgg`：先调用 `coreusage.WrapCastForAggFuncs`，再检测标量表达式并按需注入 projection。Rust 的调用点同样位于聚合参数重映射之后、标量表达式检测之前。

直接下游调用边是 `aggregation::AggFuncDesc::WrapCastForAggArgs`（`pkg/expression/aggregation/base_func.rs`）。该方法继续调用 `noNeedCastAggFuncs` 和 `castAggArg`，后者再落到 expression crate 的各种 `WrapWithCastAs*` / `BuildCastFunction`。

`pkg/planner/util/coreusage/Cargo.toml` 声明本 crate 直接依赖 `aggregation` 与 `expression`；同一 manifest 中的 `base`、`logicalop`、`types` 主要服务该 crate 的另一实现文件 `correlated_misc.rs` 或测试，不是本文件的直接源码依赖。`pkg/planner/core/Cargo.toml` 以路径依赖接入本 crate；planner case 测试也以 `astersql-planner-util-coreusage` 路径依赖直接验证公开函数。

RustCodeGraph 对精确函数图命令未返回显式边，但文件使用关系与源码检索共同确认了上述生产调用和测试调用；因此这里不把空图输出误写成“没有调用者”。

## 错误处理与边界

本函数签名不返回 `Result`，自身没有显式错误分支。空切片自然成为 no-op；`FinalMode` 和 `Partial2Mode` 也不会触碰可能尚未满足下游前置条件的 `RetTp`。

对其他模式，错误/异常边界来自 `WrapCastForAggArgs`：空 `Args` 或白名单聚合直接返回；NULL 类型参数和 `LEAD`、`LAG`、`NTH_VALUE` 的第二参数被跳过；缺失 `RetTp` 会因 `expect` panic，不支持的 `EvalType` 也会 panic。调用方因此必须在返回类型已经推导完成后使用本入口。CAST 构造没有通过本函数传播可恢复错误。

切片元素是值类型 `AggFuncDesc`，与 Go 版本的指针切片不同，不存在空描述符元素；这消除了 Go 理论上的 nil 元素解引用边界，但不改变模式判断语义。

## 并发与资源生命周期

本函数同步执行，不创建线程、异步任务、通道、锁、事务或外部资源。`&mut [AggFuncDesc]` 在类型层面要求调用期间独占该切片，避免同一批描述符被并发改写；`&dyn BuildContext` 仅在调用栈内借用并立即传给下游。

新 CAST 表达式的生命周期归属各 `AggFuncDesc.Args`：旧表达式被 clone 后作为新 CAST 节点的输入，下游用新表达式替换切片中的参数槽位。函数返回后不保留额外借用，资源随描述符及其表达式树正常释放。

## 与 Go 版本的对应关系

Go 对照为 `pkg/planner/util/coreusage/cast_misc.go::WrapCastForAggFuncs`。两版都按输入顺序遍历全部描述符，只对既非 `FinalMode` 又非 `Partial2Mode` 的项调用 `WrapCastForAggArgs`，且都原地修改调用方持有的描述符。

表示层差异是：Go 参数为 `expression.BuildContext` 与 `[]*aggregation.AggFuncDesc`，Rust 参数为 trait object 引用与 `&mut [AggFuncDesc]`；Rust 的借用和非空值切片表达了更强的所有权/空值约束。Rust 使用 `lib.rs` 再导出公开符号，Go 则直接通过 package 导出。

测试与 Go 意图相符但需分开理解：`pkg/planner/core/casetest/rule/rule_inject_extra_projection_test.rs` 复刻 Go 的 `DISTINCT × SUM × mode × 参数类型` 矩阵；Go 原测试及 Rust 复刻测试的 mode 数组都把第四项写成了重复的 `Partial1Mode`，所以该矩阵本身没有实际执行 `Partial2Mode`。`pkg/planner/util/coreusage/coreusage_aster_unit_test.rs::aggregate_cast_skips_only_final_and_partial2_modes` 另行以四个不同模式直接补足了 `Partial2Mode` 门控证据。

## 扩展指南

若新增聚合执行模式，首先判断它消费原始行还是 partial 中间结果，再修改 `WrapCastForAggFuncs` 的模式门控；必须同步扩展 `coreusage_aster_unit_test.rs` 的模式断言，并避免只改 planner case 测试中当前重复的模式数组而遗漏逐模式行为验证。

若新增聚合函数、返回求值类型或特殊参数位置，通常应修改 `pkg/expression/aggregation/base_func.rs` 的 `noNeedCastAggFuncs`、`AggCastKind`、`castAggArg` 或 `WrapCastForAggArgs`，而不是把具体 CAST 逻辑塞进本文件。相应测试应放在独立的 `*_test.rs` 文件；本仓库约定不把 Rust 测试嵌入生产源文件。

修改时重点检查三类风险：阶段判断错误会对 partial 结果二次转换，造成兼容性或精度问题；目标类型映射错误会改变 SQL 类型语义；无条件新增 CAST 会增加 projection 节点和表达式求值成本。应同时核对 Go 同路径实现、planner 的 projection 注入调用顺序，以及 Complete/Partial1/Final/Partial2 的类型保持断言。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录中的 `cast_misc.rs` 被识别为含 2 个索引节点的 Rust 文件；索引时间戳为本次分析时可用状态。
- RustCodeGraph `files --filter pkg/planner/util/coreusage`、`node --file .../cast_misc.rs` 与 `node ...WrapCastForAggArgs`：确认唯一公开函数、crate 再导出、下游方法源码及其边界。
- RustCodeGraph 精确 `callers`/`callees` 查询未输出边；随后用目标符号源码检索确认 Rust 生产调用 `pkg/planner/core/optimizer_runtime.rs:11578`、同 crate 单测调用 `pkg/planner/util/coreusage/coreusage_aster_unit_test.rs:181`、planner case 测试调用 `pkg/planner/core/casetest/rule/rule_inject_extra_projection_test.rs:68`。
- 读取 `pkg/planner/util/coreusage/Cargo.toml`、`pkg/planner/core/Cargo.toml` 和 `pkg/planner/util/coreusage/lib.rs`：确认 crate 名、路径依赖、直接依赖及公开再导出；目标包目录不存在 `doc.go`。
- Go 对照证据：`pkg/planner/util/coreusage/cast_misc.go` 与生产调用 `pkg/planner/core/rule_inject_extra_projection.go:122`。
- 测试证据：`pkg/planner/util/coreusage/coreusage_aster_unit_test.rs::aggregate_cast_skips_only_final_and_partial2_modes`；`pkg/planner/core/casetest/rule/rule_inject_extra_projection_test.rs::test_wrap_cast_for_agg_funcs`；Go 对照 `pkg/planner/core/casetest/rule/rule_inject_extra_projection_test.go::TestWrapCastForAggFuncs`。
- 本任务为只读行为分析与文档新增，未运行 Cargo；最终以任务指定的 11 章节结构命令验证文件形态，并人工复核所有行为陈述均能回指上述源码、配置或测试。
