# `pkg/ttl/cache/infoschema.rs`

## 文件定位

本文件属于 `astersql-ttl-cache` crate（见 `pkg/ttl/cache/Cargo.toml`），由 crate 入口 `pkg/ttl/cache/lib.rs` 以 `pub mod infoschema` 公开。它位于 TTL 元数据进入扫描、删除流程之前：把 InfoSchema 提供的逻辑表及分区元数据转换为以物理表 ID 为键的 `PhysicalTable` 映射，从而让后续逻辑不必反复扫描全部 TTL 表。

当前 Rust 仓库中，这个模块的 API 只在 `pkg/ttl/cache/infoschema_test.rs` 被直接使用；对生产 `.rs` 文件搜索 `NewInfoSchemaCache`、`InfoSchemaCache`、`InfoSchemaProvider` 和 `TTLTableEntry` 没有发现调用者。crate 的其他模块已被 session、ttlworker 等生产代码使用，但不能据此推断本文件已经接入 Rust TTL 调度主链。Go 对照实现则由 TTL worker 使用，因此这里应视为已实现、已测试但尚未完成生产接线的移植单元。

## 核心职责

1. `InfoSchemaProvider` 隔离具体 InfoSchema 实现，只要求提供 schema 元数据版本与 TTL 候选表列表。
2. `InfoSchemaCache` 组合 `baseCache` 的时间节拍和 schema 版本判定；时间节拍供外部决定何时尝试刷新，版本号决定一次刷新是否需要真正重建。
3. `Update` 过滤未启用 TTL、非 public 或没有 TTL 配置的表，将普通表展开为一个物理表，将分区表展开为每个分区一个物理表。
4. `newTable` 在同一物理 ID 的 `TableInfo` 未变化时复用旧 `PhysicalTable`，否则调用 `table.rs::NewPhysicalTable` 重新校验并构造。

本文件不负责定时调度、锁、后台任务、SQL 读取，也不负责执行 TTL 扫描或删除；它只维护进程内元数据快照。

## 主要符号

- `TTLTableEntry { schema, table, enabled }`：provider 返回的候选项。`schema` 是库名，`table: TableInfo` 包含表 ID、列、分区和 TTL 配置，`enabled` 对应 Go `TTLInfo.Enable`。三个字段均为公开字段，类型实现 `Clone` 与 `Debug`。
- `InfoSchemaProvider`：同步 trait。`schema_meta_version(&self) -> i64` 返回当前元数据版本，`ttl_tables(&self) -> Vec<TTLTableEntry>` 返回调用方拥有的候选列表。接口没有错误返回，也不借用列表，因此每次刷新可能分配或克隆完整列表。
- `InfoSchemaCache`：包含私有 `cache: baseCache`、私有 `schema_version: i64` 和公开 `Tables: HashMap<i64, PhysicalTable>`。键是 table ID 或 partition ID。
- `NewInfoSchemaCache(Duration) -> InfoSchemaCache`：创建版本为 `0`、表映射为空、尚未 `MarkUpdated` 的缓存。
- `ShouldUpdate(&self) -> bool`：直接委托 `baseCache::ShouldUpdate`；从未成功走到刷新末尾时为真，之后仅当经过时间严格大于 interval 才为真。
- `SetInterval(&mut self, Duration)`：调整底层刷新间隔，不会立即改变数据、schema 版本或最近更新时间。
- `SchemaVersion(&self) -> i64`：读取最近一次完成同步所记录的版本。
- `Update(&mut self, &dyn InfoSchemaProvider)`：版本变化时重建整个映射；无返回值。
- `newTable(&self, schema, info, partition) -> Result<PhysicalTable, String>`：私有构造/复用入口。空分区名表示普通表；非空分区名按 ASCII 不区分大小写查找分区。

文件没有常量、枚举、条件编译项或异步函数。

## 执行流程

典型调用流程如下：

