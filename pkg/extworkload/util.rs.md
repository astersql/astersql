# `pkg/extworkload/util.rs`

## 文件定位

本文件是 `astersql-extworkload` crate 的轻量判定与 Manager 存取边界，源码见 [`util.rs`](./util.rs)。crate 根在 [`lib.rs`](./lib.rs) 中以 `#[path = "util.rs"] mod util; pub use util::*;` 将其公开符号重导出，因此上游以 `astersql_extworkload::...` 调用，而不需要知道私有的 `util` 模块名。[`Cargo.toml`](./Cargo.toml) 声明该 crate 的库入口为 `lib.rs`；本文件没有自己的 feature 开关，也没有直接引用 Cargo 外部依赖，而是使用 crate 内的 `Manager`、`config`、`context`、`keyspacepb` 和 `ManagerError`。

在完整应用中，它位于“Domain 拥有 Manager”与“session/GC worker 消费 Manager”之间：[`pkg/domain/domain.rs`](../domain/domain.rs) 实现 `ManagerStore` 并负责替换/关闭 Manager；[`pkg/session/runtime/session.rs`](../session/runtime/session.rs) 在升级与 GCV2 初始化路径调用本文件函数；[`pkg/store/gcworker/gc_worker.rs`](../store/gcworker/gc_worker.rs) 用 keyspace GC 模式判定决定是否通知外部控制器。

## 核心职责

1. 将“Manager 是否存在”和“Manager 当前角色是否匹配”封装成无副作用谓词：`IsEnabled`、`IsMaster`、`IsGCV2Worker`、`IsTTLTaskWorker` 与 `IsAutoAnalyzeWorker`。
2. 封装升级前的 GCV2 中止协议：`AbortGCV2ForUpgrade` 仅对专职 GCV2 worker 执行 `Manager::AbortGCV2`，并通过布尔值告知调用方是否应终止后续升级流程。
3. 解析 keyspace 元数据中的 GC 管理模式：`IsKeyspaceUsingKeyspaceLevelGC` 只在 `gc_management_type == "keyspace_level"` 时返回 `true`。
4. 定义 Manager 的共享所有权和存储抽象：`SharedManager`、`ManagerStore`、`SetManagerForStore` 与 `GetManagerFromStore`，让具体 `Domain` 保留生命周期所有权，而不引入进程全局注册表。

## 主要符号

- `pub fn IsEnabled(manager: Option<&dyn Manager>) -> bool`：公开的存在性检查，等价于 `Option::is_some`；它不读取角色。
- `pub fn IsMaster/IsGCV2Worker/IsTTLTaskWorker/IsAutoAnalyzeWorker(...) -> bool`：公开的角色谓词，分别把 `config::RoleMaster`、`RoleGCV2Worker`、`RoleTTLTaskWorker`、`RoleAutoAnalyzeWorker` 交给私有 `roleIs`。常量是 `&str`，函数调用 `to_owned()` 构造 `ExternalWorkloadRole` (`String`)。
- `fn roleIs(manager: Option<&dyn Manager>, role: config::ExternalWorkloadRole) -> bool`：私有共用实现。它先解构 `Option`，只在 Manager 存在时调用 `Manager::Role`，然后做字符串等值比较。
- `pub fn AbortGCV2ForUpgrade(context: &Context, manager: Option<&mut dyn Manager>) -> Result<bool, ManagerError>`：公开的可变 Manager 操作。`Ok(false)` 表示无 Manager 或不是 GCV2 角色；中止成功才返回 `Ok(true)`；中止失败原样传播 `ManagerError`。
- `pub fn IsKeyspaceUsingKeyspaceLevelGC(meta: Option<&KeyspaceMeta>) -> bool`：公开的元数据谓词。它查找 `KeyspaceMeta.config["gc_management_type"]`，并严格匹配字符串 `"keyspace_level"`。
- `pub type SharedManager = Arc<Mutex<Box<dyn Manager>>>`：多消费者共享同一个 trait object 的所有权形式；`Mutex` 保护需要 `&mut self` 的 Manager 方法。
- `pub trait ManagerStore`：存储所有者边界，以 `replace_external_workload_manager` 在一次实现调用中替换所持有的 `Option<SharedManager>` 并返回旧值，以 `get_external_workload_manager` 返回共享句柄；具体同步保证由实现者提供。
- `pub fn SetManagerForStore(...) -> Option<SharedManager>` 与 `pub fn GetManagerFromStore(...) -> Option<SharedManager>`：对可空 `ManagerStore` 的安全适配器；store 为 `None` 时均不触发 trait 方法。前者返回被替换的 Manager，后者返回当前 Manager 的共享句柄。

