# `pkg/domain/sysvar_cache.rs`

## 文件定位

本文件属于 `astersql-domain` crate；`pkg/domain/Cargo.toml` 以 `lib.rs` 为 crate 入口，而 `pkg/domain/lib.rs` 通过 `pub mod sysvar_cache` 公开此模块。它提供一个与具体 `Domain`、SQL 执行器和系统变量注册表解耦的 Rust 系统变量缓存核心：调用方提供持久化值来源、变量定义、配置覆盖值以及全局副作用回调，缓存负责生成 session/global 两份快照并提供查询。

当前接线状态必须与 Go 版本区分：仓库中的生产 Rust 文件没有引用 `SysVarCache`、`SysVarDefinition` 或 `SysVarSource`；直接使用者仅见于 `pkg/domain/sysvar_cache_test.rs` 和 `pkg/domain/main_test.rs`。因此它目前是由 `pkg/domain/lib.rs` 暴露且经测试验证的独立组件，尚未接入 Rust `Domain` 的启动、定时刷新或 etcd 通知链。文件第 16–304 行还保留了一段块注释形式的 Go 机械翻译草稿；可执行实现从 `use std::collections::BTreeMap` 开始，不能把草稿中的 `Domain`、session pool、restricted SQL 或 info cache 接线当成现状。

## 核心职责

- `SysVarDefinition` 把重建所需的系统变量元数据压缩为名称、默认值、是否跳过 session 初始化、是否具备 global 作用域、是否已由配置初始化五项。
- `SysVarSource` 抽象一次完整的持久化快照读取。生产实现若要对齐 Go，应在该 trait 的实现中读取 `mysql.global_variables`，但本文件本身不执行 SQL。
- `SysVarCache::rebuild_with_policy` 是完整重建算法：串行取源数据、应用有限的配置覆盖、按定义选择最终值、生成两份新 map、执行可选的全局校验/副作用，再发布缓存。
- `SysVarCache::rebuild` 提供简化入口：全局值不变换，且默认每个 global 定义都执行 `set_global`。
- `session_cache` 返回独立副本，供新 session 初始化；`global_var` 返回单个 global 值。

此实现不负责缓存时效、空缓存自动重建、跨节点通知、系统 session 借还、schema cache resize 或日志/指标。这些能力存在于 Go 的 `Domain` 运行链中，但未在当前 Rust 文件中接线。

## 主要符号

- `pub struct SysVarDefinition`：重建输入中的单项定义。`initialized_from_config` 为真时强制采用 `default_value`；`skip_session_init` 只影响 session map；`global_scope` 只决定是否写 global map 及是否进入回调路径。
- `pub trait SysVarSource`：唯一方法 `table_values(&self) -> Result<BTreeMap<String, String>, String>`。返回值代表持久化表的一次快照；错误会直接终止本次重建。
- `pub struct SysVarCache`：包含私有的 `global`、`session` 两个 `RwLock<BTreeMap<...>>`，以及串行化重建的 `Mutex<()>`。字段私有，外部只能通过公开方法读写。
- `SysVarCache::is_empty(&self) -> bool`：任一 map 为空就返回真；锁中毒时会 `expect` 并 panic。
- `SysVarCache::rebuild(...)`：便捷封装，将校验策略设为原值透传，将回调筛选策略设为恒真，然后委托给 `rebuild_with_policy`。
- `SysVarCache::rebuild_with_policy(...)`：核心策略入口。`validate_global` 只变换传给副作用回调的值；缓存中保存的仍是校验前值。`should_set_global` 同时承载 Go 中“存在 `SetGlobal`”与“未 `SkipSysvarCache`”的判定。
- `SysVarCache::session_cache(...)`：在持有 session 读锁时检查非空并克隆整个 `BTreeMap`。
- `SysVarCache::global_var(name)`：在 global 读锁下克隆命中值；缺失时报字符串错误。

