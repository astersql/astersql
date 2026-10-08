# `pkg/util/sem/v2/sem.rs`

## 文件定位

本文件是 `astersql-util-sem-v2` crate 的运行时策略核心，源码见 [`sem.rs`](sem.rs)。crate 入口 [`lib.rs`](lib.rs) 将它声明为私有 `sem` 模块并把公共项重新导出；crate 边界、依赖和 Go 包映射由 [`Cargo.toml`](Cargo.toml) 定义。它位于“配置解析”与“业务调用方”之间：[`config.rs`](config.rs) 产生并校验 `Config`，本文件把配置编译成进程级 `SemImpl`，服务启动、权限检查、会话派发和兼容层再通过公开查询函数读取策略。

服务入口 `cmd/tidb-server/main.rs::setupSEM` 在 `Security.EnableSEM` 为真且 `Security.SEMConfig` 非空时调用 `semv2::Enable`；没有自定义配置时走 SEM v1，而不是本文件。因此这里实现的是配置驱动的 SEM v2，不是默认 SEM 规则集。

## 核心职责

1. 管理全局启用状态：`globalSem: LazyLock<AtomicSemPointer>` 保存 `Option<Arc<SemImpl>>`，`Enable`/`EnableBy` 安装实例，`Disable` 清空实例，`IsEnabled` 查询状态。
2. 编译配置：`buildSEMFromConfig` 把数据库、表、变量、状态变量、权限、hint 和 SQL 限制转换为适合查询的 `HashSet`/`HashMap` 与闭包。
3. 提供无实例时默认放行的查询门面：`IsInvisibleSchema`、`IsInvisibleTable`、`IsRestrictedPrivilege`、`IsInvisibleSysVar`、`IsReadOnlyVariable`、`IsInvisibleStatusVar` 和 `IsRestrictedSQL` 在 SEM 未启用时都返回 `false`。
4. 产生启用副作用：`EnableBy` 先覆盖配置指定的系统变量，再发布全局实例，最后把 `TiDBEnableEnhancedSecurity` 设为 `"CONFIG"` 并写启用日志。

本文件只给出“某对象是否受限”的策略结果，不自行完成授权豁免或生成最终 SQL 错误。例如 `pkg/session/runtime/dispatch.rs` 在 `IsRestrictedSQL` 为真后仍检查 `RESTRICTED_SQL_ADMIN`，`pkg/privilege/privileges/privileges.rs` 在隐藏表/schema 规则前检查 `RESTRICTED_TABLES_ADMIN`。

## 主要符号

- `type SEMSQLValidateFn = Arc<dyn Fn(&dyn ast::Node) -> bool + Send + Sync>`：可跨线程共享的 SQL 限制判定器；返回 `true` 表示语句受限。
- `AtomicSemPointer`：用 `RwLock<Option<Arc<SemImpl>>>` 模拟 Go 的 `atomic.Pointer[semImpl]`。`Load` 克隆 `Arc` 快照，`Store` 整体替换或清空当前实例。
- `globalSem`：进程级惰性初始化单例。其可见性为 `pub(crate)`，供 [`restricted_hint.rs`](restricted_hint.rs) 与 [`testhelper.rs`](testhelper.rs) 使用。
- `SemImpl`：已编译运行时状态。数据库、状态变量和 hint 用集合保存；表按 `schema -> table -> RestrictedTableAttr` 两级映射；变量映射到 `RestrictedVariableAttr`；权限集合单独使用 `RwLock`，以支持测试辅助函数动态增删。
- `RestrictedVariableAttr { hidden, readonly, value }`：分别驱动变量隐藏、只读判断和启用时的全局值覆盖。
- `RestrictedTableAttr { hidden }`：保存表级隐藏标志。
- `Enable(configPath)`：解析 JSON 文件后委托 `EnableBy`，错误类型为 `Result<(), String>`。
- `EnableBy(semConfig)`：校验配置、构建实例、应用变量覆盖并发布全局状态。
- `buildSEMSqlValidateFunction`：把 SQL 命令名集合与命名 `SQLRule` 编译为判定闭包。
- `buildSEMFromConfig`：把 `Config` 编译为 `Arc<SemImpl>`，是本文件的主要内部构造入口。

公开函数保留了 Go 风格大写命名；内部方法仅在 crate 内使用。`SemImpl::restrictedPrivileges` 和 `restrictedHints` 为 `pub(crate)`，其余运行时字段均为本模块私有。

