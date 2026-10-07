# `dumpling/log/log.rs`

## 文件定位

本文件是 Cargo 包 `astersql-dumpling-log` 的核心实现，对应 Go 包 `dumpling/log` 的 `log.go`。`dumpling/log/lib.rs` 以 `#[path = "log.rs"] mod log` 引入它并通过 `pub use log::*` 暴露公开 API；`dumpling/log/Cargo.toml` 将该包声明为 `library`，入口为 `lib.rs`，移植元数据指向 Go 包 `dumpling/log`。该 crate 没有第三方 Cargo 依赖，日志级别、编码、输出、轮转及保留策略均由本文件用标准库实现。

在 Dumpling 运行链中，`dumpling/export/dump.rs::initLogger` 把导出配置转换为本文件的 `Config`，调用 `InitAppLogger`，再把返回的 `Logger` 放入 Dumpling context；如果调用方已经提供 logger，则该入口不会重复初始化。`dumpling/context/context.rs::Background` 则通过 `Zap()` 为背景上下文安装包级 nop logger。因此本文件既是应用 logger 的构造边界，也是 Dumpling 各子 crate 共享的日志类型与字段协议。

## 核心职责

- 用 `Config`、`Level` 和 `ZapProperties` 表达与 Go `dumpling/log`/`pingcap/log` 对齐的配置输入和生效配置快照。
- 用 `ZapLogger` 和外层 `Logger` 提供 zap 风格的级别过滤、固定字段、文本/JSON 编码、栈位置附加以及 `Debug` 至 `Fatal` 的调用接口。
- 用 `Sink`/`FileSink` 统一 nop、stdout、文件和测试内存输出，并为文件输出实现懒创建、按大小轮转、按数量或修改时间清理备份。
- 在 `InitAppLogger` 中提前验证文件路径和权限，保持与 Go `pingcap/log` 相同的错误优先级和关键错误文本。
- 通过 `Zap` 提供不会被初始化过程替换的包级 nop logger，通过 `NewAppLogger` 包装既有 `ZapLogger`，通过 `ShortError` 生成只含错误消息的结构化字段。

## 主要符号

- `Config`：公开配置结构。`Level`、`File`、`FileMaxSize`、`FileMaxDays`、`FileMaxBackups`、`Format` 分别控制阈值、目标文件、单文件大小、保留天数、备份数量和编码格式。零值级别/格式分别解释为 `Info`/`text`；仅文件模式下 `FileMaxSize == 0` 会回落为 `DEFAULT_LOG_MAX_SIZE`（300 MB）。
- `Level` 与 `Level::parse`：公开的 zap 兼容级别枚举及解析器。枚举顺序同时是过滤顺序；无法识别的文本返回 `unrecognized level` 错误。私有 `capital` 生成编码所用的大写名称。
- `Field`：字符串键值字段，私有 `skip` 标志模拟 `zap.Skip()`；`skip`、`string`、`is_skip` 分别负责构造占位字段、普通字段和查询跳过状态。
- `ZapProperties`：记录最终生效的级别、文件名、格式和轮转参数，不持有输出资源。
- `Sink`、`FileSink`：内部输出状态。`FileSink::{write_line,rotate,next_backup_path,remove_expired_backups}` 处理单条大小检查、文件轮转、备份命名和保留清理。
- `Inner`、`ZapLogger`：实际 logger 状态及其共享句柄。`Inner` 保存阈值、格式、栈位置阈值、固定字段和共享 sink；`ZapLogger` 的克隆共享 `Arc<Inner>`，而 `with`/`with_stacktrace_at` 新建元数据并继续共享 sink。
- `Logger`：公开包装器，字段 `Logger: ZapLogger` 以及 PascalCase 日志方法保持 Go 嵌入 `*zap.Logger` 的调用形态。
- `app_logger`、`Zap`：`OnceLock<Logger>` 惰性建立包级 nop logger，并按值克隆返回；本文件没有替换该全局值的 API。
- `InitAppLogger`：配置初始化主入口，返回 `Result<(Logger, ZapProperties), String>`。
- `NewAppLogger`：把调用方已有的 `ZapLogger` 包装为公开 `Logger`。
- `ShortError`：`Some(error)` 生成键为 `error` 的消息字段，`None` 返回 skip 字段，不记录空错误。

## 执行流程

