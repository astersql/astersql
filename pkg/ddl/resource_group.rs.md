# `pkg/ddl/resource_group.rs`

## 文件定位

该文件位于 `astersql-ddl` crate 中，并由 `pkg/ddl/lib.rs` 以 `pub mod resource_group` 公开。它用一组独立的 Rust 数据类型表达资源组 DDL 的核心语义：资源组配置建模、选项解析与校验，以及创建、修改、删除时“元数据目录 + 外部 Resource Manager”之间的更新顺序。crate 边界由 `pkg/ddl/Cargo.toml` 的 `[package] name = "astersql-ddl"` 和 `[lib] path = "lib.rs"` 确认。

当前文件不是 Rust SQL 请求主链上的实际资源组执行器。全仓 Rust 搜索只发现 `pkg/ddl/resource_group_test.rs` 直接使用本文件的 `ResourceGroupCatalog` 与选项辅助函数；`pkg/session/runtime/dispatch.rs` 分发资源组 SQL 后，实际调用的是 `pkg/session/runtime/control.rs` 中维护 `RUNTIME_RESOURCE_GROUPS` 的 `execute_create_resource_group`、`execute_alter_resource_group`、`execute_drop_resource_group`。因此，本文件目前应理解为对 Go 资源组 DDL 局部语义的可测试模型，而不是已接入持久化 DDL job 的完整实现。

按 DDL 执行框架分类，Go 对照是 owner worker 执行的 job-based 元数据变更；但本 Rust 文件自身没有 job 持久化、owner/failover、schema sync 或 reorg/backfill。资源组不改变表数据，也没有多阶段表 schema state；这里只把组状态简化为 `None -> Public` 和 `Public -> 删除`。

## 核心职责

1. `ResourceGroupSettings`、`ResourceGroupInfo` 及相关枚举把 RU 速率、优先级、CPU/IO 限制、突发模式、Runaway 规则和 Background 配置建模为纯 Rust 值。
2. `build_resource_group` 与 `set_direct_*` 系列函数把已经归一化的选项写入设置，复刻 `pkg/ddl/resource_group.go` 中 `buildResourceGroup`、`SetDirectResourceGroupSettings` 及其子选项处理。
3. `parse_duration_ms` 接受 Go `time.ParseDuration` 风格的 `ns/us/µs/μs/ms/s/m/h` 组合与小数格式，并截断到毫秒。
4. `parse_background_job_types` 对逗号分隔的后台任务类型做去空白、转小写和白名单检查。
5. `ResourceGroupCatalog` 模拟 meta 目录、schema version 推进以及向 `ResourceGroupManager` 同步的顺序，用于验证失败时是否保留 Go 版本的部分副作用。

该文件不负责 AST 解析、权限检查、`IF EXISTS`/`IF NOT EXISTS` 提示、默认组禁止删除、DDL job 构造、真实 meta 存储、infosync 超时或集群 schema 同步；这些能力在 Go 中分别位于 `pkg/ddl/executor.go`、`pkg/ddl/resource_group.go` 与 job worker 链路，在当前 Rust 运行时则另有简化实现。

## 主要符号

- `DEFAULT_RESOURCE_GROUP_NAME`：值为 `"default"`，用于限制 Background 设置只能修改默认资源组。
- `UNLIMITED_RU_RATE`：`i32::MAX as u64`，与 Go 的 `unlimitedRURate = uint64(math.MaxInt32)` 对齐。
- `GroupState::{None, Public}`：本文件仅需的资源组可见状态；没有表 DDL 那样的 delete-only/write-only/reorg 状态机。
- `Burstable::{Disabled, Unlimited, Moderated}`：分别映射到 `burst_limit` 的 `0`、`-1`、`-2`。当 RU 选项自身声明 Unlimited 时，`ru_rate` 改写为 `UNLIMITED_RU_RATE`。
- `RunawaySettings`：保存执行时间、处理键数、RU 阈值、动作、切组名称以及 watch 类型/时长。
- `BackgroundSettings`：保存后台任务类型列表与 `1..=100` 的资源利用率上限。
- `ResourceGroupSettings`：资源组完整配置；默认优先级为 8，其余限额为空或零，Runaway/Background 默认为 `None`。
- `ResourceGroupInfo`：包含 `id`、名称、`GroupState` 和设置。
- `ResourceGroupOption`、`RunawayOption`、`BackgroundOption`：本文件自己的归一化选项层，不是 parser AST 类型。
- `ResourceGroupError`：区分存在性、状态、选项、时长、后台配置和外部后端错误；`Display` 当前只输出 Debug 名称。
- `ResourceGroupManager`：外部 Resource Manager/infosync 的最小抽象，要求实现 `add`、`modify`、`delete`。
- `ResourceGroupCatalog`：以私有 `BTreeMap<i64, ResourceGroupInfo>` 保存组，并公开 `schema_version`；`get` 提供只读查询。
- `build_resource_group`：从旧组复制 ID、名称和设置，故意把状态重置为 `None`，逐项应用选项，再执行有限 RU 的 burst capacity 调整。
- `set_direct_resource_group_settings`、`set_direct_runaway_option`、`set_direct_background_option`：按层写入配置并返回细分错误。
- `parse_background_job_types`、`parse_duration_ms`、`check_resource_group_validation`：分别承担后台类型、Go 风格 duration 和最终配置的校验。

