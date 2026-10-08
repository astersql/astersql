# `pkg/session/global_init.rs`

## 文件定位

本文件属于 `astersql-session` crate；crate 根 `pkg/session/lib.rs` 通过 `pub mod global_init` 暴露该模块，Cargo 入口由 `pkg/session/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定。它描述的是完整 Domain 启动前，从持久化系统表读取进程级时区与排序规则开关的初始化流程。

当前 Rust 代码的定位需要与 Go 生产实现区分：`pkg/session/global_init.rs::initGlobalVarFromSystemDB` 是由 `GlobalInitRuntime` 驱动的可注入流程，仓库内没有该 trait 的生产实现，也没有生产代码调用此函数；精确检索只找到 `pkg/session/global_init_test.rs` 的测试实现与调用。实际应用启动链仍由 `pkg/session/session.go` 调用 `pkg/session/global_init.go::initGlobalVarFromSystemDB`。因此本文件已表达并验证核心控制流，但尚不能单独证明 Rust 启动链已经接通。

## 核心职责

- `systemDBFilter` 将临时 Domain 的持久化 schema 加载范围限制为 `mysql` 库；`SkipLoadDiff` 固定返回 `false`。这里的“系统库”严格对应 `metadef.IsSystemDB`，不是包括 `information_schema`、`sys` 等在内的广义系统相关库。
- `GlobalInitRuntime` 把建 Domain、建会话、读取系统表、设置进程全局量与清理 Domain 等真实副作用定义成运行时边界，避免流程函数伪造默认成功路径。
- `initGlobalVarFromSystemDB` 固定执行初始化顺序：创建隔离 Domain，创建临时会话，读取 `mysql.tidb` 中键 `system_tz`，设置系统时区，加载新排序规则开关，设置该开关，最后关闭 Domain。
- 无论创建会话、读取时区还是读取排序规则失败，只要 Domain 已经成功创建，就必须关闭 Domain；临时会话必须先析构，随后才关闭 Domain。

## 主要符号

- `SchemaDiff`、`InfoSchema`：零字段占位类型，只用于保留 `SchemaLoadFilter::SkipLoadDiff` 与 Go 接口相近的形状；它们不是当前 Rust InfoSchema/SchemaDiff 的真实数据模型。
- `DBInfo { pub name: String }`：供过滤器判断库名的最小元信息。Go 的 `model.DBInfo.Name.L` 已是小写形式，而 Rust 的 `is_system_database` 使用 ASCII 大小写无关比较，使直接传入 `mysql`、`MySQL` 等形式都能匹配。
- `SchemaLoadFilter`：声明 `SkipLoadDiff(&SchemaDiff, &InfoSchema)` 与 `SkipLoadSchema(&DBInfo)`。方法名刻意保留 Go 风格；文件级 `#![allow(non_snake_case)]` 为此放宽 lint。
- `systemDBFilter`：无状态、零大小过滤器，实现 `SchemaLoadFilter`，同时提供两个同名公开固有方法，把调用委托给 trait 实现。固有方法让调用方不必显式导入 trait；该类型名通过 `#[allow(non_camel_case_types)]` 保留 Go 对照命名。
- `is_system_database(&str) -> bool`：私有辅助函数，仅以 ASCII 大小写无关方式判断名称是否为 `mysql`。
- `GlobalInitRuntime`：用关联类型 `Error`、`Store`、`Domain`、`Session` 抽象运行时对象；七个方法覆盖创建、读取、设置和清理边界。`get_domain_for_global_var_init` 还接收 `SyncerOption` 切片，保证临时 Domain 的 server-info 行为可被调用方验证。
- `initGlobalVarFromSystemDB<R: GlobalInitRuntime>(&mut R, &R::Store) -> Result<(), R::Error>`：本文件唯一的流程入口。所有失败均沿用运行时的 `R::Error`，不在此处改写错误。

## 执行流程

