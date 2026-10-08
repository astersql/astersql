# `tools/check/xprog/xprog.rs` 逻辑说明

## 文件定位

`tools/check/xprog/xprog.rs` 是 `astersql-tools-check-xprog` crate 的核心实现文件，crate 边界由 `tools/check/xprog/Cargo.toml` 定义。该 crate 被根 `Cargo.toml` 纳入 workspace，声明为 `kind = "binary"`，同时提供库入口 `tools/check/xprog/lib.rs` 和二进制壳 `tools/check/xprog/bin_main.rs`。启动链是 `bin_main.rs::main` → `lib.rs::main` → `xprog.rs::main`。

文件对应 Go `main` 包的 `tools/check/xprog/xprog.go`。它不是测试执行器，而是给 `go test --exec=<xprog>` 使用的包装程序：Go 工具链在临时构建目录生成测试二进制后调用它，它再把二进制搬到仓库内相应包目录，供 `tools/check/ut.go::buildTestBinaryMulti` 或 Rust 对照实现 `tools/check/ut.rs::build_test_binary_multi` 后续发现和执行。

需要区分“Rust crate 已存在”与“现行 Make 流程已切换”两件事：`Makefile` 的 `tools/bin/xprog` 目标目前仍从 `tools/check/xprog/xprog.go` 构建 Go 可执行文件；Rust 文件是 workspace 中的独立移植实现，不能据此声称默认 `make ut` 已使用 Rust 二进制。

## 核心职责

本文件完成三项紧密关联的职责：

1. `main`/`run` 接收 `go test --exec` 传来的参数，从 `argv[0]` 推导仓库根目录，从 `argv[1]` 得到临时测试二进制路径。
2. `get_package_info` 读取测试二进制同目录的 `importcfg.link` 首行，恢复形如 `github.com/pingcap/tidb/pkg/session.test` 的包标识；`run` 验证仓库前缀、移除模块前缀和 `.test` 后缀，并计算 `<仓库>/<包>/<包名>.test.bin`。
3. `run` 优先用 `std::fs::rename` 搬运文件；跨文件系统等原因导致 rename 失败时，调用 `move_file` 执行“复制内容、同步权限、删除源文件”的回退流程。

该实现刻意保持 Go 版本的可观察契约，包括负退出码、部分不合法输入触发 panic、4096 字节 `bufio.Reader.ReadLine` 首片段语义，以及 Go `filepath.Join/Clean` 的路径规则。路径兼容逻辑不在本文件重复实现，而由 `tools/check/xprog/stubs.rs::{filepath_join, filepath_clean}` 提供。

## 主要符号

- `pub fn main()`：进程级入口。收集全部 `std::env::args()` 交给 `run`；返回码非零时调用 `std::process::exit`。成功时自然返回 0。它不捕获 panic，因为 Go 对照在参数或格式契约被破坏时同样会崩溃。
- `pub fn run(args: &[String]) -> i32`：可测试的核心编排函数，也是文件的主要公开 API。它不启动测试二进制，仅解析位置并搬运文件；成功返回 `0`，已建模的失败返回 `-1` 至 `-4`。
- `pub fn get_package_info(dir: &Path) -> Result<String, i32>`：读取 `<dir>/importcfg.link` 首个缓冲片段并提取第一个空格与第一个 `=` 之间的包路径。只把打开失败和首片段读取失败表示为 `Err(-1)`、`Err(-2)`；分隔符缺失或包路径不是 UTF-8 时 panic。
- `pub fn move_file(source_path: &Path, dest_path: &Path) -> io::Result<()>`：rename 失败时的复制回退。按顺序打开源、创建目标、复制、关闭句柄、读取源权限、设置目标权限、删除源。

本文件没有模块级常量、类型、trait、`impl`、宏或条件编译项。四个函数均为 `pub`，但常规二进制调用只经 `main` 进入；其余公开性主要服务 crate 入口复用和独立契约测试。

