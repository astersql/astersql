# `pkg/executor/set.rs`

## 文件定位

`set.rs` 是 `astersql-executor` crate 中对 SQL `SET` 语句核心执行语义的 Rust 移植。模块由 [`pkg/executor/lib.rs`](./lib.rs) 以 `pub mod set` 公开，源文件本身不受 feature 条件控制；[`pkg/executor/Cargo.toml`](./Cargo.toml) 将该目录声明为 `astersql-executor`，并提供这里直接使用的 `astersql-sessionctx-variable` 依赖。

该文件位于“已解析的变量赋值”与“真实会话、权限、全局配置、审计和 InfoSchema 服务”之间：`SetExecutor<B>` 负责控制流程，`SetBackend` 把具体运行时能力抽象成关联类型和必选方法。当前仓库内可检索到的 Rust `SetBackend` 具体实现位于独立测试 [`pkg/executor/set_test.rs`](./set_test.rs)，未检索到生产后端实现或 Rust builder 对 `SetExecutor` 的实例化；因此这里是公开且可测试的执行核心，但生产主链接线尚未由现有直接证据确认。Go 生产实现位于 [`pkg/executor/set.go`](./set.go) 的同名类型中。

## 核心职责

- `SetExecutor::Next` 将一组 `VarAssignment` 依次分流到 `SET NAMES`/`SET CHARACTER SET`、用户变量和系统变量三类路径，并用 `done` 保证执行器对象只执行一次。
- 用户变量路径统一将名称转为小写；非 `NULL` 值同时保存值与表达式字段类型，`NULL` 删除值及其类型元数据。
- `setSysVariable` 处理系统变量存在性、动态权限、SEM v2 只读限制、noop/旧版 INSTANCE 兼容警告，以及 GLOBAL、INSTANCE、SESSION 三种写入路径。
- GLOBAL/INSTANCE 写入后触发审计和日志；云存储 URI 与 embedding API key 在这些观测面中脱敏，真实配置值不被替换。`tidb_service_scope` 还会用后端已规范化的当前值重新初始化任务管理器会话。
- SESSION 写入保护事务特性，并对 `tidb_snapshot`、`txn_read_ts` 做 read-ts/GC 校验、失败回滚和快照 InfoSchema 安装或清除。
- `setCharset` 实现字符集与排序规则联动，`getVarValue` 实现 `DEFAULT`、`NULL` 和普通表达式的字符串化规则。

## 主要符号

- `SetDatum::{Null, String}`：本文件所需的最小求值结果。私有方法 `string` 将 `Null` 视为空串，主要服务于字符集伪变量取值。
- `VarAssignment<E>`：一项赋值描述，保存名称、表达式、可选扩展值、`DEFAULT` 标志以及 system/global/instance 作用域标志。`setSysVariable` 在旧版 INSTANCE 兼容分支中可能把 `is_instance` 原地改为 `true`。
- `SystemVariable` 与 `SystemVariableLookup`：系统变量的规范名称、默认值、noop/INSTANCE 元数据，以及 `Found`、`Removed`、`Unknown` 三态查找结果。已移除变量被静默忽略，未知变量报错。
- `Collation`：排序规则名及所属字符集，用于拒绝 collation/charset 不匹配。
- `SetVariableNames`：由后端集中提供特殊变量名和字符集常量，避免执行器硬编码会话变量集合。
- `SetResultChunk`：只有 `reset` 的结果接口；`SET` 不产出数据行，但每次 `Next` 调用仍须清空输出。
- `SetBackend`：生产边界，共有 `Context`、`Error`、`Expression`、`FieldType`、`SnapshotInfoSchema` 五个关联类型，并覆盖求值、变量存取、权限、告警、审计、日志、任务管理和快照服务。所有对外可见操作都是必选方法，没有静默默认实现。
- `SetExecutor<B>`：持有后端 `BaseExecutor`、赋值列表 `vars` 和一次性状态 `done`。公开入口为 `Next`；其余方法用于系统变量、字符集、取值和快照回滚。
- 自由函数 `loadSnapshotInfoSchemaIfNeeded`：可由执行器之外复用；时间戳为零时清空会话快照，否则加载指定版本、附着本地临时表后安装。

## 执行流程

