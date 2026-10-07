# `pkg/kv/unistore.rs`

## 文件定位

[源文件 `pkg/kv/unistore.rs`](unistore.rs) 属于 `astersql-kv` crate（`pkg/kv/Cargo.toml`），是 UniStore 单机运行模式在 KV 公共接口层的一项进程级状态声明。`pkg/kv/lib.rs` 以 `#[path = "unistore.rs"] mod unistore_impl;` 编入该文件，再用 `pub use unistore_impl::*;` 从 crate 根导出，因此外部代码通过 `kv::StandAloneTiDB` 访问它，而不是直接访问私有的 `unistore_impl` 模块。

该文件当前只有一个 26 行的公开静态量，没有存储引擎实现、构造函数、trait、条件编译项或 I/O。真正的嵌入式 UniStore 驱动位于 `pkg/store/mockstore/unistore.rs` 及其子目录；这里仅把“当前 TiDB 进程是否选择 UniStore”暴露给需要修正键编码行为的公共层。`pkg/kv/Cargo.toml` 的 `nextgen` feature 负责联动 `kerneltype/nextgen` 与 `keyspace/nextgen`，但本文件本身不受 `#[cfg]` 控制，在 classic 和 nextgen 构建中都存在。

## 核心职责

- 用 `StandAloneTiDB` 保存进程是否以独立 TiDB + UniStore 模式运行；默认值为 `false`。
- 将 Go 的包级可变 `bool` 转成 Rust `AtomicBool`，使启动写入与后续跨线程读取不产生数据竞争。
- 为 nextgen 键处理提供判定依据：UniStore 路径没有 client-go 替调用方移除 keyspace 前缀，因此 `pkg/util/rowcodec/common.rs::RemoveKeyspacePrefix` 在非测试运行时仅于该标志为 `true` 时自行去前缀。

本文件不负责识别配置、注册或创建 UniStore，也不负责解析 keyspace 前缀。配置判定与写入发生在 `cmd/tidb-server/main.rs::createStoreDDLOwnerMgrAndDomain`，前缀处理发生在 `RemoveKeyspacePrefix`。

## 主要符号

- `pub static StandAloneTiDB: std::sync::atomic::AtomicBool`：唯一公开符号，初始为 `AtomicBool::new(false)`。它是 crate 根再导出的全局原子标志，值为 `true` 表示当前进程选择了 `config::StoreTypeUniStore`。
- `AtomicBool::store` / `AtomicBool::load`：不是本文件定义的函数，但构成该符号的完整操作面。生产启动方以 `Ordering::SeqCst` 写入；生产消费方以 `Ordering::Relaxed` 读取。原子性保证无数据竞争，业务正确性则依赖“初始化阶段写入、服务请求阶段读取”的调用顺序。

文件没有复位 API。生产路径只会在 UniStore 配置时写入 `true`，不会在运行期间切换回 `false`；测试会显式保存并恢复全局值以隔离用例。

## 执行流程

1. 静态初始化时，`StandAloneTiDB` 被创建为 `false`。
2. `cmd/tidb-server/main.rs::createStoreDDLOwnerMgrAndDomain` 读取全局配置；当 `Store == StoreTypeUniStore` 时，在初始化 storage 之前执行 `kv::StandAloneTiDB.store(true, Ordering::SeqCst)`。
3. 启动流程随后调用 `initRegisteredStorage`、启动 DDL owner manager，并 bootstrap Domain；本标志自身不参与这些资源的创建或错误处理。
4. row codec 处理键时，`pkg/util/rowcodec/common.rs::RemoveKeyspacePrefix` 先判断内核类型。classic 内核直接保持原键；nextgen 下，若既非测试又非 standalone，则假定 client-go 已处理前缀并保持原键。
5. nextgen 且处于测试或 standalone 模式时，`RemoveKeyspacePrefix` 再校验长度和 API V2 txn mode 首字节，满足条件才切掉 4 字节 keyspace 前缀。

因此该文件位于“启动配置选择 UniStore → 设置进程标志 → row codec 决定是否补做 keyspace 去前缀”的控制链上，而不在 KV 请求读写的数据通路中保存任何键值数据。

## 数据与状态

`StandAloneTiDB` 只有 `false`/`true` 两态：`false` 是默认的 TiKV、MockTiKV 或尚未完成 UniStore 判定的状态，`true` 是已选择 UniStore 的状态。它表达的是进程级存储模式，不是单个事务、请求、keyspace 或 `Storage` 实例的属性。

