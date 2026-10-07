# `pkg/expression/explain.rs`

## 文件定位

`explain.rs` 是 `astersql-expression` crate 的表达式展示内核。`pkg/expression/lib.rs` 以私有模块 `explain_kernel` 挂载本文件，并通过 `pub use explain_kernel::*` 导出其中的公开自由函数；同一 crate 的 `core_impl.rs` 则把 `Expression` trait 的 `ExplainInfo`、`ExplainNormalizedInfo` 和 `ExplainNormalizedInfo4InList` 分派到本文件为 `Column`、`Constant`、`ScalarFunction` 提供的固有方法。因此它既负责单个表达式节点的递归文本化，也负责计划算子需要的表达式列表稳定化。

该文件不执行 SQL、生成物理计划或组装完整 EXPLAIN 行。它把已经构造好的表达式树变成普通文本或归一化文本；上游的投影、选择、聚合、连接和会话 EXPLAIN 逻辑再把这些片段放入计划信息。例如 `pkg/planner/core/operator/physicalop/physical_projection.rs::ExplainInfo` 使用 `ExplainExpressionListWithColumnNumbers`，`physical_selection.rs::ExplainInfo` 与 `pkg/session/runtime/explain_query.rs::GetArgs` 使用 `SortedExplainExpressionList`。

crate 边界由 `pkg/expression/Cargo.toml` 确定：crate 名为 `astersql-expression`，库入口为 `lib.rs`，并关闭自动测试发现。与本文件直接相关的类型和常量经 crate 根适配模块提供，包括 `chunk`、`errors`、`model`、`mysql`、`types`；对应的实际依赖包括 `astersql-util-chunk`、`astersql-meta-model`、`astersql-types` 和解析器 AST/MySQL crate。本源文件没有条件编译项；独立测试 `explain_test.rs` 由 `lib.rs` 在 `#[cfg(test)]` 下显式挂载。

## 核心职责

1. 为 `ScalarFunction` 递归生成普通、归一化和忽略 IN 列表细节三种说明，保留 CAST 返回类型以及物理表 ID `-1` 显示为 `dual` 的兼容特例。
2. 为 `Column` 选择原始列名、占位符或带上下文的列文本，并服从是否隐藏自动列编号的展示策略。
3. 为 `Constant` 求值并格式化 `Datum`，执行 `OFF`、`ON`、`MARKER` 三种脱敏模式，保留标量子查询引用 ID。
4. 为投影表达式输出 `表达式->输出列` 映射，并对普通列、关联列、常量和其他表达式采用各自的格式化路径。
5. 对条件列表的说明字符串排序后用逗号连接，使表达式原始顺序不稳定时仍得到稳定的 EXPLAIN 或计划摘要文本。

职责边界是“格式化现有对象”。表达式的求值实现、列字符串底层规则、Schema 构造、计划节点树形布局、计划摘要的全局策略开关均在其他文件中。本文件只读取这些对象和上下文，不持久化结果，也不修改表达式树。

## 主要符号

