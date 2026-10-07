# `build/linter/gofmt/analyzer.rs`

## 文件定位

本文件是 Go 包 `build/linter/gofmt` 中 [`analyzer.go`](analyzer.go) 的 Rust 逐文件迁移稿，定义名为 `gofmt` 的静态分析器。它面向 Go 源文件：从 `analysis::Pass` 提取文件名，调用 `gofmt::RunRewrite` 检查格式化、`-s` 简化以及 `interface{}` 到 `any` 的改写，并把差异作为诊断报告。

当前 Rust 文件没有进入可执行的 Rust 模块树。根 `Cargo.toml` 的 workspace members 中没有 `build/linter/gofmt` crate，也没有该目录自己的 `Cargo.toml`；`pkg/lib.rs` 仅在 `#[cfg(test)]` 下通过 `#[path = "../build/linter/gofmt/analyzer_test.rs"]` 注册独立测试，而测试再以 `include_str!("analyzer.rs")` 读取本文件文本。与之相对，Go 实现由 `build/linter/gofmt/BUILD.bazel` 的 `go_library(name = "gofmt")` 编译，并作为 `build/BUILD.bazel` 中 `nogo` 目标的依赖。因此，本文件目前应视为“保留 Go 语义形状、由文本结构测试约束的迁移产物”，不能据此声称 Rust 版已在 lint 主链运行。

## 核心职责

1. `Analyzer` 描述分析器元数据：名称为 `gofmt`，无前置 analyzer，运行回调为本文件的 `run`。
2. `init` 把命令行 flag `need-simplify` 绑定到包级状态 `needSimplify`，运行默认值设为 `true`，随后调用 `util::SkipAnalyzerByConfig` 接入按配置排除文件的包装逻辑。
3. `run` 从 `pass.Files` 收集真实文件名，排除空文件名和生成的 `failpoint_binding__.go`，逐文件执行格式化/简化/改写检查；有 diff 时通过 `pass.Report` 报告，有错误时保留底层错误链并立即返回。

它不直接修改源文件，也不聚合多个文件的 diff；每个输入文件至多产生一条本函数显式上报的诊断。真正的读取、格式化和 diff 生成由外部 `gofmt::RunRewrite` 承担。

## 主要符号

- `pub static Analyzer: analysis::Analyzer`：公开的 analyzer 描述值。字段 `name` 为 `"gofmt"`，`doc` 用 `concat!` 保留 Go 版两个相邻字符串直接拼接的结果，`requires` 为空切片，`run` 指向本文件回调。因为没有分析依赖，调度器无需先提供其他 analyzer 的结果。
- `static mut needSimplify: bool`：对应 Go 包级布尔变量的迁移形状。静态初值为 `false`，但 `init` 注册 flag 时声明运行默认值为 `true`。读写都依赖 `unsafe`，其安全性要求初始化和分析执行之间有严格的时序，并且不能发生无同步的并发写入。
- `pub fn init()`：合并 Go 文件中的两个 `init` 函数，先执行 `Analyzer.Flags.BoolVar(...)`，再执行 `util::SkipAnalyzerByConfig(&Analyzer)`。顺序很重要：先建立 analyzer flag，再让配置层包装 analyzer。
- `pub fn run(pass: &mut analysis::Pass) -> anyhow::Result<Option<Box<dyn std::any::Any>>>`：分析入口。成功时固定返回 `Ok(None)`，与 Go 的动态 `nil, nil` 对应；结果载荷不供后续 analyzer 使用。

文件没有自定义结构体、枚举、trait、`impl` 或条件编译项。符号名保留 Go 风格大小写，这是迁移对照的一部分，而非惯用 Rust API 设计。

## 执行流程

初始化阶段由 `init` 完成：

1. `Analyzer.Flags.BoolVar` 将 `need-simplify` 注册到 `needSimplify`，默认开启 `gofmt -s` 类简化，帮助文本说明为 `run gofmt with -s for code simplification`。
2. `util::SkipAnalyzerByConfig(&Analyzer)` 为 analyzer 接入仓库 lint 排除配置。当前 Rust 生产文件没有模块接线，因此这是代码表达的预期初始化流程；真实在构建中执行的是 Go 版对应逻辑。

分析阶段由 `run` 完成：

1. 以容量 10 创建 `fileNames`，容量只是减少常见规模下的重新分配，不限制文件数量。
2. 遍历 `pass.Files`，用 `pass.Fset.PositionFor(f.Pos(), false)` 将每个 AST 文件的起始位置转换为未做位置调整的文件信息。
3. 跳过 `Filename` 为空的条目，也跳过文件名以 `failpoint_binding__.go` 结尾的生成文件；其余文件名按输入顺序保存。
4. 构造唯一的 `gofmt::RewriteRule`：模式 `interface{}`，替换为 `any`。
5. 按收集顺序对每个文件调用 `gofmt::RunRewrite(&f, needSimplify, &rules)`。任一调用失败即停止后续文件处理，并以文件名上下文返回错误。
6. `None` diff 不报告；`Some(diff)` 生成 `analysis::Diagnostic { Pos: 1, Message: format!("\n{}", diff) }`。固定位置 `1` 使诊断属于文件级结果，消息前置换行保留 Go 输出形状。
7. 所有文件处理完毕后返回 `Ok(None)`。