## 执行流程

1. `bin_main.rs::main` 调用 `astersql_tools_check_xprog::main`，后者转发到本文件 `main`。
2. `main` 收集命令行参数并调用 `run`。典型参数是 `$REPO/tools/bin/xprog /tmp/go-build.../pkg.test ...`；测试参数会随 Go 一并传入，但 `run` 只读取前两个元素。
3. `run` 计算 `tools/bin/xprog` 的路径文本长度，并直接从 `args[0]` 尾部裁掉相同字节数以得到仓库根。这里仅按长度裁剪，不验证真实后缀，保持 `xprog.go::main` 的切片行为。
4. `run` 按 Go 路径语义清理 `args[1]`，取父目录，调用 `get_package_info` 读取同目录 `importcfg.link`。打开失败返回 `-1`，空文件或首片段读取失败返回 `-2`。
5. `run` 要求包路径以 `github.com/pingcap/tidb` 开头，否则返回 `-3`。随后裁掉该前缀以及末尾 `.test` 长度，并从剩余包路径取最后一段作为文件基名。
6. `run` 用 `stubs::filepath_join` 和 `filepath_clean` 形成 `<cwd>/<pkg>/<leaf>.test.bin`。后续包目录必须已经存在；本函数不创建目录。
7. `run` 先调用 `fs::rename`。成功即返回 `0`；失败则调用 `move_file`，后者成功仍返回 `0`，再次失败返回 `-4`。
8. `main` 对非零结果执行 `process::exit(code)`。在 Unix 上操作系统看到的是退出状态低 8 位；源码和测试以 Go `os.Exit` 的负数调用契约记录这些分类。

## 数据与状态

该文件没有全局可变状态。主要数据均为一次进程调用内的局部值：参数切片 `args`、推导出的仓库根 `cwd`、临时二进制 `test_binary_path`、构建目录 `dir`、包路径 `pkg`、包叶名 `file` 和目标路径 `new_name`。

持久状态只体现在文件系统变化：成功后目标 `<包目录>/<包名>.test.bin` 存在，源临时测试二进制消失。rename 成功时移动由文件系统完成；复制回退时目标可能先被 `File::create` 创建或截断，然后才写入并同步权限。若复制、读取元数据、设置权限或删除源文件失败，函数返回错误，但不会主动回滚已经创建或部分写入的目标文件。

`get_package_info` 只观察 `importcfg.link` 的首个 4096 字节缓冲片段。它既不扫描后续行，也不在超长首行时继续拼接剩余片段；这是为了对齐 Go 默认 `bufio.Reader` 和被忽略的 `isPrefix` 返回值，而不是通用配置解析器行为。

## 依赖与调用关系

上游关系：

- 进程入口：`tools/check/xprog/bin_main.rs::main` → `tools/check/xprog/lib.rs::main` → `tools/check/xprog/xprog.rs::main` → `run`。
- 外部工作流：`tools/check/ut.go::buildTestBinaryMulti` 和 `tools/check/ut.rs::build_test_binary_multi` 都把 `tools/bin/xprog` 传给 `go test --exec`；二者随后按 `tools/check/{ut.go,ut.rs}::testFileName/test_file_name` 约定查找 `.test.bin`。
- 测试调用：`tools/check/xprog/parity_test.rs` 直接调用 `run`、`get_package_info`、`move_file`；`tools/check/xprog/xprog_test.rs` 直接调用 `get_package_info`。

下游关系：

- `run` 调用 `stubs::filepath_join`、`stubs::filepath_clean`、`get_package_info`、`std::fs::rename` 和 `move_file`。
- `get_package_info` 使用 `std::fs::File`、`std::io::BufReader::with_capacity(4096)` 与 `BufRead::fill_buf`。
- `move_file` 使用 `std::fs::{File, metadata, set_permissions, remove_file}` 和 `std::io::copy`。

