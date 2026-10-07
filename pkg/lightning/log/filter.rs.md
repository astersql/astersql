# `pkg/lightning/log/filter.rs`

源文件：[filter.rs](filter.rs)。本文描述当前 Rust 实现；调用关系与边界均以仓库现有代码和独立测试为准。

## 文件定位

本文件属于 `astersql-lightning-log` crate。crate 入口 [lib.rs](lib.rs) 将它声明为公开的 `filter` 模块，并通过 `pub use filter::*` 再导出其公共 API；[Cargo.toml](Cargo.toml) 指定 `lib.rs` 为库入口，运行时只有 `serde_json`（启用 `preserve_order`）这一项直接依赖。

它位于 Lightning 日志写入链的中间和基础层：一方面定义 `Level`、`Field`、`Entry`、`Core` 等本 crate 的日志数据与抽象，另一方面用 `FilterCore` 包装任意下游 `Core`，只把调用方标识命中白名单的条目继续写出。[log.rs](log.rs) 的 `InitLogger` 是生产接线点：关闭诊断日志时，它把实际输出的 `OutputCore` 包装为 `FilterCore`；开启诊断日志时则直接使用 `OutputCore`（`log.rs:265-315`）。

## 核心职责

- `Level` 负责级别解析、排序和 JSON 所需的大写名称（`filter.rs:27-64`）。级别从低到高依次为 `Debug`、`Info`、`Warn`、`Error`、`DPanic`、`Fatal`，默认值是 `Info`。
- `Field` 把字符串、整数、整数数组和 `Duration` 统一成 `serde_json::Value`，并提供不会进入最终输出的 `skip` 占位字段（`filter.rs:66-123`）。
- `Entry` 携带级别、消息、调用方标识和 logger 名称；`Core` 规定级别开关、派生字段、派生名称和写入四项能力（`filter.rs:138-182`）。
- `encode_json` 生成单行 JSON 对象，保留 `$lvl`、`$msg` 约定键，并按条件加入 `logger` 和非跳过字段（`filter.rs:184-205`）。
- `FilterCore` 保持过滤规则的同时代理下游 `Core`。只有 `Entry::caller_function` 包含任意过滤字符串时才转发写入，否则静默成功（`filter.rs:207-294`）。

因此，本文件不负责选择 stdout/文件、打开文件、维护全局 logger 或捕获调用位置；这些工作位于 [log.rs](log.rs)。

## 主要符号

- `pub enum Level`：可复制、可排序的日志级别。`Level::parse(&str) -> Result<Level, String>` 使用 ASCII 不区分大小写解析；空串和 `info` 都映射到 `Info`，`warning` 是 `Warn` 的别名，未知值返回文本错误。`Level::capital()` 返回固定大写字符串。
- `pub struct Field { key, value, skip }`：`key`、`value` 公开，`skip` 私有，调用者不能把普通字段事后改为跳过。`string`、`int`、`ints`、`duration` 和 `skip` 是构造入口。
- `fn format_duration(Duration) -> String`：私有格式化器。非零整秒输出 `秒.九位纳秒s`；不足一秒时按毫秒、微秒、纳秒中第一个非零单位输出整数值（`filter.rs:125-136`）。
- `pub struct Entry`：`new` 初始化级别和消息，并把 `caller_function`、`logger_name` 置空；`with_caller` 以 builder 形式设置调用方。
- `pub type CoreError = String` 与 `pub trait Core: Send + Sync`：错误目前统一为字符串；`Send + Sync` 是所有下游实现及包装器可跨线程共享的硬约束。
- `pub fn encode_json(...) -> String`：将条目和字段编码成 JSON 字符串，不追加换行。
- `pub struct FilterCore`：公开字段 `Core: Arc<dyn Core>` 保存下游，私有 `filters: Vec<String>` 保存允许子串。自定义 `Debug` 只显示过滤规则，不展开 trait object。
- `FilterCore::new` / `NewFilterCore`：惯用 Rust 构造器和 Go 风格兼容别名；二者都收集过滤迭代器并保留输入顺序。
- `FilterCore::with` 与同名的 `Core::with` 实现：调用下游 `with` 派生携带固定字段的新 core，同时克隆过滤列表。固有方法返回具体 `FilterCore`，trait 方法返回 `Arc<dyn Core>`。
- `FilterCore::write` 与 `Core::write` 实现：固有方法只做参数适配，真正过滤发生在 trait 实现。`enabled`、`named` 均转发到下游，并在派生对象上保留过滤列表。

