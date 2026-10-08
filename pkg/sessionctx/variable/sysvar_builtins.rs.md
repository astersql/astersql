# `pkg/sessionctx/variable/sysvar_builtins.rs`

## 文件定位

该文件是 `astersql-sessionctx-variable` crate 的内置系统变量注册表实现。模块由 [`lib.rs`](./lib.rs) 公开为 `sysvar_builtins` 并整体再导出；crate 边界及依赖见 [`Cargo.toml`](./Cargo.toml)。它不负责解析 `SET` 语句，而是把变量名、默认值、作用域、类型、上下界以及验证/读写钩子组装成 `SysVar`，再交给 `RegisterSysVar` 写入 crate 级注册表。

应用启动和会话构造通过 `register_builtin_sysvars()` 保证这些定义可见。直接入口包括 `variable.rs` 的注册表初始化、`session.rs` 的会话变量初始化、`pkg/session/hint_runtime.rs` 的 hint 桥接，以及 `pkg/session/runtime/session.rs` 的运行时启动接线。文件末尾的 `set_global_system_var()` 是测试和内部接线使用的“验证、执行全局钩子、持久化”组合入口；正常 SQL 控制流也会调用已注册 `SysVar` 的同类钩子，而非在本文件解析 SQL。

## 核心职责

1. 用 `string_var`、`bool_var`、`int_var`、`unsigned_var`、`float_var`、`enum_var` 构造一致的 `SysVar` 骨架，统一填充名称、默认值、作用域、类型和取值边界。
2. 按领域分组注册内置变量：基础执行/DDL 变量、优化器变量、SQL 与会话变量、动态 getter/default、全局进程状态、共享锁门控、内存仲裁，以及尚未被完整 Rust 定义覆盖的兼容变量。
3. 为不能只靠通用类型校验表达的语义安装 `Validation`、`SetSession`、`GetSession`、`SetGlobal`、`GetGlobal` 钩子，例如隔离级别、时区、只读状态联动、DDL 写速率、资源控制和内存阈值。
4. 把部分全局变量映射到原子状态、`vardef` 动态状态、全局配置或 trace recorder；把部分会话变量映射到 `SessionVars`/`StmtCtx`/`KVVars` 的强类型字段。
5. 保留 Go 兼容表面：退役变量仍可读取并产生警告，noop 变量从 `crate::noop::NOOP_SYS_VARS` 补注册，已存在的真实 Rust 定义优先。

## 主要符号

- `register_builtin_sysvars()`：公开且幂等的总入口。`REGISTER: Once` 使其在进程内只执行一次，并按固定顺序调用九个注册分组；`embedding_vars` 在本文件外注册，但由此入口纳入同一启动阶段。
- `set_global_system_var(vars, name, value) -> Result<String, VariableError>`：查找定义，按 `ScopeGlobal` 调用 `Validate`，执行 `SetGlobalFromHook`，最后通过 `GlobalVarsAccessor::set_global_sys_var_only` 持久化规范化值并返回它。任一步失败都会停止后续步骤。
- `SetTraceEventConfig(value) -> Result<(), String>`：只允许 TiDB X/next-gen kernel；空串关闭 recorder 和 trace mode，非空串解析 `FlightRecorderConfig`、启动 recorder、设为 full mode，并在 `TRACE_EVENT_CONFIG` 中保存原始 JSON。
- 六个 `*_var` 构造器：只创建元数据骨架，不注册、不执行钩子。`scope_both()` 返回 global/session 的位或组合。
- `noop_sys_var()` / `register_noop_compatibility_vars()`：把静态 noop 元数据转换成完整 `SysVar`；显式处理 Go 风格最大值常量，标记 `IsNoop`，并仅在 `GetSysVar` 未命中时注册。
- `register_compatibility_vars()`：补充 MLog、端口、编译平台、GC、异步提交、统计并发等基础兼容定义；其中若干变量有额外验证或直接写入 `vardef` 状态。
- `register_basic_clamped_vars()`：注册 TiFlash/MPP、执行时限、事务文件、DML、DDL 并发和数值钳制变量；`ddl_write_speed()` 使用 `ParseGoSize` 复现 Go 的二进制单位语法，并限制到 `0..=1 PiB`。
- `register_planner_tuning_vars()`：注册执行器并发、join、选择性、range、隔离读引擎等优化器参数；可用于 `SET_VAR` 的定义会设置 `IsHintUpdatableVerified`，需要强类型状态的定义附带 `SetSession`。
- `register_sql_and_session_vars()`：注册 MySQL/TiDB 建连所需变量及 `sql_mode`、时区、事务隔离、只读/noop 约束、replica read 和 TiFlash 策略。
- `register_getters_and_defaults()`：提供事务时间戳、上次事务/查询信息、warning count、last insert id、动态 timestamp 等 getter，并注册若干运行时或实例级变量。
- `register_global_vars()`：绑定进程级状态，包括统计、OOM、TS 校验、plan replayer 时长、plan cache、只读联动、DDL、内存限制、GOGC、资源控制、schema cache 等。
- `register_foreign_key_shared_lock()`：按 kernel/config 门控共享锁检查和升级，并为 persisted global 值保留读取/初始化通路。
- `register_mem_arbitrator_vars()`：注册全局仲裁模式/软限制和会话等待策略/查询保留量。
- `GetSysVarDefinition()`：从注册表克隆既有定义，以便在不共享可变对象的前提下补充 scope、默认值或 hook。

