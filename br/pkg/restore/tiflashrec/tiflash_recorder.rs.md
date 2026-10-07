# `br/pkg/restore/tiflashrec/tiflash_recorder.rs`

## 文件定位

本文件是 Cargo workspace 成员 `astersql-br-pkg-restore-tiflashrec` 的核心实现；`br/pkg/restore/tiflashrec/Cargo.toml` 将 `lib.rs` 声明为库入口，`lib.rs` 再以 `pub mod tiflash_recorder` 挂载本文件并用 `pub use tiflash_recorder::*` 扁平导出其 API。该 crate 没有声明第三方或仓库内 Cargo 依赖，模型与 InfoSchema 边界均由本文件内的最小 Rust 类型表达。

它对照 Go `br/pkg/restore/tiflashrec/tiflash_recorder.go`，保存恢复期间暂时移除的 TiFlash 表副本配置，并把记录转成恢复副本所需的 `ALTER TABLE ... SET TIFLASH REPLICA ...`。Go 注释给出的设计背景是：PiTR 期间不能让表持续向 TiFlash 复制，因此先记录并移除副本信息，表 ID 改写后继续按 ID 跟踪，最后再生成 DDL 恢复配置。

当前 Rust 接线状态比 Go 窄。RustCodeGraph 能确认本文件内部调用边和独立测试，但 Cargo/源码搜索未找到其他 crate 对 `astersql-br-pkg-restore-tiflashrec` 的依赖，也未找到目标类型的 Rust 生产调用者。`br/pkg/task/restore.rs` 定义了独立的 `TiFlashReplicaRecorder` trait，却没有为本文件的 `TiFlashRecorder` 提供实现；`br/pkg/restore/log_client/client.rs::LoadOrCreateCheckpointMetadataForLogRestore` 直接接收 checkpoint 模型的 `HashMap`，而不是本录制器。因此本文件目前应视为已移植、可独立验证但尚未证明接入 Rust 完整恢复主链的库实现。

## 核心职责

`TiFlashRecorder` 负责三件事：以 table ID 为键保存 `TiFlashReplicaInfo`；在恢复产生新 table ID 时迁移记录；从最新 InfoSchema 解析库表名并生成普通恢复 DDL或“先清零、再恢复”的成对 DDL。使用 ID 而非名称，可以承受恢复期间的 RENAME 和 ID 重写；生成 SQL 时才解析名称，则使输出使用恢复侧的当前库表标识。

本文件还封装了生成 DDL 所需的最小模型：`TiFlashReplicaInfo` 表达副本数和 location labels，`CIStr` 保留原始名称和小写形式，`TableMeta` 只保存表名，`InfoSchema` trait 以 `TableByID` 一次返回表与 schema 名。它不执行 DDL、不访问 PD/TiKV、不持久化 checkpoint，也不负责从备份表元数据中移除副本配置；这些属于上层恢复编排。

## 主要符号

- `TiFlashReplicaInfo { Count, LocationLabels }`：单表 TiFlash 配置。`Count` 为 `u64`；标签按输入顺序保留，生成 SQL 时逐项加引号和转义。
- `CIStr { O, L }` 与 `CIStr::new`：TiDB 大小写不敏感标识的最小镜像。构造时 `O` 保存原始文本，`L` 用 `to_lowercase()` 生成；本文件生成 SQL 只读取 `O`。
- `TableMeta { Name }`：DDL 生成所需的最小表元数据。
- `InfoSchema: Send + Sync`：按 table ID 查询 `(TableMeta, CIStr)` 的抽象边界；返回 `None` 同时涵盖表或 schema 无法解析。
- `TiFlashRecorder { items }`：核心容器，私有 `HashMap<i64, TiFlashReplicaInfo>` 维护 table ID 到副本配置的映射。
- `New()`：构造空 map。它是关联函数 `TiFlashRecorder::New`，返回拥有所有权的值。
- `Load(items)`：整体替换内部 map，不与旧记录合并。
- `GetItems()`：返回内部 map 的共享只读引用，不允许调用方通过该引用改写状态。
- `AddTable(tableID, replica)` / `DelTable(tableID)`：分别插入或覆盖记录、删除记录；删除不存在的 ID 静默成功。
- `Iterate(f)`：以共享引用遍历全部记录；顺序由 `HashMap` 决定，不保证稳定。
- `Rewrite(oldID, newID)`：table ID 相同则无操作；否则删除旧键，并在旧键存在时把值插入新键。
- `GenerateAlterTableDDLs(info)`：每个能由 InfoSchema 解析的记录生成一条目标副本 DDL。
- `GenerateResetAlterTableDDLs(info)`：每个能解析的记录依序生成 `REPLICA 0` 和恢复原配置两条 DDL。
- `EncloseDBAndTable(db, table)`：私有标识符格式化器；库表名分别用反引号包裹，名称内部的反引号加倍。
- `alterTableSpecOf(replica, reset)`：私有 DDL spec 格式化器；reset 分支固定返回 `SET TIFLASH REPLICA 0`，普通分支追加副本数与可选 labels。

