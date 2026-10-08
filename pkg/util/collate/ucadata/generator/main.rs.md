# `pkg/util/collate/ucadata/generator/main.rs`

## 文件定位

本文件是 `astersql-util-collate-ucadata-generator` crate 的业务核心，源文件为 [`main.rs`](main.rs)。[`lib.rs`](lib.rs) 通过 `#[path = "main.rs"] pub mod generator` 将其纳入库并重导出公开符号，[`bin.rs`](bin.rs) 的极薄 `main` 只把命令行参数交给 `runGenerator`。[`Cargo.toml`](Cargo.toml) 同时定义了该 library 和名为 `ucadata-generator` 的 binary，且不引入第三方 Rust 依赖。

它不在 SQL 请求运行时临时计算排序权重；它是开发/生成工具，把内嵌 Unicode allkeys 数据转换为 `pkg/util/collate/ucadata` 下的 Go 或 Rust 生成表。运行时排序实现随后消费 `DUCET0400Table` / `DUCET0900Table`，例如 `pkg/util/collate/unicode_0400_ci_impl.rs` 和 `pkg/util/collate/unicode_0900_ai_ci_impl.rs`。

## 核心职责

- 解析 Unicode Collation Element Table（CET）文本：`parseCETHex` 扫描十六进制前缀，`parseCETWeights` 只保留 ai_ci 需要的一级权重，`parseCETEntry` 组合码点与权重。
- 构建紧凑表：`cet::insertWeights` 移除零权重，把最多 4 个 `u16` 打包进一个 `u64`，5–8 个权重则使用 `LongRune8` 哨兵和 `LongRuneMap`。
- 补齐未在 allkeys 显式出现的码点：`calcImplicitWeight` 根据 `unicodeVersion` 选择 4.0.0 或 9.0.0 隐式权重算法；9.0.0 还处理 surrogate、U+FFFD、Hangul 和 Tangut。
- 生成源码：`generateFile` 输出 Go，`generateRustFile` 输出 Rust；两者都先渲染模板，再调用外部格式化器，最后写入文件。
- 提供可复用 CLI 业务入口：`selectOutputTarget` 依文件名选择 Unicode 版本和 Go/Rust 后端，`generateOutputTarget` 组装整个流程，`runGenerator` 校验参数个数。

## 主要符号

- `allkeys0400` / `allkeys0900`：用 `include_str!` 嵌入的 Unicode 4.0.0/9.0.0 allkeys 原文；生成时不需要网络。
- `cetEntry { char, weights }`：一条单码点 CET 记录。多字符 contraction 不是该 MySQL collation 的处理目标。
- `unicodeVersion::{unicode0400, unicode0900}`：决定长度、隐式权重规则和特殊码点分支的版本标记。
- `cet`：一次生成的可变中间状态。`Name`/`URL` 进入生成源码，`Length` 限制码点空间，`MapTable4` 是索引表，`LongRuneMap` 存储长权重，`explicitRune` 防止显式规则被隐式权重覆盖。
- `parseCETHex` / `parseCETWeights` / `parseCETEntry` / `parseAllKeys`：由字节前缀到完整表的逐层解析链。
- `cet::{insertWeights, calcImplicitWeight, getImplicitWeight0400, getImplicitWeight0900}`：权重打包与补齐算法。
- `decomposeHangulSyllable`：把 U+AC00–U+D7AF 拆为 2 或 3 个 Jamo 码点，供 9.0.0 隐式权重组合。
- `render_unicode_template` / `render_rust_unicode_template`：内部模板渲染器。长权重条目按码点排序，保证 `HashMap` 输出可重复。
- `OutputTarget`、`selectOutputTarget`、`generateOutputTarget`、`runGenerator`：对外 CLI 路由和执行 API。

## 执行流程