## 执行流程

注册阶段如下：

1. 首次调用 `register_builtin_sysvars()` 时进入 `REGISTER.call_once`；后续调用立即返回。
2. 先注册基础和优化器/SQL 定义，再注册共享锁门控、动态 getter 和全局定义；这一顺序允许 `GetSysVarDefinition()` 取得早先定义并克隆增强，例如 `tidb_hash_join_version`。
3. 接着注册外部 `embedding_vars` 和内存仲裁变量。
4. 最后补兼容变量和 noop 变量；noop 分支先查 `GetSysVar`，因此不会覆盖前面更完整的实现。
5. `RegisterSysVar` 将定义放入 crate 级表；会话初始化随后依据 scope/default/`skipInit` 读取定义，并由通用 `SysVar` API 调用本文件安装的 hook。

一次全局写入经 `set_global_system_var()` 时，顺序严格为“存在性检查 -> 通用类型/范围与专用 `Validation` -> `SetGlobalFromHook` 副作用 -> accessor 持久化”。这意味着验证失败不会执行全局副作用；但如果全局 hook 成功而 accessor 随后失败，本函数没有补偿事务，调用方会收到错误而进程内状态可能已改变。

会话写入通常由 `SysVar::SetSessionFromHook` 完成：通用层维护字符串形式的 session system map，本文件的 `SetSession` 再同步 `SessionVars`、`StmtCtx` 或 `KVVars` 的强类型字段。动态读取则优先通过 `GetSession`/`GetGlobal` 返回运行时状态，而不是静态 `Value`。

## 数据与状态

- `REGISTER: Once`：注册生命周期哨兵，不保存变量内容。
- 进程级原子量：`RESOURCE_CONTROL_ENABLED`、`RESOURCE_CONTROL_STRICT`、`SERVER_MEMORY_LIMIT`、`SERVER_MEMORY_LIMIT_SESS_MIN_SIZE`、`SERVER_MEMORY_LIMIT_GC_TRIGGER_BITS`、`SCHEMA_CACHE_SIZE`、`TS_VALIDATION_ENABLED`，以及私有的 `GOGC_THRESHOLD_BITS`、`GOGC_MIN`、`GOGC_MAX`。浮点数以 `f64::to_bits()` 存在 `AtomicU64` 中。
- `TRACE_EVENT_CONFIG: LazyLock<Mutex<Option<String>>>`：保存 trace event 原始配置字符串；真正运行状态还位于 `traceevent` flight recorder 和 trace mode 中。
- `SysVar` 元数据：`Scope`、`Type`、`MinValue`/`MaxValue`、`PossibleValues`、`ReadOnly`、`Hidden`、`Depended`、`AllowAutoValue`、`AllowEmpty`、`Aliases`、`IsHintUpdatableVerified`、`InternalSessionVariable` 和 `skipInit` 共同决定通用层行为。
- 会话状态：钩子会更新 `SessionVars` 内的执行时限、并发、隔离读取、外键共享锁、window/hash join、CDC source 等字段；警告写入 `StmtCtx`。
- 外部动态状态：部分 getter/setter 访问 `vardef` 中的原子/动态容器、`config::get_global_config()`、`GlobalVarsAccessor`、flight recorder 或系统时间。

重要不变量包括：GOGC minimum 必须小于 maximum；server memory GC trigger 必须位于 `(0, 1)` 且高于 tuner threshold 加安全间距；restricted read-only 开启时不能关闭 super read-only；next-gen kernel 下受配置门控的 foreign-key shared-lock 不能由用户开启；兼容 noop 定义不得覆盖真实定义。

## 依赖与调用关系

上游：

