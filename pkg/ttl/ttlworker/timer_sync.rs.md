# `pkg/ttl/ttlworker/timer_sync.rs`

## 文件定位

本文件属于 `astersql-ttl-ttlworker` crate；crate 入口 `pkg/ttl/ttlworker/lib.rs` 以 `pub mod timer_sync` 导出它，Cargo 包边界由 `pkg/ttl/ttlworker/Cargo.toml` 定义。它把 `crate::session::PhysicalTable` 映射为 TTL 定时器的键、标签、调度间隔和载荷，并用 `TtlTimersSyncer` 提供一套纯内存同步语义。

需要区分两条接线：当前 Rust 测试直接使用 `TtlTimersSyncer`；生产运行时 `pkg/session/runtime/ttl_timer.rs::sync_ttl_timers` 则只复用本文件的 `timer_key`、`timer_tags` 和 `TIMER_HOOK_CLASS`，自行通过 SQL 同步 `mysql.tidb_timers`。仓库搜索未发现生产代码实例化 `TtlTimersSyncer`，所以不能把这个内存缓存描述成生产定时器存储或完整 Go 客户端的替代品。

## 核心职责

- 用 `timer_key(table_id, physical_id)` 生成稳定的物理表定时器键，格式为 `/tidb/ttl/physical_table/{table_id}/{physical_id}`。逻辑表和分区均通过 `physical_id` 区分。
- 用 `timer_tags` 生成 `db=<schema>`、`table=<table>` 两个检索标签；`TIMER_HOOK_CLASS` 固定为 `tidb.ttl`。
- 用 `TtlTimersSyncer::sync_timers` 将一次 InfoSchema 风格的 TTL 物理表快照合并进本地 `BTreeMap`：创建新记录、更新变化记录、禁用消失记录并按宽限期清理。
- 用 `OLD_DEFAULT_TTL_JOB_INTERVAL` 为缺失或空调度表达式提供 `24h` 的升级兼容回退。
- 用 `manual_trigger` 校验缓存中记录存在且启用，然后返回定时器 ID 和调用方请求 ID；它不提交真实 timer client 请求，也不等待 TTL 作业完成。

## 主要符号

- `TIMER_KEY_PREFIX`、`TIMER_HOOK_CLASS`、`OLD_DEFAULT_TTL_JOB_INTERVAL`：分别约束跨模块使用的键空间、hook 类别和旧版本默认间隔。
- `TtlTimerData { table_id, physical_id }`：定时器载荷的最小表身份。两者均为公开字段，支持调用方检查映射结果。
- `TimerRecord`：内存记录，公开保存 `id`、`key`、`hook_class`、`tags`、`schedule_interval`、`data`、`enabled`、`deleted_at`；私有 `created_at` 保留首次创建时间，用于删除宽限期判断。
- `TtlTimersSyncer`：私有持有 `cached`、`last_sync_time`、`last_sync_version`、`delay_delete_seconds`。`new` 将删除宽限期设为 600 秒；派生的 `Default` 则会得到 0 秒，调用方若需要标准行为应使用 `new`。
- `set_delay_delete_interval`：覆盖宽限期，主要用于测试或策略调整。
- `reset`、`last_sync_info`、`cached_timer`：分别重置状态、读取最近同步元信息和只读查询缓存。
- `sync_timers`：核心合并入口。参数 `interval` 把每张物理表映射为可选调度字符串，使表元数据解析与缓存合并解耦。
- `manual_trigger`：内存模型中的手动触发前置校验。
- `timer_key`、`timer_tags`：生产运行时也使用的公共编码助手。
- `should_sync_timer`：仅比较标签、间隔、载荷和启用状态；不比较 ID、键、hook、删除时间或创建时间。

## 执行流程

`sync_timers` 的一次调用按以下顺序执行：

1. 创建 `live: BTreeSet<String>`。对输入的每个 `PhysicalTable` 生成键并加入 live 集合；调用方传入的列表应等价于 Go `ListTablesWithSpecialAttribute(TTLAttribute)` 的结果，即包含已定义 TTL 但 `TTL_ENABLE=OFF` 的表，不包含没有 TTL 定义的表。
2. 调用 `interval(table)`。返回 `None` 或空字符串时回退到 `24h`；否则原样保存表达式，本函数不解析或校验其语法。
3. 构造期望 `TimerRecord`：ID 为 `ttl-{physical_id}`，标签来自 `timer_tags`，载荷保留逻辑/物理 ID，`enabled` 取自 `PhysicalTable::ttl_enabled`，并清除 `deleted_at`。
4. 若键不存在，插入新记录；若已存在，继承原 `created_at`。只有 `should_sync_timer` 判断业务字段变化，或旧记录曾被标记删除时，才覆盖缓存。后者保证表重新出现时清除墓碑，即使它仍处于禁用状态。
5. `retain` 扫描全部缓存。live 记录保留；非 live 记录若 `now.saturating_sub(created_at) > delay_delete_seconds` 则删除，否则第一次失踪时设为 `enabled = false`、`deleted_at = Some(now)` 并继续保留。
6. 无论输入是否为空，最后把 `last_sync_time` 和 `last_sync_version` 更新为本次参数。

