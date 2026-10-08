# `pkg/testkit/testutil/loghook.rs`

## 文件定位

该文件属于 `astersql-testkit-testutil` crate，是测试辅助层中的 `tracing` 日志捕获器，而不是数据库运行时日志主链。crate 入口 `pkg/testkit/testutil/lib.rs` 通过私有 `loghook` 模块 `include!("loghook.rs")`，再用 `pub use loghook::*` 暴露公开类型和函数；因此调用方从 crate 根使用 `WithLogHook`、`LogHook`、`LogField` 等 API。`pkg/testkit/testutil/Cargo.toml` 声明直接依赖 `tracing = "0.1"` 与 `tracing-subscriber = "0.3"`，对应事件模型和订阅层实现。

当前仓库中实际执行该辅助器的 Rust 测试位于 `pkg/testkit/testutil/migration_aster_unit_test.rs::log_hook_filters_messages_and_retains_fields`。`pkg/session/test/session_test.rs` 中的相关行仍是注释形式，不能视为已接线调用。RustCodeGraph 对文件的索引显示 27 个符号；对限定到本文件的 `WithLogHook`、`Logs`、`on_event` 运行 callers/callees 查询时没有发现额外跨文件调用边。

## 核心职责

文件提供一条局部、可共享的日志断言链：

1. `WithLogHook` 创建 `LogHook`，把其克隆体安装到新的 `tracing::Dispatch`。
2. 测试通过 `tracing::dispatcher::with_default` 在当前作用域安装该 dispatcher。
3. `Layer::on_event` 将 `tracing::Event` 的消息与结构化字段转换为本文件的 `LogEntry`/`LogField` 表示。
4. `LogHook::Check` 以消息子串筛选事件，通过的事件由 `Write` 保存。
5. 测试通过 `CheckLogCount`、`Logs` 以及 `LogEntry` 的断言方法检查结果。

其存在的目的，是在不替换进程全局订阅器的情况下捕获测试作用域内的结构化日志，并保持与 Go 版 zap 测试钩子的主要断言语义相近。它不负责日志落盘、日志级别配置、生产日志格式或跨进程收集。

## 主要符号

- `LogValue`：私有枚举，保留字符串、`i64`、`u64`、`f64`、布尔和 Debug 文本六种字段形态；其 `Display` 实现供过滤消息转换和失败诊断编码使用。
- `LogField { key, value }`：公开字段类型，但成员保持私有，调用方通过 `LogField::new` 构造字符串字段，通过 `LogField::i64` 构造有符号整数字段。派生的 `PartialEq` 使字段断言同时比较键、值及值类型。
- `LogEntry { level, target, message, fields }`：公开日志条目类型，成员保持私有；`CheckMsg` 检查消息完全相等，`CheckField` 检查所要求字段是实际字段集合的子集，`CheckFieldNotEmpty` 要求指定键存在且值为非空 `String`。
- `LogHook { logs, messageFilter }`：公开且可克隆的订阅层。`logs: Arc<Mutex<Vec<LogEntry>>>` 是克隆体共享的有序缓存；`messageFilter: Arc<str>` 是不可变过滤条件。
- `LogHook::new`：私有构造器，初始化空缓存并固化消息过滤串。
- `LogHook::Check`：空过滤串接受所有事件，否则使用 `entry.message.contains(...)` 做区分大小写的子串判断。
- `LogHook::Write`：把已收集字段装入条目，并在互斥锁保护下追加到缓存。
- `LogHook::encode`：生成 `级别\t消息\t键=值...` 的诊断文本；它只服务于 `CheckLogCount` 的失败信息，不等价于生产日志编码器。
- `LogHook::CheckLogCount` 与 `LogHook::Logs`：前者断言快照条数并在失败时打印编码结果，后者在锁内克隆当前快照后返回。
- `EventVisitor`：私有的 `tracing::field::Visit` 实现；`record` 将名为 `message` 的字段单独写入消息，其余字段按访问顺序追加到 `fields`。
- `impl<S> Layer<S> for LogHook::on_event`：订阅入口，读取事件元数据与字段、过滤并保存。
- `WithLogHook(&str) -> (Dispatch, LogHook)`：公开装配入口，返回局部 dispatcher 和与其共享缓存的钩子句柄。

文件没有模块级常量、trait 定义或条件编译项。

## 执行流程

装配时，`WithLogHook` 调用私有 `LogHook::new`，随后把 `hook.clone()` 附加到 `Registry::default()`，并封装成 `Dispatch`。返回的 `hook` 与 dispatcher 中的克隆体共享同一个 `Arc<Mutex<Vec<LogEntry>>>`，测试因此能从外部观察订阅层写入的事件。

事件到达 `on_event` 后，流程为：创建空的 `EventVisitor`；调用 `event.record` 触发各类型的 `Visit::record_*`；从元数据复制 `level` 和 `target`，从 visitor 取出消息，并先构造字段为空的 `LogEntry`；调用 `Check` 对消息执行过滤；通过时调用 `Write` 把 visitor 中的字段装回条目并追加缓存。过滤发生在写锁获取之前，但字段访问已经完成。

