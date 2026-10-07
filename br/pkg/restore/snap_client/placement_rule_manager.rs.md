# [`br/pkg/restore/snap_client/placement_rule_manager.rs`](./placement_rule_manager.rs)

## 文件定位

本文件属于 `astersql-br-pkg-restore-snap-client` library crate；crate 入口 `br/pkg/restore/snap_client/lib.rs` 以 `placement_rule_manager` 模块装载并公开重导出这里的 API。它位于快照恢复的数据发送链路之前：`SnapClient::RestoreTables`（`tikv_sender.rs:276-342`）创建管理器、调用 `SetPlacementRule`，执行切分与 SST 恢复，最后调用 `ResetPlacementRules`。

这里实现的是恢复期间的 PD placement 规则生命周期，不负责表范围切分、SST 下载/摄取或校验和。依赖由 `br/pkg/restore/snap_client/stubs.rs` 中的本地 `StoreMeta`、`SplitClient`、`Context` 等抽象提供；`Cargo.toml` 也明确该 crate 当前采用本地 traits/stubs，而不是直接依赖 kvproto/grpcio。因此它已经接入本 crate 的 Rust 恢复流程，但客户端边界仍是迁移期的本地抽象。

## 核心职责

1. `loadRestoreStores` 从 PD store 元数据中筛选可用于在线恢复的 TiKV：先由 `GetAllTiKVStoresWithRetry` 排除带 `engine=tiflash` 标签的节点，再只保留状态为 `Up` 且带 `exclusive=restore` 标签的 store ID。
2. `NewPlacementRuleManager` 在离线恢复或没有 restore store 时返回空操作实现；在线且存在 restore store 时要求调用方提供 `SplitClient`，并构造在线管理器。
3. 在线 `SetPlacementRule` 收集表 ID 与分区 ID，为每个物理表范围安装高优先级、覆盖式 placement rule，然后等待所有相关 Region 的所有 peer 都迁移到 restore store。
4. `ResetPlacementRules` 在恢复结束后逐表删除临时规则；删除失败不会在首个错误处停止，而是聚合失败表 ID 后返回错误。

上述行为把在线恢复流量隔离到专用 TiKV 节点；离线或无专用节点场景则保持统一调用接口但不修改 PD。

## 主要符号

- `restoreLabelKey` / `restoreLabelValue`：分别固定为 `exclusive` / `restore`，既用于 store 筛选，也用于新规则的 `LabelConstraint`。
- `PlacementRuleManager: Send`：公开生命周期接口，包含可变接收者的 `SetPlacementRule` 与 `ResetPlacementRules`。返回 `Box<dyn PlacementRuleManager>` 使调用方无需识别在线/离线具体类型。
- `loadRestoreStores(ctx, pd_client)`：读取并筛选 store，返回符合条件的 ID 列表；底层元数据错误直接传播。
- `NewPlacementRuleManager(...)`：工厂函数。只有“在线且找到 restore store”的分支才消费 `tool_client`；缺少该客户端会返回 `split client required for online mode`。
- `offlinePlacementRuleManager`：两个 trait 方法均无副作用地返回成功。
- `onlinePlacementRuleManager`：持有 `Arc<dyn SplitClient>`、restore store ID 集合、以 `HashMap<i64, ()>` 表示的物理表 ID 集合，以及可供测试缩短的 `waitInterval`（生产默认 10 秒）。
- `setupPlacementRules`：读取 `pd/default` 规则作为模板，将 `Index` 设为 100、`Override` 设为 `true`，追加 restore 标签约束，再按表 ID 克隆并写入规则。
- `checkRegions` / `checkRange`：分别在表级和 Region 级检查调度完成度，返回 `(是否完成, 进度字符串)`；底层扫描错误仍用 `Result` 传播。
- `waitPlacementSchedule`：持续检查 placement，直到就绪或 `Context` 取消。
- `getRuleID(table_id)`：生成稳定规则 ID `restore-t{table_id}`。

## 执行流程

在线恢复的完整路径如下：

