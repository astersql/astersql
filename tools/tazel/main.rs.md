# `tools/tazel/main.rs`

## 文件定位

[`tools/tazel/main.rs`](main.rs) 是 `astersql-tools-tazel` crate 的业务入口模块，负责把“扫描 Go 测试数量、确认仓库根目录、遍历并改写 `BUILD.bazel`”串成一条完整流程。crate 边界由 `tools/tazel/Cargo.toml` 定义：库入口是 `lib.rs`，二进制入口是 `bin_main.rs`；`bin_main.rs::main` 调用 `astersql_tools_tazel::entry`，再由 `lib.rs::entry` 转发到本文件的 `main`。因此，本文件不是操作系统直接加载的薄入口，而是可由二进制和测试共同复用的主流程实现。

该工具属于仓库构建辅助面，不参与数据库 SQL 请求链。它原地维护仓库内 Bazel `go_test` 规则的 `timeout`、`flaky` 和 `shard_count` 属性。RustCodeGraph 的文件视图确认本文件被 `tools/tazel/lib.rs` 和 `tools/tazel/parity_test.rs` 使用；`tools/tazel/BUILD.bazel` 则描述仍存在的 Go 版 `tazel` 构建目标。

## 核心职责

1. `main` 固定从当前目录启动，并把失败转换为进程级 panic。
2. `run_from` 清空并重建目录测试数映射，验证根目录存在 `WORKSPACE`，再启动 BUILD 文件遍历。
3. `walk_build_files` 递归访问目录，只处理文件名为 `BUILD.bazel` 且未命中 `skipTazel` 的文件，完成读取、补丁和写回。
4. `patch_go_test_file` 解析一个 BUILD 文件，只修改首个顶层 `go_test`：在缺失时补 `timeout = "short"` 和 `flaky = True`，并按同目录 Go 测试数设置或删除 `shard_count`。
5. `walk_path_string` 将遍历路径稳定化为相对根目录、无 `./` 前缀的字符串，供跳过规则和错误消息使用。

本文件刻意不实现 Go 源码扫描、BUILD 语法树或序列化：测试计数委托给 `ast.rs`，BUILD 解析/编辑委托给 `stubs.rs::build`，跳过规则与落盘委托给 `util.rs`。

## 主要符号

- `pub const maxShardCount: u32 = 50`：分片数硬上限，与 `main.go::maxShardCount` 相同；名称保留 Go 风格。
- `pub fn main()`：库层进程入口。以 `Path::new(".")` 调用 `run_from`，错误时 `panic!("{err}")`。
- `pub fn run_from(root: &Path) -> Result<(), String>`：可注入根目录的主流程，供测试和库调用；先调用 `initCount`、`walk_from`，随后检查 `root/WORKSPACE`，最后调用私有的 `walk_build_files`。
- `pub fn patch_go_test_file(rel_path: &str, abs_build: &Path, data: Vec<u8>) -> Result<build::File, String>`：单文件纯补丁边界。`rel_path` 驱动跳过规则；`abs_build` 的规范化父目录用于查询测试计数；返回修改后的 AST，不自行写盘。
- `fn walk_build_files(root: &Path) -> io::Result<()>`：私有递归遍历与 I/O 编排函数。目录继续下钻，普通文件与跳过路径直接略过，目标 BUILD 文件经 `patch_go_test_file` 后交给 `write`。
- `fn walk_path_string(root: &Path, path: &Path) -> String`：私有路径规范化辅助函数；优先 `strip_prefix(root)`，失败时保留原路径，并在两条分支都裁掉可选的 `./`。

本文件没有类型、trait、`impl` 或条件编译项。公开 API 是常量 `maxShardCount` 以及 `main`、`run_from`、`patch_go_test_file`；遍历和路径格式化保持模块私有。

## 执行流程

完整二进制调用链为 `bin_main.rs::main -> lib.rs::entry -> main.rs::main -> run_from`。`run_from` 的顺序具有行为意义：