## 执行流程

创建流程由 `ResourceGroupCatalog::create` 表达：

1. 先按 ID 以及不区分 ASCII 大小写的名称检查冲突，冲突返回 `AlreadyExists`。
2. 调用 `check_resource_group_validation`；通过后把传入组状态改为 `Public`。
3. 把克隆后的组插入 `groups`，再调用 `ResourceGroupManager::add`。
4. 外部添加失败通常转为 `Backend`；唯一例外是默认组且错误文本包含 `"already exists"`，用于模拟 Go keyspace 兼容分支。
5. Manager 成功或命中上述兼容例外后，`schema_version` 饱和加一并返回新版本。

修改流程由 `ResourceGroupCatalog::alter` 表达：先按 ID 查旧值，复制整个组并替换 settings，完成校验后立即覆盖 `groups`；之后才调用 `manager.modify`。因此 Manager 失败时返回 `Backend`，但新设置已保留，schema version 不推进。成功时才推进版本。

删除流程由 `ResourceGroupCatalog::drop_group` 表达：按 ID 查找并要求状态为 `Public`，先从目录移除，再调用 `manager.delete`；外部删除失败时目录不会恢复，schema version 也不推进。成功后版本加一。

配置构建流程从 `build_resource_group` 开始。它逐项调用 `set_direct_resource_group_settings`；Runaway 和 Background 再下钻到各自子选项函数。最后，当 RU 不是 unlimited 且 burst limit 非负时，将 burst limit 调整为 RU rate，模拟 Go `ResourceGroupSettings.Adjust()` 的当前关键效果。需要注意：该构建函数本身不做最终合法性校验，调用方应显式调用 `check_resource_group_validation`；`ResourceGroupCatalog::{create,alter}` 已这样做。

## 数据与状态

`ResourceGroupCatalog.groups` 的键是数值 ID，名称唯一性只在 `create` 时通过遍历现有值检查；名称比较使用 `eq_ignore_ascii_case`。目录没有按名称的二级索引，也没有持久化能力。`schema_version` 是进程内 `u64`，使用 `saturating_add(1)`，只在目录和 Manager 两侧都达到本函数定义的成功条件后推进。

状态迁移很短：创建会无条件把输入状态设为 `Public`；修改保留旧状态；删除只接受 `Public` 并直接移除。`build_resource_group` 则故意生成 `GroupState::None`，与 Go 新建 `ResourceGroupInfo` 时状态采用零值一致，它并不代表已持久化状态。

突发值有三套相关约定：禁用为 `0`，无限为 `-1`，moderated 为 `-2`。`RuRate { burstable: Unlimited }` 还会把 RU rate 写成最大 `i32` 占位值。构建结束时，普通有限 RU 且 burst limit 非负会把 burst limit 设为 RU rate；负值 `-1/-2` 保持不变。

Runaway/Background 使用 `Option` 区分未配置与存在。Runaway 的空选项列表生成 `None`；Background 即使列表为空也生成 `Some(default)`，这是对 Go 文件中先置 nil 随后又无条件分配新对象这一实际行为的刻意保留。只有默认组允许 Background。

## 依赖与调用关系

文件唯一的标准库依赖是 `std::collections::BTreeMap`；所有领域类型和 Manager trait 都在本文件内定义。`pkg/ddl/Cargo.toml` 说明本模块属于 `astersql-ddl`，但本文件没有直接引用该 manifest 中的外部 crate。

RustCodeGraph 对主要函数给出的下游边包括：`build_resource_group -> set_direct_resource_group_settings`；`set_direct_resource_group_settings -> set_direct_runaway_option/set_direct_background_option`；`set_direct_runaway_option -> parse_duration_ms`；`set_direct_background_option -> parse_background_job_types`。`ResourceGroupCatalog::{create,alter}` 在源码中直接调用 `check_resource_group_validation` 和 Manager 方法，`drop_group` 调用 `get/remove/delete`。

