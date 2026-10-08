# `pkg/store/mockstore/unistore/tikv/dbreader/db_reader.rs`

源文件：[db_reader.rs](./db_reader.rs)

## 文件定位

本文件实现 UniStore 模拟 TiKV 存储中的 MVCC 只读适配层。它把具体存储引擎需要提供的点查、批量查和迭代能力抽象成 `ReadTxn`、`DBIterator`、`DBItem` 三个 trait，再由 `DBReader` 统一实现按时间戳读取、正反向范围扫描、MVCC 版本检查和资源关闭。对应 crate 是 `astersql-store-mockstore-unistore-tikv-dbreader`，入口 `lib.rs` 公开再导出本模块；根 workspace 还通过 `pkg/lib.rs` 的 `store::mockstore::unistore::tikv::dbreader` facade 再导出该 crate。

当前 Rust 接线需要与 Go 运行时区分：仓库搜索只发现 `migration_aster_unit_test.rs` 直接构造和调用 Rust `DBReader`；`cophandler/Cargo.toml` 虽声明了可选依赖，但其 Rust 源码没有消费本文件符号。完整请求主链的直接证据目前来自同路径 Go 代码：`tikv/server.go::requestCtx.getDBReader` 构造 reader，`tikv/mvcc.go` 和 `cophandler/*.go` 消费它。因此，本文件是已实现并有独立测试的迁移组件，而不是已验证接入 Rust UniStore 请求主链的组件。

## 核心职责

- 用 `DBItem`、`DBIterator`、`ReadTxn` 隔离底层引擎；生产逻辑不直接依赖 fjall API。fjall 只出现在 crate 依赖和独立迁移测试的适配器中。
- 维护 Region/扫描边界 `StartKey`、`EndKey`，惰性创建正向、反向和额外键空间三个迭代器。
- 在普通快照读与 `RcCheckTS` 两种模式间切换读时间戳：普通读使用调用者给定的 `startTS`，`RcCheckTS` 先读取最新版本，再比较 `commit_ts` 判断写冲突。
- 提供点查 `Get`、批量点查 `BatchGet`、正向 `Scan`、反向 `ReverseScan`、全版本 MVCC 信息 `GetMvccInfoByKey` 和按 `start_ts` 反查键 `GetKeyByStartTs`。
- 定义 `ExtraDbReaderProvider`，为 Go 侧 IndexLookUp 跨 Region 获取额外 reader 的协议保留 Rust 对应接口；本仓库尚未找到 Rust 实现者或调用者。
- 显式关闭迭代器并丢弃只读事务，避免将底层资源生命周期交给调用者猜测。

## 主要符号

- `DbReaderError::{Backend, Conflict, ScanBreak}`：分别表示适配器/引擎错误、`RcCheckTS` 写冲突和扫描处理器请求正常提前结束。`Backend` 用 `Arc<anyhow::Error>` 保持错误可克隆；其 `PartialEq` 以文本比较两个后端错误。
- `IteratorOptions { Reverse, StartKey, EndKey }`：传给 `ReadTxn::NewIterator` 的引擎无关范围配置。
- `DBItem`：暴露 `Key`、`Value`、`UserMeta`、`Version`、`IsEmpty`；默认 `KeyCopy`/`ValueCopy` 负责生成所有权数据。
- `DBIterator`：支持配置全版本、设置读时间戳、`Seek`/`Valid`/`Next`/`Item` 和 `Close`。
- `ReadTxn`：支持设置读时间戳、`Get`、顺序对齐输入的 `MultiGet`、创建迭代器和 `Discard`。
- `LocateExtraRegionResult`、`GetExtraDBReaderContext`、`ExtraDbReaderProvider`：描述本地 Region 定位和跨 Region reader 获取接口。Rust 用 `&dyn Any` 代替 Go 的 `context.Context`，且返回值用 `Option` 表达 Go 指针可空性。
- `NewDBReader`：初始化边界和事务，三个迭代器均为空，`RcCheckTS` 关闭，额外 reader provider 为空。
- `NewIterator`：只组装 `IteratorOptions` 并委托给 `ReadTxn::NewIterator`；具体键编码和引擎选项由适配器负责。
- `DBReader`：核心有状态对象，持有唯一的 `Box<dyn ReadTxn>`、三个可复用迭代器以及隔离级别和跨 Region 扩展点。
- `BatchGetFunc`：逐输入键回调 `(key, optional value, userMeta, optional error)`；回调引用只在调用期间有效。
- `ScanProcessor`：`Process` 消费一行并可返回 `ScanBreak`，`SkipValue` 允许只读键和提交时间戳，避免加载值。
- `ErrScanBreak`：构造扫描提前结束错误；`exceedEndKey` 实现空上界或半开区间的比较规则。

