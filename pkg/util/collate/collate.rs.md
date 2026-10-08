# `pkg/util/collate/collate.rs`

## 文件定位

本文件是 `astersql-util-collate` crate 的公共控制面，源码入口由 [`lib.rs`](./lib.rs) 以 `pub mod collate; pub use collate::*;` 装配并重导出。它不实现所有排序算法，而是定义统一的 `Collator` / `WildcardPattern` 接口、选择具体实现的工厂、全局新排序规则开关、名称与 ID 转换，以及多种实现共享的比较辅助逻辑。具体算法分别位于 `bin.rs`、`general_ci.rs`、`unicode_*_ci_*.rs`、`gbk_*.rs`、`gb18030_*.rs` 和 `pinyin_tidb_as_cs.rs`。

`Cargo.toml` 将该目录声明为独立库 crate；默认 feature 为 `full_collate`。crate 直接依赖 `astersql-util-dbterror`（SQL 错误契约）、`astersql-parser-charset`（字符集/排序规则元数据）和 `encoding_rs`（被具体编码实现使用）。因此本文件处在“SQL 类型或表达式携带的 collation 元数据”与“可执行的比较、排序 key、LIKE 匹配实现”之间。

## 核心职责

1. 用 `Collator` 统一比较、排序 key、通配符、克隆和最大 key 长度能力；用 `WildcardPattern` 统一 LIKE pattern 的编译与匹配。
2. 通过 `GetCollator`、`GetCollatorByID` 和内部 `new_collator` 把名称或 ID 分派到 binary、GBK、GB18030、Unicode、general-ci 等实现，并为未知/禁用场景提供明确回退。
3. 用原子变量 `newCollationEnabled` 控制新 collation 语义，同时用 `RewriteNewCollationIDIfNeeded` / `RestoreCollationIDIfNeeded` 在协议边界通过 ID 正负号区分新旧语义。
4. 包装 charset 元数据查询，维持缺失项默认值、未实现排序规则的 SQL 错误以及“实际已实现排序规则”列表。
5. 提供 PAD SPACE、CI/bin 分类、CI 到 bin 映射、UTF-8 rune 解码与通用权重比较等基础函数，供具体 collator 和上层编码/范围优化复用。

## 主要符号

- `DefaultLen: i32 = 0`：字符串 datum 未知长度时的占位值，与 Go 常量一致。
- `ErrUnsupportedCollation`、`ErrIllegalMixCollation`、`ErrIllegalMix2Collation`、`ErrIllegalMix3Collation`：分别维持 DDL 1273 与 expression 1271/1267/1270 的 `dbterror` 分类和 MySQL 错误码。`COLLATE_PACKAGE_INIT` 在 Unix/macOS/Windows 的进程初始化段中强制求值这些 `LazyLock`，避免错误注册结束后才首次初始化。
- `Collator: Send + Sync`：公开 trait。`Compare`、`Key`、`ImmutableKey`、`KeyWithoutTrimRightSpace`、`Pattern`、`Clone`、`MaxKeyLen` 是核心契约；`as_any` 支持具体类型判断。Rust 额外提供 `CompareBytes` / `KeyBytes`，以有损 UTF-8 转换承接 Go `string` 可保存任意字节的语义。
- `WildcardPattern: Send + Sync`：`Compile(patternStr, escape)` 先编译状态，`DoMatch(str_)` 再匹配；调用方必须保持这一生命周期顺序。
- `newCollationEnabled: AtomicI32`：进程级开关，初始为 `1`。`SetNewCollationEnabledForTest` 同时调用 `charset_switch::switchDefaultCollation` 并以 `SeqCst` 写入；`NewCollationEnabled` 以 `SeqCst` 读取。
- `new_collator` / `group2_collator`：内部构造器。基础组始终支持 binary、ASCII/Latin1/UTF-8 bin、0900 bin、GBK 和 GB18030；`full_collate` 开启时第二组增加 general-ci、Unicode 4.0/9.0 和拼音实现，关闭 feature 时第二组恒为 `None`。
- `GetCollator` / `GetCollatorWithCollate` / `GetCollatorByID`：公共分派入口。新语义启用时未知项回退 `binPaddingCollator`；禁用时统一使用 `derivedBinCollator`。`GetBinaryCollator` 与 `GetBinaryCollatorSlice` 显式构造无 PAD SPACE 的 binary collator。
- `CollationID2Name`、`CollationName2ID`、`GetCollationByName`、`SubstituteMissingCollationToDefault`、`GetSupportedCollations`：元数据查询、默认值和支持性校验入口。
- `truncateTailingSpace` / `truncateTailingSpaceBytes`、`decodeRune`、`sign`、`runeLen`、`compareCommon` / `compareCommonBytes`：具体实现共享的低层辅助函数。
- `CompatibleCollate`、`IsDefaultCollationForUTF8MB4`、`IsCICollation`、`ConvertAndGetBinCollation`、`ConvertAndGetBinCollator`、`IsBinCollation`、`IsPadSpaceCollation`：名称分类与优化决策辅助函数。
- `CollationToProto` / `ProtoToCollation`：名称和协议 ID 的双向桥接；`CanUseRawMemAsKey` 仅对 `binCollator` 与 `derivedBinCollator` 返回 `true`。

