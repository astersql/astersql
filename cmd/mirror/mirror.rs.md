# `cmd/mirror/mirror.rs`

## 文件定位

`mirror.rs` 是 `astersql-cmd-mirror` crate 的核心实现文件。该 crate 在 `cmd/mirror/Cargo.toml` 中声明为迁移自 Go 包 `cmd/mirror` 的 binary crate；`cmd/mirror/bin_main.rs::main` 调用 `astersql_cmd_mirror::main`，再由 `cmd/mirror/lib.rs::main` 单跳进入本文件的 `main`。因此，本文件既包含命令入口，也包含从 Go module 元数据生成 Bazel `go_repository` 定义的完整业务流程。

本文件不是 TiDB SQL 请求链的一部分，而是构建依赖生成工具：它读取根模块和 `pkg/parser` 的 `go.mod`/`go.sum`，调用 runfiles 中的 Go 工具列举并下载模块，最后把新的 `deps.bzl` 内容写到标准输出。真实 Bazel、子进程和文件系统操作通过 `cmd/mirror/stubs.rs` 中的 `Bazel`、`Runner`、`Fs` trait 注入；这既是生产边界，也是独立测试能够不访问真实网络和磁盘的原因。

直接相关文件如下：

- `cmd/mirror/Cargo.toml`：定义库、二进制目标以及仅有的 `serde`、`serde_json` 依赖。
- `cmd/mirror/lib.rs`：公开 `stubs` 和 `mirror` 模块，并挂接两份独立 Rust 测试。
- `cmd/mirror/bin_main.rs`：进程级薄入口。
- `cmd/mirror/mirror.go`：逐函数对齐的原始 Go 实现。
- `cmd/mirror/mirror_test.rs`、`cmd/mirror/parity_test.rs`：输出错误、清理错误与 Go/Rust 公共契约的回归测试。

## 核心职责

本文件有五组职责，边界都能由具体符号定位：

1. 用 `DownloadedModule` 和 `ListedModule` 反序列化 Go 命令输出，只保留生成 Bazel 规则所需字段。
2. 由 `create_tmp_dir` 建立隔离工作目录，并复制根模块及 parser 子模块的依赖清单。
3. 由 `list_all_modules` 和 `download_zips` 运行 `go list -mod=readonly -m -json all` 与 `go mod download -json ...`，再由两个解析函数把串联 JSON 对象转成按模块路径索引的映射。
4. 由仓库名转换和若干显式例外函数生成稳定排序的 `go_repository` 配置，包括 patch、build tag、proto mode、命名约定和 replace 信息。
5. 由 `mirror_with` 编排流程并确保临时目录回收；由 `run_main_with`/`main` 处理旧 flags、标准输出/错误和 Go 风格的子进程失败包装。

这里的核心契约是“生成结果与 Go 版兼容”，而非提供通用 Go module API。硬编码例外、按 Bazel 仓库名排序、忽略输出 writer 错误等行为都有测试固定，不能按通常的 Rust 风格擅自简化。

## 主要符号

