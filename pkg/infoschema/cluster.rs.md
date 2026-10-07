# `pkg/infoschema/cluster.rs`

## 文件定位

本文件属于 `astersql-infoschema` crate，是集群内存表的轻量路由与行整形模块。crate 根 `pkg/infoschema/lib.rs` 以 `pub mod cluster` 声明模块，并重新导出本文件的全部公开常量、类型与函数；`pkg/infoschema/Cargo.toml` 则把 crate 根设为 `lib.rs`。它不负责执行跨节点 RPC，也不创建 information schema 表，而是提供下列更靠近边界的决策：哪些表属于集群表、请求应面向全部 TiDB 还是 DDL Owner，以及如何把当前实例标识加到结果行前面。

截至当前代码，Rust 生产文件中没有这些函数的调用者；精确搜索只发现 crate 根导出和 Rust 测试使用。完整应用主链仍可从 Go 对照实现看到：`pkg/planner/core/operator/physicalop/physical_table_scan.go` 用 `IsClusterTableByName` 标记集群表，`pkg/executor/table_reader.go` 用 `GetClusterTableCopDestination` 选择请求目标，多个 `pkg/executor/*` reader 用 `AppendHostInfoToRows` 补实例列。因此，本文件是已实现且有测试的移植边界，但尚不能据此声称 Rust SQL 主链已经完成接线。

## 核心职责

- 用 13 个 `ClusterTable*` 字符串常量统一表示由所有 TiDB 节点提供的 `CLUSTER_*` 表名；依据是 `ClusterTableSlowLog` 至 `ClusterTableTiDBPlanCache`。
- 用 `ALL_TIDB_TABLES` 保存“本地内存表名 → 集群表名”关系，用 `DDL_OWNER_TABLES` 保存只需访问 DDL Owner 的表。目前后者包含 `TIFLASH_REPLICA` 和 `TIKV_STORAGE_CLASS_TRANSITIONS`。
- 由 `GetClusterTableCopDestination` 给表名分类；DDL Owner 名单命中时返回 `DDLOwner`，否则一律返回 `AllTiDB`。
- 由 `IsClusterTableByName` 在两个系统库中识别集群表；它只接受已经规范化为小写的库名和表名。
- 由 `GetInstanceAddr` 按普通模式、SEM 权限和 IP 地址族生成对外实例标识，再由 `AppendHostInfoToRows` 将该标识作为每行第一个 `Datum`。

本文件没有实现 Go `cluster.go` 的初始化期表列注册逻辑：Go 的 `init` 会给集群表复制本地表列并前置 `INSTANCE` 列，Rust 文件只覆盖名称/路由判定和结果行改写。

## 主要符号

- `ClusterTableSlowLog` 等 13 个公开常量：所有值均为大写 `CLUSTER_*` 名称，对应 `ALL_TIDB_TABLES` 的目标侧。新增全节点集群表时，常量和映射应同步维护。
- `ALL_TIDB_TABLES: &[(&str, &str)]`：私有静态切片，包含源表名和集群表名。当前函数只使用其目标名做识别；源名主要保存 Go 映射语义，尚未被 Rust 路由函数读取。
- `DDL_OWNER_TABLES: &[(&str, &str)]`：私有静态切片。`GetClusterTableCopDestination` 同时比较二元组两侧，`IsClusterTableByName` 只比较目标侧。
- `ClusterTableCopDestination::{AllTiDB, DDLOwner}`：公开、可复制的路由枚举。它只表达目标类别，不携带节点列表、网络连接或重试策略。
- `GetClusterTableCopDestination(table_name: &str) -> ClusterTableCopDestination`：对 DDL Owner 映射进行 ASCII 不区分大小写匹配。未知表、普通本地表以及所有全节点集群表都会落入默认值 `AllTiDB`，所以它不是“表名是否合法”的校验器。
- `IsClusterTableByName(db_name: &str, table_name: &str) -> bool`：先要求 `db_name` 精确等于小写 `information_schema` 或 `performance_schema`，再把映射中的大写目标名转成小写后与输入 `table_name` 精确比较。输入本身不会被规范化。
- `Datum`：供本模块测试和行改写使用的简化值枚举，含 `Null`、`String`、`Integer`、`Unsigned`、`Bytes`。它不是完整 SQL 类型系统中的 Datum。
- `ServerInfo { id, ip, status_port }`：构造展示地址所需的最小服务身份快照。
- `ClusterSessionContext`：注入 `server_info`、SEM 开关和受限表读取权限的公开 trait，使地址逻辑不直接依赖 domain、privilege 或 sessionctx crate。
- `GetInstanceAddr(ctx: &dyn ClusterSessionContext) -> Result<String, String>`：先取 `ServerInfo`；SEM 开启且无受限表权限时返回 `id`，否则返回 `IP:status_port`，IPv6 使用 `[IP]:port`。
- `AppendHostInfoToRows(ctx, rows) -> Result<Vec<Vec<Datum>>, String>`：只获取一次地址，为每行新建容量为原长度加一的向量，前置地址后移动原单元格。

