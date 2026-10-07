# `br/pkg/utils/storewatch/watching.rs`

## 文件定位

[`watching.rs`](watching.rs) 是 `astersql-br-pkg-utils-storewatch` library crate 的 store 生命周期差分实现。crate 根 [`lib.rs`](lib.rs) 通过 `#[path = "watching.rs"] pub mod watching` 挂载本文件，并把公开项扁平再导出；[`Cargo.toml`](Cargo.toml) 则把 crate 映射到 Go 包 `br/pkg/utils/storewatch`。它负责比较连续两次取得的 TiKV store 快照并产生“首次出现、掉线、重启”事件，本身不负责定时调度。

当前 Rust 接线需要与设计用途分开理解：根 `Cargo.toml` 已把该 crate 纳入 workspace，但仓库内没有其他 Cargo manifest 声明 `astersql-br-pkg-utils-storewatch` 依赖。RustCodeGraph 能确认本文件内部 `Step -> updateStore/retain` 的边，却没有确认来自 Rust 生产代码的真实调用边；`br/pkg/backup/store.rs` 和 `br/pkg/restore/data/data.rs` 当前使用各自 `stubs.rs` 中的同名 `storewatch`/`StoreWatcher` 适配。因此，本文件当前是独立可测试的移植实现，不能据此声称它已替代备份、恢复主链里的适配实现。

## 核心职责

- 用最小 `Store` 视图承载差分所需的 `Id`、`State` 和 `StartTimestamp`，而不是暴露完整 `metapb::Store`。
- 用 `Callback` 抽象三类通知，并用 `DynCallback`、`WithOn*` 与 `MakeCallback` 提供可选闭包式组装。
- 用 `Watcher::lastStores` 保存上一轮快照；每次 `Step` 拉取当前列表、逐项比较并删除本轮已经消失的 ID。
- 保留 Go `watching.go` 的事件判定顺序：首见只发注册事件；已有 store 的 `Up -> Offline` 发掉线事件；启动时间戳变化独立发重启事件，因此同一更新可以先后触发掉线和重启。
- 在元数据读取失败时添加固定上下文 `failed to update store list` 并停止本轮，不修改 `lastStores`。

本文件不创建线程、不设置轮询周期、不做重试，也不把 Tombstone 或列表消失自动解释成 disconnect。调用方必须主动、串行地推进 `Step`，并决定失败后的重试与调度策略。

## 主要符号

- `StoreState::{Up, Offline, Tombstone}`：本地运行态枚举，显式取值为 `0/1/2`；`Default` 是 `Up`。当前判定逻辑只对 `Up -> Offline` 有特殊处理。
- `Store { Id, State, StartTimestamp }`：可克隆的 store 快照。`GetId`、`GetState` 保留 Go getter 命名，`StartTimestamp` 直接用于重启判断。
- `Callback`：要求实现 `OnNewStoreRegistered(&Store)`、`OnDisconnect(&Store)` 和 `OnReboot(&Store)`。方法接收共享引用，事件处理者不能通过该引用修改 watcher 的快照。
- `DynCallback`：保存三个 `Option<Box<dyn Fn(&Store) + Send + Sync>>`。未配置的事件是空操作；`Send + Sync` 允许闭包捕获可跨线程共享的状态，但不代表 watcher 自身会并发调用它们。
- `DynCallbackOpt = Box<dyn FnOnce(&mut DynCallback)>`：构造期一次性选项。`WithOnNewStoreRegistered`、`WithOnDisconnect`、`WithOnReboot` 分别覆盖对应槽位；同类选项出现多次时，后应用者覆盖前者。
- `MakeCallback(Vec<DynCallbackOpt>) -> DynCallback`：按向量顺序消费选项并得到回调集合。与 Go 返回接口值不同，Rust 返回具体 `DynCallback`，再借助 blanket-free 的显式 `impl Callback for DynCallback` 使用。
- `StoreMeta::GetAllTiKVStores(&self) -> Result<Vec<Store>, String>`：当前 Rust 文件的最小元数据读取边界。它把 PD 查询、过滤及重试留给实现方或上层。
- `Watcher<C, M> { cli, cb, lastStores }`：以泛型静态持有回调和元数据源；`lastStores` 私有，外部只能通过 `Step` 推进状态。
- `New(cli, cb) -> Watcher<C, M>`：创建空基线。首轮成功读取到的每个 store 都被视为新注册。
- `Watcher::Step(&mut self) -> Result<(), String>`：唯一公开推进入口。`&mut self` 在类型层面要求同一个 watcher 的推进不可并发发生。
- `updateStore`、`retain`：私有差分原语；前者先替换快照再依据旧值发事件，后者删除本轮未记录的 ID。