## 执行流程

启用主流程如下：

1. `cmd/tidb-server/main.rs::setupSEM` 根据服务安全配置调用 `Enable(path)`。
2. `Enable` 断言 `globalSem.Load()` 为空，再由 `parseSEMConfigFromFile` 打开并反序列化配置，随后调用 `EnableBy`。
3. `EnableBy` 再次断言尚未启用，并调用 `validateSEMConfig`。该校验核对 TiDB 版本下界、受限系统变量是否存在、带强制值的变量是否为 `ScopeNone`，以及命名 SQL 规则是否已注册。
4. `buildSEMFromConfig` 建立各类索引。权限统一转大写，hint 统一转小写；SQL 命令由 `buildSEMSqlValidateFunction` 执行 `trim + to_uppercase` 并丢弃空字符串。
5. `SemImpl::overrideRestrictedVariable` 把每个非空 `value` 写入系统变量注册表。
6. `globalSem.Store(Some(sem))` 发布不可变主体的共享实例；随后增强安全系统变量被设为 `"CONFIG"`，并记录后台日志。

查询流程统一为“加载快照—若存在则委托—否则返回 false”。表查询先调用 `isInvisibleSchema`：只要 schema 受限，其下所有表都不可见；schema 未命中时才检查表级 `hidden`。权限查询把任何 `RESTRICTED_` 前缀权限视为受限，否则读取配置集合。SQL 查询同时支持命令名精确匹配与任一命名规则命中，二者为逻辑或。

关闭流程 `Disable` 先清空全局实例，再将 `TiDBEnableEnhancedSecurity` 设为 `Off`。它不会恢复 `overrideRestrictedVariable` 改写的其他系统变量；测试恢复由 [`sem_test.rs`](sem_test.rs) 和 [`testhelper.rs`](testhelper.rs) 的独立清理逻辑负责。

## 数据与状态

`SemImpl` 发布后，大部分字段不再变化，查询只读。名称规范化并不完全对称：

- `isInvisibleSchema` 会把查询参数转小写，但 `buildSEMFromConfig` 原样收集配置中的数据库名。
- `isInvisibleTable` 按函数契约接收已转小写的 schema/table 名，并以原值查询构建时原样保存的配置键。
- 受限权限在构建时转大写，公开 `IsRestrictedPrivilege` 断言调用参数已经大写。
- hint 在构建时转小写，并由邻接模块 `IsRestrictedHint` 按“小写 hint 名”契约查询。
- 系统变量与状态变量在构建和查询时都不做大小写转换。

因此新增调用点不能假设所有键都会在本文件自动规范化；应遵守各 API 的参数契约，配置生产方也应提供与查询键一致的形式。重复的表、变量、权限、状态变量、hint 或 SQL 项通过 `HashMap`/`HashSet` 自然覆盖或去重；相同 schema 的多张表合并到同一个内层映射。

`Arc<SemImpl>` 保证一次 `Load` 获得的快照在查询期间保持有效，即使其他线程随后调用 `Disable` 或安装新实例。SQL 闭包捕获构建完成的命令集合和规则映射，与该实例共享生命周期。

## 依赖与调用关系

直接下游依赖如下：

- crate 内的 `Config`、`SQLRestriction`、`SQLRule`、`parseSEMConfigFromFile`、`validateSEMConfig`、`semCommand`、`sqlRuleNameMap` 分别提供配置模型、解析/校验及 AST 规则能力。
- `ast::Node` 是 SQL 判定输入；`variable`/`vardef` 负责系统变量查询与增强安全状态；`intest::Assert` 表达迁移自 Go 的开发期不变量；`logutil` 记录启用和未知规则日志。
- 标准库 `HashMap`/`HashSet` 承载查询索引，`Arc`/`LazyLock`/`RwLock` 承载全局共享与并发访问。

已核验的主要上游路径包括：