## 执行流程

1. 调用者用 `NewDBReader(start, end, txn)` 创建 reader。迭代器按需创建，因此单纯点查不会分配扫描游标。
2. `Get` 先通过 `setReadTS` 同步事务和所有已创建迭代器。普通模式设置为 `startTS`；`RcCheckTS` 设置为 `u64::MAX` 以取得最新版本，再由 `CheckWriteItemForRcCheckTSRead` 检查该版本是否晚于读取时间戳。键不存在返回 `Ok(None)`，存在则返回值与 `DBUserMeta`。
3. `BatchGet` 以同一规则设置读时间戳并调用 `ReadTxn::MultiGet`。整体失败时，每个输入键都收到同一错误；成功时按输入下标回调。实现有意保留 Go 的循环外 `err` 语义：一个 item 的 `Value` 失败后，紧随其后的缺失 item 会继续收到该错误，直到后续非空 item 重写错误状态。独立测试 `batch_get_carries_value_error_to_following_missing_item` 固定了这一兼容行为。
4. `Scan` 设置读时间戳，向正向迭代器 `Seek(startKey)`，在迭代器失效、键达到 `endKey`、处理器返回 `ScanBreak` 或已处理数达到 `limit` 时停止。每个非空 item 先做 `RcCheckTS` 校验；`SkipValue` 为真时向处理器传空值，但仍传 `item.Version()`。
5. `ReverseScan` 向反向迭代器 `Seek(endKey)`，首项恰好等于 `endKey` 时跳过，从而保持 `[startKey, endKey)`；到达小于 `startKey` 的键即停止。其余校验、空项、`SkipValue`、`ScanBreak` 和 limit 规则与正向扫描一致。
6. `GetMvccInfoByKey` 把正向迭代器切到全版本和最大读时间戳，从目标键开始遍历，直到键变化；空值映射为 `Op::Del`，非空值映射为 `Op::Put`，并把 `start_ts`、`commit_ts` 和短值追加到 `MvccInfo.writes`。
7. `GetKeyByStartTs` 同样启用全版本和最大读时间戳，在 `[startKey, endKey)` 找到第一个 `DBUserMeta::StartTS()` 匹配项并复制键返回。
8. 调用者结束使用后调用 `Close`，依次关闭已经创建的正向、反向、额外迭代器，再调用事务 `Discard`。

## 数据与状态

`DBReader` 的主要可变状态是底层只读事务和三个缓存迭代器。`setReadTS` 同时更新事务及所有已创建迭代器，保证 reader 从普通读切换到 `RcCheckTS`（或反向切换）时，旧游标不会继续使用过期时间戳。迭代器一旦通过 `GetIter`、`GetExtraIter` 或 `getReverseIter` 创建，就在后续操作中复用；每次具体扫描仍会 `Seek` 重新定位。

`DBUserMeta` 的字节布局由相邻 `mvcc` crate 解释，本文件只读取 `StartTS()`、`CommitTS()`。`DBItem::Version()` 作为扫描回调中的 `commitTS` 传递，而冲突判断明确读取 user meta 中的 `CommitTS()`；适配器必须让两者符合各自契约。

`GetExtraIter` 克隆 reader 的范围边界并把非空边界的首字节做 wrapping 加一，再创建正向迭代器。它对应 Go 中首字节 `++` 的额外键空间映射；空边界保持为空。该变换只作用于副本，不修改 `DBReader::StartKey/EndKey`。

扫描计数只在非空 item 成功交给处理器后增加。由 Go 对齐测试可见，`limit` 为负数时第一条成功处理后 `count >= limit` 即成立，因此仍会返回一条；这不是“无限制”的哨兵。删除占位（`IsEmpty`）不计数，也不交给处理器。

## 依赖与调用关系

上游 Rust 证据：

- `pkg/store/mockstore/unistore/tikv/dbreader/lib.rs` 声明 `pub mod db_reader; pub use db_reader::*;`，并在 `cfg(test)` 下挂载独立测试。
- 根 `Cargo.toml` 以 `facade_store_mockstore_unistore_tikv_dbreader` 引入 crate，`pkg/lib.rs` 再导出其公共符号。
- `pkg/store/mockstore/unistore/tikv/Cargo.toml` 声明该 crate 依赖；`pkg/store/mockstore/unistore/cophandler/Cargo.toml` 声明可选依赖。仓库 Rust 源码搜索未发现测试之外对 `NewDBReader` 或这些方法的直接消费，因此不能据此断言 Rust 请求链已经接线。

下游依赖：

