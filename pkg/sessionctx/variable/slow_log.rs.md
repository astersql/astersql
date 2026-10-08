# `pkg/sessionctx/variable/slow_log.rs`

## 文件定位

该文件属于 `astersql-sessionctx-variable` crate，由 `pkg/sessionctx/variable/lib.rs` 以公开模块 `slow_log` 暴露。它位于会话变量层与执行器慢日志路径的边界：一方面定义一条慢查询日志的共享数据载体和基础文本格式，另一方面定义可配置慢日志规则的解析、字段取值与阈值匹配协议。执行器侧 `pkg/executor/adapter_slow_log.rs` 消费这些协议，负责组合会话/全局规则、补齐字段、判断是否写日志，并最终调用 `SessionVars::SlowLogFormat`。

crate 边界由 `pkg/sessionctx/variable/Cargo.toml` 确认：本文件直接使用标准库集合/同步/时间类型、`crc` 的 CRC64-ECMA 实现，以及工作区 `astersql-util-execdetails`（依赖别名 `execdetails`）。会话数据来自同 crate 的 `session::{SessionVars, RewritePhaseInfo}`。本文件不负责慢日志文件轮转、logger 配置、SQL 执行或完整执行计划生成。

## 核心职责

1. 以 `SlowQueryLogItems` 汇集格式化和规则匹配所需的事务、SQL、耗时、执行详情、资源消耗、RU、存储来源等数据。
2. 由 `SessionVars::SlowLogFormat` 按稳定字段顺序生成 `# key: value` 行，并把 SQL 放在末尾、确保分号结尾。
3. 以 `Threshold`、`SlowLogCondition`、`SlowLogRule`、`SlowLogRules`、`GlobalSlowLogRules` 和 `SessionSlowLogRules` 表示类型化阈值、单规则 AND 条件、多规则集合及会话缓存状态。
4. 由 `SlowLogFieldAccessor` 把字段名映射为阈值解析器、可选的惰性 Setter 和匹配函数，使执行器只采集当前规则需要的昂贵字段。
5. 解析会话级及全局规则字符串，限制字段类型、规则数量和 `Conn_ID` 的使用范围，并为全局规则生成 CRC64 哈希以支持变更检测。
6. 通过 `RegisterPlanDigestAccessor` 保留跨 crate 扩展点，让执行器在包初始化阶段注入其拥有的计划摘要逻辑，避免 sessionctx 反向依赖 planner/executor。

## 主要符号

- 字段常量：`SlowLogTxnStartTSStr`、`SlowLogQueryTimeStr`、`SlowLogDigestStr`、`SlowLogKVTotal`、`SlowLogPDTotal`、流量字段、`SlowLogRRU`/`SlowLogWRU` 等定义日志/规则使用的外部字段名；`UnsetConnID = -1` 表示未绑定具体连接的规则。
- `JSONSQLWarnForSlowLog`：慢日志警告的序列化形状；当前 `SlowQueryLogItems::Warnings` 保存该类型，但本文件的 `SlowLogFormat` 尚未输出警告字段。
- `SlowQueryLogItems`：一次慢日志处理的可变快照。`ExecDetail` 保存 SQL 执行聚合详情，`KVExecDetail` 保存 TiKV/PD 等待和流量计数，`CopTasks` 保存 cop task 汇总；其余字段覆盖 SQL、计划、资源、重试、RU 与存储来源。
- `writeSlowLogItem`、`duration_seconds`：基础格式化辅助；时长以秒表示，整数秒不带小数点。
- `SessionVars::SlowLogFormat`：本文件的格式化入口。它有条件跳过多数空值/零值字段，但始终输出若干布尔和基础字段；SQL 不带分号时补一个分号。
- `Threshold`：`String`、`Int`、`UInt`、`Float`、`Bool` 五种阈值，避免运行时无类型值。
- `SlowLogRule::threshold`：按字段查找单条规则中的阈值，主要服务测试和检查。
- `SlowLogFieldAccessor`：`Parse` 负责字符串到 `Threshold`；`Setter` 可从执行上下文/会话惰性填充 `SlowQueryLogItems`；`Match` 执行等值或“大于等于”判断。
- `SlowLogRuleFieldAccessors`：`LazyLock<BTreeMap<...>>` 全局字段注册表。键统一为 ASCII 小写；内建项由 `build_slow_log_rule_field_accessors` 生成。
- `RegisterPlanDigestAccessor`：把执行器提供的 factory 写入 `OnceLock`。`pkg/executor/adapter_slow_log.rs::SLOW_LOG_PACKAGE_INIT` 注册 `plan_digest_accessor`。
- `ParseSlowLogFieldValue`：大小写不敏感地查找字段 accessor，并调用对应阈值解析器。
- `ParseSessionSlowLogRules`、`ParseGlobalSlowLogRules`：公开规则解析入口；前者拒绝 `Conn_ID`，后者允许连接级分组并计算规范化规则串的 CRC64-ECMA 哈希。
- `MatchEqual`、`MatchEqualBool`、`matchGE*`、`matchZero`：字符串/布尔等值、数值下限及缺失执行详情时的零阈值语义。