## 数据与状态

长期状态只有 `Analyzer` 和 `needSimplify`。`Analyzer` 持有元数据、flag 集合和 `run` 回调；`needSimplify` 是进程级开关，不属于某个 `Pass`。`init` 会建立二者的绑定，之后每次 `run` 都读取该开关。

每次运行的临时数据包括 `fileNames: Vec<String>`、单元素 `rules` 向量、当前文件名和 `RunRewrite` 返回的可选 diff。文件名从 `token.FileSet`/AST 位置复制为拥有所有权的字符串，因此遍历 `pass.Files` 结束后仍可安全用于逐文件调用。代码不缓存源内容或 diff，不在不同 `Pass` 间保留结果。

关键不变量是：空文件名与 `failpoint_binding__.go` 永远不传给 `RunRewrite`；规则始终只有 `interface{}` → `any`；只有非空 diff 才报告诊断；成功返回值始终没有动态结果。

## 依赖与调用关系

下游依赖可由函数体直接确认：

- `analysis::{Analyzer, Pass, Diagnostic}` 提供 analyzer 元数据、输入 AST/文件集和诊断接口。
- `gofmt::{RewriteRule, RunRewrite}` 执行目标文件的格式化、简化和规则改写比较。
- `util::SkipAnalyzerByConfig` 将仓库的文件排除配置应用到 analyzer。
- `anyhow` 为 `RunRewrite` 错误附加当前文件名上下文，同时保留错误 source 链。
- `std::any::Any` 仅用于表达与 Go `any` 返回值兼容的动态结果类型；本实现实际总是返回 `None`。

RustCodeGraph 将本文件索引为 3 个符号（静态 `Analyzer`、`init`、`run`），但针对 `init`/`run` 的 callers 与 callees 查询均未返回图边；仓库搜索也没有发现生产 Rust 模块引用本文件。当前唯一 Rust 上游是 `pkg/lib.rs` 注册的 `analyzer_test.rs`，且测试只把源码作为字符串检查。Go 侧则由 `build/linter/gofmt/BUILD.bazel` 暴露公共 library，再由 `build/BUILD.bazel` 的 `nogo` 依赖列表接入构建期静态分析主链。

## 错误处理与边界

`gofmt::RunRewrite` 的错误通过 `anyhow::Error::new(err).context(format!("could not run gofmt ({f})"))` 返回。这样既标明失败文件，又保留原始错误供调用方沿 source 链检查。它与 Go 版 `fmt.Errorf("could not run gofmt: %w (%s)", err, f)` 在“包装底层错误并携带文件名”上对应，但最终显示文本的标点/顺序不同。

错误采用 fail-fast 策略：前面文件已经报告的诊断不会回滚，当前错误之后的文件不会继续检查。`Filename == ""` 被静默忽略；任何路径只要以 `failpoint_binding__.go` 结尾也被忽略，不要求完整 basename 相等。`RunRewrite` 返回无 diff 是正常情况，不构造诊断。

本文件没有自行处理文件不存在、权限、编码、解析失败或外部 formatter 异常，这些边界由 `RunRewrite` 的错误承担。`Pos: 1` 不是 diff 的精确行列位置；消费者应把它理解为文件级报告锚点。文档字符串的两个片段之间没有空格，这是 Go 原文件现有拼接结果，而不是本文推断的修正目标。

## 并发与资源生命周期

`run` 本身是同步的串行循环：不创建线程、异步任务、锁、通道或事务。文件按 `pass.Files` 派生的顺序依次调用 `RunRewrite`，临时向量和 diff 在函数返回时释放。文件句柄等资源若存在，完全由 `RunRewrite` 内部管理，本文件没有可见的显式清理协议。

并发风险集中在 `static mut needSimplify`：`init` 通过可变引用绑定它，`run` 通过 `unsafe` 读取它。只有在 flag 配置完成后不再写入，并且 analyzer 执行与写入不并发时，才可避免 Rust 数据竞争。由于该生产文件当前未被 Rust 编译接线，仓库没有提供类型系统或运行测试层面的安全证明；未来正式接线时应优先用一次初始化的线程安全状态、不可变配置传递或原子布尔消除 `static mut`，并验证 `analysis::Analyzer` 的 flag API 所需可变性。

## 与 Go 版本的对应关系