1. 调用方以刷新间隔创建 `InfoSchemaCache`。`baseCache.update_time` 初始为 `None`，所以 `ShouldUpdate` 返回真。
2. 调用方取得 `&mut InfoSchemaCache` 并传入 provider 调用 `Update`。
3. `Update` 先读取 `schema_meta_version()`；若等于缓存的 `schema_version`，立即返回，不读取表列表，也不调用 `MarkUpdated`。
4. 版本不同时，以旧表数量作为容量提示新建 `HashMap`，再消费 `ttl_tables()` 返回的候选列表。
5. 候选项必须同时满足 `enabled == true`、`TableInfo.public == true`、`TableInfo.ttl.is_some()`。不满足者被忽略。
6. 无分区表以 `TableInfo.id` 为键调用 `newTable(schema, table, "")`；分区表逐个以分区名调用 `newTable`，并以 `PartitionDefinition.id` 为键。
7. `newTable` 先确定物理 ID。若旧映射同 ID 的条目具有完全相等的 `TableInfo`，返回其克隆；否则调用 `NewPhysicalTable`。后者验证 TTL 时间列、public 状态、主键/句柄以及分区存在性，并组装 `PhysicalTable`（见 `pkg/ttl/cache/table.rs:251`）。
8. 构造成功才插入新映射；同一物理 ID 重复出现时，后插入项覆盖先前项。构造错误被当前文件丢弃。
9. 遍历结束后，以新映射原子式替换字段值，记录本次 provider 版本，并调用 `baseCache::MarkUpdated` 记录当前 `Instant`。

版本未变化的快速路径不会刷新时间戳。因此一旦时间间隔到期，若 schema 继续不变，后续每次外部轮询都可能再次进入 `Update` 后立即返回；这与 Go 实现的早退顺序一致。

## 数据与状态

`Tables` 是某个已记录 schema 版本的完整快照，而不是增量映射。普通表的物理 ID 等于表 ID；分区表不保留表 ID 条目，只保留每个 partition ID 条目。测试 `test_info_schema_cache_syncs_partitioned_table` 验证从普通表版本切换到分区表版本后，旧 table ID 会消失，分区 ID `10`、`11` 各有一项。

刷新时先构建局部 `tables`，最后才替换 `self.Tables`，所以遍历期间旧快照仍完整。不过 `Update` 不提供事务或回滚语义：单项构造失败只会缺少该项，整体仍被认定为版本同步完成。

复用条件是 `cached.TableInfo == *info`。这会克隆整个 `PhysicalTable`，而不是共享指针；同时复用判断不比较传入的 schema 名或分区名。正常 provider 应保证物理 ID、`TableInfo`、schema 与分区定义一致，若违反这一假设，缓存可能沿用旧 `Schema`/`Partition` 字段。公开的 `Tables` 也允许调用方绕过这些不变量直接修改映射。

初始 `schema_version` 固定为 `0`。如果 provider 的真实初始版本也恰为 `0`，第一次 `Update` 会走快速路径，空映射不被构建且缓存仍保持“从未更新时间”；当前 Rust 测试使用版本 `1` 及以上，没有覆盖该边界。

## 依赖与调用关系

直接下游关系经 RustCodeGraph 核对如下：

- `NewInfoSchemaCache` → `base.rs::newBaseCache`，并实例化 `InfoSchemaCache`。
- `ShouldUpdate` → `base.rs::baseCache::ShouldUpdate`。
- `SetInterval` → `base.rs::baseCache::SetInterval`。
- `Update` → `InfoSchemaProvider::{schema_meta_version, ttl_tables}`、私有 `newTable`、`base.rs::baseCache::MarkUpdated`。
- `newTable` → `table.rs::NewPhysicalTable`；后者再调用 `NewBasePhysicalTable` 和键列解析逻辑。

