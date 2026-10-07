# `br/pkg/rtree/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-rtree` 的 crate root。`br/pkg/rtree/Cargo.toml` 以 `[lib] path = "lib.rs"` 指向本文件，并通过 `[package.metadata.porting] go-package = "br/pkg/rtree"` 标明其 Go 对照包。它不是区间算法的实现文件，而是把同目录 Rust 模块装配成一个可供其他 crate 使用的库门面。

本文件只有 58 行，没有函数、类型、运行期初始化或可变全局状态。实际区间树与进度树算法位于 `rtree.rs`，日志格式辅助位于 `logging.rs`，移植所需的备份元数据、编码和写出接口替身位于 `stubs.rs`。当前直接生产级 Rust 消费者是 `br/pkg/restore/utils`：其 `Cargo.toml` 通过路径 `../../rtree` 依赖本 crate，`merge.rs` 与 `rewrite_rule.rs` 从 crate 根导入 `Range`、`RangeStats`、`NewRangeStatsTree`、`KeyRange` 等符号。

## 核心职责

本文件承担四项编译期边界职责：

1. 通过显式 `#[path] pub mod` 装入 `stubs.rs`、`logging.rs` 和 `rtree.rs`。
2. 通过 `pub use logging::*` 与 `pub use rtree::*` 把日志和区间树的公开符号提升到 crate 根，模拟 Go 同包文件共享的扁平命名空间。
3. 从 `stubs` 选择性重导出 `File`、`MetaWriter`、`RpcKeyRange`、`ChecksumStats`、table key 编码函数等 rtree 公共契约所需符号；`FreeListG` 和 `EncodeIntToCmpUint` 等内部兼容项仍可经公开模块 `stubs` 访问，但没有被提升到根路径。
4. 仅在 `cfg(test)` 下挂载 `parity_test.rs`、`logging_test.rs`、`merge_fuzz_test.rs` 和 `rtree_test.rs`，使测试逻辑与生产源文件分离且不进入普通库构建。

crate 级 `#![allow(...)]` 放宽死代码、Go 风格命名和未使用项告警，这是迁移期兼容设置。它不证明所有公开 API 都已接入真实业务链，也不会改变子模块行为。

## 主要符号

本文件不定义业务函数或结构体，主要符号是模块与重导出边界。经门面公开的代表性 API 包括：

- `rtree::KeyRange`、`Range`：表达空结束键可代表正无穷的半开键区间，以及该区间对应的备份 `File` 集合。
- `rtree::RangeStatsTree`、`NewRangeStatsTree`、`NeedsMerge`：按起始键排序区间，并依据体积、键数、table ID、record/index 类型及 index ID 决定是否合并。
- `rtree::RangeTree`、`NewRangeTree`、`NewRangeTreeWithFreeListG`：维护已完成、应互不重叠的备份区间，覆盖写入并计算未覆盖空洞。
- `rtree::ProgressRangeTree`、`NewProgressRangeTree`、`ProgressRange`：登记原始请求范围，聚合已完成子范围，完成后写文件元数据、触发回调并按物理 ID 汇总 checksum。
- `logging::ZapRanges`：将 `KeyRange` 集合编码为名为 `ranges` 的缩写日志字段；`KeyRange` 的 `Display` 实现用十六进制展示键。
- 选择性 stub 重导出：`File`、`ChecksumStats`、`MetaWriter`、`RpcKeyRange`、`SummaryFiles`、`AppendDataFile`，以及 `DecodeKeyHead`、`EncodeRowKeyPrefix`、`EncodeIndexKeyPrefix` 等 table key 辅助函数。

通配重导出使调用者可以写 `astersql_br_pkg_rtree::Range`，不必引用 `rtree::Range`。未来子模块新增公开名称会自动扩大 crate 根 API，并可能与另一模块的同名导出冲突。

## 执行流程

`lib.rs` 只有编译期装配流程，没有运行期控制流：

1. Cargo 读取 `Cargo.toml`，以本文件为库入口，并解析直接依赖 `astersql-br-pkg-logutil`。
2. 编译器应用 crate 级 allow 列表，随后按显式路径加载 `stubs`、`logging`、`rtree`。声明顺序不代表业务调用顺序。
3. `pub use` 建立扁平公开 API。导入 crate 本身不会构造树、写备份元数据或启动任务。
4. 实际调用者显式构造并驱动对象。例如 `br/pkg/restore/utils/merge.rs` 用 `NewRangeStatsTree` 收集待恢复范围，再由 `MergedRanges` 生成按阈值聚合的有序范围；`rewrite_rule.rs` 使用 `Range` 表达重写规则涉及的键范围。
5. 在备份侧完整概念链中，调用者可向 `RangeTree::Put`/`Update` 写入完成区间，再用 `GetIncompleteRange` 计算重试空洞；`ProgressRangeTree::GetIncompleteRanges` 会识别完成项，调用 `MetaWriter::Send`、完成回调并更新 checksum。这些行为都在 `rtree.rs` 中发生，不是 crate root 的隐式副作用。
6. 测试构建额外编译四个独立测试模块；普通依赖构建排除它们。

