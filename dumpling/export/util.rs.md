# `dumpling/export/util.rs`

## 文件定位

`util.rs` 是 `astersql-dumpling-export` library crate 的包级辅助实现。它不是独立 Rust 模块：[`dumpling/export/lib.rs`](lib.rs) 先在 crate 根导入 `HashMap`、`mpsc::{Sender, Receiver}`、`thread` 等标准库符号和 `astersql_dumpling_context as tcontext`，再以 `include!("util.rs")` 把本文件直接拼入 crate 根。因此这里的公开函数成为 crate 根符号，且可以直接使用同一作用域中的 `DB`、`EtcdClient`、`ServerType`、`Result` 等桩类型。

[`dumpling/export/Cargo.toml`](Cargo.toml) 将该 crate 声明为 Go 包 `dumpling/export` 的 Rust library 移植；注释明确说明当前在 arm64 Darwin 上用本地 stubs 代替 SQL/MySQL/storage/HTTP/metrics 等重依赖。本文件所用的 etcd 能力正来自 [`dumpling/export/stubs.rs`](stubs.rs) 的内存实现，不是 Cargo 中声明的真实 etcd 客户端。

## 核心职责

本文件汇集四类彼此独立、但被导出流程共享的工具：

1. `getPdDDLIDs` 从 `/tidb/server/info` 前缀记录的 key 尾段提取 DDL 实例 ID；`checkSameCluster` 将这些 ID 与 SQL 查询得到的 TiDB 实例 ID 排序后比较，用作“PD 是否属于同一集群”的判据。
2. `string2Map` 将两组等长字符串按下标配成列名到列类型等映射。
3. `needRepeatableRead` 决定连接是否需要 `REPEATABLE READ`；只有 TiDB 的 snapshot consistency 组合不需要。
4. `infiniteChan` 用两个 `std::sync::mpsc` 通道、一个队列和两个后台线程模拟 Go 的无限缓冲 channel，并在所有输入发送端关闭后排空队列。

这些职责对应 [`dumpling/export/util.go`](util.go) 的同名实现，但 Rust 版本仍处在迁移期：集群检查依赖可注入错误的离线 `EtcdClient` 桩，且 RustCodeGraph 未找到 `checkSameCluster` 和 `infiniteChan` 的生产调用者。

## 主要符号

- `pub const tidbServerInformationPath: &str = "/tidb/server/info"`：TiDB server info 在 etcd 中的前缀；`getPdDDLIDs` 用它做前缀扫描。
- `pub const defaultEtcdDialTimeOut: Duration = 3s`：构造 `EtcdClientConfig` 时的拨号超时。它对齐 Go 常量，但当前桩构造器仅校验 endpoint 非空，并不执行拨号。
- `etcdAutoSyncInterval: Duration = 30s`：`checkSameCluster` 的客户端自动同步配置，文件内私有。
- `etcdGetTimeout: Duration = 10s`：`getPdDDLIDs` 传给桩读取接口的超时，文件内私有；桩会记录该值供测试断言。
- `pub fn getPdDDLIDs(cli: &EtcdClient) -> Result<Vec<String>>`：读取前缀下全部键值，忽略 value，以 `/` 切分 key 并收集最后一段。读取错误通过 `?` 原样返回。
- `pub fn checkSameCluster(tctx: &tcontext::Context, db: &DB, pd_addrs: &[String]) -> Result<bool>`：构造 etcd 客户端，调用 `GetTiDBDDLIDs` 与 `getPdDDLIDs`，分别排序后做向量相等比较；任一步错误立即返回。
- `pub fn string2Map(a: &[String], b: &[String]) -> HashMap<String, String>`：按 `a` 的索引读取 `b[i]` 并插入 map；重复 key 由后值覆盖。
- `pub fn needRepeatableRead(server_type: ServerType, consistency: &str) -> bool`：表达式为 `consistency != "snapshot" || server_type != TiDB`。
- `pub fn infiniteChan<T: Send + 'static>() -> (Sender<T>, Receiver<T>)`：返回面向调用者的输入 sender 和输出 receiver；`T` 必须能跨线程移动并拥有静态生命周期。