1. `initGlobalVarFromSystemDB` 调用 `runtime.get_domain_for_global_var_init`，传入存储、`systemDBFilter` 以及唯一选项 `SyncerOption::WithoutStatusEndpointClaim`。建 Domain 失败时 `?` 立即返回，此时没有 Domain 可关闭。
2. Domain 创建成功后，函数进入局部闭包。闭包首先调用 `create_session(store, &domain)`；会话借用 Domain 而不取得其所有权。
3. 调用 `table_value(&session, "tidb", "system_tz")` 读取持久化时区，然后立即调用 `set_system_timezone`。若读取失败，不执行时区设置及后续排序规则步骤。
4. 调用 `load_collation_parameter(&session)` 读取新排序规则开关，再调用 `set_new_collation_enabled_for_test` 写入进程级状态。若此读取失败，时区已经设置，函数不做回滚。
5. 闭包结束时临时 `session` 先离开作用域并析构；闭包的 `Result` 被保存为 `result`。
6. 无条件调用 `runtime.close_domain(domain)` 消费 Domain，然后返回之前保存的 `result`。因此清理不覆盖原始业务错误，且 Domain 关闭操作本身没有返回值可传播。

`pkg/session/global_init_test.rs::global_init_preserves_go_side_effect_and_cleanup_order` 用事件序列完整断言上述成功路径；`global_init_closes_domain_after_every_post_creation_error` 覆盖建会话、时区读取和排序规则读取三个错误点以及 Domain 创建失败的特殊分支。

## 数据与状态

文件自身不保存静态可变状态，也不缓存 Domain、会话或系统表值。跨调用状态全部存在于 `GlobalInitRuntime` 的实现中：`set_system_timezone(String)` 和 `set_new_collation_enabled_for_test(bool)` 表示两个进程级写操作，`Store`/`Domain`/`Session` 则由实现者定义具体表示。

初始化值的持久化来源固定为 `mysql.tidb`：`table_value` 的表参数是 `"tidb"`，键参数是 `"system_tz"`；排序规则值通过单独的 `load_collation_parameter` 读取。流程不校验时区字符串，也不解释排序规则的存储编码，这些语义属于运行时实现边界。

`systemDBFilter` 没有字段，可以按值复制；`SchemaDiff` 与 `InfoSchema` 同样不携带实际状态。`DBInfo` 仅携带库名，因此当前过滤行为无法依据库 ID、租户或其他元数据变化。

## 依赖与调用关系

- 模块装配：`pkg/session/lib.rs` 公开 `global_init`，并在 `cfg(test)` 下装入独立的 `global_init_test.rs`；测试没有与生产源码内嵌。
- 直接 Rust 依赖：流程使用 `astersql_domain_serverinfo::SyncerOption::WithoutStatusEndpointClaim`。`pkg/session/Cargo.toml` 将 `astersql-domain-serverinfo` 声明为路径依赖 `../domain/serverinfo`，没有为本模块设置专属 feature；crate 的 `nextgen` feature 也未在本文件中条件编译。
- 相邻 Rust 接线：`pkg/session/tidb.rs::domainMap::getDomainForGlobalVarInit` 能把字符串过滤器名和相同的 `WithoutStatusEndpointClaim` 选项传给 Domain 工厂，但它并未实现 `GlobalInitRuntime`，也未调用本文件入口。两段代码目前只能视为相邻迁移材料，不能视为已经连通的调用边。
- Rust 调用者：仓库精确检索只发现 `pkg/session/global_init_test.rs` 调用 `initGlobalVarFromSystemDB` 并为 `TestRuntime` 实现 trait；没有生产调用者或生产实现者。
- Go 上游：`pkg/session/session.go` 在 bootstrap/升级、DDL 表与 MDL 初始化之后调用 Go 入口；测试 failpoint `skipInitGlobalVarFromSystemDB` 可跳过它。starter 模式的后续升级发生在全局时区和排序规则初始化之后。
- Go 下游：`pkg/session/global_init.go` 通过 `domap.getDomainForGlobalVarInit`、`createSessionWithOpt`、`sess.getTableValue`、`loadCollationParameter`、`timeutil.SetSystemTZ` 和 `collate.SetNewCollationEnabledForTest` 完成实际副作用；`pkg/session/tidb.go` 将过滤器与 `serverinfo.WithoutStatusEndpointClaim()` 传入 Domain 创建路径。

RustCodeGraph 能索引本文件（报告 25 个符号、9 个使用文件），并识别 Rust/Go 两个同名入口；但对本文件关键符号执行 `callers`/`callees` 返回空结果。因此上述具体调用关系以精确仓库检索和所列源码为依据，没有把空图边解释为生产接线存在。

