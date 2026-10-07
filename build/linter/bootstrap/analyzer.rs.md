# `build/linter/bootstrap/analyzer.rs`

## 文件定位

本文件位于构建辅助目录 `build/linter/bootstrap/`，是同目录 Go analyzer `analyzer.go` 的机械迁移草稿，用于描述对 TiDB `bootstrap.go` 与 `upgrade_def.go` 的静态一致性检查。它不属于 SQL 执行、会话 bootstrap 或升级逻辑本身，也不会执行建表或升级；检查对象是 Go AST 中的 bootstrap 元数据声明。

当前 Rust 文件不是可运行 lint 组件。源码开头明确写明“当前不保证可编译”，RustCodeGraph 将它识别为包含 8 个符号、`used by 0 files`；根 `Cargo.toml` 的 workspace 没有 `build/linter/bootstrap` crate，`pkg/lib.rs` 也只在 `#[cfg(test)]` 下通过路径纳入独立测试 `analyzer_test.rs`，并未纳入本生产文件。相对地，`build/linter/bootstrap/BUILD.bazel` 只把 `analyzer.go` 声明为 Go `go_library`。

## 核心职责

文件保留两组互补规则：

1. `bootstrap.go` 规则验证系统表定义的 ID、表名、建表 SQL 常量命名彼此对应，并确认所有 `TableBasicInfo` 定义集合恰好被唯一的 `versionedBootstrapSchema` 引用。
2. `upgrade_def.go` 规则从版本常量、升级函数、升级函数表尾项和 `currentBootstrapVersion` 四处提取最高版本，要求四者为相同的非零值。

因此该 analyzer 防止的是维护 bootstrap 元数据时“只改一处”的遗漏，而不是验证 SQL 内容、表结构正确性或升级函数的运行结果。`run` 只按文件名后缀分派，当前包内其他 Go 文件不会进入这两套规则。

## 主要符号

- `Analyzer: analysis::Analyzer`：公开的 analyzer 描述，名称为 `bootstrap`，无前置 analyzer，入口指向 `run`。当前声明为不可变 `static`，而 `init` 调用的 Rust `util::SkipAnalyzerByConfig` 实际需要 `&mut analysis::Analyzer`，这是草稿不可直接编译的证据之一。
- `bootstrapCodeFile` / `upgradeCodeFile`：分别为 `"/bootstrap.go"` 和 `"/upgrade_def.go"`，供 `run` 做路径后缀匹配。
- `run(pass)`：遍历 `pass.Files`，从 `pass.Fset` 取得文件名后分别调用 `checkBootstrapDotGo` 或 `checkUpgradeDotGo`，成功路径始终返回 `Ok(None)`。
- `isSliceVarDefNode(spec)`：识别“单个名字、单个值、值为数组/切片复合字面量、元素类型为标识符”的声明形状，返回变量名、元素类型名、复合字面量和成功标记。
- `checkSystemTablesDefinitionNode`：检查每个系统表条目的前三个字段，要求 ID 与 SQL 选择器来自 `metadef`，并校验 `XxxTableID`、`CreateXxxTable` 和去下划线后的真实表名能对应。
- `checkVersionedBootstrapSchema`：收集每个版本条目 `databases` 中第三字段 `Tables` 引用的变量名，通过 `maps::Equal` 与已发现的 schema 定义名集合比较。
- `checkBootstrapDotGo`：扫描通用声明；对 `TableBasicInfo` 切片做内容检查并登记名称，对 `versionedBootstrapSchema` 切片做集合检查，最后要求前者至少一个、后者恰好一个。
- `checkUpgradeDotGo`：计算四个版本来源的最大/当前值，检查版本常量顺序和值，并在不一致时分别定位报告。
- `init`：保留 Go `init` 的配置过滤接线意图，调用 `util::SkipAnalyzerByConfig(&Analyzer)`；当前 Rust 类型和可变性并未完成接线。

## 执行流程

预期入口来自 analyzer 框架调用 `Analyzer.run`。`run` 对 analysis pass 中每个文件取完整路径：路径以 `/bootstrap.go` 结尾时进入 bootstrap 分支，以 `/upgrade_def.go` 结尾时进入 upgrade 分支；两个独立 `if` 保留 Go 逻辑，不过正常路径不可能同时满足两个后缀。

bootstrap 分支先按源码声明顺序扫描。`TableBasicInfo` 切片会立即加入 `foundVarNames`，并逐条核对 ID、表名和 SQL 名称；遇到 `versionedBootstrapSchema` 时，使用当时已经收集的名称集合核对其 `databases[*].Tables` 引用。由此存在一个重要维护前提：schema 定义应出现在版本化总表之前，否则集合比较会把后置定义视为缺失。扫描结束还会报告没有 schema 定义或版本化总表数量不为一。

