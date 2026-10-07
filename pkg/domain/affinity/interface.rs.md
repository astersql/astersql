# `pkg/domain/affinity/interface.rs`

## 文件定位

本文件是 `astersql-domain-affinity` crate 的包级门面。crate 入口 `pkg/domain/affinity/lib.rs` 公开 `interface` 模块并再导出其符号；这里用一个进程内共享状态，把调用者连接到 `pkg/domain/affinity/manager.rs` 中的 `Manager`/`PdClient` 抽象。它本身不编码 key range、不发 HTTP，也不实现旧 PD 兼容回退；这些职责分别属于上游建组逻辑、`http_client.rs` 和 `manager.rs`。

当前 Rust 生产接线中，`pkg/session/runtime/create_table_resources.rs` 的 mock-storage 分支调用本门面的 `create_groups_if_not_exists` 与 `delete_groups_with_retry`；真实存储分支会直接用 `new_pd_manager` 构造临时 manager。RustCodeGraph 将该文件列为被 5 个文件使用，其中除上述生产调用者外，其余是 `interface_test.rs`、`migration_aster_unit_test.rs` 和两份 session 回归测试。与 Go 版本不同，索引与文本搜索未发现 Rust 生产代码调用 `init_manager` 或 `get_all_group_states`，因此不能把 Go 的完整初始化/SHOW AFFINITY 接线视为已在 Rust 中落地。

## 核心职责

- `package_state` 提供惰性创建、全进程唯一的 `PackageState`。默认 manager 是空的 `MockManager`，PD client 为空。
- `init_manager` 原子替换 manager/client 配对：传入 client 时使用 `PdManager`，否则重置为新的 `MockManager`。
- 创建、删除和按 ID 查询函数先处理空输入，再把同一个 `Context` 原样转发给当前 manager。
- `delete_groups_with_retry` 在删除失败时进行固定次数、固定间隔的同步重试，并且只在最终失败时记录一次错误。
- `get_all_group_states` 绕过 manager，直接使用保存的 PD client；这与 Go 中 SHOW AFFINITY 直接查询 PD 的意图一致。
- `set_pd_client_for_test` 只临时替换 client 并返回一次性恢复闭包，供测试注入使用；它不会同步替换 manager。

## 主要符号

- `MAX_RETRY_TIMES: usize = 3`：首次删除失败后最多再重试 3 次，总尝试次数为 4。
- `RETRY_INTERVAL: Duration = 200ms`：相邻失败尝试之间的阻塞等待时间；全部失败时累计三次等待。
- `PackageState { manager, pd_client }`：私有共享状态。`manager: Arc<dyn Manager>` 承担创建、删除和按 ID 查询；`pd_client: Option<Arc<dyn PdClient>>` 专用于全量状态查询及测试替换。
- `package_state() -> &'static RwLock<PackageState>`：通过函数局部 `OnceLock` 惰性初始化状态，避免外部直接取得静态对象。
- `init_manager(Option<Arc<dyn PdClient>>)`：构造匹配的真实或 mock manager，并在写锁下整体替换状态。
- `create_groups_if_not_exists(&dyn Context, &HashMap<String, Vec<AffinityGroupKeyRange>>)`：幂等创建门面；非空输入委托 `Manager::create_affinity_groups_if_not_exists`。
- `delete_groups(&dyn Context, &[String])`：单次删除门面；真实 `PdManager` 最终以 `force=true` 调用 PD client。
- `delete_groups_with_retry(&dyn Context, &[String])`：围绕 `delete_groups` 的同步重试层，成功立即返回，耗尽后返回最后一次错误。
- `get_groups(&dyn Context, &[String])`：按 ID 查询门面，返回 `HashMap<String, AffinityGroupState>`。
- `get_all_group_states(&dyn Context)`：直接调用 `PdClient::get_all_affinity_groups`；没有 client 时返回空 map。
- `set_pd_client_for_test(Option<Arc<dyn PdClient>>) -> impl FnOnce()`：替换并捕获原 client；返回的闭包负责恢复。

