# `br/pkg/registry/stubs.rs`

## 文件定位

`stubs.rs` 是 `astersql-br-pkg-registry` crate 的迁移期边界层，而不是 restore registry 状态机本身。crate 入口 `br/pkg/registry/lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 挂载它，并用 `pub use stubs::*` 将其符号平铺为包级 API；真正的注册、冲突检查和心跳流程位于同 crate 的 `registration.rs` 与 `heartbeat.rs`。

`br/pkg/registry/Cargo.toml` 的 `[lib] path = "lib.rs"`、`package.metadata.porting.go-package = "br/pkg/registry"` 和空 `[dependencies]` 表明该 crate 有意不直接接入 TiDB 的 session、domain、kv、table-filter 等重型 workspace 依赖。`stubs.rs` 因而以本地 trait、轻量值类型和少量兼容函数表达这些边界，使该 crate 能在当前移植环境（文件注释特别指出 darwin）独立编译。这里的类型是局部替身，不应被理解为对应 Go 子系统的完整 Rust 移植。

## 核心职责

文件承担五组职责：

1. 用 `Error`、`Result<T>`、`Context`、`SqlValue`、`Row` 和 `OptionFuncAlias` 提供 registry 内部统一的错误、取消、SQL 参数及结果行模型。
2. 用 `RestrictedSQLExecutor`、`Session`、`Storage`、`InfoSchema`、`Domain`、`Glue` 抽象注册流程所需的最小外部能力；`registration.rs::NewRestoreRegistry` 和事务路径只依赖这些接口。
3. 用 `TaskStatus`、`Database`、`Table`、`CIStr` 与裁剪后的 `PiTRIdTracker` 承载注册记录和冲突检查需要的数据。
4. 用 `ParseFilter`、`CaseInsensitive`、`MatchSchema`、`MatchTable` 实现 table-filter 的最小兼容子集，并在匹配前处理系统库开关和 BR 临时库名前缀。
5. 用原子全局量提供 stale 检查、resetting 等待和跳过睡眠的测试钩子；另外用 `WithInternalSourceType`/`InternalTxnBR` 保留 Go 调用形状，但当前不写入真正的内部事务来源标记。

它的设计目标是支撑本 crate 当前使用到的可观察行为，而不是复刻所有上游 API。源码中的 “stand-in” 和 “minimal subset” 注释，以及 `PiTRIdTracker` 仅含名称映射这一事实，都是扩展时必须保留的边界说明。

## 主要符号

- `Error { msg, code }` 与 `Result<T>`：`new` 创建无错误码错误，`with_code` 创建带静态错误码错误；`Trace` 原样返回，`Annotate`/`Annotatef` 给消息加上下文并保留 `code`。`berrors::{ErrInvalidArgument, ErrTablesAlreadyExisted, ErrDatabasesAlreadyExisted}` 构造 BR 分类错误，相应 `is_*` 函数按错误码或若干消息片段识别。
- `Context`：用 `Arc<Mutex<Option<Error>>>` 共享取消原因；`Background` 建空上下文，`cancel` 写入错误，`Err` 克隆错误，`Done` 判断是否已有错误。`cancel` 不是公开 API，当前主要供同模块/测试使用。
- `SqlValue` 与 `Row`：`SqlValue` 支持 `Null/U64/I64/Bool/Str`，并提供宽松数值、布尔和字符串转换；`Row::{GetUint64, GetInt64, GetString}` 按列索引读取。它们是 registry SQL mock/适配层的数据协议。
- `RestrictedSQLExecutor::ExecRestrictedSQL`：表达带执行选项、SQL 文本和参数的查询并返回 `Vec<Row>`。`Session` 在此基础上增加 `ExecuteInternal` 和 `Close`。两者要求实现者可跨线程移动（前者继承 `Send`），而共享和串行化由上层 `Arc<Mutex<Box<dyn Session>>>` 完成。
- `Storage`、`MemStorage`、`InfoSchema`、`Domain`、`Glue`：分别提供最小存储标识、内存占位存储、表存在性查询、domain 的 store/infoschema 访问和 session 创建。`InfoSchema::TableByName` 以 `Ok(())` 表示存在，没有返回真实表对象。
- `CIStr { O, L }`/`NewCIStr`：同时保存原始值和小写值，供 `NewRestoreRegistry` 查询 `mysql.tidb_restore_registry`。
- `TaskStatus` 与 `TaskStatusRunning/Paused/Resetting`：把 Go 字符串枚举映射为静态字符串包装，支持 SQL 参数转换和显示。
- `DBInfo`、`TableInfo`、`Database`、`Table`：只保留冲突检查读取的名称字段，不包含 Go model/metautil 类型的完整元数据。
- `PiTRIdTracker`：只保存 `DBNameToTableNames: HashMap<String, HashSet<String>>`，提供 `TrackTableName` 和只读 getter。它不含 Go `br/pkg/utils/filter.go::PiTRIdTracker` 的 DB ID、table ID、partition ID 集合。
- `Filter`、私有 `FilterRule`/`ParsedFilter`、`ParseFilter`、`CaseInsensitive`：定义 schema/table 匹配接口，解析正负规则，支持 `*`、`?` 和反斜杠转义，并通过包装器进行大小写不敏感匹配。
- `IsSysDB`、`StripTempDBPrefixIfNeeded`、`MatchSchema`、`MatchTable`：识别 `mysql`、`sys`、`workload_schema`，剥离 `__TiDB_BR_Temporary_` 前缀，并根据 `with_sys` 决定是否屏蔽系统库。
- `err_table_not_exists`/`is_table_not_exists`：为 `NewRestoreRegistry` 提供本地 `schema:ErrTableNotExists` 哨兵及兼容识别。
- `set_stale_ticker_duration_ms`、`set_wait_resetting_sleep_ms`、`set_skip_sleep` 及对应读取/睡眠函数：通过 `AtomicU64`/`AtomicBool` 覆盖默认 60 秒 stale tick、5 秒 resetting sleep，或完全跳过睡眠。
- `WithInternalSourceType` 与 `InternalTxnBR`：保留 Go `kv.WithInternalSourceType(ctx, kv.InternalTxnBR)` 的调用点；当前函数原样返回 `Context`，常量值为 `"br"`。

## 执行流程

构造 registry 时，`registration.rs::NewRestoreRegistry` 先经 `Glue::CreateSession(Domain::Store())` 创建普通 session 与 heartbeat session，再调用 `Domain::InfoSchema().TableByName(ctx, NewCIStr("mysql"), NewCIStr("tidb_restore_registry"))`。只有 `is_table_not_exists` 识别出的缺表错误会被转化为 `table_exists = false`，其他错误由 `Error::Trace` 返回。

执行注册事务时，`Registry::execute_in_transaction` 先调用 `WithInternalSourceType` 保持内部 BR 事务的调用语义，再以 `ExecOptionUseCurSession` 执行 `BEGIN PESSIMISTIC`。闭包内所有查询通过 `RestrictedSQLExecutor` 和 `SqlValue` 传参、通过 `Row` 取值；闭包失败时尽力 `ROLLBACK` 并返回原错误，成功后 `COMMIT`。因此 `stubs.rs` 规定了 SQL 边界形状，但事务状态机在 `registration.rs`。

冲突检查时，`Registry::CheckTablesWithRegisteredTasks` 从历史注册记录取得过滤字符串，依次执行 `ParseFilter`、`CaseInsensitive`，再将过滤器传给 `checkForTableConflicts`。若给出 `PiTRIdTracker` 且名称映射非空，先遍历其中的库表名；否则根据命令和显式 `Database`/`Table` 列表匹配。每次 `MatchSchema`/`MatchTable` 都先剥离临时库前缀，并在 `with_sys == false` 时拒绝系统库，然后才委派给过滤器。

`ParseFilter` 对每条文本先 trim，忽略空行和 `#` 注释；前导 `!` 表示排除规则，余下内容必须能按第一个 `.` 分成非空 schema/table pattern。解析完成后反转规则，使原命令行中靠后的匹配规则优先。`ParsedFilter` 从前向后返回第一个匹配规则的 `positive` 值；没有规则命中时返回 `false`。`CaseInsensitive` 不重写规则集合，而是把调用转发给 inner filter 的 `Match*CaseInsensitive`。

