# `pkg/util/tiflash/tiflash_replica_read.rs`

## 文件定位

只提供策略数据和转换，不执行节点发现、Region 分配或 RPC。

## 核心职责

- 用 `ReplicaRead(pub isize)` 保留 Go `type ReplicaRead int` 的可区分策略值，并让零值默认为 `AllReplicas`。
- 声明三种策略：`AllReplicas` 使用所有可用 TiFlash 节点；`ClosestAdaptive` 优先同 zone、数据不足时可跨 zone；`ClosestReplicas` 限制为同 zone，仅容忍少量远程 Region 读。
- 在策略整数值与 `vardef` 字符串之间做双向映射，保留 Go 对未知值回退到 `AllReplicas` / `all_replicas` 的行为。
- 定义 `ClosestReplicas` 每个 TiFlash 节点允许的跨 zone 远程 Region 读上限 `3`。本文件只提供策略数据和转换，不执行节点发现、Region 分配或 RPC。

## 主要符号

- `pub struct ReplicaRead(pub isize)`：可拷贝、可比较的新类型。`Default` 由唯一字段的 `isize` 导出，因此得到 `ReplicaRead(0)`，与 `AllReplicas` 相等。公开 tuple 字段也允许表示三个已知值之外的整数。
- `AllReplicas` / `ClosestAdaptive` / `ClosestReplicas`：显式取值分别为 `0` / `1` / `2`，对齐 Go `iota` 顺序。
- `ReplicaRead::IsAllReplicas(&self) -> bool`：仅在值等于 `AllReplicas` 时返回 `true`。
- `ReplicaRead::IsClosestReplicas(&self) -> bool`：仅在值等于 `ClosestReplicas` 时返回 `true`；`ClosestAdaptive` 不算作该谓词。
- `GetTiFlashReplicaRead(ReplicaRead) -> &'static str`：把已知策略转成 `vardef::{AllReplicaStr, ClosestAdaptiveStr, ClosestReplicasStr}`，未知整数返回 `AllReplicaStr`。
- `GetTiFlashReplicaReadByStr(&str) -> ReplicaRead`：精确、区分大小写地解析三个 `vardef` 值，其他输入返回 `AllReplicas`。
- `MaxRemoteReadCountPerNodeForClosestReplicas: isize = 3`：`closest_replicas` 每节点的远程读容忍阈值。

## 执行流程

1. 上游以常量、`ReplicaRead::default()` 或 `GetTiFlashReplicaReadByStr` 得到策略值。默认路径产生数值 `0`，即 `AllReplicas`。
2. 如果需要分支判断，调用方可使用 `IsAllReplicas` 或 `IsClosestReplicas`；文件没有为 `ClosestAdaptive` 单独提供谓词，调用方需直接比较常量。
3. `GetTiFlashReplicaRead` 按三个常量依次匹配并返回静态 `vardef` 字符串；不在已知集合中的整数走默认分支。
4. `GetTiFlashReplicaReadByStr` 按三个规范字符串精确匹配；空串、大小写变体或未知串均走默认分支。
5. 真正的 TiFlash store 过滤和远程读上限检查属于下游执行层，不在本文件内发生。例如 Go `pkg/store/copr/batch_coprocessor.go` 使用这组策略；当前 Rust `pkg/store/copr/batch_coprocessor.rs` 则定义了自己的 `ReplicaReadPolicy` 和阈值，不是对本文件 API 的直接调用。

## 数据与状态

`ReplicaRead` 只含一个 `isize`，没有堆分配、内部可变性或隐藏状态。三个规范值的数值编码是对外可见的移植兼容契约：`0` 必须继续表示全副本，否则 `Default` 将不再与 Go 零值一致。

字符串不在本 crate 复制存储，而是直接返回 `vardef` 中的 `&'static str`：`all_replicas`、`closest_adaptive`、`closest_replicas`。这保证系统变量定义与工具 crate 不会产生两套字面量。

## 依赖与调用关系