## 执行流程

生产路径可按下列顺序理解：

1. `log.rs::InitLogger` 解析配置并创建持有目的地、级别锁、固定字段和名称的 `OutputCore`；非诊断模式再以五个允许片段构造 `FilterCore`（`log.rs:265-309`）。
2. `Logger::Debug/Info/Warn/Error` 进入带 `#[track_caller]` 的 `Logger::log`。它先调用 `Core::enabled`，再从 `std::panic::Location::caller().file()` 取得调用文件路径，构造 `Entry` 并调用 `Core::write`（`log.rs:96-129`）。
3. `FilterCore::enabled` 原样调用下游，不提前做包路径判断。真正写入时，`FilterCore::write` 对每个白名单元素执行 `entry.caller_function.contains(filter)`；首次命中即把原条目和字段交给下游，全部不命中则返回 `Ok(())`。
4. 当前生产下游 `OutputCore::write` 填入派生 logger 名称，将固定字段与本次字段串接后调用 `encode_json`，最后输出到 stdout、追加到文件或丢弃（`log.rs:224-240`）。
5. `encode_json` 依次插入 `$lvl`、`$msg`、可选 `logger` 和业务字段。由于 JSON map 对同名键再次 `insert` 会覆盖旧值，后出现的业务字段可覆盖约定键，且本次字段可覆盖同名固定字段；这是当前实现事实，不是额外校验过的稳定协议。

派生路径中，`Logger::With` 和 `Logger::Named` 分别调用 `Core::with`、`Core::named`。当底层是 `FilterCore` 时，新对象既采用下游派生结果，也保留原有过滤规则（`filter.rs:268-279`；`log.rs:131-145`）。

## 数据与状态

`Level`、`Entry` 和 `Field` 都是拥有型值；条目及字段中的字符串和 JSON 值在需要派生时克隆。`FilterCore` 自身没有可变共享状态：它持有一个引用计数的 `Arc<dyn Core>` 和一个拥有型 `Vec<String>`。每次 `with` 或 `named` 都创建新包装器并克隆过滤向量，原实例不变。

过滤不读取消息或字段值，只读取 `Entry::caller_function`。因此字段中即使出现白名单文本也不会放行；`filter_test.rs::test_filter` 和 `migration_aster_unit_test.rs::migration_filter_uses_caller_function_and_preserves_with_fields` 都覆盖了这一不变量。空过滤列表会丢弃所有条目；反过来，空字符串过滤项会因 Rust `str::contains("")` 对任意字符串为真而放行所有条目。

`Field::skip` 用空键和 `Null` 作内部占位，但 `encode_json` 先检查私有 `skip` 标记，因此不会输出这个占位。时长字段在构造时即变成字符串，不保留原 `Duration`；不足一秒的毫秒/微秒表示按整数单位截断更细精度。

## 依赖与调用关系

直接标准库依赖包括 `fmt`（自定义 `Debug`）、`Arc`（共享 trait object）和 `Duration`。唯一第三方依赖是 `serde_json::{Map, Number, Value}`，由 [Cargo.toml](Cargo.toml) 声明，并通过 `preserve_order` 保持插入顺序，使测试中的 JSON 字段顺序可预测。

已核实的上游调用关系如下：

