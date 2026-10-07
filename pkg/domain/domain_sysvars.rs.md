# `pkg/domain/domain_sysvars.rs`

## 文件定位

本文件属于 `astersql-domain` crate。crate 入口 `pkg/domain/lib.rs` 以 `pub mod domain_sysvars` 暴露该模块，并在测试配置下从独立文件 `pkg/domain/domain_sysvars_test.rs` 装入单元测试。`pkg/domain/Cargo.toml` 指定 crate 根为 `lib.rs`；本文件自身只直接使用标准库的 `BTreeMap`、`OnceLock`、`RwLock`，以及同 crate 的 `crate::domain::Domain`。

它当前承担两种性质不同的内容：

1. 第 21–165 行是被逐行注释掉的 Go 迁移草稿，描述理想的 Domain 回调注册、PD client、oracle、统计缓存和资源控制接线；这些内容不参与 Rust 编译和运行。
2. 第 167–309 行是可编译实现：一个进程级 `DomainSysVars` 状态容器、五个系统变量名常量、类型化动态选项，以及返回全局容器的 `Domain::init_domain_sys_vars`。

因此，本文件当前是 Domain 系统变量副作用的“进程内状态模型”，不是 Go 版本真实外部副作用的完整移植。仓库搜索仅找到 `pkg/domain/domain_sysvars_test.rs` 对这些 API 的直接使用；未找到生产代码调用 `Domain::init_domain_sys_vars`，也未找到本文件向 `pkg/sessionctx/variable/tidb_vars.rs` 中 hook 槽注册回调的接线。

## 核心职责

- 以常量固定五个受支持的系统变量名，避免调用方重复书写字符串。
- 由 `set_pd_client_dynamic_option` 把字符串系统变量值解析为 `DynamicOption`，并保存到按名称索引的有序映射中。
- 保存统计缓存容量、外部时间戳、全局资源控制开关和 PD metadata 熔断阈值这四类标量状态。
- 通过 `DomainSysVars::global` 提供惰性初始化的进程级单例，通过 `Domain::init_domain_sys_vars` 暴露 Domain 侧入口。
- 以独立 Rust 测试锁定与 Go 版本相同的关键解析语义：毫秒到纳秒、布尔开关、RPC 模式大小写和标量不裁剪。

当前实现不会调用 PD client、store oracle、stats handle、resource-control hook 或 TiKV circuit-breaker 配置。名称中的 “PD client dynamic option” 表示所建模的数据语义，而非已经发生远端更新。

## 主要符号

- `TIDB_TSO_CLIENT_BATCH_MAX_WAIT_TIME`：批处理最大等待时间变量名；值按 `f64` 毫秒解析。
- `TIDB_ENABLE_TSO_FOLLOWER_PROXY`：TSO follower proxy 布尔开关变量名。
- `PD_ENABLE_FOLLOWER_HANDLE_REGION`：允许 PD follower 处理 Region 请求的布尔开关变量名。
- `TIDB_TSO_CLIENT_RPC_MODE`：TSO RPC 并发模式变量名。
- `TIDB_ENABLE_BATCH_QUERY_REGION`：批量查询 Region/router client 布尔开关变量名。
- `DynamicOption`：解析结果枚举。`DurationNanos(i64)` 表示有符号纳秒，`Bool(bool)` 表示开关，`Integer(usize)` 表示 RPC 并发度。
- `DomainSysVars`：状态容器。五个字段各自受独立 `RwLock` 保护；`options` 使用 `BTreeMap<String, DynamicOption>`。
- `DomainSysVars::global() -> &'static Self`：通过函数内 `OnceLock` 惰性创建并永久持有默认实例。
- `set_stats_cache_capacity` / `stats_cache_capacity`：原样写入和读取 `i64` 容量，不做范围校验。
- `set_pd_client_dynamic_option(&self, name, value) -> Result<(), String>`：动态选项的唯一解析与写入入口。
- `option(&self, name) -> Option<DynamicOption>`：克隆返回指定名称的当前值。
- `set_external_timestamp` / `external_timestamp`：原样写入和读取 `u64` 时间戳。
- `set_global_resource_control`：写入资源控制开关；当前没有对应 public getter，也没有触发 variable crate 的 enable/disable hook。
- `set_circuit_breaker_error_rate_ratio` / `circuit_breaker_error_rate_ratio`：原样写入和读取 `u32` 阈值。
- `tidb_opt_on`：私有解析辅助函数；仅 ASCII 大小写不敏感的 `ON` 或精确字符串 `1` 为真，其他字符串均为假。
- `Domain::init_domain_sys_vars(&self) -> &'static DomainSysVars`：返回全局容器。目前不使用 `self` 的内部状态，也不注册回调。