1. `dumpling/export/dump.rs::initLogger` 在未注入 logger 时组装 `Config`，调用 `InitAppLogger`；成功后将 logger 写入 context，并用它输出版本信息。
2. `InitAppLogger` 若配置了 `File`，先调用 `init_file_log`。该函数创建父目录，拒绝把目录当作日志文件，探测已有文件的追加权限；新文件则创建后立即删除探针文件，留待首次日志写入创建。此步骤先于级别和格式解析，因此文件错误具有更高返回优先级。
3. `Level::parse` 和 `parse_format` 校验级别及 `text`/`json` 格式。文件模式创建 `Sink::File(FileSink)`；空文件名创建 `Sink::Stdout`。随后构造 `ZapProperties`，并以 `DPanic` 作为栈位置附加阈值创建 `ZapLogger`。
4. `Logger::{Debug,Info,Warn,Error,DPanic,Panic,Fatal}` 转发到同名 `ZapLogger` 方法，最终进入私有 `log`。`log` 先做级别过滤，再合并固定字段与本次字段并排除 skip 字段，编码为文本行或 JSON 对象；达到栈阈值时追加调用位置，最后在 sink 锁内写出。
5. 文件 sink 的 `write_line` 拒绝大于单文件上限的单条记录；必要时调用 `rotate`。轮转把活动文件重命名为带纳秒时间戳的备份，重建活动文件并保留原权限，然后调用 `remove_expired_backups` 按数量或年龄删除备份。
6. `DPanic` 在此生产 logger 中只记录；`Panic` 先记录再触发 unwind；`Fatal` 先记录再以状态码 1 结束进程。

## 数据与状态

包级状态只有 `app_logger` 内的 `OnceLock<Logger>`，首次读取时固定为 `Logger::nop()`，`InitAppLogger` 不会修改它。由 `Zap()` 返回的克隆因此始终指向 nop 行为，这一点由 `dumpling/log/parity_test.rs::contract_resource_cleanup` 明确验证。

每个实际 logger 的不可变配置位于 `Arc<Inner>`。派生 logger 可以有不同的固定字段或栈阈值，但 `Arc<Mutex<Sink>>` 被继续共享，因此从 `with` 派生的 logger 与原 logger 写入同一输出流。`Sink::Memory(Vec<String>)` 仅由 `ZapLogger::capture` 构造，供独立测试通过 `entries` 观察输出；生产初始化只选择 stdout 或文件。

`FileSink::initialized` 区分首次写入与后续写入：首次写入前执行一次保留清理，并在探针文件已删除的情况下懒创建活动文件。备份文件通过同目录下的“原 stem-纳秒时间戳.原扩展名”识别；清理同时受 `max_backups` 和 `max_days` 约束，任一条件超限即删除。

## 依赖与调用关系

RustCodeGraph 将 `InitAppLogger` 的直接下游识别为 `Level::parse`、`parse_format`、`init_file_log`、`FileSink::new` 和 `ZapLogger::new`。图索引没有解析出该公开函数的跨 crate 调用者；源码搜索补足的生产调用边为 `dumpling/export/dump.rs::initLogger -> astersql_dumpling_log::InitAppLogger`，以及 `dumpling/context/context.rs::Background -> astersql_dumpling_log::Zap`。

`dumpling/export/Cargo.toml`、`dumpling/context/Cargo.toml`、`dumpling/cli/Cargo.toml` 和 `dumpling/cmd/dumpling/Cargo.toml` 均以路径依赖引用 `astersql-dumpling-log`。其中 `dumpling/cli/versions.rs` 使用 `Field`/`Logger` 输出版本信息，`dumpling/cmd/dumpling/main.rs` 使用相同公开类型，说明该 crate 同时承担跨子 crate 的日志类型边界。

文件内部仅依赖 `std::fmt`、`std::fs`、`std::io`、`std::path`、`std::sync` 和 `std::time`。关键内部链路为 `Logger::* -> ZapLogger::* -> ZapLogger::log -> Sink`；文件分支进一步调用 `FileSink::write_line -> rotate/remove_expired_backups`。

## 错误处理与边界

初始化错误通过 `Result<_, String>` 返回。`init_file_log` 保留不同阶段的上下文前缀：父目录创建失败为 `cannot create log directory`，目录被用作文件名为 `can't use directory as log file name`，已有文件不可写为 `can't write to log file`，新文件不可创建为 `can't create log file`，其他元数据错误为 `error checking log file`；权限错误统一映射为小写 `permission denied` 以满足 Go 兼容断言。

格式只接受空串、`text` 和 `json`。尽管 Go `Config` 注释列出 `console`，Go 实际编码器路径的兼容约束在此实现为仅支持 `text/json`；其他值返回 `unsupport log format`。JSON 编码由 `json_escape` 转义引号、反斜杠、常见控制字符及其他控制码；字段值均按字符串编码。

日志写入接口不返回 sink 错误：stdout、文件写入及轮转失败会被忽略，符合 zap 日志方法不把输出错误返回业务调用方的接口形态，但也意味着磁盘满、权限在初始化后变化等故障不会反馈给业务流程。sink mutex 中毒会在 `entries` 或 `log` 中通过 `expect("logger sink poisoned")` 触发 panic。`Panic` 和 `Fatal` 是刻意的终止型 API，扩展调用点时必须避免把普通错误路径升级为进程终止。

## 并发与资源生命周期

`Logger`/`ZapLogger` 的克隆基于 `Arc`，所有共享 sink 的写入均由 `Mutex<Sink>` 串行化，因此同一 logger 家族的记录和文件轮转不会并发修改同一 `FileSink`。配置、固定字段和阈值在构造后不可变；`with` 复制字段列表，不修改父 logger。