## 执行流程

1. 上层以 `TiFlashRecorder::New` 创建空录制器，或通过 `Load` 从已有状态整体恢复记录。
2. 遇到带 TiFlash 副本配置的表时调用 `AddTable`。同一 table ID 再次录制会覆盖旧配置；上层移除或过滤表时可调用 `DelTable`。
3. 恢复表 ID 发生变化时调用 `Rewrite(oldID, newID)`。它先从旧键取走值，再写入新键，所以记录跟随 ID 迁移。
4. 恢复侧 InfoSchema 可用后，调用普通或 reset DDL 生成方法。两者都通过 `Iterate` 枚举记录，并调用 `InfoSchema::TableByID` 取得恢复侧库表名；查不到时跳过该项。
5. `GenerateAlterTableDDLs` 调用 `alterTableSpecOf(..., false)`，再以 `EncloseDBAndTable` 格式化限定表名，产出一条 `ALTER TABLE`。
6. `GenerateResetAlterTableDDLs` 对同一张表先调用 `alterTableSpecOf(..., true)` 产出清零语句，再调用普通分支产出恢复语句。单表两条语句在返回向量中保持相邻且清零在前，但不同表之间的顺序不稳定。
7. 本文件只返回 `Vec<String>`；实际执行、重试、事务边界和失败恢复均由调用方负责。当前仓库尚未验证到 Rust 生产调用方执行这些字符串。

## 数据与状态

唯一可变业务状态是 `items: HashMap<i64, TiFlashReplicaInfo>`。`AddTable` 是 upsert；`DelTable` 是幂等删除；`Load` 是全量替换；`GetItems` 暴露的只是共享视图。`Iterate` 的回调取得 `&TiFlashReplicaInfo`，不能通过回调修改 map 或记录。Rust 独立测试以排序后的集合比对输出，明确不依赖 map 遍历顺序。

`Rewrite` 有两个重要覆盖规则：旧 ID 不存在时不产生新记录；新 ID 已存在时，旧记录会覆盖新 ID 原有记录。因为函数先 `remove(oldID)` 再 `insert(newID, old)`，迁移后旧键必定消失。调用方若可能产生多对一 ID 映射，必须先决定覆盖是否符合恢复语义。

`GenerateResetAlterTableDDLs` 为最多两倍记录数预分配容量；普通生成方法按记录数预分配。实际长度会因 InfoSchema 缺项而减少。label 顺序与 `LocationLabels` 一致；空 labels 不输出 `LOCATION LABELS`。库表名使用 `CIStr.O`，所以保留原始大小写，而 `CIStr.L` 在本文件没有参与比较或输出。

## 依赖与调用关系

本文件直接依赖只有标准库 `std::collections::HashMap`。RustCodeGraph 验证的内部调用边为：`GenerateAlterTableDDLs` 和 `GenerateResetAlterTableDDLs` 都调用 `Iterate`、`InfoSchema::TableByID`、`alterTableSpecOf` 与 `EncloseDBAndTable`；`TiFlashRecorder::New` 构造空 map。`lib.rs` 是模块入口，并以两个独立测试模块挂载 `tiflash_recorder_test.rs` 与 `parity_test.rs`。