- `ScalarFunction::ExplainInfo(ctx)`：普通入口，要求有效 `EvalContext`，调用私有 `explainInfo(Some(ctx), false)`。
- `ScalarFunction::explainInfo(ctx, normalized)`：标量函数核心递归器。函数名作为前缀；`ast::In` 检查 `_tidb_tid = -1` 特例；`ast::Cast` 在每个参数后追加 `RetType.String()`；其他函数按原参数顺序递归。
- `ScalarFunction::ExplainNormalizedInfo()`：无上下文的归一化入口，最终把常量变为 `?`。
- `ScalarFunction::ExplainNormalizedInfo4InList()`：为计划摘要忽略长 IN 列表；顶层 `IN` 直接写成 `in(...)`，其他函数继续递归，CAST 仍附返回类型。
- `Column::ColumnExplainInfo(ctx, normalized)` / `ColumnExplainInfoNormalized()`：普通模式调用 `StringWithCtxForExplain`；归一化模式优先使用 `OrigName`，缺失时返回 `?`。三个 trait 对应方法都由它们派生。
- `Constant::ExplainInfo(ctx)`：读取会话脱敏模式；必要时求值、调用 `formatDatum`，并为 `SubqueryRefID > 0` 包裹 `ScalarQueryCol#ID(...)`。
- `Constant::formatDatum(datum)`：`NULL` 输出大写字面量；字符串、bytes、enum、set、JSON、binary literal、bit 加双引号；其他类型使用 `TruncatedStringify()`。
- `writeRedact(builder, value, mode)`：本文件私有的列表常量脱敏助手；`MARKER` 包裹 `‹…›`，`ON` 写 `?`，其他模式原样写入。
- `ExplainExpressionList(...)`：按上下文计算列号隐藏策略，然后转发到显式策略版本。
- `ExplainExpressionListWithColumnNumbers(...)`：投影列表核心入口。按索引配对 `exprs[index]` 与 `schema.Columns[index]`，并在展示文本不同时追加 `->目标列`。
- `SortedExplainExpressionList`、`SortedExplainExpressionListIgnoreInlist`、`SortedExplainNormalizedExpressionList`、`SortedExplainNormalizedScalarFuncList`：四种公开稳定排序入口。
- `sortedExplainExpressionList(ctx, exprs, normalized, ignore_in_list)`：统一选择格式化模式、字典序排序并生成逗号分隔 `Vec<u8>`。
- `ExplainColumnList(ctx, columns)`：按输入顺序输出逗号分隔的普通列说明，不排序。

文件没有模块级常量、结构体、枚举或 trait 定义；公开 API 由上述固有方法和自由函数组成，内部实现仅有 `ScalarFunction::explainInfo`、`Constant::formatDatum`、`writeRedact` 与 `sortedExplainExpressionList`。

## 执行流程

单个标量函数的普通说明从 `ScalarFunction::ExplainInfo` 进入。`explainInfo` 先验证“非归一化时必须有上下文”，写入小写函数名和左括号。若函数是二元 `IN`，第一个参数的归一化文本以 `model::ExtraPhysTblIDName.L` 结尾、第二个参数确为值为 `-1` 的 `Constant`，就提前返回 `in(<物理表 ID 列>, dual)`。否则 CAST 对每个参数递归格式化并追加返回类型，其他函数只递归参数；最后补右括号。

归一化流程复用相同递归器，但不读取求值上下文。`Column` 使用 `OrigName` 或 `?`，`Constant` 始终使用 `?`，因而输出不携带具体常量值。忽略 IN 列表流程走另一入口：遇到当前 `ScalarFunction` 为 `IN` 时不遍历其参数，直接输出 `...`；这使不同长度或取值的 IN 列表产生相同的摘要形状。

常量普通说明先读取 `ctx.GetTiDBRedactLog()`。`ON` 模式不求值，直接返回 `?`，但子查询引用仍保留 `ScalarQueryCol#ID` 外壳。其他模式以空 `chunk::Row` 调用 `Constant::Eval`，将成功的 `Datum` 交给 `formatDatum`；`MARKER` 再包裹标记字符。求值失败不向上返回错误，而是降级为固定文本 `not recognized const value`。

投影列表流程由 `ExplainExpressionListWithColumnNumbers` 驱动：

1. 用相同索引取得表达式和目标 Schema 列，并先生成目标列文本。
2. `Column` 或 `CorrelatedColumn` 经 `as_any` 下转后统一取普通列引用，生成源列文本；只有源、目标显示不同才写 `源->目标`。
3. `Constant` 先以禁用脱敏的模式取得原字符串，再由 `writeRedact` 按调用方模式处理，随后无条件追加目标列。
4. 其他表达式使用自身 `StringWithCtx`，随后无条件追加目标列。
5. 各项以 `, ` 分隔，保持投影和 Schema 的原顺序。

