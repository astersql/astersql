# `tools/tazel/ast.rs`

## 文件定位

`tools/tazel/ast.rs` 属于 `astersql-tools-tazel` crate。该 crate 在 `tools/tazel/Cargo.toml` 中声明为同时提供库和 `astersql-tools-tazel` 二进制的工具 crate，且没有 Rust 外部依赖；`tools/tazel/lib.rs` 以 `pub mod ast` 暴露本文件。它不是 SQL 运行时的一部分，而是 `tazel` 修改 Bazel `go_test` 规则之前的预扫描阶段：统计每个目录内符合 Go 测试入口命名规则的顶层函数数，供 `tools/tazel/main.rs::patch_go_test_file` 决定 `shard_count`。

完整入口链是 `tools/tazel/lib.rs::entry` → `tools/tazel/main.rs::main` → `run_from`。`run_from` 先调用本文件的 `initCount` 和 `walk_from`，再遍历 `BUILD.bazel`；因此计数表在 BUILD 规则补丁发生前已准备好。RustCodeGraph 将本文件识别为 21 个符号，并标出直接使用它的 `tools/tazel/ast_test.rs` 与 `tools/tazel/parity_test.rs`。

## 核心职责

本文件承担三个紧密相关的职责：

1. 用进程级共享表 `testMap` 保存“规范化绝对目录 → 顶层测试函数数量”。
2. 递归查找文件名以 `_test.go` 结尾的 Go 文件，并逐文件扫描。
3. 在计数前维持 Go 版 `parser.ParseFile(..., parser.AllErrors)` 的失败契约：先由 `validate_go_syntax` 调用 `gofmt -e` 拒绝语法错误，再由 `go_tokens` 排除注释和字面量，识别 `func <identifier>` 形式的声明。

计数口径由 `scan` 明确限定为：函数名以 `Test` 开头、不是 `TestMain`，且 `func` 后不是 `(`，所以带接收者的方法不计数。每次命中都调用 `addTestMap`，按该源文件规范化绝对路径的父目录累加一次。该结果最终由 `main.rs::patch_go_test_file` 读取：数量大于 1 时生成分片数，数量等于 1 时删除旧 `shard_count`，没有目录记录时不改该属性。

## 主要符号

- `pub static testMap: OnceLock<Mutex<HashMap<String, u32>>>`：Go 包级 `testMap` 的 Rust 对应物。`OnceLock` 负责一次性建立容器，`Mutex` 负责串行化清空、累加和读取；键是目录字符串，值是命中的测试函数数。
- `fn test_map() -> &'static Mutex<HashMap<String, u32>>`：隐藏延迟初始化细节，所有共享状态操作都经它取得同一个表。
- `pub fn initCount()`：锁住并清空表。它对应 Go `initCount` 的“开始新一轮统计”语义，而不是重新分配并替换 Rust 静态对象。
- `pub fn addTestMap(path: &str)`：把目录键的计数加一，不存在时从零创建。公开主要是为了与 Go 迁移接口和独立测试保持一致。
- `pub fn test_count_for(dir: &str) -> Option<u32>`：返回计数快照；它不泄露锁守卫，也不允许调用方直接修改全局表。
- `pub fn walk()` 与 `pub fn walk_from(root: &Path)`：分别从当前目录和指定根目录启动扫描。`walk_from` 把内部 `io::Result` 错误转成 panic，错误文本以 `fail to walk` 开头。
- `fn walk_dir(root: &Path) -> io::Result<()>`：递归读取目录，仅把 `_test.go` 文件交给 `scan`；其他普通文件被忽略。
- `pub fn scan(path: &str) -> io::Result<()>`：单文件入口。它规范化路径、读取 UTF-8 文本、验证 Go 语法、提取 token，并按上述规则累加父目录。
- `fn validate_go_syntax(path: &Path) -> io::Result<()>`：执行 `gofmt -e <path>`。非零退出状态转成 `io::ErrorKind::InvalidData`，错误消息取自标准错误；无法启动进程则保留底层 I/O 错误。
- `fn go_tokens(source: &str) -> io::Result<Vec<String>>`：轻量 Go 词法扫描器。它跳过行注释、块注释、双引号字符串、单引号 rune 字面量和反引号原始字符串，收集标识符与六种括号 token，同时检查字面量闭合和括号配对。
- `is_ident_start`、`is_ident_continue`：定义词法扫描使用的标识符字节范围；ASCII 字母、下划线及非 ASCII 字节可开头，后续还允许数字。
- `invalid_go<T>`：统一构造带字节偏移的 `InvalidData` 错误，供本地词法检查报告未闭合或不平衡输入。