## 执行流程

### 角色判定

1. 调用方把可选的 `&dyn Manager` 传给公开谓词。
2. `IsEnabled` 直接返回是否为 `Some`；其他四个谓词构造目标角色并调用 `roleIs`。
3. `roleIs` 在 `None` 分支返回 `false`；在 `Some` 分支调用一次 `Role()` 并比较。因而空 Manager 不会发生方法调用。

### 升级前中止 GCV2

1. `AbortGCV2ForUpgrade` 先对 `Option<&mut dyn Manager>` 做 `let Some(...) else`，无 Manager 即 `Ok(false)`。
2. 它读取 `Role()`；非 `RoleGCV2Worker` 直接 `Ok(false)`，不执行中止。
3. 只有 GCV2 worker 会收到 `AbortGCV2(context)`。`?` 使错误立即返回；成功后返回 `Ok(true)`。
4. [`BootstrapCanonicalDomain`](../session/runtime/session.rs) 在 starter 模式且检测到需升级的旧 bootstrap 版本时，从 Domain 取 Manager、持有其互斥锁并调用此函数；错误被加上 `abort GCV2 worker failed` 上下文，`true` 会使非测试路径停止 bootstrap 升级。

### keyspace GC 门控与 Manager 存取

1. `IsKeyspaceUsingKeyspaceLevelGC` 先排除无元数据，再查找配置键，最后做严格值比较；任一步缺失都是 `false`。
2. session 初始化使用它阻止在非 keyspace-level GC 的 keyspace 上安装 GCV2 专职 Manager，并在 master 角色下决定是否初始化/更新 GCV2 生命期。GC worker 则在 safepoint 推进成功后以它为通知外部控制器的前置条件。
3. Domain 调用 `SetManagerForStore(Some(self), manager)`，其 `ManagerStore` 实现用 `RwLock` 内的 `std::mem::replace` 交换 Manager。Domain 收到返回的旧 Manager 后加锁并调用 `Close()`，关闭责任不在 `SetManagerForStore` 本身。
4. `GetManagerFromStore` 通过 trait 方法取得 `Arc` 克隆；消费者持有 `MutexGuard` 时才能访问或修改 Manager。

## 数据与状态

本文件不定义全局可变状态，也不缓存角色或 keyspace 判定结果。谓词的输出完全由当次传入的 `Option<&dyn Manager>`、`Manager::Role()` 或 `Option<&KeyspaceMeta>` 决定。

Manager 状态由 `SharedManager = Arc<Mutex<Box<dyn Manager>>>` 承载：`Arc` 提供共享所有权，`Mutex` 串行化对 trait object 的访问，`Box` 存放动态分派对象。真正的容器状态在 Domain 的 `RwLock<Option<Arc<Mutex<Box<dyn Manager>>>>>` 字段中，`ManagerStore` 只规定替换与读取协议。`SetManagerForStore` 的返回值是生命周期交接的重要数据：调用方必须决定如何处理旧 Manager。

keyspace GC 判定仅读取 `KeyspaceMeta.config` 的一个键值。大小写或其他值都不被视为 keyspace-level GC，这是严格协议而非宽松解析。

## 依赖与调用关系

- 文件内下游：`IsMaster`、`IsGCV2Worker`、`IsTTLTaskWorker`、`IsAutoAnalyzeWorker` 均调用 `roleIs`；`roleIs` 调用 `Manager::Role`；`AbortGCV2ForUpgrade` 调用 `Manager::Role` 与 `Manager::AbortGCV2`；store 适配器分别调用 `ManagerStore` 的两个方法。
- crate 内依赖：`Manager`/`ManagerError` 由 [`external_workload.rs`](./external_workload.rs) 定义并由 `lib.rs` 重导出；角色常量、`Context` 和 `KeyspaceMeta` 目前由 [`lib.rs`](./lib.rs) 中的相应模块提供。
- 生产上游：[`pkg/domain/domain.rs`](../domain/domain.rs) 实现 `ManagerStore` 并调用 set/get 适配器；[`pkg/session/runtime/session.rs`](../session/runtime/session.rs) 调用 `AbortGCV2ForUpgrade` 及 `IsKeyspaceUsingKeyspaceLevelGC`；[`pkg/store/gcworker/gc_worker.rs`](../store/gcworker/gc_worker.rs) 调用 keyspace GC 谓词。
- 当前 Rust 树中，精确文本搜索未找到四个公开角色谓词的非测试调用点；它们已通过 crate 根公开，并由独立测试固定 Go 对齐语义。不应把“已公开”误写成“已在生产主链调用”。
- Cargo 边界：[`Cargo.toml`](./Cargo.toml) 的直接依赖是 `astersql-extworkload-client`、`tokio` 和 `tonic`，但本文件本身不直接使用它们。

