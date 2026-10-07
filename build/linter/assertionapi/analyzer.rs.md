# `build/linter/assertionapi/analyzer.rs`

## 文件定位

本文件位于构建期静态检查目录 `build/linter/assertionapi/`，是同目录 Go 实现 `analyzer.go` 的机械迁移草稿。它描述一个名为 `assertionapi` 的 analyzer：在 Go 源码中限制事务断言 API `UpdateAssertionFlags` 的使用位置，只允许 `pkg/table/tables/`。文件开头明确注明“当前不保证可编译”，因此它不是 SQL 请求、事务执行或存储运行时链路的一部分，也不能据此认定 Rust 版本已经能执行 lint。

当前真实接线分成两条：Go 侧 `build/linter/assertionapi/BUILD.bazel` 定义公开 `go_library`，并由 `build/BUILD.bazel` 的 `nogo(name = "tidb_nogo")` 依赖；Rust 侧没有独立 `Cargo.toml`、模块入口或生产模块声明，根 crate 仅在 `pkg/lib.rs` 的 `#[cfg(test)]` 下引入 `analyzer_test.rs`，而测试用 `include_str!("analyzer.rs")` 把本文件当文本检查。根 `Cargo.toml` 的 workspace members 中也没有 `build/linter/assertionapi` crate。

## 核心职责

按本文件保存的设计，检查器完成三层过滤：

1. `run` 先用 `isAllowedFile` 跳过表层实现目录，保留该 API 在合法封装层中的使用。
2. 对其他文件遍历 AST，只关注选择器名为 `UpdateAssertionFlags` 的 `SelectorExpr`。
3. `isKVUpdateAssertionFlags` 再用类型信息确认方法参数来自 `github.com/pingcap/tidb/pkg/kv`，避免仅凭同名方法误报。

命中后，`run` 将诊断定位到选择器标识符，并报告该 API 仅限 `pkg/table/tables`。这一职责和判定顺序可由 `analyzer.rs` 中的 `run`、`isAllowedFile`、`isKVUpdateAssertionFlags` 以及 Go 对照文件逐项核对；Rust 当前只表达该逻辑，并未形成可执行 analyzer。

## 主要符号

- `pub static Analyzer: analysis::Analyzer`：保存 analyzer 元数据，名称为 `assertionapi`，说明文本指出限制对象是 `UpdateAssertionFlags`，声明依赖 `inspect::Analyzer`，并把执行入口绑定到 `run`。其中 `analysis`、`inspect` 目前只是迁移草稿引用，文件没有可见的 Rust 导入或 crate 依赖来提供它们。
- `pub const kvPkgPath: &str`：精确保存 Go 包路径 `github.com/pingcap/tidb/pkg/kv`，用于识别命名类型的包归属，而不是只比较类型名。
- `pub fn run(...)`：逐文件执行路径白名单过滤、AST 遍历、方法名预筛和类型签名确认，命中时调用 `pass.Reportf`。
- `pub fn isAllowedFile(filename: &str) -> bool`：经 `filepath::ToSlash` 统一路径分隔符后，检查路径是否包含 `pkg/table/tables/`。
- `pub fn isKVUpdateAssertionFlags(...) -> bool`：读取 selector 的类型选择信息，确认目标为函数签名，并接受普通方法调用的两个参数或方法表达式含 receiver 的三个参数。
- `pub fn isKVKey(...) -> bool` 与 `pub fn isKVAssertionOp(...) -> bool`：分别要求命名类型为 `Key`、`AssertionOp`，且声明包路径等于 `kvPkgPath`。
- `pub fn init()`：按顺序调用 `util::SkipAnalyzerByConfig(&Analyzer)` 和 `util::SkipAnalyzer(&Analyzer)`，保存 Go 初始化阶段的跳过注册语义；Rust 中普通 `init` 函数不会自动执行，且当前没有调用者。

所有符号在 Rust 草稿中都标为 `pub`，但由于文件未被生产模块树纳入，这些可见性声明目前不构成可供其他 crate 使用的公开 API。

## 执行流程

若按 Go 对照实现的 analyzer 生命周期理解，框架先根据 `Analyzer.requires` 准备 inspect analyzer，再调用 `run(pass)`：