本文件没有 trait、结构体、枚举、`impl` 或条件编译项；公开面集中在共享计数、扫描入口和只读查询上，其余符号均为内部实现。

## 执行流程

1. `main.rs::run_from(root)` 调用 `initCount`，清除同一进程中上一轮扫描留下的目录计数。
2. `run_from` 调用 `walk_from(root)`；兼容入口 `walk()` 则固定传入 `Path::new(".")`。
3. `walk_dir` 深度优先读取目录。目录递归下钻，非 `_test.go` 文件跳过，目标文件进入 `scan`。任一目录枚举、元数据读取或扫描错误通过 `?` 向上传播，最后由 `walk_from` panic。
4. `scan` 先用 `fs::canonicalize` 得到绝对路径，再以 `fs::read_to_string` 读取内容。路径不存在、无法访问或内容不是 UTF-8 都直接返回 I/O 错误。
5. `validate_go_syntax` 启动 `gofmt -e`。只有进程成功退出才继续；这一步使后续轻量 token 识别只处理 Go 解析器认可的源文件。
6. `go_tokens` 单次线性扫描源字节。注释和字面量内容不会进入 token；括号入栈、闭括号必须和栈顶匹配。未终止块注释/字面量或括号失配返回 `InvalidData`。
7. `scan` 遍历相邻 token 窗口。遇到 `func` 后紧跟非 `(`、以 `Test` 开头且不等于 `TestMain` 的标识符时，取规范化文件的父目录并调用 `addTestMap`。
8. 后续 `main.rs::patch_go_test_file` 规范化 `BUILD.bazel` 路径并用其父目录调用 `test_count_for`，把同目录计数转为 BUILD 规则的分片决策。

## 数据与状态

唯一持久到进程生命周期结束的模块状态是 `testMap`。它的键不是仓库相对路径，而是 `scan` 对 Go 文件执行 `canonicalize` 后得到的绝对父目录字符串；`main.rs::patch_go_test_file` 同样规范化 BUILD 文件再取父目录，二者必须维持相同规则，否则查不到计数。值为 `u32`，每个符合条件的函数加一；实现没有显式处理极端溢出，调试构建会在溢出时 panic，发布构建行为取决于 workspace 的溢出检查配置。

`test_count_for` 复制 `u32` 后立即释放锁，调用方不会持有内部引用。`go_tokens` 为每个源文件建立一个 `Vec<String>`，其中只保存标识符和括号 token；源文件文本、token 向量、目录项和 `Command::Output` 都是单次调用的局部资源。目录递归依赖调用栈，没有显式深度限制或已访问目录集合。

`initCount` 的不变量是“新一轮 walk 前表为空”。`run_from` 遵守这一顺序；若其他调用方绕过 `initCount` 直接扫描，多轮结果会累加。`scan` 在完整语法验证和 token 提取成功后才开始累加，因此一个文件的语法/词法失败不会留下该文件的部分计数。

## 依赖与调用关系

上游直接关系如下：

- `tools/tazel/main.rs::run_from` 调用 `initCount` 和 `walk_from`，建立整仓计数。
- `tools/tazel/main.rs::patch_go_test_file` 调用 `test_count_for`，消费目录计数并设置或删除 `shard_count`。
- `tools/tazel/ast_test.rs` 直接调用 `initCount`、`scan` 验证非法 Go 语法被拒绝。
- `tools/tazel/parity_test.rs` 调用 `initCount`、`addTestMap`、`scan`、`test_count_for`，并经 `run_from` 验证统计到 BUILD 写回的集成契约。