## 执行流程

格式化流程如下：调用方构造或补齐 `SlowQueryLogItems`；`SlowLogFormat` 先写事务时间戳、keyspace、连接与重试信息，再写查询时间和可选 IA/read-pool 详情；随后写数据库、digest、内存/磁盘、计划来源、结果状态、计划文本、资源组、存储来源和 RU；最后写前序语句并原样追加 SQL，必要时补分号。`pkg/executor/adapter_slow_log.rs::WriteForcedSlowLogTo` 是已确认的上游写出入口之一，它用慢查询 logger 输出该字符串；`pkg/session/runtime/dispatch.rs` 和 `control.rs` 构造/延后补齐响应耗时后调用执行器入口。

规则解析流程由 RustCodeGraph 确认为 `ParseSessionSlowLogRules`/`ParseGlobalSlowLogRules → parse_slow_log_rule_set → parse_slow_log_rule_entry → ParseSlowLogFieldValue`。输入先按分号切成最多十条规则，再按逗号扫描 `field:value` 片段；字段名归一为小写，值去除首尾单/双引号并按 accessor 类型解析。同一规则中重复字段写入 `BTreeMap`，因此后值覆盖前值；条件按字段排序形成确定性编码。会话入口只取 `UnsetConnID` 组，全局入口按连接 ID 分组并对重新编码后的规则串计算哈希。

规则执行不在本文件内完成。`pkg/executor/adapter_slow_log.rs` 先合并会话规则、当前连接的全局规则和 `UnsetConnID` 全局规则所引用的字段；对带 Setter 的字段按需采集；匹配时一条规则内部条件取 AND，多条规则以及会话/全局来源之间取 OR。该执行器随后补齐未预采集字段并填充执行侧数据。

## 数据与状态

规则集合采用 `BTreeMap`/`BTreeSet`，因此字段集合、连接分组和编码顺序确定，便于稳定哈希与测试。`SlowQueryLogItems` 默认值代表“尚未采集或值为零”；访问器对缺失 `ExecDetail`、`KVExecDetail`、scan/commit detail 的数值字段采用 `matchZero`，所以只有零数值阈值能在缺失详情时匹配。负的底层计数经 `uint64FromNonNegative` 被视为无有效无符号值，而不是转换成大整数。

`SessionSlowLogRules` 保存会话规则、已合并的有效字段、上次看到的全局规则哈希和刷新标志。该文件只定义数据形状和构造器 `NewSessionSlowLogRules`；刷新逻辑在执行器 `updateAllRuleFields`。`GlobalSlowLogRules::raw_rules_hash` 是对规范化字符串而不是用户原始输入计算的 CRC64，因此空白、字段大小写及重复字段等经解析/重编码后的差异才决定缓存变化。

## 依赖与调用关系

上游主要包括：

- `pkg/executor/adapter_slow_log.rs`：读取 `SlowLogRuleFieldAccessors`、`GlobalSlowLogRules` 和 `SlowQueryLogItems`；实现字段预采集、AND/OR 匹配、最终补齐和日志写出；在平台初始化段调用 `RegisterPlanDigestAccessor`。
- `pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/control.rs`：构造并暂存协议级慢日志项，在响应完成后补 `WriteSQLRespTotal` 并调用执行器写日志入口。
- `pkg/sessionctx/variable/session.rs`：把 `SessionSlowLogRules` 持有在 `SessionVars` 中，并提供格式化所需的数据库、连接 ID、statement context 和耗时状态。