文件没有条件编译项、宏、impl 块或模块级可变状态。

## 执行流程

路由判定从 `GetClusterTableCopDestination` 开始：遍历 `DDL_OWNER_TABLES`，用 `eq_ignore_ascii_case` 同时检查源名和集群名；任一命中立即选择 `DDLOwner`，遍历结束仍未命中则选择 `AllTiDB`。因为全节点映射没有参与检查，调用者必须先在其他位置确认表的合法性。

名称识别从 `IsClusterTableByName` 开始：先执行系统库白名单检查，库名大小写或库名不匹配时立即返回 `false`；随后把 `ALL_TIDB_TABLES` 与 `DDL_OWNER_TABLES` 串接遍历，将每个目标名临时转为 ASCII 小写，与调用者传入的表名做精确比较。`pkg/infoschema/cluster_test.rs` 明确断言大写库名和大写表名不会被接受，这与 Go 调用点传入 `.L` 小写名的契约一致。

行改写从 `AppendHostInfoToRows` 调用 `GetInstanceAddr` 开始。后者先通过 trait 获取服务信息；若 SEM 已开启且当前会话不能读受限表，则用不暴露网络拓扑的 server ID。其余情况按 IP 地址族生成地址：含冒号的 IP 被视为 IPv6，先去除两端已有方括号再输出 `[ip]:port`；IPv4/主机字符串直接输出 `ip:port`。地址成功后，每行都会得到独立的新向量和一个克隆的地址字符串，原有列次序保持不变。

## 数据与状态

所有表名清单都是不可变的编译期切片，没有懒初始化、缓存或运行期注册。`IsClusterTableByName` 每次调用都会遍历最多 15 个映射项，并对候选目标名分配小写 `String`；数据量固定且很小，但若清单显著扩大，可考虑保存预规范化名称以避免重复分配。

`ServerInfo` 是按调用时返回的值快照；本文件不保存它，也不刷新服务发现状态。`Datum` 和输入行由值传递，`AppendHostInfoToRows` 消费整个 `Vec<Vec<Datum>>`，逐行移动原 Datum 到新向量。与 Go 原地替换外层切片元素的实现相比，Rust 对调用者表现为所有权转移，没有共享可变别名。

关键不变量是：成功返回的每行长度恰好比输入多一，索引 0 必为同一个实例地址的 `Datum::String`，其余元素保持原顺序。空行会变成只含实例地址的一列；空行集合仍会先查询地址，因此 `server_info` 失败时不会返回空成功结果。

## 依赖与调用关系

本文件只使用 Rust 标准库能力，没有直接引用 `pkg/infoschema/Cargo.toml` 中的其他 workspace crate。trait 抽象刻意把 Go 实现中的 `infosync`、`sessionctx`、`privilege` 和 SEM 依赖移到调用方适配层；当前仓库尚未发现生产适配器。

