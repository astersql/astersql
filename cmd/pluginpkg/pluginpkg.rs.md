# [`cmd/pluginpkg/pluginpkg.rs`](pluginpkg.rs)

## 文件定位

`cmd/pluginpkg/pluginpkg.rs` 是 `astersql-cmd-pluginpkg` crate 的打包主逻辑，对应 Go 实现 `cmd/pluginpkg/pluginpkg.go`。它不编译 Rust 插件，而是读取插件目录中的 `manifest.toml`，生成临时 `<插件名>.gen.go`，再调用 `go build -buildmode=plugin` 生成 `.so`。进程入口为 `cmd/pluginpkg/bin_main.rs::main`，经 `cmd/pluginpkg/lib.rs::main` 转发到本文件的 `main`；`run_with` 是可注入文件系统、命令执行器和时钟的可测业务入口。

crate 边界由 `cmd/pluginpkg/Cargo.toml` 定义：库入口是 `lib.rs`，二进制入口是 `bin_main.rs`，移植元数据指向 Go 包 `cmd/pluginpkg` 且标记为 `binary`。直接依赖只有 `serde`、`serde_json` 和 `toml`；操作系统、子进程、flag 与时间边界来自同 crate 的 `cmd/pluginpkg/stubs.rs`，避免引入 TiDB 重型 crate。

## 核心职责

1. `main` 初始化并解析 `--pkg-dir`、`--out-dir`、`--pgo-file` 和 `--next-gen`，然后组装 `OsFs`/`ProdRunner`/`SystemClock`。
2. `run_with` 验证必填参数、绝对化路径、读取并解析 manifest，注入 `buildTime`，并校验 manifest 名称与包目录名一致。
3. `CODE_TEMPLATE` 与 `execute_go_template` 把动态 manifest 映射为导出 `PluginManifest` 的 Go 源文件；实现只支持当前固定模板使用的字段访问、`if` 和 `range`。
4. `build_go_flags` 生成可选 PGO、`codes[,nextgen]` tags、plugin build mode、输出路径和包目录参数；`run_with` 以包目录为工作目录、附加 `GO111MODULE=on` 执行 `go`。
5. 成功后打印输出插件路径和 manifest JSON，最后删除临时源文件；致命失败保留该文件，复制 Go `defer` 遇 `os.Exit` 不执行的行为。

## 主要符号

- `pub const CODE_TEMPLATE: &str`：固定 Go 代码模板，保留前导换行，根据 `kind` 生成具体 manifest 类型，选择性填充 `Validate`/`OnInit`/`OnShutdown`/`OnFlush`，并展开 `export` 扩展点。
- `thread_local! { PKG_DIR, OUT_DIR, PGO_FILE, NEXT_GEN }`：复制 Go 包级 flag 变量的当前线程状态。`init_flags` 重置默认值，`set_flags`/`get_flags` 在结构化 `Flags` 与线程局部状态间同步。
- `pub fn usage(log, argv0) -> !`：打印 Go flag 风格的用法与默认项，再通过 `fatal_exit` 终止。
- `pub fn decode_manifest_toml(text) -> Result<Map<...>>` 与 `toml_to_json`：将 TOML 根表递归转为 JSON 动态值；非表根值报 `manifest root must be a table`。
- `pub fn template_truthy(value) -> bool`：对齐 Go `text/template` 条件真值，缺失、null、false、空串、数字零、空数组和空对象均为假。
- `pub fn execute_code_template(manifest) -> Result<String>`：固定模板的公开渲染入口；内部 `execute_go_template`、`find_action_end`、`take_until_end`、`lookup_field`、`format_field` 实现有限 Go 模板语义。
- `assert_manifest_string`：从动态 manifest 中取字符串；缺失或类型不对时以仿 Go interface type assertion 的消息 panic。
- `write_generated_source` 与 `persist_partial_source`：分别写入完整生成物，以及模板失败前尽力保留部分生成物。
- `pub fn build_go_flags(...) -> (String, Vec<String>)`：纯参数组装边界，返回 `.so` 路径与完整 `go build` argv。
- `pub fn run_with(...)`：打包管线的核心编排器，所有外部副作经 `Fs`/`Runner`/`Clock`/`Write` 注入。
- `pub fn encode_manifest_json(...) -> io::Result<()>`：以稳定键序编码 manifest，处理 Go 默认 HTML 转义、`SetIndent(" ", "\t")` 形式和 `Encode` 尾换行。
- `pub fn main()`：生产接线层，从进程环境取参数，处理 flag 错误，然后调用 `run_with`。

