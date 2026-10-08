# `pkg/testkit/testenv/testenv.rs`

## 文件定位

本文件是独立 crate `astersql-testkit-testenv` 的行为实现，crate 根 [`lib.rs`](./lib.rs) 通过 `mod testenv` 加载它，并用 `pub use testenv::*` 暴露全部公开符号。其职责位于测试基础设施层：一部分为 Rust 测试辅助代码提供统一的并行度上限，另一部分在 nextgen 测试开始时临时切换进程级配置。它不是 SQL 请求、规划或执行主链的一环，也不修改生产请求的业务数据。

[`Cargo.toml`](./Cargo.toml) 将该 crate 映射到 Go 包 `pkg/testkit/testenv`，并声明三个直接依赖：`astersql-config`、`astersql-dxf-framework-handle` 和 `astersql-keyspace`。工作区中 `pkg/session`、`pkg/session/test/bootstraptest`、`pkg/dxf/framework/handle`、`pkg/testkit`、`pkg/server/internal/testserverclient`、`pkg/server/tests/servertestkit` 声明了对本 crate 的依赖；但仓库文本检索未发现这些 crate 的 Rust 源码直接引用本文件 API，因此依赖声明不能当作已接线调用的证据。当前能确认的 Rust 直接使用者是独立测试 [`testenv_test.rs`](./testenv_test.rs)，以及本文件内部的 snake_case 别名转发。

## 核心职责

1. `TEST_MAX_PROCS` 在进程内保存测试辅助代码应采用的最大并行度，初始值与 `SetGOMAXPROCSForTest` 写入值均为 `min(available_parallelism, 16)`；查询失败时退化为 `1`，因此正常 API 路径下结果始终处于 `1..=16`。
2. `UpdateConfigForNextgen` 将全局配置的 `keyspace_name` 改为 `keyspace::System`（当前常量值为 `"SYSTEM"`），并将 `instance.tidb_service_scope` 改为 `handle::NEXT_GEN_TARGET_SCOPE`（当前值为 `"dxf_service"`）。
3. 修改 nextgen 配置前保存完整配置快照，并通过调用方提供的 `TestContext::cleanup` 注册恢复闭包，使测试结束、提前返回或由测试框架捕获的 panic 路径可以恢复旧配置。
4. 保留与 Go 导出函数一致的 PascalCase API，同时提供 Rust 常用 snake_case 别名；本 crate 根允许 `non_snake_case`，所以两套名称都是公开接口。

该文件只设置“测试辅助选择 worker 数时读取的值”。Rust 没有在这里修改运行时或操作系统调度器；`SetGOMAXPROCSForTest` 并不等价于实际调用 Go `runtime.GOMAXPROCS`。

## 主要符号

| 符号 | 可见性与签名 | 语义 |
| --- | --- | --- |
| `MAX_TEST_PROCS` | 私有常量，`usize = 16` | 测试并行度硬上限。 |
| `TEST_MAX_PROCS` | 私有 `LazyLock<AtomicUsize>` | 进程级、惰性初始化的并行度值；初始化失败时取 `1`。 |
| `TestContext` | 公开 trait | 抽取 Go `testing.TB` 在本文件所需的最小能力：`helper(&self)` 与 `cleanup(&self, Box<dyn FnOnce() + Send + 'static>)`。 |
| `SetGOMAXPROCSForTest()` | 公开函数 | 重新读取系统可用并行度并以 `SeqCst` 写入 `TEST_MAX_PROCS`。 |
| `MaxProcsForTest() -> usize` | 公开函数 | 以 `SeqCst` 读取已保存的并行度上限。 |
| `UpdateConfigForNextgen(&dyn TestContext)` | 公开函数 | 标记 helper、注册完整配置恢复闭包，再安装 SYSTEM keyspace 和 nextgen DXF scope。 |
| `ServiceScopeForTest() -> String` | 公开函数 | 从当前全局配置复制并返回服务作用域字符串。 |
| `set_gomaxprocs_for_test`、`max_procs_for_test`、`update_config_for_nextgen`、`service_scope_for_test` | 公开函数 | 对应 PascalCase 函数的零附加逻辑转发。 |

