# `br/pkg/restore/utils/rewrite_rule.rs`

## 文件定位

该文件属于 Cargo crate `astersql-br-pkg-restore-utils`；crate 入口 `br/pkg/restore/utils/lib.rs` 以 `pub mod rewrite_rule` 挂载本模块，并通过 `pub use rewrite_rule::*` 把公开符号扁平导出。它位于 BR 恢复链路的公共工具层：把备份侧表、分区和索引键前缀映射为目标集群前缀，同时为 SST 导入、PiTR 日志恢复、区间合并和切分提供统一的规则生成、校验与应用逻辑。

`br/pkg/restore/utils/Cargo.toml` 将该目录定义为独立 library crate，直接依赖 `astersql-br-pkg-errors`、`astersql-br-pkg-rtree` 和 `astersql-errors`。protobuf、表编码、日志和模型等迁移期依赖则由同 crate 的 `stubs.rs` 门面提供；ID 映射与 CF 名称来自 `misc.rs`。

## 核心职责

1. 用 `RewriteRules` 聚合 `import_sstpb::RewriteRule`、源/目标 keyspace、目标表 ID、TS 过滤窗口及物理表 ID 重映射提示。
2. 由新旧 `model::TableInfo` 生成表级或 record/index 细粒度规则：`GetRewriteRules` 返回聚合集合，`GetRewriteRulesMap` 按旧物理表 ID 分桶，`GetRewriteRuleOfTable` 构造单表集合。
3. 在导入前校验文件起止键是否都被规则覆盖且指向同一新前缀，并分别处理 raw key、memcomparable encoded key 和 `rtree::Range`。
4. 为 default/write CF 写入 PiTR 时间范围，并提供克隆、相等比较、追加、源表 ID 二次重映射、展示和目标表 ID 查询等辅助能力。

该模块不负责读取文件、下发 SST 或执行 region split；调用方先在这里建立或应用规则，再把结果交给恢复、导入或切分组件。

## 主要符号

- `RewriteRules`：核心可变值对象。`Data` 按顺序保存旧前缀到新前缀的规则；`OldKeyspace`/`NewKeyspace` 描述 keyspace 映射；`NewTableID` 服务单表/检查点路径；`ShiftStartTs`、`StartTs`、`RestoredTs` 组成过滤窗口；`TableIDRemapHint` 记录旧、新物理表 ID 对。
- `TableIDRemap { Origin, Rewritten }`：轻量的物理表 ID 映射提示。
- `RewriteRules::{HasSetTs, SetTsRange}`：仅当 `StartTs` 和 `RestoredTs` 都非零时认为时间窗口有效；设置方法同时写入三个 TS 字段。
- `RewriteRules::RewriteSourceTableID`：只改写每条规则的 `OldKeyPrefix`，保留表前缀之后的 record/index 后缀与目标前缀；至少命中一条时返回 `true`。
- `RewriteRules::{Clone, Equal, Append}`：`Clone` 深拷贝 protobuf 规则、keyspace、目标表 ID 和 remap hint，但按 Go 实现故意把三个 TS 字段归零；`Equal` 比较显式业务字段和每条规则的前缀/时间戳；`Append` 只追加 `Data`。
- `SetTimeRangeFilter`：把表级窗口写到文件规则。default CF 的下界是 `min(ShiftStartTs, StartTs)`，write CF 的下界是 `StartTs`，上界统一为 `RestoredTs`。
- `GetRewriteRules`、`GetRewriteRulesMap`、`GetRewriteRuleOfTable`：三个规则构造入口。`getDetailRule=true` 时生成一条 record 规则及每个索引一条规则；否则每个物理表只生成整表前缀规则。
- `ValidateFileRewriteRule`：校验文件起止 raw key 都能匹配，并要求两端规则的 `NewKeyPrefix` 相同。
- `RewriteAndEncodeRawKey`：将旧前缀的第一次出现替换为新前缀，再调用 `codec::EncodeBytes`；空规则时按 Go protobuf nil getter 语义编码原键。
- `FindMatchedRewriteRule`：先拒绝起止表 ID 不同的文件，再尝试 raw key，未命中时回退到 encoded key。
- `GetRewriteRawKeys`、`GetRewriteEncodedKeys`：成对改写文件边界；提供规则时任一端未命中即返回 `ErrRestoreInvalidRewrite`。
- `GetRewriteTableID`：用旧表 record 前缀匹配规则，并从新前缀解码目标表 ID；未命中返回 `0`。
- `RewriteRange`：原地改写 `Range` 的起止键并返回克隆；跨表时返回 `ErrRestoreTableIDMismatch`，缺规则时仅告警并保留原键。
- 私有辅助 `rewriteRawKey`、`rewriteEncodedKey`、`matchOldPrefix`、`replacePrefix` 和 `br_err` 分别负责编码层转换、按顺序首条前缀匹配、Range 前缀替换及静态错误包装。

