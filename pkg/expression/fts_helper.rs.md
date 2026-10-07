# `pkg/expression/fts_helper.rs`

## 文件定位

本文件属于 `astersql-expression` crate；crate 由 `pkg/expression/Cargo.toml` 定义，根模块 `pkg/expression/lib.rs` 通过 `#[path = "fts_helper.rs"] mod fts_helper_kernel;` 将其编入。模块本身是私有模块，且 crate 根当前没有 `pub use fts_helper_kernel::*`，因此文件内虽声明了三个 `pub` 符号，它们目前仍只在该私有模块边界内可见。

它移植自同目录的 `pkg/expression/fts_helper.go`，目标角色是识别表达式树中的内部标量函数 `ast::FTSMatchWord`，以及把一个标准形状的全文检索表达式拆成查询文本和列引用。仓库搜索与 RustCodeGraph 的文件关系均未发现 Rust 调用者；当前 Rust 规划器主链 `pkg/planner/core/fts_resolve_index.rs` 使用自己的字符串计划表示和局部 `interpret_fts_expression`，并未接入这里基于 `Expression` trait 的 helper。因此，本文件当前是已编入但尚未接线的表达式辅助实现，不能据此宣称 Rust 规划器已经通过它完成 FTS 下推。

## 核心职责

文件只承担两类无副作用的结构检查：

1. `ContainsFullTextSearchFn` 从一个 `&dyn Expression` 开始，只沿 `ScalarFunction::GetArgs()` 递归，判断当前节点或任意标量函数后代的 `FuncName.L` 是否等于 `ast::FTSMatchWord`。
2. `InterpretFullTextSearchExpr` 只接受恰好为 `FTSMatchWord(Constant, Column)` 的节点，并生成便于规划阶段消费的 `FTSInfo`；任何形状不匹配都返回 `None`。

它不负责构造 FTS builtin、验证部署模式、寻找全文索引、生成 TiFlash 下推信息、执行检索或计算评分。这些职责分别位于诸如 `pkg/expression/builtin_fts.rs`、`pkg/expression/planner_bridge.rs` 和 `pkg/planner/core/fts_resolve_index.rs` 等实现中。

## 主要符号

- `pub struct FTSInfo<'a>`：成功解释后的轻量结果。`Query: String` 是从第一个常量参数复制出的查询文本；`Column: &'a Column` 借用第二个参数中的列节点，生命周期与输入表达式绑定，不取得所有权。字段名保留 Go 风格，以对齐 `pkg/expression/fts_helper.go` 的 `FTSInfo`。
- `pub fn ContainsFullTextSearchFn(expr: &dyn Expression) -> bool`：递归存在性检查。若根节点不能通过 `as_any().downcast_ref::<ScalarFunction>()` 转成标量函数，立即返回 `false`；若当前函数名匹配则立即返回 `true`；否则用迭代器 `any` 对参数短路递归。
- `pub fn InterpretFullTextSearchExpr(expr: &dyn Expression) -> Option<FTSInfo<'_>>`：严格形状解释器。依次检查节点类型、函数名、参数数量、首参 `Constant` 类型和次参 `Column` 类型；所有检查通过后读取 `query.Value.GetString()` 并返回借用列的 `FTSInfo`。

文件没有模块级常量、trait、`impl`、条件编译项或可变静态状态。

## 执行流程

`ContainsFullTextSearchFn` 的流程如下：

1. 用 `Expression::as_any` 对当前节点做运行时向下转型。
2. 非 `ScalarFunction` 节点返回 `false`。
3. 当前 `ScalarFunction.FuncName.L == ast::FTSMatchWord` 时返回 `true`，不再访问子节点。
4. 否则读取 `GetArgs()`，按参数顺序递归；`Iterator::any` 在第一个命中处短路，全部未命中才返回 `false`。

`InterpretFullTextSearchExpr` 的流程如下：

1. 要求输入节点是 `ScalarFunction`。
2. 要求 `FuncName.L` 精确等于规范化名称 `ast::FTSMatchWord`。
3. 要求 `GetArgs()` 长度恰为 2。
4. 要求第 0 个参数是 `Constant`，第 1 个参数是 `Column`；参数顺序不可交换，也不接受包装节点。
5. 用 `Constant.Value.GetString()` 取得查询文本，复制进 `String`；列节点以引用形式放入 `FTSInfo`。

两条流程互不调用：前者允许 FTS 出现在嵌套标量函数内，后者只解释传入节点本身的标准形状。

## 数据与状态