## 执行流程

按名称取得比较器时，上层调用 `GetCollator(name)`；它读取全局开关并转给 `GetCollatorWithCollate`。若新语义关闭，流程立即返回 `derivedBinCollator`；若开启，则 `new_collator` 先匹配基础实现，再按 feature 进入 `group2_collator`，最后对未知名称回退 `binPaddingCollator`。按 ID 查询时，`GetCollatorByID` 先通过 `charset::GetCollationByID` 得到规范名称，再复用 `new_collator`，查表或构造失败同样回退。

协议序列化路径为 `CollationToProto(name) -> CollationName2ID -> RewriteNewCollationIDIfNeeded`；新语义开启时非负 ID 被 `wrapping_neg` 变为负数。反序列化路径为 `ProtoToCollation(id) -> RestoreCollationIDIfNeeded -> CollationID2Name`；新语义开启时非正 ID 被恢复。两端查找失败均回退 `mysql::DefaultCollationID` / `DefaultCollationName`。

`compareCommonBytes` 先仅裁掉两端尾部 ASCII 空格，再用 `decodeRune` 逐个解码 rune，并用调用者提供的 `keyFunc(char) -> u32` 比较权重。首个不同权重立即返回 `-1` 或 `1`；任一侧遇到非法 UTF-8 首字节立即返回 `0`；共同前缀耗尽后，以剩余字节数的符号决定结果。`compareCommon` 只是把 `&str` 转成字节后进入同一路径。

真实上游调用表明该控制面横跨 SQL 主链：`pkg/expression/expr_to_pb.rs` 和 `pb_to_expr_runtime.rs` 使用协议转换；`pkg/types/datum.rs`、`enum.rs`、`set.rs` 和 `compare.rs` 使用比较器生成 key 或比较；`pkg/executor/aggfuncs/*`、`typed_hash_agg.rs` 使用它维护聚合相等性；`pkg/util/ranger/*` 用它构造边界和 sort key；`pkg/tablecodec/tablecodec.rs` 用 `IsBinCollation` 决定存储级 fast path。

## 数据与状态

唯一可变生产状态是进程级 `AtomicI32 newCollationEnabled`。值 `1` 表示开启，其他值都被读取为关闭；当前文件只写入 `0/1`。切换函数还修改 charset crate 的默认 collation，因此它不是只影响本文件的局部标志。

工厂不保存 Rust 版全局 collator map；每次查询创建一个新的 `Box<dyn Collator>`。`GetBinaryCollatorSlice(n)` 同样构造 `n` 个独立 trait object。返回的 charset `Collation` 也是拥有所有权的值。排序 key 由 `Vec<u8>` 承载，调用者拥有结果；尽管 `ImmutableKey` 名称延续 Go 契约，Rust 返回所有权值本身已阻止后续调用修改同一缓冲。

需要保持的不变量包括：协议重写/恢复在合法正 ID 上互逆；PAD SPACE 只裁 ASCII `0x20`；`IsBinCollation` 是“sort key 等于原始数据”的存储属性，刻意不包含会转码的 `gbk_bin`；`CanUseRawMemAsKey` 的判定范围比名字含 `_bin` 更窄，只认可两个真实无需重编码的具体类型。

## 依赖与调用关系

下游依赖可分为三类：

- `parser_charset::{charset, mysql}` 提供名称/ID 表、默认值和 `Collation` 元数据；`charset_switch::switchDefaultCollation` 配合测试开关切换默认规则。
- `dbterror` 提供规范化 SQL 错误、错误类别与 MySQL errno；静态初始化钩子确保错误模板在全局注册阶段及时建立。
- `crate::bin`、`gbk_*`、`gb18030_*` 以及 `full_collate` 下的 general/Unicode/拼音类型实现 `Collator`，本文件只负责选择，不复制它们的权重算法。

