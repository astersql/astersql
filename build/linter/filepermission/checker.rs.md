# `build/linter/filepermission/checker.rs`

## 文件定位

`checker.rs` 是 `build/linter/filepermission/checker.go` 的 Rust 迁移草稿，目标是表达名为 `filepermission` 的 Go 源文件权限检查器。源码第 16--19 行已经明确标注：它保留 Go 实现结构，但当前不保证可编译，也没有真正接入 Go analysis 框架。

当前仓库的实际接线仍在 Go/Bazel 一侧：`build/linter/filepermission/BUILD.bazel` 只把 `checker.go` 声明为公开 `go_library`，依赖 `//build/linter/util` 和 `golang.org/x/tools/go/analysis`。根 `Cargo.toml` 的 workspace 成员中没有 `build/linter/filepermission`，该目录也没有自己的 `Cargo.toml` 或 Rust 模块入口。因此这个 Rust 文件不是现有 Cargo 产品构建的一部分；RustCodeGraph 显示它只被 `build/linter/filepermission/checker_test.rs` 使用，后者通过 `include!("checker.rs")` 编译检查其中一部分逻辑。

## 核心职责

文件表达三个职责，分别对应 `Name`、`Analyzer`/`run` 和 `init`：

1. 用 `Name = "filepermission"` 提供分析器的稳定名称，并让诊断前缀与 `build/nogo_config.json` 中的 `filepermission` 配置键一致。
2. `run` 遍历 analysis pass 中的 Go 语法文件，把语法位置还原为磁盘文件名，读取文件元数据，并在 Unix 上发现用户、组或其他人的任一执行位时报告诊断；它只检查，不修改权限。
3. `init` 将静态 `Analyzer` 交给 `util::SkipAnalyzerByConfig`，表达与 Go 版本相同的按配置跳过语义。

此外，Rust 版本新增 `formatGoFileMode`，用于把 Unix `st_mode` 渲染成 Go `os.FileMode.String()` 风格的符号串，使诊断文本能够保留 Go 版本打印 `stat.Mode()` 时的可读形式。

## 主要符号

- `pub const Name: &str = "filepermission"`：分析器名称，也是诊断消息的方括号前缀。它对应 Go 的 `const Name`。
- `pub static Analyzer: analysis::Analyzer`：静态分析器描述，字段为 `name`、`doc`、空依赖集合 `requires` 和回调 `run`。与 Go 的指针变量不同，Rust 这里保存值，并在 `init` 中取 `&Analyzer`。
- `pub fn formatGoFileMode(mode: u32) -> String`（仅 `cfg(unix)`）：先根据 `0o170000` 文件类型掩码生成类型前缀，再按 `setuid`、`setgid`、字符设备和 sticky 位附加 `u/g/c/t`，最后依次输出九个 `rwx` 权限字符。普通文件没有特殊前缀时用 `-`，字符设备则组合为 `Dc`。该函数是 Rust 为保持 Go 诊断格式而拆出的辅助函数。
- `pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn Any>>, analysis::Error>`：分析器主回调。成功时总是返回 `Ok(None)`；读取元数据失败时由 `?` 立即返回错误。
- `pub fn init()`：调用 `util::SkipAnalyzerByConfig(&Analyzer)` 的配置接线函数。Rust 本身不会像 Go 那样自动执行名为 `init` 的普通函数；在真正建立 Rust 模块/注册机制前，它只是一个可显式调用的公开函数。

文件没有自定义 struct、enum、trait、宏或可变全局状态。`analysis::Analyzer`、`analysis::Pass`、`analysis::Error` 和 `util` 都是假定由外部模块提供的接口，并未在生产 Rust crate 中由本文件定义。

## 执行流程

以 `run` 为入口，单个 pass 的处理顺序如下：

1. 用索引遍历 `pass.Files`，先复制当前文件的 `Pos()` 到 `file_position`。这样后续调用可变的 `pass.Reportf` 时不会继续持有 `pass.Files` 的不可变借用。
2. 调用 `pass.Fset.PositionFor(file_position, false).Filename` 获取未按行指令调整的真实文件名。
3. 空文件名直接跳过，不访问文件系统，也不产生诊断。
4. 对非空文件名调用 `std::fs::metadata`。失败通过 `?` 中止整个 pass；前面已经报告的诊断不会由本函数回滚。
5. Unix 构建中通过 `PermissionsExt::mode()` 取得模式位，并用 `mode & 0o111` 检查三类执行位。结果为零时继续下一个文件。
6. 命中执行位时调用 `formatGoFileMode`，再通过 `pass.Reportf(file_position, ...)` 报告 `[filepermission] source code file should not have execute permission <mode>`。
7. 所有文件处理完后返回 `Ok(None)`，表示分析器没有额外结果对象。

