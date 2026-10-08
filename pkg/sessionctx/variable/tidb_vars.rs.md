# [`pkg/sessionctx/variable/tidb_vars.rs`](./tidb_vars.rs)

## 文件定位

本文件属于 `astersql-sessionctx-variable` crate。crate 入口 `pkg/sessionctx/variable/lib.rs` 以私有模块 `mod tidb_vars` 挂载它，再通过 `pub use tidb_vars::*` 将其公开 API 重导出。它不保存某个会话的系统变量值，也不定义系统变量元数据；其职责是在变量子系统与 Domain、DDL、统计信息、PD/TiKV 驱动等实例级组件之间提供可延迟注册的副作用钩子。

`pkg/sessionctx/variable/Cargo.toml` 将该目录声明为独立 library crate（`path = "lib.rs"`，`autotests = false`），并以 `package.metadata.porting.go-package = "pkg/sessionctx/variable"` 标记 Go 移植来源。本文件只直接依赖标准库同步原语以及 crate 内重导出的 `Context`、`VariableError`；没有条件编译项。

## 核心职责

1. 用带 `Send + Sync` 约束的 `Arc<dyn Fn...>` 类型别名统一描述实例级回调的参数、返回值和可跨线程共享性。
2. 用 `hook_slot!` 为 17 类可选回调生成全局槽位及注册/读取函数，使变量层无需依赖副作用的实际实现模块。
3. 为启用/禁用全局资源控制维护两个始终有值的回调，默认实现为空操作。
4. 为 DDL Owner 和 Stats Owner 提供“未注册即成功”的内部调用包装，供 `varsutil.rs` 的开关函数使用。
5. 提供 `clear_instance_hooks_for_test`，清理四个 Owner 相关可选槽位，减少串行测试之间的全局状态污染。

该文件是进程级依赖注入边界，而不是业务实现所在地。回调的真实效果由注册者决定；仅看到某个槽位存在，不能推断对应的 Domain、PD 或存储功能已经在 Rust 主链完成接线。

## 主要符号

- 回调类型：`Int64Setter`、`Int64Getter`、`StringPairSetter`、`BoolSetter`、`FallibleBoolSetter`、`FallibleVoidHook`、`ContextTimestampSetter`、`ContextTimestampGetter`、`ContextStringValidator`、`DurationSetter`、`ContextSizeSetter`、`U32Setter`、`VoidHook`。所有类型都以 `Arc` 持有动态函数，并要求函数对象 `Send + Sync`。带失败语义的类型返回 `Result<_, VariableError>`。
- `hook_slot!($slot, $ty, $setter, $getter)`：为每个可选钩子生成一个 `LazyLock<RwLock<Option<T>>>` 静态槽位、一个接受 `Option<T>` 的公开注册函数，以及一个克隆并返回 `Option<T>` 的公开读取函数。传入 `None` 即注销。
- 由宏生成的 17 组槽位覆盖：Analyze 内存配额的 set/get、统计缓存容量、PD 客户端动态选项、MDL、DDL 启停、Fast Create Table、外部时间戳 set/get、全局资源控制布尔设置、云存储 URI 校验、低精度 TSO 更新间隔、Schema 缓存大小、Stats Owner 启停、PD metadata circuit breaker 错误率阈值。
- `ENABLE_GLOBAL_RESOURCE_CONTROL`、`DISABLE_GLOBAL_RESOURCE_CONTROL`：两个非 `Option` 的 `LazyLock<RwLock<VoidHook>>`，初始回调为 `Arc::new(|| {})`。
- `set_enable_global_resource_control_hook`、`set_disable_global_resource_control_hook`：替换上述两个资源控制回调。
- `enable_global_resource_control`、`disable_global_resource_control`：读取并克隆回调后执行。
- `call_enable_ddl_hook`、`call_disable_ddl_hook`、`call_enable_stats_owner_hook`、`call_disable_stats_owner_hook`：crate 内可见的可选回调包装；槽位为空时返回 `Ok(())`。
- `clear_instance_hooks_for_test`：将 DDL/Stats Owner 的四个可选槽位恢复为 `None`。它不会清除另外 13 个可选槽位，也不会重置两个资源控制回调。

## 执行流程

典型可选钩子流程如下：实例组件构造一个满足相应别名签名的 `Arc` 回调，调用宏生成的 `set_*_hook(Some(callback))` 写入进程级槽位；变量处理路径调用对应 `get_*_hook()` 取得克隆；若有值则在锁外执行回调。注销时注册者写入 `None`。

当前仓库中已能确认的生产消费链是：`pkg/sessionctx/variable/varsutil.rs::switchDDL(enabled)` 根据布尔值调用 `call_enable_ddl_hook` 或 `call_disable_ddl_hook`；`switchStats(enabled)` 同样分派到 Stats Owner 的启用/禁用包装。四个包装用 `Option::map_or(Ok(()), |hook| hook())`，因此未注入时是无副作用的成功，已注入时原样传播回调结果。

资源控制的执行路径略有不同：注册函数直接替换非可选槽位；`enable_global_resource_control`/`disable_global_resource_control` 先在读锁下克隆 `Arc`，释放读锁后再调用。默认空回调保证调用者不需要判空。