状态没有所有者对象和析构时机，生命周期等同进程。生产代码当前只有一次条件式 `false → true` 写入，没有热切换协议；若同一进程重复用不同配置初始化，旧的 `true` 不会自动复位。测试 `pkg/kv/mpp_2_aster_unit_test.rs::mpp_2_scope_variables_and_process_flags_match_go` 验证可读写性后复位为 `false`，`pkg/util/rowcodec/common_test.rs::TestRemoveKeyspacePrefix` 则用 `Drop` guard 保存、恢复进入测试前的值。

## 依赖与调用关系

模块装配链是 `pkg/kv/lib.rs` → `unistore_impl` → crate 根公开再导出 `StandAloneTiDB`。本文件唯一直接依赖是 Rust 标准库的 `std::sync::atomic::AtomicBool`，没有为 `pkg/kv/Cargo.toml` 增加专属第三方依赖；Cargo 中的 `nextgen` feature 是其使用场景的相邻边界，而不是本符号的编译门禁。

已核实的生产关系如下：

- 上游写入：`cmd/tidb-server/main.rs::createStoreDDLOwnerMgrAndDomain` 在 `StoreTypeUniStore` 分支写入 `true`，对应 Go 的 `cmd/tidb-server/main.go` 同名函数。
- 下游读取：`pkg/util/rowcodec/common.rs::RemoveKeyspacePrefix` 读取标志，决定 nextgen、非测试环境是否应由 TiDB 侧去除 keyspace 前缀；对应 Go 的 `pkg/util/rowcodec/common.go`。
- 独立测试：`pkg/kv/mpp_2_aster_unit_test.rs` 直接检查原子标志可写可读；`pkg/util/rowcodec/common_test.rs` 联合 `kerneltype`、`intest::InTest` 和本标志验证行为矩阵。

RustCodeGraph 能确认目标文件被纳入索引、`lib.rs` 的装配位置及上述调用文件源码，但当前只把 `pkg/kv/unistore.rs` 识别为一个文件节点，没有为该 `pub static` 建立独立符号/调用边；精确读写关系因此由仓库级符号搜索和对应函数体补证。`cmd/tidb-server/stubs.rs` 还有一个局部桩模块中的同名原子量，不是本文件导出的符号，不应计作这里的生产调用者。

## 错误处理与边界

原子加载和存储没有 `Result`，本文件不会产生业务错误或 panic。若 storage 初始化、PD 检查、DDL owner 启动或 Domain bootstrap 失败，错误由 `createStoreDDLOwnerMgrAndDomain` 返回；已经写成 `true` 的全局标志不会回滚。不过进程启动失败时通常不会进入正常请求服务，这一事实不等于该静态量具备事务式回滚。

标志本身不验证“确实创建了 UniStore”，也不识别 classic/nextgen；调用者必须先基于配置正确写入，消费方必须继续独立检查 `kerneltype::IsClassic()`。它也不决定任意键都能去前缀：`RemoveKeyspacePrefix` 对长度不超过 4 字节或首字节不是 API V2 txn mode 标记的键保持原样。

全局状态是测试隔离边界。并行测试若同时修改它，会相互影响；新增测试必须保存并恢复旧值，并避免让多个修改该全局量的用例无协调并行执行。不能把测试里显式复位的做法误写成生产热切换能力。

## 并发与资源生命周期

`AtomicBool` 让任意线程的读写在 Rust 内存模型下合法，无需锁，也没有分配、通道、任务或清理资源。生产写入使用 `SeqCst`，消费方使用 `Relaxed`；后者只需要读取一个独立模式位，不通过该原子量发布其他内存状态。

可见性之外的时序由应用生命周期保证：主启动线程在创建 storage 和 Domain 前设值，处理键的工作发生在初始化完成之后。若未来允许运行时切换存储模式，单个原子位不足以协调已存在的 storage、连接和在途请求，需要另行设计切换屏障与资源生命周期，不能只追加一次 `store`。