## 执行流程

1. `bin_main.rs::main` 调用 `astersql_cmd_pluginpkg::main`，`lib.rs::main` 再调用本文件 `main`。
2. `main` 先用 `init_flags` 清空线程局部 flag，通过 `stubs::args_from_env`/`try_parse_flags` 解析命令行；解析失败则打印消息并进入 `usage`。
3. `run_with` 把 `Flags` 写入再读出线程局部状态。`pkg_dir` 或 `out_dir` 为空时立即显示帮助；三个非空路径通过 `Fs::abs` 归一化。
4. 管线读取 `<pkg_dir>/manifest.toml`，用 `decode_manifest_toml` 生成动态 map，再用 `Clock::now_string` 覆盖/添加 `buildTime`。
5. `assert_manifest_string` 要求 `name` 和 `version` 是字符串；`name` 还必须等于 `Fs::base(pkg_dir)`。
6. 流程先以 `0700` 创建/清空 `<pkg_dir>/<base>.gen.go`，再渲染模板。渲染失败时尽力写回已积累的部分文本；成功时写入完整文本。
7. `build_go_flags` 生成 `go build [-pgo=...] -tags=codes[,nextgen] -buildmode=plugin -o <out>/<name>-<version>.so <pkg_dir>`。`Runner::run` 在 `pkg_dir` 中执行，附加 `GO111MODULE=on`。
8. 编译成功后，标准输出先打印成功提示，后由 `encode_manifest_json` 打印 manifest。JSON 编码失败只记录日志，不把已成功的插件构建改判为失败。
9. 最后尝试删除 `.gen.go`；删除失败只记录人工清理提示。

## 数据与状态

持久输入是 `manifest.toml`，在内存中表示为 `serde_json::Map<String, JsonValue>`。该 map 既是 Go 模板的数据源，也是成功输出的 JSON 数据源；`buildTime` 在运行时写入，因此会同时出现于生成 Go 代码和最终 manifest 输出中。源 TOML 没有静态 schema，必需字段主要由模板访问与 `name`/`version` 类型断言间接约束。

进程参数的规范表示是 `stubs::Flags`，本文件另用四个 thread-local cell 模拟 Go 包变量。它们不在线程间共享；`run_with` 每次会用传入值覆盖当前线程状态，而生产 `main` 会先重置。

文件系统上的中间状态是位于插件源目录的 `.gen.go`。它在调用 Go 工具链前创建，正常返回时删除，致命失败时保留作为诊断产物。输出 `.so` 名称由 `<name>-<version>.so` 确定；本文件不保存输出文件句柄，所有权属于 `go build`。

## 依赖与调用关系

上游调用链是 `cmd/pluginpkg/bin_main.rs::main -> cmd/pluginpkg/lib.rs::main -> pluginpkg.rs::main -> run_with`。测试则直接调用 `run_with`、`execute_code_template`、`decode_manifest_toml`、`build_go_flags`、`template_truthy` 和 `encode_manifest_json`，用于分离验证编排与纯转换逻辑。

下游边界为：

- `toml::from_str -> toml_to_json`：解码并转换 manifest。
- `Fs::{abs,join,base,read_to_string,write,remove}`：路径、manifest 读取、临时文件写入与清理。生产实现是 `stubs.rs::OsFs`。
- `Clock::now_string`：产生 `buildTime`；生产实现是 `SystemClock`，测试用 `FixedClock`。
- `Runner::run`：启动 Go 工具链；生产实现 `ProdRunner` 继承 stdout/stderr 和进程环境，测试用 `ScriptedRunner` 记录调用。
- `serde_json::Serializer`：生成最终 manifest JSON；`BTreeMap` 先稳定键序，再重写缩进和 HTML 字符。
- 外部命令 `go build`：真正完成 plugin 编译。这是运行时依赖，不是 Cargo 依赖。

RustCodeGraph 的文件节点显示源文件共 838 行，并将上述公开符号定位到本文件；通用名称 `run_with` 有多个全库同名符号，因此本文档以文件限定符号和模块入口源码确定调用链，不把同名搜索结果当作调用者。

