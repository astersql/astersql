# `pkg/planner/core/plan_cache_utils.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；模块由 `pkg/planner/core/lib.rs` 声明并整体再导出。它位于 PREPARE/EXECUTE 与优化器缓存实现之间，集中承载计划缓存的共享数据模型和纯算法：PREPARE 元数据生成、缓存键构造、缓存值统计、参数类型匹配、Point Get 安全判定和参数类型解析。实际会话接线位于 `pkg/session/runtime/planning.rs`：该文件调用 `NewPlanCacheKey` 与 `NewPlanCacheValue`，而会话级/实例级容器分别由 `plan_cache_lru.rs`、`plan_cache_instance.rs` 使用这里的 `PlanCacheValue` 和 `CheckTypesCompatibility4PC`。

`pkg/planner/core/Cargo.toml` 将此目录定义为 `astersql-planner-core`，并直接依赖 `astersql-bindinfo`、`sha2`、planner base/physical operator、expression、types、hint、resolve 等 crate；`default` feature 为空，`nextgen` 只向配置依赖透传，本文件没有条件编译的生产分支，只有 `NewPlanCacheValueForTest` 带 `#[cfg(test)]`。

## 核心职责

1. 维护全局 Prepared Plan Cache 内存上限（`PreparedPlanCacheMaxMemory`）以及参数化 LIMIT 可缓存上限（`MaxCacheableLimitCount = 10_000`）。
2. 把 AST、参数标记、权限信息、schema/table 版本、binding 信息和 Point Get 快路径槽位汇聚为 `PlanCacheStmt`，并由 `GeneratePlanCacheStmtWithAST` 固化 PREPARE 顺序。
3. 由 `NewPlanCacheKeyWithMatchedBinding` 将影响计划选择或可见性的会话/语句快照按固定顺序编码为 `PlanCacheKey`，并在子查询开关、参数化 LIMIT 开关或 LIMIT 过大时返回明确的不可缓存结果。
4. 以 `PlanCacheValue` 保存拥有型物理计划快照、输出列、参数类型、提示及展示信息，并以原子变量维护跨线程运行时统计和惰性内存估算。
5. 提供缓存复用的正确性门槛：`CheckTypesCompatibility4PC` 检查参数类型，`IsSafePointGetPath4PlanCache*` 检查 Point/Batch Point Get 的访问条件形状，`FullTextPreparedCacheability` 拒绝依赖实时 TiFlash/事务状态的 FTS 计划。

## 主要符号

- `PlanCacheParamMarker` / `ExtractAndSortParamMarkers`：按 SQL 源码偏移稳定排序参数标记，重写 `order`，并清除旧 datum/执行态。`GeneratePlanCacheStmtWithAST` 对非 prepared 参数化保留已有值，这是与通用辅助函数不同的有意分支。
- `PlanCacheStmt<ColumnInfo, Executor, FastPlan>`：PREPARE 模板；公开字段保存 AST、resolve/权限/digest/binding 等信息，私有字段保存 LIMIT、子查询、统计相关表与 MDL 表。`SetDBNameAndTbls` 接收拥有型 `Vec`，使 EXECUTE 刷新表对象时不污染缓存模板。
- `PointGetExecutorCache`：三个各自受 `Mutex<Option<_>>` 保护的类型化槽位。`Take*` 转移所有权并清空；`CloneFastPlan` 为实例缓存执行前复制计划；`Reset` 清空全部槽位。
- `PlanCachePrepareInput` / `PlanCachePrepareRuntime` / `GeneratePlanCacheStmtWithAST`：用输入快照与运行时 trait 隔离预处理、构建、权限校验和告警副作用。
- `PlanCacheKeyContext`、`PlanCacheKeyResult`、`NewPlanCacheKey*`：键输入快照、结果与构造入口。`HashInt64Uint64Map` 对 map key 排序，避免哈希表迭代顺序破坏确定性。
- `PlanCacheValue` / `NewPlanCacheValue`：拥有型缓存条目；优化器环境哈希为 `SHA-256(cache key || 每个参数类型字符串)` 的小写十六进制。
- `GetPreparedStmt` / `PreparedStatementStore`：先按已缓存 ID 查找；否则按名称解析 ID，再回填 `prepared_statement_id`。
- `CheckTypesCompatibility4PC`：允许 Varchar/VarString 互换，要求字符集与排序规则一致，整数符号位一致；DECIMAL 当前宽度和小数位不得超过缓存版本。
- `IsSafePointGetPath4PlanCacheScenario1..4`：分别识别全等值、单 IN、简单 DNF、等值加单 IN；场景 2–4 受 `fix_44830_enabled` 控制。
- `ParseParamTypes`：常量直接取 `FieldType`；文本协议变量从 `GetUserVarType` 查询，缺失或无法取常量类型时回退 `TypeNull`。