排序列表流程在 `sortedExplainExpressionList` 中根据优先级选择文本：`ignore_in_list` 优先于 `normalized`，否则生成普通说明。所有文本收集到临时向量后执行字典序 `sort()`，再以 `, ` 连接并转为字节向量。公开包装函数只负责选择这三个模式以及是否传入上下文。

## 数据与状态

本文件读取的主要状态来自表达式节点和调用上下文。`ScalarFunction` 提供 `FuncName`、参数和可选 `RetType`；`Column` 提供 `OrigName`、唯一 ID 等显示信息；`Constant` 提供 `Value`、`RetType` 及 `SubqueryRefID`；`Schema.Columns` 提供投影目标列。动态表达式以 `ExprBox = Box<dyn Expression>` 保存，通过 trait 的说明方法递归，通过 `as_any` 区分投影列表中的具体类型。

`EvalContext`/`ParamValues` 只读提供常量求值、脱敏配置及列展示策略。`shouldRemoveColumnNumbers` 当前实现在 `pkg/expression/column.rs`，本文件只是调用它；显式入口允许计划层传入已经从 statement context 计算出的 `remove_column_numbers`，避免渲染时上下文状态已经恢复而丢失 `plan_tree` 格式选择。

输出全部是当前调用新建的 `String` 或 `Vec<u8>`。排序入口会创建与表达式数量相同的字符串向量；投影入口逐项增长一个 `String`。除读取节点和上下文外没有可变共享状态、缓存或全局注册表。排序会改变临时说明数组的顺序，不会改变输入表达式数组。

## 依赖与调用关系

装配链为 `pkg/expression/Cargo.toml` → `pkg/expression/lib.rs` → 私有 `explain_kernel` → 本文件；`lib.rs` 随后再导出本文件的公开自由函数。trait 调用链为 `expression.rs::Expression` 的三个说明方法 → `core_impl.rs` 对 `Column`、`Constant`、`CorrelatedColumn`、`ScalarFunction` 的实现 → 本文件相应固有方法。`CorrelatedColumn` 的 trait 实现把说明委托给其内嵌 `column`。

RustCodeGraph 对 `ExplainExpressionListWithColumnNumbers` 的查询定位到本文件第 234 行，其 callees 包含本文件 `writeRedact` 和动态下转 `as_any`。对 `SortedExplainExpressionList` 的 callees 查询确认它只转入 `sortedExplainExpressionList`；后者再调用 `Expression` trait 的三种说明方法、排序和连接。图的文件节点还报告本文件被 14 个 Rust 文件使用；由于同名 Go/Rust 符号很多，callers 查询耗时过长，具体上游以源码引用搜索消歧。

已核对的直接 Rust 上游包括：

- `physical_projection.rs`：普通投影使用显式列号策略入口，归一化投影选择普通归一化或忽略 IN 列表排序。
- `physical_selection.rs`：普通过滤条件使用排序说明，归一化路径同样根据全局摘要策略选择模式。
- `base_physical_agg.rs`、`physical_hash_join.rs`、`physical_merge_join.rs`、`physical_index_join.rs`、`physical_table_scan.rs`、`physical_union_scan.rs`：生成聚合、连接、扫描等计划节点的条件片段。
- `pkg/session/runtime/explain_query.rs`：为扫描节点补充 pushed-down filter 文本。
- `pkg/planner/util/handle_cols.rs`：直接调用 `ColumnExplainInfo` 输出句柄列。

主要下游依赖是 `Expression` 的递归说明/字符串方法、`Constant::Eval`、`Column::StringWithCtxForExplain`、`Schema.Columns`、`Datum::Kind`/`TruncatedStringify`、AST 函数名和脱敏常量。本文件不调用 executor、KV、网络或磁盘接口。

## 错误处理与边界

本文件 API 不返回 `Result`。唯一可恢复的底层失败是 `Constant::Eval`：错误被有意转成 `not recognized const value`，所以 EXPLAIN 渲染仍能继续，但调用方无法取得原错误。归一化常量和完全脱敏常量不执行求值，不会触发该失败路径。

