# [`br/cmd/br/debug.rs`](./debug.rs)

## 文件定位

`debug.rs` 属于 Cargo crate `astersql-br-cmd-br`，crate 的库入口是 `br/cmd/br/lib.rs`，二进制入口是 `br/cmd/br/bin_main.rs`（见 `br/cmd/br/Cargo.toml`）。`lib.rs` 以 `pub mod debug` 暴露本模块；`br/cmd/br/main.rs::main` 再把 `NewDebugCommand()` 加入 `br` 根命令。因此它位于 BR 命令装配层，而不是备份/恢复的常规数据路径。

本文件实现隐藏的 `br debug <subcommand>` 命令族，并用 `validate` 作为顶层别名兼容旧版 BR。它面向离线排障和人工修复：检查备份对象摘要、检查 backupmeta 的范围与 rewrite rule、在 JSON 与 backupmeta 间转换、恢复 PD 调度配置，以及按 key 搜索日志备份。当前 Rust 版本同时依赖 `br/cmd/br/stubs.rs` 中的迁移期精简实现；文中所述能力以当前源码为准，不能等同于 Go 版本的完整生产实现。

## 核心职责

- `NewDebugCommand` 构造隐藏的顶层命令，注册 `checksum`、`backupmeta validate`、`decode`、`encode`、`reset-pd-config-as-default` 和 `search-log-backup` 六条执行路径，并通过 `PersistentPreRunE` 统一执行 `Init`、版本/环境日志和参数记录。
- `newCheckSumCommand` 从配置解析外部存储，验证 `backupmeta` 可读，读取 `tables.json` 侧车清单，逐对象重新计算 SHA-256，同时汇总表级 CRC64 XOR、KV 数和字节数。
- `newBackupMetaValidateCommand` 汇集所有数据文件范围，用 `RangeTree` 检查重叠；随后模拟重新分配表、索引和分区 ID，调用 `restoreutils::GetRewriteRules` 生成规则，并以 `ValidateFileRewriteRule` 检查每个文件。
- `decodeBackupMetaCommand` 支持两种模式：未指定 `--field` 时展开索引/统计元数据并写出 `backupmeta.json`；指定字段时从原始 JSON 对象读取对应值。
- `encodeBackupMetaCommand` 校验 `backupmeta.json` 可被任务层类型解析，拒绝 MetaV2，按配置加密后写回 `backupmeta` 或保护性文件名 `backupmeta_from_json`。
- `setPDConfigCommand` 经任务层 manager 边界调用 `UpdatePDScheduleConfig`，并在成功或失败路径都显式关闭 manager。
- `searchStreamBackupCommand` 校验十六进制 key，设置可选 TSO 窗口，执行日志备份搜索并输出格式化 JSON。

## 主要符号

- `pub fn NewDebugCommand() -> Command`：本文件唯一面向命令装配层的主要公开入口。命令本身 `Hidden = true`、`SilenceUsage = false`，别名为 `validate`。
- `pub(crate) fn backup_meta_field(&TaskBackupMeta, &str) -> Option<String>`：测试可见的字段提取辅助函数。它先把任务层 `BackupMeta` 序列化为 JSON，再委托给 `backup_meta_json_field`。
- `fn backup_meta_json_field(&serde_json::Value, &str) -> Option<String>`：把旧字段名 `start-version`、`end-version` 映射为 `StartVersion`、`EndVersion`；字符串原样返回、JSON null 输出 `<nil>`、其他值使用 JSON 文本表示，未知字段返回 `None`。
- `fn read_backup_meta_json(&dyn Storage, &Config) -> Result<Value>`：读取 `backupmeta` 原始字节；明文直接解析，非明文先跳过 16 字节 IV，再解析 JSON。当前实现没有在此处解密 ciphertext，这是迁移期格式约束和风险点。
- `fn newCheckSumCommand() -> Command`：构造隐藏且拒绝位置参数的 `checksum` 命令。
- `fn newBackupMetaCommand() -> Command` / `fn newBackupMetaValidateCommand() -> Command`：分别构造 `backupmeta` 分组和其 `validate` 子命令；后者定义 `--offset: u64`。
- `fn decodeBackupMetaCommand() -> Command` / `fn encodeBackupMetaCommand() -> Command`：构造互补的 JSON 导出、导入命令，二者均拒绝位置参数。
- `fn setPDConfigCommand() -> Command` 与 `pub fn UpdatePDScheduleConfig(&dyn Mgr) -> Result<()>`：前者负责 manager 生命周期，后者提供可独立测试的调用边界。
- `fn searchStreamBackupCommand() -> Command`：定义 `--search-key`、`--start-ts`、`--end-ts` 并驱动搜索器。

