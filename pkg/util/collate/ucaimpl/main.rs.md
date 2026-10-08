# `pkg/util/collate/ucaimpl/main.rs`

## 文件定位

该文件是 `astersql-util-collate-ucaimpl` crate 的 Unicode CI Go 源码生成器实现，不是 SQL 请求运行时的排序规则实现。`pkg/util/collate/ucaimpl/lib.rs` 通过 `#[path = "main.rs"] pub mod generator` 纳入它，并将其公开项重导出；根 `Cargo.toml` 又将该 crate 列为 workspace 成员和 `facade_util_collate_ucaimpl` 路径依赖，`pkg/lib.rs::util::collate::ucaimpl` 最终对外重导出它。

`pkg/util/collate/ucaimpl/Cargo.toml` 只声明了 `[lib] path = "lib.rs"`，没有 Rust `[[bin]]` 目标；因此文件中名为 `main` 的函数是库内公开 API，不会因名称自动成为 Cargo 可执行文件入口。仓库中现行 Go 生成链仍由 `pkg/util/collate/unicode_0400_ci_impl.go` 和 `unicode_0900_ai_ci_impl.go` 的 `//go:generate go run ./ucaimpl/main.go -- ...` 指令驱动。

## 核心职责

- 用 `include_str!("unicode_ci.go.tpl")` 在编译期嵌入 Go 模板，使生成时不再依赖运行目录中的模板文件。
- 用 `Data` 提供校对器类型名和底层实现名，并由 `render_unicode_ci_template` 替换模板中仅有的 `{{.Name}}` 与 `{{.ImplName}}` 占位符。
- 由 `generate_file` 将完整渲染结果覆盖写到调用者指定的路径，返回 `io::Result<()>` 供库调用者处理 I/O 失败。
- 由 `main` 将两个受支持的 Go 目标文件名映射到固定的类型/实现名组合，并把非法目标或写入失败转为 panic。

模板产物展开 `Clone`、`Compare`、`Key`、`ImmutableKey`、`KeyWithoutTrimRightSpace`、`Pattern` 和 `MaxKeyLen` 等 Go `Collator` 方法，目的是让 `GetWeight`/`Preprocess` 调用可内联；这些排序算法存在于 `unicode_ci.go.tpl` 及生成文件，而非 Rust 生成器自身执行的业务逻辑。

## 主要符号

- `UNICODE_CI_IMPL: &str`：私有编译期常量，内容来自同目录 `unicode_ci.go.tpl`。模板变更会改变 crate 编译产物中的该字符串。
- `pub struct Data { pub name: String, pub impl_name: String }`：渲染参数。`name` 填入 Go 校对器类型名，`impl_name` 填入持有的 UCA 实现类型名。字段是公开的，不做标识符合法性校验。
- `pub fn generate_file(filename: impl AsRef<Path>, data: &Data) -> io::Result<()>`：可复用的生成 API。先在内存中渲染整份文本，再调用 `std::fs::write`。泛型路径参数允许 `&Path`、`PathBuf`、`&str` 等输入。
- `fn render_unicode_ci_template(template: &str, data: &Data) -> String`：私有纯字符串渲染函数，按 `Name` 后 `ImplName` 的顺序执行两次全局字面替换。
- `pub fn main()`：库内命令行风格的分派函数。它读取 `std::env::args()` 的最后一项，仅接受 `unicode_0400_ci_generated.go` 和 `unicode_0900_ai_ci_generated.go`。

本文件没有 trait、`impl` 块或条件编译项。`Data`、`generate_file` 和 `main` 通过 `lib.rs` 的 glob re-export 成为 crate 公开面；常量和渲染函数保持模块私有。

## 执行流程

1. `main` 收集完整参数列表，然后只查看 `args.last()`。因为正常进程参数至少包含程序名，无显式生成目标时会落入默认 panic 分支。
2. 若最后一项为 `unicode_0400_ci_generated.go`，则构造 `Data { name: "unicodeCICollator", impl_name: "unicode0400Impl" }`；若为 `unicode_0900_ai_ci_generated.go`，则构造 `Data { name: "unicode0900AICICollator", impl_name: "unicode0900Impl" }`。
3. 两个分支都将固定的当前目录相对文件名传给 `generate_file`；用于选择分支的命令行路径不会成为输出路径。
4. `generate_file` 调用 `render_unicode_ci_template(UNICODE_CI_IMPL, data)`，产生拥有所有占位符替换结果的新 `String`。
5. `std::fs::write` 创建或截断目标文件并写入全部字节。`generate_file` 将其 `io::Result` 原样返回；`main` 则用 `expect` 要求写入必须成功。

