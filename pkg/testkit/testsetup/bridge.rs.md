# `pkg/testkit/testsetup/bridge.rs`

## 文件定位

`bridge.rs` 是 `astersql-testkit-testsetup` crate 的公共测试初始化实现。crate 根文件 [`lib.rs`](lib.rs) 通过 `pub mod bridge` 声明该模块，并用 `pub use bridge::*` 将其公开项直接再导出，因此测试 crate 通常以 `astersql_testkit_testsetup::SetupForCommonTest()` 调用，而不需要显式经过 `bridge` 模块。

它位于测试基础设施而非数据库请求执行主链中：调用者主要是各包的 Rust 测试入口或测试夹具，在测试逻辑开始前读取进程环境变量 `log_level` 并设置 `log` facade 的全局 logger 和最大级别。RustCodeGraph 的文件节点将该文件关联到 104 个使用文件；仓库文本检索还可见不同导入别名及调用方式，说明它是跨 planner、executor、session、store、BR 和集成测试复用的进程级设施，而不是某个业务包的局部工具。

crate 边界由 [`Cargo.toml`](Cargo.toml) 确定：库入口是 `lib.rs`，运行时依赖只有 `log = "0.4"`，没有 feature 条件。源文件唯一的条件编译项是末尾 `#[cfg(test)]` 的独立测试模块 `bridge_test.rs`。

## 核心职责

1. `SetupForCommonTest` 提供与 Go `testsetup.SetupForCommonTest` 同名的兼容入口，使迁移后的 Rust 测试能够保留原有初始化调用位置。
2. `apply_os_log_level` 将 `log_level` 环境变量转换为 `log::LevelFilter`；变量未设置、不是 Unicode 或为空时保持现状，合法非空值则更新进程级日志配置。
3. `TextLogger` 为没有其他 logger 时提供写向标准错误的最小文本 logger，并通过 `INSTALL_LOGGER` 保证全局 logger 安装动作至多执行一次。
4. `CONFIGURED_LEVEL` 和 `configured_log_level` 保存、暴露本模块最近一次成功配置的级别，既服务 logger 过滤，也让独立测试验证全局状态变化。
5. `parse_log_level` 对齐 PingCAP/zap 常用级别名，包括大小写不敏感、`warning` 别名，以及把 `dpanic`、`panic`、`fatal` 收敛到 Rust `log` facade 可表达的 `Error`。

该文件只负责日志初始化，不创建存储、session、runtime 或测试数据，也不负责执行测试 harness。调用者仍决定何时以及是否用外层 `Once` 将公共初始化绑定到自己的测试生命周期。

## 主要符号

- `static LOGGER: TextLogger`：进程内静态 logger 实例，满足 `log::set_logger` 所需的 `'static` 生命周期。
- `static INSTALL_LOGGER: Once`：序列化首次安装；`call_once` 让并发或重复调用不会重复执行 `log::set_logger`。
- `static CONFIGURED_LEVEL: AtomicUsize`：以 `LevelFilter` 判别值保存本模块的当前级别，初值为 `Info`。读写使用 `Ordering::Relaxed`，只要求单个原子值可见，不建立其他内存状态的先后关系。
- `struct TextLogger`：私有、无字段的 `log::Log` 实现。`enabled` 比较记录级别与原子阈值，`log` 输出 `"[LEVEL] message"` 到锁定的 stderr，`flush` 刷新 stderr；两处 I/O 错误均被有意忽略。
- `pub enum ApplyLogLevel { Unchanged, Configured(LevelFilter) }`：区分“没有配置请求”和“成功应用某个级别”，避免用布尔值丢失已配置级别。
- `pub struct InvalidLogLevel(String)`：保存无法识别的原始字符串；实现 `Display` 和 `std::error::Error`，错误文本会包含原值。内部字段私有，外部通过错误接口而非直接解构取得信息。
- `pub fn SetupForCommonTest()`：兼容 Go 命名的公开无返回值入口。它调用 `apply_os_log_level`，发生解析错误时写 stderr 并以 `std::process::exit(-1)` 终止进程。
- `pub fn apply_os_log_level() -> Result<ApplyLogLevel, InvalidLogLevel>`：可测试、可组合的配置入口。与顶层入口不同，它将非法值作为 `Result::Err` 返回，不主动退出进程。
- `pub fn configured_log_level() -> LevelFilter`：读取原子判别值并恢复为 `LevelFilter`；明确匹配 `Off` 到 `Debug`，其余值回落为 `Trace`。按本文件写入路径，正常值均来自合法 `LevelFilter`。
- `fn parse_log_level(&str) -> Result<LevelFilter, InvalidLogLevel>`：私有纯解析函数；`bridge_test.rs` 通过同模块测试可见性直接覆盖其别名和大小写行为。
- `mod bridge_test`：仅测试构建启用，通过 `#[path = "bridge_test.rs"]` 保持生产实现与测试源文件分离。

