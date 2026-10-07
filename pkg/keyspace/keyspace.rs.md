# `pkg/keyspace/keyspace.rs`

## 文件定位

本文件是 `astersql-keyspace` crate 的 keyspace 核心工具实现。crate 根 `pkg/keyspace/lib.rs` 以私有模块 `mod keyspace` 挂载它，再用 `pub use keyspace::*` 将其公开符号提升到 crate 根。包级契约来自 `pkg/keyspace/doc.go`：keyspace 把同一物理集群划分成逻辑集群，`SYSTEM` 是系统服务使用的保留 keyspace；本文件提供这种隔离在配置、etcd 路径、日志和客户端上下文上的基础表达。

`pkg/keyspace/Cargo.toml` 将该 crate 命名为 `astersql-keyspace`，默认是 Classic；`nextgen` feature 同时打开 `kerneltype/nextgen` 与 `deploymode_dependency/nextgen`。本文件直接依赖 crate 根重导出的 `config` 和 `kerneltype`，不负责创建 keyspace、访问 PD/TiKV，也不管理 etcd 客户端。

## 核心职责

1. 用 `ApiVersion`、`Codec`、`BasicCodec` 和 `CodecV1` 表达生成 keyspace 路径所需的最小 codec 信息。
2. 用 `MakeKeyspaceEtcdNamespace` / `MakeKeyspaceEtcdNamespaceSlash` 把 V2 codec 的数值 ID 映射为 `/keyspaces/tidb/{id}` 路径；V1 保持无命名空间的空字符串。
3. 通过 `GetKeyspaceNameBySettings` 读取当前全局配置，并通过 `GetKeyspaceNameBytesBySettings` 提供一次初始化、进程期稳定的字节快照。
4. 通过 `WrapZapcoreWithKeyspace` 给抽象日志核心注入 `keyspaceName` 字段；空名称时保持核心不变。
5. 通过 `BuildAPIContext` 把空/非空 keyspace 名称分别映射为 V1/V2 客户端上下文。

这些职责都只做值转换或适配，不执行 I/O；真实生产接线可见 `pkg/store/etcd.rs::EtcdNamespace`、`cmd/tidb-server/main.rs`、`pkg/kv/paging_resource_control.rs`、`pkg/dxf/importinto/taskkey/task_key.rs` 和 `pkg/keyspace/username_policy.rs`。

## 主要符号

- `System: &str = "SYSTEM"`：保留系统 keyspace 名称。当前文件只声明，不实施创建顺序或访问控制。
- `tidbKeyspaceEtcdPathPrefix`：私有常量 `/keyspaces/tidb/`，是两种 etcd 路径函数的共同前缀。
- `ApiVersion::{V1, V2}`：本地二值 API 版本模型；`V1` 表示经典无前缀路径，`V2` 表示携带 keyspace ID。
- `Codec`：最小 trait，要求 `api_version() -> ApiVersion` 与 `keyspace_id() -> u32`。路径函数接收 `&dyn Codec`，因此调用方可以提供不同存储 codec。
- `BasicCodec { api_version, keyspace_id }`：字段公开的简单值对象，并直接实现 `Codec`。
- `CodecV1`：`BasicCodec` 静态值，固定为 `ApiVersion::V1` 和 ID `0`，对应 Go 包级 `tikv.NewCodecV1(tikv.ModeTxn)` 的本文件所需语义。
- `MakeKeyspaceEtcdNamespace(&dyn Codec) -> String`：V1 返回空串；否则返回无尾斜杠路径。
- `MakeKeyspaceEtcdNamespaceSlash(&dyn Codec) -> String`：V1 返回空串；否则返回带尾斜杠路径。
- `GetKeyspaceNameBySettings() -> String`：每次调用 `config::get_global_keyspace_name()`，返回当前配置的拥有型字符串。
- `keyspaceNameBytes: OnceLock<Vec<u8>>`：进程级私有缓存，只允许首次初始化成功一次。
- `GetKeyspaceNameBytesBySettings() -> &'static [u8]`：Classic 首次初始化为空 `Vec`；NextGen 首次读取全局名称并保存 UTF-8 字节，后续只借用同一缓存。
- `IsKeyspaceNameEmpty(&str) -> bool`：只判断严格空串，不做 trim、合法性或保留名判断。
- `LogCore`：日志核心适配 trait，消费 `self`，以 `with_field(key, value) -> Self` 返回增强后的核心。
- `WrapZapcoreWithKeyspace<C: LogCore>(C) -> C`：读取当前名称；非空时添加键固定为 `keyspaceName` 的字段。
- `ApiContext::{V1, V2(String)}` 与 `BuildAPIContext`：空名称生成 V1，非空名称复制为拥有型 V2 上下文。