1. 遍历 `pass.Files`，由文件集将每个 AST 文件节点的位置转换成文件名。
2. `isAllowedFile` 把路径转换为 slash 形式；路径包含 `pkg/table/tables/` 时直接跳过整个文件。
3. 对未跳过文件调用 `ast::Inspect` 深度遍历。非选择器节点、没有 `Sel` 的节点和名称不是 `UpdateAssertionFlags` 的选择器均继续遍历且不报告。
4. 对候选选择器调用 `isKVUpdateAssertionFlags`。它从 `pass.TypesInfo.Selections` 取选择信息，再依次确认对象是函数、类型是签名。
5. 参数数为 2 时按 `(0, 1)` 取 `Key` 和 `AssertionOp`；参数数为 3 时跳过第 0 个 receiver，按 `(1, 2)` 取两种业务参数；其他参数数量不匹配。
6. `isKVKey` 和 `isKVAssertionOp` 同时核对命名类型名与包路径。全部成立才在方法名位置报告诊断。
7. 遍历全部输入文件后返回 `Ok(None)`，不产生供其他 analyzer 使用的结果值。

RustCodeGraph 显示本文件内部静态调用边为 `run -> isAllowedFile`、`run -> isKVUpdateAssertionFlags`，以及 `isKVUpdateAssertionFlags -> isKVKey`、`isKVUpdateAssertionFlags -> isKVAssertionOp`；没有发现生产侧 Rust 调用者。

## 数据与状态

检查过程读取 `analysis::Pass` 中的文件列表、文件位置表和 `TypesInfo.Selections`。它不修改 AST 或类型信息，唯一预期输出是通过 `pass.Reportf` 追加诊断。`Analyzer` 和 `kvPkgPath` 是静态只读定义；每次节点判定只使用局部引用、参数索引和布尔返回值，没有缓存、跨文件聚合状态或持久化数据。

判定的不变量是：方法名、参数形状、两个命名类型名称及其包路径必须同时匹配。路径白名单则先于 AST/type 检查生效，因此合法目录中的调用无论具体表达式形状如何都不会进入报告流程。当前 Rust 文件本身没有可验证的运行时状态，因为相关 Go 风格类型和函数尚未由 Rust crate 提供。

## 依赖与调用关系

上游方面，Go 生产链路是 `build/BUILD.bazel` 中的 `tidb_nogo` 依赖 `//build/linter/assertionapi`，后者由 `build/linter/assertionapi/BUILD.bazel` 用 `analyzer.go` 构建。RustCodeGraph 对 Rust 文件报告 “used by 0 files”；仓库搜索也只发现 `pkg/lib.rs` 在测试配置下引用独立的 `analyzer_test.rs`，未发现把 `analyzer.rs` 声明为模块或调用其 `Analyzer`/`init` 的 Rust 生产入口。

下游逻辑依赖在源码中以 Go API 风格保存：`analysis::Analyzer`/`analysis::Pass` 提供 analyzer 契约和诊断，`inspect::Analyzer` 表达前置分析要求，`ast::Inspect`/`ast::SelectorExpr` 提供语法树遍历，`types` 提供函数签名与命名类型，`filepath`/`strings` 处理白名单路径，`util` 处理 analyzer 跳过注册。这些名称与同目录 Go 文件的 imports 一致，但根 `Cargo.toml` 没有为此目录建立对应 crate 边界，不能把它们视为已经解析的 Rust 外部依赖。

## 错误处理与边界

`run` 的签名允许返回 `analysis::Error`，但函数体没有显式构造或传播错误，正常结束总是 `Ok(None)`。类型信息缺失、选择对象不是函数、函数类型不是签名、参数数量不是 2 或 3、参数不是命名类型、类型对象没有包、类型名或包路径不匹配时，辅助函数都返回 `false`；这是一种保守策略，优先避免对不相关的同名标识符造成构建误报，但也意味着缺失类型信息时可能漏报。

白名单使用子串 `pkg/table/tables/` 而不是路径组件级比较：`filepath::ToSlash` 解决分隔符差异，但只要规范化字符串包含该片段就会获准。类型识别则只接受精确包路径和精确类型名；别名、未命名类型或不同包中的同名类型不会命中。方法表达式的 receiver 只通过参数数量 3 处理，超出两种预期形状直接跳过。

还需注意当前 Rust 草稿的实现边界：Go 风格字段名、方法名及占位命名空间未证明能在 Rust 中编译，普通 `init()` 也没有 Go `init` 的自动运行语义。因此本文描述的报告和错误边界以源码保存的意图及 Go 真实实现为依据，不宣称 Rust 路径已经运行。

## 并发与资源生命周期

文件没有线程、异步任务、锁、channel、事务或显式 I/O 资源。`run` 在一次 analyzer pass 的借用范围内同步遍历 `pass.Files`；AST 节点和类型对象均由 pass 所拥有的分析上下文借用，局部选择器、签名和参数引用不逃逸。遍历闭包返回 `true`，表示继续进入子节点，不承担资源清理。

`Analyzer` 按静态生命周期声明，理论上由分析框架共享；代码自身没有可变全局状态。`init` 中的两个 `util` 调用在 Go 版本属于包初始化期的全局注册/跳过副作用，调用顺序被 Rust 文本测试保存，但 Rust 草稿没有自动初始化接线，故不存在已经验证的 Rust 初始化生命周期。