标准库依赖只有 `HashMap` 与 `Duration`。crate manifest 将 `lib.rs` 设为库入口，并用 `[package.metadata.porting] go-package = "pkg/ttl/cache"` 标明 Go 来源。manifest 中列出的跨 crate 依赖目前都限定于 Windows target；本文件自身只引用同 crate 的 `base` 与 `table` 模块。

上游方面，`lib.rs` 公开本模块，`infoschema_test.rs` 创建 mock provider 并调用其 API；仓库搜索未发现生产 Rust 上游。这一点比 RustCodeGraph 针对常见短名称生成的宽泛“used by”提示更可靠，因为后者包含同名符号造成的跨模块噪声。

## 错误处理与边界

provider 接口没有 `Result`，因此 provider 无法向 `Update` 表达读取失败。`NewPhysicalTable` 的 `Result<_, String>` 则在 `Update` 中通过 `if let Ok(...)` 处理：错误项被静默跳过，没有日志、计数或对调用方可见的错误。即使有失败，`Update` 仍替换映射、推进 `schema_version` 并标记更新时间；在 schema 版本再次变化前，失败项不会因版本检查而重建。Go 版本同样逐项跳过，但会记录含 schema、table 和 partition 上下文的 warning，并且 `Update` 返回 `error` 类型。

显式过滤边界是：TTL disabled、表非 public、TTL 配置缺失。`NewPhysicalTable` 还会拒绝找不到 public TTL 时间列、无有效主键列、分区名缺失或不存在等情况。Rust 独立测试覆盖 disabled、非 public、普通表和分区展开；没有直接覆盖 TTL 配置缺失、构造错误被跳过、重复物理 ID、初始版本为零和 schema 名变化时的复用。

私有 `newTable` 查找不到所给分区名时用表 ID 作为复用查询 ID，随后 `NewPhysicalTable` 仍会返回“分区不存在”错误，因此不会把错误分区成功插入。正常 `Update` 只传入当前 `TableInfo.partitions` 中已存在的名称。

## 并发与资源生命周期

本文件没有锁、原子变量、channel、线程、异步任务、事务或外部句柄。修改操作要求 `&mut self`，Rust 借用规则防止同一实例在安全代码中并发更新；跨线程共享及同步策略必须由调用方提供。

provider 在一次 `Update` 调用期间被同步读取。`ttl_tables()` 返回拥有所有权的 `Vec`，条目随后被消费；`PhysicalTable` 持有克隆后的 `TableInfo`、列与索引数据，不借用 provider，因此 provider 生命周期不需要超过调用。

刷新会分配一个新 `HashMap`，成功后释放旧映射；复用条目也通过深层 `Clone` 进入新映射，减少重新验证但不等于零拷贝。`baseCache` 使用单调时钟 `Instant` 管理间隔；只有完成版本变化路径才更新时刻。

## 与 Go 版本的对应关系

`pkg/ttl/cache/infoschema.go` 是直接语义来源。两版均以 schema version 避免无变化重建，过滤 TTL 未启用/非 public 表，按 table ID 或 partition ID 建立完整新映射，并尝试复用旧物理表。

主要差异如下：

- Go `Update` 从 `session.Session` 取得真实 `infoschema.InfoSchema`；Rust 改为 `InfoSchemaProvider`，便于独立测试，但尚无生产 adapter/调用者。
- Go 候选列表来自 `ListTablesWithSpecialAttribute(TTLAttribute)`；Rust provider 直接返回扁平 `Vec<TTLTableEntry>`，候选枚举策略由实现者承担。
- Go 缓存保存 `*PhysicalTable` 并以 `ttlTable.TableInfo == tblInfo` 指针相等复用；Rust 保存值并以 `TableInfo` 结构相等复用，再克隆 `PhysicalTable`。因此 Rust 的复用范围更宽、成本模型也不同。
- Go 构造失败会写 warning；Rust 静默忽略。Go `Update` 的签名返回 `error`，但当前实现也没有返回非 nil 错误；Rust 直接返回 `()`。
- Go 使用 `ast.CIStr` 保存大小写信息，Rust 使用 `String`，仅在查找分区和 TTL 时间列时执行 ASCII 不区分大小写比较。
- Rust 增加了显式的 `ttl.is_none()` 过滤；Go 的等价条件是 `tblInfo.TTLInfo == nil`。