文件没有结构体、枚举、宏、条件编译项或返回 `Result` 的函数。条件编译只出现在 crate 根：`testenv_test` 模块仅在 `#[cfg(test)]` 下加载。

## 执行流程

并行度路径如下：

1. 首次读取或写入 `TEST_MAX_PROCS` 时，`LazyLock` 调用 `std::thread::available_parallelism()`。
2. 成功时把非零并行度转为 `usize`，失败时使用 `1`，最后用 `MAX_TEST_PROCS` 截断到最多 `16`。
3. `SetGOMAXPROCSForTest` 可重复执行相同计算并原子覆盖旧值；`MaxProcsForTest` 只原子读取。
4. snake_case 入口仅转发，不改变顺序、错误处理或内存序。

nextgen 配置路径如下：

1. `UpdateConfigForNextgen` 先调用 `t.helper()`，让具体测试框架把该层标记为辅助帧。
2. 通过 `config::get_global_config()` 获取 `Arc<Config>` 快照，并克隆为拥有所有权的 `Config`。
3. 把捕获该快照的 `FnOnce + Send + 'static` 闭包注册给 `t.cleanup`；闭包执行时调用 `config::store_global_config(previous_config)`，恢复整个旧配置，而不只是两个被修改字段。
4. 调用 `config::update_global`。该下游函数采用“获取当前快照、克隆、闭包修改、整体写回”流程；本文件的闭包只设置 `keyspace_name` 与 `instance.tidb_service_scope`。
5. 测试期间可用 `ServiceScopeForTest` 读取当前 scope；测试上下文执行 cleanup 后，进程级配置恢复为步骤 2 保存的完整快照。

独立测试 `update_config_for_nextgen_updates_and_restores_the_go_config_fields` 直接验证了 helper 被调用、两个字段被更新，以及显式运行 cleanup 后两个字段恢复。

## 数据与状态

本文件管理两类进程级状态：

- `TEST_MAX_PROCS` 是单个 `AtomicUsize`。它不保存原值栈，也没有每测试隔离；后一次设置会覆盖前一次设置。当前值只能通过 `MaxProcsForTest` 读取。
- nextgen 设置作用于 `astersql-config` 的全局 `Config`。下游 `pkg/config/config.rs` 使用 `RwLock<Arc<Config>>` 保存快照，`get_global_config` 克隆 `Arc`，`store_global_config` 整体替换配置并同步重建错误消息扩展缓存，`update_global` 则复制后修改再写回。

恢复闭包拥有调用时的完整 `Config` 克隆。因此 cleanup 的恢复语义是“回到调用前完整快照”，而非“只撤销 keyspace 和 scope 两个字段”。如果多个上下文交错修改同一全局配置，后执行的 cleanup 可能覆盖期间发生的其他全局更新；调用方必须串行化这类测试或确保生命周期不重叠。独立 Rust 测试为此使用私有 `GLOBAL_CONFIG_LOCK`，但该锁并非生产 API，也不会自动约束其他测试模块。

## 依赖与调用关系

上游关系：

- [`lib.rs`](./lib.rs) 重新导出本文件全部公开符号。
- [`testenv_test.rs`](./testenv_test.rs) 直接调用 `SetGOMAXPROCSForTest`、`MaxProcsForTest` 和 `UpdateConfigForNextgen`，并实现 `TestContext` 记录 helper 与 cleanup。
- 四个 snake_case 函数分别调用对应 PascalCase 函数。
- RustCodeGraph 将本文件索引为 16 个符号；对关键函数执行精确 `callers` 查询未返回跨文件调用边。局部 `rg` 同样未发现测试以外的 Rust 调用，因此本文不把 Cargo 依赖方描述为实际调用方。

下游关系：