- `cmd/tidb-server/main.rs::setupSEM -> semv2::Enable`：服务启动安装策略。
- `pkg/session/runtime/dispatch.rs -> IsEnabled + IsRestrictedSQL`：拒绝无 `RESTRICTED_SQL_ADMIN` 的受限语句。
- `pkg/privilege/privileges/privileges.rs -> IsInvisibleTable/IsInvisibleSchema`：隐藏受限对象并限制系统对象写权限。
- `pkg/privilege/privileges/cache.rs -> IsRestrictedPrivilege`：禁止受限动态权限回退到 `SUPER`。
- `pkg/planner/core/expression_rewriter.rs -> IsInvisibleSysVar`：为隐藏变量追加 `RESTRICTED_VARIABLES_ADMIN` 访问要求。
- `pkg/util/sem/compat/sem.rs`：在保证 v1/v2 互斥后，把 schema、表、变量、状态变量和权限查询转发给启用中的版本。
- [`restricted_hint.rs`](restricted_hint.rs)：通过 `globalSem.Load()` 和 `SemImpl` 的变量属性判断受限 hint 是否仍可放行。
- [`testhelper.rs`](testhelper.rs)：通过 `restrictedPrivileges.write()` 在测试中动态增删权限。

`Cargo.toml` 没有 feature 开关；该模块随 `astersql-util-sem-v2` crate 编译。运行时直接依赖 `ast`、`intest`、`logutil`、`mysql`、`vardef`、`variable`，配置模块还使用 `serde`、`serde_json`、`semver` 和 `objstore`。测试依赖 `parser`、`serial_test` 与 `tempfile`。

## 错误处理与边界

- 文件打开、JSON 解码和配置校验错误通过 `String` 原样向 `Enable` 调用者传播；实例只在这些步骤全部成功后发布。
- 重复启用不是普通 `Err`：`Enable` 和 `EnableBy` 都用 `intest::Assert` 维护“启用前全局为空”的不变量。调用方应先保证生命周期正确，不能把断言当作可恢复分支。
- `validateSEMConfig` 会在构建前拒绝未知 SQL 规则；`buildSEMSqlValidateFunction` 仍保留未知规则的警告与断言，作为内部不变量防线。
- `AtomicSemPointer` 和权限集合的锁若中毒，会因 `expect(...)` 触发 panic，而不是降级为未启用或未受限。
- `variable::SetSysVar` 在覆盖、启用标志设置和关闭标志设置处的返回值均被丢弃。因此这些写入失败不会从 `EnableBy`/`Disable` 传播；扩展错误语义时必须评估与 Go 版本的兼容性及“实例已发布但标志写入失败”的状态一致性。
- `buildSEMSqlValidateFunction(None)` 返回 `None`；当前 `buildSEMFromConfig` 总是传入 `Some(&cfg.RestrictedSQL)`，即使配置为空也会得到一个始终不命中的闭包。
- SEM 未启用时所有公开策略查询默认返回 `false`，避免把“未配置”误当作“全部受限”。

## 并发与资源生命周期

全局状态由 `LazyLock` 延迟创建，`AtomicSemPointer` 用读写锁保护整项 `Option<Arc<SemImpl>>`。`Load` 在短读锁内克隆 `Arc` 后立即释放锁，实际策略计算不持有全局锁；`Store` 只在替换指针时持有写锁。这使读取者能够继续使用旧快照，而不会引用已释放对象。

`SemImpl` 中只有 `restrictedPrivileges` 需要内部可变性，原因是测试辅助 API 会动态修改它；生产配置构建后其余集合与闭包保持只读。SQL 判定闭包要求 `Send + Sync`，可以随 `Arc<SemImpl>` 跨线程调用。源码注释和 Go 对照都说明生产正常路径只在初始化时设置 SEM，反复启停主要服务于测试；`Enable`/`Disable` 的多步骤副作用也没有被一个总锁或事务包裹，因此不应把它们当作可与查询或彼此任意并发的原子状态切换。

独立 Rust 测试用 `serial_test::serial` 串行执行涉及全局 SEM、系统变量注册表和发布版本的用例，并用 `Drop` 清理全局状态及恢复变量值。这是全局可变资源生命周期的测试证据，不应把这些测试内清理保证外推为生产 `Disable` 的行为。

## 与 Go 版本的对应关系

直接对照文件是 [`sem.go`](sem.go)，独立测试对照为 [`sem_test.go`](sem_test.go) 与 [`sem_test.rs`](sem_test.rs)。Rust 保留了 Go 的公开函数、内部类型/方法、启用顺序和主要规则：未启用返回 false；schema 隐藏覆盖表级设置；`RESTRICTED_` 前缀天然受限；权限/SQL 命令/hint 分别转大写、大写和小写；配置变量先覆盖，增强安全标志最后设为 `CONFIG`。

实现层面的主要差异是：

