# `pkg/util/collate/ucadata/generator/bin.rs`

## 文件定位

本文件是 Cargo 包 `astersql-util-collate-ucadata-generator` 的可执行入口。`pkg/util/collate/ucadata/generator/Cargo.toml` 通过 `[[bin]]` 将它注册为 `ucadata-generator`，同时以 `lib.rs` 作为同包库入口。它不属于 TiDB SQL 请求运行时主链，而是离线生成 UCA（Unicode Collation Algorithm）权重表源码的开发工具入口。

实际的参数校验、Unicode 表构造和文件渲染位于 `pkg/util/collate/ucadata/generator/main.rs`，并经 `lib.rs` 的 `pub use generator::*` 暴露。`pkg/util/collate/ucadata/data.rs:40-42` 给出了从仓库根目录调用本二进制、重新生成两份 Rust 排序表的命令，因而这是“生成期入口”，不是生成结果的运行时读取入口。

## 核心职责

本文件只承担三项进程边界职责，均集中在 `bin.rs::main`：

1. 通过 `std::env::args_os()` 原样取得操作系统参数，避免入口层提前假设参数必须是 UTF-8。
2. 把完整参数迭代器交给库函数 `astersql_util_collate_ucadata_generator::runGenerator`，不在二进制层复制目标识别或表生成逻辑。
3. 当库函数返回 `Err(String)` 时，将带 `ucadata-generator:` 前缀的诊断写到标准错误，并以状态码 `1` 终止进程；成功时让 `main` 正常返回，即状态码 `0`。

因此，本文件是一个很薄的 CLI 适配层。目标文件名到 Unicode 版本/输出语言的映射，以及具体生成算法，都不应加入这里。

## 主要符号

- `fn main()`（`bin.rs:8`）：文件中唯一的函数，也是私有的 Rust 二进制入口；没有参数和返回值。
- `std::env::args_os()`（`bin.rs:10`）：产生包含程序名在内的 `OsString` 参数流。库函数把第一项视作程序名、第二项视作输出路径，并拒绝更多参数（`main.rs::runGenerator`）。
- `astersql_util_collate_ucadata_generator::runGenerator(...)`（`bin.rs:10`）：唯一的业务委托点。其签名是 `pub fn runGenerator(args: impl IntoIterator<Item = OsString>) -> Result<(), String>`（`main.rs:637`）。
- `eprintln!(...)`（`bin.rs:11`）：只处理以 `Result::Err` 表示的可诊断失败，并写入标准错误而非标准输出。
- `std::process::exit(1)`（`bin.rs:12`）：把上述失败转换为 shell/构建系统可观察的非零退出状态。

文件中没有常量、类型、trait、`impl`、公开 API 或条件编译项。公开复用边界是库中的 `runGenerator`，不是二进制的 `main`。

## 执行流程

1. 操作系统启动 `ucadata-generator`，进入 `bin.rs::main`。
2. `main` 调用 `std::env::args_os()`，将含程序名的完整参数流传给 `main.rs::runGenerator`。
3. `runGenerator` 取出程序名和唯一输出路径：缺少输出路径或存在额外参数时，返回 `usage: <program> <output-path>`；没有程序名时仅为诊断回退到 `ucadata-generator`。
4. `runGenerator` 调用 `main.rs::generateOutputTarget`。后者通过 `selectOutputTarget` 仅按路径末尾文件名，在 Unicode 4.0.0/9.0.0 与 Go/Rust 输出后端之间选择；随后 `buildTable` 解析内嵌 allkeys 数据并计算隐式权重，再调用 `generateFile` 或 `generateRustFile` 格式化并写出调用方指定的完整路径。
5. 若第 3～4 步返回 `Ok(())`，`main` 正常结束。若返回 `Err(error)`，入口输出 `ucadata-generator: {error}`，随后立即 `exit(1)`。

