# `pkg/domain/infosync/placement_manager.rs`

## 文件定位

本文件属于 `astersql-domain-infosync` crate，定义 Placement Rule Bundle 的最小读写抽象、基于 PD HTTP 客户端的生产实现、供无 PD 场景使用的内存 mock，以及一项面向 leader 规则的本地重叠检查。crate 入口 `pkg/domain/infosync/lib.rs` 将该模块的公开项重新导出；`pkg/domain/infosync/Cargo.toml` 通过 `ddl-placement` 依赖取得 `placement::Bundle` 和 PD 规则类型，并以 `package.metadata.porting.go-package = "pkg/domain/infosync"` 声明 Go 对照包。

它不是 Placement Bundle 的构造器，也不负责 DDL 策略解析。Bundle 的数据模型及 `IsEmpty` 语义位于 `pkg/ddl/placement/bundle.rs`；本文件位于 Domain 的信息同步边界，把上层对 Bundle 的读写统一转发到 PD 或本地 mock。`InfoSyncer::placementManager` 是该抽象在应用中的持有者（`pkg/domain/infosync/info.rs`）。

## 核心职责

1. `PlacementManager` 统一三个同步操作：按组名读取、读取全部、批量写入。
2. `PDPlacementManager` 把操作转发给 `PdHttpClient`，并保持 PD 批量接口的 partial-update 参数为 `true`。
3. `mockPlacementManager` 用互斥保护的 `HashMap` 模拟 Bundle 存储；空 Bundle 表示删除，其余 Bundle 按 `ID` 覆盖写入。
4. `CheckBundle` 只检查同一 Bundle 内、角色为 leader 的规则范围是否重叠；`checkBundles` 将该检查应用到 mock 当前保存的全部 Bundle。

职责边界必须严格区分：生产 `PDPlacementManager::PutRuleBundles` 不调用 `CheckBundle`，校验最终由 PD 接口负责；本地检查只在 mock 写入路径中自动执行。文件也不处理重试，重试位于 `info.rs` 的 `PutRuleBundlesWithRetry`。

## 主要符号

- `pub trait PlacementManager: Send + Sync`：可跨线程共享的同步接口。`GetRuleBundle(&self, name)` 返回拥有所有权的 `placement::Bundle`；`GetAllRuleBundles` 返回快照向量；`PutRuleBundles` 接收借用切片。方法名保留 Go 命名以便移植对照。
- `pub struct PDPlacementManager`：仅保存 `Arc<dyn PdHttpClient>`。`Arc` 使同一 PD 客户端可被 `InfoSyncer` 及多个管理器共享。
- `impl PlacementManager for PDPlacementManager`：`GetRuleBundle` 调用 `get_placement_rule_bundle` 后强制以请求参数补齐返回 Bundle 的 `ID`；`GetAllRuleBundles` 原样返回客户端结果；`PutRuleBundles` 对空切片直接成功，否则调用 `set_placement_rule_bundles(bundles, true)`。
- `pub struct mockPlacementManager`：`Mutex<HashMap<String, placement::Bundle>>` 的默认空实例。虽然类型公开，内部 map 私有，正常入口是 `GlobalInfoSyncerInit` 在没有 PD 客户端时创建它。
- `impl PlacementManager for mockPlacementManager`：读取均克隆数据，避免调用者持有 map 内对象引用；缺失的单项查询返回仅设置请求 `ID` 的默认 Bundle。批量写入以 `Bundle::IsEmpty()` 判定删除语义。
- `pub fn CheckBundle`：收集 leader 规则的 `(StartKeyHex, EndKeyHex)`，按元组字典序排序，再检查相邻范围；发现 `next.start < previous.end` 时返回与 PD 风格一致的 8243 错误文本。
- `pub fn checkBundles`：遍历 map 中所有 Bundle，遇到第一个 `CheckBundle` 错误即短路返回。它虽然是 `pub`，但当前直接调用点只有本文件的 mock 写入实现。

本文件没有模块级常量、枚举、条件编译项或异步函数。

## 执行流程