1. `Next` 先调用 `request.reset()`；若 `done` 已为真立即成功返回，否则先将 `done` 设为真，再按原顺序遍历 `vars`。这意味着首次执行中途失败后，同一执行器不能通过再次调用 `Next` 重试。
2. 名称等于 `SetVariableNames::set_names` 或 `set_charset` 时进入 `setCharset`。`DEFAULT` 使用默认字符集；否则先求表达式，再从 `extend_value` 取得可选 collation。
3. 非系统变量先把名称折叠为小写。表达式结果为 `Null` 时调用 `unset_user_variable`；否则依次写值和字段类型。
4. 系统变量进入 `setSysVariable`：先查定义；`Removed` 直接成功，`Unknown` 由后端构造错误。随后验证变量要求的所有动态权限；SEM 未启用时，错误消息按 Go 兼容语义显示为 `SUPER or <privilege>`。SEM v2 只读变量额外要求 `RESTRICTED_VARIABLES_ADMIN`。
5. noop 变量在 noop 功能未启用时只告警、不拒绝。具有 INSTANCE 作用域的变量在 legacy 模式且调用方未显式 GLOBAL 时，会被改写为 INSTANCE 并产生迁移警告。
6. GLOBAL/INSTANCE 分支通过 `getVarValue(..., Some(system_variable))` 求值，然后写入对应持久化范围。写成功后生成展示值：云存储 URI 调用 `redact_url`，非空 embedding API key 固定显示 `******`；该展示值用于审计与日志。若变量是 `service_scope`，最后读取后端规范化值并初始化任务管理器。
7. SESSION 分支先通过 `getVarValue(..., None)` 求值，再在事务内禁止修改一次性隔离级别、`txn_read_ts`，以及 stale transaction 的 `tidb_snapshot`。写入前保存旧快照时间戳；写入后若新时间戳非零且发生变化，则验证 read-ts，`tidb_snapshot` 还额外验证 GC safe point。
8. 时间戳验证或快照 InfoSchema 加载失败时，`restoreSnapshotTimestamp` 恢复旧时间戳并返回错误。成功时记录会话变量日志。时间戳为零会通过自由函数清除快照 InfoSchema；非零则加载快照、附着本地临时表并安装。

## 数据与状态

执行器自身唯一的生命周期状态是 `done`；赋值列表按顺序执行，后项能观察到前项对同一后端产生的状态变化。错误不会事务化回滚整条 `SET` 的先前赋值，文件中仅对快照变量的时间戳提供局部补偿；即使补偿发生，后端 `set_session_system_variable` 可能产生的其他副作用也不在本文件内统一撤销。

系统变量的真实状态完全由 `SetBackend` 持有。`SetVariableNames` 是一次调用取得的值对象；`SystemVariable` 是查找返回的元数据快照。`getVarValue` 对 GLOBAL/INSTANCE 的 `DEFAULT` 取“全局初始值或编译期默认值”，对 SESSION 的 `DEFAULT` 读取当前 GLOBAL 值；普通 `Null` 转为空字符串，普通非空值经 `datum_to_string` 变为拥有所有权的 `String`，对应 Go 中避免底层缓冲复用的 `strings.Clone` 意图。

快照状态由时间戳和 `SnapshotInfoSchema` 两部分组成。执行器先通过变量 setter 更新后端时间戳，再校验并装载 schema；失败路径把时间戳恢复为保存值。成功装载顺序固定为 `snapshot_info_schema` → `attach_local_temporary_tables` → `set_snapshot_info_schema(Some(...))`。

## 依赖与调用关系

上游方面，`pkg/executor/lib.rs` 公开本模块；RustCodeGraph 对 `SetExecutor` 的实例化边只定位到 [`pkg/executor/set_test.rs`](./set_test.rs) 的 `execute` 和 `test_validate_set_var`，尚无生产调用者证据。测试辅助函数 `execute` 连续调用两次 `Next`，验证一次性语义以及两次结果 reset。

内部主边为 `SetExecutor::Next → setCharset`、`SetExecutor::Next → setSysVariable`。`setSysVariable` 再调用 `getVarValue`、`snapshotTimestampByName`、`restoreSnapshotTimestamp` 和方法版 `loadSnapshotInfoSchemaIfNeeded`；方法版仅筛选变量名，随后调用同文件自由函数。RustCodeGraph 还确认自由函数向下调用 `snapshot_info_schema`、`attach_local_temporary_tables`、`set_snapshot_info_schema` 和 `log_snapshot_info_schema` 等后端边界。