## 错误处理与边界

四个可失败边界依次是 Domain 创建、Session 创建、时区读取和排序规则读取，均用 `?` 原样传播 `R::Error`。Domain 创建失败时不会调用 `close_domain`；其余三个失败点都会先结束闭包、析构已创建的会话（若存在），再关闭 Domain，最后返回原错误。

进程级写操作和 `close_domain` 的 trait 签名均返回 `()`，所以本函数无法观察或报告它们的失败。尤其是时区成功写入而排序规则读取失败时，函数会返回错误但保留已发生的时区副作用；当前没有事务性回滚语义。运行时实现不得假设本函数会替它校验时区内容、撤销部分更新或恢复先前全局值。

过滤器边界同样明确：`SkipLoadDiff` 固定不跳过 diff。Go 注释解释临时初始化不会真正启动 Domain，因此该回调预期不被调用；Rust 测试仅验证返回 `false`，没有证明生产加载器不会调用它。`SkipLoadSchema` 只允许 `mysql`，这与 `pkg/meta/metadef/db.go::IsSystemDB` 及 Rust 对照 `pkg/meta/metadef/db.rs::IsSystemDB` 的严格语义一致。

## 并发与资源生命周期

本文件没有线程、异步任务、锁或通道，也没有为 `GlobalInitRuntime` 添加 `Send`/`Sync` 约束；入口持有 `&mut R`，从类型层面阻止同一个运行时对象在一次调用期间被另一个安全 Rust 借用同时修改。进程级 setter 的真实同步策略仍完全由实现者负责。

资源生命周期的关键不变量是 `Session drop -> Domain close`。局部闭包限制 `session` 的作用域，闭包返回后才把 `domain` 按值交给 `close_domain`；这避免会话在其所属 Domain 关闭后才析构。测试中的 `TestSession::drop` 记录 `drop_session`，成功路径断言它严格位于 `set_collation:true` 与 `close_domain` 之间。

临时 Domain 使用 `WithoutStatusEndpointClaim`，避免它占有对外服务进程的 status endpoint。该选项的具体判定位于 `pkg/domain/serverinfo/syncer.rs`。本入口本身没有保证 Domain 与正常服务 Domain 的缓存隔离；隔离创建与关闭必须由 `get_domain_for_global_var_init`/`close_domain` 的生产实现兑现，而当前 Rust 仓库尚无这样的实现者。

## 与 Go 版本的对应关系

Rust 主流程逐步对应 `pkg/session/global_init.go`：Go 的 `domap.getDomainForGlobalVarInit`、`createSessionWithOpt`、`sess.getTableValue(ctx, mysql.TiDBTable, tidbSystemTZ)`、`timeutil.SetSystemTZ`、`loadCollationParameter`、`collate.SetNewCollationEnabledForTest` 和 `defer dom.Close()`，分别映射为 `GlobalInitRuntime` 的同职责方法及闭包后的 `close_domain`。

过滤语义也保持一致：Go 将 `dbInfo.Name.L` 传给 `metadef.IsSystemDB`，而该函数只接受 `mysql.SystemDB`；Rust 的 `is_system_database` 同样只接受 `mysql`，但额外容忍 ASCII 大小写差异。`pkg/session/global_init_test.rs::system_db_filter_matches_go_metadef_is_system_db` 明确断言 `mysql`/`MySQL` 被保留，`information_schema`、`performance_schema`、`metrics_schema`、`sys`、`workload_schema` 与普通业务库被跳过。

Go 版本额外记录了必须使用独立 Domain 的原因：TableCommon 会捕获 `new_collate` 设置，若复用完整 Domain 的 schema cache 可能保留无效值；它还说明读取系统变量存在循环依赖，但 `mysql.tidb` 固定以 `utf8mb4_bin` 创建，因此在正确排序规则开关初始化前仍可安全解码。Rust 流程保留了独立 Domain 的接口和顺序，却没有把这些前提编码成类型或验证逻辑。