upgrade 分支并行维护 `maxVerVariable`、`maxVerFunc`、`maxVerFuncUsed`、`curVerVariable` 及各自位置。多规格 `const` 块提供 `versionN = N` 序列；函数声明提供最大的 `upgradeToVerN`；`upgradeToVerFunctions` 的最后一项提供实际接入的最高函数；`currentBootstrapVersion` 提供当前版本别名。只有四值相等且非零才静默返回，否则先报告总的不一致，再逐项报告数值。

## 数据与状态

分析过程只使用 pass 内 AST 和局部状态，不修改被分析源码。bootstrap 分支的两个 `HashMap<String, ()>` 被当作集合使用：一个保存已定义 schema 变量，另一个保存版本化列表使用的变量；比较不关心顺序，但要求成员完全相等。两个计数器分别约束定义集合至少一个、版本化总表恰好一个。

upgrade 分支使用四组整数和 `token::Pos`。初值为 `0`/`NoPos`；若来源缺失，最终非零一致性条件失败，并可能在 `NoPos` 报告该缺失来源。`upgradeToVerFunctions` 的最后元素被视为最高版本，隐含列表按版本递增的维护不变量；版本常量也按声明遍历顺序要求单调不降。

不存在全局可变运行状态、缓存或持久化数据。唯一全局值 `Analyzer` 和两个路径常量是静态描述信息；配置过滤原意是在初始化时包装 analyzer 的 `Run`，但 Rust 草稿尚未形成可编译的可变初始化方案。

## 依赖与调用关系

RustCodeGraph 给出的文件内调用边是：`run → checkBootstrapDotGo`、`run → checkUpgradeDotGo`；`checkBootstrapDotGo → isSliceVarDefNode`、`checkSystemTablesDefinitionNode`、`checkVersionedBootstrapSchema`。各辅助函数没有目标文件外的 Rust 调用者，目标文件本身也显示为 `used by 0 files`。

概念依赖包括占位的 Go 风格 `analysis`、`ast`、`token`、`maps`、`strconv`、`strings` 与 `util` API。它们分别承担 analysis pass/诊断、AST 形状、源码位置、集合比较、数字/字符串解析和配置过滤；源码没有真实 Rust `use` 声明或可解析 crate 依赖，不能据此声称这些依赖已经实现。

可运行链位于 Go 侧：`build/linter/bootstrap/BUILD.bazel` 将 `analyzer.go` 构建为 `//build/linter/bootstrap`；`build/debug-linter/main.go` 导入该包，并调用 `singlechecker.Main(bootstrap.Analyzer)`。其 Bazel 直接依赖是 `//build/linter/util` 和 `@org_golang_x_tools//go/analysis`。Rust `build/debug-linter/main.rs` 也只是未保证可编译的入口形状草稿。

## 错误处理与边界

可恢复的规则违例通过 `pass.Reportf` 形成诊断：包名不是 `metadef`、命名不一致、表名无法反引号、`Tables` 不是变量、版本常量缺值/值不匹配/顺序倒退，以及四个版本来源不一致均属此类。`run` 的返回类型允许 `analysis::Error`，但自身没有产生或传播错误的分支，正常返回 `Ok(None)`。

大量 AST 下钻使用 `expect`，版本字符串的某些解析使用 `panic!`。这刻意对应 Go 连续类型断言和显式 panic：被检查文件若偏离预期语法形状，analyzer 可能中止，而不是生成温和诊断。尤其是空的 `upgradeToVerFunctions`、字段不足、错误字段类型、短于固定前缀却被切片的名字，均不受完整防御。相反，`isSliceVarDefNode` 对不相关声明返回 `false`，版本函数/常量后缀无法解析为整数时会跳过。

路径匹配硬编码 `/`，没有独立处理 Windows 分隔符。表名比较将真实表名去除 `_`，再把 ID 常量主干转小写；它只验证约定的词法对应，不解析 SQL 常量内容。集合检查发生在扫描时，因而依赖声明顺序。这些都是扩展时必须保留或有意修改并补测试的边界。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、网络或文件句柄。所有局部集合、计数器和位置随一次 analyzer pass/函数调用创建并在返回时释放；AST 与文件集合由 `analysis::Pass` 借用管理。