Rust 侧相邻模块尚未形成闭环：`br/pkg/task/restore.rs::PreCheckTableTiFlashReplica` 通过其本地 `TiFlashReplicaRecorder` trait 调用 `AddTable`，但目标类型没有实现该 trait；`br/pkg/restore/log_client/client.rs::LoadOrCreateCheckpointMetadataForLogRestore` 把调用方给出的 checkpoint `TiFlashReplicaInfo` map 写入元数据，类型来自 checkpoint crate，与本文件模型不是同一类型。`br/pkg/task/stream.rs::PiTRTaskInfo::hasTiFlashItemsInCheckpoint` 还明确是恒 `false` 的桩。

Go 生产主链则有可验证调用者：`br/pkg/task/restore.go::PreCheckTableTiFlashReplica` 在移除表元数据中的副本配置前调用 `AddTable`；Go `br/pkg/restore/log_client/client.go::LoadOrCreateCheckpointMetadataForLogRestore` 读取 `GetItems` 写入 checkpoint；同一路径的 Go 文件还由 restore/task 代码调用改写和 DDL 生成逻辑。上述 Go 边说明本移植文件的设计位置，不能作为 Rust 已接线的证据。

## 错误处理与边界

公开录制操作均不返回错误。重复添加覆盖旧值，删除缺失键静默，重写缺失旧 ID 静默，InfoSchema 缺表也静默跳过。与 Go 不同，Rust `InfoSchema::TableByID` 已把 table 和 schema 解析合并为单个 `Option`，因此实现无法区分“表不存在”和“schema 不存在”，也没有 Go 路径的 warning 日志。

`alterTableSpecOf` 的签名是 `Result<String, String>`，两个当前分支实际上都只返回 `Ok`；两种 DDL 生成方法仍以 `let Ok(...) else { return; }` 吞掉潜在错误。若未来将格式化改为真正可失败，当前策略会静默丢弃该表的输出：reset 路径若清零 spec 失败则一条都不生成；若恢复 spec 失败则可能已经留下清零语句。这种部分输出风险在扩展时必须处理。

标识符转义由 `EncloseDBAndTable` 将反引号替换为双反引号。字符串标签先把反斜杠替换为双反斜杠，再把单引号替换为两个单引号，并用单引号包裹；`tiflash_recorder_test.rs::TestGenSql` 覆盖了包含注入样式单引号和反斜杠引号的标签。此逻辑是对 Go AST Restore 标志组合的手写镜像，而不是调用 Rust SQL parser，因此新增特殊字符规则时必须重新与 Go formatter 对照。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄。所有状态修改都要求 `&mut self`，读取与 DDL 生成只要求 `&self`；Rust 借用规则防止同一实例在可变操作期间被同时访问。`InfoSchema` 要求 `Send + Sync`，允许实现跨线程共享，但生成方法本身是同步串行遍历，并未并行查询 schema。

典型生命周期是：创建或装载 → 录制/删除 → ID 重写 → 基于当前 InfoSchema 生成 DDL → 由外部执行。录制器不会在生成后自动清空，也不会记住已生成或已执行状态，所以重复调用会重复产出相同集合。它也不保存 InfoSchema 引用；查询借用只在单次生成调用内存在。

时间复杂度方面，增删改写平均为 `HashMap` 的 O(1)；遍历和 DDL 生成相对记录数为 O(n)，字符串分配还与名称及 labels 总长度相关。reset 路径最多生成 2n 条字符串。内存除 map 外主要是返回的 SQL 向量和每条语句的拥有型 `String`。

## 与 Go 版本的对应关系

Rust 的 map 结构、New/Load/Get/Add/Delete/Iterate/Rewrite 语义、按 table ID 延迟解析名称、缺表跳过、普通 DDL 和 reset 成对 DDL，都直接对应 `tiflash_recorder.go`。`tiflash_recorder_test.rs::{TestRecorder, TestGenSql, TestGenResetSql}` 镜像 Go 同名测试，覆盖增删、链式 ID 改写、无/单/多/特殊标签，以及 reset 先清零再恢复；`parity_test.rs::go_rust_public_contract_matches` 额外覆盖同 ID Rewrite、Load 全量替换、GetItems 与缺表跳过。

