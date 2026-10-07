# `build/linter/unconvert/analysis.rs`

## 文件定位

本文件是 Go 包 `build/linter/unconvert` 中 `analysis.go` 的 Rust 迁移实现，定义一个名为 `unconvert` 的静态分析器，用于发现 Go 源码中目标类型与实参类型完全相同的冗余显式转换。核心入口是 `Analyzer` 和 `run`，辅助判定由 `isUntypedValue`、`asBuiltin` 完成。

当前接线需要区分两条路径：Go 版本通过 `build/linter/unconvert/BUILD.bazel` 的 `go_library` 被 `build/BUILD.bazel` 中的 `tidb_nogo` 依赖，是实际 Bazel lint 链的一部分；Rust 文件不属于根 `Cargo.toml` 的独立 workspace member，也没有生产模块入口。它目前由根 crate 的 `pkg/lib.rs` 在 `#[cfg(test)]` 下挂载 `build/linter/unconvert/analysis_test.rs`，测试再以 `include!("analysis.rs")` 编译本文件。因此，本文描述的是已具备可编译测试形状的 Rust 迁移逻辑，而不是已经替换 Go `nogo` analyzer 的生产接线。

## 核心职责

- `Name` 和 `Analyzer` 声明 analyzer 的稳定名称、说明、前置 analyzer 与运行回调；`Analyzer.requires` 指向 `inspect::Analyzer`，使 `run` 能从 `Pass.ResultOf` 取得预构建的 AST inspector。
- `init` 按 Go 版本的顺序调用 `util::SkipAnalyzerByConfig` 和 `util::SkipAnalyzer`，表达按配置排除文件以及尊重 lint/nolint 指令的注册意图。
- `run` 只遍历 `ast::CallExpr`，逐层排除普通函数调用、真实类型转换、依赖上下文定型的 untyped 值及 cgo 生成调用，最终仅报告确定的冗余转换。
- `isUntypedValue` 递归实现 Go untyped 值传播规则，避免把字面量、常量表达式和特殊内建函数的合法显式转换误报为冗余。
- `asBuiltin` 解析被任意层括号包裹的标识符，并仅在 `types::Info.Uses` 将它绑定为 `types::Builtin` 时返回内建函数对象。

## 主要符号

- `pub const Name: &str = "unconvert"`：analyzer 的公开标识，和 Go 常量 `Name` 一致。
- `pub static Analyzer: analysis::Analyzer`：静态 analyzer 描述符；`name` 为 `Name`，`doc` 为 `Remove unnecessary type conversions`，`requires` 仅含 `inspect::Analyzer`，`run` 字段绑定本文件的 `run`。它是值而非 `Lazy` 或共享引用。
- `pub fn init()`：注册期配置入口。源码签名无返回值；调用两个 `util` 过滤函数，不持有额外状态。
- `pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn Any>>, analysis::Error>`：分析入口。诊断写入 `pass`，正常完成返回 `Ok(None)`；函数本身没有构造分析结果对象。
- `pub fn isUntypedValue(n: &ast::Expr, info: &types::Info) -> bool`：递归查询表达式是否仍具有 Go 的 untyped 语义。
- `pub fn asBuiltin<'a>(n: &ast::Expr, info: &'a types::Info) -> Option<&'a types::Builtin>`：从类型使用表中借用并返回内建函数对象；返回引用生命周期绑定到 `info`。

文件没有自定义 `struct`、`enum`、`trait`、`impl`、条件编译项或可变模块级状态。命名保留了 Go 风格大小写（如 `Analyzer`、`isUntypedValue`），用于贴近迁移来源。

## 执行流程

1. analyzer 框架先执行 `inspect::Analyzer`；`run` 从 `pass.ResultOf[inspect::Analyzer]` 取回并向下转型为 `inspector::Inspector`。缺少结果或类型不符会触发 `expect` panic，因为这是 `Analyzer.requires` 应保证的不变量。
2. `run` 以 `CallExpr` 作为过滤节点进行前序遍历。闭包仍防御性匹配节点类型；非调用节点直接返回。
3. 只保留恰有一个实参且 `Ellipsis == token::NoPos` 的调用。零/多参数调用和 `...` 展开不属于简单类型转换候选。
4. 从 `pass.TypesInfo.Types` 查询 `call.Fun`。缺失则在调用位置报告 `missing type`；若存在但 `IsType()` 为假，则它是普通函数调用，停止处理。
5. 查询唯一实参的类型信息。缺失时同样报告 `missing type`。若 `types::Identical(ft.Type, at.Type)` 为假，则是改变类型的真实转换，不报告。
6. 对表面类型相同的实参调用 `isUntypedValue`。若仍为 untyped 值，则为避免 Go issue 13061 所述的上下文定型问题而跳过。
7. 若转换目标是标识符 `_cgoCheckPointer`，跳过 cgo 工具链自动生成的、用户无法修复的转换。
8. 剩余候选在 `call.Pos()` 报告 `unnecessary conversion`；遍历结束后返回 `Ok(None)`。