1. `SnapClient::RestoreTables` 把 `pd_store_meta()`、`meta_client` 与 `RestoreTablesContext::Online` 交给 `NewPlacementRuleManager`。
2. 工厂在离线模式立即返回空实现；在线模式调用 `loadRestoreStores`。筛选结果为空同样降级为空实现，非空时用 store IDs、空 `restoreTables` 和 10 秒轮询间隔构造在线实现。
3. `SetPlacementRule` 把每个 `CreatedTable.Table.ID` 放入 `restoreTables`，若表有分区，再加入每个 `Partition.Definitions[*].ID`；`HashMap` 同时完成去重。
4. `setupPlacementRules` 获取默认规则。对每个物理表 ID，以 `codec::EncodeBytes(tablecodec::EncodeTablePrefix(id))` 和下一个 ID 的前缀形成 `[start, end)`，转为 PD HTTP API 使用的十六进制字符串后调用 `SetPlacementRule`。
5. `waitPlacementSchedule` 先立即调用一次 `checkRegions`。后者逐表扫描范围；`checkRange` 检查每个 Region 的每个 peer，只有 peer 的 `StoreId` 全都属于 `restoreStores` 才视为就绪。未就绪时记录类似 `table x/y, region i/n` 的进度，等待一个间隔后重试。
6. placement 就绪后，`RestoreTables` 才进入范围整理、split、SST 恢复等主工作。主工作闭包无论成功或失败，随后都会尝试 `ResetPlacementRules`；重置错误仅告警，方法最终返回主工作结果。

需要注意：若工厂或 `SetPlacementRule` 本身失败，`RestoreTables` 会在进入主工作闭包前通过 `?` 返回，此路径不会执行后面的 reset。若设置多条规则时中途失败，已成功写入的规则也不会由本文件自动回滚。

## 数据与状态

`restoreStores: Vec<u64>` 是构造时获得的 restore 节点快照，等待期间不会重新加载；节点标签或状态随后改变时，只能通过 Region peer 的实际位置间接体现。成员检查使用线性 `Vec::contains`，复杂度约为“扫描到的 peer 数 × restore store 数”。

`restoreTables: HashMap<i64, ()>` 在每次 `SetPlacementRule` 时累积表与分区 ID，不会在成功设置或 reset 后清空。因其是 `HashMap`，规则写入、检查和删除的遍历顺序不稳定，进度中的表序号也不对应固定 table ID；正确性不能依赖处理顺序。重复 ID 会自然去重。

规则范围的结束 ID 使用 `table_id.wrapping_add(1)`。这显式保留 Go `int64` 加法的环绕语义，`i64::MAX` 的结束前缀会使用 `i64::MIN`，并由 Rust 独立测试锁定。规则从 `pd/default` 克隆，保留默认规则的其余字段，只覆盖 ID、范围、索引、override 并追加标签约束。

空表集合会读取默认规则但不写入任何表规则；`checkRegions` 因没有待检查表而立即返回就绪。`checkRange` 对空 Region 列表也返回就绪，这是当前代码事实，调用方依赖扫描客户端正确覆盖给定范围。

## 依赖与调用关系

上游生产调用边是 `tikv_sender.rs` 的 `SnapClient::RestoreTables -> NewPlacementRuleManager -> PlacementRuleManager::SetPlacementRule/ResetPlacementRules`。`lib.rs` 将本模块公开重导出；RustCodeGraph 文件关系与 `rg` 还显示独立测试 `placement_rule_manager_test.rs` 和 `parity_test.rs` 使用这些符号。

主要下游边如下：

- `loadRestoreStores -> GetAllTiKVStoresWithRetry -> StoreMeta::GetAllStores`；snap-client 的 stub helper 排除 `engine=tiflash`。
- `setupPlacementRules -> SplitClient::{GetPlacementRule, SetPlacementRule}`，并依赖 `tablecodec::EncodeTablePrefix`、`codec::EncodeBytes`、`bytes_to_hex` 生成 PD 范围。
- `checkRegions -> checkRange -> SplitClient::ScanRegions`。
- `ResetPlacementRules -> SplitClient::DeletePlacementRule`。
- 取消和错误边界来自 `Context::{Done, Err}`、本地 `Error/Result` 与 `berrors::ErrPDInvalidResponse`。