1. [`bin.rs`](bin.rs) 将 `std::env::args_os()` 传入 `runGenerator`；后者要求恰好一个输出路径，否则返回 `usage: ... <output-path>`。
2. `generateOutputTarget` 调用 `selectOutputTarget`，只接受四个确切文件名：两个 Unicode 版本各自的 `.go` 与 `.rs` 生成文件。目录保留为调用者给定的路径。
3. `buildTable` 从对应的内嵌 allkeys 文本调用 `parseAllKeys`。4.0.0 分配 `0x10000` 项；9.0.0 分配 `0x2CEA1` 项，循环范围为 `1..Length`。
4. `parseAllKeys` 在每行开头尝试 `parseCETEntry`；有效且 `entry.char < length` 的条目进入 `insertWeights`，然后无论是否解析成功都前进到下一换行。
5. `insertWeights` 执行 U+FDFA 版本特例、记录显式码点、去零、打包短/长权重，并为 9.0.0 的 U+FFFD 写入长表覆盖值。
6. `calcImplicitWeight` 遍历未显式定义的码点。9.0.0 的 Hangul 分支通过 `decomposeHangulSyllable` 查 Jamo 的低 16 位权重并组装成一个 `u64`。
7. `generateOutputTarget` 按 `OutputTarget` 选择 Go 或 Rust 模板。渲染后通过子进程运行 `gofmt` 或 `rustfmt --edition 2024 --emit stdout`，成功后用 `std::fs::write` 覆写目标文件。

## 数据与状态

`MapTable4[codepoint]` 是主索引。一到四个非零一级权重以小端槽位方式放入每 16 位一槽的 `u64`；五到八个权重时，主表写 `LongRune8`，实际两个 `u64` 放入 `LongRuneMap[codepoint]`。超过八个非零权重被视为不可达条件。

`explicitRune` 只服务于生成阶段：记录 allkeys 已定义的码点，使 `calcImplicitWeight` 只补空缺。模板输出不包含该集合。`HashMap` 本身没有稳定迭代顺序，所以两个渲染函数都在输出 `LongRuneMap` 前按 rune 排序，避免无意义的生成文件抖动。

## 依赖与调用关系

RustCodeGraph 的实际调用链为：`bin.rs::main -> runGenerator -> generateOutputTarget -> {selectOutputTarget, buildTable, generateFile/generateRustFile}`；`buildTable -> parseAllKeys -> parseCETEntry -> parseCETWeights -> parseCETHex`，且 `buildTable -> cet::calcImplicitWeight`。`getImplicitWeight0900` 调用 `decomposeHangulSyllable`。

内部依赖为 [`magic.rs`](magic.rs) 的 `reverseHexTable` 和 `LongRune8`，以及同目录的 `allkeys-4.0.0.txt`、`allkeys-9.0.0.txt`、`data.go.tpl`、`data_0400.rs.tpl`、`data_0900.rs.tpl`。标准库依赖包括 `HashMap`/`HashSet`、文件写入、路径和子进程。外部运行时工具依赖是 PATH 中的 `gofmt` 与 `rustfmt`。

该模块的直接 Rust 测试是 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，由 `lib.rs` 以独立 `#[cfg(test)]` 模块接入。生成结果另被 `pkg/util/collate/ucadata/unicode_ci_data_test.go` 与 `unicode_0900_ai_ci_data_test.go` 检查：前者对照旧 4.0.0 表并检查长权重唯一性，后者验证 Jamo 只有一个权重且长权重首段非零。

## 错误处理与边界

- `runGenerator` 和 `selectOutputTarget` 对用户输入错误返回 `Result<_, String>`：缺少/多余参数、未知目标文件名、目标路径不是 UTF-8 都会产生可诊断消息。`bin.rs` 将其打印到 stderr 并以状态 1 退出。
- CET 解析器是面向受控 allkeys 输入的字节索引实现，多处直接访问 `as_bytes()[0]` / `[1]`。它不是通用、容错的外部文本 API；截断的条目、缺少换行或非 ASCII 分隔符可导致 panic。
- `parseCETHex` 使用长度为 256 的反向表按字节查询；对 allkeys 的 ASCII 输入成立。它保留 Go 版的前缀扫描语义，而不是返回结构化解析错误。
- 权重必须不超过 `u16::MAX`；非零一级权重超过 8 个时 `insertWeights` panic。U+FDFA 在 4.0.0 被忽略，在 9.0.0 被截断到 8 个权重。
- 模板标记缺失、格式化器启动/写入/执行失败、格式化返回非零、文件写入失败均使用 `expect`/`panic!`/`assert!`。因此 `Result` 只覆盖 CLI 路由与参数错误，不覆盖生成过程的 I/O/工具错误。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或全局可变状态。每次调用的 `cet`、映射和字符串都由当前进程独占，函数返回后按 Rust 所有权规则释放。

