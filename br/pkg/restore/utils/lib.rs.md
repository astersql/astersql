# `br/pkg/restore/utils/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-restore-utils` 的 crate 根文件。`br/pkg/restore/utils/Cargo.toml` 通过 `[lib] path = "lib.rs"` 将它声明为库入口，并在 `package.metadata.porting.go-package` 中把该 crate 对应到 Go 包 `br/pkg/restore/utils`。它位于 BR 恢复工具链的共享基础层：快照恢复、日志恢复、区间拆分和部分恢复元数据流程通过这里暴露的 API 生成键重写规则、校验或改写备份文件范围，以及合并待恢复区间。

这个文件本身不执行恢复算法，也不持有业务状态；它负责确定模块边界和公共 API 表面。真实实现分别位于同目录的 `common.rs`、`merge.rs`、`misc.rs`、`rewrite_rule.rs` 和适配边界 `stubs.rs`。RustCodeGraph 将该文件识别为 59 行的 crate 入口；生产调用应继续追踪被再导出的具体符号，而不能仅把 `lib.rs` 当成算法实现。

## 核心职责

1. 用显式 `#[path = "..."]` 声明五个子模块，使 Rust 文件名与 Go 包内文件布局保持可核对关系：`stubs`、`common`、`merge`、`misc`、`rewrite_rule`。
2. 用 `pub use common::*`、`pub use merge::*`、`pub use misc::*`、`pub use rewrite_rule::*` 提供扁平公共接口。调用方因此可以从 `astersql_br_pkg_restore_utils` 直接导入 `RewriteRules`、`GetRewriteRawKeys`、`MergeAndRewriteFileRanges` 等符号，无需知道实现文件。
3. 仅从 `stubs` 精确再导出 `AppliedFile`，同时保持 `pub mod stubs` 可见。这里的桩是为当前移植代码隔离 `backuppb`、`import_sstpb`、`model`、`tablecodec`、`codec` 等外部类型的本地适配，不等于完整的 kvproto、RPC 或存储实现。
4. 在 `cfg(test)` 下挂接四个独立测试模块，保证 Rust 源文件与测试逻辑分离：`parity_test.rs`、`merge_test.rs`、`misc_test.rs`、`rewrite_rule_test.rs`。
5. 在 crate 根统一放宽迁移期命名和未使用项 lint。`dead_code`、Go 风格命名及未使用导入等告警被允许，是当前 Go→Rust 对齐阶段的兼容选择，不表示这些 API 已全部接入生产主链。

## 主要符号

`lib.rs` 没有自行定义常量、结构体、trait 或函数；它定义的是模块与再导出关系。主要公共表面如下。

- `pub mod common`：定义 `CreatedTable`，把新表 `TableInfo`、备份侧旧表和可选 `RewriteRules` 绑定为恢复建表后的中间对象。
- `pub mod merge`：公开 `MergeRangesStat` 与 `MergeAndRewriteFileRanges`。后者按 `StartKey` 归组 write/default CF 文件，调用 `RewriteRange` 后写入 `RangeStatsTree`，再按字节数和键数阈值合并范围。
- `pub mod misc`：公开 `WriteCFName`、`DefaultCFName`，以及 `GetPartitionIDMap`、`GetTableIDMap`、`GetIndexIDMap`、`TruncateTS`、`EncodeKeyPrefix` 等 ID/键辅助函数。
- `pub mod rewrite_rule`：公开核心状态 `RewriteRules`、`TableIDRemap`，规则构造 API `GetRewriteRules`、`GetRewriteRulesMap`、`GetRewriteRuleOfTable`，校验/匹配 API `ValidateFileRewriteRule`、`FindMatchedRewriteRule`，键与范围改写 API `GetRewriteRawKeys`、`GetRewriteEncodedKeys`、`RewriteAndEncodeRawKey`、`RewriteRange`，以及 `SetTimeRangeFilter` 等生命周期辅助函数。
- `pub mod stubs` 与 `pub use stubs::AppliedFile`：`AppliedFile` 抽象提供 `GetStartKey`/`GetEndKey`；`backuppb::File` 与 `DataFileInfo` 实现该 trait，使 SST 原始键和日志编码键能复用匹配接口。其余桩类型通过 `stubs` 命名空间访问，并未被通配提升到 crate 根。

公开名称保留了 Go 风格的大驼峰函数和字段名；crate 根的 `non_snake_case`、`non_camel_case_types`、`non_upper_case_globals` 允许项正是为这套迁移期 API 形状服务。

