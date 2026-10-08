# `pkg/plugin/plugin.rs` 逻辑说明

## 文件定位

`pkg/plugin/plugin.rs` 是 `astersql-plugin` crate 的插件运行时核心。crate 入口 `pkg/plugin/lib.rs` 将本模块公开并重导出其 API；清单与回调模型来自同 crate 的 `audit.rs`、`helper.rs`、`spi.rs`、`const.rs` 和 `errors.rs`。本文件负责把插件清单从静态工厂、测试钩子或动态加载抽象装入内存，完成版本校验和生命周期切换，并向 session/executor 等上层提供查询、遍历和跨节点 Flush 能力。

当前 Rust 接线中，`pkg/plugin/helper.rs::load_plugin_for_test` 会串联 `register_static_plugin`、`load` 和 `init`；`pkg/session/runtime/scan_adapter_runtime.rs` 通过 `foreach_plugin(Kind::Audit, ...)` 派发 SQL 完成事件。`pkg/executor/admin_plugins.rs` 只定义 `PluginFlagFlusher` 边界来表达 Go 侧 `ChangeDisableFlagAndFlush` 语义，未在该文件中直接调用本模块的具体函数。

## 核心职责

1. `Config` 描述插件 ID、动态库目录、失败策略、环境版本、键值客户端和加载器；`load` 据此建立新的 `Plugins` 集合。
2. `load_one` 以“静态注册工厂 → 测试钩子 → `PluginLoader`”的顺序取得 `Manifest`，并校验清单名称及（动态来源的）版本。
3. `Plugin::validate` 校验 `Manifest::require_version`，随后执行可选 `validate` 回调；`init` 执行 `on_init`、安装 `FlushWatcher` 并把成功实例置为 `State::Ready`。
4. `get`、`get_all`、`get_by_name`、`foreach_plugin` 和 `is_enabled` 提供运行期只读访问，其中业务遍历只暴露 Ready 且未禁用的插件。
5. `notify_flush` 与 `change_disable_flag_and_flush` 通过 `KeyValueClient::put` 写 `/tidb/plugins/{name}`，使节点间的禁用状态和 `on_flush` 回调由 watcher 同步。
6. `shutdown` 从全局槽位取走集合、取消 watcher、执行 `on_shutdown`，避免单个关闭回调失败阻断后续插件清理。

## 主要符号

- `ManifestFactory`：线程安全的静态清单工厂，调用后返回一个拥有所有权的 `Manifest`。
- `TestLoadHook`：测试专用加载替身，输入插件目录与 `Id`，返回清单或 `PluginError`。
- `PluginLoader::load_manifest`：动态库加载边界；调用者传入构造出的库路径和 `MANIFEST_SYMBOL`。本 crate 的 `Cargo.toml` 没有普通外部依赖，实际动态加载机制由注入实现承担。
- `KeyValueClient::{get, put, watch}`：PD/etcd 行为的最小抽象。`watch` 一次返回一批事件结果，而非暴露永久流。
- `Config`：加载参数。`plugins` 中元素应为 `name-version`；`skip_when_fail` 决定重复、加载、校验和初始化失败是跳过/禁用还是立即返回。
- `FlushWatcher`：持有独立 `Context`、键路径、键值客户端、清单、共享禁用原子值和取消标志。`new` 生成默认 Context，`with_context` 用于测试取消语义。
- `Plugin`：单个已加载实例；包含 `manifest`、计算出的 `path`、`Arc<AtomicU32>` 禁用标志、生命周期 `state` 和可选 watcher。克隆实例会共享禁用原子值以及 watcher 内部的 `Arc` 状态。
- `Plugins`：全局集合快照，按 `Kind` 保存插件，同时维护环境/插件版本表和 `dying_plugins`。`clone_plugins` 复制容器，嵌套的 `Arc` 仍按 Rust `Clone` 语义共享。
- `global_plugins`、`static_plugins`、`test_hook`：分别由 `OnceLock<RwLock<...>>` 延迟初始化的全局槽位、静态工厂表和测试钩子槽位。
- 生命周期入口：`load`、`load_one`、`init`、`shutdown`。
- 查询与控制入口：`get`、`foreach_plugin`、`is_enabled`、`get_all`、`get_by_name`、`notify_flush`、`change_disable_flag_and_flush`。

## 执行流程

加载阶段由 `load(context, config)` 开始：它先复制 `environment_versions`，然后逐个用 `Id::decode` 解析配置项。插件名若已在版本表中即视为重复；严格模式返回 `DuplicatePlugin`，跳过模式忽略该项。非重复项交给 `load_one`，成功后由 `Plugins::add` 同时写入种类列表和版本表。全部装载完毕后，`Plugin::validate` 用完整版本表做交叉依赖检查并执行清单回调；跳过模式把校验失败项标成 `Disable`，严格模式终止。只有整轮成功，集合才写入 `global_plugins`。

