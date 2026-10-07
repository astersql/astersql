# `pkg/expression/constant_fold.rs`

源码：[`constant_fold.rs`](constant_fold.rs)；Go 语义对照：[`constant_fold.go`](constant_fold.go)；独立 Rust 契约测试：[`constant_fold_38_aster_unit_test.rs`](constant_fold_38_aster_unit_test.rs)。

## 文件定位

本文件属于 `astersql-expression` crate（`pkg/expression/Cargo.toml`），实现一套从 Go `pkg/expression/constant_fold.go` 迁移而来的常量折叠算法：在表达式构造或优化阶段，把当前求值上下文中可确定的标量表达式替换为 `Constant`，同时保留 SQL 类型与排序规则元数据。`pkg/expression/lib.rs:285-286` 通过私有模块 `constant_fold_kernel` 编译本文件。

需要特别区分“已编译”和“当前 crate 根入口”：`lib.rs:368` 导出的是 `core_support::*`，其中 `pkg/expression/core_support.rs:481` 另有同名 `FoldConstant`；仓库搜索未发现 `constant_fold_kernel::` 的显式引用，也没有 `pub use constant_fold_kernel::*`。因此本文件目前是已编译的平行实现/迁移契约对象，而 `crate::FoldConstant` 及外部 crate 可见入口实际来自 `core_support.rs`。RustCodeGraph 会把若干未限定名称的调用同时关联到两个同名 Rust 符号，阅读调用图时必须结合上述模块接线消歧，不能据此断言本文件位于运行主链。

## 核心职责

- `FoldConstant` 是本模块设计上的包装入口：调用内部 `foldConstant`，再恢复原表达式的 coercibility、charset、collation 和 repertoire，避免优化改写改变字符串比较语义。
- `foldConstant` 处理三类输入：标量函数、带 `ParamMarker`/`DeferredExpr` 的常量、以及无需处理的其他表达式。
- `specialFoldHandler` 为 `IF`、`IFNULL`、`CASE WHEN`、`ISNULL` 提供短路折叠，避免提前求值不会执行的分支。
- 普通标量函数在参数全为常量时于空 `chunk::Row` 上求值；不可折叠函数、扩展函数、计划缓存敏感表达式以及求值失败路径保留原树。
- null-reject 检查中，本文件可用常量 `1` 临时替换非常量参数，探测带 NULL 参数的函数是否必为 NULL 或 false。
- 折叠结果维护 deferred 状态、`NotNullFlag`、`SubqueryRefID` 和 CASE 的 decimal 元数据。

这些职责由 `constant_fold.rs:28-333` 的函数体及 `constant_fold_38_aster_unit_test.rs` 的源码契约断言直接验证；后者不是运行时语义测试，而是通过 `include_str!` 检查实现表面和关键代码片段。

## 主要符号

- `type FoldHandler = fn(&dyn BuildContext, ScalarFunction) -> (Box<dyn Expression>, bool)`：特殊函数处理器签名。返回值第二项表示结果是否依赖执行上下文（参数标记或 deferred 表达式），而不是简单表示“是否折叠成功”。
- `specialFoldHandler()`：每次构造 `HashMap<&'static str, FoldHandler>`，登记 `ast::If`、`ast::Ifnull`、`ast::Case`、`ast::IsNull`。它没有可变全局状态。
- `init()`：返回上述分派表，用来对应 Go 的包初始化函数；仓库未发现该 Rust 函数的调用。
- `FoldConstant(ctx, expr)`：保存表达式元数据，调用 `foldConstant`，再把元数据写回结果。本模块是私有模块，因此这里的 `pub` 只使符号在父模块可见，并未自动成为 crate 根导出。
- `foldConstant(ctx, expr)`：核心递归入口。它对 `ScalarFunction` 做分类和求值，对 `Constant` 刷新参数/deferred 值，其余表达式原样返回。
- `isNullHandler`：常量参数直接求 `ISNULL`；若参数类型带 `NotNullFlag`，直接返回 `NewZero()`。
- `ifFoldHandler`：先折叠条件；仅在条件成为常量时，根据 SQL 真值规则选择并折叠一个分支。
- `ifNullFoldHandler`：第一参数为非 NULL 常量时直接返回；为 NULL 时只折叠第二参数，并把第二参数 charset/collation 写到函数返回类型。
- `caseWhenHandler`：按 WHEN 顺序求值，遇到首个非常量条件立即停止，遇到首个真条件只折叠对应 THEN；没有命中时处理 ELSE。
- `log_fold_error`、`log_param_error`：把优化期错误写入后台日志，然后由调用路径保留原表达式，使错误仍可在执行期出现。