## 执行流程

规则生成从 `GetTableIDMap(newTable, oldTable)` 和 `GetIndexIDMap(newTable, oldTable)` 开始。每个旧、新物理表 ID 对都会进入 `TableIDRemapHint`；细粒度模式为该物理表生成 record 前缀及全部旧、新索引 ID 对，粗粒度模式只生成 `EncodeTablePrefix`。`newTimeStamp` 只由 `GetRewriteRules`/`GetRewriteRulesMap` 写入生成的规则；`GetRewriteRuleOfTable` 与 Go 一致不写新时间戳，但设置 `NewTableID`。

文件导入路径先选择键形态。raw key 由 `rewriteRawKey` 调用 `matchOldPrefix`，随后 `RewriteAndEncodeRawKey` 替换并编码；encoded key 先由 `codec::DecodeBytes` 解码，再复用 raw 流程。`GetRewriteRawKeys`/`GetRewriteEncodedKeys` 先比较起止键解出的表 ID，然后分别改写两端；提供规则时两端必须各自命中。`ValidateFileRewriteRule` 更进一步要求两端命中的规则具有相同目标前缀，防止一个文件被撕裂到不同目标表或 region。

区间合并路径调用 `RewriteRange`。它先检查起止表 ID，之后通过 `replacePrefix` 对两端做前缀替换。与文件键 API 不同，某一端缺规则不会立即失败，而是记录告警并保留原键；这一差异要求调用方在导入边界继续使用更严格的校验。

PiTR 过滤路径先以 `SetTsRange` 配置表规则，再由 `SetTimeRangeFilter` 根据文件 CF 填充单条 `import_sstpb::RewriteRule` 的 `IgnoreBeforeTimestamp`/`IgnoreAfterTimestamp`。未形成完整窗口时函数直接成功返回且不修改文件规则。

## 数据与状态

所有状态都由调用方拥有，没有模块级可变全局量。`RewriteRules::Data` 的顺序具有语义：`matchOldPrefix` 与 `replacePrefix` 线性扫描并返回第一条匹配规则，因此更具体的前缀若可能被更宽泛前缀覆盖，应排在前面。构造函数当前为每个物理表选择“细粒度集合”或“单一粗粒度规则”，避免同一集合中自然产生这类覆盖。

键存在两种表示：raw TiDB table key，以及经过 `codec::EncodeBytes` 的 memcomparable key。`GetRewriteRawKeys` 的输出仍是 encoded key，这是 SST 接口契约；`RewriteRange` 则只替换 raw 范围键，不进行编码。空键在有规则时无法匹配；encoded 解码失败也以未匹配表示。

`Clone` 不是 Rust `Clone` trait 的简单语义替代，而是 Go 同名方法的业务克隆：它有意丢弃 TS 窗口。`Append` 同样只合并 `Data`，不会合并 keyspace、TS 或 remap hint。扩展时不得把这两个行为“补全”为全字段合并，否则会破坏 Go 对齐。

## 依赖与调用关系