`tools/check/xprog/Cargo.toml` 的 `[dependencies]` 为空，说明实现只依赖 Rust 标准库和 crate 内 `stubs`。RustCodeGraph 对 `run` 的已解析边显示其被本文件 `main` 及测试契约函数调用，并调用上述两个路径桩和两个本地辅助函数。由于 `go test --exec` 是跨进程调用，静态 Rust 调用图无法表达 `ut` → xprog 的运行时边，该边由 `tools/check/ut.go`、`tools/check/ut.rs` 的命令构造提供证据。

## 错误处理与边界

明确返回的状态如下：

- `-1`：无法打开 `importcfg.link`。
- `-2`：`fill_buf` 报错或首个缓冲区为空；空文件属于此类。
- `-3`：解析出的包路径不以 `github.com/pingcap/tidb` 开头。
- `-4`：`rename` 失败，且 `move_file` 回退也失败。

以下情况不是返回码，而会 panic，以保持 Go 版本的索引/切片失败形状：参数列表为空、缺失 `args[1]`、`args[0]` 比固定后缀短或落在非 UTF-8 字符边界、包文本短到不能裁掉 `.test` 长度、首片段没有空格或 `=`、分隔顺序导致无效切片、包名字节不是 UTF-8。尤其要注意，代码只验证仓库前缀，不验证包名真的以 `.test` 结尾；只要长度足够，它仍会无条件裁掉五个字节。

路径与文件系统边界同样重要：目标父目录不会自动创建；目标已存在时 `rename`/`File::create` 的覆盖行为取决于平台及文件系统；本地 `stubs` 明确复刻 Unix 风格 Go 路径语义，不承诺 Windows 卷前缀或符号链接解析。`file_name` 不存在时会得到空字符串，目标名退化为 `.test.bin`。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或共享内存。多个 xprog 进程可以被 `go test -p` 并行启动；安全性主要依赖每个 Go 包拥有不同目标目录和 `<leaf>.test.bin`。若并发调用为同一目标命名，则本文件没有锁、临时文件或原子替换协议来协调，最后结果受文件系统竞争影响。

`get_package_info` 的 `File` 与 `BufReader` 在函数返回或 panic 展开时由 RAII 关闭。`move_file` 显式在复制后 `drop(input_file)` 和 `drop(output_file)`，再进行 metadata/chmod/remove，以对应 Go 版的关闭时序并避免删除源文件时仍持有句柄。目标创建失败会先关闭源句柄；复制成功且 chmod 成功后才删除源，因此这些阶段失败时源文件仍保留。删除源失败时，源和目标会同时存在；函数返回错误并最终被 `run` 映射为 `-4`。

rename 路径通常在单一文件系统内具有原子移动性质；copy + chmod + remove 回退不是原子的，其他进程可能观察到部分目标文件或源/目标并存窗口。新增并发消费方时必须把这一差异视为接口约束。

## 与 Go 版本的对应关系

Rust `main` 对应 `xprog.go::main` 的退出边界，`run` 把 Go `main` 主体拆成可测试函数；`get_package_info` 对应 `getPackageInfo`；`move_file` 对应导出的 `MoveFile`。字段级映射包括：`os.Args` ↔ `args`，`filepath.Clean/Join/Split` ↔ `stubs::filepath_clean/filepath_join` 加 `Path::parent/file_name`，`os.Rename` ↔ `fs::rename`，`io.Copy` ↔ `io::copy`，`os.Stat/Chmod/Remove` ↔ `metadata/set_permissions/remove_file`。

行为对齐点包括：按固定长度而非后缀内容裁剪 `argv[0]`；只读 `importcfg.link` 首行/首片段；包前缀检查；无条件按 `.test` 长度裁剪；目标命名为 `<leaf>.test.bin`；rename 失败后执行 copy + chmod + remove；错误分类保持 `-1..-4`；违反隐含格式时保留 panic 而非新增容错。

