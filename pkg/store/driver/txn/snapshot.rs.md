# `pkg/store/driver/txn/snapshot.rs`

## 文件定位

本文件位于 `astersql-store-driver-txn` crate 的快照适配层，由同目录 `lib.rs` 以 `mod snapshot` 装配并通过 `pub use snapshot::*` 导出。它向 Rust 事务驱动提供 `tikvSnapshot`、快照读取选项、读拦截器以及隔离级别/优先级转换函数；`txn_driver.rs::NewTiKVTxn` 用共享存储调用 `NewSnapshot`，事务的 `Get`、`BatchGet`、`Iter` 和 `IterReverse` 再在内存写缓冲未覆盖读取时落到该快照。

当前实现与文件名所表达的 TiKV 适配目标之间存在重要边界：`tikvSnapshot` 的后端是 `Arc<RwLock<BTreeMap<Key, ValueEntry>>>`，读取操作没有调用 `tikv-client`。因此它目前是保持 TiDB API 和选项形状的内存适配实现，而不是 Go 版本 `txnsnapshot.KVSnapshot` 所代表的远端 MVCC 时间点快照。`Cargo.toml` 虽声明了带固定 tag `v0.4.2-aster.10` 的 `tikv-client` 依赖，但本文件没有直接使用该依赖。

## 核心职责

- `tikvSnapshot` 提供点查、批量查、正向范围扫描和反向范围扫描，并返回本 crate 的 `ValueEntry` 或 `KvIterator`。
- `SnapshotInterceptor` 为四类读取建立可替换入口；每次回调获得一个已移除拦截器的快照副本，从而允许拦截器继续调用原操作而不会无限递归。
- `SnapshotOption` 与 `SnapshotOptions` 保存隔离级别、副本读、请求来源、资源组、超时等 TiDB/TiKV 读取配置；`SetOption` 完成类型化分派。
- `Getter` 和 `BatchGetter` 实现执行实际映射读取，并借助 `apply_commit_ts_option` / `apply_commit_ts_option_batch` 控制是否向调用者暴露提交时间戳。
- `toTiKVKeys`、`getTiKVIsolationLevel`、`getTiKVPriority` 保留 Go 驱动中的转换边界。

必须区分“保存选项”和“执行选项”：当前实际读取逻辑没有按 `snapshot_ts`、隔离级别、副本类型、资源组、RPC 拦截器或超时改变行为；除读拦截器和 commit-ts 读取选项外，`SnapshotOptions` 主要是可观察的配置状态。

## 主要符号

- `IsoLevel::{SI, RC, RCCheckTS}` 与 `TiKVIsolationLevel::{SI, RC, RCCheckTS}`：分别表示 TiDB 侧输入和本适配器保存的 TiKV 隔离级别；两者由 `getTiKVIsolationLevel` 一一映射，默认均为 `SI`。
- `Priority::{High, Low, Normal}`：读取优先级；`getTiKVPriority(i32)` 按 Go 常量编码把 `1` 映射为 `Low`、`2` 映射为 `High`，其余值回退为 `Normal`。
- `ReplicaReadType::{Leader, Follower, Mixed}`：副本读取偏好，默认 `Leader`。
- `SnapshotRuntimeStats { rpc_count }`、`ResourceGroupTagger::{Proto, Builder}`：分别表示简化的运行统计和资源组标签来源。当前文件只保存它们，不更新 RPC 计数或构造真实协议标签。
- `SnapshotOption`：`SetOption` 的类型安全输入枚举，共覆盖隔离级别、优先级、缓存、时间戳、副本读、统计、标签、请求来源、扫描批量大小和读超时等分支。
- `SnapshotOptions`：已固化配置。`Default` 将隔离级别设为 `SI`、优先级设为 `Normal`、副本读设为 `Leader`，集合/字符串为空，数值与 `Duration` 为零。
- `SnapshotInterceptor: Send + Sync`：定义 `on_get`、`on_batch_get`、`on_iter`、`on_iter_reverse` 四个同步回调。回调可返回正常结果，也可短路并返回 `DriverError`。
- `tikvSnapshot { data, options, interceptor }`：可克隆的快照句柄。克隆会共享 `data` 和拦截器的 `Arc`，但复制一份 `SnapshotOptions`。
- `NewSnapshot` / `tikvSnapshot::from_entries`：前者从共享有序映射构造默认快照；后者收集条目到新的 `BTreeMap`，主要适合测试和本地使用。
- `BatchGet`、`Get`、`Iter`、`IterReverse`：公开读取入口，先处理拦截器，再执行底层读取或建立 `tikvScanner`。
- `range_rows` / `range_rows_reverse`：持读锁完成边界过滤和结果物化的内部辅助函数。
- `Getter::get` / `BatchGetter::batch_get`：实际点查与批量查实现；缺失点查返回 `DriverError::NotFound`，批量查忽略缺失键。

