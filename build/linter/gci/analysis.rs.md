# `build/linter/gci/analysis.rs`

## 文件定位

`build/linter/gci/analysis.rs` 是 Go linter `build/linter/gci/analysis.go` 的 Rust 机械迁移草稿，目标是描述一个名为 `gci` 的 `go/analysis` analyzer：它检查 Go 源文件的 import 分组和排序是否符合 gci 的确定性格式。文件头第 15—17 行已经明确声明“当前不保证可编译”，因此它不是 AsterSQL SQL 请求、事务或存储运行时的一部分，也不能视为已接入的 Rust lint 工具。

当前真正投入构建的是 Go 包：`build/linter/gci/BUILD.bazel` 只把 `analysis.go` 列为 `srcs`，而 `build/BUILD.bazel` 的 `tidb_nogo` 目标通过 `//build/linter/gci` 注册该 analyzer。Rust 根包 `astersql` 的 `[lib]` 指向 `pkg/lib.rs`；该文件只在 `#[cfg(test)]` 下引用 `build/linter/gci/analysis_test.rs`，没有声明或编译 `analysis.rs` 模块。也就是说，Rust 侧目前只有“源码文本契约测试”，没有生产接线。

## 核心职责

该草稿围绕三个职责组织：

1. `Analyzer` 描述 analyzer 的名称、说明文字、依赖集合和执行回调，语义对应 Go 的 `analysis.Analyzer` 字面量。
2. `run` 从 `analysis::Pass` 收集本轮所有 Go AST 文件的真实文件名，以固定配置调用 gci 计算格式化差异，再把每个非空 diff 作为 diagnostic 上报。
3. `init` 希望用 `util::SkipAnalyzerByConfig` 包装 analyzer，使 `build/nogo_config.json` 中 `gci.exclude_files` 命中的文件在进入核心检查前被过滤。

这里的目标是“报告差异”，不是原地重写文件：`run` 仅调用 `DiffFormattedFilesToArray` 并执行 `pass.Report`，没有写文件 API。由于 Rust 文件没有生产模块入口，这些职责目前只是对 Go 行为的结构化保留；生效行为仍以 `analysis.go` 为准。

## 主要符号

- `pub static Analyzer: analysis::Analyzer`（第 29—34 行）：公开静态 analyzer 描述，`name` 为 `gci`，`doc` 说明 import 顺序应始终确定，`requires` 为空，`run` 指向本文件回调。RustCodeGraph 将它与 `run`、`init` 一起识别为本文件的 3 个符号，但文件节点显示 `used by 0 files`，与未接线事实一致。
- `pub fn run(pass: &mut analysis::Pass) -> anyhow::Result<Option<Box<dyn Any>>>`（第 38—80 行）：公开执行入口。成功时返回 `Ok(None)`，用于对应 Go 的 `(nil, nil)`；gci 差异计算错误通过 `?` 向调用者传播。
- `pub fn init()`（第 83—86 行）：公开初始化函数，意图调用 `util::SkipAnalyzerByConfig(&Analyzer)`。需要注意，相邻真实 Rust 工具实现 `build/linter/util/util.rs` 的签名是 `SkipAnalyzerByConfig(analyzer: &mut analysis::Analyzer)`，而本文件传入的是对不可变 `static` 的共享引用；这是草稿尚不可编译、尚不能实际安装包装器的直接证据，不应把 `init` 描述成已经完成注册。

文件没有自定义 struct、enum、trait、常量、`impl` 或条件编译项；`Analyzer` 是唯一模块级静态值，`fileNames`、`rawCfg`、`cfg`、`diffs`、`lock` 均为 `run` 内局部状态。

## 执行流程

按 `run` 的源码顺序，预期流程如下：

1. 以 `pass.Files.len()` 预分配 `fileNames`，遍历 `pass.Files`；对每个 AST 文件调用 `pass.Fset.PositionFor(f.Pos(), false)`，提取 `Filename`。这里依赖文件集把 AST 起始位置映射回磁盘路径。
2. 构造 `config::YamlConfig`。显式布尔值为：保留行内/前缀注释、关闭 debug、跳过生成文件、不跳过 vendor、不启用自定义顺序、保留字典序；其余字段通过 `Default::default()` 取得 Go 零值对应值。
3. 调用 `rawCfg.Parse()` 得到 gci 配置。草稿使用 `expect("default gci config must parse")`，将固定默认配置解析失败视为不可恢复的迁移契约破坏。
4. 创建空 `Vec<String>` 作为差异集合和一个 `Mutex<()>`，调用 `gci::DiffFormattedFilesToArray(fileNames, cfg, &mut diffs, &lock)`。错误通过 `?` 原样返回，此后不再报告 diagnostic。
5. 顺序消费 `diffs`。空字符串被跳过；每个非空字符串被包装为 `analysis::Diagnostic { Pos: 1, Message: format!("\n{}", diff) }` 并交给 `pass.Report`。
6. 所有差异处理完成后返回 `Ok(None)`。

