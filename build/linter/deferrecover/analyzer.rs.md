# `build/linter/deferrecover/analyzer.rs`

## 文件定位

[`analyzer.rs`](analyzer.rs) 是 Go 包 `build/linter/deferrecover` 的机械迁移草稿，意图表达一个名为 `recover` 的静态分析器：检查 `github.com/pingcap/tidb/pkg/util.Recover` 是否由 `defer` 直接调用。文件头第 15—17 行已经明确声明它“当前不保证可编译”，不会实际运行 `go/analysis` 或遍历真实 Go AST。

当前真正接入构建链的是同目录的 [`analyzer.go`](analyzer.go)：[`BUILD.bazel`](BUILD.bazel) 只把 `analyzer.go` 放入 `go_library(name = "deferrecover")`，根 [`build/BUILD.bazel`](../../BUILD.bazel) 再把 `//build/linter/deferrecover` 加入 nogo analyzer 依赖。Rust 根 crate 的 [`Cargo.toml`](../../../Cargo.toml) 没有为该目录声明独立 crate；[`pkg/lib.rs`](../../../pkg/lib.rs) 也只在 `#[cfg(test)]` 下挂载 [`analyzer_test.rs`](analyzer_test.rs)，测试通过 `include_str!("analyzer.rs")` 检查源码文本，并没有把本文件作为 Rust 模块编译。因此，本文件目前是迁移语义记录，不是可执行 Rust lint 的生产入口。

## 核心职责

本文件保留 Go analyzer 的三个层次：

1. `Analyzer` 描述检查器名称、说明、所需的 `inspect::Analyzer` 和回调 `run`。
2. `run` 逐个处理 `analysis::Pass.Files`，先确认文件导入了 TiDB 的 `pkg/util`，再扫描调用表达式，只锁定形如 `<该导入名>.Recover(...)` 的调用。
3. `init` 用 `util::SkipAnalyzerByConfig` 包装 analyzer，使配置中的 `exclude_files` 在真正分析前过滤文件。

规则的核心不变量是“调用表达式的直接父 AST 节点必须是 `DeferStmt`”。因此 `defer util.Recover()` 合法，而普通调用以及把 `util.Recover()` 放进匿名函数体后再 defer 该匿名函数，都不满足本规则。判断是语法级的，不分析控制流，也不验证被调函数的类型身份。

## 主要符号

- `pub static Analyzer: analysis::Analyzer`（第 29—35 行）：公开的 analyzer 描述值。其逻辑字段对应 Go 的 `Name: "recover"`、`Doc`、`Requires: inspect.Analyzer` 和 `Run: run`。RustCodeGraph 将其识别为本文件的模块级符号，但文件级关系显示没有生产文件使用本文件。
- `packagePath`（第 38 行）：目标导入路径 `github.com/pingcap/tidb/pkg/util`，用于排除其它包中同名的 `Recover`。
- `packageName`（第 39 行）：目标包未显式起别名时采用的默认标识符 `util`。
- `funcName`（第 40 行）：目标选择器名 `Recover`。
- `pub fn run(pass: &mut analysis::Pass)`（第 44—100 行）：主要扫描入口，返回 `Ok(None)`；诊断通过 `pass.Reportf` 旁路上报，而不是通过返回值携带。
- `pub fn init()`（第 103—106 行）：保留 Go 包初始化时安装配置过滤包装器的意图。

本文件没有自定义 struct、enum、trait、条件编译项或持久状态。命名沿用 Go 的驼峰形式；根 crate 的 `#![allow(non_snake_case, non_upper_case_globals)]` 只作用于真正纳入该 crate 的模块，并不能证明本文件已经接线或可编译。

## 执行流程

`Analyzer.run` 指向 `run` 后，预期流程如下：

1. 遍历 `pass.Files`，每次只分析一个 Go 源文件。
2. 用 `util::GetPackageName` 在该文件的 import 列表中查找 `packagePath`。未导入时返回空串并跳过整份文件；显式别名存在时返回别名，否则返回默认名 `util`。
3. 为当前单文件创建 `inspector::New(vec![file])`，以 `ast::Node::CallExpr` 为过滤类型调用 `WithStack`。
4. 回调在 AST 节点退出阶段（`push == false`）立即返回；进入阶段才继续。节点先被断言为 `CallExpr`，再依次筛选：被调表达式必须是 `SelectorExpr`、选择器左侧必须是标识符、标识符必须等于实际导入名、选择器名称必须为 `Recover`。
5. 命中后取 `stack[stack.len() - 2]` 作为调用表达式的直接父节点。父节点不是 `DeferStmt` 时，在调用位置报告 `Recover() should be directly called by defer`；无论是否报告，回调都返回 `true`，继续遍历子树。
6. 全部文件扫描结束后返回 `Ok(None)`。分析成功与“产生零条或多条诊断”相互独立。