RustCodeGraph 识别的文件内调用链是 `main -> generate_file -> render_unicode_ci_template`。独立测试还直接调用 `generate_file`，以避免修改进程参数和工作目录。

## 数据与状态

`Data` 是唯一显式数据结构，两个字段均为拥有所有权的 `String`。渲染过程只借用 `&Data`，不修改它。`UNICODE_CI_IMPL` 是静态只读文本，不存在懒初始化、全局可变状态或缓存。

中间状态是 `render_unicode_ci_template` 创建的完整 `String`：第一次 `replace` 生成一份文本，第二次再生成最终文本。因此时间和峰值内存随模板大小线性增长，并可能同时存在中间与最终分配。唯一持久化状态是目标文件；已有文件会被截断后重写，这一语义由 `generating_again_truncates_the_existing_file_like_os_create` 测试明确覆盖。

## 依赖与调用关系

- 上游装配：`lib.rs` 将文件映射为 `generator` 模块并重导出。根 workspace 用 `facade_util_collate_ucaimpl` 引入 crate，`pkg/lib.rs` 再在 `util::collate::ucaimpl` 命名空间重导出。
- 直接 Rust 调用者：RustCodeGraph 给出 `main -> generate_file` 和 `generate_file -> render_unicode_ci_template`；`migration_aster_unit_test.rs::{assert_generated_file, generating_again_truncates_the_existing_file_like_os_create}` 也直接调用 `generate_file`。仓库全局搜索未发现其他 Rust 生产调用点。
- 下游标准库：`std::env::args` 提供命令行状态，`AsRef<Path>` 定义路径边界，`String::replace` 渲染文本，`std::fs::write` 完成文件 I/O，`std::io::Result` 表达写入失败。Cargo manifest 没有声明任何第三方依赖或 feature。
- 输入与产物：模板是 `ucaimpl/unicode_ci.go.tpl`；两个 Go 参考产物是上一级目录的 `unicode_0400_ci_generated.go` 和 `unicode_0900_ai_ci_generated.go`。它们与 Rust 运行时用的 `*_generated.rs` 是不同产物；本文件的 Go 模板不生成 `.rs` 文件。
- 业务链位置：生成的 Go 类型委托 `unicode0400Impl` 或 `unicode0900Impl` 做预处理、权重查询和通配符构造。生成器因而是校对源码维护链的工具边界，不在 SQL 比较/构键的运行时调用链上。

## 错误处理与边界

`generate_file` 只有文件写入阶段是可失败操作，并将 `std::fs::write` 的 I/O 错误直接返回；典型失败包括父目录不存在、权限不足、目标是目录或存储失败。它不创建父目录，不加临时文件，也不执行原子 rename；如果写入中途失败，已有文件可能已被截断或留下部分内容。

`render_unicode_ci_template` 是字面替换而不是 Go `text/template` 解析器：它不报告未知或遗留占位符，不验证 `Data` 字段是否为有效 Go 标识符，也不对插入文本做转义。当前模板只使用两个简单占位符，且测试通过与 Go 产物全文比较约束这一假设。

`main` 的命令行边界故意严格：不支持的最后一项（包括无显式目标时的程序名）触发 `panic!("unreachable")`；写入失败触发带 `write generated unicode collator implementation` 上下文的 panic。由于只比较参数文本，传入带目录前缀的同名路径也不会命中；命中后仍始终写入当前工作目录。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或长存文件句柄。模板字符串在程序整个生命期内静态存在；`Data`、参数向量和渲染 `String` 都是调用栈所属值，在函数返回后自动释放。`std::fs::write` 内部打开、写入并在返回前关闭文件，此处不持有跨调用资源。

并发调用 `generate_file` 在不同路径上没有模块内共享状态；在同一路径上则没有互斥、锁文件或原子替换保护，结果取决于竞争时序，且可能出现截断/部分写入。迁移测试的 `temporary_output` 用时间纳秒后缀降低并行测试的路径冲突，但这是测试辅助策略，不是生产 API 的并发保证。

## 与 Go 版本的对应关系

Rust `UNICODE_CI_IMPL` 对应 Go 的 `//go:embed unicode_ci.go.tpl` 与 `unicodeCIImpl`；Rust `Data { name, impl_name }` 对应 Go 私有 `data { Name, ImplName }`；Rust `generate_file` 对应 Go `generateFile`；两个 `main` 都按最后一个命令行参数选择相同的两组名称，对其他输入使用 `"unreachable"` panic。

主要语义差异如下：

