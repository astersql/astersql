# `pkg/parser/parsergen/main.rs` 逻辑说明

## 文件定位

`pkg/parser/parsergen/main.rs` 是 Cargo 二进制目标 `astersql-parsergen` 的进程入口。`pkg/parser/parsergen/Cargo.toml` 同时把 `lib.rs` 声明为库目标、把本文件声明为 `[[bin]]`；因此本文件只负责命令行协议和退出状态，生成算法位于同一 crate 的库模块中。根 `Cargo.toml` 将 `pkg/parser/parsergen` 纳入 workspace，`pkg/parser/Cargo.toml` 又以路径依赖使用该库。

该工具是显式维护命令，不在普通 parser crate 编译或数据库启动时自动运行。它以 `pkg/parser` 为根读取 `grammar/main.astergram`、`grammar/hint.astergram`，并生成或校验 `generated/main_tables.rs`、`generated/hint_tables.rs`、`generated/lexer_tokens.rs`。这些路径由 `main` 传入的 `parser_root` 和 `generate.rs` 中的 `GENERATED_OUTPUT_NAMES` 共同确定。

## 核心职责

本文件承担四项边界职责：解析且严格限制为一个位置参数；打印静态帮助文本 `HELP`；从编译期的 `CARGO_MANIFEST_DIR` 推导 parser 根目录；把 `generate`/`check` 分派给库 API，并把结果映射成适合脚本和 CI 使用的退出码。

它不解析 `.astergram`、不构造 LALR(1) 自动机、不渲染表，也不决定三个输出文件的内容。这些行为分别在 `grammar.rs`、`automaton.rs`、`table.rs`、`render.rs`、`generate.rs` 中实现，并由 `lib.rs` 重导出。本文件也不是 SQL 请求运行时入口；`pkg/parser/lib.rs` 通过 `include!("generated/...")` 消费已经提交的静态产物。

## 主要符号

- `HELP: &str`：私有静态帮助文本，定义程序名、三个命令和 `-h`/`--help` 别名。文本明确 `generate` 会写文件，而 `check` 不写文件。
- `main() -> ExitCode`：唯一函数和唯一进程入口，属于二进制内部实现，不作为库 API 导出。RustCodeGraph 的文件节点确认本文件共 60 行、只有常量和该函数两个主要符号。
- `write_generated_outputs(&Path) -> Result<(), GenerationError>`：从 `astersql_parsergen` 门面导入的下游 API，真实定义在 `pkg/parser/parsergen/generate.rs:141`。
- `check_generated_outputs(&Path) -> Result<(), CheckGeneratedOutputsError>`：同一门面导入的只读校验 API，真实定义在 `pkg/parser/parsergen/generate.rs:162`。

本文件没有类型、trait、`impl`、feature 或条件编译项；所有命令在该二进制的每次构建中都存在。

## 执行流程

1. `main` 从 `std::env::args()` 丢弃程序名，读取第一个参数为 `command`。
2. 它立即尝试读取第二个参数。只要存在多余参数，不论第一个参数是否合法，都向标准错误打印“expected exactly one command”和帮助文本，并返回退出码 2。
3. 它用编译期 `env!("CARGO_MANIFEST_DIR")` 得到 `pkg/parser/parsergen`，再取父目录作为 `pkg/parser`。该路径与调用进程的当前工作目录无关。
4. `generate` 调用 `write_generated_outputs(parser_root)`：库层读取两份文法、生成三份确定性内容、确保 `generated/` 存在并逐一写入。
5. `check` 调用 `check_generated_outputs(parser_root)`：库层重新生成期望内容，再逐字节读取并比较三个已提交文件；它只报告问题，不创建、删除或覆写产物。
6. `help`、`-h`、`--help` 向标准输出打印 `HELP` 并成功退出。
7. 未知命令或缺少命令向标准错误打印诊断和帮助，返回退出码 2；生成或检查的库错误只打印 `error: {error}`，返回退出码 1。

## 数据与状态