文件没有条件编译项、模块级常量或自定义错误类型。所有业务错误均暂用 `String`，容器选用 `BTreeMap`，因而遍历顺序稳定，但其语义不依赖排序。

## 执行流程

`rebuild` 的流程是：

1. 调用 `rebuild_with_policy`，提供“值原样通过”的校验器、恒真的回调筛选器和调用方传入的 `set_global`。
2. `rebuild_with_policy` 首先取得 `rebuild_lock`，使从读取源快照到发布新 map 的整段流程串行，避免先开始的慢读取最后覆盖后开始的新读取。
3. 调用 `source.table_values()`。若返回错误，立即返回，现有缓存不变。
4. 遍历 `config_overrides`；只有源快照中已经存在的键才会被覆盖，配置不会凭空新增变量。
5. 对每个 `SysVarDefinition` 选值：若 `initialized_from_config` 为真，使用 `default_value`；否则使用表值，表中缺失时回退到 `default_value`。
6. 若未设置 `skip_session_init`，将值加入新 session map；若 `global_scope` 为真，将值加入新 global map。
7. global 定义通过 `should_set_global` 后，先由 `validate_global` 生成仅供回调使用的值，再调用 `set_global`。回调返回的错误被有意忽略，循环继续。
8. 全部定义和回调处理完毕后，先替换 session map，再替换 global map，并返回 `Ok(())`。

读取路径不触发上述流程。`session_cache` 若 session map 为空会返回 `"sysvar cache is empty"`；`global_var` 只查询当前 global map，空 map或缺失键均返回 `"unknown system variable: <name>"`。调用方必须显式先重建，这一点不同于 Go 的 `GetSessionCache`/`GetGlobalVar` 自动调用 `rebuildSysVarCacheIfNeeded`。

## 数据与状态

缓存状态由两份完整 map 组成：session map 是新会话初始化值集合，global map 是具备 global 作用域的变量集合。两者只在成功完成一次重建后整体赋值，不对旧 map做原地逐项修改，因此源读取失败或回调期间 panic 之前不会发布半构建 map。

值选择有三个重要不变量：配置初始化的定义始终使用其 `default_value`；普通定义遵循“表值优先、默认值兜底”；配置覆盖只作用于持久化快照已经包含的键。一个定义可以因 `skip_session_init` 不进入 session map，却因 `global_scope` 进入 global map；session-only 定义则相反。

`session_cache` 克隆 map，因此调用者修改返回值不会反向修改缓存，`pkg/domain/sysvar_cache_test.rs::rebuild_uses_defaults_config_overrides_and_returns_independent_session_copy` 对此有直接断言。`global_var` 返回克隆的 `String`，同样不暴露锁内引用。

## 依赖与调用关系

直接实现依赖仅来自标准库：`std::collections::BTreeMap`、`std::sync::{Mutex, RwLock}`。虽然所属 `astersql-domain` crate 在 `pkg/domain/Cargo.toml` 中声明了配置、DDL、session、KV 等大量依赖，本文件的可执行部分没有直接引用这些 crate，也没有 feature gate。

上游方面，`pkg/domain/lib.rs` 声明公开模块；RustCodeGraph 将 `rebuild_with_policy`、`session_cache`、`global_var` 识别为本文件函数，但未给出生产调用边。仓库文本引用进一步确认目前上游只有 `pkg/domain/sysvar_cache_test.rs` 与 `pkg/domain/main_test.rs`。下游方面，`rebuild` 调用 `rebuild_with_policy`；后者调用调用方实现的 `SysVarSource::table_values` 以及三个闭包策略，并操作内部锁和 map，没有直接 SQL、etcd 或 `Domain` 调用边。

Go 的真实应用主链可作为未来接线参照：`pkg/domain/domain.go::LoadSysVarCacheLoop` 启动时重建，随后等待 etcd key `/tidb/sysvars` 或 30 秒超时再重建；`NotifyUpdateSysVarCache` 通知其他节点并可同步刷新本地；`checkReplicaRead` 还直接读取 Go cache 的 global map。上述链路不是当前 Rust 实现的调用关系。

