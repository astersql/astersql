# `pkg/session/starter_bootstrap_file.rs`

## 文件定位

本文件属于 `astersql-session` crate，由 [`pkg/session/lib.rs`](lib.rs) 的 `pub mod starter_bootstrap_file` 公开。它实现 Starter 部署模式专用的、由 JSON 文件驱动的版本化 SQL 初始化与升级流程，并与 TiDB 核心 bootstrap 生命周期分离（文件级说明与 `reconcile_starter_bootstrap_with_metadata`）。生产入口位于 [`pkg/session/runtime/session.rs`](runtime/session.rs) 的 `CanonicalSessionFactory::reconcile_configured_starter_bootstrap`：Domain 完成核心初始化并启动正常 DDL 后，它加载清单并调用本文件的协调器。

该文件还处理分支或恢复场景中的权限表重置：从 keyspace 元数据读取 `serverless_is_branch_bootstrapped`、`serverless_is_bootstrapped_for_restore` 标记，在持锁状态下清空指定权限表、重新执行 bootstrap SQL，并通过 PD HTTP CAS 更新标记。它不是通用 SQL migration 框架，也不负责核心系统表 bootstrap。

## 核心职责

1. `parse_starter_bootstrap_file` 将 JSON 解码为 `StarterBootstrapFile`，刻意兼容 Go `encoding/json` 的字段名大小写、重复字段取最后非 `null` 值以及 `null` slice/元素零值语义，同时拒绝未知字段、额外顶层 JSON 值、非法版本、空 SQL 块和未知占位符。
2. `load_starter_bootstrap_file` 只在 Starter 模式且配置了路径时读取文件；读取和解析错误都会附带路径上下文，避免启动时静默忽略配置错误。
3. `execute_starter_bootstrap_sql_blocks` 执行增量升级 SQL：替换 `<keyspace>`，要求每块恰好一条语句，在 restricted SQL 状态中执行并关闭所有结果集。
4. `run_starter_bootstrap_locked` 和 `reset_privileges_locked` 负责首次初始化及权限重置。bootstrap 内容先全部解析并验证为 DML，再开启事务，执行语句、确认 `'<keyspace>.root'@'%'` 存在、写 SQL 版本，最后提交；失败时尝试回滚。
5. `reconcile_starter_bootstrap_with_metadata` 用 KV 完成版本做无会话快路径，在调用方提供的 owner 锁内二次检查，然后选择首次初始化、增量升级、崩溃后完成键修复或权限重置分支。
6. `parse_privilege_reset`、`privilege_reset_completion_params` 和 `update_privilege_reset_config` 将 keyspace 元数据标记转换成带 observed-value precondition 的 PD PATCH 请求，防止覆盖并发更新。

## 主要符号

- `VERSION_VAR` / `VERSION_COMMENT`：`mysql.tidb` 中 SQL 侧版本记录的键与说明。`get_starter_bootstrap_version`、`update_starter_bootstrap_version` 共同维护它。
- `StarterBootstrapFile { version, bootstrap, upgrades }`：清单根对象；`StarterBootstrapUpgrade { version, sql }`：单个目标版本的升级块。两者手写 `Deserialize`，不是简单派生反序列化。
- `parse_starter_bootstrap_file(data)`：解析、完整消费输入、校验并按版本升序排列 `upgrades`。`StarterBootstrapFile::pending_upgrades` 依赖这一排序用 `partition_point` 返回版本大于已存版本的连续切片；`needs_upgrade` 对文件落后于集群的情况只告警、不降级。
- `render_starter_bootstrap_sql(sql, keyspace)`：用 `EscapeString` 后的 keyspace 名替换所有 `<keyspace>`；调用者必须把占位符放在符合 SQL 上下文的位置。
- `load_starter_bootstrap_file()`：部署模式与配置路径门控的磁盘加载入口。
- `execute_starter_bootstrap_sql_blocks`：增量 SQL 执行器；允许清单为空，每块要求恰好一条语句，但不限制语句种类。
- `prepare_bootstrap_stmts` / `execute_bootstrap_stmts` / `verify_root_user` / `run_bootstrap_txn`：首次 bootstrap 与权限重置共用的“先全量解析和类型校验，再事务执行”链路。允许的 bootstrap 语句由 `ConcreteSession::validate_starter_bootstrap_statement` 判定为 INSERT、REPLACE、UPDATE 或 DELETE。
- `PRIVILEGE_RESET_TABLES` / `PRIVILEGE_RESET_BATCH_SIZE` / `delete_privilege_batch`：按固定八张 `mysql` 权限表、每批 128 行独立事务删除，降低大事务风险。
- `get_store_starter_bootstrap_version` / `finish_starter_bootstrap`：使用 `InternalTxnBootstrap` KV 上下文读取或写入 meta 键 `StarterBootstrapKey`；这是避免重复创建 Domain/Session 的完成快照。
- `reconcile_starter_bootstrap`：无 metadata 的便捷入口；`reconcile_starter_bootstrap_with_metadata` 是完整协调入口，锁 guard 的生命周期覆盖二次检查和实际变更。
- `BRANCH_RESET_DONE_KEY` / `RESTORE_RESET_DONE_KEY`、`PrivilegeResetState`、`StarterKeyspaceMeta`：权限重置元数据协议。
- `StarterPrivilegeResetMetadata`：把 codec 快照、PD 刷新和完成写回抽象为 `snapshot`、`refresh`、`complete`；生产实现 `StarterStoreMetadata` 位于 [`pkg/session/runtime/session.rs`](runtime/session.rs)。
- `update_privilege_reset_config`：内部 PD HTTP 客户端；支持 HTTP/HTTPS、10 秒超时、多 endpoint 顺序重试，并校验成功响应中的 keyspace `state`。