## 执行流程

`checkSameCluster` 的流程是：用 `pd_addrs`、3 秒拨号超时和 30 秒自动同步间隔组装 `EtcdClientConfig`；创建客户端；通过 [`GetTiDBDDLIDs`](sql.rs) 执行 `SELECT DISTINCT TIDB_INSTANCE_ID FROM INFORMATION_SCHEMA.CLUSTER_INFO`；通过 `getPdDDLIDs` 以 10 秒超时扫描 server-info 前缀；对两侧 ID 排序；最后比较两个向量。排序使结果不受返回顺序影响，但比较保留数量语义，并非先去重后的集合比较。

`string2Map` 从左切片逐项遍历，克隆 key 与同位置 value 后插入 `HashMap`。它在 [`GetPrimaryKeyAndColumnTypes`](dump.rs) 中把元数据列名映射为列类型，在 [`getNumericIndex`](sql.rs) 中为索引列的数值类型判断建立查找表。

`needRepeatableRead` 在 [`Dumper::Dump`](dump.rs) 建立一致性连接之前执行。TiDB snapshot 已由快照时间点提供一致读，因此返回 `false`；其他 server/consistency 组合返回 `true`。Go 主链还会在每个 writer 建连时重复使用此判定。

`infiniteChan` 先创建内部 `Sender<Option<T>>`/`Receiver<Option<T>>` 和输出 `Sender<T>`/`Receiver<T>`。转发线程在队列非空时先用 `try_recv` 尽量吸收新输入，否则移除队首并阻塞发送到输出；队列为空时阻塞等待输入。第二个桥接线程读取调用者通道，将每个值包装为 `Some` 发入内部通道；全部用户 sender 被丢弃后发送 `None`，让转发线程按顺序排空并退出。

## 数据与状态

集群比较本身不保留跨调用状态。`EtcdClient` 桩持有 `Arc<Mutex<HashMap<String, String>>>`、可注入的 `get_error` 和 `last_get_timeout`；`getPdDDLIDs` 会更新最后读取超时，并克隆符合前缀的 key/value。当前 `EtcdClient::New` 每次创建一个新的空内存 map，因此 `checkSameCluster` 并不会访问真实 PD/etcd 数据。

`string2Map` 返回拥有自身 `String` 的新 `HashMap`，不借用输入。其容量预分配为 `a.len()`；重复 key 不增加条目数，后插入值覆盖先前值。

`infiniteChan` 的可变状态全部归转发线程所有：`Vec<T>` 是待发送 FIFO 队列，通道承担线程间所有权转移。没有共享锁；代价是缓存无上限，且 `Vec::remove(0)` 每次都会搬移剩余元素，在长积压时具有线性出队成本。

## 依赖与调用关系

RustCodeGraph 对本文件识别出 12 个符号，并确认以下下游边：`checkSameCluster -> getPdDDLIDs`，以及对 `defaultEtcdDialTimeOut`、`etcdAutoSyncInterval` 的引用；`getPdDDLIDs` 引用 `tidbServerInformationPath` 与 `etcdGetTimeout`。图查询没有为 `getPdDDLIDs`、`checkSameCluster`、`string2Map`、`needRepeatableRead` 或 `infiniteChan` 给出静态 caller，原因之一是 crate 使用 `include!` 组成单包视图；因此调用点又以源码检索核验。

已核实的 Rust 生产调用点是：[`dumpling/export/dump.rs`](dump.rs) 的 `Dumper::Dump` 调用 `needRepeatableRead`，同文件的 `GetPrimaryKeyAndColumnTypes` 调用 `string2Map`；[`dumpling/export/sql.rs`](sql.rs) 的 `getNumericIndex` 也调用 `string2Map`。`checkSameCluster`、`getPdDDLIDs` 和 `infiniteChan` 当前只有本文件内部关系或测试引用，不能宣称已接入 Rust 导出主链。