## 错误处理与边界

- `SysVarSource::table_values` 的 `Err(String)` 原样向上传播，且发生在发布前，旧缓存保持不变。
- `set_global` 的错误被刻意丢弃，以对齐 Go 的“记录错误但继续重建”行为；本文件没有日志接口，故错误完全不可观测。`pkg/domain/main_test.rs::canonical_sysvar_cache_keeps_values_when_set_global_callback_fails` 验证回调失败仍会成功发布。
- `session_cache` 会把 session 读锁中毒映射成 `"sysvar cache poisoned"`，并把空缓存视为错误；`global_var` 同样映射锁中毒，但不单独区分空缓存与未知变量。
- `is_empty`、重建互斥锁获取以及发布时的两个写锁均使用 `expect`；锁中毒会 panic，而不是返回 `Result`。`validate_global`、`should_set_global`、`set_global` 任一闭包 panic 也会展开并毒化重建锁。
- 重复定义名按输入顺序覆盖同一 map 键，并可能重复执行回调；本文件不去重、不校验空名称，也不判断未知表项。表中有而 definitions 中没有的键不会进入缓存。
- 回调值可经 `validate_global` 改写，但缓存值保持原始选值。测试 `rebuild_matches_go_validation_and_set_global_filtering` 验证了这一区别和筛选行为。

## 并发与资源生命周期

`rebuild_lock` 覆盖完整重建生命周期，包括源读取、配置覆盖、所有校验/副作用回调和两份 map 发布。因此同一实例不会同时执行两个 rebuild，解决 Go 注释所描述的 lost-update 场景；代价是慢 SQL 等源读取或慢回调会阻塞后续重建。

读操作只取得对应 map 的共享锁，重建的大部分计算使用局部 `BTreeMap`，不会长时间占用 map 写锁。发布时先取得 session 写锁并替换，再取得 global 写锁并替换。两份 map各自是整体替换，但二者不是在同一锁下原子发布：极短窗口内，并发读者可能观察到新 session 与旧 global 的混合代际。不要在文档或扩展代码中把“单 map整体替换”误称为“两份快照联合原子提交”。

`session_cache` 的深拷贝把数据生命周期交给调用者；锁在克隆完成后释放。这里没有后台任务、通道、事务、连接池或 session 借还；这些资源生命周期只能由未来 `SysVarSource` 和 Domain 接入层管理。

## 与 Go 版本的对应关系

`pkg/domain/sysvar_cache.go` 是直接语义基准。Rust `SysVarCache` 对应 Go `sysVarCache`，两个 `RwLock<BTreeMap>` 对应 Go 嵌入式 `RWMutex` 保护的两个 map，`rebuild_lock` 对应 `rebuildLock`。Rust definitions 循环复现了 `SkipInit`、`HasGlobalScope`、`IsInitedFromConfig`、relaxed validation、`SkipSysvarCache` 和 `SetGlobal` 的关键判定；`config_overrides` 的“仅覆盖已存在键”复现 `overrideSysVarWithConfig` 对 `MaxAllowedPacket` 的行为。

Rust 仍有明确差异：

- Go 方法属于 `Domain`，能借用 `sysSessionPool`、用 restricted SQL 读取 `mysql.global_variables`、在 Starter 模式读取真实配置，并在完成后 resize `infoCache`；Rust 将这些都留给调用方，且没有 info cache 回调。
- Go `GetSessionCache`/`GetGlobalVar` 在空缓存时自动重建；Rust getter 不重建。
- Go 使用结构化的 `variable.ErrUnknownSystemVar` 与日志；Rust 使用 `String`，且忽略回调错误时不记录日志。
- Go 的 `LoadSysVarCacheLoop`、etcd 通知和 30 秒刷新已接入应用；Rust 当前没有生产调用者。
- Go 用 `map[string]string`，Rust 用有序 `BTreeMap`。Go 用一个 `RWMutex` 同时保护两份 map并在一次临界区发布；Rust 使用两个锁顺序发布，因此跨 map 快照一致性更弱。

