# `pkg/util/sem/v2/restricted_hint.rs`

## 文件定位

`SemImpl::isRestrictedHint` 实现策略：不在 `restrictedHints` 集合中的名称直接放行；在集合中且没有守卫变量的 Hint 无条件受限；有守卫变量时，只有变量“既不隐藏、也不只读”才例外放行，否则返回限制错误。

## 核心职责

1. `hintGuardVars` 描述三个“通过覆盖系统变量生效”的 Hint 与系统变量之间的关系：`memory_quota -> tidb_mem_quota_query`、`read_consistent_replica -> tidb_replica_read`、`max_execution_time -> max_execution_time`。映射值使用 `vardef` 常量，避免硬编码变量拼写。
2. `IsRestrictedHint` 提供 crate 对外入口。SEM 未启用（`globalSem.Load()` 为 `None`）时所有 Hint 均放行；启用后委托当前 `SemImpl` 快照判断。
3. `SemImpl::isRestrictedHint` 实现策略：不在 `restrictedHints` 集合中的名称直接放行；在集合中且没有守卫变量的 Hint 无条件受限；有守卫变量时，只有变量“既不隐藏、也不只读”才例外放行，否则返回限制错误。

这里的“受限”不是拒绝整条 SQL。`pkg/session/hint_runtime.rs::restricted_hint_checker` 把错误转换为 hint crate 的无栈 warning；`pkg/util/hint/hint.rs::filterRestrictedHints` 移除对应 Hint，保留 SQL 继续执行。

## 主要符号

| 符号 | 可见性 | 语义 |
| --- | --- | --- |
| `hintGuardVars: LazyLock<HashMap<&'static str, &'static str>>` | `pub` | 小写 Hint 名到规范系统变量名的只读映射。首次访问时构造，之后进程内复用。 |
| `IsRestrictedHint(hintNameLower: &str) -> Result<(), String>` | `pub` | 全局入口；`Ok(())` 表示允许保留，`Err` 表示应过滤并警告。参数契约明确要求小写名称。 |
| `SemImpl::isRestrictedHint(&self, hintNameLower: &str) -> Result<(), String>` | `pub(crate)` | 对一个已构建的 SEM 快照执行集合命中与守卫变量判定，便于 crate 内测试直接覆盖策略分支。 |

文件没有 trait、枚举、条件编译项或可变模块级状态。`#[allow(non_snake_case)]` 与 `#[allow(non_upper_case_globals)]` 保留 Go API/符号命名，属于迁移兼容而非新的 Rust 命名约定。

## 执行流程

调用链的主流程如下：

1. `pkg/util/sem/v2/sem.rs::buildSEMFromConfig` 将 `Config.RestrictedHints` 中每个名称转为小写并收集到 `SemImpl.restrictedHints`；`EnableBy` 发布该实例后，`globalSem` 才为非空。
2. `pkg/session/hint_runtime.rs::InitializeHintRuntime` 使用 `Once` 注册 `restricted_hint_checker`；后者调用本文件的 `IsRestrictedHint`。
3. `pkg/util/hint/hint.rs` 的 `ParseStmtHints` 与 `ParsePlanHints` 都先调用 `filterRestrictedHints`。过滤器传入 AST 中的 `HintName.L`（小写形式），因此满足本文件的入参契约。
4. `IsRestrictedHint` 读取当前 `Arc<SemImpl>` 快照。无快照时返回 `Ok(())`；有快照时进入 `SemImpl::isRestrictedHint`。
5. 内部方法先查询 `restrictedHints`。未命中立即返回 `Ok(())`；命中后查询 `hintGuardVars`。若存在映射，并且 `isInvisibleSysVar(variable)` 与 `isReadOnlyVariable(variable)` 都为 `false`，仍返回 `Ok(())`；其余情况返回包含大写 Hint 名的错误文本。
6. 上层过滤器收到错误后丢弃该 Hint。statement 与 plan 两条解析路径用互补的 `shouldWarn` 规则分配 warning，避免同一 Hint 重复告警。

## 数据与状态

- `hintGuardVars` 的键和值都是静态字符串切片；`HashMap` 在首次访问时由 `LazyLock` 构造，之后不再修改。查找平均为常数时间，固定三项的内存开销稳定。
- `SemImpl.restrictedHints` 是 `HashSet<String>`。构建时统一小写，因此判定只做精确集合查询，不在热路径重复分配或大小写转换；错误路径为展示信息调用 `to_uppercase()`，会分配一个字符串。
- 守卫变量状态来自同一 `SemImpl.restrictedVariables` 快照。`isInvisibleSysVar` 读取 `hidden`，`isReadOnlyVariable` 读取 `readonly`；变量未配置时两者均为 `false`，因此有守卫映射的受限 Hint 会被放行。
- `globalSem.Load()` 从 `RwLock<Option<Arc<SemImpl>>>` 克隆 `Arc`。一次判断始终使用获得时的快照；并发 `Enable`/`Disable` 不会改变该次调用已经持有的实例。
- 本文件不自行规范化 `hintNameLower`。若绕过正式解析链传入大写名称，通常不会命中小写集合或映射；调用者必须遵守参数名和上游 `HintName.L` 所表达的小写契约。

## 依赖与调用关系

RustCodeGraph 对目标文件列出两个直接使用者：`pkg/session/hint_runtime.rs` 与 `pkg/util/sem/v2/migration_aster_unit_test.rs`，并确认 `IsRestrictedHint -> SemImpl::isRestrictedHint` 调用边。精确源码检索补充了完整关系：