- `std::thread::available_parallelism` 提供当前系统可用并行度。
- `AtomicUsize::{store, load}` 使用 `Ordering::SeqCst` 发布/读取测试并行度。
- `config::{get_global_config, store_global_config, update_global}` 读取、恢复和更新全局配置。
- `keyspace::System` 提供 SYSTEM keyspace 名称。
- `handle::NEXT_GEN_TARGET_SCOPE` 提供 nextgen DXF scope。
- `TestContext` 的实现方控制 cleanup 何时运行；本文件只注册，不执行清理。

Go 侧直接调用点可在 `pkg/session/testutil.go`、`pkg/session/test/bootstraptest/bootstrap_upgrade_test.go`、`pkg/server/internal/testserverclient/server_client.go`、`pkg/server/tests/servertestkit/testkit.go`、`pkg/testkit/mockstore.go`、`pkg/testkit/testkit.go`、`pkg/dxf/framework/handle/handle_test.go`、`pkg/store/mockstore/mockcopr/executor_test.go` 和 `pkg/store/mockstore/mockstore.go` 中找到。这些调用证明 Go 辅助函数的使用场景，但不证明相应 Rust 调用已经迁移完成。

## 错误处理与边界

- 获取可用并行度失败不会向上返回错误，而是保守退化为 `1`；上限固定为 `16`。
- 本文件没有 `Result` 或显式错误类型。配置函数的锁中毒行为由下游实现决定：`pkg/config/config.rs` 使用 `expect`，锁中毒时会 panic。
- `TestContext::cleanup` 没有返回值，因此本文件不能确认注册是否成功；正确恢复依赖实现方遵守 trait 契约并最终运行闭包。
- cleanup 闭包要求 `Send + 'static`，可以被测试框架持有或跨线程调度；但 `TestContext` trait 本身没有要求 `Send` 或 `Sync`。
- `UpdateConfigForNextgen` 不检测调用是否嵌套，也不合并恢复快照。嵌套调用是否正确取决于 cleanup 的执行顺序；常见的后进先出顺序可逐层恢复，其他顺序可能恢复到过旧状态。
- 全局配置更新没有在本文件层面持有覆盖“保存旧值—安装新值—执行 cleanup”整个生命周期的锁，因此并行测试必须由调用方隔离。
- `ServiceScopeForTest` 返回拥有所有权的 `String` 克隆，调用方修改返回值不会改动全局配置。

## 并发与资源生命周期

并行度状态使用 `AtomicUsize`，所有访问均为 `SeqCst`，因此读写不存在数据竞争，并在所有线程间提供单一全序；该强内存序偏保守，但此状态访问频率仅限测试辅助路径。`LazyLock` 保证初始化只发生一次；随后 `SetGOMAXPROCSForTest` 可并发覆盖，最终值取决于最后一次原子写入，而每次写入的计算规则相同。

全局配置由下游 `RwLock<Arc<Config>>` 保护单次读写，但 `UpdateConfigForNextgen` 的跨步骤事务不是原子的。资源生命周期由 `TestContext` 承担：注册时，旧 `Config` 移入 boxed cleanup；执行一次后，`FnOnce` 被消费并释放快照。如果实现方丢弃闭包而不调用，nextgen 配置会继续留在进程级状态中，污染后续测试。

本文件不创建线程、异步任务、channel、文件、网络连接或数据库事务。它唯一需要显式收尾的资源是全局配置覆盖；`TEST_MAX_PROCS` 没有恢复机制，设计上作为进程级测试默认值持续存在。

## 与 Go 版本的对应关系

直接对照文件是 [`testenv.go`](./testenv.go)：

