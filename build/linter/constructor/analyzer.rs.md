# `build/linter/constructor/analyzer.rs`

## 文件定位

该文件位于构建辅助模块 `build/linter/constructor`，是同目录 [`analyzer.go`](./analyzer.go) 的 Rust 机械迁移草稿。它描述名为 `constructor` 的 Go 静态分析规则：结构体通过 `constructor.Constructor` 标记字段及 `ctor:"..."` tag 声明允许手工构造它的函数名，规则在其他函数内发现受管控结构体的直接字面量、`new`、零值变量或隐式嵌入字段构造时报告诊断（`Analyzer`、`getConstructorList`、`run`）。

当前生产接线仍是 Go：[`BUILD.bazel`](./BUILD.bazel) 的 `go_library(name = "constructor")` 只以 `analyzer.go` 为源文件，并在根 [`../../BUILD.bazel`](../../BUILD.bazel) 的 `with_nogo` 分支注册该 target。Rust 根 workspace 没有 `build/linter/constructor` crate；[`../../../pkg/lib.rs`](../../../pkg/lib.rs) 只在 `#[cfg(test)]` 下挂载 [`analyzer_test.rs`](./analyzer_test.rs)，而测试通过 `include_str!("analyzer.rs")` 读取文本，并不编译本文件。因此源码开头所说的“当前不保证可编译”是实际边界，不能把本文件视为已接入的 Rust analyzer。

## 核心职责

- `Analyzer` 保存规则名、说明、空前置依赖集合和 `run` 回调，保留 Go `analysis.Analyzer` 的配置形状。
- `getConstructorList` 从结构体或结构体指针的字段类型、包路径和 `ctor` tag 提取允许的构造函数名，并递归检查命名嵌套结构体。
- `assertInConstructor` 从 AST 栈向外找到最近的函数声明，判断函数名是否属于白名单；不属于时通过 `Pass::Reportf` 报告。
- `handleCompositeLit`、`handleCallExpr`、`handleValueSpec` 分别覆盖复合字面量、内建 `new(T)`、`var v T` 三类入口，并共同复用白名单提取和报告逻辑。
- `run` 为每个文件创建 inspector，通过带栈遍历把三类节点分派到对应 handler；`init` 保留按配置跳过 analyzer 的 Go 初始化意图。

规则不负责推荐或调用某个构造器，也不验证构造器签名；它只把 tag 中以逗号分隔的名称当作函数名白名单。源码还明确保留“泛型支持尚待验证”的 Go TODO。

## 主要符号

- `pub const ConstructorUtilPath: &str`：固定为 `github.com/pingcap/tidb/pkg/util/linter/constructor`。识别标记字段时必须同时满足命名类型名为 `Constructor` 和包路径等于该常量。
- `pub static Analyzer: analysis::Analyzer`：规则描述符；`name = "constructor"`，`requires = &[]`，`run` 指向本文件函数。这里的 `analysis` 是尚未接线的占位 API。
- `pub fn getConstructorList(t, ignoreFields) -> Vec<String>`：核心类型递归。输入不是结构体或结构体指针时返回空列表；合法 marker tag 通过 `structtag::Parse` 和 `Get("ctor")` 读取，随后用 `strings::Split(..., ",")` 覆盖当前 `ctors`；非 marker 的命名结构体字段则递归追加其结果。
- `pub fn assertInConstructor(pass, n, stack, ctors) -> bool`：只判断最近外层 `FuncDecl`。命中白名单返回 `true`，不命中时报告 `struct can only be constructed in constructors %s` 并返回 `false`；若栈中没有函数声明则返回 `true`。
- `pub fn handleCompositeLit(...) -> bool`：只在 `push == true` 的进入阶段处理。它收集字面量中已显式赋值的字段名，避免把这些字段误判为由外层字面量隐式零值构造。
- `pub fn handleCallExpr(...) -> bool`：只处理函数表达式是标识符、名字为 `new` 且至少有一个参数的调用；`push` 参数目前没有参与判断，与 Go 版本一致。
- `pub fn handleValueSpec(...) -> bool`：处理变量声明；类型信息缺失时放行，指针声明因只产生 nil 指针而放行。
- `pub fn run(...) -> Result<Option<Box<dyn Any>>, analysis::Error>`：遍历入口，正常结束固定返回 `Ok(None)`；规则违规通过 pass 中的诊断通道输出。
- `pub fn init()`：调用 `util::SkipAnalyzerByConfig(&Analyzer)`。Rust 没有 Go 式自动 `init` 语义，而且该函数当前无生产调用者。

