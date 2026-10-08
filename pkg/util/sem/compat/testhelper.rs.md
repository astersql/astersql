# `pkg/util/sem/compat/testhelper.rs`

## 文件定位

本文件是 `astersql-util-sem-compat` crate 的测试辅助实现，位于 SEM（Security Enhanced Mode）v1/v2 兼容层中。`pkg/util/sem/compat/lib.rs` 无条件声明 `testhelper` 模块并通过 `pub use testhelper::*` 再导出，因此这里的 `V1`、`V2`、`SwitchToSEMForTest` 和 `compatibleSEMV2Config` 都可由依赖该 crate 的 Rust 测试直接访问；它不是 `#[cfg(test)]` 私有模块，但 API 语义明确面向测试。

crate 边界由 `pkg/util/sem/compat/Cargo.toml` 定义：本文件直接依赖 SEM v1 crate `astersql-util-sem`、SEM v2 crate `astersql-util-sem-v2`、解析器 MySQL 常量 crate `astersql-parser-mysql`（本地别名 `mysql`）以及 `tempfile`。其作用是为兼容层及上层 privilege、session、expression 等测试提供统一的版本切换入口，而不是实现 schema、表、变量、权限或 SQL 的受限判定；真实判定分别位于 `pkg/util/sem/sem.rs`、`pkg/util/sem/v2/sem.rs` 和同目录 `sem.rs` 的分发逻辑。

## 核心职责

1. 用 `V1 = "v1"`、`V2 = "v2"` 固定与 Go `pkg/util/sem/compat/testhelper.go` 相同的版本标识。
2. `SwitchToSEMForTest` 根据版本启用对应的进程级 SEM 实现，并返回必须由调用者执行的清理闭包。
3. v2 分支把 `compatibleSEMV2Config` 写入临时 JSON 文件，再交给 `astersql_util_sem_v2::EnableFromPathForTest` 走真实的文件解析、校验、启用和变量恢复路径。
4. `compatibleSEMV2Config` 保存与 Go 测试夹具对应的完整 v2 配置，覆盖受限数据库、表、状态变量、系统变量、权限以及 SQL 命令/规则，供兼容层和上层集成测试共同复用。

本文件刻意不抽象 SEM 查询接口，也不维护自己的启用状态。它只负责准备测试环境并委托给 v1/v2 实现，避免两套测试各自复制切换和配置装载流程。

## 主要符号

- `pub const V1: &str = "v1"`：`SwitchToSEMForTest` 的 v1 分支选择值，也是 privilege 和 expression 等跨 crate 测试的公开输入。
- `pub const V2: &str = "v2"`：v2 分支选择值；compat 单元/集成测试和 session 运行时测试使用它装载 JSON 配置。
- `pub fn SwitchToSEMForTest(version: &str) -> Box<dyn Fn()>`：唯一行为入口。返回闭包不携带 `Result`，初始化失败或版本未知均通过 panic 使测试立即失败。
- `pub static compatibleSEMV2Config: &str`：静态 JSON 字符串。主要字段为 `version = 1.0`、最低 `tidb_version = v9.0.0`、`metrics_schema`、系统表清单、`tidb_gc_leader_desc`、系统变量、`FILE`/`BACKUP_ADMIN` 权限以及受限 SQL 规则和命令。

文件没有自定义类型、trait 或 `impl`，也没有条件编译项。`#![allow(non_snake_case, non_upper_case_globals)]` 保留 Go API 命名，便于逐项核对迁移语义。

## 执行流程

调用 `SwitchToSEMForTest(version)` 后按以下路径执行：