唯一新建的数据是 `FTSInfo`。查询文本拥有自己的 `String`，列信息则借用原表达式树中的 `Column`，所以结果不能比输入表达式活得更久。函数不会修改 `ScalarFunction`、参数数组、`Constant.Value` 或 `Column`，也不会缓存结果。

函数名判断使用 `ScalarFunction.FuncName.L` 与 `ast::FTSMatchWord` 比较，依赖表达式构造阶段已经维护好规范化的小写名称。解释器只读取 `Constant.Value.GetString()`；本层不检查常量的 SQL 类型、NULL 语义、字符集/排序规则或空字符串，也不验证列是否具备全文索引。这些都是调用层或 builtin 构造层的职责。

## 依赖与调用关系

直接依赖通过 `use crate::*` 引入：

- `Expression` 提供对象安全的 `as_any`，使 helper 能区分具体表达式节点。
- `ScalarFunction` 提供 `FuncName` 与 `GetArgs()`，形成遍历边。
- `Constant`、`Column` 分别限定标准 FTS 的两个参数类型。
- `ast::FTSMatchWord` 提供内部函数名常量。
- `Constant.Value.GetString()` 提供查询文本读取。

RustCodeGraph 对 `pkg/expression/fts_helper.rs` 报告 `used by 0 files`，仓库级精确搜索也只找到符号定义及函数自身递归；`pkg/expression/lib.rs` 仅声明私有 `fts_helper_kernel`，没有再导出。因而当前 Rust 上游调用边为空，下游主要是上述表达式核心类型的方法调用。

Go 侧存在真实上游：`pkg/planner/core/fts_resolve_index.go` 在 WHERE 下推、TopN/ORDER BY 对齐、Projection/SELECT 对齐和残留 FTS 拒绝阶段调用 `expression.InterpretFullTextSearchExpr` 或 `expression.ContainsFullTextSearchFn`。这些调用说明本 helper 的设计用途，但不是当前 Rust 调用事实。Rust 对应规划器 `pkg/planner/core/fts_resolve_index.rs` 当前解析字符串化计划，并定义了自己的私有 `FTSInfo`，两者不能混为同一条调用链。

## 错误处理与边界

本文件没有 `Result` 或错误类型。`InterpretFullTextSearchExpr` 用 `Option` 将所有“不符合标准形状”统一表示为 `None`，包括：非标量函数、函数名不匹配、参数数目不是 2、查询参数不是 `Constant`、列参数不是 `Column`。它不会返回部分结果，也不会记录具体失败原因。

`ContainsFullTextSearchFn` 仅遍历标量函数参数；遇到非 `ScalarFunction` 输入便返回 `false`。其语义是“表达式标量函数树中是否含有目标函数”，不是任意容器或计划节点的通用遍历器。递归深度等于连续嵌套的标量函数深度，源码没有显式深度限制；极端深树存在调用栈增长风险，但正常 SQL 表达式深度通常受更上层解析/规划约束。

`GetString()` 的结果被直接采用，本层不报告类型转换错误。若未来需要区分 NULL、非字符串常量、参数标记或类型转换失败，必须先核对 `Constant`/datum 的既有语义，并决定是在 builtin 构造层拒绝还是扩展本 API；不应悄悄改变当前 `None` 合同。

## 并发与资源生命周期

两个函数都只借用输入并读取不可变字段，没有锁、原子量、线程、异步任务、通道、事务或 I/O。每次调用唯一明确的分配是成功解释时为 `Query` 创建 `String`；递归检查本身不创建持久状态。

`FTSInfo.Column` 的生命周期由编译器绑定到输入表达式，避免悬垂引用，也意味着调用者若要跨越表达式树生命周期保存结果，必须自行提取稳定的列标识或克隆所需数据。函数没有共享缓存，因此并发调用之间没有本文件引入的数据竞争；实际 `Expression` 是否可跨线程共享仍由其 trait 边界和调用者负责。

## 与 Go 版本的对应关系

`pkg/expression/fts_helper.go` 是逐项语义基准：Go 的 `FTSInfo{Query string, Column *Column}` 对应 Rust 的拥有查询字符串加借用列；Go 的类型断言对应 Rust 的 `Any` 向下转型；Go 的 `slices.ContainsFunc(x.GetArgs(), ContainsFullTextSearchFn)` 对应 Rust 的 `.iter().any(...)`；Go 的失败 `nil` 对应 Rust 的 `None`。

核心检查顺序也一致：仅处理 `ScalarFunction`，先比对 `ast.FTSMatchWord`，解释时再要求两个参数以及 `Constant`/`Column` 的固定位置。Rust 没有删减这两个 helper 的局部判断逻辑。