RustCodeGraph 将本文件列为被 `varsutil.rs`、`tidb_vars_test.rs` 和 `tidb_vars_4_aster_unit_test.rs` 使用；对宏生成访问器的图解析不完整，因此其他槽位是否接线又用全仓 `rg` 复核。当前未发现除本文件及测试外对那些访问器的 Rust 调用，文档据此只将它们描述为已定义的注入接口，而不声称已进入运行主链。

## 数据与状态

所有槽位都是进程级 `static` 状态，首次访问时由 `LazyLock` 初始化。17 个宏槽位初值为 `None`；两个全局资源控制槽位初值为可调用的空函数。状态不属于 `SessionVars`，也不会随连接关闭自动恢复。

`Arc` 让读取者获得回调所有权的廉价克隆，注册者替换槽位后，已取得的旧 `Arc` 仍可安全执行。`ContextTimestampSetter`、`ContextTimestampGetter`、`ContextStringValidator`、`ContextSizeSetter` 虽接收 `&Context`，但当前 `pkg/sessionctx/variable/variable.rs::Context` 只是 `Default` 的轻量占位结构；不能据此假定它已携带 Go `context.Context` 的取消、截止时间或请求值语义。

该文件没有缓存具体配额、时间戳、URI 或开关值，也不保证 setter/getter 成对注册。每个槽位独立更新，组合一致性由注册方和调用方负责。

## 依赖与调用关系

- 上游模块边界：`lib.rs` 私有声明并公开重导出全部符号，所以同 crate 与外部依赖者都可使用 `pub` 注册/读取接口；四个 `call_*_hook` 仅限 crate 内。
- 已确认调用者：`varsutil.rs::switchDDL` 调用两个 DDL 包装，`varsutil.rs::switchStats` 调用两个 Stats Owner 包装。`tidb_vars_4_aster_unit_test.rs::optional_instance_hooks_are_safe_and_injectable` 覆盖这条分派链。
- 已确认测试调用者：`tidb_vars_test.rs::global_resource_control_hooks_can_replace_themselves` 直接注册并从新线程调用资源控制钩子；`tidb_vars_4_aster_unit_test.rs` 注册 DDL/Stats Owner 回调并检查调用顺序。
- 下游依赖：槽位管理只调用 `std::sync::{Arc, LazyLock, RwLock}`。回调失败统一使用 crate 内 `VariableError`；上下文参数使用 crate 内 `Context`。
- 尚未发现 Rust 消费者的接口：除 DDL/Stats Owner 和资源控制调用函数外，其余宏槽位目前只有定义。`pkg/domain/domain_sysvars.rs` 存在名称相近的实例方法，但全仓搜索未显示它们注册到本文件槽位，不能将名称相似当作调用边。

RustCodeGraph 的 `node --file` 能完整列出本文件及三个使用文件，但 `callers/callees` 对宏展开访问器及这组跨模块调用未返回完整边；因此本节调用关系同时以 `varsutil.rs` 的直接源码调用和全仓符号搜索作为证据。

## 错误处理与边界

宏槽位的读写锁若中毒，注册或读取会以 `expect("variable hook poisoned")` panic；接口没有把锁中毒转换为 `VariableError`。正常业务失败由具体回调返回，DDL/Stats Owner 包装不改写错误。

可选槽位默认 `None`。只有四个 `call_*_hook` 明确定义了“缺少回调等价于成功”；宏生成的 getter 只返回 `Option`，其他调用者必须自行决定缺失回调的语义。资源控制槽位则用空函数消除缺失分支。

`clear_instance_hooks_for_test` 是局部清理工具，不是全注册表复位函数。测试若修改其他槽位或两个非可选资源控制槽位，必须自行恢复，并用串行化手段避免与并发用例争用全局状态。

回调签名之间的错误能力不同：`Int64Setter`、`BoolSetter`、`U32Setter`、`VoidHook` 无法向调用者报告失败；其他带 `Result` 的类型可以。新增钩子时应按真实失败语义选择签名，不能为了方便丢弃底层错误。

## 并发与资源生命周期

`RwLock` 使注册替换与并发读取在内存安全层面互斥；`Arc` 和 `Send + Sync` 允许回调跨线程共享。代码刻意在调用前克隆回调并释放读锁，因此回调内部可以再次注册同一个槽位而不会因持有读锁造成自锁。`tidb_vars_test.rs::global_resource_control_hooks_can_replace_themselves` 通过在线程中执行、回调内替换自身并设置一秒超时，验证了这一性质。

这里没有跨多个槽位的事务或原子快照。一个线程可能在另一个线程连续更新两个相关槽位之间观察到混合状态。钩子中捕获资源的释放由 `Arc` 生命周期决定：槽位替换或清空只减少一个强引用；正在执行或被其他读取者克隆的旧回调会继续存活。

