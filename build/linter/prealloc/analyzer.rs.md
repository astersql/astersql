# `build/linter/prealloc/analyzer.rs`

## 文件定位

本文件是同目录 [`analyzer.go`](analyzer.go) 的 Rust 迁移稿，描述名为 `prealloc` 的 Go 静态分析器：它检查 Go AST 中可提前分配容量的切片声明，并把上游 `prealloc::Check` 返回的建议转换为 `analysis::Pass` 诊断。目标不是优化数据库运行时，而是在仓库构建期帮助开发者减少可预见的切片扩容。

当前生产接线仍是 Go/Bazel 路径。`build/linter/prealloc/BUILD.bazel` 只编译 `analyzer.go`，其目标被 `build/BUILD.bazel` 的 `nogo(name = "tidb_nogo")` 依赖；根 `Cargo.toml` 没有 `build/linter/prealloc` workspace member，该目录也没有独立 `Cargo.toml`。Rust 侧仅由 `pkg/lib.rs` 在 `#[cfg(test)]` 下注册 [`analyzer_test.rs`](analyzer_test.rs)，后者用测试桩 `include!` 本文件来验证局部流程。因此，本文区分源码表达的 analyzer 语义、测试验证范围与当前真实生产调用链，不把该 Rust 文件描述为已接入 `nogo`。

## 核心职责

1. `Settings` 保存传给预分配检查器的三个布尔开关。
2. `Name` 与 `Analyzer` 声明分析器身份、说明文本、空依赖集合和运行入口。
3. `run` 使用固定默认配置逐个检查 `pass.Files`，把每条 hint 格式化并通过 `pass.Reportf` 上报。
4. `init` 表达 Go 包初始化时的两层包装：先按配置排除文件，再接入 `lint:ignore`/`nolint` 通用跳过逻辑。

本文件不实现“哪些切片值得预分配”的算法；候选发现由外部 `prealloc::Check` 负责。它也不修改源文件，只发出建议诊断。

## 主要符号

- `pub struct Settings { Simple, RangeLoops, ForLoops }`：分析策略配置。三个字段均为 `bool`；字段名保留 Go 风格。源码注释为 `RangeLoops` 和 `ForLoops` 都记录了 `mapstructure:"range-loops"`，这忠实反映 Go 原文件的重复 tag，而不能推断为两个不同配置键。
- `pub const Name: &str = "prealloc"`：诊断前缀和 analyzer 名称的单一常量来源。
- `pub static Analyzer: analysis::Analyzer`：公开 analyzer 描述值，`name` 为 `Name`，`doc` 为切片预分配说明，`requires` 是空切片，`run` 指向本文件函数。空 `requires` 表示核心检查不消费其他 analyzer 的结果；Go 侧 `init` 随后会通过通用包装追加依赖或替换回调。
- `pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn Any>>, analysis::Error>`：公开分析回调。成功固定返回 `Ok(None)`，不向后续分析器提供动态结果。
- `pub fn init()`：公开的初始化对应函数，依次调用 `util::SkipAnalyzerByConfig(&Analyzer)` 和 `util::SkipAnalyzer(&Analyzer)`。

文件没有 trait、`impl`、枚举、宏或条件编译项。RustCodeGraph 在本文件中索引到 `Settings`、`Name`、`run`、`init` 四个顶层符号；`Analyzer` 静态值可从完整文件源码确认，但未被图查询单独列为节点。

## 执行流程

`run` 的流程如下：

1. 每次调用都创建 `Settings { Simple: true, RangeLoops: true, ForLoops: false }`。这意味着启用简单声明与 range-loop 检查，关闭传统 for-loop 检查，且当前没有从外部配置覆盖这些值的路径。
2. 以索引顺序遍历 `pass.Files`。每轮先借用原始 AST 文件，再以 `std::slice::from_ref(f)` 构造长度为 1 的只读切片传给 `prealloc::Check`；不会复制 AST，也不会把整个 package 的所有文件一次交给检查器。
3. `prealloc::Check` 返回零条或多条 hint。函数按返回顺序遍历，每条使用 `hint.Pos` 作为位置。
4. `hint.DeclaredSliceName` 先经 `util::FormatCode` 处理，再生成 `[prealloc] Consider preallocating <name>`，并由 `pass.Reportf` 上报。
5. 所有文件和 hint 处理完后返回 `Ok(None)`。

`init` 表达的顺序同样重要：`SkipAnalyzerByConfig` 先根据仓库排除配置筛选文件，`SkipAnalyzer` 再处理源码中的 lint 指令。当前 Rust 生产模块没有接线，实际构建中执行的是 Go 版包初始化；独立 Rust 测试只断言这两个调用按源码存在，并未执行包装行为。