RustCodeGraph 给出的直接下游边为：`AppendHostInfoToRows -> GetInstanceAddr`，以及 `GetInstanceAddr -> ClusterSessionContext::{server_info, sem_enabled, can_read_restricted_tables}`；名称判定函数分别引用 `DDL_OWNER_TABLES`，或同时引用两个映射切片。RustCodeGraph 的 callers 查询未给出 Rust 上游边，文本搜索也仅发现 `pkg/infoschema/lib.rs` 的公开再导出和测试调用。

对应的 Go 上游链提供设计依据而非 Rust 已接线证据：planner 的 physical table scan 调用 `IsClusterTableByName`，executor table reader 调用 `GetClusterTableCopDestination`，infoschema/statement-summary readers 调用 `AppendHostInfoToRows`。将来接入 Rust 主链时，应在等价边界调用公开 API，而不是把映射和 SEM 判定复制到 planner/executor。

## 错误处理与边界

唯一显式错误源是 `ClusterSessionContext::server_info` 的 `Result<ServerInfo, String>`。`GetInstanceAddr` 用 `?` 原样传播字符串错误；`AppendHostInfoToRows` 在任何行改写前获取一次地址，因此失败是整批原子失败，不会产生部分已加列的返回值。错误类型没有结构化类别或附加上下文，生产接线若需要可观察性，应在 trait 实现或上层边界增加上下文，同时保持这里的失败传播语义。

需要特别留意的输入边界：`GetClusterTableCopDestination` 对未知名称仍返回 `AllTiDB`；`IsClusterTableByName` 要求小写输入；库名仅限两个固定值；IPv6 判断采用“字符串含冒号”的启发式且只剥离首尾方括号；空 IP、端口 0 和空 server ID 在本层不会被拒绝。`Datum` 也仅覆盖有限类型，不能直接替代完整执行器行类型。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络连接。两个映射切片是只读静态数据，可被并发安全地读取。`ClusterSessionContext` trait 本身没有 `Send`/`Sync` 约束；能否跨线程共享由具体实现和上层调用方式决定。

地址信息只在一次函数调用内存活：`GetInstanceAddr` 取得 `ServerInfo` 后生成拥有所有权的 `String`；`AppendHostInfoToRows` 在整批处理中持有该字符串，并为每行克隆一次。输入行在调用时被消费，失败发生在消费后的地址查询阶段，但函数不会暴露或部分返还输入数据，调用者需要在调用前决定是否保留副本。

## 与 Go 版本的对应关系

`pkg/infoschema/cluster.go` 是直接语义来源。13 个 `CLUSTER_*` 常量、全 TiDB 映射、DDL Owner 映射、路由枚举、名称识别、地址获取和行前置逻辑均有对应物。`pkg/infoschema/cluster_test.rs` 验证了 Go 的小写输入契约和 DDL Owner 不区分大小写路由；`pkg/infoschema/go_merge_45_test.rs` 还覆盖 storage class transitions 的 DDL Owner 路由。

存在四项重要差异。第一，Go `init` 会建立小写映射并注册带 `INSTANCE` 列的集群表 schema，Rust 本文件没有该初始化逻辑。第二，Go 从全局 infosync 和会话权限管理器读取信息，Rust 通过 `ClusterSessionContext` 注入，并把权限管理器缺失等细节折叠成布尔值。第三，Go 使用完整 `types.Datum` 并原地替换外层 rows，Rust 使用本地简化 `Datum` 并消费后重建结果。第四，Go 的 `IsClusterTableByName` 使用预建的小写 map 并在测试模式断言调用者已规范化；Rust 每次把候选转小写，以测试断言体现相同调用契约。