## 错误处理与边界

- `None` Manager 是正常的“功能未安装”状态，不是错误：所有角色谓词返回 `false`，`AbortGCV2ForUpgrade` 返回 `Ok(false)`。
- 角色为未知字符串时，所有已知角色谓词都返回 `false`；本文件不校验或规范化角色名。
- `AbortGCV2ForUpgrade` 不捕获、替换或记录 `AbortGCV2` 错误，而是使用 `?` 保留具体 `ManagerError`。出错时不会返回“应终止”的布尔值；调用方必须先处理 `Err`。
- 缺少 keyspace 元数据、缺少 `gc_management_type` 键、值不等于 `keyspace_level` 都是 `false`；本文件不会把缺省配置解释为 keyspace-level GC。源码注释中“absent metadata uses unified GC”与此行为一致。
- `SetManagerForStore(None, manager)` 返回 `None` 且不安装 Manager；`GetManagerFromStore(None)` 返回 `None`。这些适配器不关闭 Manager，也不处理锁毒化；具体 Domain 实现使用 `expect("external workload manager lock poisoned")` 决定了锁毒化时 panic 的策略。

## 并发与资源生命周期

角色与 keyspace 谓词只做短暂借用和只读查询，不启动任务、线程、定时器或通道。`AbortGCV2ForUpgrade` 接收一个已由调用方获得的可变借用；在 Domain 调用点，这个借用来自 `SharedManager` 的 `MutexGuard`，所以 `AbortGCV2` 执行期间同一 Manager 的其他互斥访问会等待。

`SharedManager` 的 `Arc` 使句柄可跨 Domain 消费者共享，但本文件不自动关闭最后一个引用。具体生命周期协议是：Domain 用 `replace_external_workload_manager` 取回旧值，随后在 `set_external_workload_manager` 中对旧值加锁并调用 `Close`。这个先替换、后关闭的顺序避免关闭时仍把旧 Manager 暴露给新读取者；但已经克隆了旧 `Arc` 的调用方仍可能持有它，因此新增消费者不应假设替换会立即撤销所有旧句柄。

`ManagerStore` 本身没有 `Send + Sync` 超级 trait 约束；并发保证由具体实现和 `SharedManager` 组成。当前 Domain 实现使用 `RwLock` 保护容器，内层 `Mutex` 保护 Manager。扩展时要避免在持有 Domain 容器锁的同时执行长时间 Manager 操作；当前 getter 先克隆 `Arc` 再释放 `RwLock`。

## 与 Go 版本的对应关系

[`util.go`](./util.go) 是主要直接对照：

- Go `IsEnabled(m Manager) bool { return m != nil }` 对应 Rust `Option<&dyn Manager>::is_some()`。Rust 用 `Option` 显式表达可空接口，不复制 Go interface 的 nil 表示。
- 四个角色谓词和 `roleIs` 保留 Go 的短路语义：Manager 为空时不调用 `Role()`，非空时严格比较角色值。
- Go `AbortGCV2ForUpgrade` 先通过 `IsGCV2Worker` 判定；Rust 内联了等价的 `None`/角色分支。两者都只在中止成功时返回 `true`，并传播 `AbortGCV2` 错误。
- Go 把 Manager 以私有 `managerStoreKey` 存在通用 `kv.Storage` option 中，setter 无返回值；Rust 不复制通用 option 表，而用 `ManagerStore` 让 canonical Domain 显式持有类型化字段，且 setter 返回旧 `SharedManager` 以便关闭。这是所有权/生命周期适配差异，不是行为无关的逐句翻译。
- Rust `IsKeyspaceUsingKeyspaceLevelGC` 没有位于 Go `pkg/extworkload/util.go` 的同名函数。Go 的 GC/session 相关调用点使用 PD 客户端的 `pd.IsKeyspaceUsingKeyspaceLevelGC`；Rust 迁移层在本 crate 中对当前 `KeyspaceMeta` 桩类型实现相同的门控语义。