文件没有模块级可变状态、trait、struct、enum 或条件编译项；执行主体都保存在 `Command::RunE` 的 `Arc` 闭包中。

## 执行流程

1. `br/cmd/br/main.rs::main` 创建根命令并调用 `NewDebugCommand`。用户选择任一 debug 子命令时，持久化前置钩子先执行公共初始化和日志记录。
2. 每个执行闭包调用 `effective_task_flags(cmd)` 合并命令 flag，构造 `task::Config` 并执行 `ParseFromFlags`；需要对象存储的路径再调用 `GetStorage` 或 `ReadBackupMeta`。
3. `checksum` 先读取并校验主元数据，再由 `LoadBackupTablesFromStorage` 解析 `tables.json`。它按数据库、表、physical table 和文件四层遍历，记录文件元信息、累积表级统计量、读取实际对象并比较 SHA-256；首个摘要不匹配立即返回 `checksum_mismatch`，全部成功才打印成功消息。
4. `backupmeta validate` 将所有文件和表拍平。文件范围被逐个插入 `RangeTree`；重叠只记录错误日志，不中止。ID allocator 先前进 `offset` 次，然后为每张表、索引和分区生成新 ID，汇总 rewrite rules，最后逐文件验证规则覆盖；规则验证错误会中止。
5. `decode` 先通过 `ReadBackupMeta` 得到任务层类型，再读取 JSON 文档。无 `--field` 时依次处理 `FileIndex`、`RawRangeIndex`、`SchemaIndex` 与 `Schemas`，把完整 JSON 写入 `backupmeta.json`；有字段时只打印字段值或“not found”，未知字段不是失败。
6. `encode` 读取 `backupmeta.json`，先检查 JSON 中的 `Version`，MetaV2 直接报未实现；然后反序列化为 `TaskBackupMeta` 作形状校验。若原 `backupmeta` 已存在则改写到 `backupmeta_from_json`，最后以 `iv + encryptedContent` 格式落盘。
7. `reset-pd-config-as-default` 创建 manager，调用 `UpdatePDScheduleConfig`。失败时先 `Close` 再附加上下文错误，成功时也显式 `Close`。
8. `search-log-backup` 拒绝空 key，hex 解码后创建前缀比较器和搜索器，写入起止 TSO，执行 `Search` 并将结果 pretty-print 为 JSON。

## 数据与状态

- 输入配置集中在 `astersql_br_pkg_task::Config`：`Storage` 决定外部存储，`CipherInfo` 决定元数据加密格式，PD/TLS/keyspace/requirements 字段供 manager 初始化。
- `checksum` 的累计量在每张表开始时归零：`calCRC64` 对文件 `Crc64Xor` 做 XOR，`totalKVs` 与 `totalBytes` 做无检查的 `u64` 加法；它们只用于日志比较，不会反写元数据。
- `backupmeta validate` 在内存中构造 `Vec<File>`、表列表、`RangeTree`、ID allocator 与 `RewriteRules`。`--offset` 通过预先消费 table ID 改变后续模拟 ID；索引使用每表独立 allocator，分区使用表 ID allocator。
- decode/encode 的固定对象名来自 `metautil::{MetaFile, MetaJSONFile}`，分别是 `backupmeta` 和 `backupmeta.json`。encode 在目标已存在时采用 `_from_json` 后缀，避免覆盖原件。
- 所有外部存储句柄以 trait object/`Arc` 传递。命令本身不缓存跨执行状态；唯一共享可变对象是 `tidbGlue()` 返回的锁保护 glue，且只在创建 manager 时持锁。
- 当前 `LoadBackupTablesFromStorage`、`RangeTree`、`restoreutils` 与 `stream_search` 均来自 `br/cmd/br/stubs.rs`。尤其表清单取自调试侧车 `tables.json`，不是 Go 版 `MetaReader + LoadBackupTables` 的完整元数据遍历。

## 依赖与调用关系

上游调用链为 `br/cmd/br/bin_main.rs` → `br/cmd/br/lib.rs::main` → `br/cmd/br/main.rs::main` → `debug.rs::NewDebugCommand`。RustCodeGraph 还识别到 `br/cmd/br/debug_test.rs::debug_command_tree_matches_go`、`br/cmd/br/parity_test.rs::contract_normal_command_tree_and_filters` 和 `contract_error_paths` 对该入口的测试调用。