## 执行流程

典型上游路径是包级测试入口调用 `SetupForCommonTest`。例如 `pkg/store/main_test.rs` 直接连续调用两次以检查幂等性；`pkg/planner/util/main_test.rs` 则在自己的 `COMMON_TEST_SETUP.call_once(...)` 中调用它，以模拟 Go `TestMain` 的“一次后再跑测试”生命周期。

`SetupForCommonTest` 的内部流程如下：

1. 调用 `apply_os_log_level`。
2. 后者通过 `std::env::var("log_level")` 读取环境；未设置、读取失败或值为空时立即返回 `Ok(ApplyLogLevel::Unchanged)`，不会安装 logger，也不会重置已有级别。
3. 对非空值调用 `parse_log_level`。解析先转为 ASCII 小写，再映射 `debug`、`info`、`warn`/`warning`、`error`/`dpanic`/`panic`/`fatal`；其他字符串构造 `InvalidLogLevel`。
4. 解析成功后，`INSTALL_LOGGER.call_once` 尝试以 `log::set_logger(&LOGGER)` 安装静态代理。安装结果被忽略，因此若进程已经安装其他 logger，本函数仍继续更新本模块原子级别和 `log` facade 的全局最大级别。
5. 使用原子存储更新 `CONFIGURED_LEVEL`，再调用 `log::set_max_level(level)`，最后返回 `Configured(level)`。后续调用不会替换 logger 实例，但会再次更新有效级别。
6. 若步骤 3 返回错误，`apply_os_log_level` 不修改任何本模块状态；直接调用它的测试可以检查错误。若错误传播到 `SetupForCommonTest`，该兼容入口打印 `applyOSLogLevel failed: ...` 并退出测试进程。

安装成功时，日志记录进入 `TextLogger::log`：先由 `enabled` 读取当前原子阈值，允许的记录以单行文本写到 stderr。`log` facade 自身的 `set_max_level` 还在调用 logger 前提供一层全局过滤。

## 数据与状态

本文件没有堆上长期对象或请求级上下文，状态全部是进程级静态值：

- logger 身份由零大小的 `LOGGER` 固定，安装后活到进程结束。
- “是否执行过安装闭包”由 `INSTALL_LOGGER` 永久记录，不能在同一进程内重置。
- 有效级别由 `CONFIGURED_LEVEL` 保存，初始 `Info`，每次成功处理非空环境值时覆盖；`Unchanged` 和解析错误都不改它。
- `log::set_max_level` 维护 `log` crate 自身的另一份全局状态。代码按“先写本模块原子值、再写 facade 最大级别”的顺序更新，但没有把两次写封装成可原子提交的事务。
- `ApplyLogLevel` 和 `InvalidLogLevel` 是按值返回的瞬时结果；错误拥有原始输入字符串，因此离开环境变量读取作用域后仍可报告。

重要不变量是 `CONFIGURED_LEVEL` 只由成功解析得到的 `LevelFilter` 写入。`configured_log_level` 的 `_ => Trace` 是防御性恢复分支，并不表示公开接口支持写入任意整数。

## 依赖与调用关系

上游方面，`lib.rs` 再导出本模块公开项，各测试 crate 通过 path dependency 引入 `astersql-testkit-testsetup`。RustCodeGraph 对 `bridge.rs` 给出 104 个使用文件；直接检索可见至少 163 个包含相关调用形式的 Rust 文件。代表性调用方式包括：

- `pkg/store/main_test.rs` 和 `pkg/store/driver/txn/main_test.rs` 直接重复调用，验证调用不会因 logger 已安装而冲突。
- `pkg/planner/util/main_test.rs`、`pkg/planner/cardinality/main_test.rs` 以及 `pkg/resourcemanager/pool/spool/spool_test.rs` 用调用方自己的 `Once` 保证测试夹具只初始化一次。
- `tests/readonlytest/main_test.rs`、`tests/graceshutdown/main_test.rs`、`br/pkg/metautil/main_test.rs` 等跨 crate 入口在运行测试前调用兼容 API。