非 Unix 构建会保留遍历、文件名解析和 `metadata` 调用，但权限掩码与报告块被条件编译移除，因此不会产生权限诊断。

## 数据与状态

主要输入状态来自可变借用的 `analysis::Pass`：`Files` 提供语法文件及位置，`Fset` 把位置映射到磁盘路径，`Reportf` 收集诊断。文件系统元数据是每次运行即时读取的外部状态，检查结果可能受检查期间文件被替换、删除或 chmod 的影响。

`Analyzer` 和 `Name` 是只读静态数据；`formatGoFileMode` 只在局部 `String` 上构造结果，没有缓存。`run` 不修改源文件、不调用 chmod、不保存跨 pass 状态，也不返回分析结果。配置排除项位于 `build/nogo_config.json`，其中包括 parser 生成文件、external、`.cgo`、生成文件、mock 和 testmain 等路径；具体过滤行为由 `util::SkipAnalyzerByConfig`/analysis 框架承担，不在本文件实现。

## 依赖与调用关系

RustCodeGraph 对该文件识别出 4 个符号，并确认直接内部调用边为 `run -> formatGoFileMode`。索引的文件级关系显示唯一 Rust 使用者是 `build/linter/filepermission/checker_test.rs`；未发现生产 Rust 调用者。由于 `run`、`init` 名称普遍存在，图查询会产生同名歧义，因此不能据此声称存在其他生产调用边。

下游依赖可由源码直接核对：

- `run -> analysis::Pass::{Files,Fset,Reportf}`：取得文件位置、解析路径和报告诊断。
- `run -> std::fs::metadata`：读取路径元数据；Unix 上再依赖 `std::os::unix::fs::PermissionsExt::mode`。
- `run -> formatGoFileMode`：只在检测到执行位的 Unix 分支调用。
- `init -> util::SkipAnalyzerByConfig`：表达按仓库配置排除 analyzer 的注册步骤。
- `Analyzer -> run`：静态描述的回调字段直接保存函数指针。

Go 生产链的 crate 等价边界并不存在：`BUILD.bazel` 只构建 `checker.go`，而根 `Cargo.toml` 没有覆盖本目录。因此 `analysis` 和 `util` 在当前 Rust 生产构建中没有已验证的 canonical crate 来源；测试文件为了验证草稿，局部声明了同名桩模块和最小 API。

## 错误处理与边界

- 空文件名是显式容忍边界：该项被静默跳过，与 Go 版本一致。
- `metadata` 的任意 I/O 错误（不存在、无权限、路径竞态等）通过 `?` 立即传播为 `analysis::Error`；不会降级为诊断，也不会继续检查余下文件。独立 Rust 测试用源码断言锁定了这一传播形态。
- 判断条件严格为任一执行位 `0o111`，不把 setuid、setgid 或 sticky 位本身视为执行权限；这些特殊位只影响诊断中的格式化文本。
- `metadata` 跟随符号链接，因此检查的是目标对象的权限，不是链接本身。源码没有专门处理链接竞态或非普通文件；`formatGoFileMode` 反而覆盖目录、符号链接、块/字符设备、管道、socket 和未知类型，保证异常输入仍可稳定显示。
- 非 Unix 平台没有同构权限检查实现，当前行为是仍读取元数据但不报告。这是 Rust 草稿相对 Go 跨平台 `os.FileMode` 逻辑的明确限制。
- `formatGoFileMode` 接受原始 `u32`，不会验证文件类型位组合是否合法；未知类型输出 `?`，其余权限位照常追加。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或长期句柄。`run` 同步且顺序地逐文件调用 `metadata`；文件数为 `n` 时是 `O(n)` 次路径解析和系统调用，内存开销除诊断字符串外为常数级。若外部 analysis 框架并发运行多个 pass，共享的 `Name`/`Analyzer` 都是只读的，本文件没有内部共享可变状态；但 `analysis::Pass` 的并发安全性属于尚未接线的外部 API，当前无法由此文件验证。