- `DownloadedModule { Path, Sum, Version, Zip }`：对应 `go mod download -json` 的对象。字段通过 `serde(rename = ...)` 接受 Go 的大写 JSON 键，并以空字符串作为缺失字段默认值。生成阶段实际读取 `Path`、`Sum`、`Version`；`Zip` 为 Go 结构对齐字段。
- `ListedModule { Path, Version, Replace }`：对应 `go list -m -json` 的对象。递归的 `Replace: Option<Box<ListedModule>>` 同时表达无替换、本地路径替换和带版本的远端替换。
- `IS_MIRROR`、`IS_UPLOAD`：线程局部的废弃 flag 状态。`init_flags` 重置它们，`set_flags` 写入解析结果；业务生成不读取这两个值，提示输出直接读取 `Flags`。
- `copy_file`：对 `Fs::copy_file` 的薄包装，保留 Go `copyFile` 的函数边界。
- `create_tmp_dir`：创建 `gomirror` 临时目录、创建 `pkg/parser` 子目录，解析根 `go.mod`/`go.sum` runfile，并通过首次字符串替换得到 parser 文件路径后复制四个文件。
- `list_all_modules`：运行 `go list -mod=readonly -m -json all`；`-mod=readonly` 是“不修改输入依赖文件”的重要约束。
- `download_zips`：拼装 `go mod download -json` 参数。本地 replace 的 `Version` 为空，因而跳过；远端 replace 下载替换后的 `Path@Version`；普通模块下载自身坐标。
- `parse_listed_modules`、`parse_downloaded_modules`：解析串联 JSON，并分别构建 `HashMap<String, ListedModule>` 和 `HashMap<String, DownloadedModule>`。前者过滤根模块 `github.com/pingcap/tidb`，防止生成自引用。
- `split_json_objects`：按行累积文本，在行首为 `}` 时完成一个对象，直接复刻 Go 版的流式切分规则；它不是通用 JSON stream parser。
- `munge_bazel_repo_name_component`、`module_path_to_bazel_repo_name`：把 module path 转成 Gazelle/Bazel 仓库名。域名段倒序，其他路径段依次拼接，所有段将 `-`、`.` 换成 `_` 并转小写。
- `dump_patch_args_for_repo`：若 runfiles 的 `build/patches/<repo>.patch` 存在，写出 `patch_args` 和 `patches`；文件不存在是正常分支，其他 stat 错误向上传播。
- `build_file_proto_mode_for_repo`：仅 `io_etcd_go_etcd_api_v3` 返回 `disable`，其余返回 `disable_global`。
- `dump_build_naming_convention_args_for_repo`：仅为 `com_github_grpc_ecosystem_grpc_gateway` 输出 `go_default_library` 命名约定。
- `dump_new_deps_bzl`：生成完整 Starlark 文本，是主要输出函数。它按转换后的 repo name 排序，跳过 parser 自身，并处理 TiKV build tags、replace、校验和、版本及上述例外。
- `TmpDirGuard`：持有临时目录路径和借用的 `Fs`。其 `Drop` 在成功、返回错误或 panic 展开时调用 `remove_all`；删除失败继续 panic。`disarm` 当前未被调用，是预留的停用能力，不应误述为现有主流程分支。
- `mirror_with`：可注入的主流程，依次执行准备目录、列举模块、下载模块、输出配置。
- `mirror`：以 `ProdBazel`、`ProdRunner`、`OsFs` 和 stdout 调用 `mirror_with`；它不是二进制最终入口，但可供库调用。
- `run_main_with`：可注入 argv/stdout/stderr 的入口。它解析 flags、打印废弃提示，并将业务错误转换为与 Go `main` 一致的 panic。
- `main`：从进程环境取 argv，装配生产实现；只对 `run_main_with` 在解析 flags 阶段返回的 `Err` 写 stderr 并以状态 2 退出。业务流程错误已在 `run_main_with` 内 panic。
- `join_path`、`replace_once`：局部路径拼接与只替换首次匹配的辅助函数。

## 执行流程

进程路径为：

`bin_main.rs::main` → `lib.rs::main` → `mirror.rs::main` → `run_main_with` → `mirror_with`。

详细步骤如下：

1. `main` 读取除 argv0 外的参数，创建生产 `ProdBazel`、`ProdRunner`、`OsFs`，并绑定 stdout/stderr。
2. `run_main_with` 先用 `init_flags` 清空线程局部状态，再调用 `stubs::parse_flags_checked`。支持 `-mirror`/`--mirror`、`-upload`/`--upload` 及布尔赋值形式；启用旧 flag 只向 stderr 打印“deprecated and ignored”，不会改变生成流程。
3. `mirror_with` 调用 `create_tmp_dir`。临时目录创建完成后，`TmpDirGuard` 立即接管回收，因此此后的正常返回和失败展开都会触发删除。
4. `list_all_modules` 解析 `bin/go` runfile，以临时目录为工作目录运行 `go list -mod=readonly -m -json all`。环境由当前进程变量加 `GOSUMDB=sum.golang.org` 组成。
5. `parse_listed_modules` 将串联 JSON 分对象解析，过滤根 TiDB module，并按原始 module path 建表。
6. `download_zips` 遍历列举结果生成下载坐标。本地 replace 跳过，远端 replace 改用替换目标，普通依赖使用本身；随后运行 `go mod download -json` 并解析下载结果。
7. `dump_new_deps_bzl` 先把每个 module path 转成 repo name 并排序，再逐项输出 `go_repository`。下载结果用“实际下载路径”查找：有 replace 时是 `Replace.Path`，否则是原 `Path`。
8. 函数返回或 panic 展开时，`TmpDirGuard::drop` 删除临时目录。删除失败本身会 panic，保持 Go `defer os.RemoveAll` 失败时 panic 的可见行为。