`isUntypedValue` 的递归分支如下：移位只继承左操作数；比较恒产生 untyped boolean；算术、位运算及逻辑二元运算要求两侧都 untyped；支持的一元运算继承操作数；基础字面量恒为 true；括号和 selector 分别递归内部表达式与最终选择标识符；标识符通过 `types::Info.Uses` 识别预声明 `nil` 或 `types::IsUntyped` 常量；调用表达式仅对内建 `real`、`imag`、`complex` 传播 untyped 状态，其余情况返回 false。

## 数据与状态

算法不建立跨调用缓存。输入和状态都由 `analysis::Pass` 提供：`ResultOf` 保存前置 analyzer 结果，`TypesInfo.Types` 将表达式映射到 `TypeAndValue`，`TypesInfo.Uses` 将标识符映射到类型对象，诊断通过 `Reportf` 追加到 pass。

`Analyzer` 是只读静态描述符，`Name` 是字符串常量。`run` 的闭包借用可变 `pass` 以提交诊断，同时只读使用 inspector 和类型表。`isUntypedValue` 仅沿 AST 引用递归，不修改节点或类型信息；`asBuiltin` 返回 `info` 内已有对象的共享引用，不转移所有权。

关键不变量是：转换候选必须单参数、无省略号；函数位置必须被类型系统标记为类型；目标类型与实参类型必须 `Identical`；实参不能保持 untyped。只有四项同时满足并且不命中 `_cgoCheckPointer` 例外，才会产生 `unnecessary conversion`。

## 依赖与调用关系

上游关系由静态描述符而非普通函数直接调用构成：`Analyzer.run` 保存 `run` 函数指针，`Analyzer.requires` 声明 `inspect::Analyzer`；analyzer 框架负责调度。Rust 测试中的直接调用者位于 `build/linter/unconvert/analysis_test.rs`：`run_preserves_go_filters_error_paths_and_diagnostics` 调用 `run`，另外两个测试分别调用 `isUntypedValue` 和 `asBuiltin`。

`run` 的直接下游包括 `inspector::Inspector::Preorder`、`analysis::Pass::Reportf`、`types::Identical` 和本文件的 `isUntypedValue`。`isUntypedValue` 会递归调用自身，并调用 `asBuiltin`、对象的 `Pkg/Name/Type`、基础类型的 `Info`；`asBuiltin` 使用 AST 的 `as_paren_expr/as_ident` 及 `types::Info.Uses.get`、`Object::as_builtin`。

crate 边界方面，根 `Cargo.toml` 没有 `build/linter/unconvert` member，`analysis.rs` 也没有在生产 `mod` 树中出现；唯一 Rust 入口是 `pkg/lib.rs` 的测试模块到 `analysis_test.rs`，再由测试中的局部适配模块 `include!` 本文件。Go 侧的边界则由 `build/linter/unconvert/BUILD.bazel` 声明依赖 `build/linter/util`、Go analysis、inspect 和 inspector，并由 `build/BUILD.bazel` 的 `nogo(name = "tidb_nogo")` 引用。

## 错误处理与边界

- 前置 inspector 缺失或 downcast 失败使用 `expect`，属于框架接线不变量被破坏后的立即失败，不转换成 `analysis::Error`。
- 转换目标或实参缺少类型信息时不会 panic，而是在调用位置报告精确文本 `missing type` 并继续扫描其他节点。
- 普通函数调用、真实转换、untyped 表达式、非单参数/可变参数形式和 `_cgoCheckPointer` 都是正常过滤分支，不产生错误或诊断。
- 不支持的 AST 表达式、二元/一元运算符，以及未能在 `Uses` 中解析的标识符，`isUntypedValue` 保守返回 false。扩展 AST 种类时需注意：false 可能使相同类型候选继续走到诊断路径，因此必须对照 Go 语义补测试。
- `real`、`imag` 分支直接读取第一个实参，`complex` 读取前两个实参；安全性依赖 Go 类型检查器保证这些已解析内建调用的参数个数合法。当前实现没有自行检查长度。
- `run` 的 `Result` 目前只有 `Ok(None)` 正常路径；遍历闭包不返回错误，诊断不等同于函数错误。

## 并发与资源生命周期

文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。每次运行的可变状态都局限在传入的 `analysis::Pass`；AST、inspector 和类型信息由框架拥有，辅助函数只借用它们。

`asBuiltin` 返回值不能超出 `types::Info` 生命周期。`run` 中遍历闭包只在 `Preorder` 调用期间存在；结束后没有回调或引用被保存。当前静态 `Analyzer` 本身没有可变字段操作，但 `init` 的两个 util 调用表达注册阶段配置行为；若未来真实 Rust analyzer 框架允许修改 analyzer，应在框架初始化完成前串行执行，不能假定运行期可无锁重配。

