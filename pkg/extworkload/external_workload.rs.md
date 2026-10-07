# `pkg/extworkload/external_workload.rs`

## 文件定位

本文件是 `astersql-extworkload` crate 的后台工作负载管理契约层，源码由
[`pkg/extworkload/lib.rs`](lib.rs) 以 `mod external_workload` 装入并通过
`pub use external_workload::*` 从 crate 根重新导出。它不负责建连或发送 RPC，而是用
`Manager` trait 统一上层对 GCV2、TTL 和 Auto Analyze 控制器操作的依赖；具体实现位于
[`pkg/extworkload/manager.rs`](manager.rs) 的 `impl Manager for manager`。

[`pkg/extworkload/Cargo.toml`](Cargo.toml) 将该目录定义为 `astersql-extworkload` 库，入口为
`lib.rs`，直接依赖客户端子 crate、Tokio 与 Tonic。Cargo 的
`package.metadata.porting.go-package = "pkg/extworkload"` 也明确了其 Go 对照包。

## 核心职责

1. 用 `Manager: Send` 建立可替换、可放入线程安全所有权容器的控制器管理接口。
2. 用 `ManagerError` 抹平具体实现的错误类型，使上层只依赖一个可跨线程传递的错误边界。
3. 将三类后台任务收束到同一契约：keyspace 级 GCV2、按表 TTL、Auto Analyze。
4. 暴露角色和 keyspace 元数据，让调用方在发起动作前执行角色及 GC 模式判断。
5. 通过 `RegisterTTLTableInfo` 的默认实现兼容 Go 公共接口名称，同时保留既有 Rust
   实现者所实现的 `RegisterTTLTask` 方法。

该文件只规定“必须提供什么操作”。30 秒超时、指标标签、TLS、Ping、日志、RPC 参数转换
等策略都由 `manager.rs` 和客户端层承担，不能从本文件本身推断其已经执行。

## 主要符号

### `ManagerError`

`pub type ManagerError = Box<dyn std::error::Error + Send + Sync>` 是所有可失败方法的错误
类型。`Send + Sync` 允许错误穿过线程安全边界；动态分派使网络、配置或实现自定义错误无需
进入本 trait 的类型参数。它不增加错误码、重试性或上下文语义，这些由实现者保留在具体错误
的 `Display`/source 链中。

### `Manager`

`pub trait Manager: Send` 是唯一主要类型，共包含以下方法组：

- 生命周期与身份：`Close(&mut self)`、`Role(&self)`、`Meta(&self)`。
- GCV2：`InitializeGCV2`、`AbortGCV2`、`RegisterGCV2`、`RecycleGCV2`、
  `UpdateGCLifeTime`。
- TTL：`RegisterTTLTask`、带默认方法体的 `RegisterTTLTableInfo`、
  `DeleteTTLTableInfo`、`RecycleTTLTask`、`UpdateTTLJobEnable`。
- Auto Analyze：`RegisterAutoAnalyze`、`RecycleAutoAnalyze`。

除了 `RegisterTTLTableInfo`，其他方法都没有默认实现，新增实现者必须显式决定每项行为。
所有业务动作都要求 `&mut self`，读取身份的 `Role`/`Meta` 只要求共享借用。

### 参数语义

- `context: &context::Context`：调用方传入的取消、超时及附加值传播位置；本 trait 不创建
  deadline。
- `gc_life_time` / `gcLifeTime: Duration`：GC 历史版本保留时长；转换到控制器使用的秒数
  是实现层责任。
- `safePoint: u64`：已完成或已处理到的 GC 安全点；`AbortGCV2` 不直接暴露该参数。
- `tableID: i64` 与 `ttlJobEnable: bool`：TTL 表标识及当前全局作业开关快照。
- `completedJobCreateTime: u64`：已完成 TTL 作业的创建时间水位。
- `taskID: u64`：Auto Analyze 任务标识。
- `Meta() -> Option<&KeyspaceMeta>`：借用管理器持有的元数据，并用 `Option` 表达 Go 指针
  可能为 `nil`，不会复制所有权。

## 执行流程

### 构造和安装

1. `pkg/extworkload/manager.rs::NewManager` / `NewManagerWithTLS` 根据配置决定返回
   `None` 或 `Box<dyn Manager>`，启用时要求存在 keyspace 元数据。
2. 具体 `manager` 完成拨号和 Ping 后被擦除为 trait object。
3. `pkg/session/runtime/session.rs::install_external_workload_manager` 将该对象交给 Domain；
   `pkg/extworkload/util.rs::SharedManager` 将其表示为
   `Arc<Mutex<Box<dyn Manager>>>`，供多个子系统共享并串行化可变调用。

### GCV2