文件唯一直接引用的仓库外部模块路径是 `astersql_sessionctx_variable::is_embedding_api_key`，用于覆盖一组 embedding API key 的统一识别；Cargo 将其映射到本地 crate `pkg/sessionctx/variable`。其余依赖均通过标准库 `Display` 或 `SetBackend` 反转，因而执行算法不直接依赖具体 SessionVars、权限管理器、Domain 或插件类型。

Go 对照的上游主链更完整：Go builder 产生 `SetExecutor`，通用 Executor 驱动其 `Next`；Go `set.go` 直接调用 SessionVars、权限、plugin audit、Domain 和临时表包装。Rust 文件把这些调用折叠为后端 trait，但当前代码搜索未证明生产适配器已经完成接线。

## 错误处理与边界

所有后端可失败操作以 `Result<_, B::Error>` 逐层传播，文件不包装错误上下文。主要拒绝条件包括：表达式求值/字符串化失败、未知系统变量、动态权限不足、SEM v2 只读限制、GLOBAL/INSTANCE 持久化或审计失败、事务中修改受保护特性、快照时间戳或 GC 校验失败、InfoSchema 加载失败、未知字符集/排序规则以及 collation 与 charset 不匹配。

已移除系统变量是明确的兼容边界：解析后执行时静默忽略。noop 系统变量也是兼容接受，仅在功能未启用时告警。`SET NAMES` 多变量更新和普通多赋值均是顺序写入；后续写失败时本文件没有回滚先前成功项。GLOBAL/INSTANCE 的真实值先落库，之后审计或 service-scope 初始化若失败，也没有由本文件执行补偿。

敏感值边界只作用于审计与日志：cloud URI 由后端脱敏，embedding API key 非空时替换为六个星号，空值保持空；写入 GLOBAL/INSTANCE 的仍是原值。SESSION 日志直接接收最终字符串，是否进一步脱敏属于后端责任。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道；一次 `Next` 内全部步骤同步、串行执行。`&mut self` 保证同一执行器在安全 Rust 中不能被并发调用，但后端关联资源是否跨线程共享、如何同步，由 `SetBackend` 实现决定。测试后端用普通集合保存多数状态，只因 `log_global_variable` 的接收者是 `&self` 而以 `RefCell` 记录日志；这只是测试替身的内部可变性，不构成生产线程安全保证。

`done` 在副作用开始前置位，生命周期是“至多尝试一次”而不是“至多成功一次”。结果 chunk 借用仅覆盖调用期并在入口清空。表达式结果与系统变量展示值均转成拥有所有权的字符串，避免借用后端短生命周期缓冲。快照 InfoSchema 的创建、临时表附着和会话安装由后端管理；清除路径显式写入 `None`，本文件不持有 schema 资源。

## 与 Go 版本的对应关系

Rust `SetExecutor::Next`、`setSysVariable`、`setCharset`、`getVarValue`、方法/自由函数版 `loadSnapshotInfoSchemaIfNeeded` 分别对应 [`pkg/executor/set.go`](./set.go) 中的同名实现。大小写折叠、用户变量 NULL 删除、权限与 SEM 检查、noop/INSTANCE 告警、三类作用域、事务限制、快照校验回滚、字符集变量集合、`DEFAULT` 语义及临时表附着顺序均保持了 Go 主流程。

Rust 用泛型 `SetBackend` 替代 Go 的 `exec.BaseExecutor` 与全局服务调用，用 `SetDatum` 替代完整 Datum，并以关联类型隔离表达式、字段类型和 InfoSchema。Rust `String` 天然拥有缓冲，因此 `getVarValue` 的 `clone` 表达的是 Go `strings.Clone` 的所有权意图，而非完全相同的内存实现。

存在可核实的迁移差异：Go GLOBAL `tidb_gc_life_time` 写入后会调用 `notifyExternalWorkloadGCLifeTime`，Rust trait 和执行流程没有对应钩子；Go 有独立的 `redactSysVarValue`/`redactAPIKey`，Rust 对 cloud URI 委托后端、对 embedding key 调用 `astersql_sessionctx_variable::is_embedding_api_key`；Rust 的审计边界显式返回 `Result`。这些差异说明不能仅凭此文件认定 Rust 已完整替代 Go 生产路径。