若未来真正接线，初始化阶段还应先让 `SkipAnalyzerByConfig` 保存原回调、基于 analyzer 名称和文件名过滤 `Pass.Files`，再调用上述 `run`。这一包装语义可由 `build/linter/util/util.go` 和 `util.rs` 交叉核对，但本文件当前的静态可变性问题尚未解决。

## 数据与状态

核心输入是可变的 `analysis::Pass`：`Files` 提供待检查 AST，`Fset` 提供位置到文件名的映射，`Report` 接收诊断。`run` 不缓存跨调用状态；所有集合和配置都在单次调用中创建并在返回时释放。

`fileNames` 只保存路径字符串，不保存 AST；容量等于输入文件数，避免增长过程中的常见重复分配。`rawCfg`/`cfg` 是一次性配置对象。`diffs` 是 gci 填充的输出集合，随后按容器顺序消费。每条诊断固定使用 `Pos: 1`，因此定位信息不是某个 import 节点的精确位置，主要有效载荷在带前导换行的统一 diff 文本中。

配置还有两层过滤：gci 内部的 `SkipGenerated: true` 跳过生成文件；Go 生产接线中的 `SkipAnalyzerByConfig` 则读取 `build/nogo_config.json` 的 `gci.exclude_files`，过滤 third-party、生成代码、cgo、protobuf、failpoint binding、parser 生成文件等路径。两者作用位置不同，扩展时不应混为一个条件。

## 依赖与调用关系

下游依赖可从源码和 Go 构建声明直接确认：

- `analysis`：提供 `Analyzer`、`Pass` 和 `Diagnostic` 抽象；Go 对应依赖为 `golang.org/x/tools/go/analysis`。
- `config` 与 `gci`：分别解析 gci 配置和计算格式化 diff。Go 模块在 `go.mod` 固定为 `github.com/daixiang0/gci v0.13.7`，`build/linter/gci/BUILD.bazel` 也声明了两个相应 Bazel 依赖。
- `util::SkipAnalyzerByConfig`：按 analyzer 名称和文件路径应用仓库级排除配置，实际语义位于 `build/linter/util/util.go`；`util.rs` 是其 Rust 迁移实现。
- `std::sync::Mutex`：作为 gci 收集多文件 diff 时的同步参数；本文件只创建并借用它。
- `anyhow` 与 `std::any::Any`：表达 Rust 草稿对 Go `(any, error)` 返回形状的映射。

上游方面，Go `Analyzer` 通过 `build/linter/gci/BUILD.bazel` 成为 `//build/linter/gci`，再由 `build/BUILD.bazel` 的 `tidb_nogo.deps` 纳入仓库 nogo 检查。RustCodeGraph 对 Rust 文件报告 `used by 0 files`，并未解析出 `run`/`init` 的可靠跨文件调用边；源码搜索也只发现 `pkg/lib.rs` 接入其测试文件。因此 Rust `Analyzer`、`run`、`init` 当前均没有真实调用者。

## 错误处理与边界

- 空的 `pass.Files` 合法：会向 gci 传入空文件列表，若下游成功则无诊断并返回 `Ok(None)`。
- 无法从文件读取/解析/格式化等 gci 错误由 `DiffFormattedFilesToArray(...)?` 直接传播；一旦出错，当前调用不会继续遍历和报告已有 `diffs`。
- 空 diff 明确忽略，防止产生无内容诊断；非空 diff 原样保留，仅增加一个前导换行。
- 固定配置解析使用 `expect` 而不是返回错误。这对应 Go 代码忽略 `rawCfg.Parse()` 的 error 后解引用 `*cfg` 的强假设，但 Rust 表现为带明确消息的 panic。若配置结构或上游版本变化，应优先把该不变量写入独立测试并决定是否改为可恢复错误，而不是静默吞掉失败。
- 路径过滤并不在 `run` 内完成；它依赖成功安装 `SkipAnalyzerByConfig` 包装器。当前 Rust `init` 与工具函数可变引用签名不匹配，所以不能声称 Rust 草稿已经获得排除配置保护。
- `Pos: 1` 是沿袭 Go 的粗粒度定位。若消费端要求精确源位置，需要重新确认 nogo/analysis 的兼容性，不能只替换常量而忽略 diff 可能覆盖多个 import 行。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道或长期后台资源。`run` 的所有权边界清晰：文件名和配置按值传给 gci，`diffs` 与 `lock` 在栈帧内创建，以可变/共享借用传入，调用返回后再消费 `diffs`，函数结束时统一释放。

`Mutex<()>` 自身不保护 Rust 侧可见的数据字段；它是为了复刻 Go API 中 `&diffs` 与 `&sync.Mutex` 的协作协议，让 gci 在可能并行处理多个文件时串行化对共享 diff 集合的更新。调用者必须保持 mutex 与 `diffs` 在整个 `DiffFormattedFilesToArray` 调用期间存活，且不能在下游仍使用它们时提前消费。当前同步调用满足这一生命周期。

