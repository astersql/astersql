# `br/pkg/restore/utils/misc.rs`

## 文件定位

`misc.rs` 属于 Cargo 包 `astersql-br-pkg-restore-utils`。该包由同目录的 `Cargo.toml` 定义为 library crate，入口是 `lib.rs`；`lib.rs` 以 `pub mod misc` 装载本文件，再以 `pub use misc::*` 扁平导出其公开项。因此，其他 crate 可以直接从 `astersql_br_pkg_restore_utils` 使用这里的常量与函数。

本文件位于 BR 恢复工具层，连接两类工作：一类是根据恢复前后的表元数据建立物理表、分区和索引 ID 映射，供 `rewrite_rule.rs` 生成 TiKV 键重写规则；另一类是处理带 MVCC 时间戳的键和 TiDB memcomparable 键前缀，供日志恢复范围扫描、去重及导入请求构造使用。它不执行网络 I/O、持久化或调度。

## 核心职责

- `GetPartitionIDMap`、`GetTableIDMap`、`GetIndexIDMap` 根据名称或表身份建立旧 ID 到新 ID 的映射，避免恢复后继续引用备份集中的旧物理 ID。
- `WriteCFName` 与 `DefaultCFName` 提供恢复规则识别 TiKV `write`、`default` 列族时使用的统一名称。
- `TruncateTS` 将编码键末尾的 8 字节时间戳移除，得到逻辑键或范围边界的用户键部分。
- `EncodeKeyPrefix` 只编码前缀中的完整 8 字节组，同时保留不足 8 字节的末尾片段，使前缀能够用于 TiKV 内部 memcomparable 键空间。

这些函数都是纯计算函数；它们只根据输入构造新 `HashMap` 或 `Vec<u8>`，没有隐藏的全局状态。

## 主要符号

- `pub const WriteCFName: &str = "write"`：write CF 的名称。它与 `DefaultCFName` 一起被 `rewrite_rule.rs` 的时间范围过滤逻辑引用。
- `pub const DefaultCFName: &str = "default"`：default CF 的名称。
- `pub fn GetPartitionIDMap(newTable: &model::TableInfo, oldTable: &model::TableInfo) -> HashMap<i64, i64>`：先以旧分区的 `Name.L` 建立名称到旧 ID 的索引，再遍历新分区；同名时写入 `old ID -> new ID`。任一表的 `Partition` 为 `None` 时返回空映射。
- `pub fn GetTableIDMap(newTable: &model::TableInfo, oldTable: &model::TableInfo) -> HashMap<i64, i64>`：复用 `GetPartitionIDMap`，然后无条件加入 `oldTable.ID -> newTable.ID`，所以非分区表也至少有一项表 ID 映射。
- `pub fn GetIndexIDMap(newTable: &model::TableInfo, oldTable: &model::TableInfo) -> HashMap<i64, i64>`：对旧、新索引做双层遍历，以 `CIStr` 相等判断同名索引并写入旧、新索引 ID。
- `pub fn TruncateTS(key: &[u8]) -> Option<Vec<u8>>`：空键返回 `None`；长度不足 8 时复制原键；否则复制除末尾 8 字节外的部分。长度恰为 8 时得到 `Some(Vec::new())`，这与真正的空输入不同。
- `pub fn EncodeKeyPrefix(key: &[u8]) -> Vec<u8>`：计算 `key.len() % 8`，将完整组交给 `codec::EncodeBytes`，移除编码器附加的最后一个 9 字节终止组，再拼回未分组尾部。

本文件没有自定义类型、trait、`impl`、宏或条件编译项。依赖的 `model::TableInfo`、`PartitionInfo`、`IndexInfo`、`CIStr` 与 `codec::EncodeBytes` 当前由同 crate 的 `stubs.rs` 提供。

## 执行流程

ID 映射的主流程如下：

1. `GetPartitionIDMap` 仅在新旧表都具有分区信息时工作。旧分区先按规范化名称 `Name.L` 建表，新分区再按同一字段查找；只有两侧同名的分区进入结果。
2. `GetTableIDMap` 取得上述分区映射后，再写入表本身的 ID 对。`rewrite_rule.rs::GetRewriteRules` 和 `GetRewriteRulesMap` 消费这个结果，为每个旧物理表生成 record/index 或整表前缀规则。
3. `GetIndexIDMap` 按名称找出索引 ID 对；同样由上述两个重写规则入口消费，在细粒度模式下为每个物理表生成索引前缀规则。

键辅助流程如下：

