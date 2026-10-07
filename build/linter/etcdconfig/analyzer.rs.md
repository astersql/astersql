# `build/linter/etcdconfig/analyzer.rs`

## 文件定位

本文件是 Go linter `build/linter/etcdconfig/analyzer.go` 的 Rust 机械迁移草稿，位于构建辅助目录 `build/linter/etcdconfig`。它描述一个名为 `etcdconfig` 的静态分析器：扫描 Go 源文件中的 `go.etcd.io/etcd/client/v3.Config` 复合字面量，要求显式设置 `AutoSyncInterval`。

当前应把它理解为迁移语义的记录，而不是已接入的 Rust linter。文件头第 15～17 行明确说明“不保证可编译”且不会运行真实 `go/analysis`；RustCodeGraph 的文件查询也显示该文件 `used by 0 files`。目录中没有 `Cargo.toml` 或 Rust 模块入口，根 `Cargo.toml` 的 workspace 成员没有 `build/linter/etcdconfig`，仓库内 Rust 引用搜索只找到本文件和读取其文本的 `analyzer_test.rs`。

真实的生产接线仍在 Go/Bazel 侧：`build/linter/etcdconfig/BUILD.bazel` 定义 `go_library(name = "etcdconfig")`，`build/BUILD.bazel` 将 `//build/linter/etcdconfig` 纳入 nogo 依赖，`build/nogo_config.json` 为名为 `etcdconfig` 的分析器配置排除文件。

## 核心职责

- `Analyzer` 保存分析器元数据，将名称 `etcdconfig`、说明文本、前置分析器 `inspect::Analyzer` 和回调 `run` 关联起来（`analyzer.rs:28-34`）。
- `run` 逐文件识别 etcd v3 客户端包的实际导入名，再逐声明遍历 AST，仅检查形如 `<导入名>.Config{...}` 的复合字面量（`analyzer.rs:43-100`）。
- 对目标字面量，`run` 检查键值字段中是否存在键 `AutoSyncInterval`；缺失时在字面量起始位置报告 `missing field AutoSyncInterval`（`analyzer.rs:75-93`）。
- `init` 表达 Go 初始化期调用 `util.SkipAnalyzerByConfig` 的接线意图，使分析器在执行前按仓库配置过滤文件（`analyzer.rs:102-105`）。

它不校验字段值、持续时间是否非零、通过变量或构造函数生成的配置，也不修改 etcd 配置。其规则只关心目标复合字面量中是否出现指定键。

## 主要符号

- `pub static Analyzer: analysis::Analyzer`：公开的分析器描述对象。`name` 是配置与诊断框架使用的稳定标识；`requires` 保留 Go 版对 `inspect.Analyzer` 的声明；`run` 指向本文件入口。RustCodeGraph 可定位 `run` 和 `init`，但未把该静态量建立为可查询符号，这也是当前索引/草稿形态的限制。
- `configPackagePath: &str = "go.etcd.io/etcd/client/v3"`：用完整导入路径判断一个文件是否使用目标包。
- `configPackageName: &str = "clientv3"`：无显式别名时采用的默认包名。
- `configStructName: &str = "Config"`：选择器右侧必须匹配的类型名。
- `pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn Any>>, analysis::Error>`：规则主体。成功时总是返回 `Ok(None)`；诊断通过 `pass.Reportf` 旁路写入分析框架。
- `pub fn init()`：表达初始化包装逻辑，调用 `util::SkipAnalyzerByConfig(&Analyzer)`。

这些 Rust 名称保留了 Go 风格的导出名和字段名（例如 `Analyzer`、`Files`、`Reportf`），并非惯用 Rust 命名；这是迁移草稿忠实对应 Go 源的结果。

## 执行流程

1. 分析框架通过 `Analyzer.run` 进入 `run`，传入包含待分析 Go AST 的 `analysis::Pass`。
2. 对 `pass.Files` 中每个文件，调用 `util::GetPackageName(&file.Imports, configPackagePath, configPackageName)`。若未导入目标路径，返回空字符串并跳过整份文件；显式别名则返回别名，否则返回 `clientv3`。Go 实现依据见 `build/linter/util/util.go:GetPackageName`。
3. 对文件中的每个声明调用 `ast::Inspect`，深度优先访问其子节点。访问器始终返回 `true`，因此不因某个不匹配节点而剪枝。
4. 依次要求节点可视为复合字面量、字面量类型可视为选择器表达式、选择器左侧可视为标识符。任一形状不匹配时继续遍历。
5. 比较选择器左侧名称与步骤 2 得到的实际导入名，并比较选择器右侧名称与 `Config`。只有两者都匹配才进入字段检查。
6. 遍历 `lit.Elts`。只有键值表达式且键是标识符时才参与匹配；找到 `AutoSyncInterval` 后设置 `found` 并提前结束字段循环。
7. 未找到该键时，在 `lit.Pos()` 报告固定消息。随后继续访问 AST 中的其他节点，最终完成该文件及整个 pass。
8. 没有运行期错误分支，扫描结束返回 `Ok(None)`。