1. 当 `version == V1` 时，调用 `semv1::Enable()` 打开 v1 的原子开关并设置增强安全相关系统变量，然后返回装箱后的 `semv1::Disable`。调用者执行该闭包后，v1 开关关闭并恢复相关变量。
2. 当 `version == V2` 时，先在 `unsafe` 块中把 `mysql::r#const::TiDBReleaseVersion` 设置为 `"v9.0.0"`，使后续配置校验满足 `compatibleSEMV2Config.tidb_version` 的最低版本要求。
3. 用 `tempfile::Builder` 创建带 `semv2_config_` 前缀和 `.json` 后缀的临时文件，写入 `compatibleSEMV2Config` 并显式 `flush`，确保按路径读取时已可见完整内容。
4. 把临时路径转换为拥有所有权的字符串，调用 `semv2::EnableFromPathForTest(&config_path)`。该下游函数先解析配置、备份会被固定值覆盖的系统变量，再调用 v2 `Enable` 校验并安装全局 `SemImpl`。
5. v2 启用成功后立即 `drop(file)`；`NamedTempFile` 删除临时文件不影响已经解析并安装的内存配置。函数把下游清理闭包原样返回，该闭包会调用 v2 `Disable` 并恢复先前备份的系统变量值。
6. 其他版本字符串进入兜底分支并以 `unknown SEM version` panic，不返回清理函数。

典型调用方会把返回闭包装入 RAII 守卫，或在断言结束后显式调用；`pkg/util/sem/compat/compat_test.rs` 和 `sem_integration_test.rs` 使用 `Drop` 守卫，`migration_aster_unit_test.rs` 同时验证显式清理后的关闭状态。

## 数据与状态

`V1`、`V2` 和 `compatibleSEMV2Config` 都是只读、进程静态数据。本文件自身不保存可变状态，但会修改下游的进程级状态：v1 使用 `pkg/util/sem/sem.rs::semEnabled` 原子量，v2 使用 `pkg/util/sem/v2/sem.rs::globalSem` 内的 `RwLock<Option<Arc<SemImpl>>>`，两者还会更新全局系统变量。

v2 配置是测试契约而非仅供展示的样例。其字段驱动实际的可见性和权限结果，例如整库隐藏 `metrics_schema`、隐藏列出的系统表、把 `BACKUP_ADMIN` 设为受限权限，并限制 `BACKUP`、`RESTORE`、`ALTER RESOURCE GROUP`。`compat_test.rs`、`sem_integration_test.rs`、`privileges_test.rs` 和 `pkg/session/runtime_test/session.rs` 会解析或启用同一字符串，因此更改字段会同时改变多个测试层面的预期。

`mysql::r#const::TiDBReleaseVersion` 是可变进程级静态值。当前 helper 在 v2 分支把它改为 `v9.0.0`，清理闭包并不恢复原值；这是与 Go helper 一致的测试环境副作用，调用者不能假定切换结束后发布版本回到调用前状态。

## 依赖与调用关系

上游方面，RustCodeGraph 对本文件报告的直接使用文件包括：

- `pkg/util/sem/compat/compat_test.rs`：解析配置、启用 v2，验证 schema、表、状态变量、系统变量和权限规则。
- `pkg/util/sem/compat/migration_aster_unit_test.rs`：覆盖关闭状态、v1/v2 分发、清理和未知版本 panic。
- `pkg/util/sem/compat/sem_integration_test.rs`：启用 v2 后验证受限 SQL 命令及 `import_with_external_id` 规则语义。
- `pkg/privilege/privileges/privileges_test.rs`：在 v1/v2 下验证用户、动态权限和对象可见性，并解析 JSON 注册 Go 初始化阶段通常提供的系统变量。
- `pkg/session/runtime_test/session.rs`：解析同一配置并在真实 session 路径启用 v2，验证认证动态权限与 SQL 执行链。

仓库文本引用还显示 `pkg/expression/integration_test/integration_part2_aster_unit_test.rs` 使用 v1，`pkg/sessionctx/sessionstates/session_states_test.rs` 使用版本切换；对应 Cargo manifest 通过路径依赖接入 `astersql-util-sem-compat`。