- `variable.rs` 在读取全局注册表前调用 `register_builtin_sysvars()`，形成按需初始化路径。
- `session.rs` 的会话构造/初始化调用注册入口，然后根据定义初始化会话变量。
- `pkg/session/hint_runtime.rs` 和 `pkg/session/runtime/session.rs` 显式调用注册入口，保证 hint 与 SQL runtime 使用同一注册表。
- `pkg/session/runtime/control.rs` 在处理 `tidb_trace_event` 时直接调用 `SetTraceEventConfig()`；其他 `SET GLOBAL` 路径通过 domain/accessor 和 `SysVar` hook 形成等价副作用链。

下游：

- crate 内：`RegisterSysVar`、`GetSysVar`、`SysVar::{Validate,SetGlobalFromHook}`、`SessionVars`、`GlobalVarsAccessor`、`vardef` 常量与动态状态。
- 配置/内核：`config` 与 `kerneltype` 决定实例配置及 classic/next-gen 条件；`naming::Check` 验证 service scope。
- 解析/格式：`astersql-config-configtypes::ParseGoSize`、本 crate 的 Go duration/内存解析器、`chrono::FixedOffset`、`serde_json`。
- 执行相关：`kv` 默认退避参数、`fixcontrol`、`tiflashcompute`、`parser_ast::misc::redact_url`。
- 追踪：`traceevent::flightrecorder` 与 trace mode。

RustCodeGraph 将该文件标记为被 `pkg/session/runtime/planning.rs`、`pkg/sessionctx/stmtctx/stmtctx.rs`、`pkg/sessionctx/variable/session.rs` 引用；精确入口又由仓库文本搜索确认到上述初始化、hint 和 runtime 路径。图工具对本文件函数 ID 的 `callers/callees` 查询发生名称解析泛化，因此文档没有采用其错误的跨仓库“function”候选，而以文件引用边和直接调用点为准。

## 错误处理与边界

- 未知变量由 `VariableError::unknown` 返回；类型、枚举和范围错误使用 `wrong_type`、`wrong_value` 或 `InvalidValue`。
- 通用 `SysVar::Validate` 先处理类型、scope 和数值边界，专用 `Validation` 再表达跨变量、kernel、配置或格式约束；部分越界值按 Go 语义被钳制并通过 `StmtCtx` 记录 warning，而不是报错。
- `SetTraceEventConfig` 将 JSON、recorder 和 mode 错误转成字符串；classic kernel 明确拒绝。空串是关闭操作。
- `ddl_write_speed` 接受 Go `units.RAMInBytes` 风格的分数、二进制单位、十六进制浮点和下划线，但拒绝内部拆散的单位、重复后缀、负值、空串和超过 1 PiB 的值。
- `time_zone` 当前只显式接受 `UTC`、`SYSTEM`、`America/Edmonton` 或合法的 `±HH:MM`；命名时区不是通用数据库查询，扩展时不能误称已完整支持 IANA 时区。
- `tidb_disable_txn_auto_retry` 始终规范化为 `ON`；输入 `OFF` 只产生废弃警告。`tidb_merge_partition_stats_concurrency` 始终读回 `1`，非 `1` 输入产生兼容警告。
- `GetGlobal` 可委托 `GlobalVarsAccessor`，因此存储层错误原样传播。`Mutex::lock().unwrap()` 在 trace 配置锁中意味着持锁线程 panic 导致 poison 后，后续访问也会 panic。

## 并发与资源生命周期

注册表初始化由 `Once` 串行化，避免并发会话重复填表。进程级布尔/整数和浮点 bit pattern 均使用原子容器；本文件对自身原子读写采用 `Ordering::SeqCst`，而 `vardef` 封装的 `Load`/`Store` 由其实现负责同步。`TRACE_EVENT_CONFIG` 用 `Mutex` 保护 `Option<String>`，但 recorder 的启动/关闭属于外部资源生命周期。

非空 trace 配置的顺序是解析 -> 启动 recorder -> 开 full mode -> 更新受锁配置；空配置的顺序是关闭 recorder -> 关 mode -> 清空配置。中间步骤失败没有显式回滚，所以扩展该流程时应考虑 recorder、mode 和缓存字符串的部分成功状态。

`Arc` 包装的 hook 被注册表和会话路径共享，闭包捕获值必须可长期存活。动态配置写入可能同时影响全局原子、克隆后的 `config::Config` 或持久化 accessor；这些操作不组成跨资源事务。测试中使用 `serial_test` 和 `Drop` 恢复器保护会修改进程全局状态的用例，新增同类测试也应采用隔离/恢复策略。

## 与 Go 版本的对应关系