- 上游生产入口是 `pkg/session/hint_runtime.rs::restricted_hint_checker`，由 `InitializeHintRuntime` 注册到 `astersql-util-hint`。`pkg/util/hint/hint.rs::filterRestrictedHints` 在 statement/plan Hint 解析前调用该检查器。
- 下游同 crate 依赖为 `globalSem.Load()`、`SemImpl.restrictedHints`、`SemImpl::isInvisibleSysVar` 和 `SemImpl::isReadOnlyVariable`，实现位于 [`sem.rs`](sem.rs)。
- 外部直接依赖只有 `vardef::{TiDBMemQuotaQuery, TiDBReplicaRead, MaxExecutionTime}`；这些常量来自 `Cargo.toml` 声明的 `astersql-sessionctx-vardef` 路径依赖。
- [`restricted_test.rs`](restricted_test.rs) 直接调用 crate 内方法验证细粒度分支；`migration_aster_unit_test.rs` 通过公开 `IsRestrictedHint` 验证启用后的整体配置；`pkg/session/hint_runtime_test.rs` 验证 `RESOURCE_GROUP` 最终被过滤且产生包含 `restricted` 的 warning。

RustCodeGraph 的通用 `callers` 查询受同名 Go/Rust 符号影响，没有单独返回公共入口的调用者；上述生产调用边因此同时用索引的“used by”结果和精确 `rg` 引用核验。

## 错误处理与边界

- 正常放行统一返回 `Ok(())`，限制返回 `Err(String)`；没有 I/O、解析或可恢复的下游错误需要传播。
- 限制错误格式为 `the NAME() optimizer hint is restricted under the current security policy and is ignored`，其中 `NAME` 由输入转为大写。该文本被上层作为 warning 展示，改变措辞可能影响用户可见诊断及断言。
- SEM 未启用是显式开放边界，而非错误。受限集合未命中也是开放边界。
- 有守卫变量的 Hint 采用“双否定放行”：仅当变量既不可见为假、只读也为假时放行；隐藏或只读任一条件为真都限制。无守卫变量时，一旦列入集合便没有变量例外。
- `globalSem.Load()` 内部在锁中毒时 `expect` 并 panic；本文件不捕获该进程内部不变量破坏。`LazyLock` 初始化固定映射也没有可失败分支。
- 现有专项测试覆盖隐藏变量，但未单独覆盖“可见但只读”、`read_consistent_replica` 映射、大小写契约和精确错误文本；这些是扩展时应补齐的边界，而不能据现有测试宣称已直接验证。

## 并发与资源生命周期

`hintGuardVars` 由标准库 `LazyLock` 保证线程安全的一次初始化，初始化后仅共享读取。当前 SEM 通过 `globalSem` 内部 `RwLock<Option<Arc<SemImpl>>>` 发布：读取时短暂持有读锁并克隆 `Arc`，随后在无锁状态查询不可变的 `HashSet`/`HashMap`。因此本文件没有跨调用锁持有、任务、通道、文件句柄或显式清理逻辑。

上层 checker 的生命周期独立于 SEM 实例：`pkg/session/hint_runtime.rs` 以 `Once` 进程级注册函数指针；`Enable`/`Disable` 只替换 `globalSem` 内容，checker 无需重注册。并发切换配置时，一次调用看到切换前或切换后的完整 `Arc` 快照，不会看到部分构造状态。

## 与 Go 版本的对应关系

配置侧也保持一致：Go `buildSEMFromConfig` 使用 `strings.ToLower`，Rust 同名函数使用 `to_lowercase`；Go 测试 [`restricted_test.go`](restricted_test.go) 与 Rust [`restricted_test.rs`](restricted_test.rs) 使用相同配置和四个断言，分别覆盖无守卫受限、隐藏变量受限、仍可调变量放行、未配置 Hint 放行。

实现层面的差异主要是并发原语和错误类型：Go 读取原子指针并返回格式化 `error`；Rust 从 `RwLock` 包装的全局指针克隆 `Arc`，返回字符串错误，再由 session 适配为 hint 错误。该差异不改变正式调用链上的过滤语义。

## 扩展指南

- 新增“覆盖系统变量”的受限 Hint 时，在 `hintGuardVars` 增加小写 Hint 名与 `vardef` 规范常量的映射；不要写裸变量字符串。只有确实以该变量可调性作为安全边界的 Hint 才应加入，否则应维持无条件限制。
- 修改放行规则时，优先修改 `SemImpl::isRestrictedHint`，并同步独立测试文件 [`restricted_test.rs`](restricted_test.rs)；不得把测试内嵌进生产源文件。至少覆盖变量隐藏、只读、既不隐藏也不只读、变量未配置，以及无守卫 Hint。
- 修改公开返回类型或错误文本时，还需同步 `pkg/session/hint_runtime.rs::restricted_hint_checker`、`pkg/session/hint_runtime_test.rs` 和 Go 对照文件/测试，确认“过滤而非拒绝 SQL”以及 warning 归属仍成立。
- 若允许非小写调用，必须在入口统一规范化，并评估每次调用的分配成本；当前正式链路已提供小写 `HintName.L`，贸然在热路径重复转换没有必要。
- 新增映射的兼容风险在于错误放行：若把不等价的 Hint 与变量关联，变量未配置时会因“可见且可写”而放行。性能风险较低，判定只有固定映射和集合查询；主要正确性风险是配置归一化、变量规范名和安全策略三者不一致。