`Cargo.toml` 声明 crate 名为 `astersql-br-pkg-restore-snap-client`，依赖同仓的 restore/utils/errors crate 以及 serde/sha2；本文件自身实际导入集中在标准库与 `crate::stubs`，没有条件编译项。

## 错误处理与边界

- store 枚举、默认规则读取、规则写入、Region 扫描的任一错误都会立即向上传播；`SetPlacementRule` 不做补偿性删除。
- 在线且确有 restore store 时，缺少 `SplitClient` 是显式构造错误；在线但没有 restore store 则告警并安全降级为空实现。
- `ResetPlacementRules` 尝试删除所有已记录规则，收集全部失败 ID，最后用 `Error::Annotatef(berrors::ErrPDInvalidResponse(...), ...)` 返回聚合错误。成功删除的规则不会因其他删除失败而恢复。
- `waitPlacementSchedule` 在每轮扫描前以及轮询等待期间检查取消；等待被拆成至多 10ms 的 sleep 片段，因此取消观察延迟不会等满默认 10 秒。若 `Context::Err()` 意外为空，则回退为 `context canceled` 错误。
- peer 不在 restore store 是“尚未完成”的瞬时状态，不是错误；函数继续轮询。扫描到 Region 但 Region 没有 peer 时当前实现会将该 Region 视为通过。
- 规则范围与 Go 一样采用表前缀的半开区间；最大表 ID 的环绕虽经过测试，但其 PD 范围是否符合真实集群业务约束，不能仅由本地 stub 测试证明。

## 并发与资源生命周期

管理器本身由 `&mut self` 的 trait 方法串行使用，trait 只要求 `Send`，没有承诺同一管理器可由多个线程同时调用。`toolClient` 用 `Arc` 共享，实际并发安全由 `SplitClient: Send + Sync` 保证；`restoreTables` 不带锁。

等待调度是当前线程上的同步轮询，并使用 `std::thread::sleep`，不会创建后台任务或通道。默认每次未就绪后等待 10 秒；测试通过直接构造在线管理器把间隔改为毫秒级。`Context` 负责外部取消，但本文件不设置超时，因此未取消且调度永不完成时会无限等待。

