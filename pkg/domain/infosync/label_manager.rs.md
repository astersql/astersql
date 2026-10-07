# `pkg/domain/infosync/label_manager.rs`

## 文件定位

本文件属于 `astersql-domain-infosync` crate，crate 入口 `pkg/domain/infosync/lib.rs` 通过 `mod label_manager` 装入模块并公开重导出其符号。它位于 Domain 的信息同步层，在上层 `InfoSyncer` 与下层 PD Region Label HTTP API 之间提供可替换的规则管理边界；没有 PD HTTP 客户端时，同一边界由内存实现支撑测试和 mock-storage 路径。crate 的归属、入口和依赖分别见 `pkg/domain/infosync/Cargo.toml` 与 `pkg/domain/infosync/lib.rs:16-31`。

这里管理的是 `ddl_label` crate 导出的 `label::Rule`，批量更新载体 `LabelRulePatch` 和 HTTP 抽象 `PdHttpClient` 则定义在 `pkg/domain/infosync/types.rs:102-159,200-209`。本文件不负责生成规则内容、初始化全局单例或实现真实网络传输。

## 核心职责

- `LabelRuleManager`（`label_manager.rs:28-37`）统一单条写入、批量增删、全量读取和按 ID 读取四种操作，使 `InfoSyncer` 不依赖具体 PD 客户端。
- `PDLabelManager`（`:40-72`）把四种操作直接适配到 `PdHttpClient` 的 Region Label 方法，并把按 ID 返回的列表整理为以规则 ID 为键的映射。
- `mockLabelManager`（`:74-122`）用锁保护的 JSON 字节映射模拟同一 CRUD 语义；采用序列化快照而不是直接保存 `Rule`，避免调用方持有的值与存储内容共享可变状态。
- `filterRulesByKeyspace`（`:125-139`）为“读取全部规则”提供租户隔离过滤；关闭 keyspace 感知时保留输入顺序和全部元素，开启时只保留 ID 以 `keyspace/<id>/` 开头的规则。

## 主要符号

- `Codec { keyspace_id: Option<u32>, keyspace_aware_rules: bool }`（`:18-25`）：本文件使用的最小 TiKV codec 上下文。缺省 keyspace ID 在过滤时解释为 `0`，而是否过滤由独立布尔值决定。
- `LabelRuleManager: Send + Sync`（`:28-37`）：对象安全的同步 trait，可放入 `Arc<dyn LabelRuleManager>` 并跨线程共享。方法名保留 Go 风格以对齐移植接口。
- `PDLabelManager { pdHTTPCli: Arc<dyn PdHttpClient> }`（`:40-43`）：真实后端适配器。客户端本身也必须满足 `Send + Sync`，其实际传输、超时和重试策略不在本文件内。
- `mockLabelManager { labelRules: RwLock<HashMap<String, Vec<u8>>> }`（`:74-79`）：内部 mock 后端，通过 `Default` 创建空存储；类型未声明为 `pub`，但由同 crate 的 `GlobalInfoSyncerInit` 使用。
- `filterRulesByKeyspace(Vec<label::Rule>, Codec) -> Vec<label::Rule>`（`:125-139`）：公开纯过滤函数，也是本文件当前独立 Rust 测试直接覆盖的符号。

文件中没有模块级业务常量、条件编译项或异步函数。`label::KeyspacePrefix` 是过滤前缀的来源，不在本文件重复定义。

## 执行流程

1. `GlobalInfoSyncerInit` 检查是否提供 `pdHTTPCli`：存在时构造 `PDLabelManager`，不存在时构造空的 `mockLabelManager`，然后把它作为 `Arc<dyn LabelRuleManager>` 存入 `InfoSyncer.labelRuleManager`（`pkg/domain/infosync/info.rs:163-181,201-218`）。
2. 上层通过 `pkg/domain/infosync/info.rs:600-629` 的公共函数访问管理器。`PutLabelRule(None)`、空的 `LabelRulePatch` 和空 ID 列表在该层提前成功返回；其他输入转发给 trait 方法。`GetAllLabelRules` 还把 `InfoSyncer.tikvCodec` 交给后端。
3. 真实后端中，`PutLabelRule`/`UpdateLabelRules` 分别调用 `set_region_label_rule`/`patch_region_label_rules`；`GetAllLabelRules` 先从 PD 取全量列表，再过滤；`GetLabelRules` 从 PD 取列表，再以每条响应规则自身的 `ID` 建表（`label_manager.rs:45-71`）。
4. mock 后端中，写入先序列化；批量更新持有一次写锁，依次删除 `DeleteRules`、再序列化并覆盖 `SetRules`，因此同一 ID 同时出现时最终保留设置值（`:81-100`）。读取持有读锁，将 JSON 反序列化为新值；按 ID 查询忽略不存在的 ID（`:102-121`）。
5. 全量读取最终调用 `filterRulesByKeyspace`。开启过滤时以 `label::KeyspacePrefix`、`codec.keyspace_id.unwrap_or_default()` 和尾随 `/` 拼接精确前缀，再用 `starts_with` 保留匹配规则（`:124-139`）。

