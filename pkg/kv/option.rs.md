# `pkg/kv/option.rs`

## 文件定位

`pkg/kv/option.rs` 是 `astersql-kv` crate 的事务选项协议与请求来源定义文件。crate 根在 `pkg/kv/Cargo.toml` 中指定为 `pkg/kv/lib.rs`；后者通过 `#[path = "option.rs"] mod option_impl` 装入本文件并 `pub use option_impl::*`，因此这里的公开项最终直接暴露为 `astersql_kv::*`，而不是要求调用方经过 `option_impl`。

本文件位于 SQL/session 层与存储驱动层之间：上层用整数选项 ID、`ReplicaReadType`、`TransactionSchemaChecker` 和请求来源值表达事务要求，下层 `Transaction` 实现解释这些异构值。它本身不执行 KV I/O，也不拥有事务；它定义跨实现必须一致的协议、轻量数据类型和 `TxnSource` 位图操作。直接依据是 `pkg/kv/option.rs`、`pkg/kv/kv.rs` 中 `Transaction::SetOption/GetOption`，以及 `pkg/store/driver/kv_adapter.rs` 的选项存储与消费逻辑。

## 核心职责

1. **固定事务选项 ABI。** `BinlogInfo = 1` 到 `PrewriteEncounterLockPolicy = 44` 是连续的 `i32` 键。值本身不携带类型；类型约定由生产者和具体 `Transaction` 实现共同维护。`pkg/store/driver/kv_adapter.rs` 以 `HashMap<i32, Box<dyn Any>>` 保存值，并在读取 `SizeLimits`、`SchemaChecker` 等键时下转为约定类型。
2. **提供跨层参数类型。** `TxnSizeLimits` 表达单条 mutation 与整事务的字节上限；`TransactionSchemaChecker` 包装可在线程间共享的提交时间戳校验闭包；`ReplicaReadType` 表达七种 TiDB 副本读策略。
3. **在可取消 `Context` 上附加请求来源。** `WithInternalSourceType`、`WithInternalSourceAndTaskType` 构造内部请求标记，`GetInternalSourceType` 读取来源类型；`InternalTxn*` 常量把 DDL、统计、BR、Lightning、TTL 等内部工作归类。
4. **编码和解码事务来源位图。** `SetCDCWriteSource` 使用低 8 位，`SetLossyDDLReorgSource` 使用随后 8 位，`LightningPhysicalImportTxnSource` 使用第 17 位；读取与“是否设置”辅助函数供调用方解释该位图。

## 主要符号

- `TransactionSchemaChecker(Arc<dyn Fn(u64) -> Result<(), SharedError> + Send + Sync>)`：提交时间戳到 schema 有效性结果的共享回调。`pkg/session/runtime/schema_validation.rs::checker` 构造它，`pkg/store/driver/kv_adapter.rs::ClientTransaction::Commit` 在底层提交前把它接入 schema lease checker，并尽量还原原始共享错误。
- `BinlogInfo` … `PrewriteEncounterLockPolicy`：44 个公开选项 ID。重要的“键—值”实例包括 `SchemaChecker`—`TransactionSchemaChecker`、`ReplicaRead`—`ReplicaReadType`、`SizeLimits`—`TxnSizeLimits`、`RequestSourceInternal`—`bool`、`RequestSourceType`/`ExplicitRequestSourceType`/`ResourceGroupName`—`String`。其余键的实际值类型必须以具体生产者和消费者为准，不能仅从整数常量推断。
- `TxnSizeLimits { Entry, Total }`：两个 `u64` 字节上限。驱动的 `Mutator::Set` 在写入前分别比较 `key + value` 大小和内存缓冲区累计大小（`pkg/store/driver/kv_adapter.rs`）。
- `ReplicaReadType`：`#[repr(u8)]` 的七值枚举，判别值固定为 0..6。`IsFollowerRead` 对除纯 Leader 外的所有策略返回真；`IsClosestRead` 只对精确的 `ReplicaReadClosest` 返回真。`pkg/store/driver/options/options.rs::GetTiKVReplicaReadType` 将七种上层策略压缩为五种存储策略，其中 Mixed、Closest、ClosestAdaptive 都映射为 Mixed。
- `RequestSourceKeyType` / `RequestSourceKey`：与 Go context key 形态对应的公开标识。在当前 Rust `Context` 实现中，请求来源存放于专用 `Option<RequestSource>` 字段，读写路径并不以该 key 做动态查找，因此该静态值主要承担 API/迁移兼容角色。
- `RequestSource`：包含 `RequestSourceInternal: bool`、`RequestSourceType: String`、`ExplicitRequestSourceType: String`。三个字段都参与派生的克隆、调试、相等比较和默认值。
- `WithInternalSourceType` / `WithInternalSourceAndTaskType` / `GetInternalSourceType`：消费并返回新的 `Context`；前两个总把 `RequestSourceInternal` 置真，后者在来源缺失时返回空字符串。
- `InternalTxn*` 常量：请求来源的稳定字符串词汇。其中 Bootstrap、Meta、CacheTable、BindInfo、SysVar、Telemetry、Privilege 等别名合并到 `"others"`，这是降低来源维度的有意行为；大小写敏感值如 `"Trace"`、`"TTL"`、`"DistTask"` 不应随意规范化。
- `SetCDCWriteSource` / `GetCDCWriteSource` / `IsCDCWriteSourceSet`：低 8 位字段的设置、读取和判定 API；内部实现为 `getCDCWriteSource`、`isCDCWriteSourceSet`。
- `SetLossyDDLReorgSource` / `GetLossyDDLReorgSource` / `IsLossyDDLReorgSourceSet`：第 9–16 位字段的设置、读取和判定 API；`LossyDDLColumnReorgSource = 1` 是当前列重组值。
- `LightningPhysicalImportTxnSource = 1 << 16`：第 17 位的物理导入标志，不属于前两个 8 位提取掩码。