RustCodeGraph 将 `runGenerator -> generateOutputTarget -> {selectOutputTarget, buildTable, generateFile/generateRustFile}`识别为库内调用链；它没有把 `bin.rs` 中带 crate 全限定名的调用解析成 `main -> runGenerator` 静态边，但该边由 `bin.rs:10` 的直接调用明确给出。

## 数据与状态

入口层唯一流经的数据是一次性的 `OsString` 参数迭代器和失败时的 `String` 错误。`main` 不缓存参数、不保存全局状态，也不持有生成表。

路径在入口处仍是 `OsString`，所以用法错误可以在不做 UTF-8 转换时判断。真正生成前，`generateOutputTarget` 才要求完整输出路径可转为 UTF-8 字符串；目标选择只检查 `Path::file_name()`，目录部分则原样用于最终写入。表数据、输出模板、Unicode 版本和 `OutputTarget` 状态均由库模块拥有。

## 依赖与调用关系

上游是操作系统或开发命令。仓库内的明确调用说明位于 `pkg/util/collate/ucadata/data.rs:40-42`，两条 `cargo run -p astersql-util-collate-ucadata-generator --bin ucadata-generator -- <path>` 命令分别生成 Unicode 9.0.0 和 4.0.0 的 Rust 数据文件。普通 Rust 源码不会像函数一样调用 `main`；RustCodeGraph 的 `callers` 查询未找到有意义的函数调用方，符合进程入口语义。

直接下游是库 crate 的 `runGenerator`。Cargo 包本身没有声明第三方依赖；入口仅使用标准库，而库代码通过 `lib.rs` 的 `#[path = "main.rs"] pub mod generator` 装配。继续向下，生成流程依赖内嵌 allkeys/模板数据、外部 `gofmt` 或 `rustfmt` 子进程，以及 `std::fs::write`。这些是委托实现的依赖，不是 `bin.rs` 自己管理的模块状态。

`Cargo.toml` 的 `[package.metadata.porting]` 把本包映射到 Go 包 `pkg/util/collate/ucadata/generator`；这也是 Go 对照与迁移边界的直接配置证据。

## 错误处理与边界

`main` 能稳定映射的错误仅是 `runGenerator` 返回的 `Err(String)`：参数数目不等于一个、目标文件名不受支持，或输出路径无法转为 UTF-8。它为错误统一增加程序名前缀、写标准错误并返回状态码 `1`，但不打印 usage 到标准输出，也不使用不同错误码区分错误类别。

需要特别注意，库中的所有失败并不都通过 `Result` 返回。`main.rs::format_source` 在格式化器无法启动、stdin 写入失败、等待失败或子进程退出失败时会 panic/assert；`write_formatted_source` 写文件失败时也会 panic。因此 `bin.rs` 的 `if let Err` 不捕获 panic，这些失败遵循 Rust 默认 panic 终止行为。当前实现也没有原子写入或回滚：生成失败是否留下旧文件或部分外部影响取决于发生失败的阶段。

边界协议是“恰好一个输出路径”，且只接受四个固定 basename：`unicode_ci_data_generated.go`、`unicode_0900_ai_ci_data_generated.go`、`unicode_ci_data_generated.rs`、`unicode_0900_ai_ci_data_generated.rs`。入口不得自行放宽该协议，否则会绕过 `selectOutputTarget` 的版本/后端约束。

## 并发与资源生命周期

本入口是单线程、同步、一次生成一个目标的短生命周期进程。它不创建线程、异步任务、锁、通道或事务，也没有需要跨调用保存的资源。

参数迭代器在 `runGenerator` 中顺序消费；构造出的表在一次调用中拥有，写出后随栈帧释放。委托实现会同步启动一个 `gofmt` 或 `rustfmt` 子进程，通过管道写入源码并用 `wait_with_output` 等待退出，随后同步写目标文件。`std::process::exit(1)` 会立即结束进程，因此未来若在入口增加需要析构器完成的清理逻辑，不能假设错误路径会运行该析构逻辑。