Rust 独立测试名称与 Go 测试意图基本对应：用户变量、字符集/排序规则、系统变量校验、legacy INSTANCE、noop、Top SQL、精度变量、service scope 和 embedding key 脱敏均有对应项；Rust 同一测试文件还包含 `set_config` 测试，但那部分属于 [`pkg/executor/set_config.rs`](./set_config.rs)，不是本文目标逻辑。

## 扩展指南

- 新增一种系统变量通用规则时，优先扩展 `SystemVariable` 元数据或 `SetBackend` 的窄接口，并在 `setSysVariable` 的写入副作用之前完成校验；若只属于某个具体后端，应避免把运行时类型耦合进本文件。
- 新增敏感变量必须同步检查写入、审计和日志三个面。embedding key 应优先维护 `astersql_sessionctx_variable::is_embedding_api_key` 的统一集合；其他类别需要决定是在后端 `redact_url` 一类接口中处理，还是增加明确的执行器策略，并与 Go `redactSysVarValue` 对齐。
- 新增快照类变量需同步更新 `SetVariableNames`、事务限制、`snapshotTimestampByName`、`restoreSnapshotTimestamp` 和 `loadSnapshotInfoSchemaIfNeeded` 的筛选条件，确保 setter 后失败仍能恢复对应时间戳。
- 修改字符集语义时，同时核对 `set_names_variables`、`set_charset_variables`、database charset/collation 的读取规则，以及显式 collation 的所属字符集校验；多变量写入的部分成功风险应保持与 Go 一致或明确设计补偿。
- 若补齐生产接线，需要在 executor builder/adapter 中提供真实 `SetBackend` 实现并证明 `Next` 被通用执行器驱动；不能用测试 `Backend` 代替生产会话、权限、审计、任务管理和 InfoSchema 服务。
- 测试应继续放在独立的 [`pkg/executor/set_test.rs`](./set_test.rs)，不要内嵌到生产源文件。至少覆盖新增成功路径、权限/事务错误、部分副作用、敏感值展示以及失败回滚；Go 行为变化时同步核对 [`pkg/executor/set_test.go`](./set_test.go) 和 [`pkg/executor/set_internal_test.go`](./set_internal_test.go)。

兼容性风险集中在 MySQL/TiDB 的 `DEFAULT`、字符集联动和作用域语义；正确性风险集中在权限检查顺序、写入后的不可回滚副作用与快照状态一致性；性能风险较低，主要成本来自逐赋值求值、持久化访问、审计，以及非零快照触发的 InfoSchema 加载。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；查询并读取了 `SetExecutor`、`setSysVariable`、`setCharset`、`getVarValue`、两个 `loadSnapshotInfoSchemaIfNeeded` 以及 `pkg/executor/set.rs` 全部 619 行。
- RustCodeGraph 调用证据：`SetExecutor` 的 Rust 实例化者仅见 `pkg/executor/set_test.rs::execute` 与 `test_validate_set_var`；`setSysVariable` 的下游包括权限、三类变量写入、审计/日志、快照验证和四个本地辅助函数；自由函数快照加载的下游包括 schema 获取、临时表附着和会话安装。
- crate 与模块证据：[`pkg/executor/Cargo.toml`](./Cargo.toml) 的 package 名为 `astersql-executor`、`lib.path = "lib.rs"`、本文件无 feature gate，并声明 `astersql-sessionctx-variable`；[`pkg/executor/lib.rs`](./lib.rs) 公开 `set`，并以独立 `set_test.rs` 作为测试模块。目标包不存在 `pkg/executor/doc.go`。
- 对照证据：完整核对 [`pkg/executor/set.go`](./set.go) 的 `SetExecutor` 主流程，并核对 [`pkg/executor/set_test.rs`](./set_test.rs) 的测试后端和相关测试；Go 测试入口来自 [`pkg/executor/set_test.go`](./set_test.go) 与 [`pkg/executor/set_internal_test.go`](./set_internal_test.go)。
- 人工复核结论：本文区分了已公开/已测试的 Rust 执行核心与未找到的生产后端接线，列明了顺序副作用、快照局部回滚、一次性状态、敏感值边界及 Go 独有的 GC lifetime 通知，不以预期架构替代当前代码事实。
