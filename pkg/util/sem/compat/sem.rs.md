# `pkg/util/sem/compat/sem.rs`

## 文件定位

该文件实现 `astersql-util-sem-compat` crate 的 SEM（Security Enhanced Mode，安全增强模式）查询门面。`pkg/util/sem/compat/lib.rs` 将其声明为私有 `sem` 模块后通过 `pub use sem::*` 重导出，因此调用方使用 crate 根上的公开函数，不直接依赖文件内部模块名。

`pkg/util/sem/compat/Cargo.toml` 将 crate 映射到 Go 包 `pkg/util/sem/compat`，并依赖 SEM v1 crate `astersql-util-sem`、SEM v2 crate `astersql-util-sem-v2` 和内部断言 crate `astersql-util-intest`。这个文件不定义启停或配置 API；相关能力由两个下游 SEM 实现及同 crate 的 `testhelper.rs` 提供。

## 核心职责

- 用一组稳定 API 隔离 SEM v1 的硬编码规则与 SEM v2 的配置驱动规则。
- 每次查询前通过 `intest::Assert` 表达“v1 与 v2 不应同时启用”的开发期不变式。
- 仅向已启用的版本转发 schema、table、status variable、system variable 和 privilege 的可见性/受限性判断。
- 保持 Go `pkg/util/sem/compat/sem.go` 的公开名称、参数语义、短路顺序与默认 `false` 行为。

本文件不持有隐藏清单或配置，也不实现 SQL 权限检查。例如 Rust 主链中 `pkg/privilege/privileges/privileges.rs` 使用 `IsInvisibleSchema`/`IsInvisibleTable`，`pkg/privilege/privileges/cache.rs` 使用 `IsRestrictedPrivilege`，`pkg/planner/core/expression_rewriter.rs` 使用 `IsInvisibleSysVar`；兼容层只回答规则查询，由上层决定拒绝、过滤或需要动态权限。

## 主要符号

| 公开符号 | 语义与输入契约 |
| --- | --- |
| `IsEnabled() -> bool` | 断言版本互斥后，返回 `sem::IsEnabled() || semv2::IsEnabled()`。 |
| `IsInvisibleSchema(dbName: &str) -> bool` | 判断 schema 是否应向无豁免调用方隐藏；具体的大小写规则由启用的版本实现。 |
| `IsInvisibleTable(dbLowerName: &str, tblLowerName: &str) -> bool` | 判断库表是否隐藏；参数名明确承接 Go 约定，库名和表名应已小写化。 |
| `IsInvisibleStatusVar(varName: &str) -> bool` | 查询 `SHOW STATUS` 类状态变量是否不可见。 |
| `IsInvisibleSysVar(varName: &str) -> bool` | 查询系统变量是否不可见；实际生产调用如 `expression_rewriter.rs` 传入 `system_variable.Name`。 |
| `IsRestrictedPrivilege(privilege: &str) -> bool` | 查询动态权限是否受限。在版本互斥检查外，还用 `privilege.to_uppercase() == privilege` 表达入参必须大写的契约。 |

文件没有常量、类型、trait、`impl` 或条件编译项；六个函数均为公开的同步纯查询门面。`#![allow(non_snake_case)]` 是为了保留 Go 风格的导出名称。

## 执行流程

1. 入口首先分别读取 `astersql_util_sem::IsEnabled()` 与 `astersql_util_sem_v2::IsEnabled()`，并把“不得同时为 true”交给 `intest::Assert`。
2. `IsEnabled` 直接对两个开关做逻辑或。
3. 其余五个函数按 v1 再 v2 的顺序短路判断：只有某版本已启用时，才调用该版本的同名规则函数。
4. 启用版本返回 `true` 时立即返回；否则继续到下一版本，最终默认返回 `false`。
5. `IsRestrictedPrivilege` 在路由前多一次大写契约断言；它不会自动规范化输入。