主要下游定义包括：`GetTiDBDDLIDs` 位于 `sql.rs`；`EtcdClient`、`EtcdClientConfig`、`DB`、`ServerType` 与项目 `Result/Error` 位于 `stubs.rs`；`ConsistencyTypeSnapshot` 位于 [`dumpling/export/consistency.rs`](consistency.rs)。Cargo 层直接依赖 `astersql-dumpling-context`，而其余名称由 `lib.rs` 的 crate 根导入或重导出。

## 错误处理与边界

- `getPdDDLIDs` 只显式处理读取错误：`GetPrefixWithTimeout` 的错误直接传播。它假定每个 key 都能按 `/` 切分；Rust `split` 至少产生一个元素，所以索引最后一项不会越界，但尾随 `/` 会产生空 ID，代码不会过滤或报错。
- `checkSameCluster` 会传播客户端构造、SQL 查询和 etcd 读取错误。空 endpoint 在当前桩中返回 `etcdclient: no available endpoints`。两侧均为空时会返回 `true`，因为它只比较排序后的向量。
- 所谓“同集群”实际要求两个排序向量完全相等；重复 ID 数量不同也会判为不等。这与 Go 的 `slices.Sort` 加 `slices.Equal` 一致。
- `string2Map` 假定 `b.len() >= a.len()`；若 values 更短，`b[i]` 索引会 panic。若 `b` 更长，多余 value 被忽略。重复 key 后值覆盖前值。
- `infiniteChan` 的用户 `send` 在标准库无界通道上不受队列容量限制，但仍会在接收链已经断开时返回错误。若输出 receiver 提前丢弃，转发线程在普通发送分支退出；关闭输入后的 drain 分支忽略每次输出发送错误并最终退出。
- 后台线程无法向调用者返回内部错误，也没有 join handle；其生命周期完全由通道关闭驱动。

## 并发与资源生命周期

`infiniteChan` 每次调用固定创建两个后台线程。调用者可克隆输入 `Sender<T>`；只有所有克隆都 drop 后，桥接线程的 `for v in user_rx` 才结束并向内部通道发送关闭哨兵。内部通道保持单一桥接 sender，因此数据和随后的 `None` 保持发送顺序；转发线程收到关闭后先 drain，再丢弃 `out_tx`，输出 receiver 随之观察到关闭。

该实现的“无限”是逻辑上的无界缓存，不代表零成本或资源无限：慢消费者会使 `Vec<T>` 持续增长并占用内存；队首删除还可能放大 CPU 搬移成本。输出消费使用阻塞 `send`，所以转发线程可能等待消费者，但用户生产者通常只把数据放入前级无界通道，不直接等待输出。

`getPdDDLIDs` 读取桩内 map 时短暂持有 mutex；返回值是克隆数据，锁在函数返回前释放。`checkSameCluster` 创建的桩客户端仅在函数栈内存活，没有显式关闭操作；这不应外推为真实 etcd 客户端的资源管理行为。

## 与 Go 版本的对应关系

[`dumpling/export/util.go`](util.go) 是直接对照源。常量值、ID 尾段提取、排序后向量比较、下标配对、重复 key 覆盖、RR 判定布尔式和无限通道的 FIFO/drain 意图均保持一致。

差异主要来自迁移基础设施：Go `getPdDDLIDs` 从父 context 派生 10 秒超时并访问真实 `clientv3.Client`；Rust 接受不带 context 的内存 `EtcdClient`，仅把 10 秒记录到桩状态。Go `checkSameCluster` 创建真实 etcd client，Rust 构造离线桩。Go `infiniteChan` 用一个 goroutine 和 `select` 在接收与发送间调度；Rust 用桥接线程加转发线程模拟，并用 `try_recv` 偏向吸收新输入，队列数据结构也从切片头移除变为 `Vec::remove(0)`。因此行为目标相同，但调度、公平性、内存与 CPU 特性不能视为完全等价。