## 与 Go 版本的对应关系

Go 对照入口是 `pkg/util/collate/ucadata/generator/main.go::main`（`main.go:403-420`）。Go 版本读取 `os.Args` 的最后一项，根据两个 `.go` 文件名直接分支，在入口中完成 allkeys 解析、表名/URL 设置、隐式权重计算和写文件；未知目标直接 `panic("unreachable")`。

Rust 版本保留相同的 Unicode 4.0.0/9.0.0 表构造语义，但把生成逻辑移到可复用库函数：`bin.rs::main` 只负责进程适配，`main.rs::runGenerator` 严格校验恰好一个输出路径，`generateOutputTarget` 同时支持 Go 和 Rust 两种输出文件。与 Go 相比，Rust 对参数/未知目标提供可打印的 `Result` 错误；而格式化和写文件失败仍以 panic 方式处理，与 Go 生成器的 panic 风格相近。

相关迁移测试在独立文件 `pkg/util/collate/ucadata/generator/migration_aster_unit_test.rs`。它覆盖 Go/Rust 模板生成、四种目标选择和未知目标错误，但当前没有直接启动二进制或断言 `main` 的标准错误、退出码，也没有直接覆盖 `runGenerator` 的缺参/多参 usage 分支。

## 扩展指南

- 新增 Unicode 版本或输出文件名时，应在库层扩展 `main.rs::OutputTarget`、`selectOutputTarget`、`buildTable` 和对应模板/生成函数；保持 `bin.rs::main` 为统一委托入口。
- 修改 CLI 参数协议时，应优先修改 `main.rs::runGenerator`，并在独立测试文件中加入缺参、多参、非 UTF-8 路径和成功路径用例；不要把测试内嵌到 `bin.rs`。若需要验证 stderr 与退出码，应新增独立的二进制集成测试。
- 若希望所有 I/O/格式化失败都得到统一前缀和状态码，需要先把 `format_source`、`write_formatted_source` 等 panic 路径改为可传播错误，再由 `runGenerator` 汇总；只改 `main` 无法捕获现有 panic。
- 若未来加入临时文件、锁或其他清理资源，应避免在仍需依赖析构清理时直接调用 `process::exit`，或者在退出前显式清理。
- 生成协议同时影响 Go 与 Rust 数据文件，修改目标选择或模板时应同步验证 `migration_aster_unit_test.rs`，并关注 Unicode 版本兼容性、生成结果稳定性、外部格式化器可用性及大表构造的时间/内存开销。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；查询到 `bin.rs` 只有 `main` 等 2 个索引符号，精确定位 `main` 于第 8 行。
- RustCodeGraph 符号/调用查询：`node pkg/util/collate/ucadata/generator/bin.rs::main`、`query/node runGenerator`、`node generateOutputTarget`、`node selectOutputTarget`、`node buildTable`、`node generateFile`、`node generateRustFile`。库内调用边显示 `runGenerator -> generateOutputTarget`，以及后者到目标选择、表构造和两种生成后端的边；跨 crate 的 `main -> runGenerator` 由 `bin.rs:10` 直接源码核验。
- 已读生产与配置路径：`pkg/util/collate/ucadata/generator/bin.rs`、`lib.rs`、`main.rs:525-653`、`Cargo.toml`、`pkg/util/collate/ucadata/data.rs:34-42`。
- 已读 Go 对照：`pkg/util/collate/ucadata/generator/main.go:377-420`，核对模板写出和 Go `main` 的两个目标分支。
- 已读独立测试：`pkg/util/collate/ucadata/generator/migration_aster_unit_test.rs`，其测试覆盖解析/打包、Go/Rust 渲染、四种目标选择和未知目标；通过 `rg` 确认仓库中没有其他 `runGenerator` 调用或专门的 `bin.rs` 测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证目标文件存在且恰好包含 11 个固定二级标题，并人工复核文档只陈述上述源码、配置、图查询和测试能够支持的事实。