1. `TruncateTS` 先处理空键，再判断是否至少有 8 字节。`log_client/import_retry.rs::CreateRangeController` 对结束键截断时间戳后调用 `PrefixNextKey`，形成扫描的半开上界；`log_client/log_file_manager.rs::ReadFilteredEntriesFromFiles` 则用截断后的 auto-ID 元数据键作为去重键，保留更高时间戳的条目。
2. `EncodeKeyPrefix` 把输入拆成“完整 8 字节组”和“原始尾巴”。`codec::EncodeBytes` 会为完整组编码 marker，并额外产生一个终止组；函数删除该终止组，再追加原始尾巴。`log_client/import.rs::downloadAndApplyKVFileOwned` 用它编码匹配到的旧、新重写前缀，然后组装发送给导入服务的 `RewriteRule`。

## 数据与状态

所有权方面，输入表信息和键都以共享借用传入，输出完全独立：映射函数返回新的 `HashMap<i64, i64>`，键函数返回新的 `Vec<u8>` 或 `Option<Vec<u8>>`。调用方后续修改输入不会改变返回值。

名称匹配依赖 `stubs.rs::model::CIStr`。分区显式使用小写规范化字段 `Name.L`；索引比较完整 `CIStr`，当前 stub 的 `PartialEq` 会同时比较 `O` 与 `L`。因此索引名称的大小写/原始拼写语义取决于该类型定义，而不是本文件另行规范化。

映射以旧 ID 为 key。同名重复项不会被报告为错误：旧分区同名时，构建名称索引的后项覆盖前项；多个新分区匹配同一旧名或多个新索引匹配同一旧索引名时，后一次 `insert` 覆盖先前结果。`HashMap` 的遍历顺序也不稳定；下游不得把规则生成顺序当作协议。

## 依赖与调用关系

直接依赖只有标准库 `std::collections::HashMap` 和 `crate::stubs::{codec, model}`。同目录 `Cargo.toml` 没有 feature 声明；其三个 path 依赖供整个 utils crate 的其他模块使用，本文件自身没有直接调用外部 crate API。

已由 RustCodeGraph 和源码调用点核对的主要关系为：

- `GetTableIDMap -> GetPartitionIDMap`。
- `rewrite_rule.rs::GetRewriteRules -> GetTableIDMap / GetIndexIDMap`。
- `rewrite_rule.rs::GetRewriteRulesMap -> GetTableIDMap / GetIndexIDMap`。
- `EncodeKeyPrefix -> stubs.rs::codec::EncodeBytes`。
- `log_client/import.rs::downloadAndApplyKVFileOwned -> EncodeKeyPrefix`，同时编码旧、新键前缀。
- `log_client/import_retry.rs::CreateRangeController -> TruncateTS`，用于扫描结束键。
- `log_client/log_file_manager.rs::ReadFilteredEntriesFromFiles -> TruncateTS`，用于 auto-ID 元数据键去重。

RustCodeGraph 还显示 `utils/parity_test.rs::go_rust_public_contract_matches` 覆盖五个公开函数，并确认 `misc.rs` 被日志恢复的 `import.rs`、`import_retry.rs`、`log_file_manager.rs` 以及 parity 测试引用。`internal/rawkv/rawkv_client.rs` 当前保留了一个同语义的私有 `TruncateTS`，并非调用本文件的公开函数，不能把该路径计作本文件的直接调用者。

## 错误处理与边界

本文件没有 `Result` 错误通道，也不会主动记录日志。无法匹配的分区或索引被静默忽略，这是“只返回可证实映射”的设计，而不是错误恢复。

关键边界如下：

- `GetPartitionIDMap`：任一侧无分区信息时为空；只有一侧存在的名称不会进入结果。
- `GetTableIDMap`：始终写入表 ID 对，即使分区映射为空。
- `GetIndexIDMap`：空索引列表或没有同名索引时为空；算法复杂度为旧索引数乘新索引数。
- `TruncateTS`：空输入用 `None` 表示；`1..=7` 字节保持不变；8 字节输入返回空向量；更长输入移除最后 8 字节。函数只按长度处理，不验证后缀是否真是合法时间戳，调用者必须保证键格式。
- `EncodeKeyPrefix`：空键和不足 8 字节的键可安全处理；`saturating_sub(9)` 避免切片下溢。函数依赖 `codec::EncodeBytes` “始终附加一个 9 字节终止组”的契约，若编码器实现改变，删除终止组的逻辑必须同步复核。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄、网络连接或事务，也不保存跨调用状态；所有局部容器都在函数返回时转移给调用方。因此函数本身可被多个线程并发调用，线程安全只受不可变输入类型约束。