本文件的全部运行时状态局限于栈上的参数迭代器、可选命令和借用的 `Path`。`HELP` 是只读的静态字符串；`parser_root` 借用编译期嵌入的清单目录字符串。没有全局可变状态、缓存、环境变量写入或数据库状态。

持久化状态只会经 `generate` 分支的下游函数改变：`generate.rs` 创建 `pkg/parser/generated` 并写三个固定名称文件。写入是按文件顺序直接进行的，不是跨三个文件的原子事务；中途失败可能留下部分文件已更新。`check` 分支仅读取文法和生成文件，积累所有 missing/stale 问题后统一返回。

## 依赖与调用关系

上游入口是操作员、维护脚本或 CI 对 Cargo 二进制的显式调用，例如仓库契约测试 `pkg/parser/parser_no_legacy_dependency_aster_unit_test.rs` 要求维护文档包含 `cargo run -p astersql-parsergen --bin astersql-parsergen -- generate` 和对应的 `check` 命令。没有证据表明数据库进程在启动或解析 SQL 时调用本二进制。

直接下游边为 `main -> write_generated_outputs` 和 `main -> check_generated_outputs`。两者先进入 `generate_outputs`，再通过 `generate_parser_output` 读取 `grammar/main.astergram` 与 `grammar/hint.astergram`，调用 `Grammar::parse`、`GeneratedParser::build` 和渲染函数。`generate` 随后进入 `std::fs::create_dir_all`/`fs::write`；`check` 进入 `fs::read` 和字节比较。

RustCodeGraph `query` 将两个导入解析到 `pkg/parser/parsergen/generate.rs:141` 和 `:162`；`node --file pkg/parser/parsergen/main.rs` 给出了本文件完整源码。图工具对该入口执行 `callers`/`callees` 时未返回可用边文本，因此上述边又以本文件的直接调用点和 `generate.rs` 定义交叉核验，而没有把图中“无输出”解释为“无调用”。

## 错误处理与边界

命令行用法错误与执行失败有意区分：缺参、多参和未知命令返回 2；生成或检查失败返回 `ExitCode::FAILURE`（1）；帮助和成功执行返回 0。所有错误诊断写到标准错误，帮助命令写到标准输出。

`parser_root` 的 `.parent().expect(...)` 是唯一 panic 边界。正常 Cargo 构建时清单目录固定为 `pkg/parser/parsergen`，因此具有父目录；若未来移动 crate 而不保持这一层级，程序会在分派前 panic。库错误不 panic，而通过 `Display` 压平成一行或多行诊断：生成错误包含具体读、解析、构造或写入路径；检查错误区分 `Missing` 与 `Stale`，并提示运行 `generate`。

参数按操作系统字符串经 `env::args()` 读取，因此非 UTF-8 参数可能由标准库在转换阶段导致 panic；本文件没有采用 `args_os()`。命令匹配区分大小写，也不接受组合选项、`--` 后参数或输出目录覆盖。

## 并发与资源生命周期

本文件是单线程、同步、一次性进程入口，没有线程、异步任务、锁、通道或共享所有权。参数迭代器和路径借用都在 `main` 返回时销毁，退出码由 Rust 运行时交给操作系统。

文件句柄由下游 `std::fs` 便捷函数在每次调用结束时关闭。`generate` 串行写入三个文件，不持有长期资源，也没有并发写保护；因此不应同时对同一工作树运行多个 `generate`，也不应在 `generate` 写入期间依赖 `check` 得到一致快照。`generate_aster_unit_test.rs` 的临时目录用 `Drop` 清理，并用原子计数器避免同一测试进程内命名冲突；这是测试夹具生命周期，不是本 CLI 的运行时机制。

## 与 Go 版本的对应关系

Go 侧最接近的生成器入口是 `pkg/parser/goyacc/main.go`，它同样是命令行程序，负责参数检查、错误输出和调用实际生成流程。但两者不是逐函数移植：Go `goyacc` 接受大量 yacc 兼容 flag、零或一个输入、可读标准输入，并生成 Go parser；Rust `astersql-parsergen` 只接受一个子命令，读取仓库固定的 `.astergram`，生成三份 Rust 静态表或检查其漂移。