配置过滤在 Go 侧发生于规则主体之前：`build/linter/util/util.go:SkipAnalyzerByConfig` 用包装函数复制 pass，只保留 `shouldRun` 接受的文件，再调用原 `Run`。`build/nogo_config.json` 当前为该规则排除测试、生成、mock、external 文件及 `pkg/parser/parser.go`。

## 数据与状态

分析器自身只有不可变元数据和三个字符串常量。`run` 的临时状态局限于当前调用：

- `packageName` 是每个 Go 文件独立计算的导入名，支持 `clientv3` 的显式别名。
- `found` 是每个候选复合字面量独立创建的布尔值，不会跨字面量或文件累积。
- `lit`、`tp`、`litPackage`、`selected`、`kv`、`key` 都是当前 AST 节点的借用/视图，不建立缓存。
- 可观察输出只有 `pass.Reportf` 产生的诊断；正常返回值没有分析结果载荷。

`Analyzer` 在 Go 原版中会被 `init` 修改其 `Run` 回调以加入配置过滤。Rust 草稿试图表达相同关系，但本文件没有展示 `analysis`、`ast`、`inspect` 或 `util` 的导入/模块定义，也没有 Cargo 接线，因此不能据此声称存在可运行的 Rust 全局状态。

## 依赖与调用关系

上游关系分为两条：

- Rust 草稿：`Analyzer` 的 `run` 字段是 `run` 的直接入口，`init` 调用 `util::SkipAnalyzerByConfig`；RustCodeGraph 对文件给出 `used by 0 files`，仓库 Rust 搜索未发现模块注册，因此没有已验证的 Rust 运行时调用者。
- Go 生产链：`build/BUILD.bazel` 依赖 `//build/linter/etcdconfig`，其 `BUILD.bazel` 将 `analyzer.go` 编译为公开 Go 库并声明对 `build/linter/util`、`go/analysis` 和 `inspect` 的依赖。nogo 聚合层据此装载分析器。

`run` 的直接下游依赖为：

- `util::GetPackageName`：解析目标导入路径对应的本地名称；Go 定义位于 `build/linter/util/util.go:249-261`。
- `ast::Inspect` 以及节点形状转换：遍历 Go AST 并识别 `CompositeLit`、`SelectorExpr`、`Ident`、`KeyValueExpr`。
- `analysis::Pass::Reportf`：把缺失字段诊断关联到字面量位置。
- `inspect::Analyzer`：在分析器元数据中声明为 prerequisite；Go/Rust 主体仍直接使用 `ast.Inspect`，没有读取 prerequisite 的结果。
- `util::SkipAnalyzerByConfig`：在初始化阶段包装运行函数并根据 `nogo_config.json` 过滤输入文件。

RustCodeGraph 的精确 `node run --file ...` 成功确认了函数定义与上述内部调用形状；泛化 `explore` 因 `Context` 等通用名称产生大量无关候选，精确 `callers run` 查询未在 60 秒内返回，因此调用者结论采用同一索引的文件级 `used by 0 files` 加仓库引用搜索交叉验证，而不虚构函数级调用边。

## 错误处理与边界

规则对大多数非目标语法采用“忽略并继续”：未导入目标包、非复合字面量、非选择器类型、选择器左侧非标识符、包名或类型名不匹配、非键值字段、键非标识符都不会报错。这样避免把其他包的 `Config` 或无关 AST 节点误判为 etcd 配置。

重要边界如下：

- 支持显式导入别名，因为包名来自 `GetPackageName`；未处理点导入后的裸 `Config{}`，因为规则要求选择器表达式。
- 只认 `clientv3.Config` 的直接复合字面量。类型别名、包装构造函数、变量间接初始化以及嵌入其他表达式不会被语义追踪。
- 只认键为标识符且名字恰为 `AutoSyncInterval`；是否赋予合理值不在规则范围内。
- Rust 草稿用 `expect("selector expression must have an identifier")` 访问 `tp.Sel`（`analyzer.rs:67-70`）。注释把它视为 Go AST 的结构不变量；若未来 Rust AST 表示允许缺失选择器，那里会 panic，应改为显式跳过或返回错误。
- 函数签名允许 `analysis::Error`，但当前主体没有构造或传播错误，只返回 `Ok(None)`。诊断不等于函数错误。
- 当前 Rust 文件缺少可验证的 crate 接线且声明“不保证可编译”；不能把文本形状测试当作编译或行为通过的证据。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、文件句柄或网络资源。扫描是同步的嵌套遍历，生命周期受一次 `analysis::Pass` 调用约束；临时变量随文件、字面量和回调访问结束而释放。

若分析框架并行运行多个 package 的 analyzer，隔离责任属于外部 `go/analysis`/nogo 调度器；本规则没有共享可变业务状态。唯一需要注意的是 Go 版 `init` 会原地包装全局 `Analyzer.Run`，该动作预期只在包初始化时执行一次。Rust 草稿的静态对象与 `&Analyzer` 调用尚未通过 crate 编译验证，不能推导其可变性或线程安全保证。