Go 测试 `pkg/infoschema/test/clustertablestest/cluster_tables_test.go` 经 SQL 验证实例列；Rust 独立测试 `pkg/infoschema/test/clustertablestest/cluster_tables_test.rs` 直接覆盖 IPv4、IPv6、SEM 脱敏和行前置。Rust 测试注释也明确说明完整 mockstore、gRPC、PD HTTP 与 SQL testkit 栈尚未移植，因此直接 helper 测试不能等同于端到端集群表验证。

## 扩展指南

新增“所有 TiDB 节点”集群表时，应增加或复用公开 `ClusterTable*` 常量，并向 `ALL_TIDB_TABLES` 添加源/目标对；同时核对 Go 的 `memTableToAllTiDBClusterTables` 和表 schema 注册。新增 DDL Owner 表时修改 `DDL_OWNER_TABLES`，并为源名、目标名及大小写变体补路由测试。若引入第三种目标节点类型，需要扩展 `ClusterTableCopDestination`，同时修改 executor 分发逻辑，不能依赖当前“非 DDL Owner 即 AllTiDB”的默认分支。

改变名称规范化策略前，要同步检查 planner 是否仍传入小写 `.L` 名称，并更新 `pkg/infoschema/cluster_test.rs`、`pkg/infoschema/test/clustertablestest/tables_test.rs` 和独立 `cluster_tables_test.rs`。改变实例展示或权限规则时，应优先修改 `GetInstanceAddr` 和 trait，而不是在 `AppendHostInfoToRows` 中分叉；至少覆盖 server-info 错误、SEM 有/无权限、权限管理器等价边界、IPv4、已带括号/未带括号 IPv6、空 rows 和多 rows。

生产接线时需要实现真实 `ClusterSessionContext` 适配器，并在 planner/executor 的 Rust 等价位置复用这些 API。兼容风险主要是大小写契约、未知表默认路由和 SEM 地址泄露；性能风险主要是每次名称判定的小写分配及每行地址克隆。测试逻辑应继续放在独立 `cluster_test.rs` 或 `test/clustertablestest/*.rs`，不要嵌入生产文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/infoschema/cluster.rs`；`node --file pkg/infoschema/cluster.rs` 读取了完整 179 行和 35 个符号；对 `GetClusterTableCopDestination`、`IsClusterTableByName`、`GetInstanceAddr`、`AppendHostInfoToRows` 执行了 `query`、`callers`、`callees`。图确认了私有映射引用、trait 方法调用及 `AppendHostInfoToRows -> GetInstanceAddr`；callers 无 Rust 生产边，随后用 `rg` 补证。
- Rust 源与装配：`pkg/infoschema/cluster.rs`、`pkg/infoschema/lib.rs`、`pkg/infoschema/Cargo.toml`。其中 Cargo 的常规依赖没有被本文件直接使用，较完整但被 `cfg(any())` 禁用的旧移植依赖也不能视为本模块已接入生产主链的证据。
- Rust 测试：`pkg/infoschema/cluster_test.rs`、`pkg/infoschema/go_merge_45_test.rs`、`pkg/infoschema/test/clustertablestest/cluster_tables_test.rs`、`pkg/infoschema/test/clustertablestest/tables_test.rs`。直接验证包括小写契约、DDL Owner 路由、storage class transitions、SEM、IPv6 和实例列前置。
- Go 对照与调用点：`pkg/infoschema/cluster.go`、`pkg/infoschema/tables_test.go`、`pkg/infoschema/test/clustertablestest/cluster_tables_test.go`，以及 `pkg/planner/core/operator/physicalop/physical_table_scan.go`、`pkg/executor/table_reader.go`、`pkg/executor/infoschema_reader.go`、`pkg/executor/stmtsummary.go`。
- 本任务是纯文档分析，按计划没有运行 Cargo 或代码测试。最终结构检查要求本文恰好包含任务指定的 11 个二级标题；人工复核同时确认没有把 Go 主链接线误称为 Rust 已支持能力。