本文件无 enum、trait、显式 `impl` 或条件编译项；相关 trait 与具体实现均定义于 `manager.rs`。

## 执行流程

初始化路径为：调用 `init_manager`，根据参数选择 `new_pd_manager(client)` 或 `new_mock_manager()`，然后取得 `PackageState` 写锁并整体替换旧状态。若从未初始化，第一次访问任意门面函数也会由 `package_state` 建立默认 mock 状态。

创建/单次删除/按 ID 查询的共同流程为：检查集合或 ID 切片是否为空；空输入直接返回成功或空 map；否则取得状态读锁，读取 `manager`，在保持读锁期间调用对应 trait 方法，并将其 `Result` 原样返回。具体幂等创建、旧 PD 回退、查询过滤以及强制删除由 `manager.rs` 实现，不在门面层重复。

重试删除流程为：空输入直接成功；随后令 `attempt` 从 0 到 3。每轮调用 `delete_groups`，成功立即结束；失败则保存该轮错误。前三轮失败后各睡眠 200ms；第 4 次失败时不再睡眠，向 `astersql_domain_affinity` 日志 target 写入错误和 group IDs，最终返回第 4 次错误。`interface_test.rs::final_delete_failure_is_logged_once_with_error_and_group_ids` 验证 4 次调用、一次日志及末次错误，`migration_aster_unit_test.rs::package_delete_retries_and_always_forces_pd_cleanup` 验证第三次成功即停止且每次下游删除都使用 `force=true`。

全量查询先在读锁内克隆 `pd_client`，随即释放锁；有 client 时再发起 `get_all_affinity_groups(ctx)`，没有时返回空 map。测试替换则在写锁内用 `std::mem::replace` 取走旧值，恢复闭包稍后再次加写锁并写回原值。

## 数据与状态

`PackageState` 是进程级全局可变状态，而不是请求、session 或 tenant 局部状态。`OnceLock` 只负责初始化外层 `RwLock` 一次；`init_manager` 可以多次替换锁内值。`Arc<dyn Manager>`/`Arc<dyn PdClient>` 允许跨调用共享实现对象，trait 自身要求 `Send + Sync`。

组定义使用 `HashMap<String, Vec<AffinityGroupKeyRange>>`，键是 group ID，值是一组半开 key range；状态查询结果使用以 ID 为键的 `HashMap<String, AffinityGroupState>`。这些类型及 `AffinityError`、`Context` 均来自 `manager.rs`。本文件不缓存 group 内容，也不维护重试计数之外的操作状态。

有两项容易误用的不变量。第一，正常初始化必须通过 `init_manager` 同时更新 manager 与 client，保持二者指向同一后端。第二，`set_pd_client_for_test` 故意只改 client，因此只适合隔离测试；它不会改变创建/删除/按 ID 查询所使用的 manager。恢复闭包是 `FnOnce`，但如果调用方不执行它，共享 client 会保持被替换状态。

## 依赖与调用关系

crate 边界由 `pkg/domain/affinity/Cargo.toml` 定义，包名为 `astersql-domain-affinity`，库入口是 `lib.rs`。本文件直接使用标准库的 `HashMap`、`Arc`、`OnceLock`、`RwLock`、线程睡眠和 `Duration`；日志依赖来自 Cargo 中的 `log = "0.4"`。`serde_json`、`base64`、`reqwest`、`url` 是同 crate 其他实现使用的依赖，并非本文件直接调用。

下游边包括：`init_manager -> new_pd_manager/new_mock_manager`；创建、删除和按 ID 查询门面分别调用 `Manager` 的同名语义方法；全量查询调用 `PdClient::get_all_affinity_groups`；重试删除先回到本文件的 `delete_groups`。`PdManager` 继续调用 PD client，并负责兼容性回退和过滤；`MockManager` 用自己的 `RwLock<HashMap<...>>` 保存内存状态。