文件没有条件编译项；Classic/NextGen 差异由运行时调用 `kerneltype::IsNextGen()`（其实现受 crate feature 影响）体现。

## 执行流程

etcd 命名空间流程从存储层进入：`pkg/store/etcd.rs::EtcdNamespace` 取得 `Storage::GetCodec()`，传给 `MakeKeyspaceEtcdNamespace`。函数先检查 API 版本；V1 立即返回空串，V2 再读取 ID 并格式化路径。带尾斜杠版本执行相同分支，只在 V2 结果末尾追加 `/`；Rust 生产代码搜索未发现该版本的直接生产调用，当前由迁移测试固定其契约。

配置字符串流程是即时读取：`GetKeyspaceNameBySettings` 每次返回全局配置当前值。调用者将它用于 server 启动参数（`cmd/tidb-server/main.rs`）、分页资源控制指标的 keyspace label（`pkg/kv/paging_resource_control.rs`）、NextGen Import Into task key（`pkg/dxf/importinto/taskkey/task_key.rs`）及 Starter 用户名前缀策略（`pkg/keyspace/username_policy.rs::GetUsernamePolicy`）。

配置字节流程是快照读取：首次调用 `GetKeyspaceNameBytesBySettings` 时，`OnceLock::get_or_init` 检查内核类型；Classic 存入空向量，NextGen 读取一次配置并存入字节向量。`cmd/tidb-server/main.rs` 将返回切片传给 `topsql::SetupTopProfiling`。首次初始化之后，即使全局配置变化，该函数仍返回旧快照；这与 Go 的 `sync.Once` 语义一致。

日志流程由 `cmd/tidb-server/main.rs::setupLog` 调用 `WrapZapcoreWithKeyspace`：函数即时读取名称，空串原样返回核心，非空则调用一次 `LogCore::with_field("keyspaceName", value)`。API 上下文流程则只按输入字符串是否为空选择枚举；代码搜索显示 Rust 当前只在 `pkg/keyspace/migration_aster_unit_test.rs` 直接验证 `BuildAPIContext`。

## 数据与状态

大多数类型是无内部可变状态的值：`ApiVersion`、`BasicCodec` 和 `ApiContext` 都派生相等性与调试能力；前两者还是 `Copy`，`ApiContext::V2` 因持有 `String` 仅为 `Clone`。路径函数始终分配新的 `String`，V1 也返回新建空串。

唯一由本文件持有的进程级可变生命周期状态是 `keyspaceNameBytes: OnceLock<Vec<u8>>`。其不变量是：初始化前无值；首次调用后永久持有一个 `Vec<u8>`；公开 API 只暴露 `&'static [u8]`，调用者不能改写或释放缓存。Classic 的空切片在语义上对应 Go 的 nil `[]byte`，但 Rust 不保留 nil 与非 nil 空切片的可观察区别。

`GetKeyspaceNameBySettings`、`WrapZapcoreWithKeyspace` 不使用上述缓存，因此它们可观察全局配置的当前值；这可能与已经缓存的字节切片不同。安全扩展时必须保留“字符串即时值、字节一次性快照”这一差异，除非同时修改所有依赖方与测试契约。

## 依赖与调用关系