## 执行流程

1. Go 分析框架调用 `Analyzer.Run`；Rust 草稿用 `Analyzer.run = run` 表达同一关系。`run` 遍历 `pass.Files`，对每个 AST 文件创建一个 `inspector`。
2. `WithStack` 只订阅 `CompositeLit`、`CallExpr` 和 `ValueSpec`，闭包按节点变体分派三个 handler，并把当前 `push` 状态与完整 AST 栈传入。
3. 对复合字面量，`handleCompositeLit` 在进入阶段取得该节点的底层类型。具名元素把 `KeyValueExpr` 的标识符 key 加入 `ignoreFields`；位置元素在底层类型是结构体时按元素下标映射回字段名。这样外层只检查没有显式提供的字段，已提供字段交由递归 AST 遍历检查其值表达式。
4. 对调用表达式，`handleCallExpr` 仅识别裸标识符 `new`；它从调用结果的底层类型提取构造器列表。普通函数调用、选择器调用或无参数调用直接放行。
5. 对变量声明，`handleValueSpec` 从显式类型语法取得类型。`var p *T` 放行；其他类型取底层类型并查找构造器约束。
6. 三个 handler 在 `ctors` 为空时返回 `true`；否则调用 `assertInConstructor`。后者反向扫描栈，遇到最近函数声明便停止：白名单函数内允许继续遍历，其他函数内报告一次并返回 `false`。
7. 全部文件遍历结束后 `run` 返回 `Ok(None)`；它不生成 analysis result object。

`getConstructorList` 的子流程是：先把 `t` 归一成结构体（允许指向结构体的指针），再逐字段处理；非命名字段跳过；被 `ignoreFields` 命中的字段跳过；marker 字段解析 tag；其他命名字段若底层仍是结构体则递归。RustCodeGraph 的调用边验证了 `run -> 三个 handler`，以及每个 handler 到 `getConstructorList`/`assertInConstructor` 的共享链路。

## 数据与状态

本文件没有数据库状态、事务状态或可变全局业务数据。进程级 `Analyzer` 是静态描述符，`ConstructorUtilPath` 是静态字符串；每次 pass 的 AST、类型信息和诊断收集由外部 `analysis::Pass` 管理。

主要短生命周期数据是 `getConstructorList` 的 `Vec<String>` 白名单，以及 `handleCompositeLit` 的 `HashMap<String, ()>` 忽略集合。白名单保持 tag 中名称的字符串顺序，不去空格、不去重，也不合并多个 marker 字段：命中 marker 时使用赋值覆盖 `ctors`，之后仍可由嵌套结构体递归追加。忽略集合只表示外层字面量已显式给值的字段，不代表该值本身安全。

测试夹具 [`testdata/src/t/construct.go`](./testdata/src/t/construct.go) 给出真实语义矩阵：两个 tag 白名单函数内的直接构造、`new` 和零值声明允许；其他函数中的值/指针字面量、切片元素、匿名结构体、`new(T)`、`var T`、嵌入值字段的隐式构造应报告；`var *T`、显式使用允许构造函数填充字段、嵌入指针的零值则允许。Rust 对照夹具保留了 11 条相同诊断注释，但只是语义文本映射，不是由 Rust analyzer 实际运行所得。

## 依赖与调用关系

当前真实生产上游是 Go nogo 构建链：

```text
build/BUILD.bazel --with_nogo--> //build/linter/constructor:constructor
                                  └── Go Analyzer.Run -> run

Rust 草稿内部：
Analyzer.run -> run
                ├── handleCompositeLit ─┬── getConstructorList
                │                      └── assertInConstructor -> Pass::Reportf
                ├── handleCallExpr ─────┤
                └── handleValueSpec ────┘

init -> util::SkipAnalyzerByConfig
```

下游抽象依赖来自 Go 语义的 Rust 占位命名空间：`ast` 提供节点和位置，`types` 提供结构体/指针/命名类型及字段 tag，`structtag` 解析 tag，`strings`/`slices` 完成拆分、连接和成员判断，`analysis` 承载 pass/诊断，`inspector` 提供带栈遍历，`util` 提供配置跳过登记。当前没有 Cargo manifest 为这些依赖建立可编译关系。

需要区分同名 marker crate：根 [`../../../Cargo.toml`](../../../Cargo.toml) 的 workspace member 和 facade 依赖是 [`../../../pkg/util/linter/constructor/Cargo.toml`](../../../pkg/util/linter/constructor/Cargo.toml)，它只导出零大小 `Constructor` 标记类型；它不是本 analyzer 的 crate，也不会使 `build/linter/constructor/analyzer.rs` 自动参与编译。Rust 测试的上游仅是 `pkg/lib.rs -> analyzer_test.rs -> include_str!("analyzer.rs")`。