下游方面，v1 分支调用 `astersql_util_sem::Enable/Disable`；v2 分支使用 `std::io::Write`、`tempfile::Builder`、`mysql::r#const::TiDBReleaseVersion` 和 `astersql_util_sem_v2::EnableFromPathForTest`。RustCodeGraph 的 `callees` 结果准确识别了配置静态量引用，但把通用 `flush`/`drop` 名称解析到其他同名定义且未完整识别跨 crate 调用，因此跨 crate 边以上述源码与 Cargo 路径依赖为准。

## 错误处理与边界

该 API 为测试失败优先设计：临时文件创建、写入、刷新或 v2 启用失败均通过带阶段信息的 `panic!` 终止当前测试；未知版本也 panic。它没有向调用方暴露可恢复错误，生产代码不应使用它承担动态配置切换。

v1 `Enable` 依赖 `Hostname` 和 `TiDBEnableEnhancedSecurity` 系统变量已注册，否则其内部 `expect` 会 panic。v2 配置校验还要求 JSON 中列出的系统变量已存在且 scope/value 约束满足。独立 Rust 测试因此先注册或重置这些变量；helper 本身不补注册，调用方必须提供与完整服务初始化等价的前置环境。

v2 下游要求 SEM 尚未启用；重复启用会触发其内部断言或失败。函数也不会自动关闭另一版本，因此安全的测试应先重置 v1/v2，再选择一个版本。返回闭包若被遗忘，进程级 SEM 状态会泄漏到后续测试。闭包类型为 `Fn`，可以被多次调用，但调用约定仍应视为一次性清理；重复调用不构成推荐或被测试保证的公共语义。

临时路径通过 `to_string_lossy` 转换。通常的系统临时目录可无损表示；若路径包含非 UTF-8 字节，传入下游的是替换后的字符串，可能导致文件读取失败并 panic。配置成功读入后文件立即删除，因此清理阶段不再依赖该路径。

## 并发与资源生命周期

临时文件的生命周期完全包含在 v2 分支内：创建、写入、刷新、解析启用，然后由 `drop(file)` 关闭并删除。下游清理闭包只持有被覆盖系统变量的备份，不持有临时文件。

SEM 状态、系统变量和 `TiDBReleaseVersion` 都是进程级共享状态。虽然 v1 开关是原子量、v2 指针由 `RwLock` 保护，但一次完整的“准备变量—启用—断言—清理”不是跨这些资源的单一事务；并行测试仍可能互相覆盖或观察中间状态。相关 compat 测试使用 `serial_test::serial`，这说明调用方应串行化涉及本 helper 的测试，并通过 RAII 守卫保证 panic 展开时也执行清理。