## 执行流程

PREPARE 流程由 `GeneratePlanCacheStmtWithAST` 明确排序：先拒绝带参数的 DDL、不支持的语句种类和超过 `u16::MAX` 的参数；再调用 `Preprocess`；随后按 offset 排序参数并编号，binary PREPARE 清除旧值而 non-prepared 参数化保留已经求出的值；在 `Build` 可能改写 AST 之前捕获原始 binding 归一化键；构建计划；结合 prepared/non-prepared 开关、AST 可缓存性、强制缓存修复开关和静态分区裁剪计算缓存资格并产生告警；最后组装 `PlanCacheStmt`、收集 LIMIT/表/MDL 元数据并执行权限校验。任一步骤报错都直接返回，不产生成功条目。

执行缓存查找时，上游从 `PlanCacheStmt` 与会话状态构造 `PlanCacheKeyContext`，调用 `NewPlanCacheKey`。键构造先检查语句文本和 schema version，再执行子查询/参数化 LIMIT 资格检查；之后按固定顺序编码用户、主机、数据库、SQL、schema/表修订、裁剪模式、隔离级别相关 schema、SQL mode、时区、隔离读引擎、select limit、binding、字符集/排序规则、只读限制、黑名单版本、外键开关等。参数化 LIMIT/OFFSET 单独编码且不得超过 10,000；可选统计版本和去重排序后的 dirty table ID 随后加入，末尾加入事务/autocommit/锁模式标志。

计划产生后，`NewPlanCacheValue` 复制展示信息、输出列、参数类型和 hints，保存 `CachedPlan`，计算优化器环境哈希，并立即调用 `MemoryUsage` 固化内存估算。`plan_cache_lru.rs` 和 `plan_cache_instance.rs` 在同一 key 下以 `CheckTypesCompatibility4PC` 选择或拒绝参数类型 bucket；命中后的执行统计通过 `UpdateRuntimeInfo` 原子累加。

## 数据与状态

`PlanCacheKey` 是私有 `Vec<u8>` 的不可变封装，只通过 `AsBytes` 暴露切片。带符号整数先翻转符号位再按大端编码，无符号整数直接大端编码；map 和 dirty table ID 均排序（后者还去重），因此相同逻辑输入不会受容器顺序影响。键没有字段长度前缀，兼容性依赖固定字段顺序和分隔位置，新增字段必须谨慎评估碰撞及跨版本行为。

`PlanCacheStmt` 拥有其 AST 和元数据；`Arc<PlanCacheStmt>` 用于 prepare 条目与存储接口。快照求值器是 `Arc<dyn Fn + Send + Sync>`，由 `EvaluateSnapshotTS` 返回 `Result<Option<u64>, _>`。`PlanCacheValue.Plan` 使用与会话上下文解耦的 `CachedPlan`，适合跨线程共享；`LoadTime` 为创建时系统时间。

`PlanCacheValue` 的 `executions`、键计数、延迟和最近使用时间均为原子量。`MemoryUsage` 首次以已知计划内存或 50 KiB 占位值为基数，再加结构、字符串 capacity、输出列及参数类型内存，缓存到 `memory`；后续字段若被扩充，现有缓存值不会自动重算，因此这些元数据在构造后应视为只读。

## 依赖与调用关系

上游直接证据包括：`pkg/session/runtime/planning.rs` 调用 `NewPlanCacheKey` 和 `NewPlanCacheValue`；`pkg/session/fts_runtime.rs` 调用 `FullTextPreparedCacheability`；`pkg/planner/core/plan_cache_lru.rs` 与 `plan_cache_instance.rs` 调用 `CheckTypesCompatibility4PC` 并持有 `PlanCacheValue`；`pkg/planner/core/logical_plan_builder_runtime.rs`、`operator/logicalop/logical_datasource.rs`、`optimizer_runtime.rs` 也在 RustCodeGraph 的文件使用者集合中。