本 crate 的直接 Cargo 依赖中，本文件使用 `astersql-br-pkg-task`（配置、存储、manager、backupmeta 类型）、`hex`、`serde_json` 和 `sha2`；CLI、日志、错误、元数据/范围树/rewrite/search 等接口通过本 crate 的 `cmd` 与 `stubs` 模块提供。`Cargo.toml` 将该 crate 标记为 `kind = "binary"`，Go 对应包为 `br/cmd/br`。

关键下游边包括：`NewDebugCommand` → 六个子命令构造器；三个元数据命令 → `ReadBackupMeta`；checksum/validate → `LoadBackupTablesFromStorage`；validate → `RangeTree::InsertRange`、`GetRewriteRules`、`ValidateFileRewriteRule`；decode → `DecodeMetaFile`/`DecodeStatsFile`；encode → `Encrypt`；PD 复位 → `NewMgr`/`Mgr::UpdatePDScheduleConfig`/`Close`；日志搜索 → `NewStreamBackupSearch`/`Search`。

## 错误处理与边界

- 配置解析、存储读取/写入、JSON 编解码、加密、manager 创建和搜索错误都通过 `?` 返回；外部错误通常转换为本 crate 的 `Error`。
- checksum 对第一个 SHA-256 不一致立即返回带文件名、计算值和声明值的 checksum mismatch；但表级 CRC/KV/字节统计只打印，不判定是否与 schema 值一致。
- validate 读取 backupmeta 或表清单失败时先记错误日志再返回；文件范围重叠却只记录日志并继续，因此成功退出不代表“绝无重叠”。当前 stub 的 `ValidateFileRewriteRule` 也只是“非空 key 必须至少有一条规则”的粗校验。
- `read_backup_meta_json` 对非明文输入要求至少 16 字节，否则返回 `backupmeta is shorter than cipher IV`；它只是丢弃 IV 后解析 JSON，没有调用解密函数。对真实加密 backupmeta 的可用性未由本任务验证。
- decode 的未知字段返回成功并打印提示；旧 kebab-case 仅兼容两个版本字段。JSON null 以 `<nil>` 打印。
- encode 明确拒绝 MetaV2。它保护已有 `backupmeta`，但若 `FileExists` 本身报错，`unwrap_or(false)` 会把错误当作“不存在”，这是扩展时应审视的边界。
- search 拒绝空 key 和非法 hex；`start-ts`、`end-ts` 默认为 0，表示不限制对应边界。当前 stub 搜索器只返回输入 key 的占位记录，并未遍历真实日志备份文件。
- ID allocator 的分配错误在 validate 中被忽略；计数累计使用普通 `u64` 运算。极端数量/溢出行为没有独立测试证据。

## 并发与资源生命周期

本文件没有创建线程、异步任务、channel 或事务；所有子命令在 `RunE` 中同步执行。`Arc` 只用于让命令框架持有可共享的回调，不代表各文件或各表会并发处理，因此大备份上的 checksum 和 validate 成本随对象数量线性增长，且 checksum 会把每个对象完整读入内存后计算摘要。

Rust 代码通过 `GetDefaultContext` 取得共享上下文，但不同于 Go 版每条命令的 `context.WithCancel`/`defer cancel`，本文件没有建立子 context 或本地取消守卫。外部取消能否及时中断，取决于存储、manager 和搜索 stub 的具体实现。

外部存储以 `Arc<dyn Storage>` 随调用链共享并在作用域结束时释放。PD 路径最明确地管理资源：`tidbGlue().lock()` 在 manager 创建语句后随 guard 作用域结束；manager 在更新失败和成功路径都调用 `Close`。不过若 `NewMgr` 失败，则没有 manager 可关闭。其他命令没有显式关闭 storage，因为对应 trait 没有在此暴露关闭操作。

## 与 Go 版本的对应关系

Rust 文件直接对照 `br/cmd/br/debug.go`，命令名、顶层别名、隐藏属性、主要 flag、成功/错误文案和六条子命令布局总体一致。`br/cmd/br/debug_test.rs` 固定了命令顺序、`no_args` 约束以及字段兼容；`br/cmd/br/parity_test.rs` 固定顶层隐藏/别名、空搜索 key 错误和 PD manager 调用边界。

目前存在必须保留在认知中的迁移差异：

