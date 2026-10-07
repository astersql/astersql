# `pkg/domain/runaway.rs`

## 文件定位

`pkg/domain/runaway.rs` 属于 `astersql-domain` crate，由 `pkg/domain/lib.rs` 以公开模块 `runaway` 装配。它位于 Domain 生命周期与资源组/Runaway 子系统的交界处：一方面为资源控制器和 Runaway 管理器定义轻量的组合抽象，另一方面把 `tikv-client` 的资源组查询控制器保存到 `Domain`，并将该控制器适配成 `astersql-resourcegroup-runaway` 所需的资源组目录接口。

当前 Rust 主启动链只接入了 `Domain::init_resource_groups_controller`：`cmd/tidb-server/main.rs` 创建 `GrpcResourceGroupProvider` 后调用它。`ResourceGroupRuntime::initialize` 及本文件自定义的 `ResourceGroupController`、`RunawayManager` trait 目前只在独立测试 `pkg/domain/runaway_test.rs` 中使用，不能据此认为生产入口已经用这套抽象启动 Runaway 后台循环。

## 核心职责

1. `ResourceControllerConfig` 描述抽象资源控制器可暴露的实例 ID、通告地址、端口和 RU 模式状态。
2. `ResourceGroupRuntime::initialize` 保证组合运行时初始化时只调用一次其 controller 的 `start`；它不启动 Runaway manager 的 flush/watch 循环。
3. `Domain::init_resource_groups_controller` 在存在 PD 资源组 provider 时构造 `ResourceGroupLookupController`，记录其 RU 版本并原子式地发布共享 controller；没有 provider 时静默成功，兼容非 PD 存储。
4. `ControllerResourceGroupCatalog::GetResourceGroup` 把 TiKV client 返回的 protobuf 资源组及 Runaway 配置转换为本地 Runaway crate 的领域类型，使 checker 的 `SwitchGroup` 等动作能复用同一个 lookup controller。

本文件不负责创建生产 `runaway::manager::Manager`、启动其记录刷新/监视同步循环，也不负责安装 Go 版本中的 TiKV resource-control interceptor；这些能力在当前 Rust 文件中没有对应的生产接线。

## 主要符号

- `ResourceControllerConfig`：可克隆、可比较的公开配置值。`advertised_ip` 使用 `std::net::IpAddr`，端口为 `u16`，`request_unit_mode` 仅保存模式事实，本文件不据此分支。
- `ResourceGroupController: Send + Sync`：抽象 controller 的公开 trait，要求 `start(&self)` 与借用式 `config(&self)`；线程安全约束允许其放入 `Arc<dyn ...>`。
- `RunawayManager: Send + Sync`：当前为空的标记 trait，只表达组合对象可跨线程共享，没有启动、停止或查询行为。
- `ResourceGroupRuntime`：公开持有 `Arc<dyn ResourceGroupController>` 和 `Arc<dyn RunawayManager>`。`initialize(self) -> Self` 消费并返回自身，在返回前调用 controller 的 `start`。
- `Domain::init_resource_groups_controller(...) -> Result<(), LookupError>`：生产接线入口。输入为可选 `ResourceGroupProvider`、keyspace ID，以及由调用方显式传入的 Starter/fallback 开关。
- `LookupError`、`ResourceGroupLookupController`、`ResourceGroupProvider`：从 `tikv_client::resource_group_lookup` 公开重导出，供 Domain 调用者与测试使用。
- `ControllerResourceGroupCatalog(Arc<ResourceGroupLookupController>)`：newtype 适配器；实现 Runaway crate 的 `ResourceGroupCatalog::GetResourceGroup`（名称沿用上游接口的 Go 风格）。

文件没有模块级常量、条件编译项或私有辅助函数。fallback 的常量和 option 构造位于相邻的 `pkg/domain/resource_group_controller_options.rs`。

## 执行流程

生产初始化路径如下：