下游方面，binding 归一化依赖 `astersql_bindinfo::{NormalizeStmtForBinding, CollectTableNames}`；键摘要依赖 `sha2::Sha256`；计划、列名、访问路径、表达式和类型分别来自 planner base/physicalop、planner util、expression 与 types crate；`resolve_dependency::ResultField` 构成 prepare 去重条目输出字段。文件不直接操作缓存容器、网络、磁盘或事务，而是以值对象、trait 与纯判定函数向这些层提供边界。

## 错误处理与边界

显式错误统一由 `PlanCacheError` 或 `SnapshotTSEvaluationError` 携带字符串并实现标准 `Error`。PREPARE 会拒绝 DDL 参数、不支持语句和参数过多；runtime 的 preprocess/build/权限错误用 `?` 原样传播。缓存键对空 SQL、未初始化 schema version 返回错误；测试可用 `allow_uninitialized_schema_version_for_test` 放宽后者。开关关闭或 LIMIT 过大不是执行错误，而是 `PlanCacheKeyResult { key: None, cacheable: false, reason }`。

类型匹配中任一侧为空切片即视为无需比较；非空长度不同必不兼容。Point Get 判定对空 range、非标量函数、函数名或 range 数/宽度不匹配均保守返回 false；场景 4 只允许一个 IN，暂不支持多个 IN 的笛卡尔积。`ParseParamTypes` 假设非常量表达式是 GetVar 形态，但 Rust 实现用安全 downcast 和缺省名称避免 panic，最终回退 NULL 类型。

FTS 检查目前只递归覆盖 WHERE、投影字段和 ORDER BY 中的 function/binary/unary/parentheses 表达式；若 AST 新增会承载 `fts_match_word` 的位置或表达式种类，需要同步扩展，否则可能漏判。系统时间早于 Unix epoch 时运行统计以默认零处理，读取时负值也夹到零。

## 并发与资源生命周期

全局内存上限以 `AtomicU64` 的 Relaxed 顺序读写，只要求最终可见，不建立其他状态的同步关系。Point Get 三个槽位各有独立 `Mutex`，一次操作只持有一个锁；锁毒化时通过 `into_inner` 继续使用已有状态。`Reset` 逐槽清空，并非对三个槽位的原子事务，调用方不得假设并发观察到“全有或全无”。

`PlanCacheValue` 的累加统计使用 Relaxed；最近使用时间及内存缓存用 Release/Acquire 发布。`MemoryUsage` 允许并发重复计算，结果相同后覆盖，不需要互斥。`CachedPlan` 和 cloneable fast plan 体现实例缓存的共享边界：共享模板不应在执行中原地修改，需在进入可变执行路径前克隆。`Arc` 管理 statement、evaluator 和缓存 value 的生命周期，文件内没有后台任务、通道或显式资源关闭逻辑。

## 与 Go 版本的对应关系

直接对照 `pkg/planner/core/plan_cache_utils.go`：Rust 保留 `MaxCacheableLimitCount`、`PreparedPlanCacheMaxMemory`、`GeneratePlanCacheStmtWithAST`、`NewPlanCacheKey`、`PlanCacheValue`、`GetPreparedStmt`、类型兼容规则、安全 Point Get 四场景和 `parseParamTypes` 的核心顺序与判定。缓存键同样纳入 schema/table 版本、会话优化环境、binding、统计版本、dirty tables 与事务状态；计划值同样以键和参数类型计算 SHA-256 环境摘要并缓存内存估算。

Rust 的边界适配并非行为简化：Go 从 session/domain 动态读取状态，Rust 通过 `PlanCacheKeyContext` 显式传入不可变快照；Go 的 `any` Point Get 槽位改为三个泛型参数；Go 的 session/infoschema/build 操作改由 `PlanCachePrepareRuntime` trait 注入；Go 的 `base.Plan` 改为拥有型 `CachedPlan`，使实例缓存跨线程共享更明确。Go 通过 AST visitor 收集 LIMIT/表信息，Rust 当前由调用者填充 `PlanCachePrepareInput`，因此调用方必须保持同等收集完整性。