- Go checksum/validate 通过真实 `MetaReader` 和 `LoadBackupTables` 展开 backupmeta；Rust 改读 `tables.json` 调试侧车，并在 `stubs.rs` 中注明“非生产 backupmeta 管线”。
- Go backupmeta 是 protobuf，并使用真实解密/`MarshalBackupMeta`/`UnmarshalBackupMeta`/`proto.Marshal`；Rust 当前把 backupmeta 当作 JSON 文档处理。非明文 decode 只跳过 IV，encode 则直接加密 JSON 字节。
- Go validate 的范围树与 rewrite-rule 工具来自生产包；Rust 当前调用本地精简 stub，范围重叠同样只记录日志，但规则校验语义更粗。
- Go 日志搜索使用 `br/pkg/stream` 的真实实现；Rust 命令引用的是 `crate::stubs::stream_search`，当前 `Search` 仅产生占位结果。
- Go 每条执行路径创建并取消子 context；Rust 只读取默认 context。Go 的 `defer mgr.Close()` 覆盖函数返回，Rust 则在更新成功和失败分支显式关闭。
- Go 字段读取使用反射，可区分不可导出字段；Rust 通过 JSON 对象查找，因而只覆盖实际序列化出的字段，但测试已覆盖任意已序列化字段和两个旧别名。

因此当前 Rust 实现已经提供命令树与若干输入/资源契约，但不能据此声称 checksum、backupmeta 编解码、rewrite 校验或日志搜索已达到 Go 生产语义。

## 扩展指南

- 新增 debug 子命令时，在独立构造函数中定义 flags/`no_args`/`RunE`，再加入 `NewDebugCommand::AddCommand`；同步更新 `br/cmd/br/debug_test.rs::debug_command_tree_matches_go` 和 `br/cmd/br/parity_test.rs` 的对外契约断言。
- 扩展 checksum 或 validate 前，应优先把 `LoadBackupTablesFromStorage` 替换为任务层真实 backupmeta reader，且保持 Go 的数据库/表/physical file 遍历、空 schema 和错误传播语义。不要通过继续丰富 `tables.json` stub 来宣称生产等价。
- 完善 decode/encode 时，应接入真实 protobuf 与加解密格式，覆盖明文、加密、短 IV、MetaV1/V2、已有目标保护和未知字段；完整往返测试应放在独立的 `debug_test.rs`，不要内嵌到源文件。
- 强化 validate 时，要核对生产 `RangeTree` 和 `restoreutils` 的规则覆盖语义，并决定范围重叠是否继续只日志告警；任何行为变化都要与 Go 对照文件同步审查。
- 将日志搜索接到 `br/pkg/stream/search.rs` 的真实实现时，应验证 key 编码、TSO 闭区间/开区间语义、CF 合并、取消传播和大文件内存开销。
- 修改 PD 复位时必须维持 manager 在所有已创建路径上的关闭不变量；如果引入提前返回或更多操作，优先使用明确的 RAII guard，避免遗漏清理。
- 性能敏感点集中在 checksum 的逐文件完整读取、validate 的全量文件/表驻留和 decode 的完整 JSON 文档复制。扩展时应以真实数据规模评估流式处理，但不得改变可观察输出和 Go 兼容语义而不补回归测试。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7032 个 Rust 文件；`files --filter br/cmd/br/debug.rs` 命中目标；`node --file br/cmd/br/debug.rs --offset 1/261` 读取完整 519 行；`explore` 查询了 `NewDebugCommand`、六个子命令、`ReadBackupMeta`、`RangeTree::InsertRange`、`GetRewriteRules`、`ValidateFileRewriteRule`、`DecodeMetaFile`、`Encrypt`、`NewMgr`、`UpdatePDScheduleConfig` 和 `NewStreamBackupSearch::Search` 的调用关系。
- 目标与入口：`br/cmd/br/debug.rs`、`br/cmd/br/lib.rs`、`br/cmd/br/main.rs`、`br/cmd/br/Cargo.toml`。
- 当前依赖实现：`br/cmd/br/stubs.rs`（`metautil::LoadBackupTablesFromStorage`、`rtree`、`restoreutils`、`stream_search`）以及 `br/pkg/task/stubs.rs` 的 `Storage`/`Mgr` 边界。
- Go 对照：`br/cmd/br/debug.go`，逐项核对命令装配、checksum、backupmeta validate/decode/encode、PD 复位和日志搜索。
- 独立 Rust 测试：`br/cmd/br/debug_test.rs` 与 `br/cmd/br/parity_test.rs`；Go 同目录未发现专门覆盖 `NewDebugCommand` 的 `*_test.go`，因此 Go 侧行为依据主要来自实现文件。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `rg` 命令验证恰有 11 个固定二级章节，并人工检查文档明确标出 stub、未验证边界和 Go/Rust 差异。
