# `dumpling/log/lib.rs`

## 文件定位

`dumpling/log/lib.rs` 是 Cargo 包 `astersql-dumpling-log` 的 crate 根，而不是日志算法的实现文件。`dumpling/log/Cargo.toml` 的 `[lib] path = "lib.rs"` 将它指定为库入口；根工作区 `Cargo.toml` 又把 `dumpling/log` 列为成员。该文件通过 `#[path = "log.rs"] mod log;` 装载同目录的真实实现，并用 `pub use log::*;` 把其中公开项提升到 crate 根，因此下游使用的是 `astersql_dumpling_log::{Config, Logger, InitAppLogger, ...}`，无需知道内部模块名。

它位于 Dumpling 逻辑导出工具链的日志基础层。直接 Cargo 依赖者包括 `dumpling/export/Cargo.toml`、`dumpling/cli/Cargo.toml`、`dumpling/context/Cargo.toml` 和 `dumpling/cmd/dumpling/Cargo.toml`；其中运行主链上的明确接线是 `dumpling/export/dump.rs::initLogger` 根据导出配置调用 `InitAppLogger`，再把返回的 `Logger` 放入 Dumpling context。

## 核心职责

本文件只承担三个边界职责：

1. 声明私有实现模块 `log`，把编译单元固定到 `dumpling/log/log.rs`。
2. 用 `pub use log::*` 建立 crate 的完整公开门面。当前被再导出的核心能力包括配置与级别（`Config`、`Level`、`ZapProperties`）、字段与 logger 类型（`Field`、`ZapLogger`、`Logger`），以及构造/访问函数（`InitAppLogger`、`NewAppLogger`、`ShortError`、`Zap`）。公开集合由 `log.rs` 的 `pub` 项决定，`lib.rs` 本身不维护逐项白名单。
3. 仅在 `cfg(test)` 下挂载 `log_test.rs` 与 `parity_test.rs`，保持生产代码和测试代码分文件，并让测试能通过 `crate::...` 检查再导出后的公开契约。

文件顶部的 `#![allow(...)]` 是 crate 级迁移兼容设置：允许 Go 风格的 PascalCase API、暂未使用的移植符号等。它影响整个 crate 的 lint 行为，但不改变日志运行语义。

## 主要符号

- `mod log`：私有模块声明，借助 `#[path = "log.rs"]` 指向实现文件。外部调用者不能以 `astersql_dumpling_log::log::...` 访问它。
- `pub use log::*`：通配再导出，是本文件唯一的生产公开面。实现中的公开符号因此可从 crate 根访问。
- `mod log_test`：`cfg(test)` 条件下编译的独立单元测试模块，重点覆盖 `InitAppLogger` 的路径权限、轮转及终止级别行为。
- `mod parity_test`：`cfg(test)` 条件下编译的 Go/Rust 公开契约测试，覆盖默认值、格式、错误优先级、资源行为和 `Zap` 全局 nop 不变式。
- crate 级 `allow` 属性：允许 `dead_code`、Go 风格命名及未使用项，服务于迁移期 API 形状兼容。

真正的主要公开 API 均定义于 `dumpling/log/log.rs`：`InitAppLogger(&Config) -> Result<(Logger, ZapProperties), String>` 初始化 stdout 或文件 sink；`NewAppLogger(ZapLogger) -> Logger` 包装已有 logger；`Zap() -> Logger` 克隆包级 nop logger；`ShortError(Option<&dyn Display>) -> Field` 只保留错误消息。

## 执行流程

编译与使用流程如下：

1. Cargo 以 `lib.rs` 为 crate 根，应用 crate 级 lint 宽免。
2. 编译器加载私有 `log.rs`，其中定义配置解析、字段编码、logger、文件 sink 与轮转实现。
3. `pub use log::*` 把所有公开实现符号加入 crate 根命名空间。
4. 下游例如 `dumpling/export/lib.rs` 以 `use astersql_dumpling_log::{self as log, Field, Logger};` 导入门面；`dumpling/export/dump.rs::initLogger` 构造 `log::Config` 并调用 `log::InitAppLogger`。
5. `InitAppLogger` 的实现先对非空文件路径执行 `init_file_log` 预检，再解析级别和格式，构造共享 sink 的 `ZapLogger`，把栈信息阈值设为 `DPanic`，最终返回 `Logger` 与实际配置快照 `ZapProperties`。返回值随后通过 `Context::WithLogger` 进入导出上下文，`cli::LogLongVersion` 等调用继续写日志。
6. 测试构建额外加载两个 `cfg(test)` 模块；普通库构建不会编译这些测试模块。

`lib.rs` 自身没有函数调用、分支或循环；以上运行流程来自它暴露的 `log.rs` 实现以及直接下游接线，不能把门面文件理解成又实现了一套 logger。

## 数据与状态

门面文件不拥有运行时数据。状态均由被再导出的实现管理：