生产初始化链如下：`GlobalInfoSyncerInit` 检查可选 `pdHTTPCli`；存在客户端时构造 `PDPlacementManager`，否则构造默认 `mockPlacementManager`；结果以 `Arc<dyn PlacementManager>` 放入 `InfoSyncer::placementManager`。随后 `info.rs` 的 `GetRuleBundle`、`GetAllRuleBundles` 和 `PutRuleBundles` 先取得全局 `InfoSyncer`，再动态分派到本文件实现。

单项生产读取时，`PDPlacementManager::GetRuleBundle` 调用 PD 客户端；错误通过 `?` 原样传播，成功响应则无条件把 `bundle.ID` 设为请求名。这补偿了 PD 响应可能不回传 group ID 的情况，并保证调用者得到稳定标识。

生产批量写入时，空切片不访问 PD；非空切片以 partial-update=`true` 发送。调用方如需恢复性重试，走 `info.rs::PutRuleBundlesWithRetry`：`Error::DomainService` 立即返回，其他错误最多在首次尝试后再重试 `maxRetry` 次。

mock 批量写入时，方法先取得整张 map 的互斥锁，再按输入顺序处理每个 Bundle：`IsEmpty()` 为真则按 ID 删除，否则克隆并覆盖同 ID 条目；全部变更完成后，在锁仍持有期间调用 `checkBundles`。因此一个批次中的后项可覆盖前项，同一时刻其他线程看不到半批状态。

`CheckBundle` 按原始规则顺序扫描。非 leader 规则完全忽略；遇到 leader 且该规则的 `Override` 为真时先清空此前累计范围，再加入当前范围。扫描结束后排序，使输入规则不必按 key 排列。相邻范围仅在严格小于（`next.start < previous.end`）时视为重叠，所以首尾相接的半开区间可通过。

## 数据与状态

`placement::Bundle` 的核心字段是 `ID`、`Index`、Bundle 级 `Override` 与 `Rules: Vec<pd::Rule>`（`pkg/ddl/placement/bundle.rs::Bundle`）。本文件的删除判定不是“规则为空”这一单一条件，而是 `Rules.is_empty() && Index == 0 && !Override`，即 `Bundle::IsEmpty()` 的完整定义；因此带非默认索引或 Bundle 级覆盖标志的零规则 Bundle仍会被保存。

生产实现自身无可变状态，状态在外部 `PdHttpClient`/PD。mock 的唯一可变状态是 `bundles` map，键取写入 Bundle 的 `ID`，值为完整克隆。单项缺失读取不会插入 map，只临时构造默认 Bundle；全量读取的向量顺序来自 `HashMap::values()`，没有稳定顺序保证。

重叠检查只建立临时 `Vec<(String, String)>`，比较的是十六进制 key 的字符串字典序，没有在本文件解码或规范化 key。正确性因此依赖上游提供可按字典序比较的规范十六进制表示。Bundle 级 `Override` 不参与此算法；只有每条 leader rule 的 `Override` 会清空之前累计的范围。

## 依赖与调用关系

上游直接关系经 RustCodeGraph 确认为：

- `pkg/domain/infosync/info.rs::GlobalInfoSyncerInit` 构造 `PDPlacementManager` 或 `mockPlacementManager`，并保存为 `InfoSyncer::placementManager`。
- `info.rs::{GetRuleBundle, GetAllRuleBundles, PutRuleBundles}` 是面向 crate 使用者的门面；`PutRuleBundlesWithRetry` 和 `PutRuleBundlesWithDefaultRetry` 在门面之上增加错误分类与重试。
- `pkg/domain/infosync/info_test.rs::test_put_bundles_retry` 通过 `RetryClient` 间接覆盖真实管理器的转发、错误传播、尝试次数和读取回写结果。

下游直接关系为：

- `crate::PdHttpClient`（定义在 `types.rs`）提供三个 placement HTTP 方法。其 trait 默认实现返回 unsupported 错误，具体客户端必须覆盖需要的操作。
- `crate::placement` 是 `ddl-placement` crate 的重导出；本文件使用 `Bundle`、`Bundle::IsEmpty`、`pd::Rule` 及 `pd::Leader`。
- `crate::{Error, Result}` 统一错误边界；`CheckBundle` 构造 `Error::External`，PD 客户端错误则不改写。
- 标准库 `Arc` 管理客户端共享所有权，`Mutex` 串行化 mock map，`HashMap` 保存 Bundle。