## 与 Go 版本的对应关系

`build/linter/assertionapi/analyzer.go` 是可构建的权威对照，Rust 文件逐段保留其结构：`Analyzer` 的 Name/Doc/Requires/Run、`kvPkgPath` 常量、逐文件 AST 遍历、目录白名单、`Selections` 类型解析、2/3 参数分支、两种命名类型检查，以及两个 skip 注册调用均一一对应。Go 的 `sel.Sel == nil` 在 Rust 草稿中写为 `let Some(ident) = sel.Sel.as_ref() else`；Go 对 `n.Obj()` 和 `obj.Pkg()` 的 nil 检查则对应 Rust 的 `let Some(...) else`。

差异主要来自迁移状态而非业务策略：Go 文件有真实 imports、Bazel library 和 nogo 接线；Rust 文件只有列举 Go imports 的注释，没有生产模块和 Cargo 依赖。Go `var Analyzer = &analysis.Analyzer{...}` 是指针对象，Rust 写成值类型静态量；Go 的 `init()` 会自动运行，Rust 的同名普通函数不会。`analyzer_test.rs` 只以字符串断言检查可选值解包、包路径、参数索引和 skip 调用，没有执行 AST/type analyzer，也没有对应的 `analyzer_test.go` 行为测试。因此目前可确认的是文本级迁移契约保持，而不是 Rust 与 Go 的运行结果已等价。

## 扩展指南

若要改变限制范围或识别规则，应在最小符号处修改并同步 Go 语义：目录规则集中在 `isAllowedFile`，目标 API 名在 `run`，签名形状在 `isKVUpdateAssertionFlags`，类型归属在 `isKVKey`、`isKVAssertionOp` 和 `kvPkgPath`。新增合法目录时要特别评估子串匹配是否会意外放行相似路径；支持新签名时应明确 receiver 与业务参数索引，不宜只扩大参数数量；支持新类型时仍应核对包路径以维持低误报。

任何 Rust 逻辑变更都应同步更新独立测试 `build/linter/assertionapi/analyzer_test.rs`，不要把测试内嵌进生产文件。现有测试只覆盖源码形状，若未来把 analyzer 真正接入 Rust，应另增可执行的 AST/type 场景测试，至少覆盖：允许目录、同名非 kv 方法、缺失 selection、2 参数方法调用、3 参数方法表达式、错误参数数量、错误类型名/包路径和诊断位置。同时需要新增明确的 Rust 模块/crate 边界、真实依赖与初始化注册入口；在这些接线完成前，不应把 `pub` 或 `init` 当作对外可用能力。

兼容性风险主要是误报和漏报：放宽名称/类型条件会误伤同名 API，保守返回会在类型信息不足时漏报；路径策略变化会改变哪些表层代码被豁免。该 analyzer 仅用于构建检查，单次线性遍历 AST，通常性能风险较低，但重复类型查询或增加全局扫描仍应通过真实 analyzer 测试评估。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；目标目录列出 `analyzer.go`、`analyzer.rs`、`analyzer_test.rs`。
- RustCodeGraph `node --file build/linter/assertionapi/analyzer.rs`：读取目标文件全部 154 行，确认 1 个静态 analyzer、1 个常量和 6 个函数，无条件编译项。
- RustCodeGraph `explore`/调用边：确认 `run` 调用 `isAllowedFile` 与 `isKVUpdateAssertionFlags`，后者调用 `isKVKey` 与 `isKVAssertionOp`；目标 Rust 文件没有被其他已索引文件使用。
- `build/linter/assertionapi/analyzer.go`：核对可运行 Go 版本的 AST 遍历、类型筛选、诊断文本和初始化行为。
- `build/linter/assertionapi/analyzer_test.rs`：核对三个独立 Rust 文本契约测试及其边界；测试通过 `include_str!` 读取生产草稿，不执行 analyzer。
- `build/linter/assertionapi/BUILD.bazel` 与 `build/BUILD.bazel`：核对 Go library 依赖和 `tidb_nogo` 接线。
- 根 `Cargo.toml` 与 `pkg/lib.rs`：核对根 crate 路径、关闭自动测试发现、目标目录未成为 workspace crate，以及独立测试只在 `cfg(test)` 下接入。
- 仓库搜索 `assertionapi`：未发现 Rust 生产模块声明或调用者；发现的 Rust 接线仅为上述测试模块。
- 按任务约束未运行 Cargo；交付前使用任务给定命令验证文档存在且恰有 11 个固定二级标题，并人工复核没有把迁移草稿描述成已运行能力。