RustCodeGraph 将本文件标为被 95 个文件使用。精确源码搜索确认的代表性调用边包括：`pkg/expression/explicit_collation.rs -> GetCollationByName`，`pkg/expression/expr_to_pb.rs -> CollationToProto`，`pkg/expression/pb_to_expr_runtime.rs -> ProtoToCollation`，`pkg/types/datum.rs -> GetCollator.Key`，`pkg/util/ranger/points.rs -> GetCollator/IsPadSpaceCollation/IsBinCollation`，`pkg/planner/property/physical_property.rs -> RewriteNewCollationIDIfNeeded/RestoreCollationIDIfNeeded`，以及 `pkg/tablecodec/tablecodec.rs -> IsBinCollation`。

RustCodeGraph 对本文件的内部边确认：`GetCollator -> NewCollationEnabled + GetCollatorWithCollate`，`GetCollatorWithCollate -> new_collator`，`GetCollatorByID -> NewCollationEnabled + GetBinaryCollator + new_collator`，`GetCollationByName -> NewCollationEnabled + new_collator`，`compareCommonBytes -> truncateTailingSpaceBytes + decodeRune + sign`。由于当前索引的重名解析使 `callers` 为空、`callees` 混入同名定义，外部调用者采用上述精确 `rg` 结果补证。

## 错误处理与边界

`GetCollationByName` 首先传播 charset 查找错误；查找会规范化大小写和 `utf8mb3` 别名，随后必须用规范名称检查本 crate 是否确有实现。新语义启用而没有实现时，返回 `[ddl:1273]` 的 `ErrUnsupportedCollation`。相比之下，`GetCollator*` 是宽容入口：未知名称、未知 ID 或 feature 未编译时不返回错误，而回退 binary 风格实现；名称/ID 转换函数也回退 MySQL 默认值。

整数取负使用 `wrapping_neg`，因此 `i32::MIN` 保持自身且不会在 debug 构建 panic。`decodeRune` 在到达末尾时返回 `('\0', false)`；非法 UTF-8 仅消费一个字节并标记 invalid，合法编码的 U+FFFD 不会被误判。`CompareBytes` / `KeyBytes` 先用 `String::from_utf8_lossy` 转换，具体 collator 可通过覆盖这些默认方法保留更精确的 Go 字节语义。

当前 Rust 与 Go 的诊断行为并不完全相同：Go 对意外符号的协议 ID、未知 collator 回退及默认替换记录 warning，Rust 当前静默回退；调用方不能依赖日志来发现配置错误。另一个需要关注的现状是：Rust `GetSupportedCollations` 仅按 `new_collator(...).is_some()` 过滤，而 `full_collate` 下 `new_collator` 包含拼音实现；独立 Rust 测试却断言开发中的 `utf8mb4_zh_pinyin_tidb_as_cs` 不应展示。文档只能确认这处代码与测试意图的张力，不能在未运行 Cargo 的本任务中断言运行结果。

## 并发与资源生命周期

`Collator` 和 `WildcardPattern` 都要求 `Send + Sync`，因此 trait object 可以跨线程使用。全局开关以 `SeqCst` 原子读写，不存在数据竞争；但“写开关 + 切换默认 charset”是两个独立操作，不构成跨模块事务。测试因此使用 `pkg/util/collate/collate_test.rs` 中的 `COLLATION_TEST_LOCK: Mutex<()>` 串行化，并在每个测试末尾恢复开关。

工厂返回拥有所有权的 `Box<dyn Collator>`，slice 工厂返回拥有元素的 `Vec`，没有全局借用、后台任务、通道、锁守卫或显式关闭动作。`WildcardPattern` 的状态位于每个 `Box` 实例中：先 `Compile`、后 `DoMatch`，如需线程间共享，调用方仍须避免并发修改同一个可变 pattern。错误静态量由 `LazyLock` 管理至进程结束；平台初始化钩子只负责提前求值，不产生可回收资源。

## 与 Go 版本的对应关系

Rust 文件直接对照 `pkg/util/collate/collate.go`，主要名称和控制流保持一致：trait 对应 Go interface，原子开关默认开启，兼容组、协议负 ID、名称/ID 默认回退、PAD SPACE、CI/bin 分类、通用权重比较及 raw-memory 判定均保留。

重要实现差异如下：