本文件的 Rust 下游仅使用标准库：`std::fs` 负责规范化、读文件和枚举目录，`std::path` 负责路径操作，`HashMap` 保存计数，`OnceLock`/`Mutex` 管理全局状态，`std::process::Command` 启动外部程序。RustCodeGraph 的精确文件节点给出 `walk_from` → `walk_dir` 调用边，并把两个独立测试文件列为本文件使用者。Cargo 清单的空 `[dependencies]` 也验证了这里没有 crate 级第三方库。

运行时仍有一个 Cargo 未表达的外部依赖：`validate_go_syntax` 要求 PATH 中存在可执行的 `gofmt`。其语法判定来自 Go 工具链，而不是 Rust crate。

## 错误处理与边界

`scan` 保留可恢复的 `io::Result<()>` 接口。路径规范化失败、文件读取失败、非 UTF-8 内容、`gofmt` 启动失败、Go 语法错误以及本地词法边界错误都会阻止该文件计数。`gofmt` 的语法失败被归类为 `InvalidData`；本地 `invalid_go` 也使用相同错误种类并附带字节偏移。

`walk_dir` 用 `?` 传播第一个错误，不会跳过坏文件继续扫描。公共 `walk_from` 和 `walk` 则把该错误升级为 panic，这与工具入口的失败即终止策略一致，但调用库 API 时应注意它们不是返回 `Result` 的可恢复接口。共享表方面，`initCount` 和 `addTestMap` 在锁中毒时通过 `expect("testMap lock")` panic；`test_count_for` 则把加锁失败静默映射成 `None`，这两类接口的锁错误策略并不完全对称。

词法边界由测试和实现共同限定：注释/字符串中的 `func Test...` 不计数；`TestMain` 不计数；`func (receiver) Test...` 因 `func` 后 token 是 `(` 而不计数；多个合法顶层 `Test*` 声明分别计数。文件名过滤只认精确后缀 `_test.go`。扫描器本身不是完整 Go parser，但前置 `gofmt -e` 先验证完整语法，避免仅凭平衡括号接受非法函数体。

## 并发与资源生命周期

`testMap` 可被多个线程安全访问，单次清空、递增和读取都在 `Mutex` 临界区内完成；锁守卫不会跨函数返回。但“清空后完成整轮递归扫描”不是一个原子事务：若多个线程同时启动扫描，另一个线程执行 `initCount` 可以清掉正在累积的结果。因此当前线程安全只保证内存访问安全，不保证并行多轮扫描的业务隔离；现有 `run_from` 是同步串行流程。

目录和文件没有长期打开的句柄：`read_dir` 迭代器、文件内容、子进程输出和锁守卫均按 Rust 作用域释放。每个 Go 测试文件都会同步启动一次 `gofmt` 子进程并等待完成，扫描期间没有异步任务、线程池或通道。大仓库下主要资源成本是递归目录遍历、逐文件读取、token 向量分配以及每文件一次进程创建；修改扫描策略时不能忽略这些成本。

`tools/tazel/parity_test.rs::contract_resource_cleanup` 验证两项生命周期行为：再次调用 `initCount` 后旧键消失；完整 `run_from` 返回后临时工作区可删除，说明流程没有保留阻止清理的文件句柄。

## 与 Go 版本的对应关系

直接对照文件是 `tools/tazel/ast.go`，入口消费方是 `tools/tazel/main.go`。

- Go `testMap map[string]uint32` 对应 Rust `OnceLock<Mutex<HashMap<String, u32>>>`；Go 通过重新分配 map 初始化，Rust 通过清空已初始化的单例保持重复运行能力。
- Go `filepath.Walk(".", ...)` 对应 Rust `walk()`；Rust 额外提供 `walk_from`，让测试和 `run_from` 可注入根目录。
- Go `filepath.Abs` 对应 Rust `fs::canonicalize`。两者都为目录计数提供绝对路径键，但 `canonicalize` 还要求目标存在并解析路径组件。
- Go `parser.ParseFile(..., parser.AllErrors)` 直接产生 AST 并遍历 `f.Decls`；Rust 先用同属 Go 工具链的 `gofmt -e` 完成语法验证，再用 `go_tokens` 抽取声明所需 token。Rust 因此不复制完整 Go AST，但保留合法语法、忽略注释/字面量、排除方法和 `TestMain` 的统计结果。
- Go 的 `walk` 在失败时 `log.Fatal`，Rust `walk_from` panic；二者都在工具主流程中终止执行，但错误呈现和库调用可恢复性不同。
- Rust 新增 `test_count_for` 作为受控读取接口；Go `main.go` 直接访问同包变量。Rust `main.rs` 用该接口维持模块边界。