## 执行流程

1. `txn_driver.rs::NewTiKVTxn` 把事务的共享 `storage` 传给 `NewSnapshot`；直接用户也可调用 `NewSnapshot` 或 `from_entries`。
2. 调用者可通过 `SetOption` 更新快照配置。普通选项写入 `self.options`，`SnapInterceptor` 写入独立的 `self.interceptor`；多个 `RPCInterceptor` 追加到向量而不是覆盖；`ScanBatchSize(0)` 被明确忽略；`TiKVClientReadTimeout(u64)` 从毫秒转换为 `Duration`。
3. `Get` 检查拦截器。若存在，则克隆快照、清空克隆的 `interceptor`，再调用 `on_get`；否则进入 `Getter::get`，持读锁查找、克隆 `ValueEntry`，缺失时返回 `NotFound`，最后根据 `GetOption` 决定保留或清零 `commit_ts`。
4. `BatchGet` 采用相同的去递归拦截流程。底层实现只遍历请求键：存在项被克隆并应用批量 commit-ts 选项，不存在项不进入结果 `HashMap`。空请求自然产生空结果。
5. `Iter(key, upper_bound)` 表示半开区间 `[key, upper_bound)`。`range_rows` 利用 `BTreeMap` 的字典序遍历过滤键，并把键和值载荷克隆到 `Vec`，随后交给 `tikvScanner::new`。
6. `IterReverse(key, lower_bound)` 先逆序遍历，再应用 `candidate < key` 与 `candidate >= lower_bound`；两个边界均可省略。其上界是开区间、下界是闭区间。
7. 事务层的上游行为见 `txn_driver.rs`：点查先查 `mem_buffer`，批量查通过 `NewBufferBatchGetter` 合并缓冲与快照，扫描通过 `NewUnionIter` 合并 dirty iterator 与 snapshot iterator。因此快照只负责持久映射侧的读取，不负责事务缓冲覆盖和墓碑合并。

## 数据与状态

`data` 是 `Arc<RwLock<BTreeMap<Key, ValueEntry>>>`。`BTreeMap` 提供确定的字典序扫描，`Arc` 让事务与其克隆快照共享后端，`RwLock` 允许并发读取并由事务提交路径写入。读取会克隆键和值，迭代器建立后不再持有映射锁，也不会观察之后的修改。

这种共享模型不保证真正的时间点隔离：`txn_driver.rs::commit_inner` 会写同一映射，既有 `tikvSnapshot` 的后续读取能看到写入后的状态。`SnapshotOptions::snapshot_ts` 当前也不参与版本筛选。文档或调用方不能据此声称 Rust 实现已具备 Go/TiKV 的 MVCC 快照语义。

`SnapshotOptions` 随 `tikvSnapshot::clone` 按值复制，后续在一个克隆上调用 `SetOption` 不会修改另一个克隆的配置。`data` 和 `Arc<dyn SnapshotInterceptor>` 则继续共享。`options()` 只提供不可变借用，外部必须通过 `SetOption` 修改状态。

`ValueEntry` 来自 `lib.rs`，包含 `value` 和 `commit_ts`。扫描器只携带 `value.value`，不会向 `KvIterator` 暴露提交时间戳；点查和批量查仅在分别出现 `WithReturnCommitTS()` 或 `WithReturnCommitTSBatch()` 时保留它，否则返回副本中的 `commit_ts` 被清零。

## 依赖与调用关系

上游直接接线主要位于 `txn_driver.rs`：`NewTiKVTxn -> NewSnapshot`；`tikvTxn::GetSnapshot` 返回克隆；`tikvTxn::{Get,BatchGet,Iter,IterReverse}` 分别回落或合并 `tikvSnapshot` 的对应读取；`tikvTxn::SetOption` 把隔离级别、优先级、时间戳、副本读、统计、拦截器、读超时等部分 `TxnOption` 转发为 `SnapshotOption`。RustCodeGraph 的符号查询也把 `txn_driver.rs::GetSnapshot`、`NewSnapshot` 和本文件的拦截器方法识别为关联入口。

