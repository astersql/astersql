# `pkg/lightning/log/log.rs`

## 文件定位

本文件是 `astersql-lightning-log` crate 的日志门面与运行时实现。crate 入口 `pkg/lightning/log/lib.rs` 将本模块声明为 `pub mod log` 并再导出其公共 API；相邻的 `filter.rs` 提供 `Core`、`Entry`、`Field`、`Level`、`FilterCore` 和 JSON 编码，本文件负责把这些原语组合成 Lightning 风格的 Logger、全局日志状态、初始化流程和任务计时接口。

`pkg/lightning/log/Cargo.toml` 指定 `lib.rs` 为库入口，生产依赖只有启用 `preserve_order` 的 `serde_json`（实际由 `filter.rs` 使用），并以 `package.metadata.porting.go-package = "pkg/lightning/log"` 标明 Go 对照包。工作区及 `pkg/lightning/common`、`pkg/lightning/duplicate`、`pkg/executor`、`pkg/importsdk`、若干 DXF/ingestor crate 通过路径依赖接入该 crate；已确认的直接 Rust 业务调用包括 `pkg/lightning/duplicate/detector.rs` 和 `worker.rs` 的 `Logger::Begin`/`Task::End`。

## 核心职责

1. `Config::Adjust` 补齐日志级别与滚动参数默认值，并兼容把 `warning` 规范成 `warn`。
2. `Logger` 在抽象 `Core` 之上提供 Debug/Info/Warn/Error、固定字段、层级名称及任务起始日志；所有级别方法都经 `Logger::log` 做级别短路、调用位置采集和写出。
3. `InitLogger` 根据配置建立 stdout 或追加文件输出，按诊断开关决定是否套用包路径白名单，并安装进程级 Logger 与可动态调整的级别。
4. `Task` 将“开始—结束—耗时—错误”固化为统一协议，区分成功、取消和普通失败。
5. `ShortError` 与 `IsContextCanceledError` 提供错误字段压缩和取消分类，避免中间层重复记录堆栈，并让取消日志降级为 Debug。

本文件不实现日志滚动。`Config` 保留 `FileMaxSize`、`FileMaxDays`、`FileMaxBackups` 以对齐 Go 配置形状，但当前 Rust `OutputCore::write` 仅用 `OpenOptions::append` 写单一文件；不能把这些字段存在理解为已经支持轮转。

## 主要符号

- `Config`：公开配置结构。`Level`、`File`、三个滚动字段和 `EnableDiagnoseLogs` 保持 Go 风格字段名；`Adjust(&mut self)` 只补零值，不验证文件路径，也不主动调用于 `InitLogger`。
- `Logger { core, level }`：可克隆门面。`core: Arc<dyn Core>` 承担过滤/编码/输出；可选 `level` 仅在由 `InitLogger` 构造时保存动态级别控制器，`Wrap` 构造的 Logger 没有该控制器。
- `Logger::log`：私有统一写入口，带 `#[track_caller]`；先调用 `Core::enabled`，再用 `Location::caller().file()` 设置 `Entry.caller`，最后忽略 `Core::write` 的错误。
- `Logger::{Debug,Info,Warn,Error}`：公开级别便捷方法。`With` 合并固定字段，`Named` 追加点号分隔的层级名，二者保留同一个动态级别控制器。
- `Logger::Begin`、`BeginTask`、`Task::{End,End2}`：任务生命周期 API。`BeginTask` 固定 Info；`End` 特判取消；`End2` 不特判取消。
- `OutputCore`、`Destination`：私有实际输出实现，目标为 stdout、文件或丢弃；`with`/`named` 创建新 Core，但共享 `Arc<RwLock<Level>>`。
- `InitLogger`：公开全局初始化入口。第二个字符串参数当前未使用，仅用于保持调用签名兼容。
- `SetAppLogger`、`L`、`Level`、`SetLevel`：进程级状态接口。全局 Logger 和全局级别分别由 `OnceLock<RwLock<_>>` 惰性创建。
- `ShortError`：`Some(error)` 生成名为 `error` 的字符串字段，`None` 生成 skip 字段。
- `CancellationError`、`IsContextCanceledError`：本地可向下转型的取消类型及错误链扫描器；变体覆盖 context、gRPC、Smithy 和嵌套 operation 的显示语义。

## 执行流程

初始化流程从调用方准备 `Config` 开始；若需要默认值，调用方须先显式调用 `Config::Adjust`。`InitLogger` 在诊断模式下设置进程环境变量 `GRPC_DEBUG=true`，随后用 `Level::parse` 解析级别。空路径和 `"-"` 选择 stdout；其他路径先拒绝目录，再创建或以追加模式打开一次以验证可写性。函数构造共享级别锁和 `OutputCore`；非诊断模式再用 `FilterCore` 包裹它，仅允许 BR、Lightning、ingestctrl、`main.main` 和 PD client 的调用路径。最后在写锁下原子替换全局 Logger 与全局级别值。