下游主要包括：

- `execdetails::execdetails`：cop/commit/scan/read-pool、RU 等聚合执行详情及字段名。
- `execdetails::util::context`：`SlowLogExecContext`，供 Setter 查找语句响应详情及 TiKV 执行详情。
- `crc::Crc<CRC_64_ECMA_182>`：全局规则规范化字符串的变更哈希。
- 标准库 `Arc`：让 Setter/Match 闭包可共享且满足 `Send + Sync`；`LazyLock`/`OnceLock`：管理全局只初始化状态。

RustCodeGraph 显示目标文件被执行器、point-get、session runtime 及相关测试等多个文件引用；精确本地检索确认实际日志格式化调用为 `adapter_slow_log.rs::WriteForcedSlowLogTo → SessionVars::SlowLogFormat`，规则消费集中在同文件的 `PrepareSlowLogItemsForRules`、`MatchSessionVars`、`ShouldWriteSlowLog` 和 `SetSlowLogItems`。

## 错误处理与边界

公开解析函数使用 `Result<_, String>`。未知字段、类型解析失败、负整数/负浮点阈值、NaN/无穷浮点、会话规则中的 `Conn_ID`、完全不含可识别 `field:value` 的条目以及超过十条规则都会报错。空输入返回空规则；分号产生的空条目被跳过。布尔值只接受 Rust 实现列出的 `1/0`、`t/f` 及三种大小写形式的 `true/false`。

解析器为贴近 Go 正则行为，会在每个逗号片段的冒号前寻找末尾 ASCII 单词，而不要求字段从片段开头开始；这使 `ignored prefix DB:TeSt` 仍可识别 `DB`。但按逗号直接切分意味着带逗号的引号值不会作为一个整体保存；相关 Go/Rust 测试把此行为作为当前兼容边界。`Conn_ID` 的 `u64` 在全局路径转为 `i64` 使用 `as`，因此超出 `i64::MAX` 的值会按 Rust 转换语义回绕；当前测试未证明超大连接 ID 的预期契约，扩展时应先与 Go 的 `uint64 → int64` 行为一起核验。

格式化使用 `writeln!` 写入 `String`，其错误被忽略是安全的，因为 `String` 的 `fmt::Write` 不产生常规 I/O 失败。相反，`DurationWaitTS` 锁中毒会在 accessor 匹配时以 `expect("wait TS lock poisoned")` panic；这是当前明确的失败策略。`RegisterPlanDigestAccessor` 忽略重复 `OnceLock::set` 的错误，首个注册者永久生效。

## 并发与资源生命周期

`SlowLogRuleFieldAccessors` 在第一次访问时由 `LazyLock` 构造，之后只读共享。`PLAN_DIGEST_ACCESSOR` 是 `OnceLock<fn() -> SlowLogFieldAccessor>`；执行器必须在注册表首次初始化之前完成 plan-digest factory 注册，否则该字段不会进入已经构造的 map。执行器通过平台初始化段实现此顺序，测试也显式覆盖注册后的 plan-digest accessor。