`pkg/parser/generate.go` 只包含 `go:generate ./genkeyword`，与本文件的 parser table 命令没有直接等价关系。当前 Rust 维护路径刻意使用已提交静态产物；`pkg/parser/parser_generated_sources_aster_unit_test.rs` 验证 parser crate 直接 `include!` 三份产物、没有 `build.rs` 和 `[build-dependencies]`。因此不能把 Go 的自动/通用 yacc 工作流描述成 Rust 当前行为。

语义对齐主要由产物和解析轨迹验证，而非 CLI 形状对齐：`pkg/parser/parsergen_baseline_aster_unit_test.rs` 将 Rust 生成的 main/hint 表与现有 Rust 基准行为比较；parser action 的独立测试继续验证消费这些表后的语义动作。

## 扩展指南

新增只影响命令分派的子命令时，修改 `HELP` 与 `main` 的 `match`，并增加独立 CLI 集成测试，至少覆盖成功、库错误、缺参、多参、未知命令和 stdout/stderr/退出码；不要把测试内嵌进 `main.rs`。若新增生成产物或改变生成语义，修改点应放在 `generate.rs` 及相应生成/渲染模块，同步 `GENERATED_OUTPUT_NAMES`、`generate_aster_unit_test.rs`、已提交的 `pkg/parser/generated/*`、消费契约测试和基准测试，而不是在 CLI 里复制算法。

移动 crate 或改变目录布局时，必须同步审查 `env!("CARGO_MANIFEST_DIR").parent()` 这一不变量以及测试中同样的根目录推导。若需要可配置根目录，优先增加明确参数并验证路径，而不是依赖当前工作目录。

兼容风险集中在命令名、诊断文本、退出码和固定产物路径，CI 脚本可能依赖它们。正确性风险集中在 `check` 是否保持只读、生成结果是否确定、三个产物是否全部覆盖；性能主要取决于每次从两份文法完整重建分析表，当前入口没有增量或缓存。若要并行或原子更新，应在库层设计临时文件与提交策略，并增加失败注入/并发测试。

## 验证依据

- 源与清单：`pkg/parser/parsergen/main.rs`、`pkg/parser/parsergen/Cargo.toml`、`pkg/parser/parsergen/lib.rs`、根 `Cargo.toml`、`pkg/parser/Cargo.toml`。
- 直接实现：`pkg/parser/parsergen/generate.rs`，重点是 `GENERATED_OUTPUT_NAMES`、`generate_outputs`、`write_generated_outputs`、`check_generated_outputs`、`generate_parser_output`。
- 独立 Rust 测试：`pkg/parser/parsergen/generate_aster_unit_test.rs` 验证确定性、三个输出名、missing/stale 分类、`check` 不写入和已提交产物同步；`pkg/parser/parser_generated_sources_aster_unit_test.rs` 验证静态产物消费；`pkg/parser/parser_no_legacy_dependency_aster_unit_test.rs` 验证维护命令契约；`pkg/parser/parsergen_baseline_aster_unit_test.rs` 验证生成表的解析行为基准。未发现直接启动该二进制并断言 CLI 输出/退出码的独立测试。
- Go 对照：`pkg/parser/goyacc/main.go` 和 `pkg/parser/generate.go`；前者是旧 Go yacc 工具入口，后者仅生成关键字，均不是本文件的一对一实现。
- RustCodeGraph：`status` 显示索引覆盖 7,032 个 Rust 文件；`files --filter pkg/parser/parsergen` 列出本 crate 的 14 个 Rust 文件；`node --file pkg/parser/parsergen/main.rs --offset 1 --limit 200` 返回完整 60 行；`query write_generated_outputs` 与 `query check_generated_outputs` 定位到 `generate.rs:141`、`:162`。精确 `callers`/`callees` 查询未产生可用输出，调用边因此以源码调用点复核。
- 本任务是纯文档分析，按任务约束不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核本说明没有把 Go 工具、库内部生成逻辑或未来设计误写成本文件现状。
