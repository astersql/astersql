# `pkg/util/workloadrepo/const.rs`

## 文件定位

`const.rs` 是 `astersql-util-workloadrepo` crate 的共享常量表。模块入口 `pkg/util/workloadrepo/lib.rs` 通过 `#[path = "const.rs"] mod consts;` 装载它，并以 `pub use consts::*;` 将其中的 16 个 `pub const` 重新导出为 crate 级 API。该文件本身没有函数、类型、trait、`impl` 或条件编译项，也不执行初始化逻辑；真正消费这些值的是 `worker.rs`、`snapshot.rs`、`sampling.rs`、`table.rs`、`housekeeper.rs` 和 `utils.rs`。

crate 边界由 `pkg/util/workloadrepo/Cargo.toml` 确定：库入口是同目录 `lib.rs`，Go 对照包是 `pkg/util/workloadrepo`，唯一声明的第三方依赖为 `chrono`。`const.rs` 只定义整数和静态字符串，不直接使用 `chrono` 或其他外部依赖。

## 核心职责

该文件把多个实现文件必须一致使用的协议值集中在一个位置：

- `ownerKey`、`promptKey`、`snapIDKey` 定义 owner/快照协调所用的名称和 etcd 键空间。
- `snapshotRetries` 限制快照号分配的重试次数，避免 CAS 冲突或后端失败时无限循环。
- `defSamplingInterval`、`defSnapshotInterval`、`defRententionDays` 为新建 worker 提供默认秒数/天数。
- `histSnapshotsTable`、`workloadSchema` 固定历史元数据表和目标 schema 的 SQL 标识符。
- 四个 `repository*` 常量固定面向系统变量层及错误消息的变量名。
- 三个 `err*` 常量提供跨实现分支比较或返回的稳定文本。

这些常量不自行校验配置、不访问 etcd、也不执行 SQL；它们是调用代码的协议输入。修改其值可能改变持久化键、SQL 对象名、用户可见变量名或错误文本，因而不能按普通内部字符串重命名。

## 主要符号

| 符号 | 类型和值 | 当前语义与直接证据 |
| --- | --- | --- |
| `ownerKey` | `&str = "/tidb/workloadrepo/owner"` | owner 竞选键。Go 的 `worker.start` 把它传给 `newOwner`（`worker.go:370-373`）；当前 Rust 生产实现由 `RepositoryBackend::is_owner` 抽象 owner 判断，Rust 搜索未发现定义之外的生产引用。 |
| `promptKey` | `&str = "workloadrepo"` | owner 日志/提示名；Go 与 `ownerKey` 一起传入 owner 构造器，当前 Rust 生产实现未直接消费。 |
| `snapIDKey` | `&str = "/tidb/workloadrepo/snap_id"` | `Worker::getSnapID` 读取它，`Worker::takeSnapshot` 通过 create/CAS 更新它（`snapshot.rs:55-60,128-160`）。 |
| `snapshotRetries` | `usize = 5` | `Worker::takeSnapshot` 的循环上界（`snapshot.rs:128-162`）。 |
| `defSamplingInterval` | `i32 = 5` | `initializeWorker` 的默认主动采样间隔（`worker.rs:157-173`）。 |
| `defSnapshotInterval` | `i32 = 3600` | `initializeWorker` 的默认快照间隔（`worker.rs:157-173`）。 |
| `defRententionDays` | `i32 = 7` | `initializeWorker` 的默认保留天数（`worker.rs:157-173`）；符号沿用 Go 中已有的 `Rentention` 拼写。 |
| `histSnapshotsTable` | `&str = "HIST_SNAPSHOTS"` | 默认元数据表、快照号恢复查询和元数据 upsert/update 共用的表名（`worker.rs:107-123`，`snapshot.rs:39-89`）。 |
| `workloadSchema` | `&str = "WORKLOAD_SCHEMA"` | Rust 生成建表、插入、快照和分区 SQL 时使用的 schema 标识符（`table.rs:23-92`，`housekeeper.rs:27-67`，`snapshot.rs:39-89`）。 |
| `repositoryDest` | `&str = "tidb_workload_repository_dest"` | `validateDest` 的用户可见错误名（`utils.rs:134-148`）。 |
| `repositoryRetentionDays` | `&str = "tidb_workload_repository_retention_days"` | `setRetentionDays` 解析失败时的用户可见变量名（`utils.rs:123-131`）。 |
| `repositorySamplingInterval` | `&str = "tidb_workload_repository_active_sampling_interval"` | `changeSamplingInterval` 解析失败时的变量名（`sampling.rs:72-85`）。 |
| `repositorySnapshotInterval` | `&str = "tidb_workload_repository_snapshot_interval"` | `changeSnapshotInterval` 解析失败时的变量名（`snapshot.rs:188-200`）。 |
| `errKeyNotFound` | `&str = "key not found"` | `getSnapID` 在 KV 返回空值时生成的哨兵文本，`takeSnapshot` 以字符串相等识别恢复分支（`snapshot.rs:53-60,128-147`）。 |
| `errWorkloadNotStarted` | `&str = "Workload repository is not enabled"` | 全局 worker 缺失或未启用时由 `worker::takeSnapshot` 返回（`worker.rs:182-193`）。 |
| `errUnsupportedEtcdRequired` | `&str = "etcd client required for workload repository"` | `worker::start` 发现后端不支持 etcd 时返回（`worker.rs:275-287`）。 |