临时规则的预期生命周期是“`SetPlacementRule` 成功后存在，恢复主工作结束后 reset”。`RestoreTables` 对主工作成功/失败都会尝试 reset，但忽略 reset 错误；设置阶段失败则可能留下部分规则。`ResetPlacementRules` 也不清空内存状态，多次调用会重复尝试删除相同 ID，能否保持幂等取决于 `SplitClient::DeletePlacementRule` 对不存在规则的语义。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/restore/snap_client/placement_rule_manager.go`，Rust 保留了相同的接口分层、标签常量、表/分区 ID 收集、默认规则克隆语义、`Index=100`、`Override=true`、`in` 标签约束、规则 ID、范围编码、全 peer 检查、轮询等待和聚合删除失败。

可见差异包括：

- Go 工厂接收 PD client、PD HTTP client 与 TLS 配置并内部构造 `split.NewClient`；Rust 工厂接收 `StoreMeta` 和可选的 `Arc<dyn SplitClient>`，符合当前本地 stub 依赖边界。
- Go 的 ticker 首次检查发生在首个 10 秒 tick；Rust 进入循环后立即检查，再在未就绪时等待。两者最终就绪/取消语义一致，但首次检查时序不同。
- Go 测试用 failpoint 把 ticker 改为 500ms；Rust 没有 failpoint，而是在 `onlinePlacementRuleManager.waitInterval` 注入短间隔。
- Rust 使用 `wrapping_add(1)` 明确表达 Go 的整数环绕；相应边界测试是 Rust 额外的迁移保护。
- Rust `RestoreTables` 的生产调用边已存在，但 `Cargo.toml` 注释表明该 crate 在 arm64 darwin 等环境仍以本地 traits/stubs 替代真实 kv/domain/kvproto/grpcio。文档不能据此声称已验证真实 PD 网络交互。

Go 测试 `placement_rule_manager_test.go` 验证离线、在线无 restore store 和在线完整生命周期；Rust 同名独立测试复刻这些场景，并额外验证未就绪重试和最大 ID 环绕。`parity_test.rs::go_rust_public_contract_matches` 还检查离线接口、store 标签筛选与规则 ID。

## 扩展指南

- 调整 store 选择策略时，应同时修改 `loadRestoreStores` 及 `stubs.rs::GetAllTiKVStoresWithRetry` 的职责边界，并在 `placement_rule_manager_test.rs` 加入 Up/Offline、TiKV/TiFlash、标签缺失/重复等组合；同步核对 Go 的 `util.SkipTiFlash` 语义。
- 改变规则内容或范围时，入口是 `setupPlacementRules` 与 `getRuleID`。必须保持 PD group/ID、tablecodec 编码、半开区间和默认规则字段兼容，并扩展独立测试去断言实际写入 `MemSplitClient.rules` 的字段。
- 改变调度完成条件时，应修改 `checkRegions`/`checkRange`，覆盖空 Region、空 peer、多 peer 跨 store、扫描错误、多个表/分区以及进度文本；不要把测试写入生产 `.rs` 文件。
- 改变等待机制时，应保留 `Context` 取消和“未就绪可重试”的合同，评估同步 sleep 对调用线程的影响，并更新 `test_context_manager_online_retries_until_regions_are_ready`。
- 强化清理可靠性时，需要同时审视 `SetPlacementRule` 部分成功的回滚、`RestoreTables` 在设置失败时的清理，以及 reset 错误当前仅记录告警的策略；这会跨到 `tikv_sender.rs`，不应只在本文件局部声称解决。
- 新增管理器状态时应注意 trait 仅要求 `Send`，当前调用为串行可变访问；若要并发调用，需要明确同步策略而不是直接共享 `restoreTables`。
- 所有行为变化都应与 `placement_rule_manager.go` 保持迁移语义一致，并更新独立的 `placement_rule_manager_test.rs`；真实 PD/TiKV 行为仍需在具备相应客户端的更高层验证。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/restore/snap_client` 定位本模块、Go 对照与测试；`node --file br/pkg/restore/snap_client/placement_rule_manager.rs --offset 1 --limit 400` 读取目标文件全部 262 行；`node` 还核对了 `tikv_sender.rs:250-364`、`stubs.rs:1590-1699`、`stubs.rs:2040-2139` 与 `stubs.rs:2240-2298` 的直接调用和依赖实现。`query NewPlacementRuleManager`、`query GetAllTiKVStoresWithRetry` 用于消除 Go/Rust 同名符号歧义；批量 callers/callees 查询无有效输出后，以精确源码节点和下述文本检索补证。
- Rust 源与入口：`br/pkg/restore/snap_client/placement_rule_manager.rs`、`lib.rs`、`tikv_sender.rs`、`stubs.rs`。
- crate 边界：`br/pkg/restore/snap_client/Cargo.toml`。
- Rust 独立测试：`br/pkg/restore/snap_client/placement_rule_manager_test.rs`、`parity_test.rs`。
- Go 对照：`br/pkg/restore/snap_client/placement_rule_manager.go`、`placement_rule_manager_test.go`、`tikv_sender.go`。
- 文本调用核验：`rg -n "NewPlacementRuleManager|SetPlacementRule|ResetPlacementRules|setupPlacementRules|waitPlacementSchedule|checkRegions|checkRange|getRuleID|restoreLabel(Key|Value)" br --glob '*.rs' --glob '*.go'`，确认 Rust 生产入口与测试调用，以及 Go 的对应链路。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证本文档存在且恰有 11 个固定二级章节，并人工复核唯一产物、源码链接与未验证边界。