## 执行流程

事务选项主流程如下：

1. 上层选择本文件的整数键并构造与该键匹配的具体值，例如 session 层通过 `schema_validation::checker` 创建 `TransactionSchemaChecker`。
2. 上层调用 `Transaction::SetOption(i32, Option<Box<dyn Any>>)`；内部事务辅助函数 `pkg/kv/txn.rs::setRequestSourceForInnerTxn` 会把 `Context` 中非空的来源拆成 `RequestSourceInternal`、`RequestSourceType`，并按需增加 `ExplicitRequestSourceType`。
3. 具体驱动保存或即时翻译该选项。`ClientTransaction` 将值放入整数键控的异构 map；写入路径读取 `SizeLimits`，提交路径读取 `SchemaChecker`，其他驱动可按相同 ID 实现不同接线。
4. 执行阶段按约定类型下转并应用行为。若键值类型不匹配，当前展示的 `downcast_ref` 路径通常表现为“未取到该选项”而非类型错误，因此类型一致性必须由调用方、实现和测试共同保证。

请求来源流程是：调用方把 `Context` 按值传入 `WithInternalSourceType` 或 `WithInternalSourceAndTaskType`；函数写入完整 `RequestSource` 后返回新 context；`RunInNewTxn` 路径读取该结构并设置事务选项。空来源在 `setRequestSourceForInnerTxn` 中不会下发，而会记录缺失来源警告（`pkg/kv/txn.rs`）。

`TxnSource` 设置流程使用按位 OR，而非清空后覆盖：CDC 值直接 OR 到低位；有损 DDL 值左移 8 位后 OR 入。读取函数再用掩码截取各字段。因此调用方若对同一字段重复写不同值，结果是位合并，不是“最后一次写入获胜”。

## 数据与状态

- 本文件没有可变全局状态。所有选项 ID、来源字符串和位图布局都是编译期常量；`RequestSourceKey` 是零大小单例。
- `TransactionSchemaChecker` 的闭包由 `Arc` 共享，要求 `Send + Sync`，可被事务/存储适配层跨所有权边界持有。闭包捕获资源的寿命随最后一个 `Arc` 克隆释放。
- `Context` 的真实状态位于 `pkg/kv/lib.rs`：一个 `tokio_util::sync::CancellationToken` 和一个 `Option<RequestSource>`。本文件的构造函数消费旧 `Context`、更新其来源字段并返回，未改变 cancellation token 的共享取消语义。
- `TxnSizeLimits` 的默认值为 `Entry = 0, Total = 0`，因为派生 `Default`。这不等于“无限制”；只有当 `SizeLimits` 选项实际存在时驱动才应用比较，因此是否安装该选项比默认结构体本身更关键。
- `TxnSource` 布局依据 `pkg/kv/option.rs` 与 Go `pkg/kv/option.go`：位 0–7 为 CDC，位 8–15 为有损 DDL，位 16 为 Lightning，余下高位预留。设置函数原地修改调用者提供的 `u64`。

## 依赖与调用关系