- `anyhow`：保存适配器后端错误。
- `kvproto` 经 `lib.rs` 重导出的 `errorpb`、`kvrpcpb`、`metapb`：协议错误、MVCC 消息、Region/Peer 和 KeyRange 类型。
- 相邻 `kverrors` crate：构造 `ErrConflict`。
- 相邻 `mvcc` crate：解析 `DBUserMeta`。
- `fjall` 不是 `db_reader.rs` 的直接 API 依赖；它由 crate 暴露，并在 `migration_aster_unit_test.rs` 中实现 `ReadTxn`/`DBIterator`/`DBItem` 测试适配器。

Go 主链对照证据：`tikv/server.go::requestCtx.getDBReader` 创建 reader、设置 `RcCheckTS` 和 `ExtraDbReaderProvider`；`tikv/mvcc.go` 调用点查、批量查、扫描、事务访问和 MVCC 辅助方法；`cophandler/closure_exec.go`、`mpp_exec.go` 等通过扫描接口执行 DAG/MPP 表扫描。这些是 Go 实现的调用边，不代表同名 Rust 调用边。

## 错误处理与边界

- 所有适配器失败统一为 `DbReaderError::Backend`；`Seek`、`Item`、取值和创建迭代器错误向上传播。迭代器声称 `Valid` 却不给 `Item` 时转为明确的后端错误。
- `RcCheckTS` 仅在开关启用且 item 存在时检查。若 `commit_ts > readTS`，返回 `ErrConflict`，其中 `StartTS=readTS`、`ConflictTS=item.start_ts`、`ConflictCommitTS=item.commit_ts`、`Reason=RcCheckTs`；Rust 当前与 Go 一样不在此处填冲突键。
- `ScanBreak` 是控制流，不作为 `Scan`/`ReverseScan` 的失败返回；其他处理器错误原样返回。
- 空 `endKey` 表示无上界；非空范围均按 `[startKey, endKey)`。反向扫描必须显式跳过等于排他上界的首项。
- `GetMvccInfoByKey` 和 `GetKeyByStartTs` 会把复用的正向迭代器设成 `all_versions=true`，本文件没有把它恢复为 `false`。后续复用同一迭代器时，是否仍返回单版本取决于适配器对该状态的实现；扩展或重排调用顺序时必须验证这一状态影响。
- `BatchGet` 假设 `MultiGet` 返回项数与输入键数一致；它按返回项枚举后索引 `keys[index]`。过长返回会 panic，过短返回则不会为尾部输入键回调。实现新适配器时必须保持一一对齐契约。
- `GetExtraIter` 的首字节 `wrapping_add(1)` 在 `0xff` 时回绕到 `0x00`，与 Rust 当前实现一致；调用者必须确保传入的是预期键前缀空间。

## 并发与资源生命周期

`DBItem`、`DBIterator`、`ReadTxn` 要求 `Send`，允许其所有权跨线程转移；但 `DBReader` 的操作普遍需要 `&mut self`，内部没有锁或共享可变状态，不支持多个调用者并发操作同一个 reader。`ExtraDbReaderProvider` 本身未声明 `Send`，也进一步表明本类型没有承诺可跨线程共享。

事务由 `DBReader` 独占持有。三个迭代器惰性创建并复用，`Close` 负责关闭已创建迭代器后 `Discard` 事务。类型没有实现 Rust `Drop` 自动调用 `Close`，因此提前返回路径的上层代码仍需显式关闭；测试适配器的 `Close`/`Discard` 是空操作，不能证明真实引擎资源会自动回收。

`DBItem` 以 `Box<dyn DBItem>` 从事务或迭代器移交所有权；扫描处理器只获得借用，并由 `ScanFunc`/`ScanProcessor` 注释明确约束不得持久保存 key/value 引用。`Backend` 错误内部使用 `Arc` 只是为了克隆错误值，不代表 reader 的事务或迭代器可共享。

## 与 Go 版本的对应关系

Rust 文件逐项保留了 Go `db_reader.go` 的公开概念和主要控制流：`DBReader` 字段、三个迭代器、`RcCheckTS`、额外 reader provider、点查/批查/扫描/MVCC 辅助方法以及关闭顺序均有对应实现。

主要适配差异如下：