Go 当前生产代码在 `dump.go` 中使用 `infiniteChan` 分发导出任务，并在经典集群 GC 初始化中调用 `checkSameCluster`；对应 Rust `dump.rs` 尚未出现这两个调用。Go `sql.go` 与 `dump.go` 使用 `string2Map`，Rust 已有对应调用；两边都用 `needRepeatableRead` 控制连接隔离级别。

## 扩展指南

若把集群校验接入 Rust 生产链，应先替换或抽象 `stubs.rs` 中的 `EtcdClient`，保留 endpoint、拨号超时、自动同步和 10 秒读取截止语义，并为真实客户端补齐关闭/取消生命周期；随后在与 Go `dump.go` 的经典集群 GC 初始化相对应的位置调用 `checkSameCluster`。不能仅让现有内存桩返回期望值便宣称完成真实接线。

修改 ID 解析或同集群判定时，应同步 [`dumpling/export/util_test.rs`](util_test.rs) 中的前缀过滤、超时、读取错误和排序比较用例，并考虑空 key、尾随斜杠、重复 ID、两侧空集合以及 SQL 查询/关闭错误。若改 `string2Map` 的长度约束或重复键策略，需同步它的 panic 与覆盖测试，并复查 `GetPrimaryKeyAndColumnTypes`、`getNumericIndex` 的列名/类型等长不变量。

修改 `needRepeatableRead` 时应同步 Rust 的 server × consistency 矩阵、[`dumpling/export/parity_test.rs`](parity_test.rs) 的 Go/Rust 契约锚点及 Go `util_test.go`。修改 `infiniteChan` 时应保留顺序、无丢失、输入关闭后 drain、输出提前关闭不泄漏线程等性质；相关测试继续放在独立 `util_test.rs`，不要内嵌到生产源文件。性能扩展宜优先消除 `Vec::remove(0)` 的线性成本，并明确是否仍接受真正无界内存增长。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter dumpling/export/util.rs` 确认目标已索引并含 12 个符号。
- RustCodeGraph `node --file`：完整读取 `dumpling/export/util.rs`，并读取 `lib.rs`、`util_test.rs`、`util.go`、`util_test.go`、`stubs.rs`、`sql.rs`、`dump.rs`、`consistency.rs` 与 `parity_test.rs` 的相关区段。
- RustCodeGraph `query`：定位 `getPdDDLIDs`、`checkSameCluster`、`string2Map`、`needRepeatableRead`、`infiniteChan` 及其 Go 对照；定位 `EtcdClient`、`EtcdClientConfig`、`GetTiDBDDLIDs`、`DB`、`ServerType`、`ConsistencyTypeSnapshot` 的真实定义。
- RustCodeGraph `callers/callees`：确认 `checkSameCluster -> getPdDDLIDs` 及常量引用；caller 结果为空后，以 `rg` 核验 Rust 与 Go 的实际调用点，并据此明确未接线边界。
- Cargo/模块证据：`dumpling/export/Cargo.toml` 的 `[lib] path = "lib.rs"`、porting metadata 和依赖声明；`lib.rs` 的 crate 根 imports、`pub use stubs::*`、`include!("util.rs")` 与独立 `#[cfg(test)] mod util_test`。
- 测试证据：`util_test.rs` 覆盖 RR 组合、10,000 项 FIFO、关闭后 drain、etcd 前缀/10 秒超时/错误传播、空 endpoints、重复 map key 和短 values panic；`parity_test.rs` 再覆盖 TiDB snapshot 与 MySQL snapshot 的关键差异。Go `util_test.go` 覆盖原始 RR 矩阵与 10,000 项无限通道顺序。
- 本任务是纯文档分析，按计划未运行 Cargo；最终只执行固定十一章节的结构验证，并人工复核当前实现、Go 对照和未接线限制均有直接证据。
