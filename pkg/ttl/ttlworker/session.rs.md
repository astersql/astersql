# `pkg/ttl/ttlworker/session.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-ttl-ttlworker`，由同目录 `lib.rs` 的 `pub mod session` 暴露。它定义 TTL worker 与 SQL 会话之间的轻量边界：参数/结果值、物理表快照、可变会话状态、执行 trait、会话准备与恢复，以及执行后的 TTL 元数据有效性校验。该 crate 的直接常规依赖只有 `astersql-ttl-cache`；`Cargo.toml` 中大量真实 TiDB 组件依赖受 `cfg(windows)` 限定，因此本文件当前实现主要是自包含的迁移抽象，而不是 Go `session.go` 中系统会话池和完整 TiDB session 的直接封装。

Rust 生产侧已确认的直接使用点是 `scan.rs` 与 `del.rs` 对 `WorkerSession::expiration_predicate`、`WorkerSession::execute_with_ttl_job` 的调用。RustCodeGraph 对本文件标为 47 个符号，但精确 `callers/callees` 查询没有返回这些 trait/方法的静态调用边；上述接线由文本引用和相邻源码核实。`prepare_session_checked`、`prepare_scan_session_checked`、`TableSession::execute_sql_with_check` 和 `validate_ttl_work` 在当前 Rust crate 内只发现测试引用，不能据此声称它们已接入生产调度主链。

## 核心职责

1. 用 `Datum`、`Row` 和 `WorkerSession::execute` 统一 TTL SQL 的参数与返回行表示。
2. 用 `PhysicalTable` 保存一次 TTL 工作需要的逻辑表、物理分区、键列、TTL 列、启用状态与过期间隔快照，并通过 `expire_time` 饱和计算过期水位。
3. 用 `SessionState` 建模池化会话中会被 TTL 临时改写的系统变量、事务标志、时区、扫描选项和当前 `ttl_job_id`。
4. 提供内存态的 `prepare_session`/`restore_session` 与 `prepare_scan_session`/`restore_scan_session`，以及会真实执行 SET/SELECT/ROLLBACK、可失败并清理的 `*_checked` 版本。
5. 通过 `execute_with_ttl_job` 在单条用户表 SQL 周围临时设置作业归属，并无论执行成功失败都恢复原值。
6. 通过 `validate_ttl_work` 和 `TableSession::execute_sql_with_check` 在语句建立事务元数据快照后判定 TTL 工作是否仍安全。

## 主要符号

- `Datum`：SQL 单元格的五种简化表示：`Null`、`Integer(i64)`、`Unsigned(u64)`、`Text(String)`、`Bytes(Vec<u8>)`；`Row` 是 `Vec<Datum>`。
- `PhysicalTable`：TTL 物理目标快照。`table_id` 表示逻辑表，`physical_id` 表示表或分区；`partition_name`、`schema`、`table`、`key_columns`、`ttl_column` 用于定位和构造 SQL；`ttl_enabled`、`expire_after_seconds` 用于继续工作校验。`definition_version` 当前只作为数据字段保存，`validate_ttl_work` 不比较它。
- `PhysicalTable::expire_time(now)`：用 `saturating_sub` 计算 `now - expire_after_seconds`，避免无符号下溢。
- `SessionState`：可克隆快照，包含变量映射、显式事务标志、UTC 偏移、内部 SQL 用户表扫描标志、DistSQL 并发度、分页开关和 TTL 作业 ID。
- `ExpirationPredicate`：一次扫描/删除所捕获的过期表达式与参数。默认实现返回表达式 `FROM_UNIXTIME(%?)` 和 `Datum::Unsigned(unix)`。
- `WorkerSession`：核心 trait。实现者必须提供 `state`、`state_mut`、`execute`；其余方法有默认行为，包括作业归属、状态刷新、过期谓词、全局开关、事务执行和禁止复用。
- `SessionError`：区分一般 `Execute`、不应重试的 `NonRetryable`，以及 `TableChanged`、`TtlDisabled`、`ExpireIntervalChanged` 三类元数据终止原因。
- `prepare_session` / `restore_session`：纯内存状态快照切换；前者关闭事务标志、关闭 SQL 重试、开启 1PC/异步提交、必要时补齐三种读引擎并固定 UTC。
- `prepare_session_checked` / `restore_session_checked`：按 Go 的语句顺序执行生命周期 SQL，记录已尝试变量，失败时尽量全部恢复并调用 `avoid_reuse`。
- `ScanSessionState` 与扫描准备/恢复函数：仅保存和改写内部用户表访问、扫描并发度和分页开关。
- `execute_lifecycle_sql`：捕获 `execute` 的 panic；panic 时先 `avoid_reuse`，再原样恢复展开。
- `validate_ttl_work`：验证表存在、逻辑/物理 ID、TTL 开关、时间列和过期水位。
- `TableSession`：绑定底层会话、起始表快照和作业过期水位；`execute_sql_with_check` 执行事务 SQL 后总是再做元数据校验。