`mirror` 是步骤 3–8 的生产便捷包装；当前二进制入口使用 `run_main_with`，以便同时覆盖 flags 和错误文本契约。

## 数据与状态

主要数据以两个 `HashMap` 在阶段间传递：`listed` 以原始 module path 为键，保存直接版本和可选 replace；`downloaded` 以实际下载对象的 path 为键，保存最终 sum/version。`dump_new_deps_bzl` 额外构建 `repo_name_to_mod_path`，因为输出按 repo name 排序、取值却仍需回到原始 module path。这隐含一个不变量：转换后的 repo name 应唯一；若两个 module path 映射到同一名称，映射中的后写值会覆盖前值而 `sorted` 仍保留重复名称，当前代码没有显式冲突检测。

`DownloadedModule`、`ListedModule` 字段保持 Go 风格大写命名，是反序列化和移植对照所需的兼容形状；crate 根通过 allow 属性容纳这些名称。`Zip` 当前不参与后续生成，不能据此推断 zip 文件会由 Rust 代码直接读取。

`IS_MIRROR` 与 `IS_UPLOAD` 使用 `thread_local!` 和 `Cell<bool>`，每次 `run_main_with` 都先重置，避免同一测试线程连续调用时泄漏旧值。它们不是跨线程共享配置，也不控制下载或输出分支。

输出状态只存在于传入的 `dyn Write`。所有 `write!`/`writeln!` 结果被显式丢弃，以匹配 Go `fmt.Print*` 未检查返回错误的行为；因此 BrokenPipe 不会立即终止生成，后续的业务错误仍可返回。`cmd/mirror/mirror_test.rs::deps_bzl_output_errors_are_ignored_like_go_fmt_print` 固定了这一点。

## 依赖与调用关系

上游关系：

- 生产调用链由 `cmd/mirror/bin_main.rs` 和 `cmd/mirror/lib.rs` 接入本文件的 `main`。
- `cmd/mirror/mirror_test.rs` 直接调用 `dump_new_deps_bzl`、`mirror_with`。
- `cmd/mirror/parity_test.rs` 直接覆盖名称转换、解析、下载、临时目录创建、配置输出、`run_main_with` 和资源回收。
- RustCodeGraph 对目标文件的文件级关系显示其被上述两份测试文件使用；对精确符号执行了 `query`、`callers`、`callees` 检查，精确 query 定位到 `mirror_with:432`、`dump_new_deps_bzl:323` 等定义，当前 callers/callees 命令未返回额外文本边，故入口链同时以模块源文件核验。

下游关系：

- 标准库：`HashMap` 保存模块集合，`Write` 抽象输出，`Path` 拼接路径，环境变量提供 Go 子进程环境。
- `serde`/`serde_json`：只用于 Go JSON 对象反序列化；Cargo 未声明其他第三方依赖。
- `crate::stubs::Bazel`：提供临时目录、runfile 和 runfiles 根路径。
- `crate::stubs::Runner`：执行 Go 子进程并返回 stdout；生产 `ProdRunner` 将非零状态转换为带 stderr 的 `Error::exit`。
- `crate::stubs::Fs`：负责目录创建、文件复制、patch stat 和递归清理；生产实现是 `OsFs`。

关键内部调用边是 `run_main_with → mirror_with → {create_tmp_dir, list_all_modules, download_zips, dump_new_deps_bzl}`；生成阶段进一步调用名称转换、仓库例外和 patch 输出函数。`list_all_modules` 与 `download_zips` 都依赖 `command_env_with_gosumdb` 和对应 JSON parser。

## 错误处理与边界

绝大多数外部错误使用 `cmd/mirror/stubs.rs::Error` 和 `Result<T>` 逐层 `?` 传播：临时目录/runfile/复制失败、子进程启动或退出失败、JSON 反序列化失败、patch stat 的非 NotFound 错误、下载结果缺失都会终止主流程。

需要特别保护的边界如下：