- `Config` 保存级别、文件路径、单文件大小、保留天数、备份数与格式；字段名保持 Go 风格。
- `Logger` 包装 `ZapLogger`；`ZapLogger` 用 `Arc<Inner>` 共享不可变配置，并用 `Arc<Mutex<Sink>>` 保护可变输出目的地。由 `with` 派生的 logger 共享同一 sink。
- `Sink` 可为 nop、stdout、轮转文件或测试用内存缓冲；文件 sink 记录路径、字节上限、保留策略及是否已初始化。
- `app_logger()` 用 `OnceLock<Logger>` 惰性创建进程级 nop logger。`Zap()` 只克隆它；`InitAppLogger` 返回新实例，不替换该全局值。
- `ZapProperties` 是初始化结果快照，不持有或控制 logger 生命周期。

由于 `pub use log::*` 是通配再导出，未来在 `log.rs` 增加新的 `pub` 项会自动扩大 crate 的外部 API；这是维护时需要显式评估的兼容面。

## 依赖与调用关系

向下依赖只有编译期模块边 `lib.rs -> log.rs`，以及测试构建下的 `lib.rs -> log_test.rs`、`lib.rs -> parity_test.rs`。`dumpling/log/Cargo.toml` 没有声明第三方 Rust 依赖，当前实现只使用标准库；Go 对照实现则依赖 `github.com/pingcap/errors`、`github.com/pingcap/log` 与 `go.uber.org/zap`。

已由源码和 Cargo 清单核实的上游关系包括：

- `dumpling/export`：直接依赖本 crate；`dump.rs::initLogger` 调用 `InitAppLogger`，并把 logger 注入 `Dumper.tctx`。
- `dumpling/context`：直接依赖本 crate；`context.rs::Background` 调用 `Zap()`，`Context` 保存并克隆 `Logger`。
- `dumpling/cli`：直接依赖本 crate；`versions.rs::LogLongVersion` 接收 `&Logger` 写版本字段。
- `dumpling/cmd/dumpling`：直接依赖本 crate；命令主链的 `DumpSession::L` 和错误处理使用 `Logger`、`Field`。

RustCodeGraph 已索引 `lib.rs` 与 `log.rs`，但对精确符号 `InitAppLogger`、`NewAppLogger`、`ShortError`、`Zap` 执行 `callers`/`callees` 得到空集；结合 `pub use` 门面和源码中存在的直接调用，可判断这是当前图索引对跨 crate 再导出解析的覆盖限制，不能据此声称 API 无调用者。

## 错误处理与边界

`lib.rs` 不产生或转换错误；错误契约由它再导出的 API 决定。`InitAppLogger` 返回 `Result<..., String>`，文件路径预检早于级别和格式解析，因此“目录被当作日志文件”等文件错误会先于非法 level/format 返回。权限错误会规范为包含小写 `permission denied` 的文本；目录路径返回 `can't use directory as log file name`；非法级别包含 `unrecognized level`；当前 Rust 实现仅接受空串/`text`/`json`，其他格式返回 `unsupport log format`。

重要边界还有：空文件名选择 stdout；空 level 视为 `Info`；空 format 视为 `text`；文件模式下 `FileMaxSize == 0` 回落为 300 MB；`ShortError(None)` 产生跳过字段。日志方法为保持 zap 风格签名，不向调用者返回 sink 写入/轮转失败；`Panic` 在记录后 panic，`Fatal` 在记录后以状态 1 退出进程。

`log_test.rs` 的权限用例在有效 UID 为 root 时跳过，因为 root 下 `chmod 000` 不能可靠制造拒绝；相关结论不应外推到非 Unix 权限模型。两个测试模块使用 `std::os::unix::fs::PermissionsExt`，也说明这些权限契约当前面向 Unix。

## 并发与资源生命周期

门面不启动线程、任务或通道。被导出的 logger 可跨 clone 共享：`Arc<Inner>` 管理 logger 生命周期，`Arc<Mutex<Sink>>` 串行化内存/文件 sink 写入及轮转，避免同一 sink 的可变状态被并发无保护访问。锁中毒目前通过 `expect("logger sink poisoned")` 触发 panic，而不是恢复或返回错误。

文件 logger 初始化时会创建缺失的父目录，并以临时空文件探测可写性；探针随后删除，首次真实写入才创建活动日志文件。`FileSink` 在首次写入和轮转时清理过期/超量备份，最后一个共享 logger 被释放时没有专门的 flush/close guard；每条文件日志都通过局部 `OpenOptions` 句柄追加并在调用结束时关闭。stdout/file 写错误被日志方法忽略。

包级 nop logger 由 `OnceLock` 持有到进程结束，初始化线程安全且不可替换。`InitAppLogger` 不修改它，因此通过 `Zap()` 获取的默认 logger 与显式初始化返回值拥有不同生命周期和用途。

## 与 Go 版本的对应关系