返回的 `Box<dyn Fn()>` 没有 `Send`/`Sync` 约束，本文件不承诺跨线程转移清理责任。即使外层自行同步，也必须考虑 `TiDBReleaseVersion` 不恢复这一长期副作用，以及 v1/v2 清理只处理各自实现的状态。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/sem/compat/testhelper.go`。两端保留相同版本字符串、相同 v1 `Enable`/`Disable` 配对、相同 v2 发布版本赋值、相同 JSON 内容和相同未知版本失败语义。

语言适配差异如下：

- Go 签名接收 `*testing.T`，用 `t.Fatalf`/`require.NoError` 报错；Rust 不依赖测试句柄，改为返回 `Box<dyn Fn()>` 并在初始化错误时 panic。
- Go 在 `t.TempDir()` 下 `os.CreateTemp`，用 defer 关闭和删除；Rust 使用 `tempfile` 的 RAII，在 v2 启用完成后显式丢弃句柄并自动删除。
- Rust 在调用按路径读取前显式 `flush`；Go 的 `WriteString` 后直接读取同一文件路径。两者都通过真实文件解析路径启用 v2。
- Go 的清理类型是 `func()`；Rust 下游清理闭包捕获系统变量备份，因此以 trait object 装箱。
- 两端都不会恢复 `TiDBReleaseVersion`。Rust 修改可变静态量需要显式 `unsafe`，把 Go 中隐含的进程级可变性暴露为需要审查的边界。

`pkg/util/sem/compat/compat_test.go` 与 `sem_integration_test.go` 提供原始测试意图；Rust 的 `compat_test.rs`、`sem_integration_test.rs` 和 `migration_aster_unit_test.rs` 验证迁移后的辅助函数与配置行为。Rust 集成测试对完整 Go session/privilege 场景作了依赖解耦，真实 session 链路另由 `pkg/session/runtime_test/session.rs` 覆盖。

## 扩展指南

新增 SEM 版本时，应同时更新版本常量、`SwitchToSEMForTest` 的匹配分支、对应真实实现依赖和 Go 对照文件；不得只让未知版本不再 panic而缺少实际启用/清理逻辑。新增分支必须明确：初始化前置条件、失败传播方式、进程状态的恢复范围以及清理闭包在 panic 路径上的执行策略。

修改 v2 JSON 时，应先在 Go `testhelper.go` 确认权威夹具变化，再逐字段同步。至少要同步检查 `compat_test.rs`、`sem_integration_test.rs`、`migration_aster_unit_test.rs`、`pkg/privilege/privileges/privileges_test.rs` 和 `pkg/session/runtime_test/session.rs`；若新增 `restricted_variables`，独立 Rust harness 还需注册相应系统变量并选择正确 scope。配置变化可能扩大隐藏对象、只读变量、受限权限或 SQL 拦截范围，兼容风险高于普通测试数据修改。

若要改变临时文件或错误处理，应保持“完整写入并可见后才解析”“解析成功后可删除文件”“启用失败不返回伪清理闭包”这些不变量。若要恢复 `TiDBReleaseVersion`，需要同时评估下游清理闭包捕获原值、并发写入和 Go 对齐，不能仅在本函数末尾恢复，因为配置启用后的测试阶段仍依赖该版本环境。

测试逻辑应继续放在独立文件，不嵌入 `testhelper.rs`。优先扩展同目录 `migration_aster_unit_test.rs` 验证切换不变量，扩展 `compat_test.rs` 验证可见性/权限配置，扩展 `sem_integration_test.rs` 验证 SQL 规则；需要真实 privilege/session 链时在各自 crate 的独立测试中覆盖。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/util/sem/compat` 确认本 crate 的 Rust/Go 源与测试集合。
- RustCodeGraph `node --file pkg/util/sem/compat/testhelper.rs --offset 1 --limit 400`：读取完整 157 行源码，报告 6 个索引符号以及 5 个直接使用文件。
- RustCodeGraph `query SwitchToSEMForTest`、`query compatibleSEMV2Config`、`node compatibleSEMV2Config`、`callees SwitchToSEMForTest`：确认两个公开行为/数据入口及函数到配置静态量的引用；`explore` 与限定名 `callers` 未返回可用边，因此调用方用索引的文件使用关系和精确文本引用交叉核验。
- crate 与模块证据：`pkg/util/sem/compat/Cargo.toml`、`pkg/util/sem/compat/lib.rs`。
- 下游实现证据：`pkg/util/sem/sem.rs`、`pkg/util/sem/v2/sem.rs`、`pkg/util/sem/v2/testhelper.rs`。
- Go 对照证据：`pkg/util/sem/compat/testhelper.go`、`pkg/util/sem/compat/compat_test.go`、`pkg/util/sem/compat/sem_integration_test.go`。
- Rust 测试证据：`pkg/util/sem/compat/compat_test.rs`、`pkg/util/sem/compat/sem_integration_test.rs`、`pkg/util/sem/compat/migration_aster_unit_test.rs`、`pkg/privilege/privileges/privileges_test.rs`、`pkg/session/runtime_test/session.rs`，以及文本引用发现的 expression/sessionstates 独立测试。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前用任务指定命令确认本文恰好具有 11 个固定二级章节，并人工复核文档中的符号、调用边、边界与扩展建议均可回溯到上述文件。