`Analyzer` 被声明为全局 `static`，但配置包装器需要修改其 `Run`。这构成当前迁移设计的资源/并发边界：若未来使用全局可变状态完成接线，必须选择一次性初始化或显式同步方案，避免重复 `init` 套娃包装以及并发读写回调；不能通过无保护的可变静态量绕过 Rust 安全模型。

## 与 Go 版本的对应关系

Rust 草稿逐段对应 `build/linter/gci/analysis.go`：`Analyzer` 的名称、文档和 `Run` 相同；文件名均由 `Pass.Files` 经 `Fset.PositionFor(..., false)` 得到；配置保持 Go 显式字段并为 Rust 结构要求的其他布尔字段补出 Go 零值；两边都调用 `DiffFormattedFilesToArray`、跳过空 diff、以位置 1 和前导换行上报诊断，最后返回空结果。

差异和迁移状态同样重要：

- Go `Analyzer` 是可变指针，`init` 能被 Go 运行时自动调用并由 `SkipAnalyzerByConfig` 替换 `Run`；Rust 使用不可变 `static` 和普通公开函数 `init`，既不会自动运行，也与 `&mut Analyzer` 要求不匹配。
- Go `rawCfg.Parse()` 忽略 error 并解引用配置指针；Rust 用 `expect` 把同一“默认配置必须可解析”假设显式化。
- Go 生产包由 Bazel/nogo 注册；Rust 文件没有 Cargo 模块声明和相应第三方 Rust 依赖，只被 `include_str!` 读取为文本。
- Go 配置依赖 gci v0.13.7；Rust 中 `config`、`gci` 和 `analysis` 是迁移占位命名，不能仅凭类似调用形状推断存在可用的 Rust crates。

因此，本文件可用于审阅预期的语义对齐，但当前运行事实、错误类型和初始化机制必须以 Go 版本为权威。

## 扩展指南

若只调整检查策略，最可能修改 `run` 中的 `config::BoolConfig`、diff 过滤或 diagnostic 构造；同时应更新 `build/linter/gci/analysis_test.rs` 的源码契约断言，并与 `analysis.go` 的实际生产逻辑保持一致。新增排除路径应优先修改 `build/nogo_config.json`，而不是在 `run` 内复制路径判断，并验证 `SkipAnalyzerByConfig` 的统一过滤语义。

若要把 Rust 草稿变为可运行实现，需要先完成独立设计工作：确定 `analysis/config/gci/util` 的真实 Rust 模块或上游依赖、在 Cargo 模块树中显式接入 `analysis.rs`、解决静态 analyzer 的可变初始化和一次性注册、把测试从 `include_str!` 形状断言升级为独立行为测试。按照仓库约定，测试仍应保留在同目录的 `analysis_test.rs`，不要嵌入生产源文件。

安全扩展时至少覆盖这些风险：多文件 diff 的顺序和同步；生成文件、vendor 及仓库 exclude 配置的组合；配置解析失败；下游读文件失败时不产生误导性部分结果；空 diff；diagnostic 的位置与消息格式。性能上应继续复用文件名容量预分配，并避免在 diff 已经由下游构造后再做不必要的大字符串复制。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter build/linter/gci` 找到 `analysis.go`、`analysis.rs`、`analysis_test.rs`；`node --file build/linter/gci/analysis.rs` 完整列出 86 行源码和 3 个符号，并报告该文件 `used by 0 files`；对精确 `run`、`init` 节点的查询确认其签名和源码，图中未得到可靠的跨文件调用边。
- 目标源码：`build/linter/gci/analysis.rs`，核对 `Analyzer`、`run`、`init`、固定配置、错误传播、diff 过滤和诊断格式。
- Go 对照：`build/linter/gci/analysis.go`，核对生产语义、`sync.Mutex`、配置零值假设和 `init` 包装。
- 独立 Rust 测试：`build/linter/gci/analysis_test.rs`，3 个测试分别约束 analyzer/签名形状、完整零值配置、文件名—diff—错误—诊断流水线；这些测试使用 `include_str!("analysis.rs")`，属于文本契约测试。
- crate 与模块边界：根 `Cargo.toml` 的包名为 `astersql`、`[lib] path = "pkg/lib.rs"`；`pkg/lib.rs` 仅以 `#[cfg(test)]` 引入 `analysis_test.rs`，未引入目标生产文件。
- Go 构建与配置：`build/linter/gci/BUILD.bazel`、`build/BUILD.bazel`、`build/nogo_config.json`、`go.mod`、`DEPS.bzl`，分别证明 Go 包依赖、nogo 注册、路径排除配置和 gci v0.13.7 版本。
- 过滤实现：`build/linter/util/util.go` 与 `build/linter/util/util.rs`，核对包装原 `Run`、复制/克隆 pass、过滤 `Files` 后再调用原回调的语义，并暴露 Rust 草稿的可变引用不匹配。
- 未运行 Cargo 或 Go 测试：任务是纯文档分析，计划明确禁止 Cargo；结论来自索引、源码、构建配置和独立测试代码审阅。最终结构验证仅检查文档存在且固定二级章节恰好为 11 个。