本文件没有模块级常量、条件编译项或异步函数。测试条件编译发生在 crate 根 `lib.rs`，由其分别挂载 `watching_test.rs` 和 `parity_test.rs`。

## 执行流程

1. 调用者通过 `MakeCallback` 组装感兴趣的钩子，再调用 `New`；此时 `lastStores` 为空。
2. `Step` 先调用 `cli.GetAllTiKVStores()`。若返回错误，函数用 `format!` 加前缀后立即返回；因为这一步早于任何插入或删除，旧基线完整保留。
3. 对本轮 `liveStores` 中的每个元素，`Step` 克隆一份传给 `updateStore`，并把其 ID 写入本轮 `recorded: HashSet<u64>`。
4. `updateStore` 用 `HashMap::insert` 原子式替换该 ID 的缓存并取得旧值。无旧值时只调用 `OnNewStoreRegistered`，随后结束该分支，不检查 reboot/disconnect。
5. 有旧值时，先检查 `last.State == Up && new.State == Offline` 并可能调用 `OnDisconnect`；再独立比较两个 `StartTimestamp` 并可能调用 `OnReboot`。两项同时成立时，回调顺序固定为 disconnect 后 reboot。
6. 所有当前元素处理完后，`retain` 仅保留 ID 存在于 `recorded` 的缓存项。空列表会清空整个基线；该 ID 后续再出现时会重新走“首次出现”。
7. 成功完成差分和清理后返回 `Ok(())`。

输入列表若包含重复 ID，会按列表顺序处理多次：第一次插入后，后续同 ID 元素会和本轮前一个元素比较，并可能额外发事件。这不是代码主动去重的场景；元数据提供方应维持“一轮每个 ID 至多一次”的正常 PD 列表约束。

## 数据与状态

`lastStores: HashMap<u64, Store>` 是唯一跨 `Step` 保留的可变状态，键为 store ID，值为最近一次成功轮次中最后处理到的快照。快照是值语义：`Step` 从 `Vec<Store>` 取得所有权后为 `updateStore` 克隆，缓存和回调看到的 `newStore` 在该次调用期间一致，不依赖元数据源后续变动。

`recorded: HashSet<u64>` 只活到单次 `Step` 结束，用于区分“仍在当前列表”与“已经消失”。列表消失仅删除缓存，不产生回调；这使再次出现被当成新注册，而不是依据消失前状态判断重启。`parity_test.rs::go_rust_public_contract_matches` 明确覆盖了“清空列表，再加入相同 ID，得到第二次 `new:1`”这一不变量。

`StoreState::Tombstone` 没有专门事件；它仍会被写入快照，并且时间戳变化仍可触发 reboot。状态从 `Offline` 到 `Offline`、`Offline` 到 `Up` 或 `Up` 到 `Tombstone` 都不会触发 disconnect。重启判断只比较时间戳，不要求新状态为 `Up`。

## 依赖与调用关系

本文件的标准库依赖只有 `HashMap` 与 `HashSet`。`Cargo.toml` 没有列出外部依赖，说明 `Store`、状态枚举和错误字符串都是 crate 内的轻量抽象。crate 根 `lib.rs` 对外再导出 `watching::*`，所以消费者理论上可从 crate 根取得全部公开符号。

RustCodeGraph 对本文件确认的局部调用关系为：