Go 测试 `pkg/ttl/cache/infoschema_test.go::TestInfoSchemaCache` 通过真实 mock store/session 与 SQL DDL 验证刷新节拍、普通表和分区表。Rust 测试用 `MockInfoSchema` 复现这些核心断言，并额外直接覆盖 disabled 和非 public 过滤，但没有覆盖真实 InfoSchema/session 集成。

## 扩展指南

- 接入生产主链时，应新增真实 InfoSchema 到 `InfoSchemaProvider` 的 adapter，并在 TTL worker 的生命周期中持有缓存；先确认 provider 列表与 Go `TTLAttribute` 枚举范围完全一致。同步增加独立测试文件中的 adapter/集成测试，不要把测试写进本源文件。
- 若要让刷新失败可观测，应修改 `Update` 的错误/报告契约，至少携带 schema、table ID、partition ID 与 `NewPhysicalTable` 错误。需要决定部分成功是否允许推进版本；这属于兼容行为变更，必须与 Go 语义和调用方重试策略一起评估。
- 修改过滤规则时，集中调整 `Update` 的候选条件，并同步 `pkg/ttl/cache/infoschema_test.rs` 与 Go 对照测试。必须保留普通表与分区表键空间不混用的不变量。
- 修改复用规则时，重点检查 schema 重命名、分区重命名、同 ID 元数据变化，以及 `PhysicalTable` 中由 `TableInfo` 派生的键列/时间列/索引是否同步刷新。若改用 `Arc` 等共享结构，需要额外评估跨线程同步和内存驻留。
- 若 provider 改为借用迭代器或异步接口，应重新定义条目生命周期、失败传播与更新的原子边界，避免在半途失败后发布不完整快照。
- 性能关注点包括完整候选列表分配、每次版本变化的全量遍历、`TableInfo`/`PhysicalTable` 克隆和 HashMap 重建；优化前应以真实 TTL 表及分区数量测量，不能用跳过校验的方式换取性能。

## 验证依据

- RustCodeGraph 索引状态：项目包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `pkg/ttl/cache/infoschema.rs` 共识别 16 个符号。
- RustCodeGraph 文件/符号读取：`pkg/ttl/cache/infoschema.rs:1-132`、`pkg/ttl/cache/base.rs:1-60`、`pkg/ttl/cache/table.rs:185-272`、`pkg/ttl/cache/infoschema_test.rs:1-174`。
- RustCodeGraph 调用边：`NewInfoSchemaCache -> newBaseCache`；`ShouldUpdate -> baseCache::ShouldUpdate`；`Update -> schema_meta_version/ttl_tables/newTable/MarkUpdated`；`newTable -> NewPhysicalTable`。
- crate 与模块证据：`pkg/ttl/cache/Cargo.toml`、`pkg/ttl/cache/lib.rs`、根 `Cargo.toml` 的 `facade_ttl_cache` workspace 依赖。
- Go 对照：`pkg/ttl/cache/infoschema.go`；Go 回归语义：`pkg/ttl/cache/infoschema_test.go::TestInfoSchemaCache`。
- Rust 回归语义：`pkg/ttl/cache/infoschema_test.rs` 的四个测试，覆盖首次刷新、普通表同步、分区展开、disabled/non-public 过滤。
- 上游接线核验：对目标四个公开符号进行全仓 Rust 搜索，排除本文件及其测试后无匹配；只据此判断“当前未发现生产接线”，不推断未来设计。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构检查，并人工复核本文没有把 Go 接线描述成 Rust 已支持能力。