一次普通日志调用进入 `Logger::log`：先按共享级别锁判断是否启用，禁用则不构造条目；启用后记录调用文件，组装 `Entry`，把派生 Logger 的固定字段和本次字段交给 Core。`OutputCore::write` 设置 Logger 名称，调用 `encode_json` 生成一行 JSON，然后打印到 stdout、每次重新打开目标文件追加一行，或在 nop Logger 中丢弃。

任务流程由 `Logger::Begin(level, name)` 立即写出 `"<name> start"` 并记录 `Instant`。`Task::End` 计算耗时：成功时恢复 Begin 的级别、保留额外字段并写 `completed`；错误链含 `CancellationError` 时改为 Debug、丢弃额外字段并写 `canceled`；其他错误使用 End 参数指定的级别、丢弃额外字段并写 `failed`。三种路径都追加 `takeTime` 与 `ShortError`。`End2` 只有成功/失败两类，当前 Rust 实现仍通过 `ShortError` 写简短错误，而不是 Go 版的完整 `zap.Error`。

## 数据与状态

进程级状态分成两个锁：`app_logger()` 保存当前 `Logger`，`app_level()` 保存对外可见的 `Level`。首次访问前 Logger 是写入 `Destination::Discard` 的 Debug 级 nop Logger，而公开级别初始为 Info。`InitLogger` 同时更新两者，并把 Logger 的 `level` 指向 `OutputCore` 所用的同一个 `Arc<RwLock<Level>>`；因此 `SetLevel` 既更新公开级别，也能立即改变已初始化 Logger 的过滤阈值。若通过 `SetAppLogger` 替换成 `Wrap` 创建的 Logger，其 `level` 为 `None`，之后 `SetLevel` 只更新全局读数，不会改动外部 Core 的过滤行为。

派生 Logger 不修改父对象：`With` 克隆已有字段后追加新字段，`Named` 克隆名称并用 `.` 拼接；它们共享输出级别锁，但各自持有目的地描述、字段和名称副本。`Task` 持有 Logger 克隆、开始级别、任务名和单调时钟 `Instant`，没有自动结束或 Drop 日志，调用方必须显式调用 `End`/`End2`。

## 依赖与调用关系

下游直接依赖集中在 `crate::filter`：`Core` 定义 enabled/with/named/write 协议，`Entry` 承载级别、消息、调用方和 Logger 名称，`Field` 表示结构化字段，`Level` 负责排序与字符串解析，`FilterCore` 做调用路径白名单，`encode_json` 负责一行 JSON。标准库依赖分别承担错误链 (`StdError`)、文件追加、输出、共享所有权与锁、惰性全局初始化以及单调计时。

上游通过 `lib.rs` 的再导出访问 API。RustCodeGraph 将 `log.rs` 标记为被约 202 个文件使用；其中有一部分是 crate/类型级引用，不能等同于每个文件都直接调用本模块函数。精确仓库搜索确认 `pkg/lightning/duplicate/detector.rs` 的“sort keys”和 `worker.rs` 的“run task”使用 `Begin`/`End`；多个 Cargo manifest 将本 crate 作为日志 facade 依赖。Go 应用主链的对照入口是 `lightning/pkg/server/lightning.rs`：它在服务启动时调用 Go `log.InitLogger`，运行中调用 `L`、`ShortError` 和 `SetLevel`。Rust 主链是否已等量接线不能仅由这些 Go 调用推断。

## 错误处理与边界

`InitLogger` 会传播未知级别、目录作为日志文件名、创建/打开文件失败；目录错误文本固定为 `can't use directory as log file name`。环境变量设置位于 `unsafe` 块，依赖初始化发生在其他线程读取该进程变量之前这一调用约束。初始化完成后的实际日志写错误被 `Logger::log` 明确忽略，因此磁盘满、权限变化等运行时失败不会返回给业务调用方。

所有全局或级别锁都用 `expect`；锁中毒会 panic，而不是转成 `Result`。文件输出每条日志重新打开文件，既没有轮转，也没有显式 flush/sync 保证。stdout 使用 `println!`。`IsContextCanceledError` 只认错误链中可向下转型为本文件 `CancellationError` 的对象，并不自动识别任意第三方 context/gRPC/Smithy 错误类型；这些外部语义必须先映射为该枚举。

过滤依赖 `#[track_caller]` 得到的 Rust 源文件路径，而 Go 版依赖 zap caller 函数路径；白名单字符串相同不意味着两侧所有路径形状完全相同。`Config::Adjust` 对负数滚动值、非法路径等不做校验，且 `InitLogger` 不消费三个滚动字段。

## 并发与资源生命周期

`Logger`、Core 和级别控制器通过 `Arc` 共享，进程全局替换与读取由 `RwLock` 串行化；一次 `L()` 取得的是 Logger 克隆，因此随后替换全局 Logger 不会改变调用方已经持有的旧克隆。由 `InitLogger` 派生的 Logger 克隆共享级别锁，`SetLevel` 的修改会传播给这些克隆。Logger 名称与字段在派生时复制，不需要共享可变容器。