`manual_trigger` 先用同一键规则查缓存：缺失返回 `timer not found`，禁用返回 `manual trigger is not allowed when timer is disabled`，成功则克隆记录 ID 并原样返回请求 ID。

## 数据与状态

缓存使用 `BTreeMap`，live 键使用 `BTreeSet`，因此遍历顺序稳定，但代码不承诺外部可观察的顺序。一个键唯一绑定 `(table_id, physical_id)`；truncate 产生新物理 ID 后会得到新键和新记录，旧记录进入退役流程。

`created_at` 是记录首次进入缓存的时间，更新和重新出现都保留原值。删除宽限期从创建时间而不是 `deleted_at` 起算：一个已经存在超过宽限期的表一旦消失，会在同一次同步中立即删除；较新的表才会先留下禁用墓碑。比较使用 `saturating_sub`，所以时钟倒退不会发生无符号下溢，也不会因此误判为过期。

`deleted_at` 只记录首次观察到消失的时间，并不参与过期计算。`last_sync_time`、`created_at`、`deleted_at` 都是调用方提供的 `u64` 时间刻度；本文件不读取系统时钟，也不规定单位之外的时区语义，注释和默认宽限期按秒解释。

## 依赖与调用关系

直接源码依赖只有标准库的 `BTreeMap`/`BTreeSet` 和 `crate::session::PhysicalTable`。`pkg/ttl/ttlworker/Cargo.toml` 将库入口设为 `lib.rs`，常规依赖仅声明相邻的 `astersql-ttl-cache`；大量完整 TTL worker 依赖位于 `cfg(windows)` 条目，本文件自身没有条件编译项，也不直接调用这些外部 crate。

RustCodeGraph 确认内部调用边为：`sync_timers -> timer_key`、`sync_timers -> timer_tags`、`sync_timers -> should_sync_timer`，以及 `manual_trigger -> timer_key`。直接实例化 `TtlTimersSyncer` 的 Rust 调用方位于 `pkg/ttl/ttlworker/timer_sync_test.rs` 和 `pkg/ttl/ttlworker/integrationtest/timer_sync_test.rs`。

生产链路 `pkg/session/runtime/ttl_timer.rs::sync_ttl_timers` 调用 `timer_key` 和 `timer_tags`，把同样的身份约定编码进 `mysql.tidb_timers` 的查询、插入、更新、禁用与删除 SQL。`pkg/session/runtime/ttl_runtime.rs::trigger_ttl_command` 也使用 `timer_key` 定位持久化定时器。由此，本文件的键格式是测试模型与生产持久化路径之间的重要兼容边界。

## 错误处理与边界

`sync_timers` 没有 `Result` 返回值：闭包只返回 `Option<String>`，所有内存操作均被视为不可恢复错误之外的确定性操作。它不会验证 interval 是否能被 timer 框架解析，也不会访问存储，因此生产 SQL/client 错误、冲突重试、指标和日志不在本实现范围内。

`manual_trigger` 使用字符串错误，且只做存在性和启用状态检查；它没有 Go `ManualTriggerTTLTimer` 的 session 获取、`syncOneTimer(..., skipCache=true)`、timer client 调用、请求超时/取消检查及 job history 轮询。调用方不能把成功返回理解为作业已经触发或完成。

删除条件是严格大于 `delay_delete_seconds`，恰好等于边界时仍保留。`now` 小于 `created_at` 时，由于饱和减法结果为 0，也会保留。空输入代表当前没有 live TTL 表，会对整个缓存执行退役流程，并不是“跳过同步”。

标签目前只有 schema 和逻辑表名。与 Go 的 `getTimerTags` 相比，分区记录缺少 `partition=<name>` 标签，因为 `PhysicalTable::partition_name` 未被 `timer_tags` 使用；扩展时必须评估已有持久化记录和测试期望的兼容性。

## 并发与资源生命周期

`TtlTimersSyncer` 的变更方法要求 `&mut self`，文件内部没有锁、原子变量、线程、异步任务、通道、session 或外部句柄。并发共享必须由上层自行串行化或放入同步原语；本类型自身不提供跨线程一致性协议。

记录生命周期为“首次插入 -> 按需原位更新 -> 表消失后禁用/标记 -> 超过基于创建时间的宽限期后移除”。表在宽限期内重新出现时，覆盖逻辑会清除 `deleted_at`，但保留最初的 `created_at`。`reset` 立即丢弃全部缓存和同步元信息，不执行外部清理，因为这里没有外部资源所有权。

生产侧资源生命周期不由 `TtlTimersSyncer` 管理：`pkg/session/runtime/ttl_timer.rs::sync_ttl_timers` 通过 `TtlWorkerSqlSession` 操作持久化表，定时作业线程则由同文件的 `SqlTtlTimerHook` 管理。