## 执行流程

`lib.rs` 的运行时作用发生在编译期名称解析，典型业务流程要经过其再导出后进入子模块：

1. 上游从 crate 根导入公共符号。例如 `br/pkg/restore/snap_client/tikv_sender.rs` 导入 `GetPartitionIDMap`、`ValidateFileRewriteRule`、`MergeAndRewriteFileRanges` 和 `RewriteRules`。
2. 恢复建表后，调用方用新旧表信息生成或携带 `RewriteRules`；规则内部记录旧/新表键前缀、表 ID 映射提示、可选 keyspace 和时间窗口。
3. 快照恢复发送路径先用 `ValidateFileRewriteRule` 确保文件起止键均命中规则且映射到相同新前缀，再用 `MergeAndRewriteFileRanges` 将同范围的 CF 文件成组、改写范围并按阈值合并。
4. 日志恢复路径在 `br/pkg/restore/log_client/import.rs` 中用 `GetRewriteEncodedKeys` 处理已 memcomparable 编码的日志键，用 `FindMatchedRewriteRule` 选择文件规则，并用 `EncodeKeyPrefix` 生成导入规则前缀。`br/pkg/restore/log_client/compacted_file_strategy.rs` 则使用 `GetRewriteRawKeys` 处理边界。
5. 范围拆分路径 `br/pkg/restore/split/splitter.rs` 使用 `GetRewriteEncodedKeys` 与 `GetRewriteTableID`，把日志范围映射到目标物理表。
6. 如果是测试构建，四个 `#[cfg(test)]` 模块才进入 crate；普通库构建不包含这些测试模块。

因此，`lib.rs` 不是串行调度器。它把同一套规则语义集中暴露给多条恢复链路，真正的分支、循环和错误传播都在被再导出的实现函数中。

## 数据与状态

crate 根自身没有全局变量、缓存、锁或可变静态状态。主要数据均由调用方拥有，并以值或借用传入子模块：

- `CreatedTable` 聚合恢复前后的表元数据和可选规则，不在 `lib.rs` 注册全局生命周期。
- `RewriteRules` 包含规则列表 `Data`、旧/新 keyspace、`NewTableID`、`ShiftStartTs`/`StartTs`/`RestoredTs` 和 `TableIDRemapHint`。`SetTsRange` 修改调用方持有的规则；`SetTimeRangeFilter` 只读取共享表规则并修改传入的单文件规则。
- `MergeRangesStat` 是一次合并调用的返回统计，包括输入文件数、各 CF 数、合并前后 Region 数及平均字节/键数；它不跨调用累积。
- `misc` 的 ID 映射函数每次构造新的 `HashMap`；键辅助函数返回新的 `Vec<u8>`。
- `stubs` 中 protobuf、模型和编码类型是本地精简表示。尤其 `metautil::Table` 当前只是占位空结构，不能据此推断完整备份表元数据已经接入该 crate。

所有权上，规则生成和键改写通常返回新集合或克隆规则；`RewriteRange` 对传入范围进行就地键替换后返回结果。文档或扩展代码应明确区分原始 SST 键与已经 `EncodeBytes` 的日志键，二者分别走 `GetRewriteRawKeys` 和 `GetRewriteEncodedKeys`。

## 依赖与调用关系

`Cargo.toml` 的直接依赖只有三个本地 crate：`astersql-br-pkg-errors` 提供恢复错误类别，`astersql-br-pkg-rtree` 提供范围树、范围与统计结构，`astersql-errors` 提供共享错误和上下文标注。`lib.rs` 不声明 feature，也没有 build dependency 或外部网络/RPC 依赖。

内部依赖方向为：

- `common.rs` → `rewrite_rule::RewriteRules` 与 `stubs::{metautil, model}`。
- `misc.rs` → `stubs::{codec, model}`。
- `rewrite_rule.rs` → `misc` 的 CF 常量/ID 映射、`stubs` 的 protobuf与编码适配，以及 errors/rtree crate。
- `merge.rs` → `misc` 的 CF 常量、`rewrite_rule::{RewriteRange, RewriteRules}`、`stubs::backuppb` 与 errors/rtree crate。

已核对的 Rust 生产调用边包括：