上游方面，RustCodeGraph 符号查询只显示 `pkg/ddl/resource_group_test.rs` 的导入，`rg` 也未发现生产 Rust 文件调用这些 API。因此不能把 Go 的真实调用边直接归给当前 Rust 文件。Go 对照的真实链路是：`pkg/ddl/executor.go` 的 `AddResourceGroup`/`AlterResourceGroup` 调用 `buildResourceGroup` 和 `checkResourceGroupValidation` 后提交 job；`pkg/ddl/job_worker.go` 按 action 调用 `onCreateResourceGroup`、`onAlterResourceGroup`、`onDropResourceGroup`；这些 handler 再操作 meta、infosync、schema version 和 job 完成状态。

当前 Rust SQL 路径是另一条链：`pkg/session/runtime/dispatch.rs` 识别三个资源组 AST 节点，调用 `pkg/session/runtime/control.rs` 的三个 `execute_*_resource_group` 方法，后者读写 `RUNTIME_RESOURCE_GROUPS`，没有调用本文件。

## 错误处理与边界

- `create` 的 ID 或大小写不敏感名称冲突为 `AlreadyExists`；`alter`/`drop_group` 的未知 ID 为 `NotFound`；删除非 `Public` 组为 `InvalidState`。
- Manager 错误以原字符串包装为 `Backend(String)`，没有结构化错误码。默认组创建只用字符串包含判断兼容 `already exists`，可能受后端错误文本变化影响。
- create、alter、drop 都不是事务式的：目录修改发生在外部同步之前，外部失败不会回滚目录。对应测试专门锁定 alter/drop 的部分成功语义；create 同样先插入再调用 Manager，虽没有独立回归测试。
- `check_resource_group_validation` 只检查非空名称、priority 不超过 16、RU/burst 基本组合以及 Background 上限；它比 Go 的 `resourcegroup.NewGroupFromOptions` 校验面窄，不能被视为完整协议验证。
- Background 利用率的直接 setter 接受 `1..=100`；最终校验仅拒绝大于 100，因此手工构造的 `0` 可通过最终校验。安全扩展时应维持“setter 校验”和“完整对象校验”的区别，或用测试明确收紧兼容性。
- 后台任务白名单是 `lightning/br/dumpling/background/ddl/stats/import`；空片段被忽略，未知类型返回携带归一化名称的 `InvalidTaskName`。
- duration 允许正负号、复合单位、小数和微秒符号，按浮点累计后向零截断为毫秒；非法格式、未知单位、非有限或越过 `i64` 范围返回 `InvalidDuration`。Runaway 执行时长随后以 `as u64` 转换，负值会按 Rust 整数转换规则变成很大的无符号数，这是为贴近 Go 将有符号毫秒直接转 `uint64` 的行为，不应在没有兼容性证据时擅自“修正”。
- 文件没有检查默认资源组禁止删除、用户对资源组的依赖或 `IF EXISTS` 语义；这些是更上层职责。

## 并发与资源生命周期

`ResourceGroupCatalog` 不包含锁、原子量、异步任务、通道或事务；其变更方法要求 `&mut self`，所以单个实例的安全串行访问由 Rust 借用规则保证。若跨线程共享，调用方必须自行放入 `Mutex`/`RwLock` 等同步容器。本文件的 `ResourceGroupManager` 同样是同步、借用式接口，没有超时、取消上下文或重试。

每次操作只克隆小型配置对象并同步调用 Manager，没有后台资源需要回收。失败生命周期的重要不变量是：Manager 失败后目录可能已经改变，但 `schema_version` 仍保持旧值。调用方若在生产环境复用这个模型，必须明确如何补偿或重试，否则目录与外部 Manager 会暂时不一致。