下游 crate 内依赖由 `lib.rs` 提供：`Key`、`ValueEntry`、`GetOption`、`BatchGetOption`、`Getter`、`BatchGetter`、`KvIterator`、`DriverError`、commit-ts 选项处理函数，以及 `scanner.rs` 中的 `tikvScanner`。标准库依赖是 `BTreeMap`/`HashMap`、`Arc`/`RwLock` 和 `Duration`。

`Cargo.toml` 声明 crate 内部依赖 `astersql-kv`、`astersql-tablecodec`、`astersql-errors` 与固定 tag 的 `tikv-client`；本文件本身只通过 crate 本地抽象和标准库工作。`lib.rs` 还把 `snapshot_test.rs` 作为独立测试模块装配，符合生产逻辑与测试不放在同一 Rust 文件的约束。

## 错误处理与边界

- `Getter::get` 对不存在的键返回 `DriverError::NotFound`；`BatchGetter::batch_get` 则按 Go 契约省略不存在的键，不把它视为错误。
- 读取 `RwLock` 遇到 poison 时使用 `PoisonError::into_inner` 继续读取，而不是传播 panic 或驱动错误。这使服务可继续访问状态，但也意味着锁内先前 panic 后的数据一致性需要由更上层保证。
- 内存扫描构造本身没有可失败步骤，所以无拦截器时 `Iter`/`IterReverse` 总是 `Ok`；拦截器可以返回任意 `DriverError` 并短路读取。
- 正向扫描下界包含、上界排除；反向扫描上界排除、下界包含。`None` 表示对应方向无界。边界比较完全按原始字节字典序。
- 拦截器收到去掉拦截器的克隆，因此回调内再次调用读取会落到实际实现；若此保护被删除，测试中的委托型拦截器会递归。
- `BatchGetter::batch_get` 内部还保留一次拦截器检查，即使公开 `BatchGet` 已检查。这使通过 `Arc<dyn BatchGetter>`（例如事务的三层 batch getter）直接调用 trait 方法时仍然执行拦截器。
- `ScanBatchSize(0)` 不修改原值；未知优先级回退 `Normal`。Rust 的枚举使未知隔离级别无法构造，因而没有 Go `default -> SI` 的运行时分支。

## 并发与资源生命周期

`SnapshotInterceptor` 要求 `Send + Sync`，`Getter` 与 `BatchGetter` 也要求 `Send + Sync`，因此快照可在并发读路径共享。映射访问只在查找或把范围物化为 `Vec` 的阶段持有 `RwLock` 读锁；锁在函数返回前释放，`tikvScanner` 不借用锁或映射。

范围扫描会一次性克隆所有匹配行，资源成本与结果集总键值大小线性相关，而不是 Go `txnsnapshot.Scanner` 的渐进 RPC/批次生命周期。`scan_batch_size` 当前仅保存，不能限制物化量。调用者仍应按 `KvIterator` 契约在完成后调用 `close`；当前 scanner 的真实释放行为由 `scanner.rs` 定义。

克隆 `tikvSnapshot` 不复制整个数据集，只递增 `Arc` 引用计数并复制选项。最后一个 `Arc` 释放后共享存储和拦截器才可销毁。范围扫描克隆出来的行独立存活，之后对共享映射的修改不会改变已创建迭代器的内容。

## 与 Go 版本的对应关系

Go 对照文件是同目录 `snapshot.go`。公开结构大致对应：`NewSnapshot` 包装底层快照；四类读取优先调用 `kv.SnapshotInterceptor`；`SetOption` 覆盖相同的 TiDB 选项；隔离级别与优先级有独立转换函数；正反扫描返回驱动 scanner。

关键差异如下：

