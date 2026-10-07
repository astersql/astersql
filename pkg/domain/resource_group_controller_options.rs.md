# `pkg/domain/resource_group_controller_options.rs`

## 文件定位

本文件属于 `astersql-domain` crate；`pkg/domain/Cargo.toml` 以 `lib.rs` 为 crate 根，而 `pkg/domain/lib.rs` 通过 `pub mod resource_group_controller_options` 公开该模块。它位于 Domain 初始化资源组查询控制器的接线边界：`Domain::init_resource_groups_controller` 在创建 `tikv_client::resource_group_lookup::ResourceGroupLookupController` 前调用本文件，把 Domain 已经判定好的部署模式和降级开关转换为 client 可消费的 `CreateOption` 列表（`pkg/domain/runaway.rs:119-143`）。

该文件不是资源组缓存、重试循环或 runaway 判定的实现；这些行为由 `tikv-client` 的查询控制器和 `pkg/resourcegroup/runaway` 承担。本文件只负责为控制器构造与 Go 版本一致的参数。

## 核心职责

1. 用 `new_default_degraded_ru_settings` 构造 Starter 临时降级资源组所需的默认 RU token bucket：填充速率为 `2_000_000`，突发上限为 `50_000_000_000`。
2. 用 `new_resource_groups_controller_options` 始终设置 runaway 侧定义的最大等待时间，并仅在 `is_starter && enable_fallback` 时追加降级 RU、降级等待时间和重试参数。
3. 保持判断边界清晰：Rust 函数不读取进程全局配置，而由调用者把部署模式与开关显式传入；这与 Go 函数在内部读取 `deploymode.IsStarter()` 和 `config.GetGlobalConfig().StarterParams.EnableRGFallback` 的结果等价，但依赖更显式。

## 主要符号

- `DEFAULT_DEGRADED_RU_FILL_RATE: u64 = 2_000_000`：降级 token bucket 的每秒 RU 填充速率。
- `DEFAULT_DEGRADED_RU_BURST_LIMIT: i64 = 50_000_000_000`：降级 token bucket 的突发容量。类型跟随 protobuf 字段 `TokenLimitSettings::burst_limit`。
- `DEFAULT_DEGRADED_MODE_WAIT_TIMEOUT: Duration = 1500ms`：查询失败后进入降级模式前的等待时长，对应 Go 的 `3 * time.Second / 2`。
- `TOKEN_WAIT_RETRY_INTERVAL: Duration = 100ms` 与 `TOKEN_WAIT_RETRY_TIMES: u32 = 20`：启用 Starter fallback 时覆盖 provider 配置的 token 等待重试节奏。
- `new_default_degraded_ru_settings() -> rm::GroupRequestUnitSettings`：创建 `GroupRequestUnitSettings -> TokenBucket -> TokenLimitSettings` 三层 protobuf 值；未指定字段使用各类型的 `Default`。
- `new_resource_groups_controller_options(is_starter, enable_fallback) -> Vec<CreateOption>`：公开的 option 工厂。返回值首项始终为 `CreateOption::MaxWaitDuration`，其毫秒值来自 `astersql_resourcegroup_runaway::manager::MaxWaitDurationMillis`（当前为 `30_000`，见 `pkg/resourcegroup/runaway/manager.rs:39`）；两个布尔参数同时为真时再按固定顺序追加四项 fallback option。

## 执行流程

`Domain::init_resource_groups_controller` 首先在 provider 缺失时直接成功返回；provider 存在时才调用 `new_resource_groups_controller_options`。option 工厂先创建只含最大等待时间的向量，然后判断 `is_starter && enable_fallback`：

1. 条件不成立时，直接返回单项向量，因此 client 保留 provider 给出的重试间隔、重试次数与默认的零降级等待时间。
2. 条件成立时，调用 `new_default_degraded_ru_settings` 生成临时 RU 设置，并依次追加 `DegradedRuSettings`、`DegradedModeWaitDuration(1500ms)`、`WaitRetryInterval(100ms)` 和 `WaitRetryTimes(20)`。
3. 调用者把切片 `&options` 交给 `ResourceGroupLookupController::new`；构造成功后，Domain 记录控制器报告的 RU 版本并绑定控制器（`pkg/domain/runaway.rs:133-143`）。控制器随后处理实际 lookup、重试、降级合成和恢复，本文件不参与运行期调用。