Go 生产链中，`init` 会先用配置包装原始 `Run`；包装器复制 `Pass`、根据 analyzer 名称 `recover` 和文件名过滤 `Files`，再调用原始扫描函数。Rust 草稿试图保留相同顺序，但尚未形成可编译、可注册的执行链。

## 数据与状态

分析输入集中在 `analysis::Pass`：`Files` 提供待分析的 Go AST，`Reportf` 接收诊断位置与消息。每个文件的 `packageName` 是局部字符串，inspector 也是单文件局部对象；AST 栈由 `WithStack` 回调按当前遍历路径提供。函数不修改 AST，不缓存跨文件结果，最终结果固定为 `None`。

`Analyzer` 是模块级静态描述值，`init` 预期改变的是其 `Run` 包装关系，而不是扫描规则本身。配置来源是 [`build/nogo_config.json`](../../nogo_config.json) 的 `deferrecover.exclude_files`：当前排除 parser 生成文件、测试文件、其它生成文件、mock 文件和 `external/`。实际 Go 包装器位于 [`build/linter/util/util.go`](../util/util.go) 的 `SkipAnalyzerByConfig`，Rust 对照位于 [`build/linter/util/util.rs`](../util/util.rs)。

## 依赖与调用关系

RustCodeGraph 对 `build/linter/deferrecover/analyzer.rs` 建立了 3 个符号节点（`Analyzer`、`run`、`init`），但文件报告 `used by 0 files`；对限定名 `...analyzer.rs::run` 和 `...analyzer.rs::init` 的 callers/callees 查询也没有得到生产调用边。索引只把 `run` 中的通用方法名错误关联到其它测试文件的同名节点，不能作为真实跨文件调用证据，因此本文不采用该噪声边。

源码层面的预期下游依赖为：

- `analysis::Analyzer` / `analysis::Pass`：定义 analyzer 元数据、输入、返回类型和诊断接口。
- `inspect::Analyzer`：作为 prerequisite 声明，为 Go 分析框架提供 AST 检查能力；当前草稿的 `run` 又自行创建单文件 inspector，并未读取 prerequisite 的结果。
- `inspector::New` / `WithStack`：遍历调用表达式并提供父节点栈。
- `ast::{CallExpr, SelectorExpr, Ident, DeferStmt}` 的草稿抽象：完成语法形状匹配。
- `util::GetPackageName`：解析默认或显式 import 别名；`util::SkipAnalyzerByConfig`：安装文件过滤包装器。

生产上游是 Bazel nogo：[`build/BUILD.bazel`](../../BUILD.bazel) 依赖同目录 Go library，Go 的 `Analyzer` 才会被聚合执行。Rust 侧唯一直接关联是 [`analyzer_test.rs`](analyzer_test.rs) 读取本文件文本，以及 [`pkg/lib.rs`](../../../pkg/lib.rs) 在测试配置下注册该文本测试模块。

## 错误处理与边界

`run` 的正常返回始终是 `Ok(None)`，规则违规通过 `Reportf` 报告，不是 Rust `Err`。当前实现没有显式可恢复错误分支。以下边界必须保留或在真正移植时有意识地修正：

- `as_call_expr().expect(...)` 与 `sel.Sel.as_ref().expect(...)` 依赖 inspector 过滤和 Go AST 结构不变量；若适配层交付不符合约定，会 panic。
- `stack[stack.len() - 2]` 假定调用节点一定存在父节点且栈长至少为 2；正常 Go AST 遍历满足这一点，畸形适配输入则可能下标 panic。
- 只接受选择器左侧为单个标识符。点导入会让 `GetPackageName` 返回 `"."`，但调用不会呈现为 `.Recover` 选择器，因此不会命中；这与 Go 版本一致。
- 规则只按 import 路径与标识符文本匹配，不查询类型信息。若导入别名在局部作用域被同名变量遮蔽，形如 `util.Recover()` 的方法调用仍可能误报；反之，通过其它表达式间接访问函数不会命中。
- 没有导入目标包的文件被快速跳过；其它包的 `Recover`、非选择器调用、不同函数名均被忽略。
- 配置过滤发生在原始 `run` 之前，被排除文件不会进入扫描。当前 Rust `init` 以 `&Analyzer` 调用，而 Rust 对照工具函数签名要求 `&mut analysis::Analyzer`；`GetPackageName` 的草稿签名也接收拥有所有权的 `Vec`，而本文件传入 `&file.Imports`。这些是“当前不保证可编译”的具体证据，不能把草稿描述为已运行实现。

## 并发与资源生命周期