代码包含若干由上游结构保证的断言或潜在 panic：

- `ScalarFunction::explainInfo` 要求普通模式存在上下文；`sortedExplainExpressionList` 要求普通且不忽略 IN 列表时存在上下文。
- CAST 格式化对 `RetType` 调用 `unwrap()`；合法构造的标量函数必须已有返回类型。
- 投影列表以表达式索引直接访问 `schema.Columns[index]`，因此 Schema 列数必须不少于表达式数；函数不会自行验证长度。
- `ctx.expect("explain context")` 依赖入口模式断言，不提供错误返回。

`IN(_tidb_tid, -1)` 特例的 Rust 实现只在第二个参数能安全下转为 `Constant` 时生效；不匹配时继续普通格式化。Go 对照使用未检查的 `args[1].(*Constant)` 类型断言，错误形状可能 panic。Rust 因而在不改变合法输入输出的前提下收窄了异常输入风险。

脱敏有两条路径：单常量 `ExplainInfo` 从上下文取模式，投影列表由显式 `redact_mode` 控制。调用方若传入未知字符串，`writeRedact` 按关闭脱敏处理。归一化输出隐藏常量，但列 `OrigName`、函数名、返回类型和 `ScalarQueryCol#ID` 等结构信息仍可能出现；它不是对整段计划信息的通用安全清洗器。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、原子变量、通道、事务或外部资源句柄。输入均以共享借用传入；局部 `String`、字符串向量和 `Vec<u8>` 由当前调用独占，返回后所有权交给调用方，其他临时对象在函数退出时释放。因此文件自身没有同步职责，也不会跨调用保存上下文或渲染结果。

时间复杂度方面，单表达式格式化按表达式树递归访问节点；投影/列列表为线性遍历。排序列表若有 `n` 个顶层表达式，除递归格式化成本外还包含 `O(n log n)` 的字符串排序和总文本长度级别的连接成本。深层表达式会增加递归栈深度，长 IN 列表在普通/一般归一化模式下会增加输出和分配；`ExplainNormalizedInfo4InList` 正是用于避免计划摘要被长列表放大。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/explain.go`。Rust 保留了 Go 的主要符号和控制流：三类核心节点的说明方法、CAST 返回类型、`_tidb_tid/-1` 到 `dual`、列原名归一化、常量脱敏及字符串类加引号、投影箭头、排序稳定化、IN 列表折叠和列列表拼接均有逐项对应。

已确认的实现差异如下：

- Rust 新增 `ExplainExpressionListWithColumnNumbers`，把列号隐藏策略作为显式参数；`ExplainExpressionList` 仍保留 Go 入口语义并自动读取策略。该扩展被 Rust `physical_projection.rs` 用来保存 statement context 中的 `plan_tree` 选择。
- Go 的投影列表直接写入 `redact.WriteRedact`；Rust 在本文件以私有 `writeRedact` 复现三种模式。
- Go 的 IN/dual 特例对第二参数执行未检查类型断言；Rust 使用 `downcast_ref::<Constant>()` 和 `is_some_and`，异常形状退回普通输出。
- Go 用 `bytes.Buffer` 和 `[]byte`，Rust 用 `String` 和 `Vec<u8>`；可观察分隔符和排序结果保持一致。
- Go 的 `SortedExplainNormalizedScalarFuncList` 把指针切片装入接口切片；Rust 将标量函数克隆为表达式集合后走同一排序内核，以满足所有权模型。

Rust 独立测试 `pkg/expression/explain_test.rs::projected_generated_columns_keep_go_alias_arrow` 专门锁定不同自动生成列 ID 时必须输出 `Column#1->Column#2`，防止错误地只比较来源元数据而省略箭头。规划器测试 `physical_selection_test.rs` 和 `physical_projection_test.rs` 验证忽略 IN 列表模式确实区别于普通归一化模式。Go 仓库没有同路径 `explain_test.go`；相关 Go 行为证据分散在计划器测试，例如 `pkg/planner/core/logical_plans_test.go` 对 `ExplainExpressionList` 的常量/列映射输出断言。