- Go 直接绑定 `*badger.Txn`、`*badger.Iterator`、`*badger.Item`；Rust 用三个 trait 解耦后端，并在独立测试中用 fjall 实现这些 trait。
- Go 的 `NewIterator` 负责用 `y.KeyWithTs(..., MaxUint64)` 编码范围；Rust 只传原始边界给 `ReadTxn::NewIterator`，版本键编码责任已下沉到适配器。
- Go 用 `nil` 和多返回值表达缺失/错误；Rust 用 `Option` 与 `Result`。Go 的全局 `ErrScanBreak` 对应 Rust 枚举变体及 `ErrScanBreak()` 构造函数。
- Rust `setReadTS` 不只更新事务，也同步所有已创建迭代器；这是 trait 化之后维持读时间戳一致性的必要局部接线。
- Rust `GetMvccInfoByKey` 和 `GetKeyByStartTs` 显式把迭代器读时间戳设为 `u64::MAX`；Go Badger 的全版本迭代行为由其事务/迭代器实现承担。
- Rust `BatchGet` 明确注释并由测试锁定 Go 的跨缺失项错误沿用行为，未将其“修正”为每轮清空。
- Go 运行时已有 server、MVCC、cophandler 调用者和 `requestCtx` 的 provider 实现；Rust 目前只有 crate/facade 暴露及独立迁移测试，尚无等价运行时调用证据。

## 扩展指南

- 新增存储后端时，在独立适配器/测试文件中实现 `DBItem`、`DBIterator`、`ReadTxn`，重点保证 `MultiGet` 与输入严格等长同序、范围是半开区间、版本可见性服从 `SetReadTS`、`SetAllVersions` 能遍历同键所有版本，且 `Close`/`Discard` 真实释放资源。不要把测试实现内嵌进本生产文件。
- 新增读取 API 时优先复用 `setReadTS` 和 `CheckWriteItemForRcCheckTSRead`，并同时考虑普通时间戳读与 `RcCheckTS` 最新版本检查；新增扫描路径还应复用半开区间、空项、`SkipValue`、`ScanBreak` 和 limit 语义。
- 修改迭代器复用逻辑时，要处理 `all_versions` 状态是否需要恢复，并覆盖“先调用全版本 API、再普通 Scan”的顺序测试。
- 修改 `BatchGet` 时不得无意改变 Go 的错误沿用兼容行为；若上游 Go 行为变化，应同时更新同路径 Go 对照分析和 `migration_aster_unit_test.rs` 的回归断言。
- 接入 Rust UniStore 请求主链时，应在 Rust server/request context 中实现 `ReadTxn` 和 `ExtraDbReaderProvider`，并让 cophandler 消费 `DBReader`；在有实际调用边前，文档和代码注释都应保持“接口已实现、运行时未验证接线”的表述。
- 涉及键空间变换时验证空边界、首字节 `0xff` 回绕和 Region 边界；涉及冲突错误时验证所有时间戳字段与 `RcCheckTs` reason。
- 测试应继续放在同目录独立文件 `migration_aster_unit_test.rs`，遵守生产 Rust 与测试逻辑分离要求。建议补充：迭代器创建/Seek/Value 失败传播、`MultiGet` 长度违约、全版本状态复用、`Close` 调用顺序、额外键空间边界及 provider 接线测试。

## 验证依据

- RustCodeGraph 状态：项目索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/store/mockstore/unistore/tikv/dbreader` 列出 `db_reader.rs`、`db_reader.go`、`lib.rs`、`migration_aster_unit_test.rs`。
- RustCodeGraph 源码节点：`node --file .../db_reader.rs --offset 1 --limit 500` 与 `--offset 500 --limit 180` 覆盖目标文件 604 行；符号查询确认 Rust `DBReader` 在第 214 行、`NewDBReader` 在第 183 行。对精确限定符执行 callers/callees 未返回方法级调用边，因此又用仓库搜索核对直接使用点，并在本文明确标记该限制。
- 源文件：`pkg/store/mockstore/unistore/tikv/dbreader/db_reader.rs`，核对了全部 trait、结构、函数、impl 和错误分支；文件没有条件编译项。
- crate/装配：`pkg/store/mockstore/unistore/tikv/dbreader/Cargo.toml`、同目录 `lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`、`pkg/store/mockstore/unistore/tikv/Cargo.toml`、`pkg/store/mockstore/unistore/cophandler/Cargo.toml`。
- Go 对照与主链：同目录 `db_reader.go`，以及搜索得到的 `pkg/store/mockstore/unistore/tikv/server.go`、`mvcc.go`、`pkg/store/mockstore/unistore/cophandler/closure_exec.go`、`mpp_exec.go`、`cop_handler.go`。
- 独立 Rust 测试：`migration_aster_unit_test.rs` 使用真实临时 fjall 数据库覆盖普通时间戳可见性、`RcCheckTS` 冲突、BatchGet 顺序/缺失项/错误沿用、正反向半开区间、`SkipValue`、`ScanBreak`、limit、全版本 MVCC 信息和按 `start_ts` 查键。
- Go 测试/调用证据：`tikv/mvcc_test.go` 包含不同 read timestamp 下的 `Get` 场景；Rust 测试已针对本文件抽象提供更聚焦的行为断言。本文未运行 Cargo，符合该纯文档任务的明确限制。