`dumpling/log/log.go` 是最近的语义基准。Rust 门面把 `log.rs` 的公开符号提升到 crate 根，对应 Go 包天然的包级导出方式；两个版本都提供 `Logger`、`Config`、`Zap`、`InitAppLogger`、`NewAppLogger` 和 `ShortError`。

主要对应关系：Go 的 `appLogger = Logger{zap.NewNop()}` 对应 Rust `OnceLock` 中的 `Logger::nop()`；Go `Logger` 嵌入 `*zap.Logger` 对应 Rust `Logger { pub Logger: ZapLogger }`；Go 调用 `pclog.InitLogger` 后添加 `zap.AddStacktrace(zap.DPanicLevel)`，Rust 在本地实现同类配置、文件预检、轮转和 `DPanic` 栈阈值；Go `ShortError(nil)` 返回 `zap.Skip()`，Rust `ShortError(None)` 返回 skip `Field`。

需要注意的差异是 Rust 并未链接 Go 的 zap/pingcap-log，而是以标准库实现兼容表面；函数签名也改为 Rust 的 `Result` 和 `Option`。Go `Config` 注释称格式可为 `text`、`json` 或 `console`，当前 Rust `parse_format` 只接受空串、`text`、`json`，所以不能宣称 `console` 已对齐。Go 返回 `*pclog.ZapProperties` 和独立 `error`，Rust 把自有 `ZapProperties` 放进 `Result`。这些差异均应由 parity 测试显式固定或在后续移植中处理。

测试对应上，`dumpling/log/log_test.rs::TestInitLogNoPermission` 对照 `dumpling/log/log_test.go::TestInitLogNoPermission`；Rust 额外的轮转、终止级别测试以及 `parity_test.rs` 扩充了 Go/Rust 契约覆盖，但不改变 Go 文件本身。

## 扩展指南

若新增日志能力，先判断归属：crate 组装、可见性或测试模块接线才修改 `lib.rs`；配置解析、sink、编码、轮转或公开 API 应修改 `log.rs`。由于现有 `pub use log::*` 会自动公开新增 `pub` 符号，新增前要检查命名冲突和下游兼容性；若希望内部使用，保持非 `pub`。

扩展现有公开行为时，应同步独立测试文件而不是把测试写入生产源码：通用 Rust 行为放入 `dumpling/log/log_test.rs`，Go/Rust 契约放入 `dumpling/log/parity_test.rs`；若 Go 基准行为也发生变化，再核对 `dumpling/log/log.go` 与 `dumpling/log/log_test.go`。涉及初始化主链时，还应检查 `dumpling/export/dump.rs::initLogger` 以及 `dumpling/export/main_test.rs`；涉及默认 logger 或上下文传播时检查 `dumpling/context/context.rs` 和 `dumpling/context/parity_test.rs`。

兼容风险主要是公开字段/函数签名、错误文本及错误优先级；正确性风险集中在权限预检、JSON 转义、轮转保留与 `Panic`/`Fatal` 终止顺序；性能风险集中在每条文件日志重新打开文件、全 sink 互斥锁以及备份目录扫描。改变这些路径时需要有针对性的独立回归测试，而不能只验证 crate 能编译。

## 验证依据

- crate 门面与条件编译：`dumpling/log/lib.rs`（`mod log`、`pub use log::*`、`mod log_test`、`mod parity_test`）。
- crate 边界：`dumpling/log/Cargo.toml`；工作区成员和下游依赖由根 `Cargo.toml` 及 `dumpling/{export,cli,context}/Cargo.toml`、`dumpling/cmd/dumpling/Cargo.toml` 核对。
- 真实实现：`dumpling/log/log.rs`，重点符号为 `Config`、`Field`、`ZapLogger`、`Logger`、`app_logger`、`Zap`、`init_file_log`、`InitAppLogger`、`NewAppLogger`、`ShortError`。
- 应用接线：`dumpling/export/lib.rs` 的 crate 导入、`dumpling/export/dump.rs::initLogger`、`dumpling/context/context.rs::Background`、`dumpling/cli/versions.rs::LogLongVersion`、`dumpling/cmd/dumpling/main.rs::DumpSession`。
- Go 对照：`dumpling/log/log.go` 与 `dumpling/log/log_test.go`。
- Rust 独立测试：`dumpling/log/log_test.rs` 与 `dumpling/log/parity_test.rs`；下游使用测试还由 `rg` 定位到 `dumpling/export/main_test.rs`、`status_test.rs`、`dump_test.rs` 及各 crate 的 `parity_test.rs`。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter dumpling/log` 列出 `lib.rs`、实现及测试；`node --file dumpling/log/lib.rs` 确认 31 行门面；`query` 确认 `InitAppLogger`、`NewAppLogger`、`ShortError`、`Zap` 的 Go/Rust 定义位置；这些 Rust 符号的精确 `callers`/`callees` 查询为空，故调用关系改用源码与 Cargo 清单核验并明确记录索引限制。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构检查要求本文恰有上述 11 个固定二级标题。
