# `pkg/lightning/log/testlogger.rs`

## 文件定位

该文件属于 Cargo crate `astersql-lightning-log`，crate 根由 `pkg/lightning/log/Cargo.toml` 的 `[lib] path = "lib.rs"` 指向 `pkg/lightning/log/lib.rs`。`lib.rs` 公开声明 `pub mod testlogger`，因此使用方可通过 `astersql_lightning_log::testlogger` 取得这里的 API；与 `filter`、`log` 不同，`testlogger` 的符号没有在 crate 根通配再导出。

它位于 Lightning 日志抽象的测试适配层：生产侧 `pkg/lightning/log/log.rs::Logger` 只依赖 `Arc<dyn Core>`，本文件提供一个写入内存而非 stdout/文件的 `MemoryCore`，并把可读取的共享缓冲区连同 `Logger` 一起返回。它不初始化进程级全局 logger，也不参与 `InitLogger` 的输出目的地选择。

尽管文件名是 `testlogger.rs`，它不是内嵌测试文件，而是 crate 的公开模块；独立 Rust 测试位于 `filter_test.rs`、`log_test.rs` 和 `migration_aster_unit_test.rs`。

## 核心职责

- `MakeTestLogger` 构造与生产日志 API 相同的 `Logger`，使测试可以照常调用 `Debug`、`Info`、`Warn`、`Error`、`With`、`Named` 和 `Begin`，同时取得用于断言的 `Buffer`。
- `MemoryCore` 实现 `pkg/lightning/log/filter.rs::Core` 的四个契约：所有级别均启用、派生固定字段、派生层级名称、把条目编码成 JSON 后写入内存。
- `Buffer` 封装 `Arc<Mutex<Vec<String>>>`，让测试持有的观察句柄、基础 Logger 及所有派生 Logger 看到同一个按行存储的输出。
- JSON 形状不在本文件重新实现，而是委托给 `filter.rs::encode_json`，从而沿用 `$lvl`、`$msg`、可选 `logger` 和字段插入顺序等约定。

本文件只用于提供可观测 sink；日志级别解析、调用方过滤、全局 logger、文件输出和任务结束策略分别由 `filter.rs` 与 `log.rs` 负责。

## 主要符号

- `pub struct Buffer { lines: Arc<Mutex<Vec<String>>> }`：公开、可克隆的观察句柄；内部行集合保持私有，调用方只能取得快照。
- `Buffer::lines(&self) -> Vec<String>`：锁住共享向量并克隆全部行。若互斥锁已 poison，会以 `expect("test log buffer poisoned")` 触发 panic，而不是返回 `Result`。
- `Buffer::stripped(&self) -> String`：调用 `lines()` 后以 `"\n"` 连接；零行返回空串，多行之间有换行，但结尾不追加换行。
- `pub enum TestLoggerOption {}`：公开的空枚举，占位以保留类似 Go 可变选项的构造签名；当前无法构造任何合法变体。
- `struct MemoryCore`：文件私有 Core，保存共享 `buffer`、派生的固定 `fields` 和当前 `name`。
- `impl Core for MemoryCore`：`enabled`、`with`、`named`、`write` 是 Logger 调用本文件的实际动态分派入口。
- `pub fn MakeTestLogger(_opts: impl IntoIterator<Item = TestLoggerOption>) -> (Logger, Buffer)`：唯一公开构造函数；当前忽略 `_opts`，创建空字段、空名称的 `MemoryCore`，再交给 `Logger::Wrap`。

文件没有模块级常量、条件编译项或内嵌 `#[test]`。

## 执行流程

1. 测试调用 `MakeTestLogger([])`。函数创建默认 `Buffer`，克隆一份放入 `MemoryCore`，再用 `Logger::Wrap(Arc::new(core))` 返回 Logger 和原观察句柄。
2. 调用 `Logger::Warn` 等级别方法时，`log.rs::Logger::log` 先调用 `MemoryCore::enabled`；这里恒为 `true`，所以 Debug 到 Error 都不会被此 Core 过滤。
3. `Logger::log` 创建包含级别、消息和 `#[track_caller]` 文件路径的 `Entry`，然后动态调用 `MemoryCore::write`。
4. `write` 先用 Core 当前名称覆盖 `entry.logger_name`，再按“Core 固定字段在前、本次日志字段在后”的顺序传给 `encode_json`。后出现的同名字段会按 `serde_json::Map::insert` 的覆盖语义替换已有值。
5. 编码结果作为一条不带尾换行的 `String`，在持有 mutex 时追加到 `Vec<String>`；成功返回 `Ok(())`。
6. 测试通过 `buffer.lines()` 按条检查，或通过 `buffer.stripped()` 得到以换行拼接的完整快照。

派生流程有两条：`Logger::With` 调用 `MemoryCore::with`，复制既有字段后追加新字段；`Logger::Named` 调用 `MemoryCore::named`，在已有非空名称后加 `.` 再追加子名称。两者都会新建 Core，但克隆的是同一个 `Buffer` 句柄。