文件初始化只做权限探测，不长期持有打开的 `File`。每次 `write_line` 都以 append 方式打开活动文件，写入结束即释放句柄；轮转时先重命名，再创建并关闭新文件，随后恢复权限。stdout 同样在单次调用中取得句柄。此实现没有后台线程、异步任务、通道或显式 flush guard；`ZapProperties` 也不是资源守卫。`Fatal` 的同步保证仅来自退出前完成当前 `log` 调用，相关行为由子进程测试验证。

## 与 Go 版本的对应关系

`dumpling/log/log.go` 定义的公开面为 `Logger`、`Config`、`Zap`、`InitAppLogger`、`NewAppLogger` 和 `ShortError`，Rust 文件保留了这些名称与核心语义：默认全局 logger 为 nop；初始化委托的语义在 Rust 内部重建；初始化 logger 附加 `DPanic` 起始的栈信息；`ShortError(nil/None)` 生成跳过字段，非空值只记录消息。

两版实现机制不同。Go 通过 `pingcap/log.InitLogger`、`zap.Logger` 和其 lumberjack 输出间接获得编码与轮转；Rust 以 `ZapLogger`、`Sink` 和 `FileSink` 自行实现这些行为。Go 错误使用 `errors.Trace` 保留包装链，Rust 返回字符串。Go 的 logger 字段嵌入指针，Rust 使用公开字段加转发方法。Rust 额外公开 `Level`、`Field`、`ZapProperties`、捕获 logger 和若干观察方法，以支撑已迁移调用方及独立测试。

`dumpling/log/log_test.go::TestInitLogNoPermission` 与 `dumpling/log/log_test.rs::TestInitLogNoPermission` 对齐了可写基线、无权限目录、只读子目录、目录作文件名、只读文件和嵌套父目录创建。Rust 还在 `log_test.rs` 验证轮转、`DPanic`/`Panic`、`Fatal`，并在 `parity_test.rs` 验证配置错误优先级、JSON 转义、共享 sink、默认值、全局 nop 不变及首次写入创建文件。

## 扩展指南

- 增加配置字段时，首先修改 `Config`，再把其生效值接入 `InitAppLogger`/`ZapProperties`；若影响文件策略，还需修改 `FileSink`。同时核对 `dumpling/log/log.go` 的公开契约以及 `dumpling/export/dump.rs::initLogger` 是否需要从应用配置传入新值。
- 增加格式或字段类型时，修改 `parse_format` 和 `ZapLogger::log` 的编码路径，并同步处理 `json_escape` 或引入的新编码边界；需要在独立的 `dumpling/log/parity_test.rs` 中覆盖 Go/Rust 公共契约，在 `dumpling/log/log_test.rs` 中覆盖 Rust 特有边界，不能把测试内嵌进本文件。
- 调整轮转或保留策略时，修改 `FileSink::{write_line,rotate,next_backup_path,remove_expired_backups}`，重点保持单条超限、恰好达到阈值、首次写入、权限保留、备份排序和数量/年龄联合清理语义。该路径位于每条文件日志的锁内，目录扫描或清理扩张会直接影响写日志延迟。
- 新增公开日志方法时，应同时在 `ZapLogger` 和 `Logger` 提供一致转发，明确其是否终止进程，并补充独立测试。不要通过 `InitAppLogger` 隐式替换 `Zap()` 的全局 nop 值，否则会破坏 Go 当前的包级状态语义和 context 默认行为。
- 兼容性风险集中在公开 PascalCase 名称、错误文本、初始化校验顺序、`DPanic` 阈值和 `ShortError(None)` 的 skip 语义；性能风险集中在每条文件日志重新打开文件、全局 sink mutex、轮转时同步扫描目录和同步删除备份。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter dumpling/log` 确认 `lib.rs`、`log.rs`、`log_test.rs`、`parity_test.rs` 及 Go 对照均在索引中；`node --file dumpling/log/log.rs` 核对了完整 811 行实现；`query`/`callees` 核对 `InitAppLogger`、`Zap`、`NewAppLogger`、`ShortError` 及初始化下游边。跨 crate callers 查询为空，故按技能规则以源码搜索补足调用者证据。
- 实现与 crate 边界：`dumpling/log/log.rs`、`dumpling/log/lib.rs`、`dumpling/log/Cargo.toml`。
- 生产入口与直接使用者：`dumpling/export/dump.rs::initLogger`、`dumpling/context/context.rs::Background`、`dumpling/cli/versions.rs`、`dumpling/cmd/dumpling/main.rs`，以及相应子 crate 的 `Cargo.toml` 路径依赖。
- Go 对照：`dumpling/log/log.go`；Go 权限行为测试：`dumpling/log/log_test.go::TestInitLogNoPermission`。
- 独立 Rust 测试：`dumpling/log/log_test.rs`（权限、轮转、终止级别）与 `dumpling/log/parity_test.rs`（公开契约、错误顺序、JSON、共享 sink、资源与全局状态）。本任务是纯文档分析，按计划未运行 Cargo；上述测试用作静态行为证据，而非本次执行结果。