Go 的主要对照是 [`variable.go`](./variable.go) 与 [`sysvar.go`](./sysvar.go)：前者定义 `SysVar` 注册表、锁和 `RegisterSysVar`/`GetSysVar`，后者集中定义默认系统变量及验证、session/global hook。本 Rust 文件把 Go 的单一大表按职责拆成多个 `register_*` 函数，但仍保持 `Scope`、类型、默认值、上下界、alias、hint 可更新标志和 hook 语义。

对应关系不是逐行复刻：

- Rust 用 `Once` 完成延迟幂等注册，并通过构造器减少表项样板；Go 使用包级表和 `RWMutex`。
- Go 的 noop 表在 Rust 中由 `noop.rs` 静态元数据转换，且只补缺失定义。
- Go 进程级变量常由原子封装或 package global 保存；Rust 对应为本文件原子量或 `vardef` 动态状态。
- Rust 明确保留若干兼容退化行为，如 merge concurrency 固定为 `1`、index join build v2 固定开启、disable txn auto retry 固定为 `ON`。
- 外键共享锁的 persisted value 可在初始化时读取，但用户 `SET` 仍受 next-gen 配置门控；这与 Go `sysvar.go` 中的 validation/getter 设计一致。

直接 Go 测试证据位于 [`sysvar_test.go`](./sysvar_test.go)：覆盖 server memory limit、GC trigger 以及 foreign-key shared-lock gate。Rust 的独立测试没有嵌入生产文件，而位于 [`sysvar_builtins_test.rs`](./sysvar_builtins_test.rs)；它核对 hash join auto 值、GOGC 严格次序、CDC source、DDL scope/write speed、全局 hook、MLog、paging、fix control 和废弃变量行为。

## 扩展指南

新增或修改系统变量时：

1. 先在 `vardef` 复用或补齐规范名称/默认值，再选择最贴近领域的 `register_*` 分组；不要把所有变量堆入 `register_builtin_sysvars()`。
2. 用构造器声明通用类型和边界；只有跨变量、动态配置或特殊格式约束才加 `Validation`。需要 planner/executor 强类型字段时同步添加 `SetSession`，否则仅字符串 map 更新可能让消费者看不到新值。
3. 全局变量若有实时副作用，应同时提供匹配的 getter/setter，并明确持久化失败后的状态策略；敏感字符串 getter 应像 cloud URI 一样脱敏。
4. 需要 hint 更新时设置 `IsHintUpdatableVerified`，并同步 hint 白名单/消费方证据；instance/global/session scope 不可仅凭变量名称推断。
5. 兼容项应优先完整实现；只有无运行效果的变量才进入 noop 元数据。不要让 noop 后注册覆盖真实定义。
6. 同步修改独立测试 `sysvar_builtins_test.rs`；若行为来自 Go，还应核对 `sysvar.go` 与 `sysvar_test.go` 的默认值、warning、钳制和 hook 顺序。测试全局状态时必须恢复原值，必要时串行执行。
7. 性能风险主要来自高频 session getter/setter 中的分配、锁或配置读取；正确性风险主要来自字符串 map 与强类型字段不一致、全局 hook 与持久化非原子、别名定义漂移及跨变量不变量失配。

## 验证依据

- RustCodeGraph `status`：本仓库索引包含 11,467 个文件；`files --filter pkg/sessionctx/variable` 显示目标文件 79 个符号、独立测试 26 个符号。
- RustCodeGraph `node --file pkg/sessionctx/variable/sysvar_builtins.rs` 分段读取了 1--2488 行；`node --file .../sysvar_builtins_test.rs` 读取了完整测试。图报告该生产文件被三个 Rust 文件引用。
- RustCodeGraph `query` 确认 `SetTraceEventConfig`、`register_builtin_sysvars` 和 `set_global_system_var` 的定义位置；`callers/callees` 对函数 ID 出现泛化候选，故未将其不可靠输出当作调用事实，转而用直接调用点搜索补证。
- 已核对：`pkg/sessionctx/variable/Cargo.toml`、`lib.rs`、`variable.rs`、`session.rs`、`sysvar_builtins_test.rs`、Go `variable.go`、`sysvar.go`、`sysvar_test.go`，以及直接调用点 `pkg/session/hint_runtime.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/control.rs`。
- 人工复核重点：总注册顺序、compatibility 覆盖规则、global write 的四步顺序、原子/锁状态、跨变量不变量、Go/Rust 测试边界和测试文件独立性。
- 本任务是纯文档分析，按任务约束未运行 Cargo；结构验证使用任务指定的 11 标题命令。