## 执行流程

启动主链如下：

1. `CanonicalSessionFactory` 完成核心 bootstrap、DDL runtime 与 Domain 启动后，调用 `reconcile_configured_starter_bootstrap`（`runtime/session.rs`）。
2. `load_starter_bootstrap_file` 检查 `IsStarter()` 和 `starter_params.bootstrap_file`。无文件时返回 `None`；若元数据仍要求权限重置，工厂会报错而不是继续启动。
3. 有清单时，`reconcile_starter_bootstrap_with_metadata` 读取权限重置快照和 KV 完成版本。没有 pending marker 且 KV 版本不落后时立即返回，不构造新 Session。
4. 需要工作时调用 `acquire` 获得与核心 bootstrap 相同命名空间的 owner 锁，并在锁内刷新 PD 元数据、重新读取 KV 完成版本；这样并发实例只有仍观察到待处理状态的一方继续。
5. 创建 `ConcreteSession` 并设置 Starter clustered-index 模式，再读取 `mysql.tidb` 的 SQL 版本。
6. 若存在权限重置标记：先拒绝“清单版本落后于 KV/SQL 已复制版本”的情况；随后分批清空八张权限表，事务性执行 bootstrap DML、校验 root、更新 SQL 版本，写 KV 完成版本，最后用 CAS precondition 把观察到的 PD 标记设为 `True`。
7. 普通路径中，若 SQL 版本已追上，则只用它修复可能因崩溃遗漏的 KV 完成键；SQL 版本为零时走首次 bootstrap 事务，否则按升序执行所有 `version > stored_version` 的升级条目。
8. 升级 SQL 每条通过 Session 正常执行边界独立提交；全部成功后才把 SQL 版本直接更新到文件目标版本，再写 KV 完成版本。调用方据此可在下一次启动重试部分失败的幂等升级。

## 数据与状态

系统维护三类相关状态：清单内的目标版本与 SQL；`mysql.tidb` 中 `starter_bootstrap_version` 的 SQL 可见版本；KV meta 中 `StarterBootstrapKey` 的快速完成版本。SQL 版本表示 SQL 工作已经完成，KV 版本是启动快路径标记。若进程在两次写入之间崩溃，下一次协调看到 SQL 版本不落后时会调用 `finish_starter_bootstrap` 修复 KV 标记。

首次 bootstrap 把 bootstrap DML、root 用户验证和 SQL 版本更新放在同一事务中，因此三者一起提交或回滚。增量 upgrades 则有意逐语句提交，失败前已成功的语句不会回滚，要求升级 SQL 可重复执行；只有全部升级成功后才推进 SQL 版本。权限表清理又是另一层粒度：每张表按 128 行一批提交，随后 bootstrap 事务负责恢复目标权限数据。

`StarterBootstrapFile::upgrades` 在解析成功后严格按 `version` 升序且无重复，`pending_upgrades` 才能安全使用分区点。`OnceLock<Regex>` 只初始化一次占位符正则并被各次解析共享。`PrivilegeResetState.pending_markers` 保存原始布尔字符串，以便完成 PATCH 的 precondition 精确匹配观察值。

## 依赖与调用关系

上游调用者：

- `pkg/session/runtime/session.rs::CanonicalSessionFactory::reconcile_configured_starter_bootstrap` 调用 `load_starter_bootstrap_file`、`parse_privilege_reset` 和 `reconcile_starter_bootstrap_with_metadata`；初始化失败时关闭 Domain。
- 同文件的生产 `StarterStoreMetadata` 实现 `StarterPrivilegeResetMetadata`：codec 提供初始快照，PD client 刷新 keyspace，PD 地址/TLS 信息交给 `update_privilege_reset_config` 完成标记。
- `CanonicalSessionFactory::from_storage_for_test` 也调用该协调入口，使独立 Rust 测试覆盖真实工厂接线。