## 数据与状态

唯一可变共享状态是 `Buffer::lines` 指向的 `Vec<String>`。每次成功写入恰好追加一个完整 JSON 字符串；读取返回克隆快照，因此读取者不能通过返回值修改内部状态。文件没有清空、截断或容量限制 API，长时间或大量写入会让内存占用随日志行数增长。

`MemoryCore::fields` 与 `MemoryCore::name` 是每个 Core 实例自己的不可变派生状态。`with` 和 `named` 不修改父 Core，而是克隆相关状态生成新的 trait object；父、子 Core 唯一共享的是 `Buffer`。因此父子 Logger 的字段和名称互不反向污染，但输出汇聚到同一行序列。

字段顺序是可观察行为：`encode_json` 先插入 `$lvl`、`$msg`，名称非空时插入 `logger`，然后依次插入固定字段与调用字段。Cargo 为 `serde_json` 启用 `preserve_order`，测试因此可以直接比较完整 JSON 字符串。

## 依赖与调用关系

直接标准库依赖是 `std::sync::{Arc, Mutex}`。crate 内依赖如下：

- `filter.rs::Core` 定义动态分派接口；`Entry`、`Field`、`Level` 是写入参数；`encode_json` 完成 JSON 序列化。
- `log.rs::Logger::Wrap` 把 `MemoryCore` 暴露为统一 Logger；`Logger::log` 是 `MemoryCore::enabled` 和 `write` 的直接上游，`Logger::With`/`Named` 分别调用 `with`/`named`。
- `Cargo.toml` 的唯一直接第三方依赖是启用 `preserve_order` 的 `serde_json`，但本文件通过 `encode_json` 间接使用它。

仓库内直接 Rust 使用点集中在同 crate 的独立测试：`filter_test.rs`、`log_test.rs`、`migration_aster_unit_test.rs` 均导入 `testlogger::MakeTestLogger`。`rg` 还发现 `lightning/pkg/importinto/stubs.rs` 和 `lightning/pkg/server/stubs.rs` 各自定义了同名辅助函数，但它们不是对本文件函数的调用者，不能视作调用边。

RustCodeGraph 将目标文件标记为被多个文件使用，且精确索引了 `MakeTestLogger`、`MemoryCore` 与 `Buffer`；其 `callers`/`callees` 命令本次未产生可用输出，因此具体调用点由上述 `rg` 结果和独立测试源码补证。

## 错误处理与边界

`MemoryCore::write` 唯一显式错误来自 mutex 加锁失败：poison 错误被转成字符串并返回 `Err(String)`。但上游 `Logger::log` 使用 `let _ = self.core.write(...)` 丢弃 Core 写入错误，所以通过普通 Logger API 写日志时，调用方不会收到该错误；直接通过 Core 调用 `write` 才能观察它。

读取路径与写入路径的 poison 策略不同：`Buffer::lines` 直接 panic，`write` 返回错误。当前测试没有主动制造锁 poison，也没有覆盖这两个异常分支。

`TestLoggerOption` 没有变体，`MakeTestLogger` 也忽略传入迭代器；因此 Rust 版本当前不能表达 Go 的 `zap.WrapCore`、`zap.AddCaller` 等构造选项。需要过滤时，现有 Rust 测试是在构造后通过 `logger.core()` 与 `FilterCore::new` 手工包装。

该实现恒定启用全部级别，没有最低级别配置；它也不输出时间、调用方或 stack 字段，除非调用者显式把相应内容作为 `Field` 传入。字段序列化规则、跳过字段与同名键覆盖均由 `encode_json` 决定。

## 并发与资源生命周期

`Core` 要求 `Send + Sync`，而 `MemoryCore` 通过 `Arc<Mutex<Vec<String>>>` 满足跨线程共享要求。每次写入只在向量 `push` 期间持锁；JSON 编码发生在加锁之前，可缩短临界区。多线程写入不会破坏单条 JSON 字符串，但跨线程的行先后顺序取决于取得锁的顺序，不能假定与线程启动顺序一致。

`Buffer`、Logger 及派生 Core 都通过 `Arc` 共同拥有行集合。只要任一克隆仍存活，日志内容就保持可读；最后一个 `Arc` 释放时，向量及所有字符串自动回收。没有后台线程、通道、异步任务、文件句柄、显式 flush 或 teardown 步骤。

