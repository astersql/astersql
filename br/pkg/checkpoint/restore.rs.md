# `br/pkg/checkpoint/restore.rs`

## 文件定位

本文件是 `astersql-br-pkg-checkpoint` crate 面向“快照恢复”场景的适配层。crate 根 `br/pkg/checkpoint/lib.rs` 以 `pub mod restore` 装入本模块，并通过 `pub use restore::*` 展平导出其公开类型与函数；`br/pkg/checkpoint/Cargo.toml` 则表明该 crate 对应 Go 包 `br/pkg/checkpoint`，直接依赖 `serde`、`serde_json` 和带 `v4` feature 的 `uuid`。

它位于通用检查点引擎 `checkpoint.rs` 与具体恢复业务之间：本文件选择分组键/值类型、定义快照恢复元数据、把完成项包装成 `CheckpointMessage`，并把快照恢复专用的 JSON marshaler 交给 `SnapshotMetaManager`。存储路径、线程循环、重试和落盘均不在本文件实现，而由 `manager.rs`、`checkpoint.rs` 及 storage 模块承担。

RustCodeGraph 将本文件识别为含 22 个符号的 Rust 文件，直接使用者仅为 `br/pkg/checkpoint/checkpoint_test.rs` 与 `br/pkg/checkpoint/restore_test.rs`；关键公开入口在图中的生产 callers 为空。虽然 `br/pkg/restore/log_client/Cargo.toml` 与 `br/pkg/task/Cargo.toml` 依赖本 crate，当前 Rust 源码搜索没有发现它们调用 `StartCheckpointRunnerForRestore` 或 `AppendRangesForRestore`。因此这里是可用且有测试的 crate API，但不能据此宣称 Rust 的完整恢复生产链已经接通。

## 核心职责

1. 用 `RestoreKeyType = i64` 和 `RestoreValueType` 定义恢复进度的持久化形状：每条记录由下游 table ID 分组，值表示一个 range key 或一个文件名。
2. 用 `CheckpointItem`、`NewCheckpointRangeKeyItem`、`NewCheckpointFileItem` 提供追加记录的场景化输入，并由 `AppendRangesForRestore` 转换为通用 Runner 消息。
3. 用 `valueMarshalerForRestore` 将整个 `RangeGroup<RestoreKeyType, RestoreValueType>` 直接序列化为 JSON；快照恢复不像 `log_restore.rs` 那样做额外折叠压缩。
4. 用 `StartCheckpointRunnerForRestore` 和测试专用的 `StartCheckpointRestoreRunnerForTest` 配置并启动通用 `CheckpointRunner`。
5. 定义可跨 Rust/Go JSON 交换的 `PreallocIDs` 与 `CheckpointMetadataForSnapshotRestore`，其中任务 UUID 通过私有 `uuid_serde` 模块固定为 Go `google/uuid.UUID` 使用的连字符文本形式。

本文件不负责判断某条恢复任务是否应该跳过、创建元数据、加载旧检查点、计算 checksum 或停止 Runner；这些动作分别属于上层恢复流程、`SnapshotMetaManager` 和 `CheckpointRunner`。

## 主要符号

- `RestoreKeyType = i64`：通用 `RangeGroup`/`CheckpointMessage` 的分组键，对应 Go 的 `int64` 下游表 ID。
- `RestoreValueType { RangeKey, Name }`：可序列化的单条完成记录。JSON 键分别是 `range-key`、`name`，空字符串通过 `skip_serializing_if` 省略；字段公开以供加载回调消费。
- `CheckpointItem { tableID, rangeKey, name }`：模块内部可构造、但字段不向 crate 外公开的追加载体。两个公开构造函数分别设置一个业务字段并清空另一个。
- `NewCheckpointRangeKeyItem(tableID, rangeKey)`：为整库/全量备份恢复构造 range-key 项。
- `NewCheckpointFileItem(tableID, fileName)`：为 raw/txn/compacted SST 恢复构造文件名项。
- `valueMarshalerForRestore(&RangeGroup) -> Result<Vec<u8>>`：私有序列化函数，调用 `serde_json::to_vec`，错误经 crate 的 `Result` 转换向上传播。
- `StartCheckpointRestoreRunnerForTest(ctx, tick, retryDuration, manager)`：把 flush 与 checksum 周期同时改为 `tick`，把重试周期改为调用者提供值，再调用 `SnapshotMetaManager::StartCheckpointRunner`。
- `StartCheckpointRunnerForRestore(ctx, manager)`：使用 `DefaultTickDurationConfig()` 启动正式 Runner。
- `AppendRangesForRestore(ctx, runner, item)`：验证并转换完成项，然后调用 `CheckpointRunner::Append`。
- `PreallocIDs { Start, ReusableBorder, End, Hash }`：任务恢复时复用 ID 区间的持久化快照，`Hash` 为固定 32 字节；本文件只承载数据，不解释或分配区间。
- `CheckpointMetadataForSnapshotRestore`：保存上游集群 ID、恢复起止时间戳、PITR 日志恢复时间戳、调度器配置、配置哈希、可选预分配 ID 与恢复 UUID。
- `uuid_serde::{serialize, deserialize}`：私有 serde 适配；序列化调用 `Uuid::hyphenated`，反序列化调用 `Uuid::parse_str` 并把错误转换为 serde 错误。