Rust 的结构性差异是把可恢复的两个配置读取错误表示为 `Result<String, i32>`，把主流程状态表示为 `i32`，再由 `main` 执行进程退出，以便测试不必派生子进程。`move_file` 使用 `io::Error` 并保留与 Go 相同的诊断前缀。Rust 还显式关闭输出句柄后再读取权限和删除源文件；Go 输出句柄由 `defer` 在函数返回时关闭，但对当前普通文件场景的最终契约相同。

## 扩展指南

- 修改参数、包名解析、目标命名或返回码时，首要接点是 `run`；必须同步 `tools/check/xprog/xprog.go` 的对应语义，检查 `tools/check/ut.go::testFileName` 与 `tools/check/ut.rs::test_file_name` 的消费者约定，并扩展 `tools/check/xprog/parity_test.rs`。
- 修改 `importcfg.link` 读取策略时应改 `get_package_info`，同时更新 `xprog_test.rs::get_package_info_matches_go_read_line_prefix_limit`。若希望支持超长行或多行搜索，这将有意偏离当前 Go 行为，不能仅当作内部重构。
- 修改跨设备搬运时应改 `move_file`，至少覆盖内容、权限、源删除、目标创建失败、copy/chmod/remove 失败后的残局。若引入临时文件加原子替换，还要明确同目标并发和覆盖策略，并评估额外 I/O 与 fsync 成本。
- 修改路径拼接时应优先检查 `tools/check/xprog/stubs.rs`，不要直接换成 `Path::join`；后续片段以 `/` 开头时，两者语义不同。需要同步该 crate 的路径契约测试。
- 若要让默认 Make 流程使用 Rust 实现，改动点不在本文件本身，而在 `Makefile` 的 `tools/bin/xprog` 构建规则及相关交付接线；这属于独立迁移任务，需验证 Go 工具调用的二进制名称、位置和退出状态兼容性。

兼容风险集中在 Go 隐含 panic/退出码契约和 Unix 路径语义；正确性风险集中在错误裁剪包路径、覆盖错误目标以及失败后留下部分文件；性能风险主要是跨设备回退必须完整复制大型测试二进制。任何扩展都不应通过吞掉这些边界或把所有失败统一成单一错误来“简化”实现。

## 验证依据

- 生产实现：`tools/check/xprog/xprog.rs`，核对 `main`、`run`、`get_package_info`、`move_file` 的完整源码与 RustCodeGraph 符号节点。
- crate/入口：`tools/check/xprog/Cargo.toml`、`tools/check/xprog/lib.rs`、`tools/check/xprog/bin_main.rs`、根 `Cargo.toml` workspace 成员列表。
- 路径依赖：`tools/check/xprog/stubs.rs::{filepath_join, filepath_clean}`。
- Go 对照：`tools/check/xprog/xprog.go::{main,getPackageInfo,MoveFile}`。
- 上下游工作流：`tools/check/ut.go::buildTestBinaryMulti/testFileName`、`tools/check/ut.rs::build_test_binary_multi/test_file_name`，以及 `Makefile` 的 `tools/bin/xprog` 目标。
- 独立测试：`tools/check/xprog/parity_test.rs` 覆盖正常 rename、复制回退、路径边界、`-1..-4`、缺参/短包名 panic、内容与权限、失败时保留源文件；`tools/check/xprog/xprog_test.rs::get_package_info_matches_go_read_line_prefix_limit` 覆盖 4096 字节首片段行为。
- RustCodeGraph 索引状态：项目索引包含 `tools/check/xprog` 的 Rust/Go 文件；`xprog.rs::run` 的节点边显示调用 `filepath_join`、`filepath_clean`、`get_package_info`、`move_file`，并由本文件 `main` 及契约测试调用。外部进程调用关系另由上述 `ut` 命令构造源码核实。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付校验只验证固定十一节结构，并人工复核所有现状判断均能回溯到以上文件或符号。