上游 Rust 生产边由 `pkg/session/runtime/create_table_resources.rs::create_affinity` 和 `delete_affinity` 提供：mock-storage 使用本文件的包级 API，真实存储直接构造 manager。测试上游包括 `interface_test.rs` 的 context 传播与终态日志测试、`migration_aster_unit_test.rs` 的重试/force 语义测试，以及两份 normal DDL session 回归测试。Go 生产上游还包括 `pkg/domain/infosync/info.go` 的初始化、`pkg/ddl/affinity.go`/`table.go` 的创建删除和 `pkg/executor/show_affinity.go` 的全量查询，但这些只能证明 Go 行为基线，不能证明对应 Rust 接线已存在。

## 错误处理与边界

空 groups/IDs 不取得共享状态锁，也不触发 manager 或 PD 调用：创建、删除返回 `Ok(())`，查询返回新建的空 map。没有 PD client 的全量查询同样返回空 map，而不是“未初始化”错误；调用者因此无法仅凭空结果区分 PD 无数据与 client 未配置。

下游 `AffinityError` 一般原样传播。重试删除是例外：中间错误被后续结果覆盖，成功会丢弃先前错误，完全失败只返回最后一次错误。最终日志包含 `Display` 格式的错误和完整 ID 列表；本文件没有退避、抖动、按错误类型筛选或取消感知，`Context` 仅转发给下游。即使 context 已取消，重试层仍可能继续睡眠和再次调用，是否快速失败取决于 manager/client。

所有共享锁都用 `expect("affinity package state lock poisoned")`；任一持锁 panic 导致毒化后，后续访问会 panic，而不是返回 `AffinityError`。重试循环至少执行一次，所以末尾 `last_error.expect(...)` 在现有控制流下可达时必有值。

## 并发与资源生命周期

`OnceLock<RwLock<PackageState>>` 保证静态状态线程安全初始化；manager 和 client trait 的 `Send + Sync` 约束允许共享。`init_manager` 与测试替换/恢复持有写锁，创建、删除和按 ID 查询持有读锁。后三者通过链式表达式在整个 manager 方法调用期间保留读 guard，因此慢速网络调用会阻塞重新初始化，但同类读操作仍可并发。全量查询只在克隆 `Arc` 时持锁，网络调用发生在锁外，不阻塞状态写入。

删除重试使用 `std::thread::sleep`，会占用当前 OS 线程，不是异步计时器；最坏失败路径在下游调用耗时之外额外等待约 600ms。状态替换只减少旧 `Arc` 的引用计数；若在途调用仍持有读锁/对象引用，其资源按 `Arc` 生命周期延后释放。文件没有后台线程、channel、事务或显式关闭协议。

测试通过 `PACKAGE_STATE_TEST_LOCK`（定义在独立的 `interface_test.rs`）串行化会修改包状态的案例；生产 API 自身并不提供测试级事务隔离。恢复闭包不会在 `Drop` 时自动运行，测试必须显式调用或用 `init_manager(None)` 清理。

## 与 Go 版本的对应关系

`pkg/domain/affinity/interface.go` 是直接语义基线：常量同为 3 次重试和 200ms 间隔，公开函数与 Rust snake_case 函数逐一对应，空输入、共 4 次删除尝试、仅最终失败记录日志、返回末次错误、全量查询绕过 manager 等行为一致。Rust 用 `Arc<dyn Trait>` 表达 Go interface，用 `Option` 表达 nil client，用 `RwLock` 补足 Go 文件中包变量未显式提供的同步保护。

主要差异有三点。其一，Go 的 `InitManager` 明确由 infosync 初始化调用；当前 Rust 搜索未发现生产初始化调用，默认状态因而是 mock，且真实存储的 session 路径选择临时 manager。其二，Go `SetPDClientForTest` 同样只替换 client，但 Rust 用 `impl FnOnce()` 强化“一次恢复”的类型约束并用锁保护替换。其三，Go 的完整 `AffinityGroupState` 来自 PD client，包含 SHOW AFFINITY 所需字段；当前 Rust `manager.rs::AffinityGroupState` 只保存 group 标识和 `range_count`，因此不能据此宣称 Rust 已具备 Go SHOW AFFINITY 的完整展示语义。