- `br/pkg/restore/snap_client/tikv_sender.rs` → `GetPartitionIDMap`、`ValidateFileRewriteRule`、`MergeAndRewriteFileRanges`。
- `br/pkg/restore/snap_client/import.rs` 与 `br/pkg/restore/misc.rs` → `GetRewriteRawKeys`。
- `br/pkg/restore/log_client/import.rs` → `GetRewriteEncodedKeys`、`FindMatchedRewriteRule`、`EncodeKeyPrefix`。
- `br/pkg/restore/log_client/compacted_file_strategy.rs` → `GetRewriteRawKeys`、`GetRewriteRuleOfTable`。
- `br/pkg/restore/split/splitter.rs` → `GetRewriteEncodedKeys`、`GetRewriteTableID`。

RustCodeGraph 对精确 `callers` 查询未返回文本边，故以上调用关系又由仓库内符号引用搜索核实；这也是当前图索引在本任务中的验证限制。另有 `br/pkg/restore/snap_client/stubs.rs` 和 `br/cmd/br/stubs.rs` 定义同名简化函数，搜索或导航时必须用文件限定符区分，不能把它们误认为本 crate 的实现。

## 错误处理与边界

crate 根不转换错误；公开 API 保留子模块的 `Result<_, SharedError>` 或可选返回语义。关键边界如下：

- `MergeAndRewriteFileRanges` 对空输入返回空范围和零统计；同一 `StartKey` 对应不同 `EndKey` 时按当前 Go 对齐逻辑触发 panic；完全无法识别 write/default CF 时返回 `ErrRestoreInvalidBackup`；重写或区间树插入冲突包装为 `ErrInvalidRange`。
- `ValidateFileRewriteRule` 在有规则但起点或终点未命中时返回 `ErrRestoreInvalidRewrite`，两端命中却映射到不同新前缀时同样拒绝文件。
- `RewriteRange` 要求范围起止键属于同一表，否则返回 `ErrRestoreTableIDMismatch`；无可匹配规则、空键或解码失败由各入口按其契约返回错误或 `None`。
- `SetTimeRangeFilter` 在未设置完整 TS 窗口时保持文件规则不变；只识别名称中包含 `default` 或 `write` 的 CF，其他值返回错误。
- `TruncateTS` 对空键返回 `None`，不足 8 字节时原样返回；`EncodeKeyPrefix` 只编码完整的 8 字节组并保留不足一组的尾部。
- `#![allow(...)]` 只抑制编译告警，不会把错误吞掉；扩展时不能用这些 allow 项替代明确的错误分支和测试。

日志边界也受桩实现限制：`stubs::log::Panic` 会真实 panic，但 `Error`、`Warn`、`Debug` 是空操作；`logutil` 只构造占位字段。因此当前本 crate 的日志副作用不能等同于 Go 生产日志设施。

## 并发与资源生命周期

`lib.rs` 不创建线程、异步任务、通道、文件句柄、网络连接或锁，也没有析构顺序要求。所有资源生命周期由调用方和子模块局部值控制。

规则查询和键改写以不可变借用为主，适合多个工作线程共享只读 `RewriteRules`，但 crate 根没有提供同步原语或线程安全包装。`rewrite_rule_test.rs` 的并发用例通过线程和通道验证多个读者调用 `SetTimeRangeFilter` 时不会回写共享 `RewriteRules`；每个线程修改的是自己的文件规则。若未来引入规则缓存或惰性初始化，需要在实现模块显式定义同步、取消和清理语义，不能仅在 `lib.rs` 添加再导出。

`MergeAndRewriteFileRanges` 在单次调用内拥有文件向量、分组 `HashMap` 和 `RangeStatsTree`，返回后这些临时容器正常释放；它没有后台工作。`AppliedFile` 只抽象键范围读取，不承诺底层文件、RPC 或存储对象的所有权。

## 与 Go 版本的对应关系

`Cargo.toml` 明确将本 crate 对应到 Go `br/pkg/restore/utils`。文件布局基本逐文件对齐：Rust `common.rs`/`merge.rs`/`misc.rs`/`rewrite_rule.rs` 分别对应同目录的 `.go` 文件；四份 Rust 测试也标注了对应的 Go 测试意图。

保持一致的主要语义包括：

- `CreatedTable` 绑定恢复后的表、旧表和重写规则。
- 表/分区/索引按 ID 和名称生成旧→新映射，支持粗粒度表前缀和细粒度 record/index 前缀规则。
- raw SST 与 encoded log key 使用不同入口；范围起止必须可安全映射到同一目标表。
- merge 按 write/default CF 成对理解逻辑 Region，并用字节/键阈值控制合并。
- `RewriteRules::Clone` 有意不复制 TS 窗口字段，这一点由 Rust 独立回归测试明确覆盖。