- Go `generateFile` 使用 `text/template.Parse` 和 `Execute`，解析或执行错误均 panic；Rust 针对当前两个占位符做两次字面替换，不存在可返回的模板解析错误。如果模板未来加入条件、循环、管道或其他 `text/template` 语法，Rust 实现不会自动等价。
- Go `os.Create` 后通过打开的 `*os.File` 执行模板，而且原实现没有显式 `Close`；Rust 先构造整份输出，再用 `std::fs::write` 创建/截断、写入和关闭。对成功产物和覆盖旧文件的可观察结果一致，但失败时机和资源管理路径不同。
- Go `generateFile` 自身无返回值并将所有错误 panic；Rust `generate_file` 保留 `io::Result` 给库调用者，只有 Rust `main` 将其转为 panic。

`migration_aster_unit_test.rs` 对 4.0.0 和 9.0.0 两组参数分别生成临时文件，并与仓库 Go `*_generated.go` 产物全文相等比较；另一个测试验证重复生成会截断旧内容。它们证明当前模板与固定参数的成功路径对齐，但没有直接调用 Rust `main`，也没有覆盖非法参数、I/O 失败或并发同路径写入。

## 扩展指南

- 新增一种同模板 Go 校对器时，在 `main` 的文件名匹配中增加目标与 `Data` 映射，同步 Go `main.go`、触发生成的 `//go:generate` 指令、生成产物以及 `migration_aster_unit_test.rs` 中的全文对齐测试。测试必须继续放在独立文件，不要内嵌到 `main.rs`。
- 修改 `unicode_ci.go.tpl` 时，同时检查两份 Go 生成文件、对应 Rust `unicode_*_generated.rs` 中是否需要同步语义，以及运行时 `unicode_0400_ci_impl.rs`/`unicode_0900_ai_ci_impl.rs` 提供的方法签名。本生成器只写 Go，不能假设 Rust 产物会自动更新。
- 引入新模板语法前，先决定是扩展 `render_unicode_ci_template` 还是引入真正的模板引擎，并增加占位符遗留、多次出现、特殊字符与非法数据测试。这是与 Go `text/template` 语义偏离风险最高的扩展点。
- 如果要让 Rust `main` 成为真正命令行工具，需在 Cargo manifest 中增加明确 bin 目标或单独的 `bin.rs`，并将输出路径、退出码、用法信息和错误链作为可观察契约测试；不应仅依赖当前库函数的名称。
- 若需要可恢复或并发安全的生成，可在目标同目录创建临时文件，完整写入并刷盘后原子替换，同时为同路径竞争和中途失败增加独立回归测试。这会改变失败语义，需与 Go 工具的预期一并评估。

## 验证依据

- 源文件：`pkg/util/collate/ucaimpl/main.rs`，核对了常量 `UNICODE_CI_IMPL`、结构体 `Data`、`generate_file`、`render_unicode_ci_template` 和 `main` 的完整定义。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/util/collate/ucaimpl` 列出已索引的 `lib.rs`、`main.rs`、`main.go` 和迁移测试；`node --file .../main.rs` 确认文件的 5 个符号；`node ...::main`、`node ...::generate_file` 和 `node ...::render_unicode_ci_template` 确认 `main -> generate_file -> render_unicode_ci_template` 调用边。单独 `callers` 查询在 30 秒内无输出，所以外部调用面另用全仓 `rg` 核验。
- crate 与装配：`pkg/util/collate/ucaimpl/Cargo.toml`、`pkg/util/collate/ucaimpl/lib.rs`、根 `Cargo.toml` 的 workspace 成员与 `facade_util_collate_ucaimpl` 声明，以及 `pkg/lib.rs::util::collate::ucaimpl`。
- Go 对照与生成边：`pkg/util/collate/ucaimpl/main.go`、`unicode_ci.go.tpl`、`pkg/util/collate/unicode_0400_ci_impl.go`、`unicode_0900_ai_ci_impl.go`、`unicode_0400_ci_generated.go` 和 `unicode_0900_ai_ci_generated.go`。`ucaimpl/BUILD.bazel` 另证明 Go 侧将 `main.go` 与嵌入模板组装为可执行目标。
- Rust 运行时边界：`pkg/util/collate/lib.rs`、`unicode_0400_ci_impl.rs`、`unicode_0900_ai_ci_impl.rs`、`unicode_0400_ci_generated.rs` 和 `unicode_0900_ai_ci_generated.rs`，用于区分 Go 生成工具与 Rust SQL 运行时校对实现。
- 独立测试：`pkg/util/collate/ucaimpl/migration_aster_unit_test.rs`，覆盖两份参考产物的全文相等性和覆盖写截断语义。本任务是纯文档分析，按计划不运行 Cargo；没有将测试执行结果当作本文档的新验证证据。