## 执行流程

池化会话完整准备流程由 `prepare_session_checked` 表达：先 `refresh_state`，克隆原状态；依次执行关闭重试、开启 1PC、开启异步提交；再执行 `ROLLBACK`，读取并保存原时区后设为 UTC；仅当原读引擎集合缺少 TiDB、TiKV 或 TiFlash 时，读取原值并设置完整集合。全部成功后再调用 `prepare_session` 同步内存态并返回原快照。任一步失败都会按 `attempted` 列表恢复已经触碰的变量、合并清理错误，并将会话标为不可复用。

扫描准备由 `prepare_scan_session_checked` 先保存三项扫描状态并在内存中设为内部扫描、并发 1、禁用分页，然后依次执行两条 SET。失败时调用 `restore_scan_session_checked` 做尽力恢复，并禁止复用。正常结束时，恢复函数即使第一条 SET 失败也会继续执行第二条，最后统一返回错误。

生产扫描路径见 `scan.rs::ScanTask::execute_with_checkpoint`：开始时只调用一次 `expiration_predicate`，随后每页复用同一表达式和参数；每次 SQL 通过 `execute_with_ttl_job` 临时写入 `ttl_job_id`。生产删除路径见 `del.rs::DeleteTask::do_delete`：同样一次捕获过期谓词，并为各批次删除临时设置作业 ID。两条当前 Rust 路径都没有直接构造 `TableSession`，也没有调用本文件的准备/恢复函数。

`TableSession::execute_sql_with_check` 自身的顺序是：先检查 `ttl_jobs_enabled`；调用 `execute_in_transaction` 并暂存结果；无论 SQL 成功或失败，都用当前元数据调用 `validate_ttl_work`；若元数据错误，优先返回该错误，否则返回原 SQL 结果。成功结果固定携带 `false`，表示无需重试。

## 数据与状态

`SessionState` 是完整可克隆快照。普通 `prepare_session`/`restore_session` 会整体替换状态，适合轻量实现和测试；checked 路径则同时维护 SQL 后端状态与本地镜像。`prepare_session_checked` 特意把 `previous.in_transaction` 改成 `false` 后返回，因为已经执行 `ROLLBACK`，恢复系统变量时不能把已经终止的事务伪装为仍在进行；`session_integration_test.rs::pooled_session_prepare_and_restore_preserve_every_setting` 验证了这一不变量。

`execute_with_ttl_job` 使用 `mem::replace` 保存旧作业 ID，调用 `execute` 后再恢复。因此普通 `Result::Err` 不会泄漏归属；但这里没有 panic guard，若实现者的 `execute` panic，恢复语句不会运行。生命周期 SQL 则通过 `execute_lifecycle_sql` 专门处理 panic 并禁止复用。

过期谓词应在一轮扫描或一个删除任务开始时捕获一次，而非每页/每批重新求值；`scan.rs` 和 `del.rs` 都遵守这一点，从而让各批次使用同一过期边界。`PhysicalTable::expire_time` 的饱和语义保证保留期大于当前时间时水位为 0。

## 依赖与调用关系

- 模块入口：`pkg/ttl/ttlworker/lib.rs` 声明 `pub mod session`，并把 `session_test.rs`、`session_integration_test.rs` 作为独立测试模块接入，测试没有内嵌在生产文件中。
- 上游 Rust：`scan.rs::ScanTask::execute_with_checkpoint` 调用 `expiration_predicate` 与 `execute_with_ttl_job`；`del.rs::DeleteTask::do_delete` 调用相同两个 trait 方法。
- 下游边界：所有 SQL 最终通过实现者提供的 `WorkerSession::execute`；事务语义可由 `execute_in_transaction` 覆盖；失败会通过 `SessionError` 向上返回；池生命周期通过 `avoid_reuse` 通知实现者。
- 内部调用：`prepare_session_checked` 调用 `refresh_state`、`execute_lifecycle_sql`、`read_lifecycle_variable`、`restore_attempted_variables` 和 `prepare_session`；扫描 checked 路径调用对应的内存态准备/恢复和生命周期 SQL 包装；`TableSession::execute_sql_with_check` 调用 `ttl_jobs_enabled`、`execute_in_transaction`、`validate_ttl_work`。
- Cargo 边界：`pkg/ttl/ttlworker/Cargo.toml` 将本目录建为独立 library crate；非 Windows 常规依赖仅列 `astersql-ttl-cache`。本文件本身只导入标准库 `BTreeMap`，没有直接使用该 crate 依赖。
- 图查询限制：RustCodeGraph 的精确 `callers/callees` 对这些符号返回空结果，不能把“无静态边”解释为“无调用”；生产引用通过 `rg` 与相邻源码行复核。