本文件的直接 Rust 依赖仅来自 crate 根：`Context`、统一 `Error` 和 `errors::New`。`std` 只用于 `Arc` 和闭包 trait；没有直接外部 crate import。其所属 `astersql-kv` crate 的 feature 为默认空集，可选 `nextgen` 同时打开 `kerneltype/nextgen` 与 `keyspace/nextgen`，但本文件没有条件编译分支（`pkg/kv/Cargo.toml`）。

上游代表性调用关系：

- `pkg/session/runtime/schema_validation.rs::checker` → `TransactionSchemaChecker`；`pkg/store/driver/kv_adapter.rs::ClientTransaction::Commit` → 读取 `SchemaChecker` 并调用其闭包。
- `pkg/session/runtime/bootstrap_wait.rs`、`modify_column_backfill.rs`、`control.rs` 等 → `WithInternalSourceType`；`pkg/kv/txn.rs::setRequestSourceForInnerTxn` → `Context::RequestSource` → 三个请求来源选项 ID。
- `pkg/session/runtime/relational_scan.rs`、`dispatch.rs` 等 → `ReplicaReadType`；`pkg/store/driver/options/options.rs::GetTiKVReplicaReadType` → 下层副本策略。
- `pkg/store/driver/kv_adapter.rs::Mutator::Set` → `SizeLimits` / `TxnSizeLimits`。

RustCodeGraph 的文件节点报告本文件被 25 个文件使用，并列出 `pkg/kv/kv.rs`、`pkg/kv/mpp.rs`、`pkg/meta/reader.rs`、`pkg/executor/statement_ru_result.rs` 等代表项。对精确函数执行 `callers/callees` 时在本次 30 秒查询窗口内没有返回边，因此上述具体边均由索引源码节点和限定范围 `rg` 交叉核对，不把超时视为“无调用者”。

## 错误处理与边界

- `SetCDCWriteSource` 的实际上界判断是 `value > cdcWriteSourceBits`，即仅 0..=8 成功；错误消息却写 `[1, 15]`，常量掩码又能表达 0..=255。这一不一致来自 Go 实现并被 Rust 注释明确保留，文档不能把消息中的 15 当作真实可接受上界。`0` 合法且表示未设置。
- `SetLossyDDLReorgSource` 接受 0..=255，256 及以上返回统一 `Error`；`0` 不新增任何位。两种 setter 都先检查再修改，所以越界错误不会部分写入。
- 两种 setter 均采用 OR，不能清除既有位，也不会验证目标字段原先是否为空。需要“替换字段”时，调用方必须先显式清位；直接重复调用可能形成新的组合值。
- `isLossyDDLReorgSourceSet` 的实现判断 `txn_source >> 8 != 0`，没有再与 `0xff` 掩码。因此任何第 9 位及以上的标志都会使其返回真，包括独立的 `LightningPhysicalImportTxnSource`。而 `getLossyDDLReorgSource` 会正确掩码，只返回位 8–15；两者在仅设置更高位时可能出现“已设置为真但读取值为 0”。这是与 Go 当前实现一致的边界，修改前必须先决定是否允许行为变化。
- `GetInternalSourceType` 对无来源返回空串，不报错；Rust 使用类型安全的专用字段。Go 版本在 key 存在但动态值类型错误时会因类型断言 panic，这一失败模式不适用于当前 Rust 表达。
- 整数选项协议没有编译期键值配对约束。新增或重排 ID 会影响所有 `Transaction` 实现；复用旧 ID 或传错 `Box<dyn Any>` 都可能静默改变行为。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务或 I/O 资源。`RequestSource` 和数值类型都是拥有数据的普通值，克隆后互不共享可变字符串。`Context` 的按值更新避免原地共享修改，但其中 cancellation token 的克隆仍共享取消状态（见 `pkg/kv/lib.rs`）。

唯一显式并发契约是 `TransactionSchemaChecker`：闭包必须 `Send + Sync`，并通过 `Arc` 共享。存储提交路径可能在闭包执行期间保存错误以便把底层映射错误恢复成原始 `SharedError`；实现者不应在闭包中依赖非线程安全的可变状态或阻塞资源。位图 setter 接受独占的 `&mut u64`，Rust 借用规则保证单次调用期间没有数据竞争，但跨线程同步仍由持有者负责。

## 与 Go 版本的对应关系

主要语义逐项对应 `pkg/kv/option.go`：