文件内没有结构体、枚举、trait、模块级业务常量或条件编译项。除 `FoldConstant`、`foldConstant`、`specialFoldHandler`、`init` 外，其余函数均为模块私有。

## 执行流程

1. 设计入口 `FoldConstant` 从输入表达式读取 coercibility、repertoire、charset、collation，然后调用 `foldConstant`；无论是否发生折叠，最后都将这些属性恢复到返回表达式。
2. `foldConstant` 若见到 `ScalarFunction`，先检查 `unFoldableFunctions`，再检查 `Function.isExtensionFunction()`。前者代表已知不能在优化期固化的函数；后者可能有外部副作用，两者都直接返回原表达式。
3. 若 `MaybeOverOptimized4PlanCache` 判断当前表达式不会在计划缓存中被过度优化，则查找特殊处理器。特殊处理器保持 SQL 短路语义：
   - `IF` 把 NULL 或 0 视为假，只递归折叠选中的 THEN/ELSE。
   - `IFNULL` 仅在第一参数已成常量后选分支。
   - `CASE` 逐个处理 WHEN，未知条件使后续分支保持未求值。
   - `ISNULL` 对常量求值，对静态 NOT NULL 参数返回 0。
4. 普通函数先扫描参数，记录 `all_constant`、`has_null`、`deferred` 和逐参数常量位图。这里不递归折叠普通函数的参数；它只依据调用者传入的当前参数形态决定下一步。
5. 参数不全为常量时，通常原样返回。唯一额外路径是 `ctx.IsInNullRejectCheck()` 且已有 NULL 常量参数：除 `NullEQ`、`ConcatWS`、`Field` 外，用 `NewOne()` 替代非常量参数，经 `NewFunctionBase` 构造临时函数并求值；若结果为 NULL 或可转换为 false，则返回临时常量，否则保留原表达式。
6. 参数全为常量时，在默认空行上调用 `function.Eval`。成功后按结果是否为 NULL 修正克隆返回类型的 `NotNullFlag`；若任一参数为 deferred/parameter 常量，则用 `Constant::with_deferred` 保存原函数以供执行期重算，否则用 `Constant::with_subquery` 并传播首个正 `SubqueryRefID`。
7. 输入本身为 `Constant` 时，`ParamMarker.GetUserVar` 或 `DeferredExpr.Eval` 刷新当前值并返回 `true` 的 deferred 标志；求值失败则记录日志并保留原对象。
8. 其他表达式类型不参与折叠，直接返回 `(expr, false)`。

## 数据与状态

算法只重写拥有所有权的 `Box<dyn Expression>` 或克隆出的 `ScalarFunction`，没有持久化、事务或全局缓存写入。主要状态如下：

- `BuildContext` 提供 `EvalContext`、类型转换上下文、计划缓存标志和 null-reject 模式；折叠结果可能随该上下文变化。
- `deferred` 是跨递归返回的布尔状态。它由 `Constant.DeferredExpr` 或 `Constant.ParamMarker` 触发，在 CASE 中跨已检查条件与选中分支累积。
- `RetType` 携带 charset、collation、decimal 和 MySQL flags。外层入口恢复原表达式的字符串元数据，CASE 选中常量分支恢复原 CASE decimal，普通全常量求值修正 `NotNullFlag`。
- `SubqueryRefID` 从普通函数的首个带正 ID 的常量参数传播到非 deferred 折叠结果，供展示/关联子查询来源。
- null-reject 路径创建的 dummy 函数与常量是一次性探测对象，返回的探测常量刻意不保留 `DeferredExpr`。
- `specialFoldHandler()` 每次调用创建新 `HashMap`；这避免了 Go 版本的可变包级表，但也意味着每次特殊分派前有一次小型映射分配。

## 依赖与调用关系