## 执行流程

正式启动路径如下：

1. 上层先选择一个实现 `SnapshotMetaManager` 的管理器，并在元数据已创建或恢复检查完成后调用 `StartCheckpointRunnerForRestore`。
2. 入口构造 `DefaultTickDurationConfig()`，连同 `valueMarshalerForRestore` 传给 `manager.StartCheckpointRunner`。
3. `manager.rs` 的表后端取得其专属 runner session，构造 `tableCheckpointStorage`；外部存储后端构造 `externalCheckpointStorage` 并携带可选 cipher。两者随后调用 `newCheckpointRunner` 和 `startCheckpointMainLoop`。表后端的 runner session 被 `take()`，所以同一管理器不能无条件重复启动。
4. 某个 SST/range 成功导入后，上层用两个构造函数之一创建 `CheckpointItem`，再调用 `AppendRangesForRestore`。
5. `AppendRangesForRestore` 优先选择非空 `rangeKey`，否则选择非空 `name`，构成只含一个 `RestoreValueType` 的 `CheckpointMessage { GroupKey: tableID, Group: vec![value] }`，然后投递到 Runner 的无界 append channel。
6. `checkpoint.rs` 的后台循环按 table ID 合并内存 `RangeGroup`，定期调用本文件的 marshaler，并交给所选存储后端落盘；checksum 由 Runner 的独立通道与接口处理，不经过本文件。
7. 上层必须调用 `CheckpointRunner::WaitForFinish(ctx, flush)`：它只发送一次 done 信号、等待工作线程退出，并关闭底层 checkpoint storage；`flush=true` 用于要求收尾刷盘。

Go 的实际生产路径印证了上述意图：`br/pkg/restore/snap_client/client.go` 加载/校验元数据与旧数据后启动 Runner；`br/pkg/restore/log_client/client.go` 为 compacted SST 恢复启动同类 Runner；`br/pkg/restore/restorer.go` 在导入成功后按文件名或 range key 追加完成项。当前 Rust 搜索未发现这些入口的等价生产调用，因此这段 Go 链是语义对照，不是 Rust 已接线的证据。

## 数据与状态

持久化 data 的逻辑单位是 `RangeGroup<i64, RestoreValueType>`：`GroupKey` 是下游 table ID，`Group` 是该表已完成单元的列表。`AppendRangesForRestore` 每次只追加一个值，但通用 Runner 会在内存中按键聚合后批量序列化。`RestoreValueType` 的两个字符串代表两类恢复粒度；正常构造和追加路径只写其中一个。

`CheckpointMetadataForSnapshotRestore` 是任务级状态而非逐 range 数据：

- `UpstreamClusterID` 防止把另一集群的检查点用于当前任务；
- `RestoreStartTS`、`RestoredTS`、`LogRestoredTS` 描述恢复时间边界；
- `SchedulersConfig: Option<ClusterConfig>` 保存可恢复的调度配置；
- `Hash: Vec<u8>` 由上层用于命令/配置一致性检查；
- `PreallocIDs: Option<PreallocIDs>` 保存可复用的 ID 分配区间与 32 字节完整性哈希；
- `RestoreUUID: Uuid` 标识恢复任务，JSON 中必须是标准连字符字符串。

所有元数据字段都允许 serde 缺省，便于读取缺字段的旧 JSON。其代价是缺失值会变成数值零、空 vector、`None` 或 nil UUID；有效性与跨任务匹配必须由上层校验，不能把成功反序列化等同于元数据有效。

## 依赖与调用关系

上游 Rust 关系：`lib.rs` 公开再导出本模块；`checkpoint_test.rs` 调用测试 Runner、range 构造器和追加入口；`restore_test.rs` 专门验证 UUID JSON；`parity_test.rs` 验证错误文案和成功落盘。RustCodeGraph 对 `StartCheckpointRunnerForRestore`、`AppendRangesForRestore` 的生产 callers/callees 没有解析出边，本地文本搜索也未找到生产调用。

下游关系：