值得关注的差异是内存统计口径：Go 依据具体物理计划动态 `MemoryUsage`，Rust 由 `PlanCacheValueBuildInfo.plan_memory_usage` 传入，未知时同样采用 50 KiB；Rust 字符串使用 capacity 计量，Go 多使用 len。Go 的 fix 44830 从 session fix-control map 读取，Rust入口接收已解析的布尔值。测试和调用接线必须保证这些适配层持续提供与 Go 相同的事实。

## 扩展指南

新增影响计划选择的会话变量时，应先扩展 `PlanCacheKeyContext`，再在 `NewPlanCacheKeyWithMatchedBinding` 的稳定位置编码，并补充键相等/不等回归；同时核对 Go `newPlanCacheKeyWithMatchedBinding`，避免 Rust 漏键导致错误复用。新增参数化 LIMIT 规则应修改 `PlanCacheLimit` 与键编码，并覆盖开关关闭、边界 10,000/10,001 和 offset/count 组合。

新增 PREPARE 语句类别或元数据时，应修改 `PlanCachePrepareInput`、`GeneratePlanCacheStmtWithAST` 和 `PlanCacheStmt`，保持“校验→预处理→排序→构建→资格覆盖→元数据→权限”的顺序；binding 必须继续在 build 改写 AST 前捕获。新增 Point Get 安全形态应以新的独立判定函数接入总入口，默认保守拒绝，并验证重复 IN 值、range 去重、复合键宽度和修复开关。

新增 `PlanCacheValue` 元数据时要同步 `NewPlanCacheValue` 和 `MemoryUsage`，保持构造后只读；新增可变统计应采用原子字段并定义一致的快照语义。测试逻辑不得放回生产文件：优先扩展同目录的 `plan_cache_utils_test.rs` 或 `plan_cache_utils_aster_unit_test.rs`；跨路径行为可扩展 `tests/prepare/prepare_test.rs`、`casetest/plancache/*`，容器行为则扩展 `plan_cache_lru_test.rs` / `plan_cache_instance_test.rs`。

## 验证依据

- RustCodeGraph：索引状态为 11,467 files / 307,296 nodes / 1,848,419 edges；目标文件已索引为 1,302 行、112 个符号，并显示被 11 个文件使用。执行过 `files`、`explore`、`query`、`node`、`callers`、`callees`，重点核对 `NewPlanCacheKey*`、`PlanCacheValue`、`NewPlanCacheValue`、`CheckTypesCompatibility4PC` 及目标文件全段源码。
- 源与装配：`pkg/planner/core/plan_cache_utils.rs`；`pkg/planner/core/lib.rs` 的 `mod plan_cache_utils`、`pub use plan_cache_utils::*` 以及两个独立测试模块声明；`pkg/planner/core/Cargo.toml` 的 crate、feature 与依赖声明。
- Go 对照：`pkg/planner/core/plan_cache_utils.go` 中 `GeneratePlanCacheStmtWithAST`、`newPlanCacheKeyWithMatchedBinding`、`PlanCacheValue`、`PointGetExecutorCache`、`PlanCacheStmt`、`GetPreparedStmt`、`checkTypesCompatibility4PC`、四个 Point Get 场景及 `parseParamTypes`。
- Rust 测试：`pkg/planner/core/plan_cache_utils_test.rs` 验证 non-prepared 参数值在排序后保留，以及 binding 在 build 前捕获且不依赖缓存开关；`pkg/planner/core/plan_cache_utils_aster_unit_test.rs` 验证槽位生命周期、statement 元数据/快照、键确定性与 LIMIT 上限、原子统计/内存、类型规则和四个安全场景。`pkg/planner/core/tests/prepare/prepare_test.rs` 直接验证公开类型兼容 API；`casetest/plancache/plan_cache_suite_test.rs`、`plan_cache_partition_test.rs`、`plan_cache_partition_table_test.rs` 验证开关、裁剪模式与新鲜统计对键的影响。
- 调用证据：`pkg/session/runtime/planning.rs` 的键/value 构造；`pkg/session/fts_runtime.rs` 的 FTS 可缓存性检查；`pkg/planner/core/plan_cache_lru.rs`、`plan_cache_instance.rs` 的类型匹配与值存储。
- 本任务是纯文档分析，按总计划不运行 Cargo；结构检查要求本文恰含规定的 11 个二级标题。