下游依赖如下：`misc::{GetTableIDMap, GetIndexIDMap, DefaultCFName, WriteCFName}` 提供元数据映射和 CF 判定；`stubs::tablecodec` 负责表/record/index 前缀编解码；`stubs::codec` 负责 memcomparable 编解码；`import_sstpb::RewriteRule` 是最终传给导入服务的规则载体；`astersql_br_pkg_rtree::Range` 是合并/切分区间；`astersql_br_pkg_errors` 与 `astersql_errors` 提供分类错误及上下文包装；`redact` 保证展示和错误信息不直接泄露键。

RustCodeGraph 显示的关键上游调用边包括：`br/pkg/restore/misc.rs::getKeyRangeForBackupFileSet -> GetRewriteRawKeys`；`br/pkg/restore/log_client/import.rs::ImportKVFiles -> GetRewriteEncodedKeys`；`br/pkg/restore/split/splitter.rs::SplitPoint -> GetRewriteEncodedKeys`；`br/pkg/restore/log_client/import.rs::downloadAndApplyKVFileOwned -> FindMatchedRewriteRule`；`br/pkg/restore/split/splitter.rs::GetAccumulations -> GetRewriteTableID`。规则构造还被 `restore/log_client`、`restore/snap_client`、`restore/split` 与 `task` 等恢复组件使用，表明本文件是全量恢复和 PiTR 共用的键映射边界。

crate 内 `lib.rs` 公开再导出这些入口，因此调用方通常通过 crate 根访问，而不必显式写 `rewrite_rule` 模块路径。

## 错误处理与边界

- `SetTimeRangeFilter`：未设置完整 TS 窗口时不报错、不修改；CF 名包含 `default` 或 `write` 时接受，其他名称返回普通格式化错误。
- `ValidateFileRewriteRule`：提供规则但起点或终点未命中时返回带 `ErrRestoreInvalidRewrite` 分类的错误；两端目标前缀不同也返回同类错误，并提示备份数据可能脏或来自不兼容 BR 版本。
- `GetRewriteRawKeys`/`GetRewriteEncodedKeys`：起止表 ID 不同、任一边界未匹配均返回 `ErrRestoreInvalidRewrite`；未提供规则时按无重映射路径处理，但跨表检查仍然执行。
- `RewriteRange`：跨表使用更精确的 `ErrRestoreTableIDMismatch`；缺规则只告警并返回保留该端原键的范围。这是有意的宽松合并语义，不应与导入前的严格校验混为一谈。
- `FindMatchedRewriteRule`：跨表、raw/encoded 均未匹配时返回 `None`，错误策略由调用方决定。
- `GetRewriteTableID`：以 `0` 表示未命中；调用者必须把它当哨兵值而非真实目标表 ID。
- 日志和错误文本中的键通过 `redact::Key` 展示；`Display` 同样对 keyspace 与规则前缀做脱敏。

## 并发与资源生命周期

本文件不开启线程、任务、通道、事务或外部连接，也不持有文件句柄。规则构造和键改写均为同步 CPU/内存操作；主要成本是前缀向量克隆、拼接、memcomparable 编解码，以及对 `Data` 的线性扫描。

只读共享 `RewriteRules` 可安全用于并发计算，前提是每个调用者把独立的 `fileRule` 传给 `SetTimeRangeFilter`；`rewrite_rule_test.rs::test_set_time_range_filter_race` 用多线程共享表规则、各自写独立输出规则验证这一模式。`SetTsRange`、`RewriteSourceTableID`、`Append` 和 `RewriteRange` 都需要可变引用，Rust 借用规则阻止同一对象未经同步地并发写入。