递归仅随表达式树深度增长，没有显式深度限制。对正常 Go AST 这是短生命周期栈递归；若引入合成的极深表达式，需要评估栈深风险，但当前测试和来源 Go 实现均采用相同递归策略。

## 与 Go 版本的对应关系

`build/linter/unconvert/analysis.go` 是逐项语义基准：`Name`、analyzer 字段、`init` 调用顺序、`run` 的过滤顺序和两条诊断文本均一致。Rust 的 `Option` 替代 Go map 的逗号-ok 查找，`downcast_ref + expect` 对应 Go 的类型断言；`Ok(None)` 对应 Go 的 `(nil, nil)`。

`isUntypedValue` 覆盖 Go switch 的全部已实现分支：移位、比较、算术/位/逻辑运算、一元运算、字面量、括号、selector、标识符，以及 `real/imag/complex` 内建调用。`asBuiltin` 同样先剥离所有括号，再要求标识符在 `Uses` 中解析为 builtin。Rust 版本额外通过 `Option<&Builtin>` 编码 Go 的 `(*types.Builtin, bool)` 双返回值，不改变成功/失败语义。

必须保留的迁移差异是接线状态：Go 文件属于 Bazel `nogo` 生产依赖；Rust 文件当前只在根 crate 测试中通过局部 mock API 编译和执行，尚未接入真实 Rust 版 `go/ast`、`go/types` 或 analyzer runtime。因此不能仅凭单元测试通过声称 Rust analyzer 已替代 Go 版本。

## 扩展指南

- 新增或调整转换过滤条件时，优先修改 `run` 中类型信息检查与报告前的分支，并同步对照 `analysis.go`；过滤顺序会影响 `missing type` 与静默跳过的行为，不应随意重排。
- 支持新的 untyped 表达式或内建函数时，修改 `isUntypedValue`，如需名称解析则同时检查 `asBuiltin`。必须在独立文件 `analysis_test.rs` 的 `untyped_value_recursion_matches_every_go_expression_branch` 或新增独立测试中覆盖 true、false 和缺失类型对象三类边界，不能把测试嵌入生产文件。
- 修改 analyzer 元数据或初始化行为时，更新 `Analyzer`/`init`，并同步 `analyzer_has_a_compilable_rust_api_shape`；若涉及生产启用，还需新增真实模块/依赖接线，而不能把当前 `include!` 测试适配层当作生产 API。
- 修改诊断条件或文本时，扩展 `run_preserves_go_filters_error_paths_and_diagnostics`，至少覆盖零参数、`...`、缺失函数类型、普通函数、缺失实参类型、真实转换、untyped 值、cgo 例外与冗余转换。
- 兼容风险主要是与 Go 类型系统语义漂移导致误报/漏报；性能风险主要是对每个调用表达式做类型表查询与递归 AST 检查。新增递归分支应保持与表达式规模线性，并避免重复遍历大子树。
- 若未来把本文件接入真实 Rust lint 链，需要首先确认 `analysis::Analyzer` 的静态可共享约束、`Pass.ResultOf` 的类型身份和 util 初始化的可变性；这些目前只由测试适配类型验证，不能从 mock 推断真实并发保证。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter build/linter/unconvert` 找到 `analysis.rs`、`analysis.go`、`analysis_test.rs`；`node --file build/linter/unconvert/analysis.rs --offset 1 --limit 260` 返回 206 行完整源码与 5 个符号；`node build/linter/unconvert/analysis.rs::run --json` 确认精确节点 `build/linter/unconvert/analysis.rs:48:function:run`。对该节点的 `callers/callees` 查询两次在 30 秒内无输出，因此调用关系以源码字段绑定和直接测试调用复核，没有宣称图中不存在调用者。
- 源码：`build/linter/unconvert/analysis.rs`，核对 `Name`、`Analyzer`、`init`、`run`、`isUntypedValue`、`asBuiltin` 及所有过滤/诊断分支。
- Go 对照：`build/linter/unconvert/analysis.go`，核对 analyzer 元数据、遍历顺序、untyped 传播、cgo 例外与诊断文本。
- Rust 独立测试：`build/linter/unconvert/analysis_test.rs`，核对三个行为测试及两个源码形状测试；该文件以 mock `analysis/ast/types/inspect/inspector/util` 模块和 `include!("analysis.rs")` 验证迁移逻辑。
- 接线与边界：根 `Cargo.toml`、`pkg/lib.rs`、`build/linter/unconvert/BUILD.bazel`、`build/BUILD.bazel`，分别核对 workspace 归属、测试入口、Go library 依赖和 Bazel `tidb_nogo` 生产接线。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证要求本文恰有十一个固定二级标题，并由任务指定 shell 命令检查。