文件本身不创建线程、异步任务、锁、通道、事务或外部资源。`run` 同步遍历 `Pass.Files`；每个 inspector 和导入名只活到当前循环迭代结束，回调只在 `WithStack` 调用期间使用 `pass` 与局部导入名。它不持有文件句柄，不修改输入 AST，也没有显式清理阶段。

若未来把它接入可并发执行的 Rust analyzer 框架，需要重点解决静态 `Analyzer` 的初始化与可变包装：Go 在包初始化期原地替换 `Run`，而 Rust 的共享 `static` 不能安全地通过不可变引用修改。应采用框架认可的一次性构造/注册机制，避免可变全局状态和并发注册竞争；同时确认诊断回调在并行文件分析时是否要求 `Send`/`Sync`。当前源码没有提供这些保证。

## 与 Go 版本的对应关系

[`analyzer.go`](analyzer.go) 是行为基准，Rust 草稿逐段保持了以下语义：Analyzer 的名称、文档、inspect prerequisite、`run` 绑定；三个目标常量；逐文件 import 检查；单文件 inspector；只在 push 阶段处理；CallExpr/SelectorExpr/Ident 筛选；直接父节点必须是 DeferStmt；相同诊断文本；以及初始化时调用 `SkipAnalyzerByConfig`。

主要差异不是规则设计，而是运行状态与 API 可用性：Go 文件由 `BUILD.bazel` 编译并接入 nogo，Rust 文件没有生产模块入口；Go AST 类型断言和必填 `Sel` 字段在草稿中被翻译为 `Option`/`expect`；Go 返回 `(nil, nil)`，草稿返回 `Ok(None)`；Go 可在 `init` 中修改 analyzer 指针，草稿当前的不可变静态值与 Rust 工具函数的可变参数不匹配。Go 同目录没有 `analyzer_test.go`，因此没有独立的行为回归测试；现有 Rust [`analyzer_test.rs`](analyzer_test.rs) 仅验证关键源码片段存在与部分展开方式，不执行 AST 样例。

## 扩展指南

若只调整规则目标（包路径、默认名或函数名），应修改 `packagePath`、`packageName`、`funcName`，并同步 Go 基准与独立测试。若改变“直接 defer”的定义，应集中修改 `run` 中父栈判断，并增加正反 AST 样例，至少覆盖直接 defer、普通调用、匿名函数包装、import 别名、无目标 import、点导入和局部同名遮蔽。

若要把草稿变成真正可用 Rust 实现，不能只让文本测试通过：需要先确定 Rust `go/analysis`/AST 适配 crate 的真实所有权与字段 API，修正 `GetPackageName` 参数所有权、静态 Analyzer 的可变初始化、`Run` 回调类型和 inspector 节点表示；再把生产模块显式接入某个 Cargo crate/注册表。测试逻辑必须继续放在独立的 [`analyzer_test.rs`](analyzer_test.rs)，不要内嵌到生产文件。接线后还需补行为测试，证明诊断位置、配置排除和别名解析与 Go 一致。

兼容风险主要是误改 Go 规则语义或配置名称，导致 nogo 诊断集变化；正确性风险集中在无类型信息的遮蔽误报和父栈边界；性能风险较低，因为先按 import 快速过滤，之后对每个相关文件做一次线性 AST 遍历。若复用 prerequisite 的 inspector，可避免重复构造，但必须先证明其文件范围与栈语义等价。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter build/linter/deferrecover` 列出 Go/Rust 源和 Rust 测试；`node --file` 核对了本文件 106 行源码与测试 40 行源码；限定名 `node/callers/callees` 核对 `run`、`init`，未发现真实生产调用者。
- Rust 源与测试：[`analyzer.rs`](analyzer.rs) 的 `Analyzer`、三个常量、`run`、`init`；[`analyzer_test.rs`](analyzer_test.rs) 的三个文本契约测试。
- Go 对照与构建：[`analyzer.go`](analyzer.go)、[`BUILD.bazel`](BUILD.bazel)、[`build/BUILD.bazel`](../../BUILD.bazel)、[`build/nogo_config.json`](../../nogo_config.json)。目录中不存在 `analyzer_test.go`。
- Cargo 与模块边界：根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace/package 声明；[`pkg/lib.rs`](../../../pkg/lib.rs) 仅在测试配置下挂载 `analyzer_test.rs`。
- 直接辅助实现：[`build/linter/util/util.go`](../util/util.go) 与 [`build/linter/util/util.rs`](../util/util.rs) 中的 `GetPackageName`、`SkipAnalyzerByConfig`。
- 本任务是纯文档分析，按任务约束未运行 Cargo；最终结构检查要求本文恰好包含本计划规定的 11 个二级标题。
