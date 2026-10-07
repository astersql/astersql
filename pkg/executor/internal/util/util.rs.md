# `pkg/executor/internal/util/util.rs`

## 文件定位

本文件属于 `astersql-executor-internal-util` crate（见同目录 `Cargo.toml`），实现 executor 内部的测试与调试辅助：生成随机 ASCII 字符串、从调用栈识别调用方，以及递归检查临时目录中是否残留指定前缀的文件。`lib.rs` 以私有模块 `mod util` 装入本文件，再通过 `pub use util::*` 将其公开项从 crate 根导出。

它不参与 SQL 请求的规划或执行数据路径。RustCodeGraph 对本文件公开函数的调用边只指向 `migration_aster_unit_test.rs`；仓库文本检索也未发现其他 Rust 调用方。因此当前 Rust 版本的真实角色是已移植且有独立测试覆盖的辅助 API，尚无证据表明 executor 的 Rust spill 测试或生产逻辑已经接入它。Go 对照文件 `util.go` 的调用者则主要是聚合、排序和 Join 的测试代码。

## 核心职责

- `GenerateRandomString` 从固定的 62 字符字母表中独立抽样，生成调用者指定字节长度的伪随机字符串，适合构造测试数据，不提供密码学安全保证。
- `GetFunctionName` 捕获当前 Rust 调用栈，避开自身和 `backtrace` 内部帧，返回最近的调用方符号名；它用于把测试函数身份转换为临时文件名前缀。
- `CheckNoLeakFiles` 从调用者给出的目录开始递归遍历，发现 basename 以测试前缀开头的任意非目录项时立即报错，用于验证 spill/临时文件生命周期已经收尾。
- `LeakCheckError` 把“目录遍历失败”和“发现泄漏项”区分为可匹配的错误类型，并统一提供关联路径。

这些职责互相配合时形成测试闭环：调用方取得函数名作为唯一前缀，相关逻辑创建带该前缀的临时文件，测试结束后扫描目录确认没有残留。Rust 当前独立测试分别验证这些组件，但仓库中没有发现把三者组合起来的 Rust spill 测试。

## 主要符号

- `pub const LETTER_BYTES: &str`：大小写英文字母与十进制数字组成的 ASCII 字母表。它被公开是为了让测试直接校验移植契约；常量内容与 Go 的私有 `letterBytes` 相同。
- `pub fn GenerateRandomString(length: usize) -> String`：用 `rand::thread_rng()` 取得线程本地随机数生成器；每个输出位置通过 `gen_range(0..letters.len())` 均匀选择一个字节并转成 `char`，最后收集为 `String`。零长度自然得到空串。
- `pub fn GetFunctionName() -> String`：带 `#[inline(never)]`，避免本函数被内联后丢失可识别栈帧。它构造 `backtrace::Backtrace`，收集所有可用符号名，先定位包含 `GetFunctionName` 的自身帧，再从后续帧选取第一个既不包含该名称、也不以 `backtrace::` 开头的名称。无法定位时返回空串。
- `pub enum LeakCheckError`：由 `thiserror::Error` 派生。`Walk(#[from] walkdir::Error)` 保留遍历错误；`LeakedFile { path: PathBuf }` 保存首个匹配项的拥有型路径。
- `pub fn LeakCheckError::path(&self) -> &Path`：泄漏错误直接返回保存的路径；遍历错误尽量返回 `walkdir::Error::path()`，若底层错误没有路径，则返回空路径 `Path::new("")`。
- `pub fn CheckNoLeakFiles(temp_storage_path: impl AsRef<Path>, file_name_prefix_for_test: &str) -> Result<(), LeakCheckError>`：同步、递归、遇错即停的目录检查入口。泛型路径参数允许 `Path`、`PathBuf`、字符串等常见路径表示直接传入。

文件没有 trait、struct、宏定义或条件编译项；唯一的 `impl` 是 `LeakCheckError::path`。命名沿用 Go API 的大驼峰形式，crate 根在 `lib.rs` 中用 lint allow 接受该迁移命名。

## 执行流程

`GenerateRandomString` 的流程是：读取 `LETTER_BYTES` 的字节视图；取得当前线程的随机数生成器；迭代 `0..length`；每次从 `[0, 62)` 选取索引并把对应 ASCII 字节转成字符；收集并返回字符串。没有重试、去重或额外编码步骤。

`GetFunctionName` 的流程是：强制保留独立函数帧；立即捕获回溯；按帧顺序展开每个帧的符号；丢弃没有名称的符号；把符号 demangle 后转成字符串；定位自身帧；只在自身之后查找最近的非自身、非 `backtrace::` 帧；找到则克隆名称，找不到则返回空串。返回值可能包含 Rust 模块路径和哈希等实现相关信息，调用者不应假设它只有裸函数名。