## 扩展指南

新增表达式节点类型时，首先在独立的 `Expression` trait 实现文件中定义三种说明方法；若投影列表需要区别于“其他表达式”的显示行为，再扩展 `ExplainExpressionListWithColumnNumbers` 的 `as_any` 分派。测试应放在独立 `*_test.rs` 文件，并由 `lib.rs` 显式挂载，不能嵌入本源文件。

修改函数格式时，应同步核对 `ScalarFunction::explainInfo` 和 `ExplainNormalizedInfo4InList`，避免普通、归一化、摘要三种文本漂移。新增类似 CAST 的特殊格式必须覆盖嵌套参数、缺失返回类型前提和归一化输出；新增结构折叠规则必须确认是否影响计划摘要兼容性与缓存命中观察。

修改常量显示或脱敏时，要同时审查 `Constant::ExplainInfo`、`formatDatum`、投影列表的 `writeRedact` 以及 `StringWithCtx` 的既有规则。尤其要覆盖 `OFF`、`ON`、`MARKER`、`NULL`、字符串类、求值失败和 `SubqueryRefID`，避免普通节点显示与投影列表显示采用不同安全策略。

修改投影箭头时，应保留表达式和 Schema 等长的调用约束，并同步扩展 `pkg/expression/explain_test.rs`。若更改列号隐藏策略，还需检查 `physical_projection.rs` 的显式 statement-context 接线及相关计划树测试。修改排序或 IN 列表折叠会改变 EXPLAIN/plan digest 的稳定文本，应同步检查选择、投影以及所有使用 `SortedExplain*` 的物理算子测试，兼容性风险高于普通展示文案变更。

性能上，避免在每个递归层重复计算昂贵的上下文策略，避免为单个节点引入全列表复制；当前投影入口只计算一次列号策略，排序入口只建立一次顶层文本数组。任何新增敏感数据字段都必须明确普通、归一化与脱敏模式下是否可见。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/expression/explain.rs` 全部 328 行，重点核对 `ScalarFunction`、`Column`、`Constant` 的 impl，以及 `ExplainExpressionListWithColumnNumbers`、`sortedExplainExpressionList`、`ExplainColumnList`。
- crate 与装配：`pkg/expression/Cargo.toml`；`pkg/expression/lib.rs` 中 `explain_kernel`、`pub use explain_kernel::*` 和 `#[cfg(test)] explain_test`。
- trait 接线：`pkg/expression/expression.rs::Expression` 与 `pkg/expression/core_impl.rs` 中四类表达式的说明方法分派。
- Rust 直接调用者：`pkg/planner/core/operator/physicalop/physical_projection.rs`、`physical_selection.rs`、聚合/连接/扫描算子，以及 `pkg/session/runtime/explain_query.rs`。
- Rust 独立测试：`pkg/expression/explain_test.rs`；相关规划器测试 `physical_selection_test.rs`、`physical_projection_test.rs`。
- Go 对照：`pkg/expression/explain.go`；相关预期见 `pkg/planner/core/logical_plans_test.go`。仓库中不存在同路径 `pkg/expression/explain_test.go`。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`node --file pkg/expression/explain.rs` 返回完整源码并报告 14 个 Rust 使用文件；`query/callees` 核对了 `ExplainExpressionListWithColumnNumbers`、`SortedExplainExpressionList` 和 `sortedExplainExpressionList`。callers 查询因同名符号图遍历超过 90 秒后中止，故上游调用点使用 `rg` 与源码读取补齐并消歧。

本任务为纯文档分析，没有运行 Cargo 或代码测试。结构验证单独检查目标文档存在且恰好包含约定的十一个二级章节；人工复核覆盖了文件存在理由、执行路径、数据/错误/资源边界、Go 对应关系与安全扩展位置。
