# `pkg/util/sem/v2/testhelper.rs`

## 文件定位

本文件是 `astersql-util-sem-v2` crate 的测试辅助层，源码入口为 [`testhelper.rs`](testhelper.rs)。[`lib.rs`](lib.rs) 通过 `mod testhelper` 加载它，并用 `pub use testhelper::*` 将三个函数作为 crate 公共 API 导出。它不实现新的 SEM 策略，而是围绕 [`sem.rs`](sem.rs) 中的正式启停流程和进程级 `globalSem` 提供测试装配、清理及临时权限调整能力。

虽然文件名为 `testhelper.rs`，该模块并没有 `#[cfg(test)]` 限制，三个 API 会进入普通库构建。这样 [`../compat/testhelper.rs`](../compat/testhelper.rs) 等兼容测试基础设施可以通过 `astersql_util_sem_v2` 调用它。当前直接 Rust 使用点是兼容层的 `SwitchToSEMForTest`，以及 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 中的迁移回归测试。

## 核心职责

1. `EnableFromPathForTest` 用配置文件启用 SEM v2，并把启用过程会覆盖的系统变量默认值快照到清理闭包中，使测试能够显式恢复进程级状态。
2. `AddRestrictedPrivilegesForTest` 与 `RemoveRestrictedPrivilegesForTest` 临时修改当前 SEM 实例的受限权限集合，便于测试未写入配置文件的权限场景。
3. 维持 Go `pkg/util/sem/v2/testhelper.go` 的测试辅助语义：配置解析或启用失败时返回错误；权限名统一转成大写；SEM 未启用时权限增删静默返回。

这些函数都修改进程级共享状态，因此它们适合有明确清理顺序、必要时串行化的测试，不是请求级或会话级 API。

## 主要符号

- `pub fn EnableFromPathForTest(configPath: &str) -> Result<Box<dyn Fn()>, String>`：先调用 `parseSEMConfigFromFile` 读取 `Config`，遍历 `Config::RestrictedVariables`；只对 `Value` 非空且能由 `variable::GetSysVar` 找到的变量保存当前 `SysVar::Value`。随后调用正式的 `Enable`。成功结果中的闭包先执行 `Disable`，再逐项调用 `variable::SetSysVar` 恢复快照。
- `pub fn AddRestrictedPrivilegesForTest(privilege: &str)`：若 `globalSem.Load()` 返回当前 `Arc<SemImpl>`，取得 `SemImpl::restrictedPrivileges` 的写锁，并插入 `privilege.to_uppercase()`。
- `pub fn RemoveRestrictedPrivilegesForTest(privilege: &str)`：同样加载当前 SEM、取得写锁，再删除大写后的权限名。不存在该权限时删除操作无效果。
- 文件没有自定义常量、类型、trait、`impl` 或条件编译项；内部唯一显式数据结构是 `EnableFromPathForTest` 中用于快照的 `HashMap<String, String>`。

三个函数沿用 Go 命名，所以分别带有 `#[allow(non_snake_case)]`。`globalSem` 与配置解析函数是 crate 内可见符号，测试辅助 API 则是公开符号。

## 执行流程

`EnableFromPathForTest` 的正常流程如下：

1. 从 `configPath` 打开并反序列化一个 SEM v2 JSON 配置；打开或解码失败直接返回 `Err(String)`，不会启用 SEM。
2. 检查配置中的每个 `RestrictedVariables` 项。只有非空 `Value` 会在正式启用时强制覆盖系统变量，因此只为这些项建立恢复快照；未注册变量此时跳过，但稍后的 `Enable` 校验会拒绝无效受限变量。
3. 调用 `Enable(configPath)`。该函数会再次解析文件，再由 `EnableBy` 校验版本、系统变量和 SQL 规则，构建 `SemImpl`，覆盖受限变量，将实例写入 `globalSem`，并把增强安全开关设为 `CONFIG`。
4. 返回捕获快照所有权的 `Box<dyn Fn()>`。调用清理闭包时，`Disable` 先清空 `globalSem` 并把增强安全开关设为 `Off`，随后快照值通过 `SetSysVar` 写回。闭包实现 `Fn`，类型上允许被重复调用；正常测试应只清理一次，避免重复清理掩盖其他状态变更。

权限辅助的流程更短：每次调用只加载一次当前 `Arc<SemImpl>`；没有实例便返回，有实例便在写锁内按大写键插入或删除。之后 `IsRestrictedPrivilege` 会在同一集合的读锁下观察结果。

## 数据与状态

