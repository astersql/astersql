# `build/linter/copyloopvar/analyzer.rs`

## 文件定位

该文件位于构建辅助目录 `build/linter/copyloopvar/`，是同目录 Go analyzer 注册文件 `analyzer.go` 的机械迁移草稿。它试图在 Rust 中保留两个接线点：导出的 analyzer 单例 `Analyzer` 和配置包装入口 `init()`。它不是 SQL 请求、规划或执行链的一部分；对应的 Go 包由 `build/BUILD.bazel` 的 `tidb_nogo` 目标在构建期静态分析链中加载。

当前 Rust 文件尚未进入生产模块树：根 crate 的 `pkg/lib.rs` 只在 `#[cfg(test)]` 下通过 `build/linter/copyloopvar/analyzer_test.rs` 注册文本测试，而没有用 `mod` 或 `#[path]` 引入 `analyzer.rs`。根 `Cargo.toml` 也没有声明本文件写到的 `once_cell`、`analysis` 或 Rust 版 `copyloopvar` 依赖。因此应把它视为“记录预期注册形状、当前不保证编译”的迁移草稿，而不是已经可运行的 Rust linter。

## 核心职责

1. `Analyzer` 表达“从上游 copyloopvar 工厂取得一个 analyzer，并在进程内共享”的意图，对应 Go 的 `var Analyzer = copyloopvar.NewAnalyzer()`。
2. `init()` 表达“在 analyzer 交给分析驱动前，用仓库统一配置包装其执行入口”的意图，对应 Go 的 `util.SkipAnalyzerByConfig(Analyzer)`。
3. 本文件不实现 loop-variable 检查算法，不读取 `build/nogo_config.json`，也不直接遍历或修改源码；真正的 Go 检查逻辑来自 `go.mod` 锁定的 `github.com/karamaru-alpha/copyloopvar v1.2.2`。

## 主要符号

- `pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer>`（第 28 行）：公开的惰性全局值。闭包 `|| copyloopvar::NewAnalyzer()` 表示第一次解引用时构造 analyzer，后续访问复用同一实例。名称使用 Go 风格大写；文件级注释和根 crate 的 lint 放宽说明这是迁移期命名。
- `copyloopvar::NewAnalyzer()`（第 29 行）：预期的外部工厂调用。仓库当前没有对应 Rust crate/API 声明；这里只保留 Go 工厂的名称和构造关系。
- `pub fn init()`（第 33 行）：公开普通函数。Rust 没有 Go 包级 `init` 的自动执行机制，所以除非未来模块入口显式调用，它不会自行运行。
- `util::SkipAnalyzerByConfig(&Analyzer)`（第 36 行）：预期把共享 analyzer 交给统一配置包装器。现有 `build/linter/util/util.rs` 的同名函数需要 `&mut analysis::Analyzer`，而本文件提供的是惰性静态的共享引用形状；这是尚未完成接线时必须解决的可变性/API 差异。

文件没有模块级常量、结构体、枚举、trait、`impl`、条件编译项或本地错误类型。

## 执行流程

按文件表达的预期，流程如下：

1. 某个未来的 Rust 注册器首次访问 `Analyzer`。
2. `once_cell::sync::Lazy` 执行一次初始化闭包，调用预期的 `copyloopvar::NewAnalyzer()`，保存返回的 `analysis::Analyzer`。
3. 注册阶段显式调用 `init()`；该函数把 analyzer 交给 `util::SkipAnalyzerByConfig`，意图包装其 `Run` 回调。
4. 真正运行 analyzer 时，配置包装器应先按 analyzer 名称和文件路径筛掉 `exclude_files`，再调用原始检查回调。

第 3、4 步是从 Go 对照与 `build/linter/util/{util.go,util.rs}` 可验证的目标语义，不是当前 Rust 应用已经发生的行为：RustCodeGraph 对 `init` 的 `callers` 和 `callees` 查询均为空，仓库 Rust 模块树也没有引入该源文件。

Go 生产链则是完整的：`build/linter/copyloopvar/BUILD.bazel` 把 `analyzer.go`、`//build/linter/util` 与外部 copyloopvar 库组成 Go 库，`build/BUILD.bazel` 再把 `//build/linter/copyloopvar` 放入 `tidb_nogo.deps`。`build/nogo_config.json` 的 `copyloopvar.exclude_files` 为解析器生成文件、`external/` 和通用生成文件提供过滤规则。

## 数据与状态