- `split_json_objects` 只在某行以 `}` 开头时提交对象；末尾没有该终止行的残余文本会被忽略。这是对 Go 当前实现的机械对齐，不具备任意空白/任意 JSON stream 的通用保证。
- `String::from_utf8_lossy` 用于解析 Go 输出和展示子进程 stderr，因此非法 UTF-8 会替换为替代字符，而不是产生 UTF-8 错误。
- 根模块仅按字面值 `github.com/pingcap/tidb` 过滤；仓库名称变化不会被自动识别。
- 本地 replace 在下载阶段被跳过，但如果它仍进入 `dump_new_deps_bzl`，生成阶段会按本地 `Replace.Path` 查找下载结果并报缺失。现有测试固定的是“下载参数跳过本地路径”，没有证明本地 replace 能生成规则，扩展时不可把两者混为一谈。
- 缺少 patch 文件是正常情况；其他 stat 错误必须返回。
- 缺少 downloaded 条目会返回包含 `path@version` 的明确业务错误。
- writer 写入错误被忽略，这是刻意保持 Go `fmt.Print*` 行为；不要改成 `?` 而不同时评估兼容性。
- `run_main_with` 对 `is_exit` 错误 panic，并把 stderr 加上 `subprocess exited with stderr:` 前缀；普通业务错误也 panic，但直接显示错误文本。flag 解析错误则作为 `Err` 返回给 `main`，由其打印并 `exit(2)`。
- 清理失败由 `TmpDirGuard::drop` panic。若它发生在另一个 panic 的展开过程中，Rust 进程可能因双重 panic 终止；代码没有提供第二错误聚合或降级日志路径。

## 并发与资源生命周期

单次 `mirror_with` 是同步、串行流程：只依次启动两个 Go 子进程，没有线程、异步任务、通道或共享事务。模块映射遍历本身无顺序保证，因此输出前必须对 repo name 排序；下载参数顺序未排序，当前契约只要求包含正确坐标，不要求参数稳定顺序。

资源生命周期以临时目录为中心：`create_tmp_dir` 返回成功后立刻构造 `TmpDirGuard`，守卫借用同一个 `Fs` 并拥有路径；作用域退出时自动回收。`parity_test.rs::contract_resource_cleanup` 覆盖成功、list 失败和删除失败，`mirror_test.rs::cleanup_panic_uses_go_error_text_instead_of_rust_debug_fields` 进一步固定清理 panic 文本。

`ProdBazel::NewTmpDir` 的唯一性由进程 ID和 `AtomicU64` 序列共同保证，实际实现位于 `stubs.rs`；本文件只消费创建后的路径。两个废弃 flag 的状态是线程局部而非全局锁保护状态，因此并行测试线程互不污染，但同一线程的调用依赖 `run_main_with` 开头重置。传入的 trait 对象和 writer 没有 `Send`/`Sync` 约束，本 API 不承诺可跨线程共享。

## 与 Go 版本的对应关系

`cmd/mirror/mirror.go` 与本文件基本逐函数对应：

- Go 的两个 module struct 对应 Rust 的 `DownloadedModule`、`ListedModule`。
- `copyFile`、`createTmpDir`、`downloadZips`、`listAllModules`、名称转换和各 dump helper 均有同名 snake_case Rust 实现。
- Go 的 `exec.Command(...).Output()`、Bazel runfile API 和 `os` 操作被拆成 `Runner`、`Bazel`、`Fs` trait；这是为测试引入的结构差异，不改变业务步骤。
- Go 通过 `strings.Builder` 按行拼 JSON；Rust 的 `split_json_objects` 先形成对象字符串，再交给 serde，刷新条件仍是行首 `}`。
- Go 对 map 生成的仓库名排序；Rust 同样先排序 `Vec<String>`，再输出。
- Go `defer os.RemoveAll(tmpdir)` 对应 Rust `TmpDirGuard::drop`；成功和错误路径都清理，清理失败都 panic。
- Go 的包级 bool 由 `flag.BoolVar` 写入；Rust 通过 `parse_flags_checked` 和线程局部 Cell 保留旧参数与提示语义。
- Go 直接向 stdout 调用 `fmt.Print*`；Rust 传入 `dyn Write` 以便测试，但刻意忽略写错误以保持外观一致。
- Go 用 `errors.As(err, *exec.ExitError)` 取 stderr；Rust 依赖 `Error.is_exit`/`stderr` 复制同一 panic 文本。

已知结构差异包括：Rust 将生产入口拆成 `mirror`、`mirror_with`、`run_main_with` 以支持依赖注入；Rust 的 parser runfile 路径仍通过首次字符串替换构造，而非单独调用 `Runfile`，与 Go 保持一致。两份 Rust 测试提供了命名、排序、replace、patch、flags、JSON、错误文本和清理的对齐证据；本目录未发现独立 `*_test.go`，Go 行为依据来自 `mirror.go` 本身及 Rust parity 测试的逐项断言。