测试中的 `Drop` guard 是异常展开时也能恢复标志的资源管理措施，但它只保护单个测试作用域，不提供跨并行测试的互斥。`mpp_2_aster_unit_test.rs` 的直接复位也假定该测试没有在断言前 panic。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/kv/unistore.go`。Go 定义 `var StandAloneTiDB bool`，注释说明该标志只为 nextgen 引入，因为 UniStore 不会从键中移除 keyspace 前缀；Rust 保留同名和相同默认值，仅把表示类型改成 `AtomicBool` 以适配并发安全。

写入语义与 `cmd/tidb-server/main.go::createStoreDDLOwnerMgrAndDomain` 一致：当全局 store 类型为 UniStore 时设为 `true`。消费语义与 `pkg/util/rowcodec/common.go::RemoveKeyspacePrefix` 一致：classic 不处理；nextgen 的常规非 standalone 生产路径依赖 client-go 去前缀；测试或 standalone 路径由 row codec 检查并移除 API V2 的 4 字节前缀。

Rust 与 Go 的可观察差异是访问语法和同步机制：Go 直接赋值/读取普通 `bool`，Rust 必须显式指定原子内存顺序。Rust 启动写入用了 `SeqCst`，实际消费读取用了 `Relaxed`；这不改变当前单向初始化后的布尔判定结果。Go 测试 `pkg/util/rowcodec/common_test.go::TestRemoveKeyspacePrefix` 与 Rust 同名独立测试覆盖相同四象限意图。

## 扩展指南

- 新增 UniStore 模式判定逻辑时，优先保持写入位置在 `createStoreDDLOwnerMgrAndDomain` 的 storage 初始化之前，并同步检查 Go 同名函数；不要在请求路径中反复从配置推导这一全局状态。
- 修改标志的语义、默认值或名称时，必须同步检查唯一生产消费方 `RemoveKeyspacePrefix`、Go 对照 `pkg/kv/unistore.go`、启动端和两组独立 Rust 测试。该值会改变 nextgen 键的字节视图，错误判定可能造成查找不到键或错误剥离普通键。
- 如需支持复位或运行时切换，应把 storage/Domain 生命周期和在途请求纳入设计，增加显式 API 与同步协议；不应把公开 `AtomicBool` 的任意 `store(false)` 当成安全切换。
- 新增测试不得写进 `unistore.rs`。原子标志自身契约应放在 `pkg/kv` 的独立测试文件；与键编码有关的分支应扩展 `pkg/util/rowcodec/common_test.rs`，并继续使用恢复 guard。若改动 Go/Rust 对齐语义，还应同步 `pkg/util/rowcodec/common_test.go` 的测试意图。
- 若新增更多消费者，应明确各自需要的原子顺序。仅做模式分支可沿用 `Relaxed`；若试图借此发布其他状态，则必须建立并证明相应 happens-before 关系，而不是凭全局布尔量推断资源已就绪。

## 验证依据

- 目标源码与 crate 边界：`pkg/kv/unistore.rs::StandAloneTiDB`、`pkg/kv/lib.rs` 第 555–558 行的模块装配、`pkg/kv/Cargo.toml` 的 crate 与 `nextgen` feature 声明。
- Rust 生产链：`cmd/tidb-server/main.rs::createStoreDDLOwnerMgrAndDomain` 写入标志；`pkg/util/rowcodec/common.rs::RemoveKeyspacePrefix` 读取标志并执行键前缀分支。
- Go 对照：`pkg/kv/unistore.go::StandAloneTiDB`、`cmd/tidb-server/main.go::createStoreDDLOwnerMgrAndDomain`、`pkg/util/rowcodec/common.go::RemoveKeyspacePrefix`。
- 独立测试：`pkg/kv/mpp_2_aster_unit_test.rs::mpp_2_scope_variables_and_process_flags_match_go`；`pkg/util/rowcodec/common_test.rs::TestRemoveKeyspacePrefix`；Go 对照测试 `pkg/util/rowcodec/common_test.go::TestRemoveKeyspacePrefix`。
- RustCodeGraph：`status` 报告索引包含 11,467 个文件且目标文件在列；`files --filter pkg/kv/unistore.rs` 与 `node --file` 确认文件及源码；对 `pkg/kv/lib.rs`、`cmd/tidb-server/main.rs`、`pkg/util/rowcodec/common.rs` 和两份 Rust 测试执行文件节点读取。由于索引未为目标静态量建立节点，`query/node/callers/callees StandAloneTiDB` 无法可靠区分它与桩中同名量，故使用精确仓库搜索核对全部读写点并人工阅读命中函数。