`pkg/session/runtime/session.rs::initialize_external_workload_gcv2` 先读取 `Role` 和 `Meta`，
只在 master 且 keyspace 使用 keyspace-level GC 时调用 `InitializeGCV2`。运行中，
`pkg/store/gcworker/gc_worker.rs` 在相应角色上调用 `RegisterGCV2` 与 `RecycleGCV2`；
session 的配置通知路径调用 `UpdateGCLifeTime`。升级辅助函数
`pkg/extworkload/util.rs::AbortGCV2ForUpgrade` 只对 GCV2 worker 调用 `AbortGCV2`。

### TTL

`pkg/domain/domain.rs` 在表 TTL 元数据创建或变化时调用 `RegisterTTLTableInfo`，在禁用 TTL
或删除表时调用 `DeleteTTLTableInfo`，并在失败补偿中重新注册先前删除的表。完成作业由
`RecycleTTLTask` 上报；全局 `tidb_ttl_job_enable` 变化由 `UpdateTTLJobEnable` 上报。
默认 `RegisterTTLTableInfo` 随即调用 `self.RegisterTTLTask(...)`，因此现有实现者无需重复
实现两个等价注册入口。

### Auto Analyze

契约为任务注册和完成分别提供 `RegisterAutoAnalyze` 与 `RecycleAutoAnalyze`，具体
`manager.rs` 已转发到客户端。此次针对生产 Rust 文件的直接引用检索未找到这两个方法的
上层生产调用点，因此文档只确认契约和实现存在，不把完整业务接线表述为已验证。

## 数据与状态

本文件不定义 struct 字段、全局变量、缓存或可变静态状态。trait 方法操作的状态归具体
实现所有：当前生产实现的 `manager` 保存客户端、角色和一份克隆的 `KeyspaceMeta`。

关键状态约束如下：

- `Role` 和 `Meta` 是管理器身份快照，调用方用它们决定本实例可以执行哪类后台任务。
- `Meta` 的返回借用生命周期受 `&self` 约束；调用方不能让引用越过管理器生命周期。
- GCV2 safe point、TTL 表 ID/开关/完成水位和 Auto Analyze task ID 都按值传入，契约层
  不记忆、去重或检查单调性。
- `Duration` 在契约层保持纳秒精度；当前 `manager.rs` 转发时用 `as_secs_f64() as i64`
  截断小数秒。`manager_test.rs::test_gcv2_lifetime_truncates_fractional_seconds` 固定了这一
  实现行为，但它不是 trait 自身施加的校验。

## 依赖与调用关系

本文件只直接依赖 crate 根的三个模块：

- `config`：提供 `ExternalWorkloadRole`。
- `context`：提供所有有上下文动作的借用参数。
- `keyspacepb`：提供 `KeyspaceMeta`。

核心关系可概括为：上层 Domain/session/GC worker → `Box<dyn Manager>` →
`manager.rs::manager` → `client::Client` → external workload controller。RustCodeGraph 对目标
文件给出的直接文件引用为 `pkg/extworkload/manager_test.rs` 与
`pkg/session/runtime/session.rs`；由于 `lib.rs` 进行了 crate 根再导出，按方法名和 trait
对象的补充检索还确认了 `pkg/domain/domain.rs`、`pkg/store/gcworker/gc_worker.rs` 和
`pkg/extworkload/util.rs` 的间接使用。

`Manager: Send` 只保证管理器值可以在线程间转移，并不保证共享访问；共享形态由
`SharedManager = Arc<Mutex<Box<dyn Manager>>>` 提供。调用方取得互斥锁后才能调用需要
`&mut self` 的业务方法。

## 错误处理与边界

- 所有可能触达实现或外部控制器的动作返回 `Result<(), ManagerError>`；`Role` 和 `Meta`
  是无失败读取。
- trait 不吞错、不记录日志、不重试，也不把失败分类。当前 `manager.rs` 将客户端错误装箱
  后向上传播，`manager_test.rs::test_manager_method_error_propagation` 验证消息 `boom`
  保持可见。
- 默认 `RegisterTTLTableInfo` 原样返回 `RegisterTTLTask` 的结果，不包装错误或执行补偿。
- `Meta == None` 是接口允许的边界；生产构造器启用时拒绝缺少 meta，但测试替身或其他实现
  仍可返回 `None`，所以调用方必须保留 `Option` 分支。
- 本契约不验证角色与动作是否匹配、不验证 safe point 单调性、不检查 ID 或时长范围；这些
  前置条件由调用方和具体实现负责。
- `Close` 需要可变借用且可能失败；trait 未声明幂等，也未提供 `Drop` 自动关闭保证，资源
  所有者必须显式安排关闭路径。

## 并发与资源生命周期

`Manager: Send` 允许把实现移交到其他线程，但没有 `Sync` 约束。仓库当前用
`Arc<Mutex<Box<dyn Manager>>>` 在 Domain 中共享：`Arc` 管理引用计数生命周期，`Mutex`
保证同一时刻只有一个可变调用。这意味着调用者不应在持锁期间执行无关的长操作；当前实现
可能在锁内等待控制器 RPC，扩展时需评估锁竞争和重入风险。