现有对齐证据集中在 `tools/tazel/parity_test.rs`：`contract_normal_paths` 断言两个顶层测试被计为 2，而 `TestMain` 和接收者方法被排除；`scan_uses_go_syntax_instead_of_matching_comment_text` 断言注释中的伪声明不计数；`contract_boundary` 和 `contract_resource_cleanup` 分别覆盖单测试计数与全局表重置。`tools/tazel/ast_test.rs::scan_rejects_go_syntax_errors` 进一步锁定非法函数体必须失败的 Go parser 契约。

## 扩展指南

若要改变测试函数识别规则，首要修改点是 `scan` 的相邻 token 条件；若需要识别更多 Go 语法上下文，则应评估扩展 `go_tokens` 是否仍能保持与 `go/ast` 的等价结果，而不是绕过 `validate_go_syntax`。任何口径变化都应同步扩展独立的 `tools/tazel/ast_test.rs` 或 `tools/tazel/parity_test.rs`，至少覆盖正常声明、`TestMain`、接收者方法、注释/字符串、非法语法和新增边界。Rust 单元测试不应内嵌回本源文件。

若要改变目录键或路径规范化，必须同时检查 `scan` 与 `main.rs::patch_go_test_file`，确保 Go 文件和 `BUILD.bazel` 仍生成完全相同的父目录键；并在临时目录中做端到端 `run_from` 验证。若要让扫描错误可恢复，应从 `walk_from` 的签名和 `main.rs::run_from` 的错误映射一起设计，避免只在内部吞错后生成不完整分片信息。

并行化扫描前必须把“一轮统计”的状态从全局单例隔离出来，或至少为清空至消费的完整阶段提供事务边界；仅依赖现有 `Mutex` 不足以防止不同扫描轮次互相清空。性能优化可优先评估减少每文件 `gofmt` 进程开销和避免保存全部 token，但必须保留 Go 完整语法错误契约及注释/字面量过滤。引入 Rust Go-parser 依赖还会改变当前空依赖的 Cargo 边界，需要单独审查可复现性和 Go/Rust 语义一致性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标目录可见 `ast.rs` 的 21 个符号；`files --filter tools/tazel` 列出 Go/Rust 对照和测试；`node --file tools/tazel/ast.rs --offset 1 --limit 260` 返回本文件完整 243 行并标记使用者 `ast_test.rs`、`parity_test.rs`；`query` 定位 `initCount`、`addTestMap`、`walk_from`、`go_tokens`、`validate_go_syntax`；`callees walk_from` 给出 `walk_from` → `walk_dir`。对通用名 `scan` 的全局图查询存在重名噪声，因此其精确上下游以目标文件节点和入口源码交叉验证。
- 生产源码：`tools/tazel/ast.rs`（目标实现）、`tools/tazel/lib.rs`（模块公开与 crate 入口）、`tools/tazel/main.rs`（`run_from` 上游和 `patch_go_test_file` 消费点）。
- crate 边界：`tools/tazel/Cargo.toml`，确认库/二进制入口、`go-package = "tools/tazel"`、`kind = "binary"` 及无 Rust 外部依赖。
- Go 对照：`tools/tazel/ast.go`（原统计算法）和 `tools/tazel/main.go`（计数消费及分片上限流程）。
- 独立 Rust 测试：`tools/tazel/ast_test.rs`（非法 Go 语法）与 `tools/tazel/parity_test.rs`（正常、边界、错误、资源清理和完整入口对齐）。同目录没有单独的 Go `ast_test.go`；Go 语义依据来自生产实现，Rust 回归依据来自上述独立测试。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前另运行任务规定的 11 节结构检查，并人工检查只新增本文档、不修改生产源码、Cargo、Go 文件或只读 `plan.md`。