[`util_test.go`](./util_test.go) 与 [`util_test.rs`](./util_test.rs) 对齐空 Manager、四角色互斥矩阵、非 GCV2 不中止、GCV2 成功中止和中止错误传播。Rust 还有 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 对空值与角色矩阵的 crate 级回归。ManagerStore 与 keyspace GC 门控的集成语义则由 [`pkg/domain/canonical_domain_test.rs`](../domain/canonical_domain_test.rs)、[`pkg/store/gcworker/gc_worker_test.rs`](../store/gcworker/gc_worker_test.rs) 和 [`pkg/session/runtime/ttl_sysvar_test.rs`](../session/runtime/ttl_sysvar_test.rs) 覆盖。

## 扩展指南

- 新增外部工作负载角色时，先在 `config` 定义角色值，再在本文件增加经 `roleIs` 的公开谓词。同步扩展 [`util_test.rs`](./util_test.rs) 和 [`util_test.go`](./util_test.go) 的交叉矩阵，以证明新角色不会被旧谓词误识别；Rust 测试仍保持在独立测试文件中。
- 修改升级中止条件或返回协议时，必须同时审核 `AbortGCV2ForUpgrade`、`BootstrapCanonicalDomain` 和 Go 对照函数，并扩展成功/非目标角色/错误三个分支的独立测试。该协议影响进程是否继续升级，兼容性风险高。
- 修改 GC 配置键或认可值时，必须与 PD/Go 协议同步，并更新 GC worker 与 session 的 keyspace-level/unified/missing-meta 用例。不要在本函数中默认接受未知值，否则可能让两套 GC 管理路径同时运行或都不运行。
- 新增 `ManagerStore` 实现时，实现者要保证 replace/get 的并发一致性，并明确旧 Manager 由谁、在何时调用 `Close`。还要测试 store 为 `None`、首次安装、替换、删除与 store 实例隔离。当前可参考 `canonical_domain_test.rs` 的隔离与删除断言。
- 性能上，角色谓词目前为常数次查询，但四个包装函数每次会为 `&str` 角色常量创建 `String`；如果要优化这一点，应保持 Go/Rust 公开签名与字符串比较语义，用基准或热路径证据驱动，不要为文档对齐任务扩大修改范围。

## 验证依据

- RustCodeGraph 索引状态：`rustcodegraph status` 报告项目已索引 11,467 个文件，其中 Rust 7,032 个；`rustcodegraph files --filter pkg/extworkload` 确认 `util.rs`、`util_test.rs`、Go 对照和模块入口均在索引中。
- RustCodeGraph 源码/符号：`rustcodegraph explore "pkg/extworkload/util.rs ..."` 返回目标文件完整源码；对公开函数、`ManagerStore` 和 `SharedManager` 的 `query` 确认定义位置及 Domain 中的存取相关符号。`node Manager` 进一步显示 Domain 的 Manager 字段为 `RwLock<Option<Arc<Mutex<Box<dyn Manager>>>>>`。
- RustCodeGraph 调用边限制：精确 `callers SetManagerForStore` 在本地索引上连续 90 秒无输出，已中止；因此调用点以 `rg` 对 Rust 源文件做精确符号搜索后读取直接上下文核验，没有伪称该条图查询成功。
- 已读源码与边界：[`util.rs`](./util.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)、[`external_workload.rs`](./external_workload.rs)、[`pkg/domain/domain.rs`](../domain/domain.rs)、[`pkg/session/runtime/session.rs`](../session/runtime/session.rs) 与 [`pkg/store/gcworker/gc_worker.rs`](../store/gcworker/gc_worker.rs)。`pkg/extworkload` 目录下无 `doc.go`，因此无额外包约定文件可读。
- 已读对照与测试：[`util.go`](./util.go)、[`util_test.rs`](./util_test.rs)、[`util_test.go`](./util_test.go)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)；并通过直接调用点与用例定位了 `canonical_domain_test.rs`、`gc_worker_test.rs` 和 `ttl_sysvar_test.rs` 的 ManagerStore/keyspace GC 边界覆盖。
- 本任务只新增说明文档，没有修改 Rust、Go、Cargo 或测试，按计划不运行 Cargo。交付时使用任务指定的 11 章结构检查，并人工复核符号、调用边、Go 差异和扩展风险。