## 数据与状态

长期静态数据是 `Name` 和 `Analyzer`。`Settings` 不是全局配置：`run` 每次新建一份固定值，生命周期只覆盖一次分析调用。每个文件的 `hints` 也只在该轮循环内存在；每条 hint 被立即转换为诊断，没有跨文件缓存或汇总。

关键不变量包括：传给 `prealloc::Check` 的文件切片长度恒为 1；切片引用指向 `pass.Files` 中的原始元素；三个配置值恒为 `(true, true, false)`；诊断名称恒取 `Name`；成功结果恒为 `None`。`analyzer_test.rs` 通过记录传入文件地址和值验证了前两项，并验证两文件时保持检查与报告顺序。

`run` 会在处理期间可变借用 `Pass` 以记录诊断。源码用局部块限制对 `pass.Files[index]` 的不可变借用，使该借用在调用 `pass.Reportf` 前结束，避免同时持有字段借用与整个 `Pass` 的可变借用。

## 依赖与调用关系

下游依赖由源码和构建声明共同确认：

- `analysis::{Analyzer, Pass, Error}` 提供 analyzer 元数据、输入文件集合、返回错误类型和诊断接口。
- `prealloc::Check` 执行真正的 AST 候选发现，返回含 `Pos`、`DeclaredSliceName` 的 hint。Go 生产依赖在 `go.mod` 中固定为 `github.com/golangci/prealloc`，并由 `build/linter/prealloc/BUILD.bazel` 的 `@com_github_golangci_prealloc//:prealloc` 声明。
- `util::FormatCode` 给代码片段加反引号（已有反引号时保持原值）；`SkipAnalyzerByConfig` 与 `SkipAnalyzer` 负责配置排除和 lint 指令兼容。
- `std::slice::from_ref` 在不分配、不复制 AST 的情况下构造单元素切片；`std::any::Any` 只用于保持与 Go `any` 结果槽相似的接口，本函数没有实际结果载荷。

RustCodeGraph 对 `run`、`init` 的 callers 查询没有返回上游，对精确符号的 callees 查询也没有生成有效边；这是图对当前未接线、通过 `include!` 测试的文件解析能力边界，不能据此否认函数体内的直接调用。仓库搜索确认唯一 Rust 接线是 `pkg/lib.rs` → `analyzer_test.rs` → `include!("analyzer.rs")`；Go 生产链则是 `build/BUILD.bazel:tidb_nogo` → `build/linter/prealloc:prealloc` → `analyzer.go`。

## 错误处理与边界

`run` 的签名允许返回 `analysis::Error`，但函数体没有显式构造错误，也没有对 `prealloc::Check` 或 `Reportf` 使用 `?`；在当前接口形状下，正常完成只返回 `Ok(None)`。测试桩将 `prealloc::Check` 模拟为直接返回 `Vec<Hint>`，从而验证零显式错误传播的设计。

若检查器或报告路径发生 panic，本文件不捕获，panic 会中断当前和后续文件处理；此前已经写入 `Pass` 的诊断不会回滚。独立测试配置第二次检查 panic，观察到第一个文件的报告仍保留、第二个和后续处理停止。该证据验证的是局部 panic/副作用顺序，不代表生产上游库承诺用 panic 报错。

空 `pass.Files` 是合法边界：循环不执行并返回 `Ok(None)`。某文件没有 hint 时不会报告。文件解析错误、类型错误、生成文件过滤和 `nil` AST 等边界不在本函数处理范围内，应由 analysis 驱动器、上游检查器或包装层负责。`FormatCode` 决定名称转义；本文件不自行清洗 hint 文本。

## 并发与资源生命周期

`run` 是同步、串行、按文件顺序执行的；不创建线程、异步任务、锁、通道、事务或文件句柄。局部 `Settings`、单元素借用切片和 hints 在相应作用域结束时释放。算法在本文件可见层面的工作量与文件数及返回 hint 总数线性相关；真正 AST 扫描成本由 `prealloc::Check` 决定。

本文件自身没有可变全局状态，但 `Analyzer` 初始化包装会改变 analyzer 回调/依赖的语义。Go 包的 `init` 天然在分析运行前完成；未来若正式接入 Rust，必须确认 `analysis::Analyzer` 的可变性和一次初始化模型。仓库现有 Rust `util.rs` 的包装函数签名接收 `&mut analysis::Analyzer`，而本文件 `init` 传入不可变静态引用 `&Analyzer`；当前测试用只读桩规避了这一差异，因此不能把测试通过等同于与真实 Rust util 已完成类型级集成。

## 与 Go 版本的对应关系

同目录 `analyzer.go` 是直接对照来源：