RustCodeGraph 将目标文件列为由 `info.rs` 和 `info_test.rs` 两个 Rust 文件使用。`lib.rs` 的 `pub use placement_manager::*` 让 `CheckBundle` 等公开符号进入 crate 根命名空间，但当前 Rust 代码没有独立的直接行为测试调用 `CheckBundle`。

## 错误处理与边界

所有接口使用 crate 的 `Result<T>`。PD 三个操作的错误直接传播；`GetRuleBundle` 只有在客户端成功后才补写 ID。空批写入是明确的 no-op，既不验证也不调用客户端。

mock 查询在 Bundle 不存在时返回成功和空 Bundle，而不是 not-found 错误。这与 Go mock 一致，也让“删除后读取并用 `IsEmpty` 判断”成为可用测试模式。mock 的锁使用 `lock().unwrap()`；若持锁线程 panic 导致 mutex poisoned，后续调用会 panic，而不是转换为 `Error`。

`CheckBundle` 的检查范围有限：只检查单个 Bundle 内的 leader 范围，不检查 voter/learner，不跨 Bundle 比较，不验证空区间、非法十六进制、PD 索引优先级或完整的 PD apply-rules 语义。错误文本模拟 PD 的 `ErrBuildRuleList`，但错误分类是本 crate 的 `Error::External`。

需要特别注意 mock 的失败语义：`PutRuleBundles` 先修改 map，再运行 `checkBundles`，且错误时没有回滚。因此调用返回重叠错误后，非法状态仍保留在 mock 中。这是当前 Rust 与 Go 的共同顺序，扩展时不得在未同步评估兼容性的情况下改成事务式回滚。

规则 `Override` 的清空动作与规则输入顺序相关：后出现的 override leader 会抛弃此前累计 leader 范围，但不会影响其后规则；排序只发生在全部扫描完成之后。范围端点相等不报错，因为条件是严格 `<`。

## 并发与资源生命周期

`PlacementManager: Send + Sync` 与 `Arc<dyn PlacementManager>` 允许 `InfoSyncer` 被多线程共享。本文件全部 API 是同步阻塞调用；没有 future、后台任务、通道或显式超时/取消。Rust 接口也没有 Go 版本的 `context.Context` 参数，所以取消与 deadline 必须由 `PdHttpClient` 的具体实现或更上层机制承担，本文件无法逐请求传递上下文。

`PDPlacementManager` 克隆 `Arc` 只增减引用计数；客户端在最后一个 `Arc` 释放时销毁。本文件不关闭网络连接。mock 在整个批处理和全量校验期间持有同一把 mutex，保证写入与检查相对于其他 mock 操作是原子的可见区段，但大 Bundle 集合会延长临界区。读取会在锁内克隆 Bundle；锁在方法返回前释放，返回值与后续写入互不借用。

`GetAllRuleBundles` 的 mock 结果没有排序，并发调用只能依赖集合内容而不能依赖顺序。发生校验错误时锁会正常随 guard 离开作用域而释放，但已写入状态仍保留。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/domain/infosync/placement_manager.go`。Rust 保留了同名接口、生产/模拟两种实现、空批 no-op、读取后补 ID、partial-update=`true`、空 Bundle 删除、leader-only 重叠检查以及 override 清空累计范围等核心顺序。

主要表示差异如下：Go 使用指针 Bundle 切片，mock map 也保存指针；Rust 使用拥有所有权的 `Bundle`，在写入和读取时克隆，避免别名修改。Go 的接口每个方法接收 `context.Context`，Rust 同步 trait 不接收上下文。Go 首次写入会惰性初始化可能为 nil 的 map；Rust `#[derive(Default)]` 直接得到可用空 `HashMap`。Go 为范围定义 `keyRange`，Rust用临时元组。Go 的错误通过 `fmt.Errorf` 构造，Rust映射为 `Error::External`。