- `variableDefValue: HashMap<String, String>` 是启用前的局部快照，键来自配置的变量名，值来自当时系统变量注册表中的 `SysVar::Value`。它被移动进清理闭包，生命周期延续到闭包销毁。
- `globalSem` 定义在 [`sem.rs`](sem.rs)，内部以 `RwLock<Option<Arc<SemImpl>>>` 保存当前进程级 SEM 实例。`Enable` 写入实例，`Disable` 清空实例。
- `SemImpl::restrictedPrivileges` 是 `RwLock<HashSet<String>>`。配置构建与测试增删都把权限规范化为大写；查询 API `IsRestrictedPrivilege` 要求调用方传入大写名称。
- 系统变量注册表属于 `astersql-sessionctx-variable` 依赖。`GetSysVar` 返回已注册项的 `Arc<SysVar>` 快照，`SetSysVar` 克隆条目、更新 `Value` 并重新注册；本文件不持有注册表锁跨越 `Enable` 或清理调用。

清理闭包只保存“配置中有非空强制值且启用前已经注册”的变量。它不会保存整个 SEM 配置、整个系统变量注册表，也不会撤销闭包创建后其他代码对非快照变量的修改。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：本 crate 名为 `astersql-util-sem-v2`，本文件直接使用标准库 `HashMap`，并经 crate 内再导出调用 `parseSEMConfigFromFile`、`Enable`、`Disable`、`globalSem`；系统变量操作来自普通依赖 `variable = astersql-sessionctx-variable`。

已验证的上游关系：