`OutputCore` 不长期持有文件句柄：初始化先验证/创建文件，每条日志再打开、追加、关闭，资源生命周期短但有每条日志一次 open 的成本。多个线程可同时追加同一路径；本文件没有额外互斥、跨进程锁或单行原子性保证。`Task` 使用 `Instant` 避免系统时钟回拨影响耗时；它没有取消令牌或后台任务。`GRPC_DEBUG` 是进程级环境状态，本文件只在诊断开启时设置，不在关闭或重初始化时清除。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/lightning/log/log.go`，测试为 `log_test.go`。一致点包括：配置字段与默认值、`warning` 别名、stdout 的 `"-"` 约定、非诊断模式白名单、全局 `L`/`Level`/`SetLevel`、`ShortError`、Logger 的 `With`/`Named`、任务开始/完成/取消/失败文案，以及成功才保留额外字段的规则。Rust `migration_aster_unit_test.rs` 进一步固定了默认值、字段合并、过滤匹配、JSON 形状和 Task 分支。

当前 Rust 不是 Go zap 栈的逐对象复刻。Go `InitLogger` 还初始化 TiDB/GRPC logger、调用 `pingcap/log.InitLogger`、安装全局 zap logger、使用 lumberjack 风格滚动配置并关闭普通错误栈；Rust 只建立本 crate 的 JSON stdout/追加文件 Core。Go 的取消判断识别 `context.Canceled`、gRPC status、Smithy CanceledError/OperationError；Rust 用自有 `CancellationError` 和标准错误链模拟这些类别。Go `End2` 对失败使用完整 `zap.Error`，Rust 当前调用 `ShortError`。Go `SetAppLogger` 强制 DPanic 才附加栈，Rust只是克隆传入 Logger。这些都是扩展时需要保留或有意缩小的已知差异，不能在文档中宣称完全等价。

## 扩展指南

- 增加配置默认值或别名时修改 `Config::Adjust`，并同步 `pkg/lightning/log/log_test.rs` 与 `migration_aster_unit_test.rs`；若改变序列化契约，还要核对 Go `Config` 的 TOML/JSON 标签，但 Rust 结构当前没有 serde 派生。
- 增加输出目标、日志轮转或长期文件句柄时，以 `Destination`、`OutputCore::write` 和 `InitLogger` 为接入点；需要明确并发写入、重开/轮转、flush、错误可见性与三个滚动配置字段的真实语义，不能只让测试通过而省略 Go 行为。
- 改变过滤规则时同步 `InitLogger` 白名单和 `filter.rs` 的匹配逻辑，并在独立的 `filter_test.rs`/`migration_aster_unit_test.rs` 增加调用路径边界测试。
- 增加 Logger 方法应尽量汇入 `Logger::log`，以保留级别短路和调用方采集；修改 `With`/`Named` 时须保持固定字段、名称和共享级别控制器的不变量。
- 扩展取消识别时修改 `CancellationError`/`IsContextCanceledError`，为直接、包装、多层 source 和非取消错误分别增加 `log_test.rs` 用例。若接入外部错误库，应显式记录与 Go `errors.Cause`/`errors.Is`/`errors.As` 的差异。
- 修改任务协议时同时覆盖 `Begin`、`BeginTask`、`End`、`End2` 的消息、级别、额外字段、错误字段和耗时；Rust 单元测试必须继续位于独立测试文件，不能内嵌进 `log.rs`。
- 任何性能优化都应特别测量文件目标每条日志 open 的成本、字段克隆和全局锁竞争；兼容性检查应覆盖 JSON 字段名、白名单路径、错误文本和 Go 调用签名。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/lightning/log` 找到本 crate 的 12 个 Go/Rust 文件；`node --file pkg/lightning/log/log.rs --offset 1 --limit 500` 完整读取 463 行并列出约 202 个使用文件；`query` 确认 Rust/Go 的 `InitLogger`、`BeginTask`、`IsContextCanceledError` 定义。对这些大写 Go 风格符号运行精确 `callers/callees --file pkg/lightning/log/log.rs` 未返回调用边，因此上游关系另用直接引用搜索核验，没有据此虚构边。
- 源码与 crate 边界：`pkg/lightning/log/log.rs`、`lib.rs`、`filter.rs`（由目标文件的导入和符号边确认）、`Cargo.toml`；工作区和直接依赖清单由各级 `Cargo.toml` 中的 `astersql-lightning-log` 路径依赖核对。
- Rust 独立测试：`pkg/lightning/log/log_test.rs` 覆盖默认调整、目录拒绝、JSON、stdout/诊断环境变量、动态级别和取消错误链；`migration_aster_unit_test.rs` 覆盖 Go 对齐的默认值、过滤、任务三分支和 Named/With；`filter_test.rs` 是过滤 Core 的相关测试面。
- Go 对照：`pkg/lightning/log/log.go`、`log_test.go`；Go 应用接线参考 `lightning/pkg/server/lightning.rs`。Rust 直接业务使用由 `pkg/lightning/duplicate/detector.rs`、`worker.rs` 的 `Begin`/`End` 调用核对。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定的 11 章结构命令，并人工复核：文档区分已实现行为、Go 差异、尚未接线/未实现能力及安全扩展位置。