## 错误处理与边界

- 目标类型不是结构体或结构体指针、字段不是命名类型、tag 无效或没有 `ctor` key 时，`getConstructorList` 静默跳过，最终可能放行；这与 Go 版本“无有效白名单声明即不受规则约束”的行为一致。
- `named.Obj()` 或 marker 对象的 `Pkg()` 缺失时，Rust 草稿使用 `expect` 并 panic；Go 版本直接解引用这些框架不变量。复合字面量与 `new` 调用缺失类型信息时 Rust 也 `expect`，而当前 Go 源在 `TypeOf(n).Underlying()` 后才做 nil 检查，实际同样依赖类型信息存在。`ValueSpec` 是例外：缺失类型信息会放行。
- `assertInConstructor` 只认最近的命名函数声明，不识别函数文字/闭包自身为白名单构造器；若整个栈没有 `FuncDecl`，函数返回 `true`，包级构造不会报告。
- 白名单按函数的简单名称比较，不校验包、receiver、签名或重载语义；tag 中空项、空格和重复名称也不会规范化。
- `handleCallExpr` 只看语法名字 `new`，没有用类型对象确认它是未被遮蔽的内建函数；这继承了 Go 源的判定方式。
- 位置式复合字面量依赖元素下标能映射到底层结构体字段；源码没有显式越界保护，依赖类型正确的 Go AST/类型信息不变量。
- 嵌套递归只沿命名字段的底层结构体展开，没有循环检测。合法 Go 类型不能按值形成无限递归结构；通过指针打断的递归类型在此处也不会作为底层结构体继续递归。
- 泛型适用性仍是未验证项，不能据此声称规则覆盖所有参数化类型构造。

## 并发与资源生命周期

实现没有线程、异步任务、锁、通道、文件句柄、网络连接或数据库资源。一次 `run` 同步、顺序遍历 `pass.Files`；每个文件创建局部 inspector，遍历结束后释放。handler 只在调用期间借用 pass、节点和栈，白名单与忽略集合均为调用栈上的临时所有值，不跨文件缓存。

诊断写入的同步和 pass 是否可并行由外部 Go analysis 框架决定，本文件没有提供额外并发保证。`Analyzer` 是只读描述符；`init` 可能通过外部 `util` 修改 analyzer 配置，但 Rust 草稿当前既没有自动初始化机制，也没有生产调用链，因此不存在已验证的 Rust 初始化生命周期。

性能成本主要来自对三个节点类别的整树遍历，以及每个候选构造沿字段递归查找 marker。源码没有 memoization；同一类型在多个构造点会重复扫描。通常字段数和嵌套深度有限，但扩展递归范围时需要留意分析时延。

## 与 Go 版本的对应关系

Rust 文件逐项保留 [`analyzer.go`](./analyzer.go) 的常量、全局 analyzer、六个函数及控制流：结构体/指针归一化，marker 名称与包路径双重匹配，无效 tag 跳过，嵌套结构体递归，最近函数名白名单判定，复合字面量显式字段排除，`new` 和 `var` 分支，三类节点的 `WithStack` 分派，以及按配置跳过的 `init`。

表示差异主要是机械类型映射：Go 的 `nil` map/切片用 Rust `Option<HashMap<...>>`/`Vec` 表达，Go 类型断言用 `as_struct`/`as_pointer` 等占位方法表达，Go `(any, error)` 用 `Result<Option<Box<dyn Any>>, analysis::Error>` 表达。`Analyzer` 在 Go 中是指针变量，在 Rust 草稿中是静态值；Go `init()` 自动运行，Rust 普通 `init` 函数不会自动运行。

Go 测试 [`analyzer_test.go`](./analyzer_test.go) 通过 `analysistest.Run` 对 `t` 与 marker 包运行真实 analyzer，并用反射校验 `ConstructorUtilPath`；目标带 `//go:build !intest`，注释说明 CI PATH 中缺少 `go` 的历史限制。Rust 测试 [`analyzer_test.rs`](./analyzer_test.rs) 则检查源码必须含有预期 API/分支、类型信息处理文本、marker 匹配条件，以及 Rust fixture 恰有 11 条诊断注释。它没有构造 AST、没有调用 `run`，也没有证明目标源码可编译。