- [lib.rs](lib.rs) 公开模块和再导出 API，并把 [filter_test.rs](filter_test.rs) 与 [migration_aster_unit_test.rs](migration_aster_unit_test.rs) 作为独立测试模块接入。
- `log.rs::Logger::log` 构造 `Entry` 并调用 `Core::write`；`Logger::With/Named` 调用相应 trait 方法。
- `log.rs::InitLogger` 是 `FilterCore::new` 的生产调用者，在 `EnableDiagnoseLogs == false` 时安装过滤器。
- `log.rs::OutputCore::write` 是当前生产下游，调用本文件的 `encode_json`。

下游调用关系保持很窄：级别解析依赖标准字符串处理；字段编码依赖 `serde_json`；过滤命中后只调用包装的 `Core::write`，而 `enabled`、`with`、`named` 分别委托同名 trait 方法。RustCodeGraph 的精确名称检索还识别到 `filter_test.rs::test_filter` 和迁移测试对 `FilterCore` 的调用；通用 callers/callees 命令因 Go/Rust 同名符号没有在本次查询时限内返回，本文未据此推断额外调用边。

## 错误处理与边界

- `Level::parse` 对未知字符串返回 `Err("unrecognized log level ...")`，不 panic；它只做 ASCII 小写转换。
- `encode_json` 返回 `String` 而非 `Result`，因为当前输入已是 `serde_json::Value`，序列化路径没有暴露可恢复错误。
- 命中过滤器时，`FilterCore::write` 不包装或吞掉下游错误，而是原样返回 `CoreError`；未命中时无论下游状态如何都返回 `Ok(())`。
- 匹配是无锚点、区分大小写的普通子串匹配，不理解 Rust/Go 模块边界、目录边界或正则表达式。测试明确证明 `/ingestor/ingestctrl` 命中 `.../ingestctrl.(*worker)...`，而追加尾斜杠的 `/ingestor/ingestctrl/` 不命中。
- 当前 Rust 生产调用方来自 `Location::caller().file()`，通常是文件路径；Go 原版读取的是包含包路径的 `Caller.Function`。因此配置片段必须同时考虑 Rust 实际路径形状，不能仅凭 Go 函数名格式新增规则。
- 同名 JSON 键没有拒绝或告警，后插入值覆盖先插入值。`logger_name` 为空时省略 `logger` 键。

## 并发与资源生命周期

`Core: Send + Sync` 和 `Arc<dyn Core>` 允许 logger/core 在工作线程之间共享；`FilterCore::write` 只读过滤向量，没有锁、通道、异步任务或事务。创建、`with`、`named` 都返回拥有独立过滤向量的新包装器，因此不会就过滤配置发生并发修改。

资源的实际生命周期由下游决定。当前 `OutputCore` 的级别通过共享 `Arc<RwLock<Level>>` 动态更新，目的地和字段则在派生时克隆；文件句柄并不长期保存在本文件或 `OutputCore` 中，而是每次写入时打开追加（`log.rs:185-240`）。`FilterCore` 不缓冲、不重试、不刷新，也不改变下游写入的同步性质。若下游 `Core` 内部锁中毒或 I/O 失败，行为由下游实现负责；过滤命中时错误沿调用栈返回，但 `Logger::log` 当前以 `let _ = ...` 忽略最终写入结果（`log.rs:104`）。

## 与 Go 版本的对应关系

Go 对照文件是 [filter.go](filter.go)，测试是 [filter_test.go](filter_test.go)。两版的核心结构一致：`FilterCore` 嵌入/持有下游 core 和过滤字符串，`With` 在保留过滤规则的情况下派生下游，`Write` 对调用方标识逐项执行子串包含判断，命中即转发，未命中返回成功。Rust 的 `NewFilterCore` 保留 Go 风格名称以降低迁移接线差异。

主要差异如下：