1. `initCount()` 清空 `ast.rs` 中的全局 `testMap`。
2. `walk_from(root)` 递归扫描 `_test.go`，只统计无接收者、名称以 `Test` 开头且不是 `TestMain` 的顶层函数；目录绝对路径成为计数键。
3. 扫描结束后检查 `root.join("WORKSPACE")`。缺失时返回 `It should run from the project root`。注意：校验发生在预扫之后，这是对 Go `main.go` 顺序的保留。
4. `walk_build_files(root)` 深度优先递归目录。遇到 `BUILD.bazel` 时先通过 `skipTazel(path_string)` 过滤，然后读取字节。
5. `patch_go_test_file` 调用 `build::ParseBuild("BUILD.bazel", data)`，取得 `Rules("go_test")`，只选择索引 0 的规则。
6. 首个规则缺少字符串 `timeout` 时补 `"short"`；路径未被 `skipFlaky` 排除且缺少字面量 `flaky` 时补 `True`。
7. 路径未被 `skipShardCount` 排除时，规范化 BUILD 文件绝对路径并取父目录，从 `test_count_for` 查询预扫结果。计数大于 1 时写入 `min(cnt, 50)`；计数为 1 时删除历史 `shard_count`；无计数时保持原属性不变。
8. `walk_build_files` 把返回的 AST 交给 `util.rs::write`；后者执行 `Rewrite`、`Format`，再以截断方式写回原文件。

## 数据与状态

本文件自身没有持久内存状态。关键跨阶段状态是 `ast.rs::testMap: OnceLock<Mutex<HashMap<String, u32>>>`：`run_from` 先清空它，扫描阶段按规范化后的 Go 测试文件父目录累加，补丁阶段再用 BUILD 文件规范化后的父目录查询。两端目录字符串必须一致，否则 `test_count_for` 返回 `None`，`shard_count` 会保持原状。

`patch_go_test_file` 的输入 `Vec<u8>` 被 BUILD 解析器消费，输出是拥有所有权的 `build::File`。规则借用被限制在内部代码块中，确保返回 `buildfile` 前可变借用已经结束。路径状态分为两个用途：仓库相对的 `rel_path` 用于稳定匹配跳过名单，磁盘上的 `abs_build` 用于和绝对目录计数键对齐。

`maxShardCount` 保证测试数量再大也最多生成 50 个分片。只处理首个 `go_test` 是当前 Go/Rust 共同契约，不代表文件中的其余 `go_test` 已被覆盖。

## 依赖与调用关系

上游关系：

- `tools/tazel/bin_main.rs::main` 调用 `astersql_tools_tazel::entry`。
- `tools/tazel/lib.rs::entry` 调用本文件 `main`，并通过 `#[path = "main.rs"] pub mod main` 暴露本模块。
- `tools/tazel/parity_test.rs` 直接导入 `maxShardCount`、`run_from` 和 `patch_go_test_file`，既覆盖完整入口，也覆盖单文件补丁边界。

下游关系：

- `run_from -> ast.rs::{initCount, walk_from}`，建立分片决策所需的测试计数。
- `run_from -> walk_build_files`，进入 BUILD 文件处理阶段。
- `walk_build_files -> std::fs::{read_dir, read}`，并递归调用自身。
- `walk_build_files -> util.rs::{skipTazel, write}`，分别执行路径过滤和格式化写回。
- `walk_build_files -> patch_go_test_file`，把 I/O 遍历与 AST 补丁分离。
- `patch_go_test_file -> stubs.rs::build::{ParseBuild, File::Rules, Rule 属性操作}`，完成 BUILD AST 解析和变更。
- `patch_go_test_file -> util.rs::{skipFlaky, skipShardCount}` 与 `ast.rs::test_count_for`，决定属性是否以及如何变化。