目前的主要迁移差异不是流程缩减，而是接线状态：Go 入口已在 `pkg/session/session.go` 的真实 bootstrap 链中运行，Rust 入口只有测试调用；Rust 的 `SchemaDiff`、`InfoSchema`、`DBInfo` 也是局部最小模型，而非复用真实 infoschema/model 类型。

## 扩展指南

- 接通 Rust 生产链时，应在真实会话/Domain 适配器中实现 `GlobalInitRuntime`，并从 Rust bootstrap 的对应阶段调用 `initGlobalVarFromSystemDB`；不要仅把 `pkg/session/tidb.rs::getDomainForGlobalVarInit` 当作已经完成的适配。新增生产接线应配套独立测试文件，优先扩展 `pkg/session/global_init_test.rs` 或相应 bootstrap/runtime 测试，而不要把测试写进 `global_init.rs`。
- 若新增另一个持久化全局量，应在 `initGlobalVarFromSystemDB` 中明确其相对时区与排序规则的顺序、失败后的部分副作用以及是否必须在 Session 析构前完成，并扩展事件序列测试和每个新错误点的 Domain 清理测试。
- 若改变允许加载的数据库范围，应先核对 Go `metadef.IsSystemDB` 的语义。不要用 `IsSystemRelatedDB` 或内存 schema 集合替代严格的 `mysql` 判定，除非 Go 行为也发生对应变更；同步更新过滤器测试的允许/拒绝矩阵。
- 若把占位 `DBInfo`、`SchemaDiff`、`InfoSchema` 替换为真实类型，应保持 trait 对加载器的语义兼容，并检查 Cargo 是否已经直接声明相应 crate 依赖。不要为了接线复制外部类型或扩大本文件职责。
- 若让 setter 或 `close_domain` 可失败，需要决定多个错误同时出现时的优先级，并增加“业务失败 + 清理失败”的回归测试；当前 `()` 返回值刻意只传播读取/创建错误。
- 性能风险主要来自误加载业务 schema 或意外复用完整 Domain；兼容性风险主要来自更改初始化顺序、`mysql.tidb` 的键名或 `new_collate` 捕获时机；正确性风险主要来自失败路径漏关 Domain、会话晚于 Domain 析构及并发写进程全局量。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/session/global_init.rs` 确认目标已索引；`node --file pkg/session/global_init.rs` 读取完整 151 行并报告 25 个符号、9 个使用文件；`query` 分别识别 `initGlobalVarFromSystemDB` 的 Rust/Go 定义、Rust `GlobalInitRuntime`、Rust/Go `systemDBFilter` 与私有 `is_system_database`。对入口、trait 和辅助函数运行 `callers`/`callees` 返回空集合，因此再用精确文本检索补证。
- 已读生产与装配文件：`pkg/session/global_init.rs`、`pkg/session/lib.rs`、`pkg/session/Cargo.toml`、`pkg/session/tidb.rs`、`pkg/session/global_init.go`、`pkg/session/tidb.go`、`pkg/session/session.go`、`pkg/meta/metadef/db.go`、`pkg/meta/metadef/db.rs`、`pkg/domain/serverinfo/syncer.rs`。`pkg/session` 目录不存在 `doc.go`，没有额外包级契约可读。
- 已读测试：`pkg/session/global_init_test.rs` 提供过滤范围、完整成功顺序以及全部四类错误分支的直接证据；`pkg/meta/metadef/db_test.go`/`db_test.rs` 佐证严格 `IsSystemDB` 只接受 `mysql`；`pkg/util/timeutil/time_zone_test.go` 仅作为 Go 时区 setter 的相邻行为证据，不等同于本入口测试。
- 生产接线检索：`rg` 对 `initGlobalVarFromSystemDB`、`GlobalInitRuntime`、`systemDBFilter` 及过滤方法的全仓 Rust 查询只发现本文件、独立测试和相邻字符串接线；Go 查询定位到 `pkg/session/session.go` 的 bootstrap 调用及 `pkg/session/tidb.go` 的 Domain 创建路径。
- 本任务为纯文档分析，按计划不运行 Cargo。仓库说明引用的 `.agents/skills/tidb-verify-profile` 在当前检出中不存在，无法加载额外 Ready profile；交付使用任务指定的固定 11 章节结构检查、路径/链接事实复核和 diff 自审，不据此声称 Rust 生产链已运行验证。