返回的 `import_sstpb::RewriteRule` 多为克隆值，调用方不持有 `RewriteRules::Data` 内元素的借用；这简化了后续异步导入生命周期，但增加了与规则大小成正比的复制成本。大规则集合或热路径优化应优先评估首条匹配线性扫描及克隆开销。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/restore/utils/rewrite_rule.go`，Rust 文件公开结构、方法和自由函数基本逐项同名移植。独立测试 `rewrite_rule_test.rs` 也对应 `rewrite_rule_test.go` 的测试族：文件规则校验、raw/encoded 键、Range、表 ID、三种规则构造、匹配、跨表拒绝、时间过滤及并发读取。

需要特别保留的 Go 语义包括：`Clone` 只复制 Go 结构体字面量列出的字段并把 TS 留为零；`Append` 只追加规则数据；前缀查找采用第一条匹配；`RewriteAndEncodeRawKey` 模拟 `bytes.Replace(..., 1)`，包括空旧前缀在偏移零插入；`GetRewriteRuleOfTable` 不写 `NewTimestamp`；`RewriteRange` 缺规则只告警。Rust 用 `Option` 表示 Go 的 nil 规则/键，用 `SharedError` 表示带分类和注释的 Go error。

可观察到的实现形态差异是 Rust `RewriteRange` 返回 `Range` 克隆，同时也修改传入的 `&mut Range`；Go 返回原指针。两者对调用方都暴露已改写区间。Rust `RewriteAndEncodeRawKey(None)` 显式编码原键，以复现 Go 对 nil protobuf getter 返回空前缀的效果，而不是解引用空指针。

## 扩展指南

新增键类别或规则粒度时，优先修改 `GetRewriteRules`、`GetRewriteRulesMap` 和 `GetRewriteRuleOfTable` 的同构分支，并检查 `Data` 的前缀顺序，避免宽规则遮蔽窄规则。同步扩展 `rewrite_rule_test.rs` 的规则数量、目标前缀和时间戳断言，并对照 Go 文件及 `rewrite_rule_test.go` 保持行为一致。

改变文件边界策略时，应同时审查 `ValidateFileRewriteRule`、`GetRewriteRawKeys`、`GetRewriteEncodedKeys` 和 `FindMatchedRewriteRule`；raw/encoded 两条路径必须覆盖相同的跨表、空键、解码失败和未命中语义。涉及 Range 的改变还需检查 `br/pkg/restore/utils/merge.rs` 及 `restore/split` 调用方，明确是保持“缺规则告警”还是提升为错误。

增加 CF 或时间窗口规则时，在 `SetTimeRangeFilter` 中显式定义下界/上界，并补充零 TS、未知 CF、default/write 差异和并发只读测试。修改 `Clone`、`Equal`、`Append` 或展示字段时，要分别判断业务克隆、相等契约、聚合语义和脱敏要求，不能凭字段新增自动扩展。

性能优化可考虑预排序或索引化前缀匹配，但必须维持“首条规则胜出”的兼容语义，并用重叠前缀回归测试证明结果不变。任何日志增强都必须继续使用 `redact::Key` 或 `logutil::RewriteRule`。

## 验证依据

- RustCodeGraph：`status` 确认仓库索引可用；`explore "br/pkg/restore/utils/rewrite_rule.rs ..."` 确认 28 个 Rust 符号及跨模块调用；`node --file br/pkg/restore/utils/rewrite_rule.rs --offset 1/500` 完整读取 721 行实现；callers 结果确认 `GetRewriteRawKeys`、`GetRewriteEncodedKeys`、`FindMatchedRewriteRule`、`GetRewriteTableID` 的直接上游。
- 源与模块：`br/pkg/restore/utils/rewrite_rule.rs`、`br/pkg/restore/utils/lib.rs`。
- crate 边界：`br/pkg/restore/utils/Cargo.toml`。
- Go 对照：`br/pkg/restore/utils/rewrite_rule.go`。
- 独立 Rust 测试：`br/pkg/restore/utils/rewrite_rule_test.rs`，覆盖 `test_clone_matches_go_field_selection`、`test_rewrite_and_encode_raw_key_*`、`test_validate_file_rewrite_rule`、`test_rewrite_file_keys`、`test_rewrite_range`、规则生成/匹配/跨表及 `test_set_time_range_filter_race`。
- Go 测试：`br/pkg/restore/utils/rewrite_rule_test.go`；公共跨模块契约补充见 `br/pkg/restore/utils/parity_test.rs::go_rust_public_contract_matches`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查文档存在且固定章节恰为 11 个，并人工复核本文未把宽松 Range 行为误写为严格文件导入行为。