## 扩展指南

新增功能时应先判断落点，并同步独立测试，而不是把测试内嵌回生产源文件：

- 新增 Go module JSON 字段：修改 `DownloadedModule` 或 `ListedModule`，确认 serde 键名和缺省行为，并在 `parity_test.rs` 增加串联 JSON 场景。
- 调整列举或下载参数：修改 `list_all_modules`/`download_zips`，保持工作目录和环境叠加语义；在 `ScriptedEnv.commands` 断言参数，尤其关注 `-mod=readonly`、本地/远端 replace。
- 新增仓库级 Bazel 特判：优先放入 `build_file_proto_mode_for_repo`、`dump_build_naming_convention_args_for_repo`、`dump_patch_args_for_repo` 或 `dump_new_deps_bzl` 中对应的显式分支，并在 `parity_test.rs` 固定输出。需要同时评估 repo-name 稳定性以及现有 patch 标签。
- 修改 module path 映射：修改两个名称转换函数，并补覆盖域名倒序、大小写、点、连字符和多段路径的测试；这是高兼容风险修改，因为 Bazel label 与 patch 文件名都依赖结果。
- 改写 JSON parser：必须保留或有意迁移“串联对象”输入，并增加尾部、空行、非法 JSON 和嵌套对象的明确测试，不能只验证 JSON 数组。
- 改变输出模板或排序：更新 `dump_new_deps_bzl` 以及正常路径契约测试，人工检查生成 diff 是否仅含预期变更；性能重点是避免对大型依赖集合引入重复全量扫描，但当前 `O(n log n)` 排序是稳定输出的必要成本。
- 修改清理或错误策略：同步 `contract_error_paths`、`contract_resource_cleanup` 和 `mirror_test.rs`，特别验证成功、业务失败、子进程失败、writer 失败和 cleanup 失败。
- 新增外部能力：把边界加到 `stubs.rs` 的 trait 及生产/脚本实现中，本文件只编排；同步修改不能放进本文件的测试模块，因为仓库要求 Rust 测试与源文件分离。

兼容风险主要是生成文本、Bazel repo name、replace/patch 选择和错误文本漂移；正确性风险主要是下载映射缺失、错误过滤根模块或漏清理；性能风险相对有限，主要来自模块数量增长时的子进程与排序成本。本文件属于构建工具，修改 Go imports、Go 测试入口或 Bazel 元数据时还需遵守仓库的 `bazel_prepare` 触发规则。

## 验证依据

本说明基于以下直接证据：

- `cmd/mirror/mirror.rs`：完整读取 531 行，核对全部结构体、线程局部状态、公开/私有函数、`TmpDirGuard` 及其 `Drop` 实现。
- `cmd/mirror/Cargo.toml`：核对 crate 名称、library/binary 入口、porting metadata 和 `serde`/`serde_json` 依赖。
- `cmd/mirror/bin_main.rs`、`cmd/mirror/lib.rs`：核对生产入口链和独立测试挂接方式。
- `cmd/mirror/stubs.rs`：核对 `Error`、`Flags`、`Bazel`、`Runner`、`Fs` 以及生产/测试实现的真实语义。
- `cmd/mirror/mirror.go`：完整读取 334 行，逐段核对函数顺序、命令参数、JSON 切分、输出例外、错误包装和 defer 清理。
- `cmd/mirror/mirror_test.rs`：核对忽略 writer 错误、保留后续业务错误和 cleanup panic 文本。
- `cmd/mirror/parity_test.rs`：核对命名、proto mode、稳定排序、特殊 build 配置、本地/远端 replace、patch、只读 list 参数、废弃 flags、坏 JSON、ExitError stderr 以及临时目录生命周期。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter cmd/mirror` 定位 7 个相关源文件；目标文件关系显示被 `mirror_test.rs`、`parity_test.rs` 使用；`query` 定位 `Bazel`、`Runner`、`Fs`、`ProdRunner`、`mirror_with`、`dump_new_deps_bzl` 等符号；并对关键入口执行了 `callers`/`callees` 查询，未得到额外文本调用边。

结构验证应保证本文存在且恰有上述十一个固定二级标题。由于任务为纯文档分析，按计划不运行 Cargo；行为结论来自源码、Go 对照、RustCodeGraph 和现有独立测试内容，未在本任务中执行运行时测试。