两者仍有可观察差异。Go `New` 返回指针，Rust返回值；Go `GetItems` 返回可修改 map，Rust 返回共享引用。Go 使用真实 `model.TiFlashReplicaInfo`、`infoschema.InfoSchema` 和 parser AST，Rust 使用本地最小类型并手写 SQL。Go 分别执行 `TableByID` 与 `SchemaByTable` 并记录 warning，Rust合并为一次 `Option` 查询且静默跳过。Go `AddTable`/`Rewrite` 记录日志，Rust没有日志。Go reset 向量初始容量为 `len(items)`，Rust为 `2 * len(items)`，只影响分配策略。

Go 的 `model.TiFlashReplicaInfo` 还含可用性相关字段，而 Rust本地类型仅保留生成 DDL 所需的 `Count` 和 `LocationLabels`。更关键的是，Go 类型已接入生产恢复链，Rust本地类型尚未与 task/checkpoint 模型适配；API 同名和测试对齐不能替代生产接线证据。

## 扩展指南

若要把该实现接入 Rust 恢复主链，优先建立明确的模型适配：为 `br/pkg/task/restore.rs::TiFlashReplicaRecorder` 实现桥接，或统一 trait/模型归属；为 checkpoint 的 `TiFlashReplicaInfo` 提供无损转换；再让 PiTR checkpoint 读取、ID 重写、DDL 生成和执行形成可追踪调用边。不能仅在某处复制 map，因为 `Load`、`Rewrite` 和 reset 顺序都属于恢复一致性协议。

新增字段时应判断它是否影响最终 DDL；仅运行期状态不应无依据地塞进最小 `TiFlashReplicaInfo`。修改 `Rewrite` 时要为目标 ID 冲突定义明确策略，并在独立 `tiflash_recorder_test.rs` 增加旧 ID 缺失、同 ID、多对一覆盖测试。修改 SQL 格式化时必须同步 Go `alterTableSpecOf` 的 Restore flags，覆盖反引号、单引号、反斜杠、空字符串和多标签，并保持测试不内嵌到生产文件。

如果让 `alterTableSpecOf` 真正返回错误，应把生成 API 改为可向上游传播错误，或至少保证 reset 的两条语句按表原子地产生，避免只返回清零语句。若调用方需要稳定 DDL 顺序，应在生成层按 table ID 排序或在外层排序，不能依赖 `HashMap`。兼容性风险集中在 Go/Rust parser 转义差异和本地模型漂移；性能风险主要是大量表与长 labels 造成的字符串分配，但当前算法本身为线性。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter br/pkg/restore/tiflashrec` 找到本目录的 Rust/Go 实现、入口和测试。
- RustCodeGraph `node --file br/pkg/restore/tiflashrec/tiflash_recorder.rs --offset 1 --limit 400`：核对完整 185 行源码、类型、公开方法、两个 DDL 生成分支及转义实现。
- RustCodeGraph `query TiFlashRecorder`、`query GenerateResetAlterTableDDLs`、`query GenerateAlterTableDDLs`、`query alterTableSpecOf` 与精确 `explore`：核对 Rust/Go 同名符号，并确认生成方法到 `Iterate`、`TableByID`、`EncloseDBAndTable`、`alterTableSpecOf` 的调用边。
- 读取 `br/pkg/restore/tiflashrec/Cargo.toml`、根 `Cargo.toml` 与同目录 `lib.rs`：核对 workspace 成员、crate 名、无依赖声明、模块挂载、扁平导出和独立测试接线；目标目录不存在 `doc.go`。
- RustCodeGraph 读取 `tiflash_recorder.go` 和 `tiflash_recorder_test.go`：核对 Go 设计背景、生产 API、AST Restore flags、日志/缺 schema 分支与三组对照测试。
- RustCodeGraph 读取 `tiflash_recorder_test.rs` 和 `parity_test.rs`：核对增删改写、Load/GetItems、HashMap 顺序无关、标签转义、reset 顺序和缺表跳过。
- RustCodeGraph `node PreCheckTableTiFlashReplica`、`node LoadOrCreateCheckpointMetadataForLogRestore`，以及 Cargo/源码定向搜索：核对 Go 生产调用边和 Rust task/checkpoint 的不同接口；未找到目标 Rust 类型实现 task trait或被其他 Cargo 包依赖的证据。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试；交付仅运行任务指定的 11 章节结构检查，并人工复核所有“当前已接线”结论均有直接证据。