所有符号均为不可变的编译期常量。字符串为 `'static` 借用，不产生堆分配；只有消费者在构造 `String`、SQL 或错误时才会复制/格式化。

## 执行流程

文件本身没有可执行控制流；常量进入工作负载仓库主链的顺序如下：

1. `initializeWorker` 用三个 `def*` 常量初始化 `WorkerState`，以 5 秒采样、3600 秒快照和 7 天保留作为内部默认值（`worker.rs:157-173`）。
2. `defaultWorkloadTables` 用 `workloadSchema` 与 `histSnapshotsTable` 生成元数据表 DDL；`table.rs` 后续继续用 `workloadSchema` 构造目标表 DDL/DML（`worker.rs:107-154`，`table.rs:23-92`）。
3. 快照触发后，`getSnapID` 从 `snapIDKey` 读取当前编号。键不存在时返回 `errKeyNotFound`，`takeSnapshot` 转而查询 `WORKLOAD_SCHEMA.HIST_SNAPSHOTS` 的最大编号；随后先 upsert 新元数据行，再对同一键执行 create 或 CAS（`snapshot.rs:39-74,125-163`）。
4. 第 3 步中的读取、查询、upsert 或 KV 更新失败会继续重试，但总次数受 `snapshotRetries` 限制；成功则返回递增后的编号，耗尽次数则返回最后一次错误文本。
5. 采样间隔、快照间隔、保留天数及目的地的 hook 使用四个 `repository*` 名称生成可定位到具体系统变量的错误。范围钳制并不在这些常量或 hook 内完成；Rust 注释和独立测试表明其预期由外部系统变量层处理（`sampling_test.rs:8-21`，`snapshot_test.rs:127-134`）。
6. housekeeper 使用 `workloadSchema` 对各目标表执行增删分区；保留天数为 0 时上层逻辑跳过删除（`housekeeper.rs:27-90`）。

`ownerKey`/`promptKey` 在 Go 流程中参与 owner 构造，而当前 Rust worker 只调用后端的 `is_owner()`/`etcd_available()`；因此这两个 Rust 常量目前代表与 Go 保持一致的协议值和未来接线点，不应描述成已参与 Rust owner 竞选。

## 数据与状态

`const.rs` 不持有运行时状态。三个数值默认值会被复制到 `WorkerState::{samplingInterval,snapshotInterval,retentionDays}`，之后可由配置 hook 修改；修改 worker 状态不会反写常量（`worker.rs:90-94,157-173`）。

`snapIDKey` 指向的值位于后端 KV/etcd，而 `histSnapshotsTable` 指向的元数据位于 SQL 仓库。`Worker::takeSnapshot` 用 SQL 最大 `SNAP_ID` 在 KV 键丢失时恢复编号，再以 create/CAS 提交新编号。这两个数据源共同维护快照序列，但 `const.rs` 只定义它们的寻址名称。

`workloadSchema` 和表名被插入内部固定模板，表的动态名称则由 `table.rs::identifier` 转义。当前两个固定常量不接收用户输入。四个系统变量名是外部兼容面：Go `worker.go:150-197` 用相同名称注册全局系统变量；Rust crate 当前只在 hook 错误消息中使用这些名称，未在本 crate 内完成等价注册。

## 依赖与调用关系

模块依赖方向是 `lib.rs -> consts`，再由 `pub use consts::*` 让同 crate 模块通过 `use crate::*` 或根路径消费常量。直接 Rust 数据依赖为：