1. `cmd/tidb-server/main.rs` 从配置中的 TiKV/PD endpoint、keyspace ID 和 TLS 配置构造 `GrpcResourceGroupProvider`。
2. 主程序调用 `Domain::init_resource_groups_controller(Some(provider), keyspace_id, true, true)`。如果传入 `None`，函数立即返回 `Ok(())`，不改变 Domain 中已有的 controller 或 RU 版本。
3. 函数调用 `new_resource_groups_controller_options(is_starter, enable_fallback)`。该相邻模块总是设置 Runaway 最大等待时间；仅两个布尔值都为真时追加降级 RU 配置、1.5 秒降级等待时间和重试参数。
4. `ResourceGroupLookupController::new(provider, keyspace_id, &options)` 成功后，先通过 `controller.ru_version()` 写入 `Domain::set_ru_version`，再经 `bind_resource_group_lookup_controller(Some(controller))` 发布共享 `Arc`。
5. 需要 Runaway 资源组目录时，调用方用 `ControllerResourceGroupCatalog` 包装同一 controller。`GetResourceGroup` 调用底层 `get_resource_group(name)`，然后逐字段转换规则、动作和 watch 类型。

转换规则中，`exec_elapsed_time_ms: u64` 在超过 `i64::MAX` 时饱和为 `i64::MAX`；已知 action 映射为 `DryRun`、`CoolDown`、`Kill` 或 `SwitchGroup`，未知/无效枚举映射为 `NoneAction`；watch 类型同理，未知值映射为 `None`。底层成功返回的资源组总被包装为 `Ok(Some(...))`。

抽象组合路径更短：构造 `ResourceGroupRuntime` 后调用 `initialize`，controller 的 `start` 先执行，原组合对象随后返回；manager 不发生任何调用。该行为由 `initialize_starts_only_the_resource_controller_like_go` 验证。

## 数据与状态

`ResourceGroupRuntime` 自身没有锁或可变字段；共享性来自两个 `Arc<dyn ...>`。`initialize` 消费 `self`，避免初始化过程中额外复制组合对象，但 trait 没有“已经启动”状态，因此重复构造或对同一 `Arc` 放入多个 runtime 时，幂等性必须由 controller 实现保证。

生产 controller 状态保存在 `Domain`：`resource_group_lookup_controller` 是 `RwLock<Option<Arc<ResourceGroupLookupController>>>`，`ru_version` 是原子整数。初始化按“构造成功后再写版本并发布”的顺序进行；构造失败不会发布 controller。`Domain::close` 会调用 `bind_resource_group_lookup_controller(None)` 释放 Domain 持有的 lookup controller，但当前不会在这里取出或停止 `runaway_manager`。

目录适配是即时读取：每次 `GetResourceGroup` 都调用底层 controller，并新建本地 `runaway::ResourceGroup`。Runaway settings、rule 和 watch 均按值转换；`switch_group_name` 和资源组名称移动到新对象中。Starter 降级资源组是否缓存由 `tikv-client` controller 决定，`pkg/domain/runaway_test.rs` 验证降级结果不会阻止下一次读取恢复后的真实组。

## 依赖与调用关系

上游调用与装配：

- `pkg/domain/lib.rs` 公开声明 `pub mod runaway`，并仅在 `cfg(test)` 下装配 `runaway_test`。
- `cmd/tidb-server/main.rs` 是 `Domain::init_resource_groups_controller` 的已确认生产调用者，负责创建 gRPC provider；当前传入 `is_starter = true`、`enable_fallback = true`。
- `pkg/domain/domain.rs` 提供 `set_ru_version`、controller 的 bind/get 方法、Runaway manager 的 bind/get 方法以及 `close` 时的 controller 清理。

下游依赖：

- `tikv-client`（`pkg/domain/Cargo.toml` 固定到带 tag 的 `v0.4.2-aster.10`）提供 provider、lookup controller、错误类型和 resource-manager protobuf。
- `pkg/domain/resource_group_controller_options.rs` 构造 `CreateOption`，并读取 `astersql-resourcegroup-runaway::manager::MaxWaitDurationMillis`。
- `astersql-resourcegroup-runaway` 提供 `ResourceGroupCatalog`、转换目标类型、checker 和 manager；本文件只实现目录适配，不创建生产 manager。