`lines()` 在锁内克隆整个向量与其中字符串，适合确定性单元测试，但高频轮询或超大日志会产生与当前累计内容成比例的复制成本。`stripped()` 还会额外分配拼接字符串。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/log/testlogger.go::MakeTestLogger`。两者都返回 Logger 与共享缓冲，启用 Debug 及以上全部日志，并输出可直接断言的 JSON；`log_test.go::TestTestLogger` 与 Rust `log_test.rs::test_test_logger` 使用相同消息、整数、整数数组和期望字符串，确认 `$lvl`/`$msg` 及字段顺序对齐。

Go 版本用 `zapcore.NewJSONEncoder`、`zapcore.NewCore` 和 `zaptest.Buffer` 组合完成这些行为；Rust 版本用仓库内 `Core`、`MemoryCore`、`encode_json` 和 `Buffer` 自行实现等价测试边界。Go encoder 显式配置大写级别与字符串 duration；Rust 的大写级别和 duration 格式分别由 `Level::capital` 与 `Field`/`filter.rs` 编码逻辑负责。

尚未完全对齐的是 options：Go 将任意 `zap.Option` 传给 `zap.New`，`filter_test.go` 借此使用 `zap.WrapCore` 和 `zap.AddCaller`；Rust 的 `TestLoggerOption` 为空且 `_opts` 被忽略，Rust `filter_test.rs` 改为显式取得 `logger.core()` 后包装 `FilterCore`。这是当前代码事实，不应把 Rust 占位签名描述为已支持 Go 选项。

`migration_aster_unit_test.rs::migration_logger_with_and_named_share_the_same_sink` 进一步验证了 Rust `Named`/`With` 派生与 Go zap 的共享 sink 语义：输出包含 `logger:"worker"` 和固定字段 `engine:"42"`。

## 扩展指南

- 若新增测试 Logger 选项，应优先为 `TestLoggerOption` 增加明确变体，并在 `MakeTestLogger` 构造 Core 时应用；需要同步覆盖每个选项的独立测试，不能只保留被忽略的形参。若目标是对齐 Go，重点核对 `zap.WrapCore`、caller 信息和最低级别行为。
- 若改变 JSON 键、大小写、字段顺序、名称格式或同名字段覆盖规则，应修改共享的 `filter.rs::encode_json` 或明确说明仅测试 Core 的差异，并同步 `log_test.rs::test_test_logger`、`filter_test.rs::test_filter`、`migration_aster_unit_test.rs::migration_test_logger_emits_go_compatible_json` 以及对应 Go 测试预期。
- 若增加清空、增量读取或容量限制，应把逻辑放在 `Buffer`，同时明确克隆句柄之间的可见性和多线程顺序；测试仍应放在独立 `*_test.rs`，不要嵌入本源文件。
- 若调整 `with`/`named`，必须保持派生 Core 不修改父状态且共享同一 sink，重点回归 `migration_logger_with_and_named_share_the_same_sink`；层级名称目前以 `.` 拼接。
- 若要暴露写入失败，不仅要修改 `MemoryCore::write`，还需评估 `log.rs::Logger::log` 当前丢弃 `Result` 的公共行为。直接改变它可能影响所有 Core，而非仅本文件。

主要兼容风险是破坏与 Go JSON 输出或 options 语义的对齐；主要正确性风险是字段合并顺序、锁 poison 策略和派生 Logger 状态共享发生变化；主要性能风险是无界缓冲以及 `lines`/`stripped` 的全量复制。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件，目标 `pkg/lightning/log/testlogger.rs` 被完整读取为 107 行、11 个符号；精确查询确认 `MakeTestLogger`、`MemoryCore`、`Buffer` 节点，`encode_json` 与 `Core` 位于 `filter.rs`。图的 callers/callees 查询未返回可用边，故未据此臆造调用关系。
- 目标源码：`pkg/lightning/log/testlogger.rs`，核对 `Buffer::{lines,stripped}`、`TestLoggerOption`、`MemoryCore` 的全部 Core 实现及 `MakeTestLogger`。
- crate 与模块边界：`pkg/lightning/log/Cargo.toml`、`pkg/lightning/log/lib.rs`；同时检查 `BUILD.bazel`，确认 Go 包与 Go 测试文件集合。
- Rust 下游契约：`pkg/lightning/log/filter.rs::{Core,Entry,encode_json}`、`pkg/lightning/log/log.rs::Logger`。
- Rust 独立测试：`pkg/lightning/log/filter_test.rs::test_filter`、`pkg/lightning/log/log_test.rs::test_test_logger`、`pkg/lightning/log/migration_aster_unit_test.rs::{migration_test_logger_emits_go_compatible_json,migration_logger_with_and_named_share_the_same_sink}`。
- Go 对照与测试：`pkg/lightning/log/testlogger.go::MakeTestLogger`、`pkg/lightning/log/filter_test.go::TestFilter`、`pkg/lightning/log/log_test.go::TestTestLogger`。
- 调用点补查：`rg` 搜索 `MakeTestLogger`、`testlogger::` 与 `Buffer`，确认本 crate 独立测试是目标 Rust API 的直接使用方，并区分其他目录的同名 stub。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时执行任务指定的 11 章节结构验证；仓库说明引用的 `.agents/skills/tidb-verify-profile` 在当前工作区不存在，因此无法加载额外 Ready profile，未将其视为代码行为验证。