`tools/tazel/Cargo.toml` 的 `[dependencies]` 为空，说明 Rust 版在此 crate 内使用标准库和本地 BUILD 解析桩，没有声明外部 Rust crate。Go 版的 Bazel 目标则依赖 `buildtools/build`、PingCAP 日志、Zap 和 `pkg/util/set`；这是两版依赖实现方式的差异，不改变入口策略。

## 错误处理与边界

- `run_from` 返回 `Result<(), String>`，但 `main` 将任何错误升级为 panic；作为命令行工具，这会以失败结束进程。
- `walk_from` 自身在扫描失败时 panic，而不是把错误纳入 `run_from` 的 `Result`。因此即使 `WORKSPACE` 缺失，预扫期间的不可读目录、无效 Go 文件或缺少 `gofmt` 也可能先终止流程。
- `WORKSPACE` 只用 `Path::exists` 判断。任何导致该路径不可见的情况都会被表达为“应从项目根运行”，具体 I/O 原因不会保留。
- `walk_build_files` 传播目录枚举、元数据和写入错误；读取错误会增加 `fail to read file, path: ...` 上下文，解析/补丁错误被包装成 `InvalidData` 并增加 `fail to parser BUILD.bazel, path: ...` 上下文，最外层再增加 `fail to filepath.Walk`。
- `patch_go_test_file` 中 `fs::canonicalize(abs_build)` 失败会退化成仅含底层消息的 `String`。没有父目录时使用空字符串查询计数，通常得到 `None`。
- 无 `go_test` 时返回未改动 AST；有多个 `go_test` 时只改第一个；已有非空 `timeout`/`flaky` 不覆盖；跳过分片或查不到计数时不删除现有 `shard_count`；只有明确计数为 1 时删除它。
- 遍历使用 `entry.metadata()?.is_dir()`，未显式处理符号链接循环或并发目录变化；错误按普通 I/O 错误向上传播。

## 并发与资源生命周期

主流程是单线程、同步、深度优先执行，没有任务、通道或异步运行时。每个 `read_dir` 迭代器、文件字节和 BUILD AST 都在对应递归调用或循环迭代内释放；`util.rs::write` 每次创建并关闭一个文件句柄。`parity_test.rs::contract_resource_cleanup` 通过执行后删除临时目录验证没有残留句柄阻止清理。

唯一共享并发状态位于 `ast.rs::testMap`，由 `OnceLock<Mutex<_>>` 保护。`initCount` 在每轮入口先清空状态，避免同一进程重复调用时累积旧数据；`test_count_for` 只返回复制出的计数快照。互斥锁保证单次访问安全，但完整的“清空—扫描—改写”不是一个整体事务：若多个线程并发调用 `run_from`，它们仍可能相互清空或混合计数，因此当前 API 应按串行命令执行模型使用。

写回也不是跨文件事务：前面的 BUILD 文件可能已经落盘，后续文件才失败。`util.rs::write` 使用 `truncate(true)` 直接覆盖，不通过临时文件重命名提供崩溃原子性；扩展错误恢复时必须考虑部分完成状态。

## 与 Go 版本的对应关系

Rust 文件直接对应 `tools/tazel/main.go`：两者均定义 `maxShardCount = 50`，先 `initCount` 和扫描，再检查 `WORKSPACE`，然后遍历 `BUILD.bazel`；都只修改首个 `go_test`，使用相同跳过谓词，且对计数执行“大于 1 则设置并封顶、否则删除”的逻辑。

主要结构差异如下：