`CheckNoLeakFiles` 的流程是：以输入目录构造 `walkdir::WalkDir`；逐项处理迭代结果；遍历错误通过 `?` 转换为 `LeakCheckError::Walk`；目录项被忽略；其余项把 basename 以有损 UTF-8 形式转换并执行 `starts_with`；首个匹配项转成 `PathBuf` 并返回 `LeakedFile`；完整遍历且没有匹配时返回 `Ok(())`。默认 `WalkDir` 不跟随符号链接，因此符号链接自身作为非目录项接受前缀检查，而不会递归进入其目标。

## 数据与状态

本文件没有全局可变状态。`LETTER_BYTES` 是只读静态字符串；随机数生成器是 `rand` 提供的线程本地句柄；回溯名称、泄漏路径及错误值都由单次调用拥有，调用结束后按 Rust 所有权规则释放。

随机字符串的长度参数是字节数。因为字母表全是单字节 ASCII，结果的字节长度、字符数量和请求长度一致。抽样允许重复，输出也不承诺跨线程或跨运行唯一。

泄漏匹配只看每个条目的 basename，不看完整相对路径；空前缀会匹配遇到的第一个非目录项。文件名用 `to_string_lossy()` 比较，非 UTF-8 字节可能被替换字符表示，因此契约不是原始字节前缀比较。扫描顺序由 `walkdir` 决定，若有多个泄漏，仅报告第一个。

## 依赖与调用关系

crate 边界由 `pkg/executor/internal/util/Cargo.toml` 定义：`backtrace = "0.3"` 服务于栈捕获与符号解析，`rand = "0.8"` 服务于随机抽样，`thiserror = "2"` 生成错误展示与来源转换，`walkdir = "2"` 服务于递归目录遍历；`tempfile = "3"` 仅是独立测试的开发依赖。Cargo metadata 把对应 Go package 标记为 `pkg/executor/internal/util`。

上游方面，`lib.rs` 把本文件公开项再导出为 `astersql_executor_internal_util::{...}`。RustCodeGraph 显示 `GenerateRandomString` 的直接调用者是测试 `random_string_has_requested_length_and_go_alphabet`，`GetFunctionName` 的直接调用者是探针 `function_name_probe`；`CheckNoLeakFiles` 的调用由同一独立测试文件中的泄漏检查用例覆盖。仓库 `rg` 结果没有发现本文件以外的 Rust 生产调用方。

Go 侧的对应 API 使用范围更广：`GenerateRandomString` 用于 `pkg/executor/aggregate/agg_spill_test.go`、`pkg/executor/aggfuncs/aggfunc_test.go` 和 Join 测试辅助；`GetFunctionName`/`CheckNoLeakFiles` 成对出现在 aggregate、sortexec、join 等 spill 测试中。这些 Go 调用点说明 API 的设计意图，但不能作为 Rust 已接线的证据。

## 错误处理与边界

`GenerateRandomString` 对任意 `usize` 长度都尝试分配并生成对应字符串；极大长度可能因内存不足而失败，接口不返回可恢复错误。字母表当前非空，因而 `gen_range` 的区间合法；若未来把 `LETTER_BYTES` 改为空串，生成非空结果会在随机区间处失败。

`GetFunctionName` 把缺失符号、自身帧未被定位、或没有合格后续帧统一降级为空串，不暴露 backtrace 错误。由于栈展开、优化和平台符号信息会影响结果，它只适合诊断与测试命名，不适合业务正确性判断。`#[inline(never)]` 是维持当前策略的重要不变量。

`CheckNoLeakFiles` 对不存在、无权限或遍历期间变化的目录返回 `LeakCheckError::Walk`，不会把扫描失败误判为“无泄漏”。遇到泄漏返回 `LeakCheckError::LeakedFile`；两类错误均可通过 `path()` 读取路径，但底层 walk 错误未携带路径时只能得到空路径。函数不会删除泄漏项，也不会继续汇总所有泄漏。

目录不参与前缀匹配，普通文件和符号链接等非目录项参与。独立 Unix 测试 `leak_check_matches_go_for_prefixed_symbolic_links` 明确验证了带前缀符号链接会被报告；`leak_check_walks_recursively_and_matches_only_prefixes` 验证递归、忽略目录名、忽略不匹配文件及命中路径。

## 并发与资源生命周期

所有 API 都是同步函数，没有锁、通道、异步任务或后台线程生命周期。`GenerateRandomString` 使用线程本地 RNG，因此调用之间不共享本文件管理的可变状态；它并不提供跨线程去重保证。

`GetFunctionName` 在调用期间临时持有完整 `Backtrace` 和派生的名称向量，返回前释放回溯资源，只把选中的名称作为拥有型 `String` 交给调用者。捕获完整栈的成本明显高于读取显式测试名，因而不应放入高频生产路径。

`CheckNoLeakFiles` 在当前线程中惰性遍历目录，每个 `DirEntry` 随循环推进释放；命中时只保留首个路径。它没有对目录树加锁，因此若其他线程或进程同时创建、删除临时文件，结果是扫描时刻的观察值，不能作为强一致的生命周期屏障。安全用法是在被测工作线程已停止、文件句柄已关闭并完成清理后调用。