- `Watcher::Step -> StoreMeta::GetAllTiKVStores`；
- `Watcher::Step -> Watcher::updateStore -> Store::{GetId, GetState}`；
- `Watcher::updateStore -> Callback::{OnNewStoreRegistered, OnDisconnect, OnReboot}`；
- `Watcher::Step -> Watcher::retain`。

RustCodeGraph 的宽泛 `Step` 查询曾把 `lightning/pkg/importinto/job_progress.rs::isGlobalSortStatus` 列为调用者，但读取该函数可见它只匹配导入任务的 phase/step 字符串，并不引用 storewatch；这是同名图节点消歧不足产生的误配，不能作为本文件上游证据。仓库级 `rg` 也未找到 crate 名或本文件公开 API 被其他 Rust crate 引用。相对地，Go 生产调用者可在 `br/pkg/backup/store.go::ObserveStoreChangesAsync` 和 `br/pkg/restore/data/data.go` 的 watcher 创建逻辑中找到；Rust 对应文件目前各自走 stub 适配，属于后续统一接线的候选位置。

## 错误处理与边界

`StoreMeta` 将所有失败压平为 `String`，`Step` 只增加 `failed to update store list: ` 前缀，不分类、不记录来源链，也不自行重试。`parity_test.rs::step_annotates_store_list_errors` 固定了完整错误文本。错误发生在状态更新前，因此失败轮不会部分触发回调，也不会因空/不完整结果清理缓存。

回调返回 `()`，无法向 `Step` 报错；闭包若 panic，会展开出 `Step`，且此前已经执行的缓存插入和回调不会回滚。互斥锁中毒之类的行为属于回调自身责任。空回调配置完全合法，所有事件静默忽略。

正常的空列表不是错误：它成功清空缓存。列表中重复 ID、ID 为 0、时间戳回退、`Tombstone` 后恢复等输入均未被拒绝，处理结果完全由当前差分规则决定。这里也没有 Go `context.Context` 对应物，不能在 `Step` 内表达取消或截止时间。

## 并发与资源生命周期

`Watcher::Step` 需要 `&mut self`，所以安全 Rust 中同一实例不能被多个线程同时推进；本文件也没有给 `Watcher` 增加内部锁。是否把 watcher 放入 `Mutex`、何时创建后台线程、多久调用一次以及何时停止，都由调用者控制。

`DynCallback` 的闭包要求 `Send + Sync + 'static`，便于拥有闭包并让捕获值跨线程，但每个事件仍在调用 `Step` 的线程上同步执行。慢回调会延长整个轮次；回调若再次尝试锁住调用者已经持有的同一资源，可能死锁。三个回调槽随 `DynCallback`/`Watcher` 一起释放；`DynCallbackOpt` 在 `MakeCallback` 中被依次消费，不会留存。

没有文件句柄、网络连接、异步任务或显式关闭协议。`StoreMeta` 的实际资源生命周期由其具体实现决定；`Watcher` 只按值拥有它，并在自身 drop 时一并释放。缓存大小上界通常等于最近成功列表中的不同 store ID 数量，`retain` 每轮遍历缓存清理消失项。

## 与 Go 版本的对应关系

Rust 的 `Callback`、`DynCallback`、三个 `WithOn*`、`MakeCallback`、`Watcher`、`New`、`Step`、`updateStore` 和 `retain` 均能在 [`watching.go`](watching.go) 找到直接对应物。核心事件规则、首见提前结束、disconnect 先于 reboot、消失项清理以及错误前缀均保持一致；`watching_test.rs` 逐项复刻 Go 的注册、掉线、重启三项测试，`parity_test.rs` 另外覆盖 retain、双事件顺序和错误注解。

仍存在明确的移植边界差异：