- 唯一长期状态是 `Analyzer`。`Lazy` 设计成全局单例并保证一次初始化；文件本身没有集合、缓存、锁或显式生命周期参数。
- analyzer 的业务状态来自上游构造器。Go v1.2.2 工厂设置名称 `copyloopvar`、说明、`inspect.Analyzer` 依赖、运行回调以及 `check-alias` 布尔 flag；本 Rust 文件没有复制这些字段，只假设工厂返回完整对象。
- `init()` 的预期副作用是替换/包装 analyzer 的运行回调。Go 的 `SkipAnalyzerByConfig` 保存旧 `Run`，对每次 analysis pass 复制一份 pass、过滤 `Files`，然后调用旧回调；它不会修改被分析源码。
- 当前 Rust 草稿把全局值暴露为共享静态，但现有 Rust util 包装器需要可变 analyzer。这意味着真正接线前必须确定初始化期间的独占可变访问方案，不能把当前签名当作已解决的不变量。

## 依赖与调用关系

上游与下游关系可分成实际 Go 链和待接线 Rust 链：

- Go 上游注册器：`build/BUILD.bazel:tidb_nogo` 依赖 `//build/linter/copyloopvar`。
- Go 包装入口：`build/linter/copyloopvar/analyzer.go:init` 调用 `build/linter/util/util.go:SkipAnalyzerByConfig`。
- Go 下游实现：`github.com/karamaru-alpha/copyloopvar v1.2.2` 的 `NewAnalyzer` 构造 analyzer；其运行逻辑依赖 `golang.org/x/tools/go/analysis/passes/inspect` 遍历 `RangeStmt` 和 `ForStmt`。
- 配置：`build/nogo_config.json` 以 analyzer 名称 `copyloopvar` 提供排除文件正则；Go util 的 `shouldRun`/`SkipAnalyzerByConfig` 消费该配置。
- Rust 预期下游：`once_cell::sync::Lazy`、`analysis::Analyzer`、`copyloopvar::NewAnalyzer` 和 `util::SkipAnalyzerByConfig`。根 `Cargo.toml` 未给前三者建立本文件可用的依赖接线。
- Rust 实际上游：没有生产调用者。`pkg/lib.rs` 只注册 `analyzer_test.rs`，测试通过 `include_str!("analyzer.rs")` 读取文本，并不编译 `Analyzer` 或 `init()`。

RustCodeGraph 将 `analyzer.rs` 识别为含两个符号的文件，其中可导航函数为 `build/linter/copyloopvar/analyzer.rs::init`；该函数的 caller/callee 图均为空。静态量内的外部工厂调用和函数体内未解析的外部调用没有形成图边，故依赖结论由源文件、Cargo、模块入口和 Go/Bazel 文件补证。

## 错误处理与边界

- `init()` 没有返回值或显式错误通道；按目标语义，配置包装与 analyzer 构造失败只能由依赖 API 的 panic/初始化失败策略处理，但当前草稿没有定义这些 API，因此不能声称具备具体错误行为。
- 本文件不检查空文件集、语法错误或诊断上报失败。Go 上游 analyzer 的 `run` 正常返回 `(nil, nil)`，通过 analysis framework 报告诊断；Rust 草稿尚未实现这一层。
- Go 配置包装只过滤 `analysis.Pass.Files`，不会屏蔽包本身，也不会吞掉上游 `Run` 返回的错误。过滤后即使文件集为空，仍会调用旧 `Run`。
- `build/nogo_config.json` 当前排除 `pkg/parser/parser.go`、`external/`、`.*_generated\.go$`；新增排除规则应保持 analyzer 名称与配置键一致。
- 上游 Go v1.2.2 只检查循环体顶层的定义赋值，并根据 `check-alias` 决定是否把别名赋值也报告；安全自动修复只针对单一、同名的简单复制。上述是依赖版本的行为边界，不是本 Rust 文件自身实现。

## 并发与资源生命周期

`once_cell::sync::Lazy` 的意图是线程安全的一次初始化与进程级存活：初始化成功后 analyzer 与进程同寿命，没有显式销毁。文件不创建线程、异步任务、通道、文件句柄或网络资源。

需要注意，线程安全初始化不等于 analyzer 可并发变更。`init()` 期望对 analyzer 的 `Run` 进行一次包装，而现有 Rust util API 要求 `&mut analysis::Analyzer`；若未来在 analyzer 已被并发读取后再修改，会产生设计冲突。安全接线应把包装限定在单线程/独占的注册阶段，完成后只共享不可变 analyzer，或改用能明确表达一次性初始化后冻结状态的容器。

## 与 Go 版本的对应关系