资源成本集中在分配与复制：映射函数分配 `HashMap`，`TruncateTS` 总是复制返回的字节，`EncodeKeyPrefix` 先取得编码缓冲区再为最终输出分配并复制。大键或高频调用场景若要优化分配，必须同时保持现有拥有型返回值及 Go 对齐语义，不能简单返回临时切片。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/restore/utils/misc.go`，五个函数与两个常量在名称和主要分支上逐项对应：分区按 `Name.L` 匹配、表映射包含分区加整表、索引按名称匹配、时间戳固定移除 8 字节、键前缀仅编码完整 8 字节组。

可见的语言适配差异为：

- Go 的空 `TruncateTS` 返回 `nil`，Rust 用 `None` 表达；非空结果由 Go 切片视图改为拥有型 `Vec<u8>`。
- Go 的短键可直接返回原切片，Rust 会复制数据，从而切断输入与输出的生命周期联系。
- Go 的 `EncodeKeyPrefix` 依赖 `codec.EncodeBytes` 后直接做切片；Rust 使用 `saturating_sub`，使空键/异常短编码结果不发生下溢。
- Go 使用真实 `pkg/meta/model` 与 `pkg/util/codec`；当前 Rust crate 通过 `stubs.rs` 的精简模型和 codec 门面实现所需契约。尤其索引 `CIStr` 的相等规则应随真实模型接线再次核对。

`misc_test.go` 与独立的 `misc_test.rs` 都验证 16 字节、15 字节、2 字节键的截断/前缀编码结果。Rust 的 `parity_test.rs` 额外验证空键、分区/表/索引 ID 映射及公开常量契约。

## 扩展指南

- 新增 ID 映射规则时，优先修改对应的 `Get*IDMap`，并检查 `rewrite_rule.rs::GetRewriteRules` 与 `GetRewriteRulesMap` 是否仍能正确消费映射；不要在下游重复实现名称匹配。
- 若调整名称匹配（例如索引改用 `Name.L`），需要同时核对 Go `misc.go`、`stubs.rs::model::CIStr`、`misc_test.rs` 和 `parity_test.rs`，并覆盖大小写、缺失名称及重复名称。
- 若改变时间戳长度或键格式，必须同步检查 `import_retry.rs::CreateRangeController` 与 `log_file_manager.rs::ReadFilteredEntriesFromFiles` 的范围和去重语义；还应评估 `internal/rawkv/rawkv_client.rs` 中同语义私有实现的漂移。
- 若替换或增强 `codec::EncodeBytes`，应保留“每 8 字节一组、每组 1 字节 marker、末尾终止组”的契约，或同步重写 `EncodeKeyPrefix`，并在 `misc_test.rs` 增加 0、7、8、9、15、16、17 字节边界。
- Rust 单元测试继续放在同目录独立文件 `misc_test.rs`；跨模块 Go/Rust 公开契约放在 `parity_test.rs`，不要把测试嵌回 `misc.rs`。

## 验证依据

本说明基于以下直接证据：

- 源码：`br/pkg/restore/utils/misc.rs`（两个常量、五个公开函数的完整实现）。
- crate 边界：`br/pkg/restore/utils/Cargo.toml`、`br/pkg/restore/utils/lib.rs`。
- 直接实现依赖：`br/pkg/restore/utils/stubs.rs` 中的 `model` 类型与 `codec::EncodeBytes`。
- 下游调用：`br/pkg/restore/utils/rewrite_rule.rs`、`br/pkg/restore/log_client/import.rs`、`br/pkg/restore/log_client/import_retry.rs`、`br/pkg/restore/log_client/log_file_manager.rs`。
- 对照实现与测试：`br/pkg/restore/utils/misc.go`、`misc_test.go`、`misc_test.rs`、`parity_test.rs`。
- 相邻同语义实现边界：`br/pkg/restore/internal/rawkv/rawkv_client.rs` 的私有 `TruncateTS`。
- RustCodeGraph：索引状态为 7,032 个 Rust 文件；`node --file br/pkg/restore/utils/misc.rs` 确认本文件 105 行及引用文件，`explore` 确认 `Get*IDMap`、`TruncateTS`、`EncodeKeyPrefix` 的主要调用边；对未由图精确定位的 Cargo、Go 和测试内容使用文件读取与 `rg` 核验。

本任务为纯文档分析，未运行 Cargo。结构验证应确认目标文件存在，且上述固定二级标题恰好为 11 个。
