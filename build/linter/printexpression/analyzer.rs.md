# `build/linter/printexpression/analyzer.rs`

## 文件定位

该文件是 Go 静态分析器 [`analyzer.go`](./analyzer.go) 的 Rust 对照实现，位于构建辅助目录 `build/linter/printexpression`。它描述名为 `printexpression` 的规则：禁止把“提供 `StringWithCtx`、但不提供 `String`”的表达式类型直接交给通用打印或错误格式化入口，以免输出地址或内部状态，而不是面向用户的表达式文本（`Analyzer`、`run`）。

当前接线必须区分 Go 与 Rust：[`BUILD.bazel`](./BUILD.bazel) 的生产 `go_library` 只包含 `analyzer.go`；根 Cargo 包 `astersql` 的库入口是 [`../../../pkg/lib.rs`](../../../pkg/lib.rs)，其中仅在 `#[cfg(test)]` 下挂载 [`analyzer_test.rs`](./analyzer_test.rs)，测试再以 `include!("analyzer.rs")` 编译本文件。RustCodeGraph 也显示本文件 `used by 0 files`。因此它目前是已实现并有独立编译测试的 Rust 移植单元，不是已注册到生产 Rust linter 主链的模块。

## 核心职责

- 用静态值 `Analyzer` 声明规则名称、说明、对 `inspect::Analyzer` 的依赖和执行回调。
- `run` 只遍历 `ast::CallExpr`，先用 `funcIsFormat` 筛选受管控调用，再逐个检查实参。
- `argIsNotAllowed` 从 `types::Info` 获取表达式类型，递归剥离指针和切片后，把可枚举方法的类型交给 `typIsNotAllowed`。
- `typIsNotAllowed` 实现核心不变量：方法集中有 `StringWithCtx` 且没有 `String` 时返回 `true`；必要时还检查命名类型的底层接口。
- `init` 保留 Go 版本的注册副作用形状，依次调用 `util::SkipAnalyzerByConfig` 和 `util::SkipAnalyzer`。

该规则不解析格式串，也不判断 `%s`、`%v` 等具体占位符；一旦调用名命中，就检查所有实参。是否报告完全由实参类型的方法集决定（`run` 第 65–77 行）。

## 主要符号

- `pub static Analyzer: analysis::Analyzer`：规则描述符，`name` 为 `printexpression`，`requires` 固定包含 `inspect::Analyzer`，`run` 指向本文件回调。
- `pub fn run(&mut analysis::Pass) -> Result<Option<Box<dyn Any>>, analysis::Error>`：分析入口。成功时总是返回 `Ok(None)`；诊断通过 `Pass::Reportf` 侧向写入。
- `pub fn funcIsFormat(&ast::Expr) -> bool`：识别 `fmt.Printf`、`fmt.Sprintf`、`fmt.Println`，以及任意 receiver 上名为 `GenWithStack`、`GenWithStackByArgs`、`FastGen`、`FastGenByArgs` 的选择器调用。
- `pub fn argIsNotAllowed(&types::Info, &ast::Expr) -> bool`：连接 AST 实参与类型系统；缺少类型信息或类型不支持 `methodLookup` 时保守返回 `false`。
- `pub trait methodLookup: types::Type`：局部最小接口，只要求 `NumMethods`、`Method` 和 `Underlying`，对应 Go 文件中的同名局部 interface。
- `pub fn typIsNotAllowed(&dyn methodLookup) -> bool`：枚举方法名并实施 `StringWithCtx && !String` 判定；对底层接口做一次受控递归。
- `pub fn elementType(&dyn types::Type) -> &dyn types::Type`：递归剥离任意层数的 `Pointer` 和 `Slice`。
- `pub fn init()`：保留 analyzer 跳过/配置登记调用；本文件没有 Rust 自动初始化机制，且当前生产模块未接线，不能把函数存在等同于它会自动运行。

这些符号虽然声明为 `pub`，其实际可见边界仍取决于包含它们的模块；当前只有测试中的私有 `compiled_printexpression::implementation` 模块包含该文件。

## 执行流程

1. 分析框架依据 `Analyzer.requires` 先运行 inspect pass；`run` 从 `pass.ResultOf[inspect::Analyzer]` 取出并向下转型为 `inspector::Inspector`。
2. `Inspector::Preorder` 使用只含默认 `CallExpr` 的过滤器遍历调用表达式。回调再次模式匹配 `ast::Node::CallExpr`，其他节点立即返回。
3. `funcIsFormat` 要求被调对象是 `SelectorExpr`。对于三个 `fmt` 函数，还要求选择器左侧是名称恰为 `fmt` 的标识符；四个错误生成函数当前只校验方法名。
4. 对命中调用的每个参数，`argIsNotAllowed` 查询 `TypesInfo.Types`。无类型信息时放行；有类型时由 `elementType` 递归拆除指针/切片包装。
5. 若最终类型能转换为 `methodLookup`，`typIsNotAllowed` 枚举其全部方法。发现 `StringWithCtx` 且没有 `String` 时立即判违规。
6. 若当前方法集尚不能判违规，并且当前值本身不是接口、但 `Underlying()` 是接口，则递归检查该底层接口；“接口的底层仍是自身”的防护避免无限递归。
7. 违规参数在自身 `Pos()` 位置报告固定消息，提示调用 `Expression.StringWithCtx()`；遍历继续，因此同一调用的多个违规参数可分别产生诊断。