断言时，`CheckLogCount` 首先调用 `Logs` 取得克隆快照，再逐条调用 `encode`，最后比较快照长度。`migration_aster_unit_test.rs::log_hook_filters_messages_and_retains_fields` 证明了当前预期：不含 `needle` 的 info 事件被丢弃，包含 `needle` 的 warn 事件被保留，整数 `answer = 42` 保持为 `I64`，字符串字段 `detail` 可做非空断言。

## 数据与状态

缓存状态仅存在于 `LogHook::logs` 中。`Vec` 保留进入该 layer 的事件顺序；`Logs` 返回深克隆快照，因此调用方之后修改或持有结果不会继续占用锁，也不会看到后续事件。文件没有清空缓存的方法，单个 hook 的结果会持续累积到所有 `LogHook`/`Dispatch` 克隆体被释放。

`messageFilter` 在构造时转为 `Arc<str>`，所有克隆体共享同一不可变字符串。空串是“不筛选”的哨兵。消息来自 tracing 的特殊字段名 `message`；若事件没有该字段，`EventVisitor::default` 留下空消息，此时只有空过滤器或能被空串包含的过滤条件才可能通过（按当前 `contains` 逻辑，非空过滤器不会通过）。

字段保留访问时的基础类型，Debug 类型则不可逆地格式化为字符串。`LogEntry::target` 被记录但当前公开断言与 `encode` 都不读取它；它仍参与条目克隆和 Debug 输出。

## 依赖与调用关系

上游装配关系是 `pkg/testkit/testutil/lib.rs -> include!("loghook.rs") -> pub use loghook::*`。已验证的执行调用方是 `pkg/testkit/testutil/migration_aster_unit_test.rs::log_hook_filters_messages_and_retains_fields -> WithLogHook`，随后由 `tracing::dispatcher::with_default` 驱动 dispatcher 中的 layer。RustCodeGraph 还确认了内部边 `WithLogHook -> LogHook::new`、`on_event -> Check/Write`、`CheckLogCount -> Logs/encode`，以及各 `Visit::record_* -> EventVisitor::record`。

下游依赖分为两层：`tracing::{Dispatch, Event, Subscriber, field::{Field, Visit}}` 提供事件与字段访问协议；`tracing_subscriber::{Registry, Layer}` 和 `SubscriberExt::with` 提供订阅器及 layer 组合。标准库的 `Arc`/`Mutex` 负责共享同步，`fmt::Write` 负责向 `String` 写诊断编码。

该文件没有调用 SQL session、planner、executor、存储或生产日志配置代码；它在完整应用中的位置是测试边界。Cargo 元数据中的 `go-package = "pkg/testkit/testutil"` 表明其移植来源，其他 codec、KV、MySQL、随机数和时区依赖属于同 crate 的别的测试辅助模块，不是本文件的直接依赖。

## 错误处理与边界

这是断言辅助 API，失败策略以 panic 为主：`CheckMsg`、`CheckField`、`CheckFieldNotEmpty` 和 `CheckLogCount` 使用断言；缺失字段、空字符串、字段类型不是 `String` 或数量不符都会终止当前测试。互斥锁若被持锁线程 panic 污染，`Write` 和 `Logs` 的 `expect("log hook mutex poisoned")` 也会 panic，而不是恢复或返回错误。

`encode` 的签名保留 `Result<String, fmt::Error>`，但当前目标是 `String`，其格式化写入通常不可失败；`CheckLogCount` 仍以 `expect("encode captured log")` 明确处理该接口边界。字段断言采用“包含全部期望字段”而非完整集合相等，所以额外字段允许存在；相同键也没有唯一性约束，只要任一完整键值匹配即可。

消息过滤是区分大小写的纯子串匹配，不包含正则、级别或 target 条件。`CheckFieldNotEmpty` 只接受 `LogValue::String`，由 `record_debug` 产生的 `Debug(String)` 即使非空也会被判为类型错误。公开构造器目前只覆盖字符串和 `i64`，所以外部测试不能直接构造期望的 `U64`、`F64`、`Bool` 或 `Debug` 字段。

## 并发与资源生命周期

`Arc<Mutex<Vec<LogEntry>>>` 让 dispatcher 持有的 layer 克隆和测试持有的 hook 跨线程安全共享缓存；每次 `Write` 的追加与每次 `Logs` 的完整克隆都在同一互斥锁下串行化。锁不会跨事件过滤或文本编码持有：`on_event` 先完成访问与筛选才进入 `Write`，`CheckLogCount` 先取快照再编码，降低了临界区范围。

多个线程并发发出事件时，每条追加本身是原子的，但跨线程条目的先后顺序取决于获得互斥锁的顺序，不应把它当作严格的业务时间顺序。`Logs` 只能保证取得某一锁时刻的一致快照；快照返回后仍可能有新事件写入。