## 错误处理与边界

`SessionError` 的三类元数据错误会使 TTL 工作终止：当前表缺失、逻辑/物理 ID 或 TTL 列改变归为 `TableChanged`；关闭 TTL 为 `TtlDisabled`；新配置计算出的过期水位早于作业水位为 `ExpireIntervalChanged`。键列、`definition_version` 等其他变化当前不会中止，`session_test.rs::safe_non_ttl_metadata_changes_do_not_abort_work` 明确覆盖该行为。

`validate_ttl_work` 不直接比较 schema 名、表名、分区名、定义版本或原表的 `ttl_enabled`；调用者必须传入按原目标解析出的当前 `PhysicalTable`。与 Go 版相比，Go 会从 InfoSchema 按名称重新取表、重建物理表，并显式检查分区名；Rust 简化函数只能校验参数中已有的信息，因此分区重命名若未被上游转换为缺失/不同物理 ID，单靠本函数无法发现。

checked 准备/恢复遵循“SET 即使报错也可能已生效”的保守策略：准备失败总是 `avoid_reuse`；恢复会尝试所有变量并合并错误，而不是遇到首错就跳过后续清理。读取生命周期变量时必须得到首行首列的 `Datum::Text`，否则返回 `Execute("failed to get ... variable")`。恢复缺失的原值时使用硬编码兜底值，这要求新增变量时同时定义正确的默认恢复语义。

`execute_lifecycle_sql` 只保护生命周期 SQL；`WorkerSession::execute_with_ttl_job` 和默认 `execute_in_transaction` 没有 panic 清理。`TableSession` 返回类型中虽然含 `bool`，当前成功分支总是 `false`，错误分支没有返回 retryable 标志；这比 Go 的 `([]chunk.Row, bool, error)` 表达力更窄，调用方不能从 Rust 返回值区分普通可重试错误与元数据不可重试错误，只能依赖 `SessionError` 分类。

## 并发与资源生命周期

本文件没有启动线程、异步任务、锁或通道。所有可变操作都通过 `&mut dyn WorkerSession` 串行进行，借用规则阻止同一个会话被这些 API 同时可变使用；真正的线程安全和连接池归还策略由 trait 实现者负责。

资源生命周期的核心不变量是“可能部分改写或恢复失败的池化会话不得复用”。`avoid_reuse` 是该协议的唯一钩子：准备失败、恢复失败和生命周期 SQL panic 都会调用它。成功的 checked 准备返回快照，调用者必须在所有退出路径调用对应 checked 恢复；本文件没有 RAII guard，因此遗忘恢复不会由类型系统阻止。

扫描状态分两层恢复：Rust 内存字段与 SQL 变量。`restore_scan_session_checked` 先恢复内部扫描标志，再依次尝试两条 SET；只有两条都成功时才整体应用 `restore_scan_session`。测试验证了即使第一条恢复失败，第二条仍会执行，并将会话标记不可复用。

## 与 Go 版本的对应关系

Rust 的 `prepare_session_checked`/`restore_session_checked` 对应 Go `prepareSession` 返回的 restore 闭包，保留关闭重试、开启 1PC/异步提交、ROLLBACK、UTC 时区和完整读引擎集合的次序，也对齐失败时尽力恢复、合并错误和 `AvoidReuse`。Rust `prepare_scan_session_checked`/`restore_scan_session_checked` 对应 Go `NewScanSession` 中的三项状态改写与 restore 闭包。

Rust `TableSession`、`execute_sql_with_check`、`validate_ttl_work` 分别对应 Go `ttlTableSession`、`ExecuteSQLWithCheck`、`validateTTLWork`。二者都要求 SQL 执行后再校验，因为此时才能确定事务实际使用的元数据快照；二者也都让元数据变化优先覆盖 SQL 执行错误。