`load_one` 先解析名称和版本文本，再无条件计算 `<plugin_dir>/<id><LIBRARY_SUFFIX>`。同名静态工厂优先，且静态清单特意不比较 ID 中的版本；否则尝试全局测试钩子，再尝试配置的 `PluginLoader`，都不存在时返回模拟 Go `plugin.Open` 的清单错误。取得清单后总是比较导出名称，动态/测试来源还比较数值版本的十进制文本，最后以 `Uninitialized` 状态构造 `Plugin`。

初始化阶段由 `init(context, config)` 在全局写锁内遍历插件。存在 `on_init` 时先调用；失败在跳过模式下置 `Disable` 并继续，否则返回。清单含 `on_flush` 且配置了键值客户端时，创建与插件禁用原子值共享状态的 `FlushWatcher`，先同步读取一次禁用键并调用 `on_flush`，然后启动后台 watch 线程。首次刷新失败时，跳过模式仍启动 watcher、保留 `Disable` 并继续；严格模式立即返回。走完整条成功路径后状态才变为 `Ready`。

运行期，`foreach_plugin` 在全局读锁下按种类顺序调用 Ready 且 `disabled != 1` 的实例，并原样传播首个回调错误；`is_enabled` 做同样谓词的存在性判断。`notify_flush` 只把当前原子标志写入键值存储；`change_disable_flag_and_flush` 先改本地原子标志，再写入键值存储。两者都先通过 `supports_flush` 检查存在性、Ready 状态和 watcher。

关闭阶段，`shutdown` 在全局写锁下用 `take` 原子式地把槽位置空，然后不持有全局锁遍历本地集合。每项先置 `Dying`，取消 watcher，再调用可选 `on_shutdown`；回调错误被忽略以继续清理。代码把克隆项追加到本地 `dying_plugins`，但集合随后离开作用域，当前没有公开 API 读取这批关闭记录。

## 数据与状态

生命周期主线是 `Uninitialized → Ready`；校验、初始化或首次刷新在 `skip_when_fail` 模式下可转为 `Disable`；关闭时转为 `Dying`。`foreach_plugin`/`is_enabled` 同时检查生命周期和独立禁用标志，因此 `State::Disable` 与运行时 `disabled == 1` 是不同维度，但都会让插件不参与业务回调。

`Plugins::versions` 初始包含环境组件版本，随后按插件逻辑名加入已成功装载的插件版本。这使后装插件和最终交叉校验都能看到所有已装载版本。名称重复判断也复用该表，因此插件名若与环境组件键冲突，会被当成重复项。

禁用状态使用 `Arc<AtomicU32>`，值恰为 `1` 才表示禁用；读取使用 `Acquire`、写入使用 `Release`。`get` 和 `get_all` 返回克隆的 `Plugin`/容器，因此普通字段是快照，而 `disabled` 和克隆出的 `FlushWatcher` 仍共享原子与取消状态。`state` 是按值复制的快照，调用者修改返回值不会回写全局集合。

键路径固定为 `/tidb/plugins/{plugin_name}`。`get_plugin_disabled_flag` 仅把字符串 `"1"` 解释为禁用；键不存在或其它内容均为启用。`refresh_plugin_state` 先更新原子值，再调用 `on_flush`，所以即使回调报错，禁用状态也已经改变。

## 依赖与调用关系

下游依赖均来自标准库和 crate 内重导出：路径拼装用 `std::path::Path`，共享表用 `HashMap`，全局初始化与并发访问用 `OnceLock`/`RwLock`，禁用与取消用原子类型，watch 辅助用线程、时长和 MPSC 接收端；领域类型为 `Context`、`Id`、`Kind`、`Manifest`、`State`、`PluginError` 及常量 `LIBRARY_SUFFIX`、`MANIFEST_SYMBOL`。

内部主要调用边为：`load → Id::decode / load_one / Plugins::add / Plugin::validate / global_plugins`；`load_one → get_static_plugin / test_hook / PluginLoader::load_manifest / Plugin::new`；`init → FlushWatcher::new / refresh_plugin_state / watch_loop`；`watch_loop → watch_loop_once → KeyValueClient::watch / refresh_plugin_state`；`refresh_plugin_state → get_plugin_disabled_flag → KeyValueClient::get`；`notify_flush` 与 `change_disable_flag_and_flush → get_by_name → get_all → supports_flush → KeyValueClient::put`。