RustCodeGraph 能定位 `ControllerResourceGroupCatalog`、目标文件源码以及 `pkg/domain/runaway_test.rs` 的使用，但没有为 `init_resource_groups_controller` 返回符号查询/调用边；生产调用关系因此由精确源码搜索及 `cmd/tidb-server/main.rs` 直接调用核验。

## 错误处理与边界

- provider 缺失是受支持边界：`init_resource_groups_controller(None, ...)` 返回成功且不发布 controller，独立测试 `controller_initialization_skips_non_pd_storage` 覆盖该行为。
- controller 构造失败直接传播 `LookupError`。因为 RU 版本和 Domain controller 均在构造成功后才写入，失败路径不会发布半初始化的新对象。
- 目录查询错误被转换为 `runaway::Error::Storage(error.to_string())`；这里保留可读消息，但丢失原 `LookupError` 的结构化类型。
- action/watch 的未知 protobuf 枚举不会报错，分别降级为 `NoneAction` 与 `RunawayWatchType::None`，保证前向兼容时不执行未知处置。
- 执行时长从无符号 64 位转换为有符号 64 位时采用上限钳制，避免溢出或环绕；`request_unit`、`processed_keys` 和 watch duration 按 protobuf 字段类型直接传递。
- 本地 `ResourceGroupCatalog` trait 语义允许 `Ok(None)`，但此适配器对底层成功结果恒定返回 `Some`；资源组不存在时究竟由底层返回默认/降级组还是错误，属于 `tikv-client` controller 的契约。
- `RwLock` 中毒在 `Domain` 的 bind/get 方法中通过 `expect` 触发 panic，而不是转为本文件的 `LookupError`。

## 并发与资源生命周期

公开 trait 均要求 `Send + Sync`，生产 controller 和 manager 通过 `Arc` 共享。lookup controller 在 `Domain` 的 `RwLock<Option<Arc<_>>>` 中发布，读取者获得克隆的 `Arc`，因此替换或 `Domain::close` 清空槽位不会立即使仍由调用者持有的 controller 失效。

`Domain::close` 具备原子幂等保护，并清空 lookup controller。测试会在每个场景结束调用 `domain.close()`，验证 controller 槽位变为 `None`。本文件没有显式 spawn、channel、事务或文件/网络句柄管理；实际 gRPC 连接、重试和降级计时属于 `tikv-client` controller。

Go 的完整生命周期更宽：`initResourceGroupsController` 会立即 `control.Start(ctx)`、创建并保存 Runaway manager、设置全局 TiKV interceptor；`Domain.Start` 再把 manager 的记录刷新与 watch 同步循环加入 wait group，`Domain.Close` 调用 manager `Stop`。Rust 当前的 `ResourceGroupRuntime::initialize` 只在测试抽象上表达“启动 controller”，而已接线的 `ResourceGroupLookupController` 路径没有在本文件中显式启动或停止 Runaway manager，扩展时必须避免把两套生命周期混为一谈。

## 与 Go 版本的对应关系

直接 Go 对照为 `pkg/domain/runaway.go`：两版都把“无 PD provider”视为静默成功，并依据 keyspace 构造资源组 controller。Rust 将 Go 隐式读取进程部署模式/全局配置的 `newResourceGroupsControllerOptions()` 改成显式布尔参数；相同的 fallback 数值及语义由 `pkg/domain/resource_group_controller_options.rs` 与 `.go` 对照实现。

Rust 尚未完整复刻 Go 初始化：没有在该入口读取 server info/拼接 server address，没有创建 `runaway.NewRunawayManager`，没有保存 manager，也没有安装 `tikv.SetResourceControlInterceptor`。此外，Go 从 Store codec 推导 keyspace ID，而 Rust 由 `cmd/tidb-server/main.rs` 调用方传入；Go controller 的 `Start(ctx)` 有上下文和显式停止契约，Rust lookup controller 的具体后台生命周期由 client 库封装。