- Rust 的 1..44 选项常量保持 Go `iota + 1` 顺序；`TxnSizeLimits` 字段、七种 `ReplicaReadType` 判别值和两个判断方法同构。
- Go 的 `RequestSourceKey`、`RequestSource` 和两个 `WithInternal*` 直接别名到 client-go `util`；Rust 在本 crate 内定义相应类型，并把来源存入自有 `Context` 字段。外部表现对齐，但内部承载机制不同。
- 来源字符串与别名保持一致，包括 `InternalTxnMViewMaintenance = "mview_maintain"`；Rust 独立测试 `pkg/kv/option_test.rs::go_merge_4_materialized_view_internal_source` 固定该合并点。
- 位图布局、OR 写入、提取公式和错误文案与 Go 相同。Rust额外公开 `GetCDCWriteSource`、`IsCDCWriteSourceSet`、`GetLossyDDLReorgSource`、`IsLossyDDLReorgSourceSet` 包装，而 Go 同文件的对应 getter/checker 为包内小写函数。
- `pkg/kv/option_test.rs` 复刻 Go `pkg/kv/option_test.go` 的核心表格：CDC 的 1、0、16，以及有损 DDL 的空位图、保留 CDC 低位、0、256。Rust测试对错误类别使用消息子串断言。

当前迁移状态不是桩：请求来源已被 session/executor/statistics 等 Rust 路径调用，schema checker 和大小限制已被存储适配器消费，副本策略已有明确映射。不过 44 个选项并非都能仅凭本文件证明已在每个 Rust `Transaction` 实现中完整接线；具体支持度应以对应实现的 `SetOption/GetOption` 为准。

## 扩展指南

新增事务选项时，应在现有最大 ID 后追加稳定值，不插入或重编号；同时定义清楚值的 Rust 类型，并更新所有相关 `Transaction`/`Snapshot` 实现的设置、读取、删除语义。至少在独立测试文件中覆盖正确类型、缺失值、清除值和错误类型值；不要把测试内嵌回 `option.rs`。若由 `ClientTransaction` 消费，应同步检查 `pkg/store/driver/kv_adapter.rs` 及其同目录测试。

新增请求来源类别时，应核对 Go `pkg/kv/option.go`、大小写和是否应折叠到 `InternalTxnOthers`，并在调用入口用 `WithInternalSourceType` 或任务版本接线。需要验证传播时，测试应覆盖 `Context` 构造、`GetInternalSourceType`，以及 `RunInNewTxn` 设置的整数选项，而不只断言常量文本。

扩展 `TxnSource` 时，应先画定不重叠的位段，同时审计 setter 的清位/覆盖语义和所有“是否设置”函数是否严格掩码。尤其要为“仅设置其他高位”的情况添加回归测试，以免重复 `isLossyDDLReorgSourceSet` 当前的宽判定。若计划修正 CDC 上界或错误消息，必须作为 Go/Rust 兼容行为变更处理，并同步两侧 `option_test`。

扩展副本读策略时，除本枚举和方法外，还必须更新 `pkg/store/driver/options/options.rs::GetTiKVReplicaReadType` 以及 session/distsql 的穷尽匹配；`#[repr(u8)]` 值属于可观察 ABI，不应复用或移动。性能风险主要来自选择更宽松的副本路由、为每次事务增加动态分配的 `Box<dyn Any>`，或在 schema checker 闭包中加入阻塞工作。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/kv/option.rs` 命中目标；`node --file pkg/kv/option.rs --offset 1 --limit 500` 读取完整 339 行并报告 25 个使用文件；`query` 核对了 `SetCDCWriteSource`、`WithInternalSourceType`、`IsLossyDDLReorgSourceSet` 的 Rust/Go 符号候选。精确 `callers/callees` 查询在 30 秒内无输出，未据此作负面结论。
- crate 与装配：`pkg/kv/Cargo.toml`、`pkg/kv/lib.rs`；事务协议：`pkg/kv/kv.rs`；请求来源传播：`pkg/kv/txn.rs` 与 `pkg/kv/lib.rs::context`。
- 真实消费者：`pkg/store/driver/kv_adapter.rs`（`SizeLimits`、`SchemaChecker`、异构选项 map）、`pkg/session/runtime/schema_validation.rs`（checker 构造）、`pkg/store/driver/options/options.rs`（副本策略映射），并通过限定范围 `rg` 核对 session、executor、statistics 等调用点。
- Go 对照：`pkg/kv/option.go`；独立测试：`pkg/kv/option_test.rs` 与 `pkg/kv/option_test.go`。另参考 `pkg/kv/txn_test.rs` 中请求来源使用，但未将测试文件当作生产实现。
- 本任务为纯文档分析，按计划不运行 Cargo。交付结构以任务指定命令验证，人工复核重点为 11 个固定章节、真实符号/路径、当前边界而非理想化设计，以及扩展测试仍位于独立测试文件。