迁移状态存在重要差异：Go helper 是 `expression` 包导出 API，并被 `pkg/planner/core/fts_resolve_index.go` 多处调用；Rust helper 所在模块未从 crate 根导出，且 Rust 规划器目前使用字符串计划模型的局部实现。因此 Go 测试 `pkg/planner/core/fts_resolve_index_test.go` 能间接覆盖 Go helper，而 Rust 测试 `pkg/planner/core/fts_resolve_index_test.rs` 覆盖的是规划器局部字符串解析与下推规则，不能作为本文件直接执行覆盖的证据。

## 扩展指南

若要安全扩展或接入本文件，应按职责选择修改点：

- 新增可识别的 FTS 函数形态：修改 `ContainsFullTextSearchFn` 的名称判定，并同步审查 `InterpretFullTextSearchExpr` 是否也应接受该形态；不能只扩展其中一个而不说明“存在性检查”和“标准形状解释”的差异。
- 改变参数布局或允许包装/多列：修改 `InterpretFullTextSearchExpr` 的参数数量和类型检查，同时核对 Go 文件、builtin 构造约束及规划器下推的数据模型，尤其是全文索引列匹配规则。
- 将其接入 Rust 规划器：先在 `pkg/expression/lib.rs` 建立有意的 crate API，再让使用真实 `Expression` 树的调用层消费它；当前 `pkg/planner/core/fts_resolve_index.rs` 使用字符串计划，不能仅靠增加 `pub use` 替换其局部解析器。
- 增加错误诊断：若从 `Option` 改成可区分错误的接口，需要评估所有调用者以及 Go 对齐要求，避免把普通“不匹配”升级为用户可见错误。

本文件目前没有同名独立 Rust 测试。新增或修改行为时应新建独立测试文件（例如 `pkg/expression/fts_helper_test.rs`）并由 `lib.rs` 在 `#[cfg(test)]` 下装配，遵守“源文件与单元测试不放在同一文件”的仓库规则。最低用例应覆盖：根节点命中、嵌套命中、非标量节点、非 FTS 标量函数、错误参数数目、参数顺序/类型错误、成功提取查询和保持同一列引用。规划器接线变化还应同步 `pkg/planner/core/fts_resolve_index_test.rs`，Go 语义变化则应核对 `pkg/planner/core/fts_resolve_index_test.go`。

兼容风险主要是错误分类和 Go/Rust 行为漂移；性能风险主要来自对深层表达式的递归遍历和成功路径上的字符串复制。若做性能优化，应保留当前短路顺序和借用列的生命周期约束。

## 验证依据

- 目标实现：`pkg/expression/fts_helper.rs`。确认文件共 71 行，包含 `FTSInfo`、`ContainsFullTextSearchFn`、`InterpretFullTextSearchExpr`，无条件编译或隐藏实现。
- crate 边界：`pkg/expression/Cargo.toml` 的包名为 `astersql-expression`、库入口为 `lib.rs`；`pkg/expression/lib.rs` 将文件声明为私有 `fts_helper_kernel`，未再导出其符号。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；精确查询定位了 Rust/Go 两套同名符号；文件节点显示 `used by 0 files`。`callers`/`callees` 命令在本地未在限定时间内返回，所以调用者结论另由仓库级精确搜索交叉验证，未把超时当作空结果。
- 仓库搜索：`rg -w 'ContainsFullTextSearchFn|InterpretFullTextSearchExpr|FTSInfo'` 未发现目标 Rust 符号的外部使用；只见目标文件、Go 对照、Go 规划器调用，以及 Rust 规划器的独立同名私有结构。
- Go 对照：`pkg/expression/fts_helper.go`，确认类型、递归短路、严格二参数形状及 `nil` 失败合同与 Rust 局部实现一致。
- 应用主链证据：`pkg/planner/core/fts_resolve_index.go` 展示 Go helper 在 WHERE、ORDER BY、SELECT 和残留检查中的真实调用；`pkg/planner/core/fts_resolve_index.rs` 展示 Rust 当前使用局部字符串解释器的差异。
- 测试证据：`pkg/planner/core/fts_resolve_index_test.go` 覆盖 Go 主链的匹配索引、嵌套/重复使用、查询词/列不匹配、参数标记和脏事务等边界；`pkg/planner/core/fts_resolve_index_test.rs` 覆盖 Rust 当前规划器实现，但两者都不是目标 Rust helper 的直接单元测试。同目录没有发现引用目标 Rust 符号的独立测试。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；最终只执行固定 11 章节的结构验证，并人工复核上述“为何存在、如何运行、如何安全扩展”三类信息。