等待相关流程从 `stale_ticker_duration` 或 `wait_resetting_sleep_duration` 读取全局覆盖值，随后由 `maybe_sleep` 决定是否调用 `std::thread::sleep`。值为零表示采用生产默认值，不表示零延迟；只有 `SKIP_SLEEP` 或传入零 `Duration` 才直接返回。

## 数据与状态

`Context` 的唯一可变状态是共享的 `Option<Error>`。克隆 `Context` 只克隆 `Arc`，所以各副本看到同一个取消原因；后续 `cancel` 会覆盖之前的错误，没有 parent/child、deadline、值传播或通知 channel。

`SqlValue`/`Row` 使用拥有所有权的值并在读取字符串时克隆。转换是刻意宽松的：字符串解析失败、`Null`、缺列或类型不匹配往往退化为 `0`、空字符串或 `false`；有符号/无符号互转使用 Rust `as`，可能发生补码重解释或截断语义。调用者不能把这些 accessor 当成严格 schema 校验。

`TaskStatus` 的有效状态没有由类型封闭：其 tuple 字段公开，任何 `&'static str` 都能构造状态。三个常量才是 registry 当前协议使用的值。`Database`/`Table` 和 `CIStr` 同样是最小数据投影；`CIStr::L` 用 Unicode `to_lowercase` 计算，不承担完整 TiDB 标识符排序规则。