- Go 用 `atomic.Pointer[semImpl]`，Rust 用 `RwLock<Option<Arc<SemImpl>>>`；Rust 的 `Load` 返回拥有共享引用计数的快照。
- Go 的 `semImpl.restrictedPrivileges` 是普通 map，Rust 为配合测试辅助的动态增删使用 `RwLock<HashSet<String>>`。
- Go 的 SQL 判定参数是 `ast.StmtNode` 并直接调用 `stmt.SEMCommand()`；Rust 接收 `&dyn ast::Node`，通过 `semCommand(stmt)` 做命令映射。
- Go 日志使用结构化字段记录未知规则，Rust 当前把规则名格式化进字符串。
- Rust 的系统变量写入显式忽略 `Result`；行为结果与 Go 不传播写入错误一致，但 Rust 实现中这一取舍可从 `let _ = variable::SetSysVar(...)` 直接观察。

Rust 测试覆盖了 Go 测试的权限大小写归一化、schema/表隐藏、变量隐藏、强制值覆盖和从临时 JSON 文件启用后的公开 API，并额外显式注册/注销系统变量、串行化全局状态用例。当前这两份测试未直接覆盖本文件的状态变量、只读查询、SQL 命令/规则闭包、重复启用断言或锁中毒路径；相关 SQL 规则行为还分布在 `sql_rule_test.rs` 与迁移测试中。

## 扩展指南

- 新增一种配置属性时，应在 [`config.rs`](config.rs) 定义和校验数据，在 `SemImpl` 增加查询友好的运行时表示，在 `buildSEMFromConfig` 完成唯一编译入口，并新增公开门面或邻接模块方法；不要让业务调用方反复扫描原始 `Config`。
- 新增 SQL 限制规则时，应同步 `sqlRuleNameMap`/`SQLRule`、配置校验和 `buildSEMSqlValidateFunction`，并在独立 `sql_rule_test.rs` 或 `sem_test.rs` 增加解析真实 AST 的测试。
- 新增名称型规则时，要明确配置端、构建端和查询端各自的大小写契约，并补大小写与重复项用例；现有不同类别的规范化策略并不统一。
- 修改启用/关闭副作用时，要保持“校验失败不发布实例”，并明确系统变量写入失败的处理和回滚顺序；若要求真正原子切换，需要同时设计变量恢复和并发观察语义，而不只是更换指针容器。
- 修改全局状态或权限集合时，要同步 [`sem_test.rs`](sem_test.rs) 的串行夹具和 [`testhelper.rs`](testhelper.rs) 的清理/锁逻辑。Rust 单元测试必须继续放在独立测试文件，不应嵌入 `sem.rs`。
- 改动公开兼容行为时，还应同步检查 `pkg/util/sem/compat/sem.rs`、服务启动、会话派发、权限缓存/校验和表达式重写调用点；性能风险主要来自给高频权限/规划路径增加锁竞争、分配或线性扫描。

## 验证依据

本说明使用以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录 20 个 Go/Rust 文件均已索引。
- RustCodeGraph `node --file pkg/util/sem/v2/sem.rs`：核对本文件 346 行完整实现及 29 个符号；`callees EnableBy` 确认其到 `Load`、`Store`、`overrideRestrictedVariable`、`buildSEMFromConfig` 的边；`callees buildSEMFromConfig` 确认其到 `buildSEMSqlValidateFunction` 的边。
- RustCodeGraph `node`：读取 `pkg/util/sem/v2/lib.rs`、`config.rs`、`sem.go`、`sem_test.rs`、`sem_test.go`、`restricted_hint.rs`、`testhelper.rs`、`pkg/util/sem/compat/sem.rs` 以及上述服务、会话、权限和规划器调用片段。
- RustCodeGraph `callers` 对 Go/Rust 同名及跨 crate 调用未返回有效边，因此用 `rg` 补核实际 Rust 调用点；这部分证据仅用于弥补索引调用边缺口。
- [`Cargo.toml`](Cargo.toml)：核对 crate 名、`lib.rs` 入口、直接依赖、开发依赖、无 feature 声明，以及 `go-package = "pkg/util/sem/v2"` 的移植映射。
- [`sem_test.rs`](sem_test.rs)：核对权限、schema/表、变量覆盖、文件启用、串行执行及资源清理的现有回归范围；未运行 Cargo，符合本纯文档任务约束。

交付结构以任务指定命令校验：目标文件存在，且恰好包含“文件定位”至“验证依据”的 11 个固定二级标题。人工复核范围包括：没有把未覆盖路径写成已验证行为，没有复制整段源码，也没有建议把测试嵌入生产源文件。