本文件以 `use crate::*` 取得 expression crate 根导出的 `BuildContext`、`Expression`、`ScalarFunction`、`Constant`、`NewFunctionBase`、`NewOne`、`NewZero`、`unFoldableFunctions`、`ast`、`mysql`、`types`、`chunk` 和 `logutil` 等定义；唯一直接标准库依赖是 `std::collections::HashMap`。这些能力的外部 crate 归属可在 `pkg/expression/Cargo.toml` 中追到 parser AST/MySQL、types、chunk、logutil、exprctx 等路径依赖。

内部调用边为：`FoldConstant -> foldConstant`；`foldConstant -> specialFoldHandler -> {ifFoldHandler, ifNullFoldHandler, caseWhenHandler, isNullHandler}`；各 handler 再递归调用 `foldConstant`。普通函数路径调用 `MaybeOverOptimized4PlanCache`、`NewFunctionBase`、`ScalarFunction::Eval` 和 datum 的 `ToBool`；常量路径调用参数标记或 deferred 表达式求值。

RustCodeGraph 的 `node FoldConstant` 报告 `expression.rs`、`scalar_function.rs`、`util.rs`、`builtin.rs` 和 planner 代码中的同名调用候选。但由于 crate 根 `FoldConstant` 由 `core_support.rs` 导出，且没有显式 `constant_fold_kernel` 路径，这些调用当前应归入 `core_support.rs` 主实现，而不能作为本文件已接线的证据。本文件可确认的直接使用者是 `lib.rs` 的模块声明和 `constant_fold_38_aster_unit_test.rs` 的 `include_str!("constant_fold.rs")` 契约扫描。

## 错误处理与边界

- 标量函数、特殊 `IF`/`ISNULL`、参数标记和 deferred 表达式在折叠期求值失败时不向调用者返回 `Result`；实现记录 warning 并保留原表达式，使执行期仍有机会产生面向客户端的错误。
- CASE 条件求值、dummy 函数构造/求值失败则安静回退原表达式，没有日志。这与 Go 对照文件的相应回退方向一致。
- 多处使用索引和 `unwrap()`：特殊处理器假定函数构造层已保证 IF 有三个参数、IFNULL/ISNULL 参数数正确且 `ScalarFunction.RetType` 存在。直接绕过正规构造器建立畸形表达式可能 panic。
- `caseWhenHandler` 用 `saturating_sub(1)` 防止长度为 0 时下溢，但命中条件时仍要求存在相邻 THEN；参数布局依赖 CASE 构造不变量。
- null-reject dummy 值 `1` 只适合判断当前受支持函数在该探测下是否恒 NULL/false，因此明确排除 `NullEQ`、`ConcatWS`、`Field`；新增具有类似反例的函数必须加入排除集或改进探测策略。
- 扩展函数一律不折叠，以免优化期触发外部副作用。不可确定或可能依赖计划缓存参数的特殊表达式也保持到执行期。
- 源码契约测试主要验证字符串片段存在，不能证明实际求值结果、panic 边界或本模块被运行时入口调用。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、文件句柄、网络连接或事务。表达式树通过 `Box` 所有权传入和返回；函数参数需要保留时使用表达式克隆，临时 dummy 参数、默认 `chunk::Row` 和分派 `HashMap` 均在单次调用内释放。

并发安全取决于传入的 `BuildContext`/`EvalContext` 及表达式实现；本文件仅持有共享借用，不保存引用到全局状态。日志写入由 `logutil::BgLogger` 管理。对 correlated datum 等锁的生命周期不在本文件中；本算法只通过通用 `Expression::Eval` 间接访问下游资源。扩展函数被禁止折叠，也是避免在优化期跨越外部资源/副作用边界的重要约束。

## 与 Go 版本的对应关系

`pkg/expression/constant_fold.go` 是直接语义对照：Rust 保留了 `FoldConstant`、`foldConstant`、四个特殊 handler、不可折叠/扩展函数保护、计划缓存保护、null-reject dummy 探测、deferred/parameter 刷新、类型标志和 `SubqueryRefID` 传播。

主要实现差异如下：