因此可以确认的是“Rust 文本保留了 Go 规则的结构和测试矩阵”；无法确认且当前接线否定的是“Rust analyzer 已可执行或已替代 Go nogo 规则”。

## 扩展指南

- 新增构造入口形态时，应在 `run` 的节点过滤器中增加对应节点，并新增独立 handler；同步修改 Go `analyzer.go`、Go analysistest fixture、Rust `analyzer_test.rs` 的源码契约和 Rust/Go 对照 fixture，避免只让文本断言通过。
- 修改 tag 语法或 marker 身份时，入口是 `ConstructorUtilPath` 和 `getConstructorList`。必须覆盖无效 tag、缺失 `ctor`、多 marker、嵌套结构体、指针字段、空格/重复/空白名单等边界，并评估既有结构体的误报与漏报变化。
- 修改允许位置的判断时，入口是 `assertInConstructor`。若要支持方法、闭包或限定路径，不能只比较简单函数名；应明确 receiver/包身份和最近作用域规则，并保持诊断文本兼容。
- 修改复合字面量逻辑时，重点维护“显式字段由子表达式检查、外层只检查隐式零值字段”的不变量；具名与位置式字面量都要有回归测试。
- 若要把该草稿变成生产 Rust analyzer，需要单独建立真实 crate/module、AST/type-analysis 依赖、注册入口与初始化策略，并把当前 `include_str!` 契约测试升级为可执行行为测试；不能仅把 marker crate 当作 analyzer 接线。
- Rust 测试继续保存在同目录独立的 `analyzer_test.rs`，不要嵌入生产文件。Go 是当前可执行语义基准，迁移时不得删减 11 个诊断场景或允许场景。

兼容风险集中在 tag 解析、名称解析、诊断位置/文本和误报/漏报集合；性能风险集中在类型字段递归与同类型重复扫描。泛型、遮蔽的 `new`、包级构造和函数文字是新增行为前必须明确的既有边界。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter build/linter/constructor` 列出目标、Go 对照、测试和四个 fixture；`query` 定位 Rust/Go 两套 `getConstructorList`、`assertInConstructor`、三个 handler；`callees handleCompositeLit` 验证其到 `getConstructorList`/`assertInConstructor` 的边，`callees run` 验证 Rust `run` 到三个 handler 的边。常见名称查询存在跨仓库同名噪声，结论只采纳路径明确位于本目录的边。
- 目标源码：[`analyzer.rs`](./analyzer.rs) 的 `ConstructorUtilPath`、`Analyzer`、`getConstructorList`、`assertInConstructor`、`handleCompositeLit`、`handleCallExpr`、`handleValueSpec`、`run`、`init`；文件头明确声明机械迁移、占位依赖和当前不保证可编译。
- Go 对照与生产构建：[`analyzer.go`](./analyzer.go)、[`BUILD.bazel`](./BUILD.bazel)、根 [`../../BUILD.bazel`](../../BUILD.bazel) 的 `with_nogo` 选择分支。
- Cargo/模块边界：根 [`../../../Cargo.toml`](../../../Cargo.toml) 只纳入 marker crate；[`../../../pkg/lib.rs`](../../../pkg/lib.rs) 只在测试配置挂载 `analyzer_test.rs`；marker crate 的 [`../../../pkg/util/linter/constructor/Cargo.toml`](../../../pkg/util/linter/constructor/Cargo.toml)、[`../../../pkg/util/linter/constructor/lib.rs`](../../../pkg/util/linter/constructor/lib.rs) 和 [`../../../pkg/util/linter/constructor/constructorflag.rs`](../../../pkg/util/linter/constructor/constructorflag.rs) 证明其职责仅为导出标记类型。
- 独立测试：[`analyzer_test.rs`](./analyzer_test.rs) 的五组文本/fixture 契约测试；[`analyzer_test.go`](./analyzer_test.go) 的真实 `analysistest.Run` 与路径反射校验。
- 夹具：[`testdata/src/t/construct.go`](./testdata/src/t/construct.go) 与 [`construct.rs`](./testdata/src/t/construct.rs) 的允许/诊断矩阵；[`testdata/src/github.com/pingcap/tidb/pkg/util/linter/constructor/constructorflag.go`](./testdata/src/github.com/pingcap/tidb/pkg/util/linter/constructor/constructorflag.go) 及 Rust 对照的 marker 形状。
- 本任务只新增文档，按计划未运行 Cargo。交付结构使用任务指定命令验证：目标文件必须存在，且固定二级标题恰好为 11 个。