- `worker.rs`：`histSnapshotsTable`、`workloadSchema`、三个 `def*` 默认值、`errWorkloadNotStarted`、`errUnsupportedEtcdRequired`。
- `snapshot.rs`：`snapIDKey`、`snapshotRetries`、`histSnapshotsTable`、`workloadSchema`、`repositorySnapshotInterval`、`errKeyNotFound`。
- `sampling.rs`：`repositorySamplingInterval`。
- `table.rs`、`housekeeper.rs`：`workloadSchema`。
- `utils.rs`：`repositoryRetentionDays`、`repositoryDest`。
- 独立测试 `worker_test.rs`、`sampling_test.rs`、`utils_test.rs`：分别验证快照键恢复和变量名进入错误信息等行为。

RustCodeGraph 的 `node` 查询识别到 `snapshot.rs::takeSnapshot` 调用 `getSnapID`、`queryMaxSnapID`、`upsertHistSnapshot`、`etcdCreate` 和 `etcdCAS`，与上述快照号流程一致。但当前索引只把 `const.rs` 识别为一个文件级节点，精确查询 `snapshotRetries`、`repositoryDest` 等常量返回空结果，`ownerKey` 查询还命中了别处的近似名称；因此常量级引用以 `rg` 的逐符号结果和所列源码位置为准，而不采用 RustCodeGraph 的错误“used by”文件摘要。

## 错误处理与边界

这些错误常量在 Rust 中只是 `&str`，不会携带 TiDB 错误码、错误类别、堆栈或来源链：

- `errKeyNotFound` 被转换成 `String` 后再通过 `error == errKeyNotFound` 做精确文本比较。若改动大小写或措辞，会使 KV 缺键无法进入 SQL 恢复分支。
- `errWorkloadNotStarted` 同时覆盖“全局 worker 未安装”和“worker 已安装但未启用”两种状态，调用者不能仅凭文本区分二者。
- `errUnsupportedEtcdRequired` 只表示启动前置条件不满足；它不负责探测或重连 etcd。

数值常量也有明确边界：`snapshotRetries = 5` 表示最多五次完整分配尝试，不是每个后端操作各重试五次；三个 `i32` 默认值只负责初始化，不表达系统变量的合法范围。Go 的系统变量注册给出了保留天数 `0..=365`、采样间隔 `0..=600`、快照间隔 `900..=7200`（`worker.go:165-197`），而当前 Rust hook 会接受任何可解析的对应整数，边界校验依赖 crate 外部接线。

## 并发与资源生命周期

静态常量天然只读且可跨线程共享，不需要锁，也没有析构或资源释放。并发语义来自消费者：

- `snapIDKey` 是集群级协调点；`Worker::takeSnapshot` 通过 create/CAS 和 `snapshotRetries` 处理竞争，而不是依赖进程内锁保证全局唯一（`snapshot.rs:125-163`）。
- 默认参数被复制进受 `Mutex<WorkerState>` 保护的实例状态，后续读取/更新由 worker 的锁管理（`worker.rs:98-102,157-173`）。
- `workloadSchema`/`histSnapshotsTable` 可被多个快照线程只读复用；`startSnapshot` 的 scoped threads 共享 worker，但不会修改常量（`snapshot.rs:165-186`）。
- `ownerKey` 在 Go 中标识跨节点 owner 竞选资源；当前 Rust 后端接口只暴露 `is_owner()`，所以此文件不能证明 Rust 已管理 owner session、租约或释放生命周期。

调整 KV 键或 SQL 名称会形成跨版本生命周期问题：旧节点、旧 KV 数据和既有表不会随编译期常量自动迁移，必须设计兼容读取/迁移和滚动升级策略。

## 与 Go 版本的对应关系

`pkg/util/workloadrepo/const.go` 是主要对照。Rust 对 `ownerKey`、`promptKey`、`snapIDKey`、`snapshotRetries`、三个默认值、`histSnapshotsTable` 以及三条错误文案保持同值；四个 `repository*` 名称来自 Go `worker.go:61-66`。

当前差异需要明确保留在理解和扩展中：