## 数据与状态

本文件没有可变全局业务状态。`Analyzer` 是只读静态描述符；一次分析的输入和输出状态都由可变借用的 `analysis::Pass` 承载，包括前置 pass 结果、类型映射和诊断收集器。

核心临时状态仅有 `typIsNotAllowed` 中的两个布尔值 `implString`、`implStringWithCtx`。判定只比较方法名，不检查签名、可见性或返回类型；这是与 Go 实现一致的规则粒度。`elementType` 借用并返回既有类型对象，不复制类型图；递归只沿指针和切片的元素边前进。

测试夹具给出实际矩阵：仅有 `StringWithCtx` 的 `Expression`、`Constant`、`*Constant` 和 `[]*Constant` 应报告；同时具备 `String` 的 `Column`、组合了 `String` 约束的 `ExpressionWithString`、以及自身补充 `String` 的 `EmbeddedConstant` 应放行（`testdata/src/**`）。

## 依赖与调用关系

上游逻辑入口是 `Analyzer.run -> run`；但生产 Rust 上游当前未接线。测试上游为 `pkg/lib.rs` 的 `build_linter_printexpression_analyzer_test` 模块，再进入 `analyzer_test.rs::compiled_printexpression::implementation` 的 `include!("analyzer.rs")`。

文件内部的关键调用边为：

```text
run
├── Inspector::Preorder
├── funcIsFormat
└── argIsNotAllowed
    ├── elementType ──(Pointer/Slice)──> elementType
    └── typIsNotAllowed ──(named type underlying interface)──> typIsNotAllowed

init
├── util::SkipAnalyzerByConfig
└── util::SkipAnalyzer
```

抽象外部依赖分别对应 Go 标准/工具链概念：`ast` 提供表达式与位置，`types` 提供类型和方法集，`analysis` 提供 pass/诊断协议，`inspect` 与 `inspector` 提供带节点过滤的 AST 遍历，`util` 提供仓库级 analyzer 跳过登记。它们在 Rust 测试中由局部桩模块提供，不是 Cargo 外部依赖。根 [`../../../Cargo.toml`](../../../Cargo.toml) 没有为本目录声明独立 crate 或相关 feature；Bazel 依赖则只服务 Go 目标。

## 错误处理与边界

- `run` 对缺失或类型不符的 inspect 前置结果调用 `expect`，会 panic；这是对 `Analyzer.requires` 框架不变量的直接表达，而非可恢复分析错误。
- 正常路径不构造 `analysis::Error`，结束时固定 `Ok(None)`；规则违规是诊断，不是函数错误。
- 类型信息缺失、最终类型不能枚举方法时返回 `false`，避免因分析信息不完整产生误报，但也意味着可能漏报。
- `fmt` 三个函数只接受左侧为裸标识符 `fmt` 的选择器；别名导入、重新绑定或其他调用形状不会命中。相反，四个错误生成方法不验证 receiver 是否真为 `*Error`，源码和 Go 版本都保留了这一待改进边界，存在同名方法误报可能。
- 规则只看方法名，不验证 Go `fmt.Stringer` 的精确 `String() string` 签名，也不检查 `StringWithCtx` 签名。
- `elementType` 只处理指针与切片，不专门展开数组、map、元组或其他容器。
- 底层接口递归明确排除了“当前类型已经是接口”的情况，防止 `Underlying()` 返回自身导致无限递归。

## 并发与资源生命周期

实现没有线程、异步任务、锁、通道、文件句柄或网络资源。分析生命周期受外部 `analysis::Pass` 和 `Inspector::Preorder` 控制；闭包在遍历期间借用 `pass` 并同步追加诊断，函数返回后不保存 AST 节点、类型引用或闭包状态。

`Analyzer` 为进程级静态只读值，本文件自身不做同步。是否能并行运行多个 pass 取决于外部分析框架及 `Pass`/`Inspector` 实现，目标文件没有提供或声称线程安全保证。类型递归消耗调用栈，但只沿有限的指针/切片层级或一次命名类型到底层接口的关系推进；接口自底层递归由显式类型检查截断。

## 与 Go 版本的对应关系