`PiTRIdTracker` 的 map/set 去重表名，但保留传入字符串的大小写；实际冲突匹配通过 `CaseInsensitive` 过滤器消除大小写差异。它只用于 registry 的名称冲突路径，不能替代 Go tracker 的 ID 跟踪用途。

三个全局测试开关采用 `SeqCst` 原子顺序，因此跨线程可见，但它们属于整个进程而非某个 `Registry` 实例。测试修改后若不恢复，可能影响并行或后续用例。

## 依赖与调用关系

上游直接消费者如下：

- `br/pkg/registry/lib.rs` 声明并公开重导出全部符号。
- `br/pkg/registry/registration.rs` 导入 `Context`、session/domain/glue trait、SQL 值、状态、过滤器、tracker、错误分类、睡眠函数和内部事务标记。关键调用边包括 `NewRestoreRegistry -> Glue::CreateSession/InfoSchema::TableByName`、`execute_in_transaction -> RestrictedSQLExecutor::ExecRestrictedSQL`、`CheckTablesWithRegisteredTasks -> ParseFilter/CaseInsensitive/MatchSchema/MatchTable`。
- `br/pkg/registry/heartbeat.rs` 使用 `Context`、`Error`、`Result`、`Session`、`SqlValue`，并把 session 放进 `Arc<Mutex<Box<dyn Session>>>` 供后台心跳线程串行访问。
- `br/pkg/registry/parity_test.rs` 实现这些 trait 的内存版本，并验证过滤规则、状态、错误和时序钩子；`br/pkg/task/stream_test.rs` 也为 registry trait 提供测试实现。
- `br/pkg/task/stream.rs` 从 crate 根使用 `Registry`、`Context` 和 `Error`，说明这些桩经 `lib.rs` 重导出后已经构成跨 crate 的公开兼容面。

下游仅依赖 Rust 标准库：`HashMap/HashSet`、`Arc/Mutex`、原子类型和 `Duration/thread::sleep`。`Cargo.toml` 没有第三方依赖；Go 侧对应能力则分散在 `context`、`session`/`sqlexec`、`domain`/`kv`、`pkg/util/table-filter`、`br/pkg/utils`、`br/pkg/errors` 和 model/metautil 类型中。

RustCodeGraph 对文件的索引显示 120 个符号，并识别到 `registration.rs`、`heartbeat.rs`、`parity_test.rs` 及跨 crate 测试调用。对重名 `MatchTable`/`Session` 的全仓查询会混入其他 crate 和 Go 符号，因此本说明只采用路径限定后能与上述文件互相印证的调用边。

## 错误处理与边界

`Error::Trace` 不增加回溯信息；`Annotate`/`Annotatef` 只拼接字符串。`berrors::is_*` 和 `is_table_not_exists` 除错误码外还接受消息子串，这是兼容测试/桩错误的降级路径，也意味着相同文字可能被误分类。扩展错误体系时应优先保持稳定 code，并审查依赖字符串匹配的调用者。

`Context` 与 registry 上层的 session 锁都调用 `Mutex::lock().unwrap()`；如果持锁线程 panic 导致锁中毒，当前行为是继续 panic，而不是返回 `Error`。该 Context 也没有阻塞式 `Done` channel，调用者只能轮询 `Done`/`Err`。

过滤解析只覆盖 registry 当前用到的 table-filter 子集。它不应被宣称支持 Go `pkg/util/table-filter` 的全部语法；当前明确支持 `schema.table`、`!`、`#`、`*`、`?` 和反斜杠转义。缺少 `.` 或任一 pattern 为空会返回语法错误。递归通配符匹配的时间/栈消耗会随 pattern 和输入增长，不适合未经限制的超长不可信规则。