accessor 闭包装在 `Arc<dyn Fn + Send + Sync>` 中，可跨线程共享；它们自身不持有每语句可变状态。KV 等待和流量字段以 `Ordering::Relaxed` 读取原子计数，语义是获取近似的统计快照，不建立额外同步关系。等待 TS 时长通过 `Mutex` 读取。`SlowQueryLogItems` 的取得、清零和对象池复用由 `pkg/executor/adapter_slow_log.rs` 管理，本文件不拥有池和日志文件句柄；各 `Box` 详情随 item 生命周期释放或在归池清零时释放。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/sessionctx/variable/slow_log.go`，完整行为测试对照位于 `pkg/sessionctx/variable/tests/slowlog/slow_log_test.go` 与同目录 Rust 文件。Rust 保留了 Go 的核心规则语义：字段名大小写不敏感、数值条件使用实际值大于等于阈值、字符串/布尔使用等值、单规则 AND、多规则 OR、规则最多十条、会话级禁止 `Conn_ID`、全局按连接分组、缺失详情可匹配零阈值，以及 CRC64-ECMA 哈希。

Rust 用 `Threshold` 取代 Go 的 `any`，用 `BTreeMap/BTreeSet` 取代 Go map，使重编码顺序稳定；用上下文类型安全查询替代 Go `context.Value` 的类型断言；用执行器注册 factory 的方式补入 Go 中由 executor `init` 添加的 `Plan_digest` accessor。`pkg/executor/adapter_slow_log.rs` 的初始化段是该 Go `init` 契约的 Rust 对应物。

当前 Rust `SlowLogFormat` 明确只输出本文件已拥有的字段，Go 函数还格式化用户/主机、预处理子查询、统计、cop/backoff、告警、CPU 等更完整字段。不能据此文件宣称 Go 的全部慢日志文本格式都已迁移；执行器或其他模块提供的详情也只有在实际接入格式化路径后才会出现。另一方面，Rust 已额外以类型化结构和显式可选值表达缺失状态，这属于实现机制差异，不改变已覆盖字段的外部语义。

## 扩展指南

新增规则字段时，应同时完成以下接线：定义或复用外部字段名；在 `build_slow_log_rule_field_accessors` 注册小写键、正确的 `Parse`、必要的惰性 `Setter` 与 `Match`；若使用 `field_kind`，同步类型分类；确认缺失详情匹配零还是不匹配；在独立测试文件中覆盖解析、采集、边界值与 AND/OR 行为。执行器拥有的数据应沿 `RegisterPlanDigestAccessor` 这类反向注册边界接入，避免给 sessionctx crate 引入 planner/executor 依赖。

新增格式化字段时，应在 `SlowQueryLogItems` 增加数据，确定零值是否省略、单位与字段顺序，再修改 `SlowLogFormat`；同步 `pkg/sessionctx/variable/slow_log_test.rs` 的格式测试，并对照 Go `SlowLogFormat` 及 `pkg/sessionctx/variable/tests/slowlog/slow_log_test.{go,rs}`。不要把测试写回生产源文件。若字段由 session runtime 或 executor 产生，还需同步其构造/补齐路径，尤其是延后到协议响应完成才可得的 `WriteSQLRespTotal`。

兼容风险集中在日志字段名/顺序/单位、规则规范化字符串及哈希、大小写策略、缺失详情的零阈值行为和初始化顺序。性能风险集中在无条件采集昂贵计划或执行详情；应继续依赖有效字段集合和 Setter 的惰性采集。并发字段新增时必须明确原子快照或锁的语义，不能把统计读取误作同步屏障。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`node --file pkg/sessionctx/variable/slow_log.rs` 读取完整 1,308 行并列出 12 个使用文件；`explore` 确认 Rust 规则解析链 `ParseSessionSlowLogRules → parse_slow_log_rule_set → parse_slow_log_rule_entry → ParseSlowLogFieldValue`，并定位执行器 `MatchSessionVars`；精确 `query` 确认会话/全局解析函数与 `RegisterPlanDigestAccessor` 的符号。
- 源与边界：`pkg/sessionctx/variable/slow_log.rs`、`pkg/sessionctx/variable/Cargo.toml`、`pkg/sessionctx/variable/lib.rs`、`pkg/sessionctx/variable/session.rs`。
- 上下游证据：`pkg/executor/adapter_slow_log.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/control.rs`。
- Go 对照：`pkg/sessionctx/variable/slow_log.go`、`pkg/sessionctx/variable/tests/slowlog/slow_log_test.go`。
- Rust 独立测试：`pkg/sessionctx/variable/slow_log_test.rs` 覆盖字段注册、正则式兼容片段、digest/等待 TS Setter、IA/read-pool、存储来源和 RU 格式；`pkg/sessionctx/variable/tests/slowlog/slow_log_test.rs` 覆盖 39 个 accessor、AND/OR 匹配、类型/负值/非有限值、重复字段、十条上限、会话/全局解析、plan digest 注册及 Go 对齐行为。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅运行固定 11 章节结构检查并人工复核引用和范围。