Rust 版的显著迁移差异是 `stubs.rs`：Go 直接使用 kvproto、TiDB model/tablecodec、日志设施和 protobuf 类型，而当前 Rust crate 用精简本地类型隔离这些依赖。它足以支撑已移植算法和单元测试，但没有 RPC、真实存储或完整 protobuf 字段。另一个差异是 Rust 通过 crate 根通配再导出模拟 Go 包级命名空间，并用 crate 级 lint allow 保留 Go 风格符号。

当前接线并不完全统一：快照恢复的 `snap_client/stubs.rs` 仍存在同名简化实现，而日志恢复和部分恢复代码直接依赖本 crate。扩展或替换调用时必须先确认实际导入路径，不能只凭同名符号判断已经复用 canonical 实现。

## 扩展指南

- 新增通用恢复算法时，先选择职责对应的实现文件；只有确实属于新的子域时才新增 `pub mod`。不要把业务逻辑直接写入 `lib.rs`。
- 若符号应成为稳定公共 API，可由现有模块的通配再导出自动暴露；若来自 `stubs`，应像 `AppliedFile` 一样审慎选择性再导出，避免把适配细节变成无意的根级契约。
- 修改重写规则时，应同步检查 `rewrite_rule_test.rs` 与 `parity_test.rs`，并核对 `rewrite_rule_test.go`；至少覆盖 raw/encoded 分流、跨表范围、规则缺失、时间窗口、分区/索引映射以及并发只读行为。
- 修改范围合并时，应同步检查 `merge_test.rs`、`merge_test.go` 和 `parity_test.rs`，覆盖空输入、未知 CF、同起点不同终点、重复区间以及字节/键阈值边界。
- 修改键辅助函数时，应同步检查 `misc_test.rs`、`misc_test.go` 和 parity 夹具，尤其是空键、短键、8 字节分组边界。
- 将桩替换为真实外部依赖时，需要先核对上游类型的字段、protobuf getter、编码、日志与错误语义；不能只让类型签名编译通过。外部 Rust 依赖还必须遵循仓库规则，在独立上游仓库移植、提交并发布 tag 后再由 Cargo 统一引用。
- 任何新增 Rust 测试继续放在独立 `*_test.rs` 文件，并在 `lib.rs` 通过 `#[cfg(test)]`/`#[path]` 挂接；不要把测试内嵌到生产源文件。
- 性能风险主要在规则线性匹配、规则/键克隆、按文件分组和范围树构建。优化前应保留 Go 行为、规则顺序和首个前缀匹配语义，并用独立测试证明兼容性。

## 验证依据

本说明基于以下直接证据：

- crate 与模块边界：`br/pkg/restore/utils/lib.rs`、`br/pkg/restore/utils/Cargo.toml`。
- Rust 实现：`br/pkg/restore/utils/common.rs`、`merge.rs`、`misc.rs`、`rewrite_rule.rs`、`stubs.rs`。
- Rust 独立测试：`br/pkg/restore/utils/parity_test.rs`、`merge_test.rs`、`misc_test.rs`、`rewrite_rule_test.rs`。
- Go 对照：`br/pkg/restore/utils/common.go`、`merge.go`、`misc.go`、`rewrite_rule.go` 及同目录对应 `*_test.go`。
- 生产调用：`br/pkg/restore/snap_client/tikv_sender.rs`、`snap_client/import.rs`、`br/pkg/restore/log_client/import.rs`、`log_client/compacted_file_strategy.rs`、`br/pkg/restore/split/splitter.rs`、`br/pkg/restore/misc.rs`。
- RustCodeGraph：`status` 显示 7032 个 Rust 文件、索引可用；`node --file` 用于读取 crate 根及实现/测试/Go 对照；`query` 确认本 crate 与其他桩模块存在同名符号。精确 `callers` 命令未输出边，因此调用点通过 `rg` 的文件限定搜索补充验证。

结构验收应执行任务指定命令，确认目标文件存在且恰好包含十一个固定二级标题。本任务是纯文档分析，没有修改 Rust/Go/Cargo 行为，也不运行 Cargo；对真实集群、RPC 与性能的运行时行为未作本地验证。