`MatchSchema` 对排除规则有一个与 table-filter 语义相关的特殊条件：只有 positive 规则或 table pattern 为 `*` 的命中才能决定 schema 结果。没有匹配默认拒绝。系统库判断要求小写名；Rust wrapper 在判断前显式 `to_lowercase`，而 Go `utils.IsSysDB` 的契约名称本身就是 `dbLowerName`。

`Row` 越界和类型不匹配不会报错，这便于轻量测试，但可能掩盖 SQL 列顺序漂移。新增生产读取点时应通过独立测试覆盖列索引、类型和空结果，不能依赖默认零值代表合法数据库值。

## 并发与资源生命周期

`Context` 通过 `Arc<Mutex<...>>` 可在 clone 之间共享取消状态；`Storage`、`InfoSchema`、`Domain`、`Glue` 要求 `Send + Sync`，`RestrictedSQLExecutor` 要求 `Send`。`Session` 本身不要求 `Sync`，实际由 `registration.rs::Registry` 和 `heartbeat.rs::HeartbeatManager` 放入 `Arc<Mutex<Box<dyn Session>>>` 后串行访问。锁的获取、session `Close` 和 heartbeat worker 的启停不在本文件实现。

`stubs.rs` 不创建线程；唯一阻塞操作是 `maybe_sleep` 的同步睡眠。后台线程生命周期由 `HeartbeatManager::{Start, Stop}` 管理，`Registry::{StartHeartbeatManager, StopHeartbeatManager, Close, Unregister, PauseTask}` 负责调用。`parity_test.rs` 的 start/stop 用例证明 Stop-before-Start 不更新心跳，而 Start 后 Stop 会等待并观察到更新。

时序配置使用静态原子量而非锁，读取/写入均为 `Ordering::SeqCst`。这保证可见性，不保证测试隔离；并行测试若同时调用 setter 会发生逻辑竞争。默认值通过“原子值为 0”的哨兵表达，故无法用 setter 将 stale/reset sleep 配成真正的 0ms；需要零延迟时使用 `set_skip_sleep(true)`。

`Error`、`SqlValue`、`Row` 等拥有自身数据，没有外部借用生命周期。`Filter` trait object 是 `Send + Sync`，`CaseInsensitive` 拥有 inner filter，随外层对象释放；`PiTRIdTracker` 也完全拥有 map/set，不涉及外部资源。

## 与 Go 版本的对应关系

registry 的直接 Go 文件 `br/pkg/registry/registration.go` 使用真实 `glue.Glue`、`domain.Domain`、`session.Session`、`sqlexec.RestrictedSQLExecutor`、`kv.WithInternalSourceType`、`table-filter` 和 `br/pkg/utils`。Rust 将这些跨包依赖压缩到本文件的最小 trait/类型，再由 `registration.rs` 保持调用顺序和状态分支。

明确对齐项包括：

- `TaskStatusRunning/Paused/Resetting` 对应 Go `registration.go` 的三个字符串常量。
- `MatchSchema`/`MatchTable`、`IsSysDB`、`StripTempDBPrefixIfNeeded` 对应 `br/pkg/utils/filter.go` 与 `schema.go`；系统库集合和临时前缀相同。
- `ParseFilter`/`CaseInsensitive` 模拟 Go `filter.Parse` 与 `filter.CaseInsensitive` 在 registry 冲突检查中的所需行为；`parity_test.rs::table_filter_preserves_go_rule_order_and_parser_contract` 验证末条匹配规则优先、注释/空行和缺点号错误，`case_insensitive_filter_normalizes_patterns_and_inputs` 验证 pattern 与输入同时忽略大小写。
- `Error`/`berrors` 对应 `errors.Trace/Annotate[f]` 和 BR 错误分类，但没有真实 error stack，仅保存消息与可选 code。
- `WithInternalSourceType` 保留 Go 调用位置却是 no-op；Rust Context 不携带内部 source value。这是当前迁移缺口，不是等价实现。

必须特别区分 `PiTRIdTracker`：Go `br/pkg/utils/filter.go` 同时维护 `DBIds`、`TableIdToDBIds`、`PartitionIds` 和 `DBNameToTableNames`，Rust 这里只移植 registry 冲突分支读取的最后一项。因此它仅在此 crate 的名称冲突场景对齐，不能用于 Go tracker 的完整恢复过滤流程。