每次元数据结果只存于循环当前迭代，离开迭代后释放；没有打开文件描述符需要显式关闭。诊断字符串在 `Reportf` 调用点临时创建，其是否被复制或保留由 `analysis::Pass` 的真实实现决定，测试桩不会保存它。

## 与 Go 版本的对应关系

`build/linter/filepermission/checker.go` 是当前语义基准：`Name`、analyzer 文档、逐文件遍历、`PositionFor(..., false)`、空路径跳过、`os.Stat` 错误返回、`Mode() & 0111` 判断、同位置报告和最终空结果，都在 Rust 中逐项保留。

已确认的差异如下：

- Go 的 `Analyzer` 是 `*analysis.Analyzer`，Rust 草稿是静态值，并额外显式给出空 `requires`。
- Go `os.FileMode.String()` 隐式完成模式渲染；Rust 增加 `formatGoFileMode` 模拟该格式。
- Go 的权限位抽象可跨目标平台表达；Rust 权限读取和诊断受 `cfg(unix)` 限制，非 Unix 当前不报告。
- Go 包级 `init()` 会由运行时自动执行；Rust 的 `init` 只是普通公开函数，尚无模块注册者调用它。
- Go 实现由 Bazel 目标构建并可被现有 nogo 链使用；Rust 文件没有 Cargo crate/模块接线，只在独立测试的桩环境中被 `include!`。

同目录没有 Go `checker_test.go`。现有直接回归证据是 `checker_test.rs`：它在 Unix 上实际编译并测试模式字符串，还以源码断言固定 analyzer 字面量、回调结果类型、错误传播、执行位掩码和报告消息结构。

## 扩展指南

若只调整检查规则，应优先修改 `run`，并同步 `build/linter/filepermission/checker_test.rs`：新增边界应覆盖空路径、元数据失败、三类执行位和不应命中的特殊位。若改变诊断中的权限格式，应修改 `formatGoFileMode` 并扩充表驱动用例，尤其关注特殊文件类型、setuid/setgid/sticky 和字符设备 `Dc` 前缀；同时必须与 Go `os.FileMode.String()` 的输出重新对照。

若要让它成为真正可用的 Rust analyzer，不能只在本文件增加代码：需要先建立明确的 Cargo crate 与模块入口，提供非测试桩的 `analysis`/`util` API，实现 analyzer 注册和配置排除机制，并决定非 Unix 语义。完成这些接线后再把 `init` 接到真实生命周期，并用独立测试文件验证真实诊断，而不是把测试写回生产源文件。由于当前 Go/Bazel 目标仍是生产实现，任何语义调整还应同步评估 `checker.go`、`BUILD.bazel` 和 `build/nogo_config.json`，避免两个版本漂移。

兼容风险集中在诊断文本（可能被工具或 golden 输出依赖）、路径到文件系统的竞态，以及不同平台的权限表示；性能风险主要来自每个文件一次同步 `metadata`，不应在格式化辅助函数中引入额外 I/O 或全局锁。

## 验证依据

- RustCodeGraph 索引状态：仓库索引包含 7,032 个 Rust 文件；`files --filter build/linter/filepermission` 返回 `checker.go`、`checker.rs`、`checker_test.rs`。
- RustCodeGraph 文件节点：`node --file build/linter/filepermission/checker.rs` 识别 `Name`、`Analyzer`、`formatGoFileMode`、`run`、`init` 所在源码，并报告该文件由 `checker_test.rs` 使用；图中可确认 `run -> formatGoFileMode`。
- 生产源码：`build/linter/filepermission/checker.rs`（全部 130 行）和 Go 对照 `build/linter/filepermission/checker.go`。
- 独立测试：`build/linter/filepermission/checker_test.rs`，包含真实 Unix 格式化用例及对回调签名、错误传播、执行位条件和报告文本的源码约束。
- 构建与配置：根 `Cargo.toml`、`build/linter/filepermission/BUILD.bazel`、`build/nogo_config.json`；它们分别证明 Rust workspace 未接入本目录、Go 库的真实依赖边界及 analyzer 排除配置。
- 人工复核结论：该文件存在的目的，是为 Go filepermission analyzer 保存 Rust 迁移语义；其当前可执行证据仅限测试桩环境，不能描述为已接入生产 Rust 架构。