性能主要由 AST 节点数与候选字面量字段数决定：每个文件先线性扫描 imports，随后遍历全部声明子树；每个匹配字面量至多线性扫描其元素，找到字段即提前退出。没有额外持久分配或跨文件缓存。

## 与 Go 版本的对应关系

`build/linter/etcdconfig/analyzer.go` 是当前语义基准。Rust 草稿逐项保留：

- 相同的 analyzer 名称、文档、`inspect.Analyzer` prerequisite 和 `run` 绑定。
- 相同的包路径、默认包名和结构体名常量。
- 相同的逐文件导入过滤、逐声明 `ast.Inspect`、AST 形状判断与名称比较顺序。
- 相同的 `AutoSyncInterval` 键扫描、提前退出、报告位置与消息。
- 相同的最终空结果/无错误语义，以及初始化时按配置跳过文件的意图。

Rust 适配差异主要是用 `Option` 模式匹配代替 Go 类型断言，用 `expect` 显式展开 `SelectorExpr.Sel`，并把 Go 的 `(any, error)` 写成 `Result<Option<Box<dyn Any>>, analysis::Error>`。这些变化由 `build/linter/etcdconfig/analyzer_test.rs` 的文本断言保护，但测试没有构造 AST 或执行 `run`。

Go 侧有真实 Bazel 构建和 nogo 配置；本目录没有 Go 单元测试或 testdata。Rust 独立测试包含三个测试函数，分别检查 analyzer 接线文本、选择器展开文本和导入过滤/字段扫描/诊断文本。它们能发现机械迁移形状漂移，但不能证明 Rust 文件可编译、AST 遍历能执行或诊断位置正确。

## 扩展指南

新增规则能力时应先判断修改是否仍属于“必要 etcd 配置字段”检查：

- 新增必填字段：在 `run` 的字面量字段扫描处维护所需字段集合，并确保一次扫描能分别报告缺失项；同步修改 Go 基准和独立 Rust 测试，避免只改文本草稿。
- 支持其他 etcd 配置类型或包路径：修改三个常量及类型识别分支，并补充别名导入、错误包名、同名非目标类型的回归用例。
- 支持点导入、类型别名或间接构造：需要引入类型信息，而不只是扩展字符串判断；这会改变依赖和误报风险，应先在 Go `analysis.Pass.TypesInfo` 上设计并验证。
- 改变排除范围：修改 `build/nogo_config.json`；若改变 Go 构建接线，还要同步 `build/linter/etcdconfig/BUILD.bazel` 或 `build/BUILD.bazel`。
- 让 Rust 版本真正运行：需要先建立明确的 crate/module 边界和可用的 Go AST/analysis 抽象，再解决静态 `Analyzer` 初始化与 `SkipAnalyzerByConfig` 所需可变性的接口问题。不能仅依靠当前 `include_str!` 测试宣布完成。

测试必须保持在独立的 `build/linter/etcdconfig/analyzer_test.rs`，不要嵌入生产源文件。至少应覆盖：默认导入名、显式别名、有/无字段、其他包的 `Config`、非键值元素、点导入/别名等明确边界；当存在可运行 Rust crate 后，还应把纯文本断言升级或补充为真实 AST 行为测试。兼容风险集中在诊断名称/文本和排除配置（CI 可能依赖）；性能风险集中在新增多次 AST 遍历，应优先维持单次遍历。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter build/linter/etcdconfig` 确认 Go/Rust 源和 Rust 测试；`node --file build/linter/etcdconfig/analyzer.rs` 读取 105 行全貌并报告 `used by 0 files`；`node run --file ...`、`node init --file ...` 精确确认入口源码。函数级 `callers run` 查询超时，未将其作为完成证据。
- Rust 源：`build/linter/etcdconfig/analyzer.rs`，核对 `Analyzer`、三个常量、`run`、`init` 及草稿声明。
- Cargo/模块证据：根 `Cargo.toml` 的 workspace/package 定义，以及仓库内对 `etcdconfig` 的 Rust 引用搜索；未发现本目录 crate、模块入口或运行时调用者。
- Go 对照：`build/linter/etcdconfig/analyzer.go` 与 `build/linter/util/util.go`，核对完整规则、`GetPackageName` 和 `SkipAnalyzerByConfig` 语义。
- 构建/配置：`build/linter/etcdconfig/BUILD.bazel`、`build/BUILD.bazel`、`build/nogo_config.json`，核对 Go 库依赖、nogo 聚合接线和排除规则。
- 测试：`build/linter/etcdconfig/analyzer_test.rs`；目录与仓库搜索未发现该 analyzer 的 Go 测试/testdata。测试只校验源码文本，不等同于编译或行为测试。
- 本任务是纯文档分析，按计划未运行 Cargo，也未修改 Rust、Go、Cargo、Bazel 配置或只读的 `plan.md`。交付前以任务指定命令验证目标文档恰含十一个固定二级章节，并人工复核上述结论均可回指到符号或文件。