- Go 使用包级可变 `specialFoldHandler` 并在 `init()` 填充；Rust 的 `specialFoldHandler()` 每次返回新映射，`init()` 仅作为无调用者的对应面存在。
- Go 的公开入口是 package 实际入口；本 Rust 文件的同名入口位于私有模块，当前 crate 根入口是 `core_support.rs` 中的平行版本。
- Rust 通过 `Box<dyn Expression>`、`Option`、模式匹配及 `clone_with_value`/`with_deferred`/`with_subquery` 构造器表达 Go 的接口、nil 指针与结构体字面量。
- Go 参数标记路径包含 `intest.AssertNoError` 后再回退；本文件仅记录 warning 并回退，不在优化期断言。
- Rust 日志目前使用 warning 文本，Go 多数折叠错误使用带表达式信息的 debug 日志；对客户端的关键行为都是保留原表达式而非吞掉执行期错误。
- Go `IFNULL` 在先折叠第二参数后修改原函数类型；Rust 先从第二参数读取 charset/collation 写入 `expr.RetType`，再折叠第二参数。外层 `FoldConstant` 还会恢复调用入口原表达式元数据，因此接线前应以行为测试核对最终排序规则效果。

`pkg/expression/constant_fold_38_aster_unit_test.rs` 对照两份源码，锁定函数面与大量关键片段；Go 的 `constant_test.go` 还包含真实 `FoldConstant` 调用，但它验证的是 Go 实现，不能替代 Rust 运行时测试。

## 扩展指南

- 新增需要短路的函数时，在 `specialFoldHandler` 登记专用 handler；handler 只能折叠已确定会执行的分支，并正确合并 deferred 标志。若该功能也要进入实际 crate 根主链，必须同步 `core_support.rs` 的平行实现或先完成明确的单一实现接线，不能只改本私有模块。
- 新增不可在优化期执行或具有副作用/非确定性的函数时，应更新 `unFoldableFunctions` 或相应函数 trait；不要依赖“参数恰好不是常量”来规避折叠。
- 改变 null-reject 探测时，检查 dummy `1` 对新函数是否安全，并同步排除集；重点覆盖 NULL、false、类型转换失败及非常量参数取不同值时结果变化的反例。
- 改变常量构造时必须维持 charset、collation、coercibility、repertoire、decimal、`NotNullFlag`、deferred/parameter 信息和 `SubqueryRefID`，否则可能改变比较语义、计划缓存或 explain 输出。
- 测试应放在独立文件中。现有最近的 Rust 文件是 `pkg/expression/constant_fold_38_aster_unit_test.rs`，但它是源码契约测试；实际接线或行为修改应另补/扩展独立 Rust 行为测试，覆盖 IF/IFNULL/CASE/ISNULL 短路、错误分支不提前求值、计划缓存、null-reject、排序规则和 deferred 参数。Go 语义变化还应同步 `constant_fold.go` 的既有测试面。
- 性能上注意 `specialFoldHandler()` 的逐次 HashMap 分配、表达式克隆以及 null-reject 的临时函数构造；若优化这些开销，必须保持无可变全局状态和相同分派语义。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/constant_fold.rs` 识别本文件含 11 个符号。
- RustCodeGraph 源码查询：`node --file pkg/expression/constant_fold.rs --offset 1 --limit 500` 读取了完整 333 行；`query FoldConstant`、`query foldConstant`、`query specialFoldHandler` 核对同名定义；`node FoldConstant` 显示 Go、本文件和 `core_support.rs` 三个定义以及候选调用边。同名调用边已结合模块声明与 re-export 人工消歧。
- 已读源码/配置：`pkg/expression/constant_fold.rs`、`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`、`pkg/expression/core_support.rs`、`pkg/expression/constant_fold.go`。
- 已读直接调用与测试证据：`pkg/expression/expression.rs`、`pkg/expression/scalar_function.rs`、`pkg/expression/util.rs` 的调用片段；`pkg/expression/constant_fold_38_aster_unit_test.rs` 全文；`pkg/expression/constant_test.go` 中的 Go 调用位置。仓库没有 `pkg/expression/doc.go`。
- 接线核验：`rg` 未找到任何 `constant_fold_kernel::`、`crate::constant_fold_kernel` 或 `use ... constant_fold_kernel`；`lib.rs` 声明私有模块但只从 `core_support` 导出 `FoldConstant`。因此文档没有把 RustCodeGraph 的歧义候选当成本文件的已验证运行时调用者。
- 按任务约束未运行 Cargo 或代码测试；本次只创建说明文档，最终结构验证检查文件存在且恰有 11 个规定的二级标题。