对应源是同目录 `analyzer.go`：

- Go `var Analyzer = &analysis.Analyzer{...}` 对应 Rust 静态 `Analyzer`；`Name`、拼接后的 `Doc`、空依赖和 `Run: run` 意图一致。
- Go `var needSimplify bool` 的零值为 `false`，对应 Rust 静态初值；两边都由 flag 注册把默认运行值设为 `true`。
- Go 的两个 `init` 分别注册 flag、调用 `SkipAnalyzerByConfig`；Rust 合并为一个函数并维持源码顺序。
- 两边都通过 `PositionFor(..., false)` 取文件名，排除空名和 `failpoint_binding__.go`，预分配容量 10，并应用 `interface{}` → `any` 规则。
- Go 的 `diff == nil` 对应 Rust 的 `None`，Go 返回 `nil, nil` 对应 Rust `Ok(None)`；两边有 diff 时都以位置 1、换行开头的消息报告。
- 两边错误都包装原始 `RunRewrite` 错误并记录文件名，但错误文本格式不完全相同。

相关 Rust 测试 `analyzer_test.rs` 的三个测试只做源码字符串断言：分别检查 analyzer/init 形状、文件过滤与 rewrite/diagnostic 形状、动态空结果与错误链写法。它们不会编译或执行 `analyzer.rs`，也不会实际创建临时 Go 文件调用 formatter。同目录没有 Go 单元测试；仓库中 `build/linter/util/exclude_test.go` 只覆盖通用排除逻辑对名称 `gofmt` 的处理，不直接覆盖本文件的 `run`。

## 扩展指南

- 新增或修改格式化规则：改 `run` 中 `rules` 的构造，并在独立的 `analyzer_test.rs` 更新结构断言；正式接线后还应增加行为测试，覆盖命中、不命中、规则冲突和 Go 版本边界。
- 改变文件筛选：修改收集 `fileNames` 的条件，并分别覆盖空文件名、普通文件、路径结尾命中/不命中 `failpoint_binding__.go`。不要把测试逻辑内嵌进生产文件。
- 改变 flag 或初始化：修改 `needSimplify`/`init` 时保持 flag 默认值、初始化顺序和 `SkipAnalyzerByConfig` 行为与 Go 版同步；若转为线程安全状态，应同步调整 analyzer flag API 和并发测试。
- 改变诊断：关注下游对固定 `Pos: 1`、消息前导换行和 diff 文本的解析兼容性。若需要精确定位，应先确认 `RunRewrite` 是否提供可映射的位置信息。
- 正式接入 Rust：需要为目录建立真实 crate 或模块声明、声明 `analysis`/`gofmt`/`util`/`anyhow` 依赖，并把文本断言升级为可编译、可执行的独立测试。此工作会改变当前“未接线迁移稿”的边界，不应仅靠本文件局部编辑完成。

兼容风险主要是与 Go analyzer 的 flag、排除配置、错误链和诊断文本漂移；正确性风险是漏检生成文件或重复/错误 rewrite；性能风险来自逐文件串行读取和格式化，规则或输入规模增加时尤其需要测量。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter build/linter/gofmt` 找到 `analyzer.go`、`analyzer.rs`、`analyzer_test.rs`。
- RustCodeGraph `node --file build/linter/gofmt/analyzer.rs --offset 1 --limit 240`：核对完整 86 行源码、3 个主要符号及所有分支；`query gofmt --limit 20 --json` 核对 Rust/Go 同名符号和三个独立 Rust 测试。
- RustCodeGraph 对 `build/linter/gofmt/analyzer.rs::init`、`::run` 的 callers/callees 查询未返回边；再以仓库搜索核对生产模块引用，确认当前没有 Rust 生产接线。
- `build/linter/gofmt/analyzer.go`：核对 Go 原始 analyzer、两个 `init`、文件筛选、rewrite、错误包装与诊断行为。
- `build/linter/gofmt/analyzer_test.rs`、`pkg/lib.rs`：核对 Rust 测试仅通过 `include_str!` 做源码结构断言，以及该测试的 `#[cfg(test)]` 注册位置。
- 根 `Cargo.toml`：核对 workspace/crate 边界；`build/linter/gofmt` 不是 workspace member，且目录无独立 Cargo manifest。
- `build/linter/gofmt/BUILD.bazel`、`build/BUILD.bazel`：核对 Go library 的依赖和其进入 `nogo` 的真实构建调用链。
- `build/linter/util/exclude_test.go`：核对仓库中与名称 `gofmt` 有关的 Go 测试只覆盖通用排除判断，不是本 analyzer 的直接行为测试。
- 按任务约束不运行 Cargo；交付验证使用任务指定的 11 章节结构命令，并人工复核本文所有“当前已接线/未接线”陈述均有上述源码、构建清单或图查询依据。