dispatcher 由调用方负责在合适作用域安装和持有。当前测试用 `tracing::dispatcher::with_default` 做词法作用域内安装，闭包结束后恢复先前 dispatcher；本文件不创建线程、异步任务、通道或后台清理任务。只有当 `Dispatch`、其 layer 克隆及所有外部 `LogHook` 克隆都释放后，共享缓存和过滤串才被回收。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/testkit/testutil/loghook.go`。两版都有日志条目、字段断言、消息子串过滤、写入缓存、日志计数诊断和 `WithLogHook` 装配入口。Rust 的 `CheckMsg`/`CheckField`/`CheckFieldNotEmpty` 对应 Go 同名方法；`Check` 都在消息不含过滤串时拒绝条目；`CheckLogCount` 都先编码条目以便计数失败时展示内容。

重要差异如下：

- Go 版包装 `zapcore.Core`，用 `context.WithValue(..., logutil.CtxLogKey, logger)` 返回替换 logger 的 context；Rust 版实现 `tracing_subscriber::Layer`，返回需由调用方局部安装的 `Dispatch`，不接受 context 或测试对象。
- Go 版 `Write` 返回 `error`，Rust 版 `Write` 无返回值并在锁污染时 panic；Go 断言通过 `testing.T`/`require` 报告，Rust 使用 `assert!`/`assert_eq!`。
- Go 字段沿用完整 `zapcore.Field` 语义，Rust 只保存 `Visit` 暴露的六类值，并只向外提供字符串与 `i64` 期望值构造器。
- Go `encode` 使用 zap 文本 encoder，包含其格式策略；Rust `encode` 是本地简化的制表符格式，且省略 target。二者都用于诊断，但输出文本不保证逐字相同。
- Go 的 `Logs` 是公开切片字段且没有同步保护；Rust 使用私有互斥缓存和克隆快照，更适合 dispatcher 克隆体之间共享。

因此当前 Rust 版本对齐的是测试意图和主要筛选/断言行为，不是 zap API、上下文传播或编码格式的逐项复制。

## 扩展指南

若要新增可断言字段类型，最小接入点是 `LogValue`、对应的 `Visit::record_*`（若 tracing trait 提供）、`LogField` 的公开构造器以及 `Display`；应在独立测试文件 `pkg/testkit/testutil/migration_aster_unit_test.rs` 增加类型保持与错误类型断言，不要把测试嵌入 `loghook.rs`。新增构造器时需避免把 `Debug(String)` 与普通 `String` 混为一谈，因为 `PartialEq` 当前有意保留类型差异。

若要扩展过滤条件，应修改 `LogHook` 的不可变过滤状态和 `Check`，并覆盖空过滤器、大小写、无 message 事件以及并发写入。若要按 level 或 target 筛选，可使用 `on_event` 已写入 `LogEntry` 的元数据，但应明确 Go 版是否需要同步变化，避免悄然偏离 `loghook.go::Check` 的消息子串契约。

若要支持清空或排空缓存，应在 `LogHook` 上新增显式方法并定义并发语义：是返回原子快照并清空，还是只清空已观察条目。需测试 dispatcher 克隆仍在写入时的行为。若日志量可能很大，当前无界 `Vec` 和 `Logs` 全量克隆会带来内存与复制成本；可考虑有界缓存，但必须明确丢弃策略并保留 `CheckLogCount` 的可诊断性。

若要改变安装模型，入口是 `WithLogHook`。当前 API 有意把 dispatcher 的安装责任交给调用方；改为全局订阅器会引入一次性初始化和测试互相干扰风险，不应在没有隔离设计与并发回归测试时直接替换。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件，其中目标 `pkg/testkit/testutil/loghook.rs` 有 27 个符号；读取了该文件全部 253 行。
- RustCodeGraph 查询：对 `WithLogHook`、`LogHook`、`CheckLogCount` 做了精确查询；对限定到本文件的 `WithLogHook`、`Logs`、`on_event` 做了 callers/callees 查询。图确认内部调用，但未返回额外跨文件调用边，因此跨文件使用由文本搜索补证。
- 源码与模块边界：`pkg/testkit/testutil/loghook.rs`、`pkg/testkit/testutil/lib.rs`、`pkg/testkit/testutil/Cargo.toml`。
- Go 对照：`pkg/testkit/testutil/loghook.go` 全部 116 行。
- 独立 Rust 测试：`pkg/testkit/testutil/migration_aster_unit_test.rs::log_hook_filters_messages_and_retains_fields`；文本搜索还发现 `pkg/session/test/session_test.rs` 中只有注释掉的 Go 风格示例，不计为执行覆盖。
- 人工复核结论：文件是局部 tracing 测试捕获器；主要运行链、共享状态、panic 边界、Go 差异和安全扩展点均可回溯到上述符号与文件，没有把注释调用或未提供的能力描述为已支持。