已核实的直接上游包括 `pkg/plugin/helper.rs::load_plugin_for_test` 对注册、加载和初始化入口的组合，`pkg/session/runtime/scan_adapter_runtime.rs` 对 `foreach_plugin` 的审计事件派发，以及 `pkg/session/runtime_test/typed_adapter_bridge.rs` 的静态插件集成接线。Cargo 侧 `pkg/session/Cargo.toml`、`pkg/server/Cargo.toml`、`pkg/executor/Cargo.toml` 和 `pkg/server/tests/commontest/Cargo.toml` 声明对 `astersql-plugin` 的路径依赖；本 crate 自身仅声明测试依赖 `serial_test = "3"`。

## 错误处理与边界

可识别错误包括：ID 无法解码、名称重复、无加载来源/动态打开失败、导出名称不匹配、动态版本不匹配、要求版本不足、生命周期回调失败、键值存储失败和锁中毒。严格模式在第一个相关错误处返回；`skip_when_fail` 只对重复、单项加载、校验、`on_init` 与首次刷新提供降级，不会把 `Id::decode` 失败吞掉，也不能掩盖最终全局写锁中毒。

`watch_loop_once` 会传播建立 watch 的错误，但逐事件只在 `event.is_ok()` 时刷新；事件自身错误被跳过，刷新错误也故意忽略，让后续事件仍有机会处理。`watch_loop` 对 watch 关闭和错误统一等待五秒再建；其 `thread::sleep` 不能被取消立即唤醒。`watch_loop_with_chan` 是用于对齐 Go 通道退出语义的测试辅助：取消返回 `true`，发送端断开返回 `false`，刷新错误同样忽略。

全局注册表和测试钩子的清理函数在锁中毒时静默不做；查询函数则通常把锁中毒降级为 `None`/`false`。相对地，改变生产状态的注册、加载、初始化和遍历入口会返回后端错误。`notify_flush`/`change_disable_flag_and_flush` 中的 `expect` 依赖刚完成的 `supports_flush` 检查；因为操作对象是局部克隆，该断言在函数内部没有异步替换窗口。

本文件不负责解析动态库 ABI、持有真实库句柄或回滚已经执行的插件回调；这些能力分别留给 `PluginLoader` 实现和上层生命周期编排。严格模式下若 `init` 中途失败，先前插件可能已经运行过 `on_init` 或已启动 watcher，调用者需要通过 `shutdown` 做后续清理。

## 并发与资源生命周期

全局插件集合由 `RwLock<Option<Plugins>>` 保护。`load` 在所有装载和校验完成后一次性替换全局集合；若此前已有集合，本函数不会先执行其 `on_shutdown`。`init` 持有写锁期间调用用户提供的 `on_init`/首次 `on_flush`，`foreach_plugin` 持有读锁期间调用业务回调；扩展回调若重入需要获取同一全局锁的 API，存在阻塞风险，应避免这种调用结构。

每个带 Flush 的插件由 `init` 启动一个脱离式 `std::thread`。线程拥有 watcher 克隆并循环重建 watch；`shutdown` 只设置两个取消信号，不保存 `JoinHandle`、不等待线程结束。若线程正处于键值客户端的同步 `watch` 调用或五秒退避睡眠，它只能在调用返回或睡眠结束后观察取消。

`FlushWatcher::cancel` 同时设置 `AtomicBool` 并取消内部 `Context`。`watch_loop_once` 在建 watch 前及处理每个事件前检查两者；`watch_loop_with_chan` 每毫秒超时一次来轮询取消。禁用原子由全局插件、返回给调用者的插件克隆和 watcher 共享，允许 Flush 更新被查询路径观察到。

静态注册与测试钩子也是进程级状态。`pkg/plugin/plugin_test.rs` 用 `serial_test` 和 `reset_plugin_globals` 隔离会修改这些槽位的测试，说明并行测试或多个独立运行时若共享进程，必须自行协调注册、加载和清理。

## 与 Go 版本的对应关系

主要参照是 `pkg/plugin/plugin.go`。Rust 的 `Config`、`Plugin`、`Plugins`、加载/校验/初始化/关闭、静态注册表、查询遍历、`flushWatcher` 和两个 Flush 控制函数都保留了 Go 的总体顺序和关键分支；`pkg/plugin/plugin_test.rs` 对照 `pkg/plugin/plugin_test.go` 覆盖静态优先、成功加载、跳过失败、严格失败、集合克隆及 watch 退出。