## 数据与状态

本文件自身只保存静态编译信息：模块路径、可见性、重导出集合、lint 策略和测试条件。`pub use` 只改变名称解析，不复制数据，也不创建单例。

门面公开的状态所有者位于子模块：`RangeStatsTree` 和 `RangeTree` 使用 `BTreeMap<Vec<u8>, ...>` 按 `StartKey` 排序；`RangeTree` 还保存 `PhysicalID`；`ProgressRangeTree` 保存进度范围树、按物理 ID 聚合的 checksum、`skipChecksum`、可选 `MetaWriter` 和完成回调。`File`、`RpcKeyRange`、`ChecksumStats` 等位于 `stubs.rs`，是当前 Rust 移植边界使用的最小数据形状，不等同于完整 kvproto/metautil 类型实现。

核心不变量来自 `rtree.rs`：键区间按左闭右开解释，空 `EndKey` 表示无穷上界；正常 `RangeTree` 内容应互不重叠；`ProgressRangeTree` 的原始范围不允许重叠；checksum 的 CRC 用异或累计，键数和字节数使用与 Go 整数行为一致的回绕加法。

## 依赖与调用关系

向下依赖分为两层：

- 模块层：`logging.rs` 依赖 `rtree::KeyRange`；`rtree.rs` 依赖 `stubs` 的 `File`、`RpcKeyRange`、`ChecksumStats`、`MetaWriter`、`FreeListG` 和 table key 编解码辅助。
- Cargo 层：`Cargo.toml` 只声明路径依赖 `astersql-br-pkg-logutil`，供 `logging::ZapRanges` 使用 `AbbreviatedStringers` 与 `Field`。区间容器使用标准库 `BTreeMap`，没有外部 btree 依赖。

反向依赖由 manifest 和源码搜索确认：`br/pkg/restore/utils/Cargo.toml` 是唯一声明 `astersql-br-pkg-rtree` 的 Cargo manifest；生产文件 `merge.rs` 导入 `File as RtreeFile`、`KeyRange`、`NewRangeStatsTree`、`Range`、`RangeStats`，`rewrite_rule.rs` 导入 `Range`。同 crate 的 `rewrite_rule_test.rs` 和 `parity_test.rs` 也从根路径导入这些公开类型。

RustCodeGraph 对目标文件的文件节点只报告模块装配关系，没有可执行函数可查询 callers/callees；对 `NewRangeTree` 和 `NewProgressRangeTree` 的精确查询同时定位到 `rtree.rs` 与对应 `rtree.go`。具体行为调用关系因此由已索引实现、Cargo 反向依赖和直接源码引用交叉验证，而不是把 crate root 当作运行期调用节点。

## 错误处理与边界

本文件没有 `Result`、错误分支或恢复逻辑。模块文件缺失、未满足依赖、无效重导出和测试模块编译失败会成为编译期错误。

运行期错误由原模块原样暴露：`ProgressRangeTree::Insert` 拒绝与既有原始范围重叠的项；`FindContained` 在命中项不能完整包含请求范围时返回错误；`GetIncompleteRanges` 传播 `MetaWriter::Send` 失败，并在出错时停止完成项删除与 checksum 更新。`NeedsMerge` 遇到无法解码的 table key 时保守拒绝合并。crate root 不捕获、包装或降级这些结果。

需要保持的边界包括：空 `EndKey` 是正无穷语义而不是普通空值；`RangeTree::Put` 强制替换重叠范围，而 `PutForce(..., false)` 会拒绝重叠；直接 `InsertRange` 只按相同 `StartKey` 替换，不主动清理其他重叠项，调用者必须维持不变量；`stubs` 只覆盖本 crate 所需契约，不能据此推断真实 protobuf、持久化或 RPC 行为已完整实现。

## 并发与资源生命周期

`lib.rs` 自身不启动线程、异步任务、通道、网络连接或事务，也不持有锁和关闭钩子。模块声明与再导出都在编译期生效。

当前 `RangeTree`、`RangeStatsTree` 和 `ProgressRangeTree` 的变更方法普遍要求 `&mut self`，由 Rust 借用规则保证单次可变访问；内部 `BTreeMap` 不提供隐式并发同步。`MetaWriter` trait 要求 `Send`，完成回调类型为 `Box<dyn Fn() + Send>`，但 `ProgressRangeTree` 本身没有后台执行器，回调和 `Send` 都在调用 `GetIncompleteRanges` 的线程同步发生。该方法先收集待删除键，再分阶段移除完成项，避免遍历期间修改树。

`MetaWriter` 的实际资源所有权、持久化和关闭协议由注入实现负责；本 crate root 不管理它。扩展时不应在门面中增加隐藏的全局锁或后台任务，应把取消、同步和释放责任放进具体实现并用独立测试验证。

## 与 Go 版本的对应关系

Go 目录没有对应的 `lib.go`：`logging.go` 与 `rtree.go` 通过同一个 `package rtree` 自然共享扁平命名空间。Rust 必须显式建立 crate root，所以本文件以模块声明和重导出模拟 Go 包级 API：