测试 `starter_degraded_resource_group_recovers_without_caching` 证明临时 RPC 失败时会得到具有上述默认 RU 设置的降级组；provider 后续恢复后，再次查询返回真实组且总调用次数为两次，说明降级结果没有写入正常缓存（`pkg/domain/runaway_test.rs:119-153`）。

## 数据与状态

本文件没有全局可变状态、缓存或对象生命周期。五个参数均为编译期常量；两个函数每次调用都创建并返回拥有所有权的新 protobuf 值或 `Vec<CreateOption>`。

降级 RU 设置只填写 `r_u.settings.fill_rate` 和 `burst_limit`；`TokenBucket` 与 `TokenLimitSettings` 的其他字段保持默认值。option 向量中的顺序是稳定的：最大等待时间在首位，四项 fallback 设置随后追加。调用者可在控制器构造完成后释放该局部向量，因为后续状态由 controller 自己持有；这一点由调用方式 `ResourceGroupLookupController::new(..., &options)` 及控制器配置断言间接验证。

## 依赖与调用关系

- 上游模块声明：`pkg/domain/lib.rs:31` 公开本模块。
- 直接生产调用者：`Domain::init_resource_groups_controller`（`pkg/domain/runaway.rs:119-143`）。RustCodeGraph 的文件查询也报告目标文件由 `pkg/domain/runaway.rs` 和 `pkg/domain/runaway_test.rs` 使用。
- 下游标准库依赖：`std::time::Duration`，负责以强类型表达毫秒时长。
- 下游外部依赖：`tikv_client::proto::resource_manager` 提供 protobuf RU 类型；`tikv_client::resource_group_lookup::CreateOption` 提供控制器构造选项。`pkg/domain/Cargo.toml` 将该依赖固定到 `astersql/client-rust` 的 tag `v0.4.2-aster.10`，且关闭默认 features。
- 下游仓库依赖：`astersql_resourcegroup_runaway::manager::MaxWaitDurationMillis` 提供共享的 30 秒最大等待上限，避免 Domain 再维护一份数值。
- 独立 Rust 测试：`pkg/domain/runaway_test.rs`，由 `pkg/domain/lib.rs:93-94` 在 `cfg(test)` 下作为独立模块接入，没有把测试嵌入生产源文件。

RustCodeGraph 对两个精确函数执行 `callers`/`callees` 时没有输出可用边；因此上述具体调用和依赖关系以图的文件级 `used by` 结果、模块声明、调用现场及测试现场交叉核验，不把缺失的函数级边解释为“没有调用者”。

## 错误处理与边界

两个函数都是纯构造函数，不返回 `Result`，也没有显式 panic、I/O 或锁操作。protobuf 未设置字段通过 `Default::default()` 补齐；数值均为常量，因此本层没有解析失败或溢出分支。

关键边界是布尔合取而非任选其一：只有 Starter 且显式启用 fallback 才能覆盖 provider 的重试设置并允许降级。`resource_group_options_preserve_provider_settings_outside_enabled_starter` 覆盖 `(true, true)`、`(true, false)`、`(false, true)`，验证后两种组合保留 provider 的 `250ms`/`4` 次设置、降级等待为零，而且资源组 lookup 仍返回错误（`pkg/domain/runaway_test.rs:155-188`）。

provider 缺失、client 构造失败、资源组 RPC 失败和 Domain 关闭由调用者或 `tikv-client` 处理：`Domain::init_resource_groups_controller` 对无 provider 返回 `Ok(())`，对 `ResourceGroupLookupController::new` 的 `LookupError` 使用 `?` 传播；这些不属于本文件的错误策略。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或长生命周期资源，函数只在控制器初始化阶段同步执行。构造后的 option 被消费到 `ResourceGroupLookupController` 配置中，运行时重试与降级 lookup 的并发安全由 client 控制器负责。