## 执行流程

动态 PD 选项写入流程如下：

1. 调用者向 `set_pd_client_dynamic_option` 提供变量名和字符串值。
2. 批等待变量用 `str::parse::<f64>` 解析毫秒数，再乘 `1_000_000.0` 并以 Rust 浮点到整数的 `as i64` 转换为纳秒。
3. 三个布尔变量共同调用 `tidb_opt_on`；不合法或未知的布尔文本不会报错，而会得到 `false`。
4. RPC 模式仅接受精确大写值：`DEFAULT`、`PARALLEL`、`PARALLEL-FAST`，分别映射为并发度 1、2、4；其他值立即返回错误。
5. 未知变量名立即返回 `Ok(())`，不创建 map 项。
6. 解析成功后获取 `options` 写锁，插入或覆盖对应键，再返回 `Ok(())`。解析失败发生在加锁前，旧值保持不变。

其他 setter 都是单锁、单字段赋值；getter 获取读锁并复制或克隆当前值。`DomainSysVars::global` 第一次调用时用 `Default` 构造全零/false/空 map 的实例，后续调用返回同一静态引用。`Domain::init_domain_sys_vars` 只是这个单例入口的薄封装。

## 数据与状态

`DomainSysVars` 的默认状态由派生的 `Default` 决定：动态选项 map 为空，三个整数标量为 0，资源控制为 `false`。状态没有磁盘持久化、网络同步或租户隔离；全局实例的值会持续到进程退出。直接构造 `DomainSysVars::default()` 则得到彼此隔离的实例，测试正采用这种方式避免污染全局状态。

五个字段使用不同的 `RwLock`，所以不同类别状态可并行访问，但本文件没有跨字段原子快照或事务语义。动态选项按变量名覆盖保存；`option` 返回克隆值，调用者不能借返回值修改内部 map。

值得注意的表示边界：等待时间以 `DurationNanos(i64)` 保存，保留 Go `time.Duration` 可表达负数这一点；`f64` 到 `i64` 的转换沿用 Rust `as` 的截断/饱和规则。RPC 并发度使用 `usize`，但当前只产生 1、2、4。统计容量和熔断比例均不裁剪，测试明确验证 `-7` 和 `101` 会原样保存。

## 依赖与调用关系

上游关系：

- `pkg/domain/lib.rs` 声明公开模块，并在 `#[cfg(test)]` 下声明 `domain_sysvars_test`。
- `pkg/domain/domain_sysvars_test.rs` 直接构造 `DomainSysVars::default()`，调用动态选项和标量 API。
- RustCodeGraph 对 `DomainSysVars` 找到结构体、`global` 和 `init_domain_sys_vars` 等符号；其文件视图标记本文件被测试及若干同名引用文件关联。对 `init_domain_sys_vars` 的 callers 查询在本次环境中超时，随后用全仓库精确文本搜索确认除定义外没有 Rust 调用点。

下游关系：