- [`../compat/testhelper.rs`](../compat/testhelper.rs) 的 `SwitchToSEMForTest(V2)` 创建临时 JSON 文件，调用 `semv2::EnableFromPathForTest`，然后把其清理闭包返回给调用测试。
- [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 直接调用全部三个 API：权限测试验证 `RELOAD` 的增删，`migration_enable_from_path_restores_variables_on_cleanup` 验证启用、关闭和系统变量恢复。
- Go 侧 `pkg/privilege/privileges/privileges_test.go` 使用 Go 对应的权限增删函数覆盖授权行为；它是移植语义的测试证据，不是 Rust 调用边。

主要下游关系是 `EnableFromPathForTest -> parseSEMConfigFromFile -> Enable -> EnableBy`，以及清理闭包 `-> Disable`、`-> variable::SetSysVar`。权限辅助关系为 `Add/RemoveRestrictedPrivilegesForTest -> globalSem.Load -> RwLock<HashSet<_>>::write -> insert/remove`。

## 错误处理与边界

- `EnableFromPathForTest` 保留配置读取、JSON 解码及正式 `Enable` 的字符串错误。只有全部成功才返回清理闭包；调用者不能在错误结果上执行清理。
- `Enable`/`EnableBy` 要求 SEM 尚未启用，并通过 `intest::Assert` 表达该前置条件；测试在再次启用前必须先清理。
- 快照阶段忽略未注册变量，但 `validateSEMConfig` 会拒绝配置中的任何未知受限变量。非空强制值还必须用于 `ScopeNone` 变量。
- 清理阶段显式忽略 `variable::SetSysVar` 的错误；`Disable` 内设置增强安全开关的错误也被忽略。这与当前启停实现一致，但意味着闭包没有途径向调用者报告恢复失败。
- 权限增删在 SEM 未启用时静默无操作。写锁中毒会以 `expect("SEM privilege lock poisoned")` 触发 panic；函数没有可恢复错误返回值。
- 权限名使用 Unicode `to_uppercase` 规范化。调用方应使用与 SEM 权限模型一致的 ASCII 权限名，并在查询 `IsRestrictedPrivilege` 时传入大写形式。

## 并发与资源生命周期

SEM 及系统变量注册表都是进程级共享资源。`globalSem.Load()` 克隆一个 `Arc<SemImpl>`，所以一次权限增删在取得实例后，即使其他线程随后 `Disable`，该实例在本次操作结束前仍存活；但修改已经从全局指针移除的旧实例不会影响之后重新启用的新实例。`restrictedPrivileges` 自身有读写锁，可避免集合内部的数据竞争，不过源码契约仍明确要求这些测试辅助操作不要与其他 SEM 变更并发。

`EnableFromPathForTest` 的返回类型是 `Box<dyn Fn()>`，没有声明 `Send` 或 `Sync`；应在拥有该测试状态的线程中保存和调用。它不自动采用 RAII：丢弃闭包不会执行 `Disable`，调用者必须显式调用，或像 Go 版本用 `defer` 那样安排清理。兼容层在配置已被同步读入后可以删除临时文件，因为闭包只持有变量快照，不再访问配置路径。

相关 Rust 回归使用 `#[serial]` 隔离 SEM 和系统变量注册表的共享状态。新增涉及启停、变量覆盖或全局权限变更的测试也应采用同类串行约束，并在结束时恢复状态。

## 与 Go 版本的对应关系

直接对照文件是 [`testhelper.go`](testhelper.go)。Rust 保留了 Go 的三个函数名、配置路径启用步骤、只快照非空强制值、跳过快照阶段不存在的变量、清理时先关闭 SEM 再恢复变量、权限大写化，以及未启用时静默返回的行为。

实现层差异主要来自语言和并发模型：

- Go 返回 `(func(), error)`；Rust 返回 `Result<Box<dyn Fn()>, String>`，用所有权把快照移动到闭包。
- Go 的权限集合直接写 map，并用注释禁止与多 goroutine SEM 操作并发；Rust 将集合放在 `RwLock<HashSet<String>>` 中，读写集合受锁保护，但仍保留不得与其他 SEM 变更竞态的契约。
- Go 在增删函数中先判断一次、再第二次 `globalSem.Load()` 取实例；Rust 用 `let Some(sem) = globalSem.Load()` 固定同一个 `Arc`，避免两次加载之间切换实例。
- 两边都忽略清理时设置系统变量的返回值。Rust 的 `SetSysVar` 明确返回 `Result`，本文件用 `let _ =` 丢弃它。

## 扩展指南

- 若增加新的“启用时覆盖、清理时恢复”的状态，应先确认正式覆盖发生在哪个 `EnableBy` 阶段，再在 `EnableFromPathForTest` 中于覆盖之前快照，并在 `Disable` 之后按依赖顺序恢复；不要无差别复制整个全局注册表。
- 若清理失败必须可观察，需要调整闭包签名及所有上游调用者，而不能仅在闭包中新增可能失败的操作。重点同步 [`../compat/testhelper.rs`](../compat/testhelper.rs) 和迁移测试。
- 若扩展临时策略修改 API，应复用 `globalSem.Load()` 得到的单个 `Arc`，遵循目标字段的锁和大小写规范，并明确 SEM 未启用时是无操作还是错误。不要绕过 `SemImpl` 的同步原语。
- 测试应放在独立的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 或其他独立 `*_test.rs` 文件，不要嵌入本生产源文件。至少覆盖成功变更、清理恢复、未启用分支、规范化规则和错误路径。
- 修改移植行为时同步核对 [`testhelper.go`](testhelper.go)；若有意产生差异，应在测试和本文中记录原因。兼容风险集中在全局状态残留、清理顺序、重复清理以及并发启停；性能不是主要风险，集合操作与变量快照规模均受测试配置大小限制。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含 `pkg/util/sem/v2/testhelper.rs`；`files --filter pkg/util/sem/v2` 确认 Rust/Go 对照文件及独立测试；`node --file pkg/util/sem/v2/testhelper.rs --offset 1 --limit 240` 读取到完整 77 行，并报告该文件被 `pkg/util/sem/compat/testhelper.rs` 与 `pkg/util/sem/v2/migration_aster_unit_test.rs` 使用。符号查询确认 Rust 与 Go 各有一个 `EnableFromPathForTest`。精确 `callers/callees` 命令在本地索引上未在 30 秒内返回，因此调用边又通过直接引用搜索与源码阅读核验。
- 源码：[`testhelper.rs`](testhelper.rs)、[`sem.rs`](sem.rs)、[`config.rs`](config.rs)、[`lib.rs`](lib.rs) 和 `pkg/sessionctx/variable/variable.rs`，分别用于核对辅助流程、全局状态与锁、配置解析/校验、公开导出和系统变量注册表语义。
- crate 配置：[`Cargo.toml`](Cargo.toml)，用于核对 crate 名、库入口以及 `variable`、`serial_test`、`tempfile` 等普通/测试依赖边界。
- Go 对照：[`testhelper.go`](testhelper.go) 与 `pkg/util/sem/compat/testhelper.go`；Go 权限场景参考 `pkg/privilege/privileges/privileges_test.go`。
- Rust 测试：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 中的权限增删断言和 `migration_enable_from_path_restores_variables_on_cleanup`；兼容调用参考 [`../compat/testhelper.rs`](../compat/testhelper.rs)。同目录不存在专门的 `testhelper_test.rs`，相关回归集中在迁移测试文件。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时使用任务规定的 11 个固定二级标题结构检查，并人工复核本文只陈述上述源码与测试能够支持的行为。