## 与 Go 版本的对应关系

`LETTER_BYTES` 与 Go `letterBytes` 内容完全一致；两版 `GenerateRandomString` 都逐位置从同一字母表随机抽样。差异是 Go 接受 `int` 且使用包级 `math/rand`，Rust 接受 `usize` 且使用 `rand::thread_rng()`；两者都只适合测试随机数据。

Go `GetFunctionName` 通过 `runtime.Caller(1)` 和 `path.Base` 返回直接调用函数的基名。Rust 因没有相同运行时接口而捕获完整 backtrace，并返回最近合格的 demangle 符号；测试只断言结果包含 `function_name_probe`，没有承诺与 Go 一样只返回 basename。因此依赖精确字符串格式会形成跨平台兼容风险。

Go `CheckNoLeakFiles` 接受 `*testing.T`，从全局配置读取 `TempStoragePath`，先断言该路径与 `t.TempDir()` 同父目录，再通过 `require` 直接令测试失败。Rust 版本改为显式传入扫描根目录并返回结构化 `Result`，没有复制全局配置与父目录约束；调用者负责选择正确目录并处理错误。两版都递归遍历、忽略目录、按 basename 前缀检查所有非目录项。Rust 的 `LeakCheckError` 是为可组合错误处理新增的类型，在 Go 文件中没有直接对应物。

## 扩展指南

扩展随机串行为时，应优先修改 `LETTER_BYTES`/`GenerateRandomString`，并同步更新独立测试 `random_string_has_requested_length_and_go_alphabet`；若要求与 Go 对齐，还必须同步核对 `util.go` 的字母表、长度类型及随机源语义。不要把它升级为安全令牌生成器；需要密码学随机时应新增语义清晰的独立 API。

调整调用方识别时，应保留 `#[inline(never)]` 或提供等效保证，并在 `function_name_probe` 与 `function_name_reports_its_caller` 中增加目标平台允许的稳定断言。若要返回裸函数名，需明确处理模块路径、闭包、测试包装器和符号哈希，并评估与 Go `path.Base` 的兼容性。

扩展泄漏检查时，最合适的接入点是 `CheckNoLeakFiles` 的遍历过滤与 `LeakCheckError` 的错误分类；相关测试必须继续放在独立的 `migration_aster_unit_test.rs`，不要内嵌到生产源文件。新增“跟随链接”“收集全部泄漏”“按原始字节匹配”或“自动删除”等行为都会改变现有边界及安全性，应使用显式选项或新 API，并补充循环链接、权限错误、非 UTF-8 名称、空前缀和并发清理等测试。若 Rust spill 测试开始接线，应在被测工作完成清理后调用，并同时验证扫描根目录来源，避免遗漏 Go 版本的全局临时目录约束。

性能风险主要来自完整 backtrace 捕获和大型目录树的线性遍历；兼容风险主要来自函数名格式、符号链接处理和 Go/Rust 临时目录来源不同。修改公开错误枚举还可能影响下游穷举匹配，应谨慎新增变体。

## 验证依据

- 目标源码：`pkg/executor/internal/util/util.rs`，共 110 行；RustCodeGraph `node --file` 核对了常量、三个公开函数、错误枚举及其 `path` 方法的完整实现。
- crate 装配：`pkg/executor/internal/util/lib.rs` 中的 `mod util` 与 `pub use util::*`；`pkg/executor/internal/util/Cargo.toml` 中的 crate 名、库入口、四个运行依赖、`tempfile` 开发依赖及 Go package metadata。
- 图查询：`rustcodegraph status` 显示索引包含 11,467 个文件且覆盖本目录；`files --filter pkg/executor/internal/util` 列出 `util.rs`、`util.go`、`lib.rs` 和独立迁移测试；`query`/`explore` 确认 Rust 直接调用边包括 `GenerateRandomString -> random_string_has_requested_length_and_go_alphabet`、`GetFunctionName -> function_name_probe`，并把泄漏检查测试定位到同一测试文件。对常见符号名的全库结果存在噪声，因此生产接线结论另用限定后缀的 `rg` 复核。
- Go 对照：`pkg/executor/internal/util/util.go`；Go 使用点由 `rg` 定位到 `pkg/executor/aggregate/agg_spill_test.go`、`pkg/executor/sortexec/*spill_test.go`、`pkg/executor/join/*spill_test.go`、`pkg/executor/aggfuncs/aggfunc_test.go` 等测试文件。
- Rust 独立测试：`pkg/executor/internal/util/migration_aster_unit_test.rs` 中的 `random_string_has_requested_length_and_go_alphabet`、`function_name_reports_its_caller`、`leak_check_walks_recursively_and_matches_only_prefixes`、Unix 条件测试 `leak_check_matches_go_for_prefixed_symbolic_links`。
- 按计划本任务是纯文档分析，没有运行 Cargo 或代码测试；验收采用固定章节结构检查，并人工复核“文件为何存在、如何运行、如何安全扩展”均有源码、图查询、Cargo、Go 对照或独立测试证据。