`InfoSchema::TableByName`、`Storage`、`Database`、`Table` 和 `CIStr` 也都是投影类型。把 crate 接到真实 TiDB Rust 子系统时，应替换/适配这些边界，而不是继续向桩里复制完整 domain、model 或 session 实现。

## 扩展指南

新增 registry SQL 操作时，优先扩展 `RestrictedSQLExecutor`/`Session` 所需的最小稳定能力，并在独立测试文件实现相应 fake；不要把 SQL 引擎逻辑写进 `stubs.rs`。若新增结果列，必须同步覆盖 `SqlValue`/`Row` 的类型和越界行为，避免默认零值掩盖协议错误。

新增过滤语法前先核对 Go `pkg/util/table-filter` 的真实解析与优先级，并扩展 `br/pkg/registry/parity_test.rs` 的独立用例；尤其需要覆盖正负规则交错、转义、大小写、临时系统库名及 `with_sys`。若需求超出当前最小子集，更安全的方向是引入已移植的 canonical filter crate，而不是在局部桩中逐步制造第二套不完整解析器。

若 registry 需要 PiTR ID 能力，应在对应 canonical Rust utils 实现完整 tracker，再让本 crate 依赖其已发布接口；不要把 Go tracker 的其他字段随意塞入这个名称专用桩。至少同步验证跨库 rename、partition ID、DB/table ID 组合和名称冲突路径。

若接入真实 Context/internal transaction source，应保持 `registration.rs` 现有 `WithInternalSourceType(..., InternalTxnBR)` 调用点，并增加能够观察 source 标记的测试。替换 session/domain/glue trait 时，也要保留 `NewRestoreRegistry` 创建两个 session、缺表错误的特殊处理、事务 begin/rollback/commit 顺序，以及 Close/heartbeat 的资源释放契约。

时序 setter 仅应用于测试。新增测试应放在独立的 `parity_test.rs`、`heartbeat_test.rs` 或同目录新的 `*_test.rs`，不得嵌入生产源文件；同时应串行化或用 guard 恢复全局原子值，避免跨测试污染。生产默认时长如有变化，需同步 Go 常量/行为和 stale/resetting 相关测试。

## 验证依据

本说明基于以下直接证据：

- 源码全貌：`br/pkg/registry/stubs.rs`（846 行），核对了所有公开类型、trait、函数、常量、私有过滤规则及条件/原子状态；文件没有条件编译项。
- crate 边界：`br/pkg/registry/Cargo.toml`、`br/pkg/registry/lib.rs`，确认 crate 路径、Go 包映射、空依赖、模块挂载和公开重导出。
- Rust 主调用链：`br/pkg/registry/registration.rs` 的 imports、`NewRestoreRegistry`、`execute_in_transaction`、`ResumeOrCreateRegistration`、`CheckTablesWithRegisteredTasks`/`checkForTableConflicts`；`br/pkg/registry/heartbeat.rs` 的 session/context 使用。
- Rust 独立测试：`br/pkg/registry/parity_test.rs` 的 `case_insensitive_filter_normalizes_patterns_and_inputs`、`table_filter_preserves_go_rule_order_and_parser_contract`、`go_rust_public_contract_matches` 及内存 session/trait 实现；另核对 `br/pkg/registry/heartbeat_test.rs` 的存在和 `br/pkg/task/stream_test.rs` 对 registry SQL trait 的实现引用。
- Go 对照：`br/pkg/registry/registration.go` 的状态常量、过滤器创建、internal source 与冲突检查调用；`br/pkg/utils/filter.go` 的完整 `PiTRIdTracker` 及 `MatchSchema`/`MatchTable`；`br/pkg/utils/schema.go` 的系统库集合和临时库前缀逻辑。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/registry` 确认目标及相邻 Rust/Go 文件；`node --file br/pkg/registry/stubs.rs` 分段读取并显示目标有 120 个符号；路径限定的 `explore`/callers/callees 查询确认 `registration.rs`、`heartbeat.rs`、`parity_test.rs` 与 `stubs.rs` 的直接关系。全仓重名符号结果仅作候选，未据此推导 registry 专属行为。

结构验证使用任务指定命令，要求文件存在且上述固定二级标题恰好 11 个。任务为纯文档分析，按计划不运行 Cargo；行为结论通过源码、调用边、Go 对照和现有独立测试代码交叉核验，不把测试是否实际执行宣称为本次证据。