- Go 的 `etcdOpTimeout = 5 * time.Second`、`workloadSchemaCIStr`、`zeroTime`、`errWrongValueForVar` 和 `errCouldNotStartSnapshot` 没有移植到本 Rust 常量文件。不能据此推断 Rust 已实现对应超时、CIStr、零时间或错误包装语义。
- Go SQL 通过 `mysql.WorkloadSchema` 获得 schema 名；Rust 用本文件的字符串 `workloadSchema` 直接拼接。
- Go 的 `errUnsupportedEtcdRequired`、`errWorkloadNotStarted` 是 `dbterror` 标准错误，分别带 TiDB 错误码；Rust 对应项是普通字符串，错误分类和堆栈语义尚不等价。
- Go `init` 注册四个系统变量并把手工快照入口接到 executor（`worker.go:150-197`）；Rust `worker.rs::init` 当前为空。因此常量名称已对齐，不代表系统变量注册链已经对齐。
- Go `initializeWorker` 创建并停止 ticker、准备 wait group，并保留 owner/etcd/session 资源；Rust 只初始化后端、表和受锁状态。常量默认值相同，但资源模型不同。
- Go 和 Rust 都保留拼写 `defRententionDays`，这是源码兼容名称而不是新的英文约定。

Go `worker_test.go:495-562` 验证系统变量范围、目的地枚举及变量名对应关系；Go `worker_test.go:1035-1055` 验证快照键读写与缺键错误。Rust 的对应独立测试分布在 `sampling_test.rs`、`snapshot_test.rs`、`utils_test.rs` 和 `worker_test.rs`，未把测试嵌入生产源文件。

## 扩展指南

新增或修改常量时应先判断其兼容面，再修改最小的消费链：

1. 修改 `snapIDKey`、`ownerKey` 或 schema/表名之前，先设计旧键/旧表的发现、迁移、双读或回滚方案；仅替换字符串会割裂滚动升级中的节点和已有数据。
2. 修改默认间隔/保留天数时，同步核对 Rust `initializeWorker`、Go `initializeWorker`、Go 系统变量默认值与 min/max，以及定时器创建位置。不要在 hook 内额外钳制而破坏当前“系统变量层先校验”的约定。
3. 新增系统变量名时，Rust 常量、未来的注册接线、解析 hook 和 Go 对照应保持一致；错误消息测试应断言真实变量名。
4. 新增可分支判断的错误时，优先引入结构化错误或枚举并让调用者按类型匹配；继续用字符串哨兵会使文案修改影响控制流。若必须保持 Go 错误码兼容，还需在本 crate 外的错误映射层验证。
5. 对 `ownerKey`/`promptKey` 的 Rust 接线必须进入 owner/后端边界，并增加独立测试验证键和 prompt 确实传给 owner 实现；不要仅凭常量存在宣称支持竞选。
6. 测试应继续放在同目录独立文件：快照键与重试放在 `snapshot_test.rs` 或 `worker_test.rs`，默认状态/启动错误放在 `worker_test.rs`，采样变量放在 `sampling_test.rs`，保留天数和目的地放在 `utils_test.rs`。不要把测试模块内嵌回 `const.rs`。

主要风险分别是：协议/持久化键改变导致兼容性故障；重试或间隔增大带来后端负载和延迟；错误文本变化破坏 `errKeyNotFound` 的控制流判断；只改 Rust 或只改 Go 导致双实现语义漂移。

## 验证依据

本说明基于以下直接证据：

- 目标与装配：`pkg/util/workloadrepo/const.rs:1-41`、`pkg/util/workloadrepo/lib.rs:1-44`、`pkg/util/workloadrepo/Cargo.toml`。
- Rust 消费链：`worker.rs:107-193,275-287`、`snapshot.rs:39-200`、`sampling.rs:72-85`、`table.rs:23-92`、`housekeeper.rs:27-90`、`utils.rs:123-149`。
- Rust 独立测试：`worker_test.rs:558-566` 验证缺键时从 SQL 最大值恢复并写入 `snapIDKey`；`snapshot_test.rs:127-134` 验证快照间隔 hook 不额外钳制；`sampling_test.rs:8-21` 验证采样 hook 与变量名错误；`utils_test.rs:45-69` 验证保留天数解析和目的地错误文本。
- Go 对照：`const.go:28-51`、`worker.go:61-66,132-215,370-373`、`snapshot.go:81-200`、`worker_test.go:495-562,1035-1055`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件；`files --filter pkg/util/workloadrepo` 定位 22 个 Go/Rust 文件；`node --file pkg/util/workloadrepo/const.rs` 核对完整 41 行；`node snapshot.rs::takeSnapshot` 核对其五条下游调用边。常量级 `query/callers/callees` 不能可靠解析本文件，已用 `rg` 的逐符号引用结果补证并在“依赖与调用关系”中注明限制。

结构验证应使用任务指定命令，确认文件存在且恰好含有上述 11 个固定二级标题。本任务是纯文档分析，按计划不运行 Cargo，也未通过编译或测试执行推断额外运行时能力。