Go 的 `manager_test.go` 主要验证 manager 层兼容回退和过滤，而本文件自身的 Rust 对齐证据集中在 `interface_test.rs` 与 `migration_aster_unit_test.rs`。扩展时应继续区分门面契约测试与 manager 策略测试。

## 扩展指南

新增包级操作时，应先判断它属于门面编排还是 manager 策略：只做空输入处理、全局实现选择和统一重试的函数放在本文件；PD HTTP、兼容回退、结果过滤放在 `manager.rs`/`http_client.rs`。新增 manager 能力需同步扩展 `Manager` trait、`PdManager`、`MockManager`，并在独立 `manager_test.rs` 中覆盖；不要把测试模块嵌入生产源文件。

若修改初始化或测试注入，必须维持 manager/client 一致性，或明确说明为何只替换其中一个。若让恢复更抗测试 panic，可考虑独立的 RAII guard，但这会改变公开返回类型和 Go 对齐面，需单独评估。若改变重试次数、间隔、日志 target 或错误选择，应同步 `interface_test.rs` 的 4 次/一次日志断言和 `migration_aster_unit_test.rs` 的 force/retry 断言，并核对 Go `interface.go`。

若把该 API 引入异步执行链，应避免直接复用 `thread::sleep`；需要选择与运行时匹配的异步计时方式，并重新评估锁跨下游调用的问题。若接入 Rust SHOW AFFINITY，则还需补齐状态模型与生产初始化，不能仅调用现有 `get_all_group_states` 就假定与 Go 输出等价。

性能风险主要来自持读锁执行 manager 调用、同步睡眠和全量查询返回大 map；兼容风险来自公开函数签名、默认 mock 行为、空输入短路以及 Go/Rust 错误与日志语义。对应回归应放在同目录独立测试文件，并为真实上游接线保留 session/executor 层测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/domain/affinity` 确认目标、Go 对照、manager/http client 与独立测试均已索引。
- RustCodeGraph `node --file pkg/domain/affinity/interface.rs`：读取完整 163 行源码，确认 10 个符号、默认状态、所有空输入分支、锁范围、重试循环、日志和测试恢复闭包；图报告该文件被 5 个文件使用。
- RustCodeGraph `explore`、`query` 及目标文件 `node`：确认 `init_manager`、`create_groups_if_not_exists`、`delete_groups_with_retry` 等调用边；精确 `callers/callees` 命令没有产生额外文本，因此未把缺失输出解释为“无调用者”，而以图的 used-by、源码节点和文本搜索交叉核验。
- `pkg/domain/affinity/manager.rs`：核对 `Context`、`PdClient`、`Manager`、`PdManager`、`MockManager`、`new_pd_manager`、`new_mock_manager`，以及强制删除、创建/查询兼容回退和内存状态行为。
- `pkg/domain/affinity/Cargo.toml` 与 `lib.rs`：核对 crate 名称、库入口、依赖、公开再导出和独立测试模块装配。
- `pkg/session/runtime/create_table_resources.rs`：核对 Rust `create_affinity`/`delete_affinity` 的 mock-storage 包级调用和真实存储临时 manager 路径。
- `pkg/domain/affinity/interface.go`、`pkg/domain/infosync/info.go`、`pkg/executor/show_affinity.go`、`pkg/ddl/affinity.go` 与 `pkg/ddl/table.go`：核对 Go API、初始化、DDL 和 SHOW AFFINITY 基线，并据此标明 Rust 尚未验证的生产接线。
- `pkg/domain/affinity/interface_test.rs`：核对 context 原样传播、4 次失败、只记录一次最终日志、日志 target/内容和最后错误返回。
- `pkg/domain/affinity/migration_aster_unit_test.rs`：核对第三次成功停止重试，以及每轮删除均向 PD 传 `force=true`。
- `pkg/domain/affinity/manager_test.go`：核对 Go manager 的创建兼容回退、查询过滤和全量扫描边界；它是下游 manager 的证据，不替代本门面的 Rust 测试。
- 本任务是纯文档分析，按计划不运行 Cargo。结构检查要求目标文件存在且恰有 11 个规定的二级标题。