- Go 使用 `metapb.Store` 与 `metapb.StoreState`；Rust 使用本地裁剪结构和枚举，尚未证明能无损承载完整 PD 元数据。
- Go `Step(ctx)` 调用 `conn.GetAllTiKVStoresWithRetry(ctx, cli, util.SkipTiFlash)`，包含上下文、重试和跳过 TiFlash 的策略；Rust `Step()` 只调用抽象的 `GetAllTiKVStores()`。除非具体 `StoreMeta` 实现承担这些语义，否则两者并不等价。
- Go `MakeCallback(opts ...DynCallbackOpt) Callback` 返回接口值，Rust 接受 `Vec` 并返回具体类型；Rust 闭包还额外要求 `Send + Sync + 'static`。
- Go `New` 返回指针，Rust 返回拥有所有字段的值，并以 `&mut self` 推进。
- Go 生产代码已有备份、恢复调用点；该 Rust crate 当前没有跨 crate 的生产依赖，不能把 Go 调用关系直接当作 Rust 已接线事实。

## 扩展指南

新增事件类型时，应同步修改 `Callback`、`DynCallback` 字段与转发方法、对应 `WithOn*` 构造器、`MakeCallback` 初始化以及 `updateStore` 的判定；同时在独立的 `watching_test.rs` 或 `parity_test.rs` 增加顺序和互斥/共现断言，不要把测试内嵌进生产文件。若事件可与现有事件在同一更新中同时发生，必须明确并测试调用顺序。

若要接入真实生产主链，最关键的不是简单替换同名 stub，而是提供与 Go `GetAllTiKVStoresWithRetry(..., SkipTiFlash)` 等价的 `StoreMeta` 适配，补回取消、重试、TiFlash 过滤和错误链语义；然后在消费者 Cargo manifest 中统一声明本 crate 依赖，并迁移 `br/pkg/backup/store.rs`、`br/pkg/restore/data/data.rs` 的调用。该工作会影响线程停止、错误策略和 store 类型转换，需用调用方的独立测试验证，不能在本文件里做简化接线。

修改差分规则时重点保护这些兼容约束：首轮全为 new；只有 `Up -> Offline` 为 disconnect；时间戳变化独立为 reboot；同轮双事件顺序为 disconnect 后 reboot；读取失败不改变基线；列表消失后再次出现为 new。若要处理重复 ID 或 Tombstone，需先决定与 Go 的兼容性和是否会改变通知风暴/缓存语义。

性能上，单轮时间复杂度约为 `O(L + C)`，其中 `L` 是输入列表长度，`C` 是旧缓存长度；每个输入 store 当前至少克隆一次。若 store 结构扩充为完整 protobuf，克隆成本可能显著增加，应考虑以所有权移动或引用/共享表示优化，同时保持回调生命周期安全。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`node --file br/pkg/utils/storewatch/watching.rs` 完整读取了 185 行源码。
- RustCodeGraph `explore "br/pkg/utils/storewatch/watching.rs Watcher::Step"` 与精确文件查询：确认 `Step -> GetAllTiKVStores/updateStore/retain`、`updateStore -> GetId/GetState/三类回调` 的文件内边；对宽泛查询给出的 `isGlobalSortStatus` 上游又读取 `lightning/pkg/importinto/job_progress.rs` 复核，确认其为同名误配。
- crate 与接线：读取 `br/pkg/utils/storewatch/Cargo.toml`、`br/pkg/utils/storewatch/lib.rs` 和根 `Cargo.toml`；用仓库搜索检查 `astersql-br-pkg-utils-storewatch`、`storewatch::`、`Watcher` 等引用，并读取 `br/pkg/backup/store.rs`、`br/pkg/restore/data/data.rs` 的相邻实现。
- Go 对照：读取 `br/pkg/utils/storewatch/watching.go`、`br/pkg/backup/store.go`、`br/pkg/restore/data/data.go`，区分算法对应关系与实际生产接线。
- 测试证据：读取 `br/pkg/utils/storewatch/watching_test.rs`、`br/pkg/utils/storewatch/parity_test.rs` 和 `br/pkg/utils/storewatch/watching_test.go`；覆盖注册、掉线、重启、同轮双事件顺序、读取错误与 retain 后重新注册。
- 本任务只新增说明文档，按任务约束不运行 Cargo。交付检查以固定 11 章节结构、引用路径存在、diff/空白检查和人工事实复核为准。