`ControllerResourceGroupCatalog` 是 Rust 特有的边界适配：Go 的 PD controller 直接满足 Runaway manager 需要的接口，Rust 则把 client protobuf 映射到 `astersql-resourcegroup-runaway` 的本地类型。`pkg/domain/runaway_test.go` 和 `.rs` 都验证 Starter fallback、恢复后不缓存降级组、配置开关，以及 `SwitchGroup` 能把请求切到目标组；Rust 还明确覆盖 `None` provider 和 Domain 关闭后清空 controller。

## 扩展指南

- 若补齐生产 Runaway manager 接线，最可能修改 `Domain::init_resource_groups_controller`、`ControllerResourceGroupCatalog`、`pkg/domain/domain.rs` 的启动/关闭流程及 `cmd/tidb-server/main.rs`。必须与 Go 的 manager 构造参数、两个后台循环和停止顺序逐项对齐，且在独立的 `pkg/domain/runaway_test.rs` 中新增生命周期回归测试，不把测试写入生产源文件。
- 若增加 protobuf action、watch 类型或规则字段，应在 `ControllerResourceGroupCatalog::GetResourceGroup` 的 match/结构转换中显式映射，并为已知值、未知值及数值边界添加测试。保持未知枚举安全降级，评估旧 server/new client 的兼容性。
- 若改变 fallback 行为，应同步检查 `pkg/domain/resource_group_controller_options.rs`、对应 `.go` 文件及两种语言的 `runaway_test`；特别要保留“只在 Starter 且开关开启时覆盖 provider 设置”和“降级组不进入正常缓存”的不变量。
- 若需要重初始化或动态切换 provider，应先定义旧 controller 的停止/排空契约以及 RU 版本与 controller 发布的一致性；当前两个状态分别写入，读者可能观察到短暂的版本/对象交错。性能上应避免在 `GetResourceGroup` 转换路径增加阻塞锁或额外远程查询。
- `ResourceGroupController`/`RunawayManager`/`ResourceGroupRuntime` 当前不是生产主链。若保留它们，应接入真实实现并补充停止语义；若调整抽象，先核对是否仍需承载 Go 生命周期意图，不能仅因当前调用少而删减。

## 验证依据

- 目标源码：`pkg/domain/runaway.rs`，核对全部公开结构、trait、impl、重导出、转换分支及无条件编译事实。
- crate 与装配：`pkg/domain/Cargo.toml`、`pkg/domain/lib.rs`；确认 `astersql-domain` 边界、`tikv-client` tag、Runaway crate 依赖和独立测试模块。
- Rust 直接入口与状态：`cmd/tidb-server/main.rs`、`pkg/domain/domain.rs`、`pkg/domain/resource_group_controller_options.rs`。
- Rust 独立测试：`pkg/domain/runaway_test.rs`；覆盖抽象 controller 启动、Starter 降级/恢复、option 保留、SwitchGroup、无 provider 和关闭清理。
- Go 对照：`pkg/domain/runaway.go`、`pkg/domain/resource_group_controller_options.go`、`pkg/domain/domain.go`、`pkg/domain/runaway_test.go`。
- Runaway 接口定义：`pkg/resourcegroup/runaway/lib.rs` 中的 `ResourceGroupCatalog`、`ResourceGroup`、`RunawaySettings`、`RunawayRule` 与 `RunawayWatch`。
- RustCodeGraph：`status` 显示索引可用；路径过滤未命中，但 `query ControllerResourceGroupCatalog --kind struct` 与 `node --file pkg/domain/runaway.rs --offset 80 --limit 120` 成功定位源码，`query init_resource_groups_controller --kind method` 未返回结果。缺失的调用边用上述直接源码和精确 `rg` 搜索核验。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构用任务指定的 11 章节命令验证。