- `crate::checkpoint::{CheckpointMessage, CheckpointRunner, RangeGroup}` 提供消息契约、异步线程 Runner 与分组容器；
- `crate::manager::{DefaultTickDurationConfig, SnapshotMetaManager}` 提供默认周期和表/对象存储后端抽象；
- `crate::stubs::{ClusterConfig, Context, Error, Result}` 提供移植期边界类型；
- `serde_json` 决定 data 与 metadata JSON 表示；
- `uuid` 负责恢复任务标识解析与规范文本输出。

Go 对照中的真实业务调用者包括 `br/pkg/restore/snap_client/client.go`、`br/pkg/restore/log_client/client.go` 与 `br/pkg/restore/restorer.go`。Go 的 `br/pkg/task/restore.go` 还消费 `CheckpointMetadataForSnapshotRestore.PreallocIDs`。Rust 侧 `br/pkg/restore/internal/prealloc_table_id/alloc.rs` 有自己的检查点转换类型与逻辑，名称相似不代表已经通过本模块贯通；扩展前应先确认类型桥接位置。

## 错误处理与边界

`AppendRangesForRestore` 的显式失败边界是两个字符串都为空，此时返回 `either rangekey or name should be used in checkpoint append`，与 Go 文案一致。若两个字段都非空，当前实现按 `if/else if` 选择 `rangeKey` 而不会报错；因此源码注释所说的“互斥”主要由两个构造函数保证，并不是该函数对任意 `CheckpointItem` 强制验证的完整不变量。crate 外无法直接写私有字段，但模块内代码和测试可以构造异常项。

追加还可能因 context 已取消、Runner 后台错误、Runner 已关闭或 append channel 关闭而失败，这些错误来自 `CheckpointRunner::Append` 并原样返回。其 append channel 是无界通道，因此高生产速率不会在发送处形成容量背压，而会增加内存中待合并状态；上层应控制并发与及时收尾。

`valueMarshalerForRestore` 将 serde JSON 错误向上传播；通用 Runner 会把 marshaler/落盘失败纳入失败批次和重试流程。UUID 反序列化只接受 `Uuid::parse_str` 可解析的文本，非法字符串直接导致整个元数据反序列化失败。`Default` 产生 nil UUID，不代表一个可投入生产的新任务 UUID。

本文件没有验证 table ID 正负、range/file 名称格式、元数据 hash 长度、`Start <= ReusableBorder <= End` 或时间戳顺序；这些都属于调用者/领域层边界。修改时不要在这里静默加入与 Go 不一致的拒绝规则。

## 并发与资源生命周期

本文件自身不持有锁、线程、session 或通道；它返回的 `CheckpointRunner` 才是并发资源所有者。`checkpoint.rs::CheckpointRunner` 内含 append/checksum/done/error 通道、受 `Mutex`/`RwLock` 保护的共享分组与错误状态，以及后台 `JoinHandle` 列表。`AppendRangesForRestore` 只克隆字符串并同步投递消息，不等待真实落盘完成。

`SnapshotMetaManager: Send + Sync` 允许从并发恢复流程共享管理器。启动后，表后端把 runner session 移交给 checkpoint storage；Go 源码明确注明 session 由 Runner 拥有并随 Runner 关闭，Rust `manager.rs` 通过 `take()` 与 `WaitForFinish`/storage `close()` 实现同类所有权边界。外部存储后端没有 session，其 `Close` 是空操作，但 Runner 仍需停止后台线程。

测试路径通过自定义短 tick 驱动 flush/checksum/retry；正式路径采用默认 flush 30 秒、checksum 5 秒、retry 3 秒的配置（定义于 `checkpoint.rs`）。取消 context 或后台错误会阻止新的追加，但调用者仍应执行收尾。`WaitForFinish` 的 done 信号由 `done_sent` 保证只发送一次。

## 与 Go 版本的对应关系

核心 API 与 `br/pkg/checkpoint/restore.go` 基本逐项对应：类型别名、两个 value 字段、两个 item 构造函数、直接 JSON marshaler、测试/正式 Runner 启动函数、追加转换、`PreallocIDs` 和快照恢复元数据均保留了 Go 命名与职责。Rust 通过 serde rename 显式固定 Go JSON tag；`RangeKey`/`Name` 的空值省略与 Go `omitempty` 对齐。

主要语言差异包括：