- `logging.rs` 对应 `logging.go`，`logging_test.rs` 对应 `logging_test.go`。
- `rtree.rs` 对应 `rtree.go`，`rtree_test.rs` 对应 `rtree_test.go`。
- `merge_fuzz_test.rs` 对应 `merge_fuzz_test.go`；Rust 固定测试用例模拟 Go fuzz 种子/属性覆盖。
- `parity_test.rs` 是 Rust 侧新增的综合 Go/Rust 公共契约测试，没有同名 Go 文件。
- `stubs.rs` 是 Rust 移植兼容层，没有同名 Go 源文件；Go 直接依赖 `backuppb`、`kvrpcpb`、`metautil`、`tablecodec`、TiKV client 和 `google/btree`。

`BUILD.bazel` 确认 Go 生产目标只包含 `logging.go`、`rtree.go`，测试目标包含 `logging_test.go`、`main_test.go`、`merge_fuzz_test.go`、`rtree_test.go`。Rust manifest 的依赖面显著更小，并用标准库 `BTreeMap` 和本地 stubs 替代 Go 的真实依赖；因此只能声称算法和公开契约按测试对齐，不能把 Go 的真实 kvproto、meta writer 或 btree 资源行为直接视为 Rust 已具备。

## 扩展指南

按职责选择修改位置：区间、合并、空洞和进度算法进入 `rtree.rs`；日志展示进入 `logging.rs`；仅为移植边界所需的最小外部类型/编码适配进入 `stubs.rs`。不要把业务算法写入 crate root。

新增公开模块或符号时，应先判断是否需要保持 Go 包式根 API。若需要，在本文件增加显式模块声明及有意的重导出，并检查根命名冲突和兼容性扩张；若只供内部使用，应收窄可见性。新增算法测试应放在独立 `*_test.rs`，并在本文件的 `cfg(test)` 区挂载；公开契约变化还应同步 `parity_test.rs` 及相应 Go 对照测试。

修改 `RangeTree` 时重点验证重叠替换、空树、点区间、开放上界和尾部空洞；修改合并时验证阈值、空文件、同表 record、同表同 index、跨表/跨 index 和 API V2 keyspace 前缀；修改进度树时验证重叠拒绝、越界 region、writer 失败、回调次数、完成项删除、`skipChecksum` 及 u64 回绕。修改 stub API 时还要检查 `lib.rs` 的选择性重导出以及 `restore/utils` 的外部导入是否受影响。

性能风险主要在范围扫描、克隆文件列表和 `BTreeMap` 操作；兼容性风险主要在根导出路径、Go 风格命名、错误文本和空结束键语义。若要替换 stubs 为真实上游依赖，必须按仓库规则在独立上游仓库移植、提交并发布 tag，再统一更新 Cargo manifest，不能在本仓库复制 vendor 或使用本地 `[patch]`。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：当前索引包含 7,032 个 Rust 文件；`files --filter br/pkg/rtree` 列出本目录 14 个已索引 Go/Rust 文件。
- RustCodeGraph `node --file br/pkg/rtree/lib.rs --offset 1 --limit 240`：确认目标文件共 58 行，包含 3 个公开模块、2 个通配重导出、1 组选择性 stub 重导出及 4 个 `cfg(test)` 测试模块，且没有业务函数或类型。
- RustCodeGraph `query NewRangeTree --kind function` 与 `query NewProgressRangeTree --kind function`：同时定位 Rust `rtree.rs` 和 Go `rtree.go` 的对应构造入口。精确 `callers` 本次没有返回文本，因此未把缺失的静态调用边写成事实。
- 读过的 Rust 实现：`logging.rs`、`rtree.rs`、`stubs.rs`；确认日志脱敏/缩写、区间与合并算法、进度完成/写出/checksum 以及本地兼容类型边界。
- 读过的 crate/build 边界：`br/pkg/rtree/Cargo.toml`、`BUILD.bazel`；确认 crate 名、入口、Go 包映射、直接 Rust 依赖及 Go 生产/测试源集合。
- 全仓搜索 `astersql-br-pkg-rtree|astersql_br_pkg_rtree`：确认 `br/pkg/restore/utils/Cargo.toml` 是唯一反向 Cargo 依赖，生产引用位于 `merge.rs` 与 `rewrite_rule.rs`。
- Rust 独立测试 `logging_test.rs`、`merge_fuzz_test.rs`、`parity_test.rs`、`rtree_test.rs`，以及 Go 对照 `logging_test.go`、`merge_fuzz_test.go`、`rtree_test.go`：覆盖日志列表边界、合并属性、半开区间、空洞、强制写入、table/index 合并约束、进度回调、writer、checksum 与整数回绕。

本任务是纯文档分析，按计划未运行 Cargo，也未验证真实 kvproto、持久化、网络或性能行为。交付验证限定为任务指定的固定 11 章节结构、路径与事实人工复核、Markdown 差异检查和只暂存目标文档的提交检查。