`context::Context` 通过借用传递，调用期间有效；实际超时 context 由 `manager.rs` 派生并在
RPC 返回后取消。本文件不创建线程、异步任务或通道。控制器连接由具体实现持有，最后应通过
`Close` 释放；移除或替换 Domain 中的共享管理器时，也必须确认旧管理器的关闭责任没有丢失。

## 与 Go 版本的对应关系

Go 权威对照为 [`pkg/extworkload/external_workload.go`](external_workload.go)。两版都定义
`Manager`，方法组按 Close/身份、GCV2、TTL、Auto Analyze 排列，参数的 ID 宽度和时长
语义保持一致。主要语言映射为：

- Go `error` → Rust `Result<(), ManagerError>`。
- Go `context.Context` 值 → Rust `&context::Context` 借用。
- Go `*keyspacepb.KeyspaceMeta` → Rust `Option<&KeyspaceMeta>`。
- Go 可空接口 → Rust 构造及存储边界上的 `Option<Box<dyn Manager>>`。
- Go 接口隐式可由实现满足 → Rust 显式 `impl Manager for manager`。

当前存在一项有意的 Rust 兼容层差异：Go 只公开 `RegisterTTLTableInfo`，Rust trait 同时要求
`RegisterTTLTask`，并让 `RegisterTTLTableInfo` 默认委托前者。这样保持 Go 调用语义，又不迫使
既有 Rust 实现者立即改名。扩展或收敛 API 时必须同时检查两者，避免递归委托或让两条路径
产生不同语义。

Go `manager.go` 与 Rust `manager.rs` 都把初始化映射为 `RegisterGCV2(0, lifetime)`、把中止
映射为 `RecycleGCV2(MAX)`，并对注册/回收动作附加对应指标；这些是实现对齐证据，而不是本
trait 的默认行为。

## 扩展指南

新增或修改管理动作时建议按以下顺序接入：

1. 在本文件修改 `Manager` 签名，先明确是否必须带 context、是否改变状态以及错误边界。
2. 同步 Go 的 `external_workload.go`，或明确记录仅 Rust 存在的兼容原因；不要让两版公共
   方法组静默漂移。
3. 在 `manager.rs::impl Manager for manager` 实现超时、指标和客户端转发，并在
   `client::Client`、真实客户端及协议层补齐相同动作。
4. 更新所有独立实现者和替身。重点包括 `manager_test.rs`、`util_test.rs`、
   `migration_aster_unit_test.rs`、`pkg/domain/canonical_domain_test.rs`、
   `pkg/session/runtime/ttl_sysvar_test.rs` 与 `pkg/store/gcworker/gc_worker_test.rs`。
5. 在最靠近真实上层调用者的独立测试中覆盖角色门控、参数传递、错误传播与资源关闭；不要把
   Rust 测试嵌入本生产文件。

兼容性风险主要是 trait 新增必实现方法会破坏全部实现者编译，或修改 `RegisterTTLTableInfo`
桥接导致 TTL 注册行为分叉。正确性风险包括 safe point/任务 ID 语义误用、忽略 `Meta=None`
和丢失 context。性能风险集中在共享 `Mutex` 持锁期间执行网络调用；接口本身不提供并发队列
或批处理能力。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；
  `files --filter pkg/extworkload` 确认目标及同目录实现/测试集合。
- RustCodeGraph 源码与结构：
  `node --file pkg/extworkload/external_workload.rs --offset 1 --limit 500`；目标文件共 145 行、
  19 个符号，并报告直接被 `manager_test.rs`、`session/runtime/session.rs` 使用。
- RustCodeGraph 对照读取：`manager.rs`、`util.rs`、`lib.rs`、
  `session/runtime/session.rs`、`external_workload.go`、`manager.go`。
- crate 边界：`pkg/extworkload/Cargo.toml`；模块再导出：`pkg/extworkload/lib.rs:434-436`。
- Rust 独立测试：`pkg/extworkload/manager_test.rs` 验证生命周期、Ping 失败、全部转发方法的
  deadline/指标、默认 TTL 桥接、错误传播和时长截断；
  `pkg/extworkload/migration_aster_unit_test.rs` 验证 Go 参数及角色语义；其他 trait 替身测试
  路径列于“扩展指南”。
- Go 对照与测试：`pkg/extworkload/external_workload.go`、`manager.go`、`manager_test.go`。
- 上层接线补充检索：在 `pkg/domain`、`pkg/session`、`pkg/store`、`pkg/ddl`、
  `pkg/statistics` 中按全部 trait 方法名执行 `rg`，确认 GCV2 与 TTL 的生产调用点；本次未发现
  Auto Analyze 两个方法的生产 Rust 调用点，故保留为“未验证完整接线”。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前另以固定标题命令验证恰有 11 个二级章节。