- Go `main` 把全部流程放在一个函数和 `filepath.Walk` 回调中；Rust 拆为 `run_from`、`walk_build_files` 和 `patch_go_test_file`，以便注入临时根目录和独立测试 AST 补丁。
- Go 用 `filepath.Abs(path)`；Rust 用 `fs::canonicalize(abs_build)`。后者要求目标存在并解析符号链接，因此在不存在路径和符号链接布局上的失败/键值语义可能不同。
- Go 对读/解析错误立即 `log.Fatal`，遍历回调错误最终也 `log.Fatal`；Rust 内层返回 `Result`/`io::Result`，仅在公开 `main` 统一 panic。命令最终都失败，但日志格式和析构路径不同。
- Go 的 `filepath.Walk` 回调忽略传入的 error 参数并直接读取 `d`；Rust 手工 `read_dir` 并显式传播枚举和元数据错误。
- Rust 的 BUILD 解析由同 crate 的 `stubs.rs` 提供，而 Go 使用 Bazel buildtools；语法保真度由 `parity_test.rs` 的顶层规则、单行规则、无关语法保留和无效输入用例约束，但不能据此推断桩支持 buildtools 的全部语法。

## 扩展指南

- 新增或修改规则属性时，最合适的接入点是 `patch_go_test_file`。保持“已有显式值不覆盖”的兼容策略，并在 `tools/tazel/parity_test.rs` 增加正常、已有值、跳过路径和多规则用例；不要把测试内嵌到 `main.rs`。
- 修改遍历过滤或路径形式时，应集中调整 `walk_build_files`/`walk_path_string`，同时核对 `util.rs` 的精确字符串/前缀匹配。Windows 分隔符、`root = "."`、绝对根目录和符号链接是路径兼容风险。
- 修改测试计数到分片的映射时，应同步 `maxShardCount`、`ast.rs` 的计数口径、`main.go` 的对应逻辑和 `parity_test.rs::contract_boundary`。分片增加会影响 CI 并发与资源消耗，属于性能和调度风险。
- 若要支持一个 BUILD 文件中的全部 `go_test`，必须明确这是对当前 Go 契约的扩展；同时更新 Go 对照或记录有意差异，并增加“多个顶层规则”的独立测试，防止只验证文本中仍有第二个规则而漏掉属性语义。
- 若要使写回具备故障原子性或可回滚性，应在 `util.rs::write` 实现临时文件/重命名策略，并为中途失败设计集成级测试；本文件的逐文件顺序不提供全仓库事务。
- 若要允许并发 `run_from`，必须先移除或按执行实例隔离全局 `testMap`。仅依赖 `Mutex` 不足以保护跨多个调用步骤的一致性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `tools/tazel` 的 11 个 Go/Rust 文件；`files --filter tools/tazel` 列出本文件及装配、测试和 Go 对照；`node --file tools/tazel/main.rs --offset 1 --limit 260` 返回完整 185 行，并报告本文件被 `tools/tazel/lib.rs`、`tools/tazel/parity_test.rs` 使用。
- RustCodeGraph 限制：对 `run_from`、`patch_go_test_file`、`walk_build_files`、`walk_path_string` 执行的 `query/callers/callees` 未返回可用符号或调用边；因此上述调用关系又由 `main.rs` 的函数体、`lib.rs::entry`、`bin_main.rs::main` 和 `parity_test.rs` 的直接导入/调用交叉核验，没有把缺失的图边当作已验证事实。
- 源码与配置：阅读了 `tools/tazel/main.rs`、`ast.rs`、`util.rs`、`lib.rs`、`bin_main.rs`、`Cargo.toml` 和 `BUILD.bazel`；它们分别证明流程实现、计数状态、过滤/写回、crate 装配、二进制转发和两种构建边界。
- Go 对照：阅读 `tools/tazel/main.go`，逐项核对入口顺序、首个 `go_test`、默认属性、跳过条件、分片上限与错误出口。
- 独立测试：`tools/tazel/parity_test.rs` 覆盖完整 `run_from` 写回、首个顶层规则、属性保留、单测试删除分片、跳过名单、缺少 `WORKSPACE`、语法保真和资源清理；`tools/tazel/ast_test.rs` 属于下游扫描模块测试，不直接调用本文件。仓库搜索未发现其他直接引用本文件公开符号的测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认目标文件存在且恰有 11 个固定二级章节，并人工复查文档能够回答文件为何存在、如何运行及如何安全扩展。