`pkg/domain/domain_test.go` 覆盖 Go 端重建重复执行 `SetGlobal` 回调的回归意图；Rust 的 `go_merge_43_rebuild_reapplies_unchanged_internal_summary_callback` 对应验证即使值未变化，每次 rebuild 仍应回调。

## 扩展指南

接入生产 Rust Domain 时，最可能新增的是本文件外的适配层，而不是改变核心算法：实现 `SysVarSource` 以 restricted SQL 读取 `mysql.global_variables`，把实际系统变量注册表转换为 `SysVarDefinition`，并将 relaxed validation、`SetGlobal`、`SkipSysvarCache` 接入两个策略闭包。还需在 Domain 生命周期中显式实现首次加载、定时/etcd 刷新、本地更新通知、内部 session 归还、指标和日志；不能仅实例化 `SysVarCache` 就声称完成 Go 行为对齐。

若新增定义字段或改变选值优先级，应修改 `SysVarDefinition` 与 `rebuild_with_policy`，并在独立的 `pkg/domain/sysvar_cache_test.rs` 增加边界用例，不要把测试内嵌回生产源文件。若改变便捷入口默认策略，应同时检查 `rebuild` 与 `pkg/domain/main_test.rs` 的回调失败语义。

涉及并发一致性时，应先决定是否需要 session/global 联合快照；若需要，应该改为单锁保护组合状态或引入代际快照，而不是简单调整两个赋值次序。任何锁结构变化都应新增独立并发测试，特别覆盖源读取失败、回调失败/panic、并发重建和并发读取。引入自动空缓存重建时还要避免多个读者触发重建风暴，并与 Go 的错误类型和日志行为保持兼容。

性能风险主要来自：每次 session 初始化克隆整张 map；`BTreeMap` 操作是对数复杂度；重建锁把外部读取与全部回调串行化。兼容风险主要来自错误字符串、回调筛选语义、配置覆盖仅作用于既有表键，以及 `initialized_from_config` 对表值的强制忽略。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库 `11467` 个文件，目标 `pkg/domain/sysvar_cache.rs` 已收录；`node --file pkg/domain/sysvar_cache.rs` 核对了全文件 452 行及可执行实现。
- RustCodeGraph `query`：确认 `SysVarCache` 位于第 331 行，`rebuild_with_policy` 位于第 377 行，`session_cache` 位于第 431 行，`global_var` 位于第 444 行；对这些符号执行 callers/callees 未得到生产调用边。
- 源与模块边界：[`pkg/domain/sysvar_cache.rs`](sysvar_cache.rs)、[`pkg/domain/lib.rs`](lib.rs)、[`pkg/domain/Cargo.toml`](Cargo.toml)。
- Rust 测试：[`pkg/domain/sysvar_cache_test.rs`](sysvar_cache_test.rs) 验证校验后回调、回调筛选、作用域拆分、默认值、配置覆盖、配置初始化、深拷贝和重复重建回调；[`pkg/domain/main_test.rs`](main_test.rs) 验证 `set_global` 返回错误仍发布缓存。
- Go 对照与真实接线：[`pkg/domain/sysvar_cache.go`](sysvar_cache.go)、[`pkg/domain/domain.go`](domain.go)；相关 Go 回归证据位于 [`pkg/domain/domain_test.go`](domain_test.go)。
- 仓库引用搜索：除目标文件外，生产 Rust 代码未引用三个公开类型；当前直接 Rust 使用点仅在上述两个独立测试文件。该结果与 RustCodeGraph 未发现生产 callers 相符。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工核对文档没有把注释草稿或 Go 的运行接线描述为当前 Rust 事实。