## 错误处理与边界

错误分为三类。第一类是用法/路径/读取/解析/生成/编译等致命错误：先向 `log` 写入 Go 对应文案，再调用 `fatal_exit`。`fatal_exit` 在生产构建中是进程退出码 1，在 `cfg(test)` 中是可被 `catch_unwind` 捕获的 panic。flag 语法错误由 `try_parse_flags` 返回消息后进入 `usage`。

第二类是故意对齐 Go 非受控 panic 的动态类型错误：`name` 或 `version` 不是字符串时，`assert_manifest_string` 模拟 `manifest[...].(string)` 的 interface conversion panic，不会改写为名称不匹配等受控日志。

第三类是非致命收尾错误：manifest JSON 编码失败和成功路径的临时文件删除失败只记录日志。此时插件构建已经成功，不再回滚 `.so`。

模板边界必须特别注意：它不是完整 Go `text/template` 实现。未支持的 action、未闭合 action/控制块、对字符串等不可迭代值执行 `range`、以及 range 项不能提供字段都会返回错误。range 内的 `.` 只绑定当前项，缺失字段产生 `<no value>` 而不回退到根 manifest。

## 并发与资源生命周期

本文件本身不创建线程、异步任务、channel、锁或事务。`run_with` 是同步管线，在 `Runner::run` 返回前一直阻塞；`ProdRunner` 启动的 Go 子进程继承当前进程的 stdout/stderr 和除附加项外的环境。

flag 状态是 thread-local 而非全局锁保护状态，因此不会在并发线程间传播。可测边界使用 `&dyn Fs`/`&dyn Runner`/`&dyn Clock` 和可变 `Write` 借用，所有权在调用期内都留在上游。实现没有承诺 `run_with` 可在同一目录并发执行：固定 `.gen.go` 文件名会让同目录的并发打包互相截断或删除中间文件，扩展时不应假定它具备并发安全性。

`.gen.go` 生命周期复制 Go 的 `OpenFile + defer Remove + os.Exit`：先以截断模式创建，再逐步生成；正常返回时在所有输出完成后删除，任一后续致命退出均保留它。`pluginpkg_test.rs::deferred_remove_runs_after_manifest_output` 证明临时文件在 manifest 输出时仍存在；`parity_test.rs::contract_resource_cleanup` 证明构建失败保留、成功删除及删除失败仅记日志。

## 与 Go 版本的对应关系

`CODE_TEMPLATE` 对应 Go `codeTemplate`；thread-local flag 对应 Go 包变量；`init_flags`/`usage`/`main` 对应 Go `init`/`usage`/`main`。`run_with` 是为可测性从 Go `main` 抽出的 Rust 编排器，保留路径绝对化、TOML 动态 map、当前时间、目录名校验、`0700` 临时文件、构建参数、工作目录、`GO111MODULE=on`、成功文本和清理时机。

Rust 版与 Go 版的实现形式差异主要是：

- Go 直接使用 `os`/`exec`/`time`/`flag`；Rust 通过 `Fs`/`Runner`/`Clock`/`Flags` 注入，生产实现放在 `stubs.rs`。
- Go 使用 BurntSushi TOML 解码到 `map[string]any`；Rust 使用 `toml::Value` 再转 `serde_json::Value`。
- Go 使用完整 `text/template`；Rust 只解释固定模板已用的语法子集，因此修改 `CODE_TEMPLATE` 时必须同步评估解释器。
- Go 的 `defer os.Remove` 是语言机制；Rust 显式在成功末尾删除，故意不用无条件 RAII 清理。
- Go map 的 JSON 键序不是本文件依赖的合同；Rust `encode_manifest_json` 为测试可重复性先用 `BTreeMap` 排序，同时仿真 Go HTML 转义、缩进和尾换行。
- Rust 测试以 panic 代表可捕获的 `os.Exit`；非测试构建仍调用 `std::process::exit(1)`。

同路径未发现 Go `*_test.go`；Go 对照依据是 `cmd/pluginpkg/pluginpkg.go`，Rust 回归依据是独立文件 `pluginpkg_test.rs` 和 `parity_test.rs`。

## 扩展指南