仓库中的实际 Rust 上游之一是 `pkg/session/runtime/create_table_resources.rs:541-618`：`update_labels` 根据表和分区生成旧规则 ID，调用 `infosync::GetLabelRules` 读取旧规则，重置规则的 schema/table/物理 ID，最后通过 `infosync::UpdateLabelRules` 原子表达“可选删除旧 ID + 设置新规则”。

## 数据与状态

真实后端的持久状态位于 PD；`PDLabelManager` 自身只保存共享客户端句柄。`GetLabelRules` 返回的 `HashMap` 以 PD 响应中的 `Rule.ID` 为准，因此返回顺序不构成接口契约，重复 ID 会被后出现的规则覆盖。

mock 后端以 `Rule.ID -> JSON bytes` 存储。`PutLabelRule` 和补丁的设置部分对同 ID 执行覆盖；删除不存在的 ID、查询不存在的 ID都不报错。`GetAllLabelRules` 从 `HashMap` 收集结果，顺序不稳定；keyspace 过滤仅删减该次返回的向量，不修改底层存储。`LabelRulePatch` 的 JSON 字段形状是 `deletes`/`sets`，见 `pkg/domain/infosync/types.rs:200-209`。

关键不变量是：开启感知后，只有规则 ID 满足 `keyspace/<当前 ID>/...` 才可见；前缀包含尾随 `/`，避免 keyspace `4` 错误匹配 `42`。关闭感知时函数直接返回原向量，不重新排序或复制元素。

## 依赖与调用关系

上游装配链为 `GlobalInfoSyncerInit -> Arc<dyn LabelRuleManager> -> InfoSyncer.labelRuleManager`，公共转发 API 为 `PutLabelRule`、`UpdateLabelRules`、`GetAllLabelRules`、`GetLabelRules`（`pkg/domain/infosync/info.rs:163-218,600-629`）。RustCodeGraph 的文件查询显示 `label_manager.rs` 被 `info.rs` 及相关测试等文件引用；符号探索还确认 `update_labels` 是 `GetLabelRules` 和 `UpdateLabelRules` 的生产调用者。

下游边包括：

- `PDLabelManager::PutLabelRule -> PdHttpClient::set_region_label_rule`；
- `PDLabelManager::UpdateLabelRules -> PdHttpClient::patch_region_label_rules`；
- `PDLabelManager::GetAllLabelRules -> get_all_region_label_rules -> filterRulesByKeyspace`；
- `PDLabelManager::GetLabelRules -> get_region_label_rules_by_ids -> HashMap collect`；
- mock 方法到 `serde_json::{to_vec, from_slice}` 以及 `RwLock<HashMap<...>>`。

`Cargo.toml` 直接声明了 `ddl-label`、`serde_json`，并通过本 crate 的 `types.rs` 间接使用 `PdHttpClient`/`LabelRulePatch`；`tikv-client` 固定引用上游 tag `v0.4.2-aster.10`。本文件的 `Codec` 是本地移植类型，并非直接使用该依赖中的 codec 类型。

## 错误处理与边界

所有管理操作返回 crate 的 `Result`。PD 客户端错误通过 `?` 原样传播；默认 `PdHttpClient` 方法会返回 `Error::External("... is unsupported")`（`types.rs:138-159`）。mock 的 JSON 序列化/反序列化错误也通过 `?` 转换并传播。

trait 实现把 `None` 规则或补丁视为成功空操作（`label_manager.rs:46-56,82-92`）。公共 `InfoSyncer` 转发层还会跳过空补丁和空 ID 列表。按 ID 查询只返回实际存在或由 PD 返回的规则，不为缺失 ID制造占位值。keyspace 过滤只检查字符串前缀，不验证 ID 的其余语法，也不会报告格式错误。

`RwLock::read/write().unwrap()` 在锁中毒时会 panic，而不是返回领域错误；这是当前实现的明确边界。mock 批量设置若某条规则序列化失败，之前已经完成的删除或设置不会回滚，因此它不是事务性存储。真实 PD 补丁是否原子、是否重试由注入的客户端实现决定，本文件没有提供额外保证。

## 并发与资源生命周期

`LabelRuleManager: Send + Sync` 与 `Arc<dyn ...>` 使一个管理器可由多个调用线程共享。`PDLabelManager` 不增加本地锁，调用并发能力和连接生命周期由 `Arc<dyn PdHttpClient>` 承担；删除最后一个 `Arc` 后客户端才会释放。