| Go | Rust | 对应情况 |
| --- | --- | --- |
| `SetGOMAXPROCSForTest` | `SetGOMAXPROCSForTest` | 都把并行度限制到最多 16。Go 实际调用 `runtime.GOMAXPROCS` 改变 Go 调度器；Rust 只写入 `TEST_MAX_PROCS`，需要消费者主动读取，不能视为相同的进程调度效果。 |
| `UpdateConfigForNextgen(testing.TB)` | `UpdateConfigForNextgen(&dyn TestContext)` | 两者都调用 helper、保存完整配置、注册 cleanup，并设置 SYSTEM keyspace 与 nextgen DXF scope。Rust 用最小 trait 代替完整测试框架接口。 |
| 无对应导出函数 | `MaxProcsForTest`、`ServiceScopeForTest` 及四个 snake_case 别名 | Rust 为无法直接修改调度器的场景增加读取接口，并提供命名兼容层；这些是 Rust 侧扩展。 |

Go 注释还明确说明：nextgen 单元测试默认运行在 SYSTEM keyspace；需要引导多个 keyspace 的测试应使用 `CreateMockStoreAndDomainForKS`。Rust 实现保留了字段更新行为，但本文件没有提供多 keyspace 创建路径，因此不能用它替代该专用辅助函数。

## 扩展指南

- 若改变并行度策略，应同时修改 `MAX_TEST_PROCS`、`TEST_MAX_PROCS` 初始化与 `SetGOMAXPROCSForTest`，避免初值和重设值漂移；在 [`testenv_test.rs`](./testenv_test.rs) 中扩充独立测试，不要把测试嵌入生产源文件。
- 若新增 nextgen 必需配置字段，应在 `UpdateConfigForNextgen` 的同一个 `config::update_global` 闭包中设置，并扩展恢复测试。当前完整快照恢复会自动覆盖新增字段，但测试仍应验证安装值和 cleanup 后的旧值。
- 若新增需要恢复的另一类全局状态，不应假设配置快照会包含它；应为该状态单独捕获旧值并注册 cleanup，明确多个 cleanup 的顺序要求。
- 若希望实际限制 Rust worker 数，消费者必须在创建线程池/worker 时读取 `MaxProcsForTest`；不要仅调用 setter 后假设现有运行时已被重配。新增调用点应有对应的独立测试证明 worker 选择确实采用该值。
- 若要支持并行运行多个 nextgen 测试，应先设计进程级配置的串行化或作用域化机制。只在本函数内加短时锁不能覆盖测试主体与 cleanup 之间的生命周期。
- PascalCase 名称承担 Go 移植兼容，snake_case 名称承担 Rust 风格接口。删除或改变任一组前都应搜索外部使用者，并评估 API 兼容风险。
- 性能风险主要来自全局配置的完整克隆与错误扩展缓存重建；该路径面向测试且调用频率低。若未来进入高频路径，应先测量而不是改为部分原地更新，因为当前快照与整体替换语义也是并发安全边界。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录中识别 `lib.rs`、`testenv.rs`、`testenv_test.rs`、`testenv.go`；目标文件被识别为 123 行、16 个符号。
- RustCodeGraph `node --file pkg/testkit/testenv/testenv.rs`：核对全部常量、静态量、trait、公开函数、别名和函数体。
- RustCodeGraph `query`：定位 Rust/Go 两版 `SetGOMAXPROCSForTest`、`UpdateConfigForNextgen`，以及 Rust 的 `MaxProcsForTest`、`ServiceScopeForTest`；定位下游 `get_global_config`、`store_global_config`、`update_global` 和 `NEXT_GEN_TARGET_SCOPE`。
- RustCodeGraph `callers` / `callees`：对 Rust 的四个关键公开函数执行精确查询，未返回可用调用边；因此又用局部 `rg` 检索符号名和 crate 名补齐并交叉检查调用/依赖事实。
- 源码与配置：[`testenv.rs`](./testenv.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)、`pkg/config/config.rs`、`pkg/keyspace/keyspace.rs`、`pkg/dxf/framework/handle/handle.rs`。
- 语义对照与测试：[`testenv.go`](./testenv.go)、[`testenv_test.rs`](./testenv_test.rs)，以及上述 Go 调用点；测试证明并行度处于 `1..=16`，并证明 helper、安装值和 cleanup 恢复行为。
- 未运行 Cargo 或代码测试：本任务只新增说明文档，按计划以源码事实核验和章节结构命令作为验证。