## 与 Go 版本的对应关系

Go 对照实现位于 `pkg/ttl/ttlworker/timer_sync.go`。共同语义包括：相同的 key prefix 和 hook class；按逻辑/物理 ID 建键；旧版本空 job interval 回退为 `24h`；TTL disabled 的表仍拥有 disabled timer；记录消失后按 timer `CreateTime` 而非首次消失时间应用十分钟宽限期；标签、间隔或 enable 变化时更新；truncate 后新旧物理 ID 分离。

Rust 内存模型保留了上述核心决策，但不是逐项完整移植。Go `TTLTimersSyncer` 持有 system session pool、`timerapi.TimerClient`、定期全量拉取缓存时间和真实 `TimerRecord`，并记录指标、日志、存储错误；Rust 类型只持有本地值。Go `syncOneTimer` 在创建时读取 TTL table status 选择 watermark，并通过 client 创建/更新后重新读取记录；Rust `sync_timers` 不包含 watermark、timezone、policy type、版本或外部 I/O。Go 手动触发会真正请求 timer client 并返回等待闭包，Rust `manual_trigger` 仅返回两段标识。

另一个可见差异是 Go 分区标签包含 `partition=<name>`，而 Rust `timer_tags` 不包含分区名。当前生产 SQL 同步复用了 Rust 标签助手，因此这是现有 Rust 行为，不应在文档中假定已经与 Go 完全一致。

## 扩展指南

- 修改键格式、hook class 或载荷字段时，应同时检查 `timer_key`、`TtlTimerData`、`pkg/session/runtime/ttl_timer.rs::sync_ttl_timers`、`pkg/session/runtime/ttl_runtime.rs::trigger_ttl_command` 以及已有 `mysql.tidb_timers` 数据的兼容/迁移策略。键格式变化可能制造重复定时器。
- 增加同步比较字段时，更新 `TimerRecord` 构造和 `should_sync_timer`，并在独立测试 `pkg/ttl/ttlworker/integrationtest/timer_sync_test.rs` 增加“变化记录被更新、无关记录保持不变”的用例。
- 调整退役策略时，保留对创建时间、严格边界、时钟倒退、重新出现且仍 disabled 等情况的明确选择；同步更新 `pkg/ttl/ttlworker/timer_sync_test.rs` 的边界回归，以及集成测试中的 drop/truncate 场景。
- 若补齐 Go 的分区标签，应使用 `PhysicalTable::partition_name`，并同步生产 SQL 路径与测试；需评估标签改变是否触发现有 timer 更新及查询兼容性。
- 若把 `manual_trigger` 接入真实 timer client，不应只扩写当前字符串返回值；需对齐 Go 的强制同步、版本冲突/存储错误、超时、取消、事件 ID 和 job history 完成判定，并把 I/O 测试放在独立测试文件而非本源文件。
- 若要在生产侧直接采用 `TtlTimersSyncer`，必须先明确它与 `sync_ttl_timers` 的单一所有权，避免内存模型和 SQL 同步器同时管理同一 key 空间。

## 验证依据

- 源码：`pkg/ttl/ttlworker/timer_sync.rs`（全部 3 个常量、2 个数据结构、`TtlTimersSyncer` 的 7 个方法和 3 个自由函数）；模块入口 `pkg/ttl/ttlworker/lib.rs`；数据来源类型 `pkg/ttl/ttlworker/session.rs::PhysicalTable`。
- crate 边界：`pkg/ttl/ttlworker/Cargo.toml` 的 `[package]`、`[lib]`、常规依赖和 `cfg(windows)` 依赖声明。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/ttl/ttlworker` 覆盖目标、Go 对照和两组 Rust 测试；`explore`/`callers`/`callees` 核对了 `sync_timers`、`manual_trigger`、`timer_key`、`timer_tags`、`should_sync_timer` 及生产运行时引用。
- Go 对照：`pkg/ttl/ttlworker/timer_sync.go` 的 `TTLTimersSyncer`、`SyncTimers`、`syncOneTimer`、`ManualTriggerTTLTimer`、`getTimerTags`、`buildTimerKeyWithID` 和 `getTTLSchedulePolicy`。
- 测试：`pkg/ttl/ttlworker/timer_sync_test.rs` 验证老记录立即删除及 disabled 表重新出现时清除墓碑；`pkg/ttl/ttlworker/integrationtest/timer_sync_test.rs` 验证创建、默认间隔、增量更新、truncate、延迟删除、reset 和手动触发三条分支。Go 集成测试入口为 `pkg/ttl/ttlworker/integrationtest/timer_sync_test.go`。
- 生产接线：`pkg/session/runtime/ttl_timer.rs::sync_ttl_timers` 和 `pkg/session/runtime/ttl_runtime.rs::trigger_ttl_command`。仓库级 Rust 引用搜索确认 `TtlTimersSyncer` 本体目前没有非测试实例化点。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付前仅执行任务指定的 11 章节结构检查，并人工复核所有现状结论均能回指以上符号或文件。