这一顺序意味着，即使内部断言在当前构建中不激活，同时启用两版本时也会按 v1 优先、v2 次之的分支执行，而不是合并两套结果。`migration_aster_unit_test.rs::compatibility_invariants_follow_default_build_and_reject_unknown_versions` 对默认构建下断言未激活时的这一边界有直接记录。

## 数据与状态

兼容层本身无可变静态数据、缓存或配置对象，参数都以借用 `&str` 传入，返回值都是 `bool`。

真实状态在下游：SEM v1 的 `pkg/util/sem/sem.rs::IsEnabled` 以 `SeqCst` 读取原子开关，v1 规则为硬编码集合；SEM v2 的 `pkg/util/sem/v2/sem.rs` 从 `globalSem` 中在 `RwLock` 读锁下克隆 `Arc<SemImpl>`，然后查询配置构建的集合。因此每次兼容查询都使用下游当前快照，没有跨调用保留状态。

## 依赖与调用关系

下游边界为：

- `intest::Assert` 接收版本互斥与权限名大写条件。
- `astersql_util_sem` 提供 v1 开关和五类规则查询。
- `astersql_util_sem_v2` 提供 v2 开关和对应的配置查询。

上游生产路径的直接证据包括：

- `pkg/privilege/privileges/privileges.rs` 在列举库表时用 `IsEnabled`、`IsInvisibleTable` 和 `IsInvisibleSchema` 过滤 SEM 对象。
- `pkg/privilege/privileges/cache.rs` 用 `IsEnabled` 与 `IsRestrictedPrivilege` 决定某权限是否不能被普通权限语义替代。
- `pkg/planner/core/expression_rewriter.rs` 在改写系统变量表达式时用 `IsInvisibleSysVar`。
- `pkg/session/runtime.rs`、`pkg/session/runtime/control.rs`、`pkg/session/runtime/dispatch.rs` 和 `pkg/session/runtime/system_session.rs` 使用 crate 根的 `IsEnabled` 把 SEM 状态接入 session 运行时分支。

RustCodeGraph 对目标文件报告的直接“used by”文件是 `compat_test.rs`、`migration_aster_unit_test.rs`、`sem_integration_test.rs` 和 `pkg/util/sem/v2/migration_aster_unit_test.rs`。因 crate 重导出与 Cargo 依赖别名，生产调用边另用 Cargo manifest 和上述源码引用核验，不将图的文件级列表误解为全部调用者。

## 错误处理与边界

六个 API 都不返回 `Result` 也不吞掉下游错误；它们只做布尔路由。两个 SEM 版本都未启用时，`IsEnabled` 和所有规则查询均返回 `false`，即“SEM 不介入”。

`intest::Assert` 是开发/内部不变式，不应被文档成稳定的运行时错误接口。已有迁移测试明确记录，仓库默认验证没有启用 Go `intest` build tag 对应的断言行为：小写权限名不 panic 而返回 `false`，v1/v2 临时共存时 `IsEnabled` 仍为 `true`。因此调用方仍必须遵守大写权限名和版本互斥的前置条件，不能依赖断言做生产输入校验。

schema 的大小写不敏感行为是下游 v1/v2 的事实；而 table 与 system-variable 参数有预先归一化契约。新调用方若传入未归一化字符串，可能得到 `false` 而不是显式错误。

## 并发与资源生命周期

本文件不创建锁、任务、线程、通道、事务、文件句柄或堆所有权对象，也没有异步边界。`&str` 的生命周期仅限于当次调用，不会被兼容层保存。

并发安全由下游开关实现承担：v1 使用 `SeqCst` 原子状态，v2 用 `RwLock<Option<Arc<SemImpl>>>` 发布/读取当前实例，其受限权限集合还有内部读锁。兼容层一次查询会多次读取 `IsEnabled`，并未获取能同时冻结 v1/v2 的联合快照；动态切换函数应继续被视为测试/启动生命周期操作，而非与普通查询并发使用的请求级开关。涉及全局 SEM 开关的 Rust 测试使用 `serial_test::serial` 和清理守卫，证明了隔离此全局状态的必要性。