- Go 在 `init()` 中建立名称/ID 到单例 collator 的 map；Rust 用 `match` 构造新对象，并用 Cargo `full_collate` feature 编译第二组实现。Go 的 `GetBinaryCollatorSlice(1)` 复用单例 slice，Rust始终分配独立对象。
- Go 的 `string` 可含非法字节；Rust 主接口采用 `&str`，另增 `CompareBytes` / `KeyBytes` 作为迁移桥。`compareCommonBytes` 显式复刻 Go `utf8.DecodeRuneInString` 的“非法序列为 RuneError 且消费一字节”判定。
- Go 对若干异常回退写日志；Rust 不记录。Go `SubstituteMissingCollationToDefault` 再查询 UTF8MB4 默认项，Rust直接返回 `mysql::DefaultCollationName`。
- Go `GetSupportedCollations` 显式排除开发中的拼音 collation；Rust 当前没有等价的显式排除条件，尽管测试保留了该预期。
- Go 以 map 中规范化 ID 判断支持性；Rust `GetCollationByName` 先由 charset 规范化名称，再以规范名称调用 `new_collator`，从而让 `UTF8MB4_BIN`、`utf8mb3_bin` 等别名继承目标实现的支持状态。

独立 Rust 测试 `collate_test.rs` 对齐 Go `collate_test.go`，覆盖八类 collator 的比较/key 表、新开关、ID 重写恢复、名称/ID 分派、未知项回退、SQL 错误码、别名规范化和非法 UTF-8。Rust 测试没有内嵌在生产源文件中，符合仓库测试分离要求。

## 扩展指南

新增排序规则时，至少要同步以下位置：在对应独立 `.rs` 文件实现完整 `Collator`（及需要的 `WildcardPattern`）；在 `lib.rs` 装配并重导出；在本文件 `new_collator` 或受 `full_collate` 控制的 `group2_collator` 添加规范名称；确保 `parser_charset` 能把名称与 ID 映射到同一 `Collation`；在 `collate_test.rs` 增加分派类型、Compare、Key、PAD SPACE、未知/别名和非法字节用例，并同步核对 Go 文件及 `collate_test.go` 的语义。

若新增规则的 sort key 等于原始数据，必须分别评估 `IsBinCollation` 与 `CanUseRawMemAsKey`：前者影响 `tablecodec`、ranger 和选择率 fast path，错误放宽可能破坏索引编码；后者依赖具体类型向下转型，不能仅凭名称后缀加入。若规则是 CI 或有 bin 对应项，还要同步 `IsCICollation`、`ConvertAndGetBinCollation`、`IsPadSpaceCollation` 和 `CompatibleCollate`，否则表达式比较与范围构造可能采用不一致语义。

修改全局开关或协议 ID 时，需要保持混合集群的负 ID 约定并覆盖 `0`、负输入和 `i32::MIN`。修改错误模板时必须保留 `dbterror` 类别、errno、RFC code 以及静态初始化时机。修改字节接口时要特别验证非法 UTF-8、合法 U+FFFD、GBK/GB18030 替换字节和尾部空格。性能风险主要来自每次工厂分配、Unicode 权重 key 的扩张，以及误判 raw-memory fast path；兼容性风险集中在排序结果、唯一索引相等性、协议 ID 和默认回退。

## 验证依据

- 生产源码：`pkg/util/collate/collate.rs`（全部 449 行）；crate 装配：`pkg/util/collate/lib.rs`；依赖/feature：`pkg/util/collate/Cargo.toml`。
- Go 对照：`pkg/util/collate/collate.go`；Go 测试：`pkg/util/collate/collate_test.go`；独立 Rust 测试：`pkg/util/collate/collate_test.rs`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/collate` 确认目标文件含 67 个符号；`node --file pkg/util/collate/collate.rs --offset 1 --limit 500` 返回完整源码并报告 95 个使用文件；`query` 核对了 `GetCollator`、`GetCollatorByID`、`GetCollationByName`、`RewriteNewCollationIDIfNeeded`、`compareCommonBytes`、`CanUseRawMemAsKey` 的 Rust 定义；内部 callees 结果与源码一致。外部调用边因图查询重名解析限制，使用精确 `rg` 补证。
- 人工复核：逐项检查工厂分派、feature 分支、原子状态、协议转换、错误回退、非法 UTF-8、测试隔离，以及 Go/Rust 差异；未把未运行的代码测试描述成通过。本任务按计划只做文档分析，不运行 Cargo。