- Go 实现满足 `zapcore.Core`，通过 `Check` 把包装器加入 `CheckedEntry`；Rust 的自定义 `Core` 没有 `Check`，`Logger::log` 先调用 `enabled`，再直接调用 `write`。
- Go 的调用方是启用 `zap.AddCaller()` 后的 `entry.Caller.Function`；Rust 的 `Logger::log` 用 `#[track_caller]` 和 `Location::caller().file()` 填充同名语义字段。显式测试构造时，两版都可直接提供完整函数路径。
- Go 文件只实现过滤包装器，日志级别、字段和编码来自 zap；Rust 为摆脱 zap 依赖，在同一文件提供这些基础类型和 JSON 编码。
- Go benchmark 实测比较 `strings.Contains` 和正则；Rust 测试文件只保留两种循环形状的普通辅助函数，稳定测试框架下没有实际 benchmark，也没有引入 `regex` crate。
- Rust 的 `named` 是自定义 trait 的额外能力，并显式保留过滤器；Go 依靠 zap core/logger 的既有命名能力。

[filter_test.rs](filter_test.rs) 与 Go 测试对齐了无过滤基础输出、不命中丢弃、命中放行、`with` 字段保留、字段文本不能冒充调用方及尾斜杠边界；迁移测试又独立覆盖了调用方过滤和 `Contains` 语义。

## 扩展指南

- 新增日志级别时，应同步修改 `Level` 枚举、`parse`、`capital`、排序预期及独立测试；还需检查 [log.rs](log.rs) 的级别门控和配置解析。不能只增加枚举成员，否则解析或 JSON 名称会不完整。
- 新增字段类型时，优先在 `Field` 上增加构造器并继续产出 `serde_json::Value`；在 [filter_test.rs](filter_test.rs) 或 [log_test.rs](log_test.rs) 增加独立测试，不要把测试内嵌进生产文件。若改变重复键、顺序或跳过规则，要同时评估 Go 兼容 JSON 和 `serde_json` 的 `preserve_order` 依赖。
- 修改过滤算法时，主要接入点是 `Core for FilterCore::write`。必须保留下游错误传播、`with`/`named` 后规则不丢失，并同步 Rust 的 [filter_test.rs](filter_test.rs)、[migration_aster_unit_test.rs](migration_aster_unit_test.rs) 和 Go 对照测试意图。路径规范化、前缀/正则匹配会改变当前 `Contains` 合约，并可能增加每条日志的 CPU 或分配开销。
- 修改调用方采集方式时，应在 [log.rs](log.rs) 的 `Logger::log` 接线，而不是让 `FilterCore` 自行回溯栈；同时验证生产白名单既能命中 Rust 路径，也不会意外放行相似子串。
- 若需要运行时更新过滤规则，不能直接给现有 `Vec<String>` 增加可变入口；需要先设计共享状态、锁开销和派生 core 的一致性。目前的不可变克隆模型是并发安全边界。
- 若增加新的下游 core，实现必须满足 `Send + Sync`，并明确 `with`、`named`、错误传播与资源生命周期。过滤器只负责决定是否调用下游，不提供缓冲、重试或容错。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/lightning/log` 识别目标 `filter.rs` 的 30 个符号及同目录 Rust/Go 实现与测试。
- RustCodeGraph `node --file pkg/lightning/log/filter.rs`：核对了文件全部 294 行，包括所有类型、函数、trait 和 impl；`query FilterCore`、`query NewFilterCore`、`query encode_json` 核对了生产入口、测试引用和 Go 同名符号。
- RustCodeGraph 文件节点：读取并核对了 [log.rs](log.rs)、[lib.rs](lib.rs)、[filter_test.rs](filter_test.rs)、[migration_aster_unit_test.rs](migration_aster_unit_test.rs)、[filter.go](filter.go) 和 [filter_test.go](filter_test.go)。
- Cargo 边界：读取 [Cargo.toml](Cargo.toml)，确认 crate 名、库入口、`serde_json` 依赖和 Go 包迁移元数据。
- 人工事实复核：确认文档区分过滤器与下游输出职责，覆盖实际生产接线、字段/状态、错误与并发边界、Go 差异和安全扩展位置；没有把测试辅助 benchmark 描述为已接线性能基准。
- 结构验证按任务规定的命令执行，要求目标文件存在且恰有十一个固定二级标题；本任务是纯文档分析，按计划不运行 Cargo。