测试中的 `ProviderStub` 使用 `Mutex` 保护可切换响应、用 `AtomicUsize` 统计调用次数；这证明的运行期性质是“失败后返回临时降级值，恢复后仍会重新访问 provider”，而不是本文件自身具有缓存或同步机制。`domain.close()` 后 controller 绑定被清除的断言也属于 Domain 生命周期，不应归因于 option 工厂。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/domain/resource_group_controller_options.go`：五个常量数值、降级 RU protobuf 形状、最大等待 option 和四项条件追加 option 均与 Rust 一致。Go 的 `newDefaultDegradedRUSettings` 返回指针，Rust 返回拥有所有权的 protobuf 值；这是语言所有权模型差异，不改变字段语义。

主要接线差异是配置来源。Go `newResourceGroupsControllerOptions()` 无参数，内部查询进程级 `deploymode` 与全局配置；Rust `new_resource_groups_controller_options(is_starter, enable_fallback)` 接收调用者已判定的值。Rust 源码注释明确称其为 entry adapter 的显式 deploy-mode 决策，测试也可以直接覆盖三个组合而不修改全局配置。

Go 的 `TestResourceGroupsControllerOptions`（`pkg/domain/runaway_test.go:188-258`）验证 Starter 开启、Starter 关闭和非 Starter 三条分支；Rust 的 `resource_group_options_preserve_provider_settings_outside_enabled_starter` 保留相同分支意图。Rust 的 `starter_degraded_resource_group_recovers_without_caching` 还直接核验默认 RU 设置和恢复行为。Go 注释强调降级组是资源管理器暂时不可用时的 best-effort UX fallback，不定义精确的跨 RPC RU 限流/记账契约，也不能进入正常元数据缓存；Rust 当前行为由恢复测试支持这一边界。

## 扩展指南

- 新增或调整 controller 构造选项时，优先修改 `new_resource_groups_controller_options`，并确认 option 应始终生效还是只属于 `is_starter && enable_fallback` 分支；不要把 lookup、缓存或重试循环搬入本文件。
- 调整降级 RU 形状时，修改 `new_default_degraded_ru_settings` 及对应常量，并与 `pkg/domain/resource_group_controller_options.go` 的 protobuf 字段、单位和符号类型逐项对齐。
- 若修改最大等待时间，应先核对共享来源 `pkg/resourcegroup/runaway/manager.rs::MaxWaitDurationMillis` 及 Go 的 `runaway.MaxWaitDuration`，避免 Domain 与 runaway 出现两套值。
- 测试应继续放在独立的 `pkg/domain/runaway_test.rs` 中。至少同步三类断言：开启分支的最终 client config、关闭/非 Starter 时 provider config 不被覆盖、临时失败后的降级值不会缓存且能恢复到真实值；Go 对照行为变化时也应核对 `pkg/domain/runaway_test.go::TestResourceGroupsControllerOptions`。
- 兼容性风险主要是 option 条件、时间单位、整数类型或顺序改变造成 client 配置漂移；性能风险主要来自重试间隔/次数和降级等待时长改变，而不是本文件的构造成本。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/domain/resource_group_controller_options.rs` 报告 10 个索引符号；`node --file ... --offset 1 --limit 260` 读取完整 53 行源码并报告 `pkg/domain/runaway.rs`、`pkg/domain/runaway_test.rs` 两个使用文件；`query` 分别唯一定位两个公开函数。对两函数运行 `callers`/`callees` 未得到函数级输出，未据此作否定推断。
- 生产源码与 crate 边界：`pkg/domain/resource_group_controller_options.rs`、`pkg/domain/runaway.rs`、`pkg/domain/lib.rs`、`pkg/domain/Cargo.toml`、`pkg/resourcegroup/runaway/manager.rs`。
- Go 对照：`pkg/domain/resource_group_controller_options.go`、`pkg/domain/runaway.go`。
- 独立测试：`pkg/domain/runaway_test.rs`、`pkg/domain/runaway_test.go`；重点核对降级 RU 值、三个开关组合、provider 恢复与不缓存行为。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核本文没有把 client/Domain 的运行期职责误写成本文件职责。