Rust 文件逐项保留 [`analyzer.go`](./analyzer.go) 的结构：`Analyzer` 的 Name/Doc/Requires/Run，`run` 的 CallExpr 预序遍历和逐参数报告，`funcIsFormat` 的选择器分支，`argIsNotAllowed` 的类型缺失放行，`methodLookup` 的最小方法集，`typIsNotAllowed` 的方法名判定及底层接口回退，以及 `elementType` 的指针/切片递归。

主要表示差异来自语言边界：Go 使用类型断言和 `any`，Rust 使用枚举模式匹配、`dyn trait`、`downcast_ref` 与借用；Go 的 analyzer 由真实 `golang.org/x/tools/go/analysis` 和 Bazel `go_library` 构建，Rust 当前由独立测试桩模拟这些接口。Rust 测试 `format_target_detection_matches_go_switch` 和 `type_rule_handles_missing_pointer_slice_and_underlying_interface` 执行核心判定，其他测试检查源码契约、无 `unsafe`、两组 Go/Rust fixture 各保留三个预期诊断。Go 测试则以 `analysistest.Run` 对 `t` 和 expression fixture 执行真实 analyzer。

因此，“核心算法与 Go 语义对齐”有测试证据；“Rust analyzer 已接入真实生产分析框架”没有证据，且当前 Cargo/Bazel/module 接线明确不支持该结论。

## 扩展指南

- 新增受控格式化入口时修改 `funcIsFormat`，并在 `analyzer_test.rs::format_target_detection_matches_go_switch` 增加正反例；若 Go 版本仍是语义源，还应同步 `analyzer.go` 及 Go analysistest fixture。
- 若要消除错误生成函数的同名误报，应在 `funcIsFormat` 中结合 `TypesInfo` 验证 receiver；这会改变现有函数签名与调用边，需覆盖真实 `*Error`、同名非 Error 方法、别名/嵌入等边界。
- 若要精确验证 `String`/`StringWithCtx` 签名，应扩展 `methodLookup` 或使用完整类型接口，并补充“同名错误签名”的独立测试，避免仅凭名称放行。
- 若要支持数组或其他容器，应修改 `elementType`，同时明确是否符合 Go 打印语义，增加嵌套容器与递归终止测试。
- 若要接入生产 Rust linter，必须新增真实模块/依赖/注册链，而不是依赖测试 `include!`；这属于接线工作，应单独验证 Cargo feature、分析框架类型和初始化策略。
- 测试逻辑应继续放在同目录独立文件 `analyzer_test.rs`，不要内嵌到生产源文件；扩展后同步两组 `testdata` 的 Go/Rust 对照用例及诊断数量断言。

兼容风险主要是误报/漏报集合变化和诊断位置/文本变化；性能风险集中在对每个命中调用逐参数枚举方法，以及新增更深类型展开后可能扩大的遍历成本。当前方法枚举和递归都不做缓存，扩展时应维持局部、有限的类型图访问。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter build/linter/printexpression` 确认目标、Go 对照、测试与 fixture；`node --file build/linter/printexpression/analyzer.rs` 读取 173 行完整实现并显示 `used by 0 files`；`query` 分别定位 Rust/Go 的 `funcIsFormat`、`argIsNotAllowed`、`typIsNotAllowed`、`elementType`；`explore` 给出内部边 `run -> funcIsFormat/argIsNotAllowed`、`argIsNotAllowed -> elementType/typIsNotAllowed` 和方法访问边。
- 源码：[`analyzer.rs`](./analyzer.rs) 的 `Analyzer`、`run`、`funcIsFormat`、`argIsNotAllowed`、`methodLookup`、`typIsNotAllowed`、`elementType`、`init`。
- crate/构建边界：根 [`../../../Cargo.toml`](../../../Cargo.toml) 的 `[package] name = "astersql"` 与 `[lib] path = "pkg/lib.rs"`；[`../../../pkg/lib.rs`](../../../pkg/lib.rs) 的测试专用模块；[`BUILD.bazel`](./BUILD.bazel) 仅声明 Go library/test。
- Go 对照：[`analyzer.go`](./analyzer.go) 和 [`analyzer_test.go`](./analyzer_test.go)。后者通过 `analysistest.Run` 覆盖 `t` 与 expression 两个 fixture 包，并带 `!intest` 构建约束。
- Rust 独立测试：[`analyzer_test.rs`](./analyzer_test.rs)，包括可执行核心规则测试、源码契约检查、fixture 诊断矩阵和许可证/占位文本检查。
- 夹具：[`testdata/src/github.com/pingcap/tidb/pkg/expression/expression.rs`](./testdata/src/github.com/pingcap/tidb/pkg/expression/expression.rs)、其 Go 对照，以及 [`testdata/src/t/test_file.rs`](./testdata/src/t/test_file.rs)、其 Go 对照。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构以任务指定命令验证，要求目标文件存在且固定二级标题恰为 11 个。