差异必须保留为当前事实：Go `withSession` 已把系统会话池、统计收集器挂接、准备、defer 恢复和业务闭包串成生产路径；Rust 本文件没有 `with_session` 等池入口，checked 生命周期 API 当前只见于测试。Go `validateTTLWork` 自行访问 InfoSchema、重建分区物理表、检查分区名并按实际 TTL 表达式求新过期时间；Rust 接收已构造的 `current` 和整数秒数，校验范围更窄。Go 用 phase tracer、乐观事务和独立 retryable 布尔值；Rust 抽象未包含 tracer/事务模式，并主要靠 `SessionError` 分类。Go 的时间是带时区的 `time.Time`，Rust 当前使用 `u64` Unix 秒和显式时区偏移字段。

## 扩展指南

- 新增会话变量：同时更新 `SessionState`（若需要本地镜像）、`prepare_session_checked` 的设置顺序、`attempted` 记录、`restore_attempted_variables` 的恢复 SQL与默认值、`restore_session_checked` 的完整列表，并在 `session_test.rs` 与 `session_integration_test.rs` 增加每个设置/恢复失败点测试。
- 扩展 `Datum`：同步所有 `WorkerSession` 实现、生命周期变量读取、scan/del 参数生成和独立测试；避免改变现有参数顺序。
- 加强元数据校验：优先扩展 `PhysicalTable` 与 `validate_ttl_work`，并逐项对照 Go `validateTTLWork`；分区名、InfoSchema 查找错误和 TTL 表达式求值是当前明显的抽象缺口。安全的非 TTL 元数据变化仍应允许继续。
- 把 checked 生命周期接入生产：应在拥有池借用/归还语义的边界完成，并保证所有正常、错误和 panic 路径恢复或丢弃会话；不要只在 `scan.rs`/`del.rs` 的单条 SQL 周围零散调用。
- 调整作业归属：修改 `execute_with_ttl_job` 时必须验证嵌套调用、错误以及 panic 后的恢复策略；Go 集成测试 `TestTTLJobRUAttribution` 是资源归属语义的重要对照。
- 变更 `TableSession` 返回契约：需要同时检查 scan/del 的重试分类。当前生产 Rust 未直接调用它，接线时不能把“测试通过”等同于完整 Go 主链已迁移。
- 测试继续放在独立文件 `session_test.rs` 或 `session_integration_test.rs`，不要放回 `session.rs`。

## 验证依据

- 目标源码：`pkg/ttl/ttlworker/session.rs`，通过 RustCodeGraph `node --file` 完整读取 510 行并核对 47 个索引符号。
- 索引状态：RustCodeGraph 数据库包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ttl/ttlworker` 确认本模块 58 个 Go/Rust 文件。对 `session.rs::prepare_session_checked`、`prepare_scan_session_checked`、`execute_sql_with_check`、`validate_ttl_work` 执行精确 callers/callees 查询均未产生静态边，故以直接源码引用补证。
- 模块与 Cargo：读取 `pkg/ttl/ttlworker/lib.rs` 和 `pkg/ttl/ttlworker/Cargo.toml`，确认模块导出、独立测试模块、crate 名、常规依赖和 Windows 条件依赖。
- Rust 上游：读取 `pkg/ttl/ttlworker/scan.rs::ScanTask::execute_with_checkpoint` 与 `pkg/ttl/ttlworker/del.rs::DeleteTask::do_delete`，确认过期谓词捕获、作业 ID 归属和错误分类的实际生产引用。
- Rust 测试：读取 `pkg/ttl/ttlworker/session_test.rs` 与 `session_integration_test.rs`。覆盖 TTL 关闭、ID/时间列/区间变化、安全元数据变化、精确状态恢复、每个准备/恢复失败点、panic 丢弃和继续清理。
- Go 对照：读取 `pkg/ttl/ttlworker/session.go`，并参考 `session_test.go`、`session_integration_test.go`；核对 `withSession`、`prepareSession`、`NewScanSession`、`ttlTableSession.ExecuteSQLWithCheck`、`validateTTLWork` 以及故障注入、分区变化和作业 RU 归属测试。
- 本任务为纯文档分析，按任务约束未运行 Cargo。交付前执行固定 11 章节结构检查，并人工复核文档没有把未接线 API 写成已接入生产。