- Go `Settings`、`Name`、`Analyzer` 分别对应同名 Rust 结构体、常量和静态值；名称、doc、空初始依赖及 `run` 入口一致。
- Go `run` 创建 `&Settings{true, true, false}`；Rust 创建值类型 `Settings`，传参结果一致。
- Go 对 `pass.Files` 使用 range，Rust 使用索引循环；两者都把单个原始文件包装成长度 1 的切片调用 `Check`，并保持输入顺序。
- Go `pass.Reportf(hint.Pos, "[%s] Consider preallocating %s", ...)` 对应 Rust `format!` 后调用 `Reportf`，预期消息一致。
- Go 返回 `nil, nil` 对应 Rust `Ok(None)`。
- Go 包级 `init` 会自动执行并可就地修改 `*analysis.Analyzer`；Rust 的普通 `init` 函数不会自动运行，且当前静态值/真实 util 的可变性尚未接线解决。

Rust 独立测试包含四个测试层面：编译桩中的行为测试验证逐文件调用、默认值、地址、诊断和 panic 后的部分副作用；三个外层测试分别检查 analyzer/初始化结构、借用原 AST 与诊断文本、版权及无占位声明。它没有调用真实 `github.com/golangci/prealloc`，也没有验证 Bazel `nogo` 的生产执行。目录中没有直接的 Go `*_test.go`。

## 扩展指南

- 调整检测策略：修改 `run` 中三个默认值，并同步更新 `analyzer_test.rs` 的调用记录断言；若要开放配置，先确定 Go `Settings` 的 tag 兼容性，尤其不要默默“修正”当前两个字段共用 `range-loops` tag 的既有事实。
- 调整诊断：修改 `Name`、消息模板或 `FormatCode` 调用时，应同步验证消费者可见文本、位置和反引号行为，并保持 Go/Rust 对照明确。
- 改为整包检查：当前每次只传一个文件。若合并为一次传入所有 `pass.Files`，可能改变上游算法的候选推断、顺序、复杂度和去重行为，需要真实上游库测试，不能只改循环写法。
- 正式接入 Rust：需要建立模块或 crate 边界、提供真实 `analysis`/`prealloc`/`util` 依赖、定义初始化调用时机，并解决不可变 `Analyzer` 与 util 包装所需可变引用的冲突。届时应把测试桩升级为独立可编译的集成/单元测试，测试仍放在 `analyzer_test.rs`，不要内嵌进生产文件。
- 扩展错误策略或并行执行：需定义 hint 顺序、部分诊断保留、panic 隔离和上游检查器线程安全契约。并行化前应先测量真实 AST 扫描成本，避免破坏确定性。

主要风险是：正确性上漏报/误报或改变逐文件语义；兼容性上诊断文本、配置键、跳过指令和初始化顺序漂移；性能上对每个文件重复进入检查器以及未来并行化带来的调度开销。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，其中 Rust 7,032 个；`files --filter build/linter/prealloc` 找到 `analyzer.go`、`analyzer.rs`、`analyzer_test.rs`。
- RustCodeGraph `node --file build/linter/prealloc/analyzer.rs --offset 1 --limit 400` 与精确 `node` 查询：核对完整 81 行源码以及 `Settings`、`run`、`init` 的定义；`query prealloc` 额外核对 Rust/Go 的 `init` 和 Go `run` 候选。
- RustCodeGraph 对 `build/linter/prealloc/analyzer.rs::run`、`::init` 的 callers/callees 查询：未得到可靠调用边；本文因此用源码和模块/构建搜索补齐直接证据，并明确图验证限制。
- [`analyzer.rs`](analyzer.rs)、[`analyzer.go`](analyzer.go)：逐项核对设置默认值、单文件调用粒度、hint 顺序、诊断格式、空结果和初始化顺序。
- [`analyzer_test.rs`](analyzer_test.rs)、`pkg/lib.rs`：核对 Rust 测试的注册、`include!` 编译桩、行为断言、panic 边界及文本结构断言。
- `Cargo.toml`、[`BUILD.bazel`](BUILD.bazel)、`build/BUILD.bazel`、`go.mod`：核对 Rust crate 边界、Go library 依赖、`nogo` 生产接线与上游 Go module 版本；确认目录没有独立 Cargo manifest。
- `build/linter/util/util.go`、`build/linter/util/util.rs`：核对 `FormatCode`、两种 skip 包装的职责，以及 Rust 真实 util 所需可变 analyzer 与本文件当前静态引用之间的集成缺口。
- 按任务约束未运行 Cargo。交付时运行任务指定的 11 章节结构命令，并人工复核本文能回答文件存在目的、执行过程、依赖/边界、真实接线状态和安全扩展位置。