- 编译实现只依赖 `crate::domain::Domain` 和标准库集合/同步原语。
- `set_pd_client_dynamic_option` 的被调用逻辑完全在本文件内：数值解析、`tidb_opt_on` 和 map 写入。
- `pkg/sessionctx/variable/tidb_vars.rs` 已定义 stats、PD option、external timestamp、resource control、low-resolution TSO、schema cache 和 circuit breaker 等 hook 槽，但本文件没有导入或设置这些槽。

应用主链方面，Go 的 `pkg/domain/domain.go` 在 Domain 初始化中调用 `initDomainSysVars`；Rust Domain 主链未发现对应调用。因此当前 Rust API 是公开可调用但尚未接入 Domain 生命周期的局部实现，不能据此断言 SQL `SET GLOBAL` 已产生这些副作用。

## 错误处理与边界

- 批等待值无法解析为 `f64` 时，将标准解析错误转为 `String` 返回；map 不变。
- RPC 模式不匹配三个精确常量时，返回 `wrong value for {name}: {value}`；匹配大小写敏感。
- 未知变量名是有意的 no-op，并返回成功。这与 Go `switch` 无 default 错误的行为一致。
- 布尔值解析是宽松的：只有 `ON`（ASCII 大小写不敏感）和 `1` 为真，其余值包括任意文本均为假，不返回错误。
- 所有锁都通过 `expect("sysvar lock poisoned")` 处理 poisoning；持锁线程 panic 后的下一次访问会继续 panic，而不是返回可恢复错误。
- setter 不验证负容量、熔断比例上限、时间戳单调性或动态等待值是否有限。是否应在上游校验属于接线时必须核对的契约，不能在本文件中擅自增加与 Go 不同的裁剪。
- 资源控制字段只有 setter，且当前无可观察副作用；若需要读取或驱动 hook，应新增明确 API 和独立测试，而不是依赖私有字段。

## 并发与资源生命周期

`DomainSysVars::global` 的 `OnceLock` 保证并发首次访问只初始化一次；返回值具有 `'static` 生命周期，不会主动销毁或重置。每个字段的 `std::sync::RwLock` 保证跨线程共享时的数据竞争安全；读操作可并行，同一字段写操作互斥。

锁的临界区很短：标量访问只包含一次复制，map 写入只包含一次 `insert`，map 读取在锁内克隆一个枚举值。本文件不启动线程、异步任务、定时器或通道，也不拥有 PD/store 等外部资源。不同字段的更新顺序不构成一致性保证；若未来一个系统变量需要原子更新多个字段，应重新设计统一锁或显式事务状态，而不能依赖当前多个锁的偶然顺序。

全局单例会使测试间共享状态，因此现有测试使用局部 `Default` 实例。新增测试除非专门验证单例身份，否则也应使用局部实例，避免并行测试相互污染。

## 与 Go 版本的对应关系

Go 权威对照文件是 `pkg/domain/domain_sysvars.go`。对应关系与缺口如下：

- Go `initDomainSysVars` 把多个 Domain 方法注册到 `variable` 全局回调；Rust `init_domain_sys_vars` 只返回全局 `DomainSysVars`，没有注册行为。
- Go `setStatsCacheCapacity` 调用真实 `StatsHandle`，且 handle 为空时 no-op；Rust 只把容量写入内存字段。
- Go `setPDClientDynamicOption` 更新真实 PD client，并在部分分支同步 `vardef` 原子状态；Rust 只保存 `DynamicOption`。两者对毫秒转换、布尔判断、RPC 模式并发度和未知变量 no-op 的解析语义基本一致。
- Go `updatePDClient` 对非 PD store 或 nil client 返回成功，否则传播 `UpdateOption` 错误；Rust 可编译实现没有该函数和外部错误传播面。
- Go external timestamp 与 low-resolution TSO 间隔通过 store oracle；Rust 只实现 external timestamp 的本地标量，没有 low-resolution TSO API。
- Go resource control 调用 enable/disable 函数；Rust 只保存 bool。
- Go schema cache、external-workload TTL job 和 circuit-breaker 更新均有真实组件接线；Rust只对 circuit-breaker ratio 保存标量，另两项未实现。