- Go `tikvSnapshot` 嵌入 `*txnsnapshot.KVSnapshot`，`Get`/`BatchGet`/扫描调用 client-go 并通过 `extractKeyErr` 或 `derr.ToTiDBErr` 转换后端错误；Rust 使用共享 `BTreeMap`，没有 RPC、区域错误或 key-error 转换。
- Go 的快照由 TiKV MVCC 版本固定；Rust 的 `snapshot_ts` 只是字段，且共享映射会变化，因此当前不提供等价的一致性保证。
- Go 的多数选项立即调用底层 `KVSnapshot` setter；Rust 仅保存在 `SnapshotOptions`。RPC interceptor、资源组、匹配 store label、副本策略和超时目前不会影响实际 I/O。
- Go `toTiKVKeys` 用 `unsafe` 零拷贝重解释 `[]kv.Key`；Rust `toTiKVKeys` 调用 `to_vec()`，会克隆每个 `Vec<u8>`，更安全但有额外分配与复制。
- Go 的 scanner 延迟从底层快照读取；Rust 先完整物化范围。两者边界语义保持一致，但性能和读取时序不同。
- Rust 用类型化 `SnapshotOption` 代替 Go 的整数 option 加 `any` 断言，消除了错误类型断言 panic，但也把部分 Go 接口的动态行为简化成字符串或本地枚举。

## 扩展指南

若要接入真实 TiKV 快照，最可能修改 `tikvSnapshot` 的 `data` 字段和构造入口，以及 `Getter::get`、`BatchGetter::batch_get`、`Iter`、`IterReverse`；需要保留当前半开边界、缺失键、commit-ts 和去递归拦截语义，并在 `error.rs` 建立与 Go `extractKeyErr`/`ToTiDBErr` 对应的错误转换。不要仅让选项字段存在：应逐项把 `SnapshotOptions` 接到已发布 tag 的 `tikv-client` API，并验证隔离级别、副本读、资源组、超时和统计的实际效果。

若新增选项，应同时扩展 `SnapshotOption`、`SnapshotOptions`、`Default` 与 `SetOption`，并判断 `txn_driver.rs::TxnOption` 是否需要转发。累积型选项（如 RPC interceptor）和覆盖型选项必须明确区分；时间单位转换应继续在边界处完成。

测试必须放在独立文件。局部转换与默认值可扩展 `pkg/store/driver/txn/snapshot_test.rs`；公开读范围、commit-ts 与拦截器委托应扩展 `pkg/store/driver/snap_interceptor_test.rs`；与 memBuffer 合并及 `TxnOption` 转发应扩展 `pkg/store/driver/txn_test.rs` 或同目录 `txn_driver_test.rs`。性能风险集中在范围全量物化、键克隆和大批量 `toTiKVKeys` 复制；兼容风险集中在边界开闭、未命中行为、拦截器只执行一次以及 Go 选项语义缺失。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/store/driver/txn` 确认 `snapshot.rs`、`snapshot.go`、`snapshot_test.rs`、`txn_driver.rs` 与 `lib.rs` 均已索引；`node --file pkg/store/driver/txn/snapshot.rs --offset 1 --limit 500` 读取目标文件全部 439 行；`query TiKVSnapshot`、`query SnapshotInterceptor`、`query NewSnapshot --kind function --json`、`query BatchGet --kind method --json` 和 `query SetOption --kind method --json` 用于定位本文件及事务/KV 层相关符号。限定名 `callers/callees` 查询未返回可用边，因此调用关系又由直接入口源码核验，没有据此推断缺失边。
- 源文件：`pkg/store/driver/txn/snapshot.rs`，核对全部枚举、结构、trait、函数、impl、边界表达式、锁处理和选项分支；该文件没有条件编译项。
- crate 与入口：`pkg/store/driver/txn/Cargo.toml`、`pkg/store/driver/txn/lib.rs`、`pkg/store/driver/txn/txn_driver.rs`，核对 crate 归属、依赖 tag、模块导出、独立测试装配、构造入口、读取回落/合并和选项转发。
- Go 对照：`pkg/store/driver/txn/snapshot.go`，核对真实 `KVSnapshot` 后端、读取与错误转换、拦截器、选项 setter、扫描器及转换函数。
- Rust 测试：`pkg/store/driver/txn/snapshot_test.rs` 验证优先级映射；`pkg/store/driver/snap_interceptor_test.rs` 验证无/有拦截器时的点查、批量查、commit-ts、范围边界和错误短路；`pkg/store/driver/txn_test.rs` 验证事务缓冲优先、未解析键回落快照、UnionIter 扫描和拦截错误传播。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `rg` 结构命令确认恰有十一个固定二级标题，并人工复核文档没有把未接线的配置或 MVCC/RPC 能力写成已支持。