mock 使用 `std::sync::RwLock`：单条写入和整个补丁持有独占写锁，全量读取的“遍历 + 全部反序列化”以及按 ID 查询都在共享读锁内完成。补丁的删后设对其他使用同一 mock 的线程不可见中间状态，但 JSON 错误返回后不会撤销锁内已经执行的修改。代码不创建线程、任务、通道、事务或显式取消上下文。

`InfoSyncer` 在进程级 `OnceLock<RwLock<Option<Arc<InfoSyncer>>>>` 中持有管理器（`info.rs:104-154`），初始化时确定真实或 mock 后端；本文件自身没有热切换逻辑。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/domain/infosync/label_manager.go`。两版均定义四方法 `LabelRuleManager`、PD 与内存实现、删后设的补丁顺序、JSON 快照存储、按 ID 忽略缺失规则，以及 `keyspace/<id>/` 前缀过滤。Rust 独立测试 `label_manager_test.rs:12-45` 对齐 Go 测试 `label_manager_test.go:27-46` 的核心两条路径：关闭感知返回全部规则，开启且 ID 为 42 时只返回对应规则。

已验证的移植差异如下：

- Go 方法都接收 `context.Context`；Rust trait 是同步 API且没有 context 参数，取消与超时只能由 `PdHttpClient` 内部承担。
- Go 使用 `tikv.Codec` 并通过 `label.UseKeyspaceAwareRules(codec)` 结合内核类型判断是否过滤；Rust 使用显式 `Codec` 布尔位。因此 Go 测试包含 classic kernel 下即使 CodecV2 也不滤的分支，Rust 测试没有等价的内核类型分支，调用方必须正确设置 `keyspace_aware_rules`。
- Go 的 `Rule` 和补丁设置项是指针，可跳过 `nil`；Rust 使用拥有所有权的 `Rule` 值，类型层面不存在 `SetRules` 中的空元素。Rust 将 trait 层的 `None` 明确视为空操作。
- Go mock 的 `GetLabelRules` 对请求 ID 与全部存储执行嵌套扫描；Rust 直接按 ID 查询 `HashMap`，结果语义一致而复杂度更低。
- Go `GetAllLabelRules` 跳过存储中的 `nil` 字节值；Rust 字段私有且正常写路径只存有效 JSON 字节，不表示该 `nil` 状态。

## 扩展指南

新增一种标签规则操作时，应先扩展 `LabelRuleManager`，同时实现 `PDLabelManager` 与 `mockLabelManager`，再在 `info.rs` 添加必要的公共转发；网络形状或客户端能力应落在 `types.rs` 的 `PdHttpClient`，不要把 HTTP 细节泄漏到上层。新增功能必须同步放在独立测试文件 `pkg/domain/infosync/label_manager_test.rs`，不要把测试嵌入生产源文件；若要保持 Go 移植一致，也应核对 `label_manager.go` 与 `label_manager_test.go`。

修改过滤规则时，重点覆盖关闭感知、`None`/0 keyspace、相邻数字前缀、格式错误 ID、输入顺序，以及 classic/non-classic 语义由谁决定。若要改变 mock 的更新原子性，应避免序列化到一半才失败，可先在锁外完成全部序列化，再一次持锁提交；这会改变错误时可见状态，需要新增回归测试并核对 Go 行为。

扩展并发或异步支持时要保留 `Send + Sync` 边界，并明确 context/取消、超时和重试由哪一层负责。性能风险主要在全量规则读取和 JSON 反序列化仍发生在读锁内，以及全量 PD 拉取后才做本地 keyspace 过滤；改变为服务端过滤前必须确认 PD API 与多租户兼容性。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；索引中 `pkg/domain/infosync/label_manager.rs` 为 139 行、18 个符号。
- RustCodeGraph 文件/符号查询：读取 `label_manager.rs:1-139`，查询到 Rust `LabelRuleManager`、三个 `UpdateLabelRules` 和三个 `GetAllLabelRules` 定义；探索结果确认 trait 到 PD/mock 实现的分派边、两个 `GetAllLabelRules` 实现到 `filterRulesByKeyspace` 的边，以及 `info.rs`/`create_table_resources.rs::update_labels` 的上游使用。
- 已读 Rust 路径：`pkg/domain/infosync/label_manager.rs`、`lib.rs`、`info.rs:1-225,600-629`、`types.rs:1-220`、`label_manager_test.rs`、`pkg/session/runtime/create_table_resources.rs:500-618`。
- 已读配置与 Go 对照：`pkg/domain/infosync/Cargo.toml`、`label_manager.go`、`label_manager_test.go`。`pkg/domain` 下未找到适用的 `doc.go`。
- 独立测试现有覆盖仅直接验证 keyspace 过滤两条路径；本任务是纯文档分析，按计划不运行 Cargo，未声称运行期测试结果。
- 交付结构以任务指定命令验证，要求目标文件存在且恰有本文的十一个固定二级标题；人工复核同时检查了文件存在理由、真实/模拟执行链、边界、Go 差异和安全扩展入口。