Go 版本创建时用 `context.WithTimeout(..., 5s)` 调用 infosync，而 alter/drop 使用 `context.TODO()`；本 Rust trait 没有表达这些 context 生命周期。Go 的 job owner、持久化、恢复与 follower schema sync 也都不在本文件中。当前 Rust SQL 运行时的 `RUNTIME_RESOURCE_GROUPS` 使用全局 mutex，但那属于 `pkg/session/runtime/control.rs`，不能视为 `ResourceGroupCatalog` 的并发保证。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ddl/resource_group.go`：

- Rust 常量、option/settings 类型和 `build_resource_group` 对应 Go 的 `unlimitedRURate`、parser/model 选项以及 `buildResourceGroup`；Rust 为避免跨 crate 依赖，重新定义了精简领域类型。
- `set_direct_resource_group_settings` 对应 `SetDirectResourceGroupSettings`，包括 RU unlimited、priority、CPU/IO、burst `0/-1/-2`、Runaway 和仅 default 可改 Background 的规则。
- `set_direct_runaway_option` 对应 `SetDirectResourceGroupRunawayOption`；`parse_duration_ms` 补足 Go `time.ParseDuration` 在测试覆盖范围内的复合/小数格式。
- `set_direct_background_option` 与 `parse_background_job_types` 对应 Go 同名逻辑。Rust 白名单是源码内常量；Go 使用 client-go 的 `kvutil.ExplicitTypeList`，未来上游白名单变化不会自动同步。
- `check_resource_group_validation` 名称对应 Go 函数，但 Rust 只实现局部检查；Go 委托 `resourcegroup.NewGroupFromOptions` 做 proto 转换和完整验证。
- `ResourceGroupCatalog::{create,alter,drop_group}` 概括 Go `onCreateResourceGroup`、`onAlterResourceGroup`、`onDropResourceGroup` 的关键更新顺序和版本推进，却省略 job 参数解码、job 状态、日志、真实 meta、infosync context、`updateSchemaVersion` 与 `FinishDBJob`。

`pkg/ddl/resource_group_test.rs` 是当前 Rust 直接测试文件，覆盖 alter/drop 在 Manager 失败时不回滚 meta、build 重置状态且不额外校验、空 Background 与任务白名单，以及 Go duration 兼容。仓库中不存在同路径的 `pkg/ddl/resource_group_test.go`；Go 主链证据来自 `resource_group.go`、`executor.go`、`job_worker.go` 及更高层 SQL 测试，不能据此宣称 Rust 本文件已经具备同等端到端能力。

## 扩展指南

若扩展选项语义，优先修改 `ResourceGroupOption`（或对应子枚举）及 `set_direct_resource_group_settings`/子 setter；同步检查 `ResourceGroupSettings` 默认值、`build_resource_group` 的 Adjust 语义和 `check_resource_group_validation`。同时在独立文件 `pkg/ddl/resource_group_test.rs` 增加边界测试，不要把测试嵌入生产源文件。

若新增 Background task type，应先核对 Go `kvutil.ExplicitTypeList` 的实际集合，再更新 `parse_background_job_types` 与允许/拒绝测试；单独改 Rust 白名单会造成跨语言漂移。若修改 duration，应以 Go `time.ParseDuration` 为兼容基准，覆盖复合单位、小数、正负号、微秒别名、溢出和毫秒截断。

若要把本文件接入生产 DDL，不能只调用 `ResourceGroupCatalog`：必须决定是替换当前 `pkg/session/runtime/control.rs` 的简化路径，还是接入 `pkg/ddl/executor.rs`/job worker，并补齐持久化 job、owner failover、真实 meta、infosync 超时与重试、schema version/sync、默认组保护、权限依赖检查及 `IF EXISTS` 告警。该接线属于独立实现任务，不能由本文档推断为已存在。

兼容性风险集中在：错误类型/错误码、默认组特例、配置白名单、失败后的部分副作用和 schema version 推进时点。性能风险较小，但 `create` 的名称唯一性是 O(n) 遍历；若目录规模增长，可新增名称索引，同时必须保证 ID/名称两套索引的原子一致性。并发接入时不要仅依赖 `&mut self`，应定义锁范围以及 Manager 调用是否在锁内。

## 验证依据

- 源码全量阅读：`pkg/ddl/resource_group.rs`，确认常量、7 个主要领域类型/枚举、错误类型、Manager trait、Catalog 与全部辅助函数；无条件编译分支。
- crate 与模块接线：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`；后者公开 `resource_group`，并仅在 `#[cfg(test)]` 下装入独立测试模块 `resource_group_test`。
- RustCodeGraph：`status` 显示索引有效；符号查询定位 `ResourceGroupCatalog`、`build_resource_group`、`set_direct_resource_group_settings`、`check_resource_group_validation` 等；callees 查询确认 `build -> set_direct`、settings setter -> 两个子 setter、Runaway setter -> duration parser、Background setter -> task parser。路径过滤未返回目标文件且 callers 输出为空，因此又以 `rg` 核对上游。
- Rust 上游与接线核验：全仓 Rust 搜索仅发现 `pkg/ddl/resource_group_test.rs` 使用本文件 API；`pkg/session/runtime/dispatch.rs` 与 `pkg/session/runtime/control.rs` 证明当前 SQL 路径使用另一套运行时实现。
- Rust 独立测试：`pkg/ddl/resource_group_test.rs` 的 5 个测试分别覆盖 Manager 同步失败顺序、state/validation 边界、Background 白名单和 Go duration 语法。本任务按计划只读测试，不运行 Cargo。
- Go 对照：`pkg/ddl/resource_group.go`；入口和真实 job 链路补充证据为 `pkg/ddl/executor.go` 的 `AddResourceGroup`/`AlterResourceGroup`/`DropResourceGroup` 与 `pkg/ddl/job_worker.go` 的三个 action 分发。
- 人工复核结论：本文件存在的价值是保存和测试资源组 DDL 的局部移植语义；它当前通过纯内存 Catalog 和 Manager trait 运行，尚未成为生产 Rust DDL job 主链。安全扩展必须同步独立 Rust 测试和 Go 对照，并明确上述接线缺口。