| Go `build/linter/copyloopvar/analyzer.go` | Rust 草稿 | 当前差异 |
| --- | --- | --- |
| `var Analyzer = copyloopvar.NewAnalyzer()` | `Lazy<analysis::Analyzer>` 调用同名工厂 | Go 在包初始化时构造并得到指针；Rust 表达首次访问时构造，但外部 Rust 工厂尚不存在/未声明。 |
| 包级 `init()` 自动执行 | `pub fn init()` 普通函数 | Rust 必须由模块注册器显式调用；当前没有调用者。 |
| `SkipAnalyzerByConfig(Analyzer)` 接收可变对象指针 | `SkipAnalyzerByConfig(&Analyzer)` 传共享惰性静态 | 与现有 Rust util 的 `&mut analysis::Analyzer` 签名不匹配，不能直接编译。 |
| Bazel `nogo` 依赖实际加载 Go 包 | 无 Rust 生产模块接线 | Rust 仅有源码文本测试，不参与 linter 运行。 |

保持一致的意图是：复用上游 analyzer 单例，并且只应用配置化文件过滤；与 `mirror`、`makezero` 等包装不同，Go 此处没有额外调用 `util.SkipAnalyzer`，因此不应无依据添加 `lint:ignore`/`nolint` 指令处理。

## 扩展指南

- 若要真正接入 Rust，先在明确的 crate/module 中声明 `analysis`、copyloopvar 实现和 `once_cell`（或等价一次性初始化机制），再从注册入口引入本文件并显式调用初始化逻辑。不要仅让文本测试通过就宣称接线完成。
- 优先解决所有权：让工厂先返回局部可变 analyzer，调用 `SkipAnalyzerByConfig(&mut analyzer)` 后再放入只读全局；或调整统一注册 API。不要用不安全共享可变性绕过现有签名。
- 若移植检查算法，应逐项对齐锁定的 Go v1.2.2：`inspect` 依赖、range/for 两类循环、仅定义赋值、`check-alias` flag、诊断文案与简单赋值修复范围。算法及测试应放在独立 Rust 源文件/测试文件中，不要把测试内嵌进 `analyzer.rs`。
- 配置变化应同步检查 `build/nogo_config.json` 与 Rust 配置读取路径，保证使用 analyzer 名称 `copyloopvar`。若 Rust 与 Go 同时存在，需避免在同一构建链重复报告相同诊断。
- 更新注册形状时同步修改 `build/linter/copyloopvar/analyzer_test.rs`；若增加可执行 Rust 行为，应另加独立行为测试，覆盖首次初始化、重复访问、配置过滤、空文件集、上游错误传播及并发只读访问。
- 兼容风险主要是 Go 版本语义（尤其 Go 1.22 后循环变量行为）、配置键和诊断/修复一致性；性能风险主要来自重复 AST 遍历或每次 pass 重复编译过滤正则。当前薄包装本身没有可量化运行时性能，因为尚未接线。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter build/linter/copyloopvar` 找到 Go/Rust 源与 Rust 测试；`node --file build/linter/copyloopvar/analyzer.rs` 读取 37 行全貌；`query CopyLoopVar` 定位 `init`；`node`、`callers`、`callees` 核对 `init`，两类调用边结果均为空。
- Rust 源与测试：`build/linter/copyloopvar/analyzer.rs`、`build/linter/copyloopvar/analyzer_test.rs`、`build/linter/util/util.rs`、`pkg/lib.rs`。
- Go/Bazel/配置：`build/linter/copyloopvar/analyzer.go`、`build/linter/copyloopvar/BUILD.bazel`、`build/BUILD.bazel`、`build/linter/util/util.go`、`build/nogo_config.json`、`go.mod`、`go.sum`。
- Cargo：根 `Cargo.toml` 定义 workspace 与根包，但没有把目标源文件设为模块，也没有声明其预期的 Rust copyloopvar/analysis/once_cell 依赖；目标目录没有更近的 `Cargo.toml` 或 `doc.go`。
- 上游直接证据：本地 Go module cache 中 `github.com/karamaru-alpha/copyloopvar@v1.2.2/copyloopvar.go`，版本由 `go.mod`/`go.sum` 锁定，用于核对工厂字段、遍历节点、诊断和修复边界。
- 独立测试只断言源文本包含 `Lazy` 工厂、按引用调用配置包装器，且排除直接静态工厂初始化；它没有编译或执行目标文件。人工复核确认本文明确回答了文件为何存在、预期流程、当前未接线状态和安全扩展路径。