`format_source` 为每个输出启动一个格式化子进程，持有其 stdin/stdout/stderr：父进程写完 stdin 后调用 `wait_with_output`同步等待，再取 stdout。单次 CLI 只生成一个目标，没有内部并发写入。如多个外部进程同时指向同一文件，`std::fs::write` 没有锁或原子替换保护，调用方必须串行化。

## 与 Go 版本的对应关系

直接对照文件是 [`main.go`](main.go)。Rust 保留了 Go 的 CET 解析、去零与权重打包、U+FDFA/U+FFFD 特例、Unicode 4.0.0/9.0.0 隐式权重区间、Hangul 分解、表名和来源 URL。`parseAllKeys` 的逐行恢复策略也与 Go 一致。

差异主要在工程化边界：

- Go `main` 只支持两个 Go 输出，并使用最后一个 `os.Args` 选择；Rust 要求恰好一个路径，并额外支持两个 Rust 输出。
- Go 通过 `text/template` 和 `go/format.Source` 在进程内生成 Go 源码；Rust 为 Go 模板实现定向渲染，并通过外部 `gofmt` 处理 Go 输出，Rust 输出则使用版本专用模板和外部 `rustfmt`。
- Go `generateFile` 总是写固定当前目录文件；Rust `generateOutputTarget` 保留输入路径的目录，仅用 basename 判定目标类型。
- Rust 的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 是独立测试文件，实例化地核对上述移植语义，符合本仓库“Rust 源码与测试分文件”的约束。

## 扩展指南

- 增加新 Unicode 版本时，需同步扩展 `unicodeVersion`、内嵌 allkeys 常量、`buildTable`、版本专用隐式权重规则、Rust 模板选择、`OutputTarget` 和 `selectOutputTarget`；不能只复制旧版本区间。
- 修改解析时优先保持 `parseCETHex -> parseCETWeights -> parseCETEntry -> parseAllKeys` 的剩余切片协议，并在 `migration_aster_unit_test.rs` 中增加截断输入、非法分隔符、越界码点和最大权重数的独立测试。
- 修改权重打包或隐式算法时，必须同时核对 Go `main.go`、`unicode_ci_data_test.go`、`unicode_0900_ai_ci_data_test.go` 以及 Rust 运行时消费者。兼容风险是已存排序键与 Go/MySQL 排序结果改变。
- 修改模板渲染时保持长表排序和生成文件的公共 API 形状；两个 Rust 模板的字段名/API 并不完全相同。在独立测试文件中扩展现有双版本生成用例，不要把测试写入 `main.rs`。
- 若要将 panic 改为可恢复错误，需从 `format_source`/`write_formatted_source` 一直向上调整 `generateFile`/`generateRustFile` 和 `generateOutputTarget` 签名，同时保留 `bin.rs` 非零退出语义。
- 性能上，`calcImplicitWeight` 对整个码点空间线性扫描，生成源码也按整表在内存中构造。扩大 `Length` 前应评估内存、渲染时间和生成文件体积。

## 验证依据

- RustCodeGraph 索引状态：项目共索引 11,467 个文件，目标目录中 `main.rs`、`bin.rs`、`lib.rs`、`magic.rs`、`main.go` 和 `migration_aster_unit_test.rs` 均可见。
- RustCodeGraph 符号/调用证据：`node runGenerator`、`node generateOutputTarget`、`node buildTable`、`node parseAllKeys`。其中 Rust `parseAllKeys` 被 `buildTable` 调用，并调用 `parseCETEntry` 和 `insertWeights`；`generateOutputTarget` 被 `runGenerator` 调用，并调用目标选择、建表与两种生成后端。
- 已读生产/装配文件：[`main.rs`](main.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`bin.rs`](bin.rs)、[`magic.rs`](magic.rs)，以及运行时消费者 `pkg/util/collate/unicode_0400_ci_impl.rs` 和 `pkg/util/collate/unicode_0900_ai_ci_impl.rs`。本 package 无 `doc.go`。
- 已读 Go 对照与测试：[`main.go`](main.go)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、`pkg/util/collate/ucadata/unicode_ci_data_test.go`、`pkg/util/collate/ucadata/unicode_0900_ai_ci_data_test.go`。
- `migration_aster_unit_test.rs` 直接覆盖：十六进制/权重/条目解析，短长权重打包，Hangul 分解，4.0.0/9.0.0 隐式权重，Go/Rust 格式化输出，四种目标路由与未知目标拒绝。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用任务文件指定的 11 章节结构命令，并人工复核结论均可回溯到上述符号和文件。