下游方面，`SetupForCommonTest -> apply_os_log_level -> parse_log_level` 是主调用链。成功分支还调用标准库环境读取、`Once::call_once`、原子存储、`log::set_logger` 和 `log::set_max_level`；日志消费路径由 `log` facade 回调 `TextLogger::{enabled, log, flush}`。`configured_log_level` 没有生产调用依赖，只被本 crate 的迁移测试用于观测配置结果。

RustCodeGraph 能精确识别 `SetupForCommonTest`、`apply_os_log_level`、`parse_log_level`、`configured_log_level` 四个节点及签名，但当前 callers/callees 命令对这些名称发生了跨文件同名扩展，不能据其噪声结果断言精确边数；因此上述具体边同时由目标源码与列出的调用入口核验。

## 错误处理与边界

- 环境变量不存在、值不是 Unicode 或为空都被视为“没有覆盖请求”，返回 `Unchanged`。这意味着无效 Unicode 不会作为配置错误暴露。
- 解析大小写不敏感，但不做首尾空白裁剪；例如 `" info "` 会返回 `InvalidLogLevel`。支持集合不包含 `trace` 和 `off`，尽管 `configured_log_level` 能表示这两个 `LevelFilter` 值。
- 非法字符串不会安装 logger，也不会更新原子级别或 facade 最大级别，因为解析发生在所有写操作之前。
- `apply_os_log_level` 保留可恢复错误；`SetupForCommonTest` 则遵循 Go 初始化入口的失败即终止语义。进程退出码使用 `-1`，在具体操作系统上呈现为何值由平台决定。
- `log::set_logger` 的错误被忽略。如果其他组件已先安装 logger，返回值仍是 `Configured(level)`；此时 `CONFIGURED_LEVEL` 只控制本文件的 `TextLogger`，但 `log::set_max_level` 仍会影响已经安装的全局 logger。调用者不能据 `Configured` 推断 `TextLogger` 一定是活动 logger。
- stderr 写入和刷新错误被忽略，日志设施不会因输出设备失败而 panic 或向调用者返回错误。
- 输出只包含级别和格式化消息，不包含时间、模块、目标、源码位置、结构化字段或 Go logger 的 stacktrace 配置。

## 并发与资源生命周期

`INSTALL_LOGGER` 使首次安装闭包线程安全且最多执行一次；`LOGGER` 是静态无状态对象，不需要析构。`CONFIGURED_LEVEL` 的原子读写避免并发数据竞争，重复调用可以安全地替换级别，不会重复注册 logger。调用方若要求“整个初始化逻辑只执行一次”，仍需像 `pkg/planner/util/main_test.rs` 那样维护自己的 `Once`，因为本文件只一次化 logger 安装，不一次化级别更新。

`Ordering::Relaxed` 足以保护独立级别整数的原子性，但不为环境变量修改、`CONFIGURED_LEVEL` 与 `log::set_max_level` 之间建立跨线程事务。并发调用两个不同合法级别时，最终值取决于交错顺序，短时间内本模块阈值与 facade 最大级别也可能不一致。因此测试若修改进程环境必须自行串行化；`migration_aster_unit_test.rs` 使用 `OnceLock<Mutex<()>>` 的 `env_lock` 正是这一约束的直接证据。

每次实际输出临时锁定 stderr，写完即释放；代码不持有后台任务、通道、文件句柄或显式缓冲区。全局 logger 与两个静态同步对象持续到进程结束，没有卸载或复位接口。

## 与 Go 版本的对应关系

同目录 [`bridge.go`](bridge.go) 是直接对照：Go 的 `SetupForCommonTest` 调用私有 `applyOSLogLevel`；后者读取同名 `log_level`，非空时构建文本格式的 PingCAP logger 配置，初始化失败写 stderr 并 `os.Exit(-1)`，成功则用 `log.ReplaceGlobals` 替换全局 logger。Rust 保留了入口名、环境变量名、空值不操作、文本 stderr 方向及顶层失败退出语义。

两者并非完全等价：