测试证据分两层：`pkg/domain/infosync/info_test.rs::test_put_bundles_retry` 是可执行 Rust 测试，覆盖 PD 管理器经门面的重试与读取；`pkg/ddl/placement_policy_test.go::TestCheckBundle` 是 Go 行为矩阵，证明无重叠 leader 成功、表与分区 leader 使用同一范围时失败。对应的 `pkg/ddl/placement_policy_test.rs::test_check_bundle_go_draft` 目前只是保留 Go 调用顺序的测试草稿，主体为注释，没有实际构造 Bundle 或调用 Rust `CheckBundle`，不能作为 Rust 行为已执行的证据。

## 扩展指南

新增 PlacementManager 操作时，应同时修改 `PlacementManager` trait、`PDPlacementManager`、`mockPlacementManager`、`PdHttpClient` 及 `info.rs` 门面，避免只有某一路径可用；随后在独立测试文件（优先 `pkg/domain/infosync/info_test.rs`，不要把测试嵌入生产文件）为 PD 转发与 mock 状态语义各加覆盖。若是 PD API 能力，还要核对 `pkg/domain/infosync/Cargo.toml` 的依赖是否已提供对应类型，但不要在本文件复制外部客户端实现。

扩展校验时，应先明确它是仅供 mock 的近似检查，还是生产写入前必须执行的契约。若要让生产路径也调用校验，需评估与 PD 完整规则算法、跨 Bundle/优先级语义、错误分类及已有调用者的兼容性；不能把当前 `CheckBundle` 当成完整 PD 验证器。若增强 key 比较，应优先复用 placement/PD 已有的规范化类型，而不是继续增加字符串假设。

修改 mock 写入顺序时要特别测试：同 ID 多次出现、删除后重建、校验失败后的持久状态、override leader 位于规则序列不同位置、相邻与重叠端点，以及并发读写。若希望校验失败回滚，这是可观察行为变化，必须同步 Go 版本或明确记录迁移差异。

性能方面，`CheckBundle` 对 `L` 条 leader 规则使用 `O(L log L)` 排序；`checkBundles` 会在每次 mock 写入后重新扫描全部保存 Bundle。当前适合作为测试实现，但若 mock 被用于大规模规则压力测试，应测量临界区长度再决定是否维护增量索引。不要依赖 `GetAllRuleBundles` 的 HashMap 顺序。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标 Rust 文件已索引。
- RustCodeGraph `explore "pkg/domain/infosync/placement_manager.rs symbols callers callees placement rules"`：确认 `InfoSyncer` 持有 `PlacementManager`，本文件三项操作经 `info.rs` 门面调用，并定位 `CheckBundle -> checkBundles -> mock PutRuleBundles` 关系。
- RustCodeGraph `node --file pkg/domain/infosync/placement_manager.rs --offset 1 --limit 400`：完整读取 117 行目标文件，核对全部 trait、struct、impl 和函数；图报告直接使用文件为 `info.rs`、`info_test.rs`。
- RustCodeGraph `node` 读取 `pkg/domain/infosync/info.rs` 的初始化段与 510--579 行：核对管理器选择、全局持有、三项门面和重试边界。
- RustCodeGraph `node` 读取 `pkg/domain/infosync/types.rs` 90--154 行：核对 `PdHttpClient` placement 方法及默认 unsupported 行为。
- RustCodeGraph `node` 读取 `pkg/ddl/placement/bundle.rs` 35--79、440--474 行：核对 Bundle 字段、所有权类型及 `IsEmpty` 精确定义。
- 直接读取 `pkg/domain/infosync/Cargo.toml` 与 `lib.rs`：核对 crate 名称、Go 包元数据、`ddl-placement` 依赖和公开重导出。
- 直接对照 `pkg/domain/infosync/placement_manager.go`、`pkg/domain/infosync/info_test.go::TestPutBundlesRetry`、`pkg/ddl/placement_policy_test.go::TestCheckBundle`：核对移植顺序、错误/删除语义与重叠用例。
- 读取 `pkg/domain/infosync/info_test.rs::test_put_bundles_retry` 和 `pkg/ddl/placement_policy_test.rs::test_check_bundle_go_draft`：确认前者为可执行 Rust 回归，后者尚为注释化草稿；当前没有独立 Rust 测试真实执行 `CheckBundle` 的 Go 用例矩阵。
- 本任务仅新增文档，按计划不运行 Cargo。交付前使用任务指定的命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核以上边界没有被表述为超出当前代码的能力。