存在以下实现层差异：Go 直接使用 `plugin.Open`/`Lookup` 并在 `Plugin` 中保留库句柄，Rust 通过注入的 `PluginLoader` 抽象动态加载且不保存句柄；Go 通过原子指针和 copy-on-write 容器发布全局状态，Rust 使用 `OnceLock<RwLock<Option<Plugins>>>`；Go watcher 使用 etcd 的持续 `WatchChan`，Rust 的 `KeyValueClient::watch` 返回事件批次并由外层循环重建；Go 用 `util.WithRecovery` 包裹 goroutine，Rust 后台线程没有 panic 恢复封装；Go 的 `NotifyFlush` 从 `domain.Domain` 获取 etcd 客户端，Rust 直接复用 watcher 中注入的客户端。

Rust `shutdown` 会先从全局槽位取走集合再执行回调，Go 则遍历后用 CAS 清空指针；Rust 因而不会让并发查询继续看到正在关闭的集合。Rust 还把关闭项克隆进本地 `dying_plugins`，但目前没有持久化该集合。静态插件跳过版本文本校验、回调错误的跳过策略、Ready/disabled 过滤以及 watcher 键格式与 Go 意图一致。

## 扩展指南

新增加载来源时，应在 `load_one` 中明确其相对静态工厂、测试钩子和 `PluginLoader` 的优先级，并在独立的 `pkg/plugin/plugin_test.rs` 增加名称、版本、错误映射和优先级回归；不要把测试写入生产源文件。若要接入真实动态库，应实现 `PluginLoader`，同时明确库句柄的存活期、符号 ABI 和卸载限制，不能只返回一个脱离库资源的清单。

新增生命周期状态或失败策略时，需要同步检查 `Plugin::validate`、`load`、`init`、`shutdown`、`foreach_plugin`、`is_enabled` 和 `supports_flush` 的谓词，并与 `pkg/plugin/const.rs` 及 Go 对照保持一致。尤其要测试严格模式的部分初始化清理、跳过模式下 Disable 状态，以及返回克隆的状态/原子共享语义。

扩展 Flush 协议时，应同时审视 `KeyValueClient`、`FlushWatcher::{refresh_plugin_state, watch_loop_once, watch_loop_with_chan, cancel}` 和两个写入入口。需要保持“先更新禁用值、再执行回调”的既有顺序，决定事件级错误是否继续忽略，并为取消延迟、watch 重连和写入失败补充 `plugin_test.rs` 中的独立测试。若改变键格式或值编码，还必须考虑与 Go 节点的混合集群兼容性。

新增上层消费者时，优先通过 `foreach_plugin` 使用 Ready 且启用的实例，而不是从 `get_all` 快照自行复制过滤规则。回调不得在持有全局锁的调用路径中重入插件注册、load/init/shutdown 等写操作；若确需支持重入，应先重构锁与回调边界并增加并发回归测试。性能上需注意 `get_all` 会克隆整个按种类映射，跨种类单次查找 `get_by_name` 也是线性扫描。

## 验证依据

- 源码全量阅读：`pkg/plugin/plugin.rs`；crate 边界与重导出：`pkg/plugin/lib.rs`、`pkg/plugin/Cargo.toml`。
- Go 语义对照：`pkg/plugin/plugin.go`；Rust/Go 独立测试对照：`pkg/plugin/plugin_test.rs`、`pkg/plugin/plugin_test.go`。
- 直接入口与消费者：`pkg/plugin/helper.rs::load_plugin_for_test`、`pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/session/runtime_test/typed_adapter_bridge.rs`、`pkg/executor/admin_plugins.rs`，以及 session/server/executor 的 Cargo 路径依赖声明。
- RustCodeGraph 状态显示索引覆盖 11,467 个文件、307,296 个节点，`pkg/plugin/plugin.rs` 被识别为含 60 个符号；精确查询定位了 `load_one`（第 402 行）、`load_one_from_dir`（第 445 行）、`foreach_plugin`（第 540 行）和 `change_disable_flag_and_flush`（第 632 行）。本次 `explore`、按文件 `node` 及批量 callers/callees 未返回完整可用调用边，因此调用关系另由上述源文件和 `rg` 结果核实，未把空图输出当成事实。
- 测试证据表明：静态工厂优先且不校验 ID 版本；成功 load/init 后查询与遍历可见；`skip_when_fail` 会跳过重复/缺失项并屏蔽 Disable 插件；严格模式返回错误；克隆容器不受后续容器修改影响；watch 通道取消与断连返回值不同；初始化会执行首次刷新并启动 watcher 处理后续事件。
- 本任务只新增说明文档，按计划不运行 Cargo。交付前使用任务文件规定的命令确认目标文档存在且恰有十一个固定二级章节，并人工复核所有行为陈述均能回指到上述符号或文件。