- 新增 CLI 参数时，同步更新 `stubs.rs::Flags`、`try_parse_flags`、本文件 thread-local 状态、`init_flags`/`set_flags`/`get_flags`/`usage`、生产 `main` 接线及 `parity_test.rs` 的 flag 语法测试。参数名、默认值、解析停止位置与布尔字面量都属于 Go 兼容面。
- 新增 manifest 字段或回调时，先修改 Go `codeTemplate` 与 Rust `CODE_TEMPLATE`，再确认 `execute_go_template` 已支持所用语法，并在 `parity_test.rs::contract_boundary_flags_and_template` 添加缺失、空值和正常值回归。不应把完整 Go 模板语法视为已支持。
- 改变 `go build` 参数、tags、PGO 或产物命名时，优先改 `build_go_flags`，然后更新 `parity_test.rs::contract_normal_packaging` 和 `contract_boundary_flags_and_template`，按完整 argv 顺序断言。
- 改变错误处理或清理时，必须同时考察 `pluginpkg.go` 的 `os.Exit`/`defer` 相互作用，并更新 `pluginpkg_test.rs` 和 `parity_test.rs::contract_error_paths`/`contract_resource_cleanup`。无条件 scope guard 会改变失败后保留 `.gen.go` 的可观测行为。
- 改变 JSON 输出时，要同时验证键序、Go HTML 转义、缩进前缀和尾换行；相关独立测试为 `manifest_json_uses_go_default_html_escaping` 以及 `contract_resource_cleanup` 尾部的 JSON 断言。
- 如需支持并发打包同一插件目录，必须先解决固定 `.gen.go` 文件名的冲突，并评估这是否仍符合 Go 工具对外契约；当前实现不提供这项保证。
- Rust 测试必须继续放在独立 `pluginpkg_test.rs`/`parity_test.rs` 中，不内嵌到生产文件。若改动模板或打包契约，同时与 `pluginpkg.go` 对照，不为了使测试通过而简化 Go 语义。

## 验证依据

- RustCodeGraph `status`：项目索引可用，包含 7,032 个 Rust 文件；`files --filter cmd/pluginpkg` 列出 `pluginpkg.rs`、`lib.rs`、`bin_main.rs`、`stubs.rs`、`pluginpkg_test.rs`、`parity_test.rs` 和 Go 对照文件。
- RustCodeGraph `node --file cmd/pluginpkg/pluginpkg.rs --offset 1 --limit 400` 与 `--offset 401 --limit 500`：读取 838 行源文件全貌，核对常量、thread-local 状态、公开/私有函数及主管线。
- RustCodeGraph 精确 `query`：确认 `run_with` 在本文件第 543 行、`build_go_flags` 在第 506 行、`decode_manifest_toml` 在第 173 行、`execute_code_template` 在第 230 行、`encode_manifest_json` 在第 739 行。对文件限定符号的 `callers`/`callees` 查询未输出边，因此调用链另由已索引的入口和测试源码直接核对，没有推测图边。
- 入口与 crate：`cmd/pluginpkg/Cargo.toml`、`cmd/pluginpkg/lib.rs`、`cmd/pluginpkg/bin_main.rs`；前者验证包名、lib/bin 路径、Go 包映射和 `serde`/`serde_json`/`toml` 依赖，后两者验证生产入口转发链。
- Go 对照：`cmd/pluginpkg/pluginpkg.go`全文，核对 flag、模板、TOML、名称校验、生成文件、`go build`、输出与 `defer` 清理语义。同目录未发现 Go 测试文件。
- 边界实现：RustCodeGraph 读取 `cmd/pluginpkg/stubs.rs`全文，核对 `Flags`、`fatal_exit`、`Fs`、`Clock`、`Runner`、`OsFs`、`SystemClock` 和 `ProdRunner` 的生产/测试语义。
- 独立 Rust 测试：`cmd/pluginpkg/pluginpkg_test.rs`全文及 `cmd/pluginpkg/parity_test.rs`全文。前者验证 range 字段作用域、模板失败的部分文件、成功输出后清理和非字符串名称 panic；后者验证完整成功路径、flags/template 边界、错误分支、临时文件资源契约、数字 version、Go flag 语法、生产 runner 退出文本、系统时钟、缺失模板字段和 HTML 转义。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用任务指定的 11 章结构检查，不以编译或测试代替文档事实复核。