主要下游依赖：

- `ConcreteSession` 提供 SQL 解析计数、语句类型校验、restricted SQL 状态保存/恢复、执行、协议 affected rows 与 clustered-index 模式；`quote_argument` 用于动态 SQL 字面量。
- `astersql-domain` 暴露共享存储句柄；`astersql-kv` 提供带 `InternalTxnBootstrap` 来源的读写事务；`astersql-meta` 生成 meta 键并应用 mutator 事务选项。
- `serde`/`serde_json` 负责兼容性 JSON 解码，`regex` 验证占位符，`astersql-util-sqlescape` 转义 keyspace，`astersql-util-logutil` 记录加载、升级、耗时及回滚告警。
- `reqwest`、`url` 与 `astersql-domain-infosync::UpdateKeyspaceConfigParams` 实现权限重置完成 PATCH。上述 crate 均由 [`pkg/session/Cargo.toml`](Cargo.toml) 声明；`nextgen` feature 透传到部署模式和 kernel type，部分 Starter 加载/工厂测试据此条件编译。

RustCodeGraph 的仓库索引状态可读取，但 `files --filter pkg/session/starter_bootstrap_file` 未返回目标文件，所以本任务没有把图中缺失的 callers/callees 当成事实；调用关系改由上述源文件符号引用和 `rg` 交叉核验。

## 错误处理与边界

解析边界包括：版本必须大于零；升级版本不得大于文件版本且不得重复；SQL 块去空白后不得为空；形如 `<...>` 的占位符只允许 `<keyspace>`；输入只能包含一个 JSON 值。未知 JSON 字段立即报错，而字段名匹配大小写不敏感；`null` 根对象会落为默认值并随后因版本零失败。

SQL 执行前先检查每块恰好一条语句。首次 bootstrap/权限重置还会在任何删除或执行前解析全部块并验证语句类型，避免因后续块非法而先破坏权限数据；增量升级保留 Go 行为，不做 DML 类型限制。所有结果集都会显式 `close`，close 错误也会传播。

事务主体失败后会尝试 `ROLLBACK`，但 rollback 自身失败只记录警告，返回原始主体错误。权限删除批次也采用同一原则。文件比集群状态旧时普通升级只告警并跳过；权限重置分支若发现 copied version 新于文件则硬错误，因为继续清表并重建会造成降级风险。

PD 完成请求对各 endpoint 依次尝试；网络错误和非 2xx 响应可转向下一个 endpoint，最终返回最后一个错误。2xx 响应还必须是 JSON 且包含受支持的 keyspace 状态。TLS 文件读取、证书解析、URL 生成和响应解析错误都会直接失败。当前实现是阻塞 HTTP，且 endpoint 之间串行尝试。

## 并发与资源生命周期

跨实例互斥由调用方传入的 `acquire` 闭包负责。`_guard` 在 `reconcile_starter_bootstrap_with_metadata` 返回前一直存活，覆盖锁内二次检查、Session SQL、KV 完成写入和 PD 标记完成。锁前快路径只减少开销，正确性依赖锁内再次读取状态。

`ConcreteSession` 在协调器内局部创建并借用同一个 `Arc<Domain>`；本文件不启动或关闭 Domain，生命周期由工厂管理。SQL `RecordSet` 在读完一行或执行完成后显式关闭。事务通过 `RunInNewTxn` 或显式 BEGIN/COMMIT 管理；显式路径在错误时尽力回滚。

权限重置的批量删除牺牲全局原子性换取较小事务和可重试性。升级也不是全局事务：注释和 Go 对照明确要求每条升级 SQL 幂等。PD PATCH 使用观察值作为 precondition，使并发元数据变化表现为 CAS 失败而非静默覆盖；调用方可在后续启动重新读取并重试。

`OnceLock` 保证占位符正则线程安全惰性初始化。配置和部署模式读取自全局状态；生产中预期启动期稳定，测试使用 RAII guard 恢复这些全局值。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/session/starter_bootstrap_file.go`](starter_bootstrap_file.go)，对应测试是 [`pkg/session/starter_bootstrap_file_test.go`](starter_bootstrap_file_test.go)。Rust 保留了 Go 的核心语义：清单字段和验证规则、版本排序及二分选择、Starter/空路径门控、单语句执行、restricted SQL 恢复、首次 bootstrap 事务、root 用户不变量、逐语句幂等升级、双版本崩溃修复、owner 锁二次检查、权限表批量删除、keyspace reset 标记及 PD CAS 完成。

Rust 手写 `Deserialize` 是为弥合 serde 与 Go `encoding/json` 默认行为的差异：字段名大小写不敏感、重复字段覆盖、`null` slice/元素转零值，同时仍执行 Go 的 `DisallowUnknownFields` 和单顶层值约束。错误文本也在反序列化阶段把反引号替换成双引号，以贴近 Go 测试可观察信息。

接线层面存在实现形态差异：Go `upgradeStarterBootstrap` 自行创建并销毁 session/domain、直接获取锁；Rust 在已经初始化的 `CanonicalSessionFactory` 中复用 Domain，并由工厂注入锁和 metadata 适配器。Go 使用 PD client/infosync；Rust 的生产适配器刷新元数据后调用本文件的阻塞 HTTP PATCH。两者目标状态机一致，但这些边界变化是 Rust 架构接线，不应误写成逐行翻译。

Rust 独立测试 [`pkg/session/starter_bootstrap_file_test.rs`](starter_bootstrap_file_test.rs) 对应 Go 测试覆盖解析兼容性、加载门控、SQL/事务行为、部分升级失败、版本快路径与锁内复查、DML/root 校验、大表分批删除、布尔标记、PD CAS/HTTP 和工厂启动接线。测试逻辑没有内嵌在生产源文件；仅由 `lib.rs` 的 `#[cfg(test)] mod starter_bootstrap_file_test` 装配。