- Go 每次成功时通过 `InitLogger`/`ReplaceGlobals` 安装完整 PingCAP/zap logger，并为 fatal 级别配置 stacktrace；Rust 使用最小 `log::Log` 实现，没有结构化字段和 stacktrace。
- Go 是否接受某个级别由 PingCAP/zap 配置解析决定；Rust 显式维护映射，其中 `warning` 大小写别名由 `bridge_test.rs` 验证，`dpanic`/`panic`/`fatal` 都降格映射为 `Error`。
- Go 私有函数不返回结果；Rust 将结果拆成 `ApplyLogLevel`/`InvalidLogLevel`，让测试可以验证 noop、成功和错误，而公开兼容入口仍执行退出策略。
- Go `ReplaceGlobals` 可替换全局 logger；Rust `log::set_logger` 受“一进程一次”约束，所以稳定保留一个代理并仅调整级别，而且无法覆盖先安装的第三方 logger。
- Go 文件带 `//go:build !codes`；Rust crate 没有对应 feature，生产实现始终编译，仅独立测试模块受 `cfg(test)` 控制。

同目录没有 Go `_test.go` 覆盖此文件。Rust 侧以 `bridge_test.rs` 检查解析细节，以 `migration_aster_unit_test.rs` 检查未设置、成功覆盖和非法值三条配置路径，并由多个包级 `main_test.rs` 验证实际接线与重复调用。

## 扩展指南

- 新增级别名或别名应修改 `parse_log_level`，并在独立的 `bridge_test.rs` 增加大小写、别名、非法值和是否接受空白的边界测试；不要把测试嵌回 `bridge.rs`。
- 改变环境变量处理或返回语义应修改 `apply_os_log_level`，同步扩展 `migration_aster_unit_test.rs`。环境测试必须继续使用进程级锁并清理变量，避免并行污染。
- 改变日志格式、目标或 I/O 策略应修改 `TextLogger` 的 `Log` 实现，并评估大量并发测试写 stderr 时的锁竞争、丢弃 I/O 错误是否仍合适，以及与 Go 文本日志的兼容性。
- 若要准确报告 logger 安装冲突，不能继续丢弃 `log::set_logger` 的结果；需要先决定“已有 logger 算成功还是错误”的兼容契约，并为两种安装顺序增加独立测试。当前 `Once` 一旦闭包运行便不会重试，修改时尤其要审查失败后的生命周期。
- 若要支持并发一致的动态级别更新，需要同时考虑本地原子阈值与 `log::set_max_level` 两份状态；只增强原子排序不能自动把两个写操作变成事务。
- 改动公开入口 `SetupForCommonTest` 的命名或失败策略会影响大量测试 crate 和 Go 对齐，应优先保留兼容门面，把可恢复的新行为放在 `apply_os_log_level` 或新的公开函数中。
- 添加生产依赖时要同步 `pkg/testkit/testsetup/Cargo.toml`；当前 crate 的低依赖面使它适合作为广泛测试依赖，重量级依赖会扩大大量测试目标的构建成本。

## 验证依据

- 目标实现：`pkg/testkit/testsetup/bridge.rs`，核对了三个静态状态、`TextLogger` 的 `Log` 实现、两个公开结果类型、三个公开函数、私有解析函数及 `cfg(test)` 模块声明。
- crate 边界：`pkg/testkit/testsetup/lib.rs` 与 `pkg/testkit/testsetup/Cargo.toml`，确认模块公开再导出、库入口、唯一 `log` 依赖、无 feature，以及 Go 包迁移元数据。
- Go 对照：`pkg/testkit/testsetup/bridge.go`，核对 `SetupForCommonTest -> applyOSLogLevel`、`log_level`、文本 logger、错误输出、退出和全局替换行为；目录检索确认没有同路径 Go 测试文件。
- Rust 测试：`pkg/testkit/testsetup/bridge_test.rs` 验证 `warning` 别名与大小写；`pkg/testkit/testsetup/migration_aster_unit_test.rs` 验证 unset noop、`debug` 覆盖和非法值错误，并展示环境变量串行锁。
- 上游样本：`pkg/store/main_test.rs`、`pkg/store/driver/txn/main_test.rs` 验证重复调用；`pkg/planner/util/main_test.rs` 验证调用方用 `Once` 模拟包级初始化。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件和 307,296 个节点；`files --filter pkg/testkit/testsetup` 覆盖目标、Go 对照、crate 根及两个 Rust 测试；目标文件节点显示 136 行、22 个符号和 104 个使用文件；精确 `query` 确认四个核心函数节点及签名。callers/callees 对同名符号产生跨文件噪声，具体调用边因此以源码和入口检索交叉核验。
- 结构验证以任务指定命令检查文档存在且恰有十一个固定二级标题。该任务只生成文档，按计划没有运行 Cargo、构建或代码测试。