- Go 构造函数返回 `*CheckpointItem`，Rust 返回按值的 `CheckpointItem`，追加时借用；
- Go 接口参数是 `SnapshotMetaManagerT`，Rust 使用 `&dyn SnapshotMetaManager`；
- Go 的可空指针映射为 Rust `Option`；Go `[]byte` 映射为 `Vec<u8>`，`[32]byte` 映射为 `[u8; 32]`；
- Go `uuid.UUID` 依靠 `encoding.TextMarshaler` 输出文本，Rust 用 `uuid_serde` 明确复刻该 JSON 表示；
- Go 源码对正式 Runner 明确记录 session 所有权，Rust 的所有权行为落在 `manager.rs` 与 `checkpoint.rs`，本入口注释本身未完整复述；
- Rust 为所有 metadata 字段加了 serde `default`，比 Go 的常规 `encoding/json` 解码更明确地支持缺字段输入。

最重要的迁移状态差异是：Go 生产代码已有启动、加载、追加和停止链；Rust 当前证据只显示本 crate 内测试直接调用这些 API，其他 Rust crate 虽声明依赖但未找到对应生产调用。后续文档或实现不得把 Go 调用者直接当作 Rust 调用者。

## 扩展指南

新增一种恢复完成粒度时，应先判断它是否能继续表达为 `RestoreValueType` 的互斥字段。若新增字段，需要同时修改该结构的 serde 名称、`CheckpointItem`/构造入口、`AppendRangesForRestore` 的选择规则、Go `restore.go`、读取方以及独立 Rust 测试；还要考虑旧 reader 忽略新字段、新 reader 读取旧 JSON 时的默认值，以及同一项出现多个字段时的优先级。格式变化会影响已落盘检查点兼容性，不能只修改 marshaler。

新增或加强校验时，最可能修改 `AppendRangesForRestore`。须先用 Go 行为决定“双字段非空”、空白字符串、非法 table ID 等情况是拒绝还是兼容，并在 `br/pkg/checkpoint/parity_test.rs` 添加边界回归；不要把 Rust 规则单方面收紧。

修改 Runner 周期或生命周期时，应改 `StartCheckpointRunnerForRestore`/`StartCheckpointRestoreRunnerForTest` 与 `manager.rs`，并同步 `checkpoint_test.rs` 的 flush、retry、`WaitForFinish` 场景。表后端单次移交 session、无界 append channel 的内存风险、退出时是否强制 flush 都需要兼容性和性能评估。

扩展任务元数据时，应同步 `CheckpointMetadataForSnapshotRestore`、Go 同名结构、manager 的 save/load 路径和 `restore_test.rs` 的 JSON 往返测试。UUID 表示变化尤其会破坏跨语言读取。`PreallocIDs` 的区间业务规则属于预分配模块，若改变含义还需同步 `br/pkg/restore/internal/prealloc_table_id` 的 Go/Rust 实现与独立测试。

若要接通 Rust 生产链，入口应放在实际 Rust snapshot/log restore 客户端完成元数据加载或创建之后，并在 SST 成功导入后追加；同时明确谁在成功、取消和错误路径调用 `WaitForFinish`。这属于上层接线任务，不应通过在本文件中伪造调用或简化 storage/runner 逻辑来完成。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/checkpoint/restore.rs` 确认本文件有 22 个符号并被两个 Rust 测试文件使用；`node --file ...` 读取全文件；`query` 核对 `StartCheckpointRunnerForRestore`、`AppendRangesForRestore`、`NewCheckpointRangeKeyItem`、`valueMarshalerForRestore`；对关键 Rust 函数执行 `callers`/`callees`，结果为空；`node checkpoint.rs::CheckpointRunner` 与按行读取确认通道、锁、线程和收尾语义。
- 源与 crate 边界：`br/pkg/checkpoint/restore.rs`、`br/pkg/checkpoint/lib.rs`、`br/pkg/checkpoint/Cargo.toml`、`br/pkg/checkpoint/checkpoint.rs`、`br/pkg/checkpoint/manager.rs`。
- Go 对照与生产调用：`br/pkg/checkpoint/restore.go`、`br/pkg/restore/snap_client/client.go`、`br/pkg/restore/log_client/client.go`、`br/pkg/restore/restorer.go`、`br/pkg/task/restore.go`。
- 独立测试：`br/pkg/checkpoint/restore_test.rs` 验证 UUID 文本 JSON 往返；`br/pkg/checkpoint/checkpoint_test.rs` 验证 metadata、table ID 分组、两种后端、checksum、重试和无重试路径；`br/pkg/checkpoint/parity_test.rs` 验证空 item 错误与成功 append/flush/load；Go 对照为 `br/pkg/checkpoint/checkpoint_test.go`。
- 生产接线核验：仓库文本搜索 Rust/Go 的公开符号；Rust 仅在本 crate 测试、manager 类型引用及依赖声明中命中，Go 则命中上述恢复客户端和 restorer。该结论仅反映当前仓库静态证据，未运行 Cargo 或动态端到端恢复。