下游依赖很窄：`config::get_global_keyspace_name()` 提供配置字符串，`kerneltype::IsNextGen()` 决定是否缓存字节；标准库 `OnceLock` 提供线程安全的一次初始化。路径、空值、上下文和日志适配逻辑没有网络、磁盘或异步依赖。

RustCodeGraph 对目标文件记录了 18 个符号并显示它被 34 个文件使用，但对七个公开函数的精确 `callers` / `callees` 查询均返回空边。因此调用关系以索引文件节点、精确符号查询和补充的 Rust 文本引用共同核对；不能把缺失的图边解释为“没有调用者”。已确认的直接关系包括：

- `pkg/store/etcd.rs::EtcdNamespace -> MakeKeyspaceEtcdNamespace`；
- `pkg/keyspace/username_policy.rs::GetUsernamePolicy -> GetKeyspaceNameBySettings`；
- `pkg/kv/paging_resource_control.rs::{observe_request, observe_response} -> GetKeyspaceNameBySettings`；
- `pkg/dxf/importinto/taskkey/task_key.rs::ForJob -> GetKeyspaceNameBySettings`（仅 NextGen 分支）；
- `cmd/tidb-server/main.rs` 启动链读取字符串、为 TopSQL 读取字节，并在 `setupLog` 中包装日志核心；
- `pkg/kv/kv_test.rs` 读取缓存字节；`pkg/keyspace/keyspace_test.rs` 与 `pkg/keyspace/migration_aster_unit_test.rs` 提供本文件的直接 Rust 测试证据。

`pkg/keyspace/lib.rs` 的 `pub use keyspace::*` 是所有外部 Rust 调用从 crate 根访问这些符号的必要接线。

## 错误处理与边界

本文件所有公开函数都是不可失败签名，不返回 `Result`。它们假设传入的 `Codec` 方法和 `LogCore::with_field` 本身可完成；trait 也没有错误通道。格式化 keyspace ID 使用无符号 `u32` 十进制，不存在负数分支。

关键边界如下：V1 无论 ID 是多少都返回空命名空间；V2 的 ID（包括 `0`）都按原值进入路径。只有长度为零的名称被视为空，空白、点号、保留名或其他格式不会在这里校验。非空日志名称会原样写入字段，非空 API context 名称会完整复制。UTF-8 不会失败，因为配置源已经是 Rust `String`。

缓存没有刷新 API；测试不能在同一进程内安全地把它重置为另一配置。`pkg/keyspace/keyspace_test.rs::run_isolated` 因而为缓存敏感测试启动新的测试进程。若新增可变配置行为，不能仅修改读取函数而忽略这一不可刷新边界。

## 并发与资源生命周期

`OnceLock` 保证多个线程竞争首次字节读取时初始化闭包最多成功执行一次，并在之后提供共享的静态切片；调用者无需加锁，返回引用的生命周期覆盖整个进程。缓存持有的少量字节直到进程退出才释放，没有显式清理阶段。