文件前半的注释草稿覆盖了较早的 Go 形状，但当前 Go 文件还包含 `UpdateExternalWorkloadTTLJobEnable` 接线；故扩展时应重新读取 Go 源码，不能把注释草稿当作最新实现。

## 扩展指南

若只增加一种进程内动态选项，应同步修改变量名常量、`DynamicOption`（如需新类型）、`set_pd_client_dynamic_option` 分支，并在 `pkg/domain/domain_sysvars_test.rs` 添加正常值、非法值、覆盖旧值和未知名行为的测试。不要把测试写入生产 `.rs` 文件。

若目标是补齐 Go 的真实副作用，应从 `Domain::init_domain_sys_vars` 接线开始，复用 `pkg/sessionctx/variable/tidb_vars.rs` 已有 hook 注册 API，并把闭包生命周期、`Arc` 所有权和 Domain 销毁语义设计清楚；随后分别接入 stats handle、PD client、store oracle、resource control 与 circuit breaker 的实际 Rust 抽象。每项接线都应保留 Go 的 no-op 条件和错误传播，不应为了编译而以状态字段代替外部行为。

安全扩展时重点检查：

- 兼容性：系统变量名、RPC 模式大小写、毫秒到纳秒单位、未知变量 no-op 和布尔解析必须与 Go/variable 层契约一致。
- 正确性：外部更新失败时不得提前提交本地镜像，否则会造成状态与 PD/oracle 分裂。
- 并发：hook 注册与替换、Domain 生命周期、全局单例和并行测试之间不能形成悬垂引用或状态串扰。
- 性能：高频读取应避免无必要的 String 分配；若选项集合增大，应评估单一 map 锁竞争。
- 测试：继续放在独立的 `pkg/domain/domain_sysvars_test.rs`；真实接线还需要覆盖 hook 调用、外部错误传播、非 PD store/nil client no-op，以及 Domain 初始化确实注册回调。

## 验证依据

本说明基于以下直接证据：

- `pkg/domain/domain_sysvars.rs`：RustCodeGraph 文件节点返回完整 1–309 行；核对了常量、枚举、结构体、所有方法、私有解析函数和注释迁移草稿。
- `pkg/domain/domain_sysvars_test.rs`：三项独立测试验证毫秒/纳秒换算、负 duration、三类布尔 false、RPC 模式大小写及并发度、标量不裁剪。
- `pkg/domain/domain_sysvars.go`：核对 Go 初始化注册、stats、PD option、resource control、external workload、oracle、PD client no-op 和 circuit breaker 行为。
- `pkg/domain/domain.go:628`：Go Domain 初始化调用 `do.initDomainSysVars()`。
- `pkg/domain/lib.rs`：核对模块公开性和独立测试模块声明。
- `pkg/domain/Cargo.toml`：核对 crate 名、`lib.rs` 入口、Go package 元数据和依赖边界；当前实现本身未使用列出的外部 crate。
- `pkg/sessionctx/variable/tidb_vars.rs`：核对 Rust hook 槽已存在，但目标文件尚未注册这些槽。
- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`query DomainSysVar` 返回 `DomainSysVars`、`init_domain_sys_vars` 和 `global` 等候选。`callers init_domain_sys_vars` 查询超时无结果，故没有把该空输出当作无调用证据，改用 `rg -n "init_domain_sys_vars\\(" --glob '*.rs' .` 精确复核，仅命中定义本身。
- 结构验证使用任务指定命令，要求文件存在且恰好含有上述 11 个固定二级标题。

人工复核结论：本文区分了可编译实现与注释草稿，能够回答文件为何存在、当前如何运行、哪些 Go 行为尚未接线，以及新增状态模型或真实副作用时应修改和测试的位置。