生产代码不会自动清理全局槽位。注册方应把它们视为进程生命周期配置；测试则应串行运行并显式恢复。现有两个相关测试均使用 `serial_test::serial`，其依赖来自 `Cargo.toml` 的 `dev-dependencies`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/sessionctx/variable/tidb_vars.go`。Rust 的大多数槽位逐项对应 Go 包级函数变量：`SetMemQuotaAnalyze`/`GetMemQuotaAnalyze`、`SetStatsCacheCapacity`、`SetPDClientDynamicOption`、`SwitchMDL`、`EnableDDL`/`DisableDDL`、`SwitchFastCreateTable`、`SetExternalTimestamp`/`GetExternalTimestamp`、`SetGlobalResourceControl`、`ValidateCloudStorageURI`、`SetLowResolutionTSOUpdateInterval`、`ChangeSchemaCacheSize`、`EnableStatsOwner`/`DisableStatsOwner`、`ChangePDMetadataCircuitBreakerErrorRateThresholdRatio`，以及两个全局资源控制函数。

Rust 没有照搬 Go 的可变全局函数变量，而是统一封装为锁保护的访问器；Go 只对部分入口使用 `atomic.Pointer`，其他入口是可为 `nil` 的函数变量。Rust 还通过 `Arc` 明确要求 `Send + Sync`，并通过资源控制调用函数确保执行时不持锁。

存在两项重要迁移差异：Go 文件包含 `UpdateExternalWorkloadTTLJobEnable func(context.Context, bool) error`，Rust 本文件没有对应类型或槽位；Rust 的 `Context` 当前是轻量占位，尚不等价于 Go `context.Context`。此外，Go 的 `varsutil.go::switchDDL`/`switchStats` 直接判空调用包变量，Rust 将相同行为收敛到四个 `call_*_hook` 包装中。

Go 侧系统变量定义在 `sysvar.go` 中大量消费这些函数变量；当前 Rust 全仓搜索没有显示多数对应 getter 已被系统变量定义调用。因此“接口已移植”与“完整行为已接线”必须区分，后续移植应逐条验证注册端和消费端。

## 扩展指南

新增实例级副作用时，先确认 Go 增量及真实注册者/消费者，再选择现有回调类型或新增精确类型别名。可缺省的能力应使用 `hook_slot!` 并明确 `None` 的调用语义；必须始终可调用的能力可仿照资源控制槽位提供安全默认值和专用调用函数。

调用回调时必须遵循“锁内克隆、锁外执行”，避免回调重入注册导致死锁。若一项操作可能失败，应返回 `Result<(), VariableError>`（或所需结果类型），不要选用无法传播错误的 setter。若多个槽位必须一致更新，现有独立锁模型不足，应先设计单一聚合状态，而不是依次写多个槽位后声称原子性。

完成接线至少要同时检查三个位置：本文件的类型/槽位；实际组件的注册代码；系统变量或执行路径的消费代码。涉及 Go 对齐时还要检查 `tidb_vars.go` 及相应 `sysvar.go` 使用点。新增或修改测试应放在独立测试文件，不应内嵌到 `tidb_vars.rs`；并发/重入性质优先扩展 `tidb_vars_test.rs`，DDL/Stats 分派语义可扩展 `tidb_vars_4_aster_unit_test.rs`。由于状态是全局的，相关用例应串行化并完整恢复所修改的槽位。

特别地，若要补齐 `UpdateExternalWorkloadTTLJobEnable`，不能只增加槽位：还需确定 Rust `Context` 能否表达所需请求语义、定位 TTL 调度注册端与系统变量消费端，并增加成功、失败、未注册和并发替换测试。

## 验证依据

- 源文件：`pkg/sessionctx/variable/tidb_vars.rs`，核对了全部类型别名、17 次 `hook_slot!` 展开调用、两个资源控制静态槽位、九个显式注册/调用/清理函数及其可见性。
- crate 边界：`pkg/sessionctx/variable/Cargo.toml` 与 `pkg/sessionctx/variable/lib.rs`，核对 library 路径、Go 包映射、`serial_test` 开发依赖、模块私有声明及公开重导出。
- Rust 调用路径：`pkg/sessionctx/variable/varsutil.rs::switchDDL`、`switchStats`；`pkg/sessionctx/variable/variable.rs::Context`。
- Rust 独立测试：`pkg/sessionctx/variable/tidb_vars_test.rs::global_resource_control_hooks_can_replace_themselves`；`pkg/sessionctx/variable/tidb_vars_4_aster_unit_test.rs::optional_instance_hooks_are_safe_and_injectable`。
- Go 对照：`pkg/sessionctx/variable/tidb_vars.go`、`pkg/sessionctx/variable/varsutil.go::switchDDL`/`switchStats`，并通过 `pkg/sessionctx/variable/sysvar.go` 的调用点核对 Go 侧实际消费者。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/sessionctx/variable/tidb_vars.rs` 与 `node --file ...` 确认目标文件有 27 个索引符号并被三个 Rust 文件使用；对 `switchDDL`、`call_enable_ddl_hook`、`enable_global_resource_control`、`clear_instance_hooks_for_test` 做了 `query/callers/callees` 查询。宏展开导致图中部分访问器和边缺失，已用全仓精确符号搜索补证。
- 未运行 Cargo 或代码测试：本任务只新增说明文档，按计划以事实复核和固定章节结构检查作为验证。