- 下游依赖：`GetTiFlashReplicaRead*` 只读取 `astersql-sessionctx-vardef` 里的三个静态字符串；比较方法和常量不调用其他函数。
- crate 边界：[`lib.rs`](lib.rs) 以 `crate::sessionctx::vardef` 把 Cargo 依赖引入 `include!` 模块，并全量再导出本文件的公开符号。
- 已验证的 Rust 上游：`astersql-distsql-context` 在 Cargo 中以 `tiflash-dependency` 依赖本 crate；`DistSQLContext` 保存、复制并默认构造 `tiflash::ReplicaRead`。`pkg/distsql/context/context_test.rs` 和 `migration_aster_unit_test.rs` 使用 `ClosestAdaptive` 验证分离上下文时策略保留。
- 函数级调用现状：全库 Rust 精确搜索没有找到同目录测试之外对 `GetTiFlashReplicaRead`、`GetTiFlashReplicaReadByStr`、两个谓词或远程读阈值的直接调用。因此它们当前是已移植并有单元测试的公开 API，不应推断为已完整接入 Rust store 选择主链。

## 错误处理与边界

本文件不返回 `Result` 也不主动报错。两个转换函数对非规范输入采用宽容回退：未知 `ReplicaRead(isize)` 序列化为 `all_replicas`，未知字符串解析为 `AllReplicas`。这是 Go `switch default` 的明确兼容行为，不是输入校验。

字符串解析不做 trim、大小写归一或别名处理；例如 `"ALL_REPLICAS"` 会回退到 `AllReplicas`，结果与正常值恰好相同，但不代表该别名被接受。系统变量层有自己的枚举值校验：`pkg/sessionctx/variable/sysvar_test.rs::TestTiDBTiFlashReplicaRead` 验证 `random` 在那一层返回错误，不应与本文件的宽容解析混为一谈。

## 并发与资源生命周期

所有类型和函数都是纯值操作：不获取锁，不创建线程或任务，不使用通道，不进行 I/O，也不持有借用的运行时资源。`ReplicaRead` 实现 `Copy`，因而上下文间传递是整数值拷贝；转换返回的字符串拥有 `'static` 生命周期，无需分配或释放。

由于本文件不执行真正的副本选择，节点列表的并发更新、RPC 取消、跨 zone 读计数等生命周期都在下游实现中，不能从本文件推导其保证。

## 与 Go 版本的对应关系

Rust 文件逐项对应 [`tiflash_replica_read.go`](tiflash_replica_read.go)：Go 的 `type ReplicaRead int` 对应 Rust tuple newtype；三个 `iota` 常量对应显式的 `0/1/2`；两个值接收者方法对应 Rust `&self` 方法；两个 `switch` 对应带默认分支的 `match`；远程读阈值均为 `3`。

差异主要是语言表达：Rust 为了保留 Go 风格 API 名称使用 `allow(non_snake_case/non_upper_case_globals)`；策略常量不依赖声明顺序自增；字符串转换返回静态引用而非 Go `string` 值。

当前 Rust 搜索证据只能确认类型进入 DistSQL 上下文，不能声称上述两条 Go 调用链已按相同 API 完整接通。

## 扩展指南

- 新增策略时，必须明确一个稳定数值，并同步修改 `GetTiFlashReplicaRead`、`GetTiFlashReplicaReadByStr`、`sessionctx/vardef` 字符串及同目录独立测试。不应改变现有 `0/1/2` 编码或零值语义。
- 如果要改成拒绝未知值，需把返回类型升级为可表达错误的形式，并先评估 Go 默认回退兼容性及系统变量层的重复校验。
- 修改远程读阈值时，应同步 Go 常量、`migration_aster_unit_test.rs` 和真正执行 store 过滤的下游。特别要检查当前 Rust `pkg/store/copr/batch_coprocessor.rs::MAX_REMOTE_READ_COUNT_PER_NODE_FOR_CLOSEST_REPLICAS`，它是独立常量，存在漂移风险。
- 若将本 crate 的策略直接接入 Rust Coprocessor，需一并处理当地 `ReplicaReadPolicy` 的转换或去重，并扩展 `pkg/store/copr/batch_coprocessor_test.rs` 的同 zone/跨 zone/远程读边界用例；不要只改本文件便假定调度行为已变更。
- Rust 测试逵守仓库规则，继续放在独立的 `migration_aster_unit_test.rs` 或相应下游 `*_test.rs` 中，不内嵌进生产源文件。