预期框架可对不同 package 并行运行 analyzer，但本逻辑没有跨 pass 共享的可变业务状态。若将 Rust 草稿真正接线，`init` 对全局 analyzer 的包装方式必须解决安全初始化与可变性，不能直接把当前不可变 `static Analyzer` 以共享引用传给需要可变引用的 `SkipAnalyzerByConfig`。

## 与 Go 版本的对应关系

`build/linter/bootstrap/analyzer.go` 是当前权威、可由 Bazel 构建的实现；Rust 文件逐函数保留其结构和诊断文本：`Analyzer`、两个文件后缀、`run`、五个检查辅助函数和 `init` 均可一一对应。Rust 用 `HashMap<String, ()>` 模拟 Go `map[string]struct{}`，用数组迭代的 `min`/`max` 模拟 Go 内建的四参数 `min`/`max`，用 `Option`/模式匹配表达部分类型断言，并用 `expect` 保留其余强制断言的 panic 契约。

差异主要是迁移状态而非业务设计：Go analyzer 是指针并可在 `init` 中被包装，Rust 声明当前不可变；Go AST、analysis 和工具包具有真实依赖，Rust 名称仍是占位接口；Go Bazel 目标包含 `analyzer.go`，Rust workspace 没有目标文件对应 crate。`build/linter/bootstrap/analyzer_test.rs` 的四个测试只通过 `include_str!` 检查 Rust 源码是否包含关键文本与分支契约，不构造 AST、不调用这些函数，也不证明目标文件可编译或可执行。仓库中没有同目录 `analyzer_test.go`。

## 扩展指南

新增 bootstrap schema 约束时，应优先在 `checkSystemTablesDefinitionNode` 或 `checkVersionedBootstrapSchema` 接入；改变识别的声明形状时修改 `isSliceVarDefNode`；新增版本来源或调整升级命名规则时集中修改 `checkUpgradeDotGo` 的四源收集与最终比较。新增受检文件则修改两个路径常量及 `run` 分派。任何变化都应先与 `analyzer.go` 的实际行为对齐，避免 Rust 文本先于可运行 Go 规则产生虚假能力描述。

若要让 Rust 版本真正运行，需要另行建立 crate/module 边界，实现或引入 Rust 侧 AST/analysis/token/工具接口，解决 `Analyzer` 的静态可变初始化，并把生产文件纳入 Cargo；这些属于当前单文件文档任务范围外，不能用现有文本测试替代。测试必须继续放在独立的 `build/linter/bootstrap/analyzer_test.rs`，不应内嵌进生产文件。建议在现有四项源码契约测试之外增加可执行 AST fixture，覆盖正常 schema、错误命名、漏挂 schema、版本四源不一致、畸形 AST panic，以及路径与声明顺序边界；若 Go 规则同步变化，还应为 Go analyzer 增加同等语义测试。

兼容性风险集中在诊断文本、AST 形状假设和 Go/Rust 行为偏离；正确性风险集中在声明顺序和只取函数表尾项；性能风险较低，主体为对 pass 文件和目标 AST 的线性扫描，集合操作按名称数量近似常数，但不应为便利重复遍历完整 AST。

## 验证依据

- RustCodeGraph `status`：索引含 7,032 个 Rust 文件；`files --filter build/linter/bootstrap` 找到 Go/Rust 实现和 Rust 独立测试。
- RustCodeGraph `node --file build/linter/bootstrap/analyzer.rs --offset 1 --limit 260` 及 `--offset 261 --limit 320`：核对 551 行目标文件的全部符号、分支、诊断和注释；结果标明 `used by 0 files`。
- RustCodeGraph `explore "build/linter/bootstrap/analyzer.rs symbols callers callees bootstrap analyzer"` 与精确调用流查询：确认 `run` 的两条分派边、bootstrap 分支的三个辅助调用边，以及无文件外 Rust 调用者。
- `build/linter/bootstrap/analyzer.go`：逐函数核对 Go 权威实现、类型断言、诊断文本、版本四源比较和 `init`。
- `build/linter/bootstrap/analyzer_test.rs` 与 `pkg/lib.rs`：确认四个测试是独立文件中的源码文本契约测试，并仅由根 crate 在 `#[cfg(test)]` 下纳入。
- 根 `Cargo.toml`、`build/linter/bootstrap/BUILD.bazel`、`build/debug-linter/main.go`、`build/debug-linter/main.rs`、`build/linter/util/util.rs`：核对 Rust crate 未接线、Go Bazel 边界与 debug 入口，以及 Rust 配置包装函数要求可变 analyzer。
- 本任务是纯文档分析，按计划不运行 Cargo；结构通过固定十一章节命令验证，内容人工复核为未把草稿描述成已支持能力。