## 扩展指南

- 增加 JSON 字段时，同时修改两个手写 `Deserialize` visitor、校验逻辑、Go `starterBootstrapFileSpec`/对应验证以及 Rust/Go 解析测试；明确 `null`、重复字段、大小写和未知字段兼容契约。
- 增加占位符时，修改 `validate_blocks` 与 `render_starter_bootstrap_sql`，并考虑占位符所在 SQL 语法上下文。仅做字符串替换不能替代标识符或参数绑定；必须增加恶意字符和转义回归用例。
- 改变首次 bootstrap 允许的语句种类，应从 `ConcreteSession::validate_starter_bootstrap_statement` 接入，并同步 `prepare_bootstrap_stmts` 相关测试。不要取消“全部解析/验证先于权限删除”的顺序。
- 增加或移除权限表时更新 `PRIVILEGE_RESET_TABLES`，同步 Go 列表及批量删除测试，并评估外键、历史表保留、事务大小和重试幂等性。
- 修改版本推进顺序时必须维护 SQL 版本与 KV 完成键的崩溃恢复不变量；增量升级不能假设失败会回滚先前语句。新升级 SQL应设计为可重复执行，并在 `starter_upgrade_partial_failure_keeps_old_version_and_committed_statement` 类测试中证明。
- 扩展 reset marker 或 PD 协议时修改 `parse_privilege_reset`、`privilege_reset_completion_params`、生产 metadata 实现和 HTTP 测试；保留 observed-value precondition，避免丢失并发修改。
- 涉及并发或启动时序的变更，应检查 `runtime/session.rs::reconcile_configured_starter_bootstrap` 与 owner lock 生命周期，确保核心 Domain/DDL 已可用且锁内仍进行二次状态检查。
- Rust 生产修改应同步独立测试文件 `pkg/session/starter_bootstrap_file_test.rs`，并继续与 Go 的同路径实现和测试保持语义一致，不把测试移入本源文件。

## 验证依据

- 目标源码：[`pkg/session/starter_bootstrap_file.rs`](starter_bootstrap_file.rs)，逐项核对全部常量、数据结构、trait、公开函数、私有辅助函数及事务/HTTP 分支。
- crate 与模块边界：[`pkg/session/Cargo.toml`](Cargo.toml)、[`pkg/session/lib.rs`](lib.rs)；确认 `astersql-session`、`nextgen` feature、模块公开与独立测试装配，以及 serde、regex、Domain、KV、meta、日志、SQL 转义等依赖。
- 直接上游与生产适配器：[`pkg/session/runtime/session.rs`](runtime/session.rs)；确认 Domain 启动后的调用顺序、owner lock 注入、`StarterStoreMetadata` 和失败关闭 Domain 的行为。
- Go 对照：[`pkg/session/starter_bootstrap_file.go`](starter_bootstrap_file.go) 与 [`pkg/session/session.go`](session.go)；后者在 Go 启动 bootstrap 主链调用 `upgradeStarterBootstrap`。
- 独立测试：[`pkg/session/starter_bootstrap_file_test.rs`](starter_bootstrap_file_test.rs) 与 [`pkg/session/starter_bootstrap_file_test.go`](starter_bootstrap_file_test.go)，覆盖解析、加载、首次初始化、升级、崩溃恢复、权限重置和 PD 完成边界。
- RustCodeGraph：`rustcodegraph status` 显示索引可用（11,467 files / 307,296 nodes / 1,848,419 edges），但 `rustcodegraph files --filter pkg/session/starter_bootstrap_file` 返回 `No files found matching the criteria`；因此没有虚构图调用边，改用源码符号引用与 `rg` 结果核验直接调用关系。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务规定的 11 章节结构命令，并人工复核唯一生产物、链接、事实限定和扩展风险。