其余函数只使用局部拥有值或不可变借用，没有任务、通道、事务、异步等待或外部资源。`WrapZapcoreWithKeyspace` 按值消费并返回日志核心，具体核心内部资源的共享/释放规则属于 `LogCore` 实现者，不由本文件管理。`GetKeyspaceNameBySettings` 所访问的全局配置同步由 `astersql-config` 负责；本文件不额外锁定配置，也不保证字符串读取与已缓存字节属于同一时刻。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/keyspace/keyspace.go`。`System`、etcd 前缀、V1/V2 路径分支、全局名称读取、空串判断和空/非空 API context 选择保持相同语义。Rust 用本地 `Codec` / `BasicCodec` 代替 Go `tikv.Codec` 的宽接口，用本地 `ApiContext` 枚举代替 `pd.APIContext`，因此这些 Rust 类型是所需契约的轻量表达，不等同于外部库完整类型。

Go `CodecV1` 是事务模式的 client-go codec；Rust 静态值只保存本文件路径逻辑需要的 API 版本和 ID。Go 日志函数返回 `zap.Option` 并通过 `zap.WrapCore` 延迟包装；Rust 函数直接接收实现 `LogCore` 的核心并返回新核心。二者对空名称不加字段、非空名称添加 `keyspaceName` 的可观察结果一致，但类型和包装时机不同。

Go 用 `sync.Once` 和包级 `[]byte` 缓存；Rust 用 `OnceLock<Vec<u8>>`。Classic 下 Go 返回 nil 切片，Rust 返回空切片；调用方按内容判断时等价。Go 测试可重置包级 `sync.Once`，Rust 生产 `OnceLock` 不暴露重置，因此 Rust 测试用子进程隔离。

`pkg/keyspace/keyspace_test.rs` 对照 Go 的配置名称、空名称和字节行为；`pkg/keyspace/migration_aster_unit_test.rs` 额外验证 V1/V2 路径、API context 和日志字段。当前证据没有显示 Rust 将 `BuildAPIContext` 接到真实 PD 客户端，因此文档只将其描述为已实现且受测试的契约。

## 扩展指南

- 新增 API 版本或路径规则时，先扩展 `ApiVersion` 与 `MakeKeyspaceEtcdNamespace*`，明确新版本是否需要 ID、尾斜杠及兼容旧 etcd 数据；同步独立测试 `pkg/keyspace/migration_aster_unit_test.rs`，并检查 `pkg/store/etcd.rs::EtcdNamespace` 的实际消费方式。
- 扩展 codec 信息应优先保持 `Codec` 为路径计算所需的最小接口；如果字段属于存储实现而非 keyspace 路径，不应无故加入此 trait。需要验证 `pkg/store/store.rs`、`pkg/store/mockstore/teststore/store.rs` 及其独立测试中的实现。
- 改动配置缓存时必须明确是保留一次快照还是改成动态值。前者应继续用线程安全的一次初始化；后者会影响 TopSQL 等持有借用数据的调用方式，并需更新 `pkg/keyspace/keyspace_test.rs` 的子进程隔离与缓存断言。
- 新增日志字段时通过 `LogCore` 抽象表达可观察行为，并在 `migration_aster_unit_test.rs` 的 `TestCore` 中验证空/非空配置分支；不要把具体日志库资源管理塞进本文件。
- 将 `ApiContext` 接入真实客户端前，要核对目标客户端的 V1/V2 类型、错误与生命周期，而不是仅依赖当前本地枚举。测试逻辑继续放在独立 `*_test.rs` 文件，不嵌入生产源文件。
- 任何行为改动都应同步核对 `pkg/keyspace/keyspace.go` 和 `pkg/keyspace/keyspace_test.go`，保留 Go 分支、边界和测试意图；路径格式、日志字段名和 Classic/NextGen 差异都属于兼容性风险。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标目录中 `keyspace.rs` 有 18 个符号；`node --file pkg/keyspace/keyspace.rs` 读取完整 178 行并报告 34 个使用文件；对七个公开函数做了精确 `query`，再以文件符号 ID 查询 `callers` / `callees`，结果均为空边，故使用直接引用搜索补足图未覆盖关系。
- 生产与装配：`pkg/keyspace/keyspace.rs`、`pkg/keyspace/lib.rs`、`pkg/keyspace/Cargo.toml`、`pkg/keyspace/doc.go`。
- Go 对照：`pkg/keyspace/keyspace.go`、`pkg/keyspace/keyspace_test.go`。
- Rust 测试：`pkg/keyspace/keyspace_test.rs`、`pkg/keyspace/migration_aster_unit_test.rs`。
- 直接调用证据：`pkg/store/etcd.rs`、`pkg/keyspace/username_policy.rs`、`pkg/kv/paging_resource_control.rs`、`pkg/dxf/importinto/taskkey/task_key.rs`、`cmd/tidb-server/main.rs`；补充引用搜索还确认 `pkg/kv/kv_test.rs` 使用字节缓存。
- 人工复核重点：V1/V2 路径分支、空名称边界、即时字符串与一次性字节快照的区别、Classic/NextGen 分支、日志字段名、Rust 与 Go 类型适配差异，以及当前仅测试覆盖的 API。