## 与 Go 版本的对应关系

Rust `sem.rs` 的六个函数与 `pkg/util/sem/compat/sem.go` 逐一对应：相同的导出名、相同的 v1/v2 互斥断言、相同的 v1 优先 `if` / v2 `else if` 顺序，以及相同的默认 `false`。`IsRestrictedPrivilege` 用 Rust `to_uppercase()` 对应 Go `strings.ToUpper`。

`pkg/util/sem/compat/compat_test.go` 的 v2 用例已迁移到独立 `compat_test.rs`，覆盖大小写 schema、多个系统库的隐藏表、受限权限、status variable 与 system variable。`migration_aster_unit_test.rs` 额外覆盖关闭态、v1 路由、v2 路由、清理后恢复与默认构建断言边界。

Go `sem_integration_test.go` 会经过 mock store、认证与 `RESTRICTED_SQL_ADMIN` 执行完整 SQL 路径；Rust 同目录 `sem_integration_test.rs` 直接验证 v2 配置中的 `IsRestrictedSQL` 和 `ImportWithExternalIDRule`，完整 session/privilege 接线由其注释指向的 `pkg/session/runtime_test.rs` 相关用例承担。这些 SQL 规则不是本 `sem.rs` 六个 API 的直接职责。

## 扩展指南

- 增加新的跨版本查询时，在本文件增加与 Go 兼容包同名的公开门面，保留版本互斥断言、已启用判断和 v1 再 v2 的短路顺序。同时要在 v1/v2 crate 提供语义对等的下游 API，不应把规则数据复制到兼容层。
- 改动入参规范化或优先级会改变安全边界；必须与 `sem.go` 同步，并分别验证关闭、v1、v2 以及输入大小写。不要为了“宽容”调用方而在此静默转换 table/sysvar/privilege 名称。
- 测试逻辑必须保持在独立文件。优先扩展 `pkg/util/sem/compat/compat_test.rs` 的具体规则样例，扩展 `migration_aster_unit_test.rs` 的版本路由/不变式样例；涉及 SQL 主链时同步独立 session 集成测试。
- 涉及全局启停的测试要保留 `serial_test::serial` 和无论正常返回还是 panic 都能恢复状态的 RAII/清理机制，否则并行用例会相互污染。
- 性能风险主要来自热路径上重复的开关读取、v2 `RwLock` 读取和字符串规范化。优化时不能缓存跨越 SEM 启停周期的结果，也不能破坏 Go 对齐的短路语义。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件，目标 `pkg/util/sem/compat/sem.rs` 已索引为 137 行、7 个符号；`node --file` 核对了六个公开函数的完整实现与 4 个直接使用文件。
- 精确符号查询核对了 `sem.rs::IsEnabled`、`IsInvisibleSchema`、`IsInvisibleTable`、`IsInvisibleStatusVar`、`IsInvisibleSysVar` 和 `IsRestrictedPrivilege` 以及 v1/v2/Go 同名对应项。精确 `callers` 命令在本次环境中长时间无输出后中止，因此上游边由 Cargo 别名和 `rg` 的 Rust 生产引用交叉核对。
- 已读取：`pkg/util/sem/compat/sem.rs`、`Cargo.toml`、`lib.rs`、`sem.go`、`compat_test.rs`、`migration_aster_unit_test.rs`、`sem_integration_test.rs`、`compat_test.go`、`sem_integration_test.go`；直接下游状态与规则还用 RustCodeGraph 核对了 `pkg/util/sem/sem.rs` 和 `pkg/util/sem/v2/sem.rs`。
- 测试证据：`compat_test.rs` 验证 v2 的各查询族；`migration_aster_unit_test.rs` 验证关闭、v1、v2 和默认断言边界；`sem_integration_test.rs` 及其 Go 对照验证相关 SEM v2 SQL 配置的上层集成语义。
- 本任务仅产生文档，按计划不运行 Cargo。交付前使用任务指定的 `test -f` 与固定标题计数命令验证文件存在且恰有 11 个规定二级章节。
