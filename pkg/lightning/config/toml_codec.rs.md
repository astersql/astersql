# `pkg/lightning/config/toml_codec.rs`

## 文件定位

本文件属于 `astersql-lightning-config` crate。`pkg/lightning/config/Cargo.toml` 将该 crate 的库入口指定为 `lib.rs`，并声明对 `toml = "0.8"` 的直接依赖；`pkg/lightning/config/lib.rs` 公开 `toml_codec` 模块，但不把模块内函数再导出到 crate 根。它处在 TOML 文本与 `pkg/lightning/config/config.rs` 中完整 `Config` 对象之间，负责把任务配置按 Lightning 的配置段逐字段写入现有对象，并提供 `post-restore`、`cron` 两个段的文本编码辅助。

运行时公开入口是 `Config::load_from_toml(&mut self, data)`，其实现直接调用 `toml_codec::load_config_from_toml(self, data)`。`Config::load_from_global` 也先经过这个入口加载 `GlobalConfig::config_file_content`，随后再用全局对象中的若干字段覆盖结果。因此本文件负责“解码和写入”，不负责 `Config::adjust` 中的默认值补齐、字段间合法性检查、TLS 构造或导入执行。

## 核心职责

- `load_config_from_toml` 将字节验证为 UTF-8，交给 `toml::from_str` 解析成 `toml::Value`，再把已知顶层段分发给各 `apply_*` 函数。
- 解码采用显式字段映射而非 `serde::Deserialize`：它覆盖 `[lightning]`、`[tidb]`、`[checkpoint]`、`[mydumper]`、`[tikv-importer]`、`[post-restore]`、`[cron]`、`[security]`、`[conflict]` 和 `[[routes]]`，直接更新传入 `Config` 的对应子结构。
- `used: BTreeSet<String>` 与 `collect_keys` 共同模拟 Go BurntSushi TOML 的 `MetaData.Undecoded()` 检查：只有同时不属于任务 `Config` 和 `GlobalConfig` 的键才形成 `ConfigError::Invalid`；全局专属键被接受但不写入任务配置。
- `PostOpLevel::from_toml_value`、`CheckpointKeepStrategy::from_toml_value` 和 `Duration::from_toml_value` 承担 Go 自定义 `UnmarshalTOML`/`UnmarshalText` 语义的局部移植。
- `encode_post_restore` 与 `encode_cron` 生成确定顺序的 TOML 段内容；`encode_duplicate`、`encode_compression` 当前只是无返回值、无行为的迁移占位，不能视为已实现编码。

## 主要符号

- `GLOBAL_ONLY_PREFIXES`：列出 `lightning.status-addr`、`lightning.server-mode`、`lightning.pprof-port`、`lightning.level`、`lightning.file`、`tidb.log-level`。这些键属于 Go `GlobalConfig` 语义，在任务配置加载时不写入 `Config`，也不作为未知项报错。
- `pub fn load_config_from_toml(&mut Config, &[u8]) -> Result<(), ConfigError>`：唯一完整 TOML 解码入口。它依次完成文本解析、字段应用、全键收集和未知键汇总。
- `apply_value`：顶层路由器。它标记顶层段已访问，再把各段传给 `apply_lightning`、`apply_tidb`、`apply_checkpoint`、`apply_mydumper`、`apply_importer`、`apply_post_restore`、`apply_cron`、`apply_security`、`apply_conflict` 或 `apply_routes`。
- `apply_max_error`：兼容整数遗留写法和表写法；通过 `MaxError` 的原子字段/setter 写入错误上限，并保持 Go 路径中特殊的默认与忽略规则。
- `apply_csv`、`apply_files`、`apply_ignore_columns`、`apply_routes`：处理嵌套结构和数组。文件路由与忽略列规则先清空原向量再重建；routes 要求数组元素必须是表。
- `collect_keys`、`mark_subtree_used`、`is_global_config_key`：分别生成点分键集合、把复杂子树整体声明为已消费、识别全局专属键。数组下标以 `.0`、`.1` 形式进入路径。
- `string_value`、`string_array`、`string_map`、`string_or_string_slice`、`bool_value`、`int_value`、`i64_value`、`u64_value`、`float_value`：集中完成标量/容器转换并构造带路径的解析错误。`string_value` 有意允许字符串、整数、布尔和浮点转成字符串；`string_map` 则严格要求值已经是 TOML 字符串。
- `PostOpLevel::from_toml_value`：接受大小写不敏感的 `off`/`optional`/`required`，并把布尔 `false`/`true` 分别映射为 `Off`/`Required`。
- `CheckpointKeepStrategy::from_toml_value` 与 `marshal_text`：布尔 `false`/`true` 映射为 `Remove`/`Rename`，字符串交给类型自身的 `from_string_value`；编码返回 `remove`、`rename` 或 `origin`。
- `Duration::from_toml_value`：先转成字符串，再调用 `Duration::unmarshal_text` 解析 Go 风格时长。
- `encode_post_restore`、`encode_cron`：分别编码后处理开关和三个周期时长，返回类型虽为 `Result`，当前格式化路径本身没有显式失败分支。

## 执行流程

1. `Config::load_from_toml` 把已有 `Config` 的可变引用和输入字节传入 `load_config_from_toml`。
2. `std::str::from_utf8` 拒绝非法 UTF-8；`toml::from_str` 把合法文本构造成 `toml::Value`，并在解析失败时为错误文本增加 `toml:` 前缀。
3. `apply_value` 只在根值为表时遍历。每个已识别顶层段由相应 `apply_*` 逐键匹配；已知字段经转换辅助函数写入配置，未知字段会从 `used` 移除，留给末尾检查。
4. 嵌套字段按其数据形态处理：标量直接覆盖；`session-vars` 构造严格的 `HashMap<String, String>`；文件规则、忽略列和 routes 清空旧数组后逐项构造；ByteSize、枚举、Duration 等交给各类型的专用解析方法。
5. 应用结束后，`collect_keys` 重新递归枚举输入中的所有表键和数组路径。已消费键、`lightning.max-error.type/conflict` 特例、全局专属键以及无点号的纯顶层结构键被排除。
6. 剩余键按 `BTreeSet` 的稳定顺序进入 `both_unused`；非空时一次性返回 `ConfigError::Invalid("config file contained unknown configuration options: ...")`，否则加载成功。
7. 编码路径不走上述加载器：`encode_post_restore` 直接读取六个字段并格式化，`encode_cron` 调用三个 `Duration::go_string()` 后格式化；测试再把结果拼接到段标题下执行往返解码。

## 数据与状态

解码器不建立长期对象，主要临时状态是解析树 `toml::Value`、已消费路径集合 `BTreeSet<String>`、全路径集合以及未知键向量。选择 `BTreeSet` 使未知键诊断顺序稳定；`apply_max_error` 临时使用 `HashMap<String, i64>` 收集表字段，`string_map` 用 `HashMap` 保存 TiDB session variables。

传入的 `Config` 是原地更新而非事务式替换。解析语法失败发生在任何字段写入之前，但字段转换、枚举解析或最后的未知键检查失败时，之前成功处理的字段可能已经改变；调用方若需要失败时保持原配置不变，应先在副本上加载再替换。本文件也不调用 `Config::adjust`，所以成功解码仅代表文本和已映射字段可接受，不代表跨字段配置已经具备运行条件。

复杂数组的替换语义值得特别注意：`apply_files`、`apply_ignore_columns` 和 `apply_routes` 在构造新项前清空原集合；中途错误可能留下部分新集合。`apply_max_error` 写入的是 `MaxError` 内的原子计数，但这里仍在持有整个 `Config` 的独占可变引用。

## 依赖与调用关系

上游直接调用边经源码确认如下：

- `Config::load_from_toml` → `load_config_from_toml`。
- `Config::load_from_global` → `Config::load_from_toml`，随后执行全局字段覆盖。
- `pkg/lightning/config/config_test.rs` 与 `toml_codec_test.rs` 通过上述入口验证解码；前者还直接调用 `encode_post_restore`、`encode_cron`。

下游依赖分为三类：

- TOML 基础设施：`toml::from_str` 与 `toml::Value` 提供语法解析和动态值树。
- 配置类型：`Config` 及其 `Lightning`、`Security`、`Conflict`、`Routes` 等子结构是写入目标；`ByteSize`、`Duration`、`PostOpLevel`、`CheckpointKeepStrategy`、冲突/压缩枚举提供字段级转换。
- 标准库：`BTreeSet` 跟踪键并稳定诊断顺序，`HashMap` 承载映射，`Ordering::Relaxed` 用于更新 `MaxError` 的原子字段。

RustCodeGraph 的文件查询确认目标文件含 41 个索引符号，并给出 `pkg/lightning/config/config.rs`、`config_test.rs` 等文件级使用关系；精确 `callers load_config_from_toml` 在 30 秒查询窗口内没有返回，因此具体调用边以上述源码调用点为准。Cargo 元数据将 Go 对照包明确标为 `pkg/lightning/config`。

## 错误处理与边界

- 非 UTF-8 输入和 TOML 语法错误映射为 `ConfigError::Parse`；类型不符、整数越界、负数写入 `u64` 字段、数组/表形态错误也沿 `Result` 立即返回。
- 未知配置不是立即失败：各 `apply_*` 撤销对应 `used` 标记，末尾统一聚合为 `ConfigError::Invalid`。`max-error` 的自定义子键处理、文件规则与忽略列中的未知子字段存在兼容性忽略行为，不能把所有嵌套未知键都概括为严格拒绝。
- `apply_value` 以及大多数段处理器在值不是表时直接返回 `Ok(())`；该路径最终是否被未知键检查捕获取决于收集到的路径。相对地，`apply_files` 和 `apply_routes` 明确要求数组，route 元素明确要求表。
- `max-error` 整数路径通过 `set_legacy_value` 保留遗留行为；表路径只认可 `syntax`、`charset`、`type`、`conflict` 的整数值，并按 Go 移植规则重设原子值。源码明确让 `charset` 保持 `i64::MAX`，`syntax` 保持零，负的 type/conflict 被截为零。
- 全局专属键只是被视为合法，不会像 Go 版本那样在这里实际发出 warning；Rust 当前路径没有日志依赖。这是“接受但不生效”，不是任务配置支持这些字段。
- `encode_duplicate` 和 `encode_compression` 是空函数；任何需要文本编码的功能都不能依赖它们。编码函数也只返回段内字段，不包含 `[post-restore]` 或 `[cron]` 标题。

## 并发与资源生命周期

加载过程同步执行，不创建线程、异步任务、锁、通道、文件句柄或网络连接；解析树和键集合在函数返回时释放。`&mut Config` 保证一次调用对该对象拥有 Rust 级独占写访问，因此本文件没有内部并发协调需求。

唯一显式并发原语是 `MaxError` 字段内的原子整数。`apply_max_error` 使用 `Ordering::Relaxed` 写入，因为这里仅需要独立计数值的原子可见性，不用这些写操作建立跨字段 happens-before 关系。配置整体仍不具备原子提交语义：错误返回时可能保留先前写入，数组也可能处于部分重建状态。

内存开销与输入结构规模近似线性：TOML 首先完整构树，随后 `collect_keys` 再遍历并保存路径字符串；`mark_subtree_used` 对复杂数组/表额外递归。扩展到超大配置时需关注重复路径分配，但正常 Lightning 配置没有流式解析或外部资源生命周期。

## 与 Go 版本的对应关系

Go 基线位于 `pkg/lightning/config/config.go`。Go `Config.LoadFromTOML` 先用 BurntSushi `toml.Decode` 直接解码结构体，再分别读取 `Config` 与 `GlobalConfig` 的 `Undecoded()` 集合：两边都未消费的键报错，只被全局配置消费的键记录 warning。Rust 用显式 `apply_*`、`used`、`collect_keys` 和 `GLOBAL_ONLY_PREFIXES` 复现核心接受/拒绝语义，但当前不会输出 Go 的全局键 warning。

字段级对应关系包括：Go `PostOpLevel.UnmarshalTOML`/`MarshalText` 对应 Rust `PostOpLevel::from_toml_value` 与 `encode_post_restore`；Go `CheckpointKeepStrategy.UnmarshalTOML`/`MarshalText` 对应 Rust `from_toml_value`/`marshal_text`；Go `Duration.UnmarshalText`/`MarshalText` 对应 Rust `Duration::unmarshal_text`、`from_toml_value`、`go_string`；Go `MaxError.UnmarshalTOML` 对应 `apply_max_error`。Go 通过 struct tag 自动覆盖大量字段，Rust 则必须在所属 `apply_*` 中逐个接线，所以新增 Go 配置字段不会自动进入 Rust。

Go 的 `DuplicateResolutionAlgorithm` 和 `CompressionType` 已实现 TOML 解码与 `MarshalText`，Rust 本文件的加载路径会调用类型自身的字符串解析方法，但对应 `encode_duplicate`、`encode_compression` 仍为空占位。Go 测试 `config_test.go` 中的 `TestInvalidTOML`、`TestTOMLUnusedKeys`、`TestTomlPostRestore`、`TestCronEncodeDecode` 是 Rust `config_test.rs` 相应测试的语义来源；Rust 独立 `toml_codec_test.rs` 额外集中检查由本 codec 显式拥有的字段接线。

## 扩展指南

- 新增普通配置字段时，先在 `config.rs` 的对应结构与默认构造中定义语义，再在本文件相应 `apply_*` 分支增加 TOML 键映射；不要只新增结构字段，否则配置文本仍会被判为未知。随后在独立的 `toml_codec_test.rs` 扩展“字段确实写入”的回归用例，必要时在 `config_test.rs` 验证错误和调整阶段交互。
- 新增顶层段时，需要同时更新 `apply_value` 路由和未知键跟踪；若该段只属于 `GlobalConfig`，应核对 `global.rs` 与 Go `GlobalConfig` 的真实字段，而不是笼统跳过整棵树。
- 新增数组/嵌套表字段时，应明确未知子键是严格拒绝还是为 Go 兼容而忽略，并确保 `used`/`mark_subtree_used` 与 `collect_keys` 的数组下标路径一致。过度调用 `mark_subtree_used` 会掩盖拼写错误。
- 改动标量转换时要评估兼容面：`string_value` 当前接受非字符串标量，而 `string_map` 严格；`int_value` 限制为 i32，`u64_value` 拒绝负数。错误文本被现有测试和 Go 对照约束，不宜无依据改写。
- 若实现 `encode_duplicate` 或 `encode_compression`，应对齐 Go `MarshalText` 的输出，并在独立测试文件增加每个枚举值及非法/占位行为测试；不要把 Rust 测试嵌入生产源文件。
- 若需要失败不污染原配置，应在 `Config::load_from_toml` 边界引入“副本解码成功后替换”的明确设计并评估克隆成本；不能仅调整某个 `apply_*` 就宣称整体事务化。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/lightning/config/toml_codec.rs` 确认目标已索引；`node --file ... --offset 1/401` 读取了完整 926 行并报告 41 个符号；`query load_config_from_toml --kind function` 精确定位公开入口。精确 callers 查询在 30 秒内无返回，未把它当作调用边证据。
- Rust 源码与模块：`pkg/lightning/config/toml_codec.rs`（全部实现）、`pkg/lightning/config/config.rs`（`Config::load_from_toml` 与 `load_from_global` 调用边）、`pkg/lightning/config/lib.rs`（公开模块和独立测试模块接线）。目标目录没有 `doc.go`。
- crate 边界：`pkg/lightning/config/Cargo.toml`（crate 名、`lib.rs` 入口、`toml`/serde 依赖及 `go-package = "pkg/lightning/config"` 移植元数据）。
- Rust 测试：`pkg/lightning/config/toml_codec_test.rs` 验证 codec 所属字段和 routes 的完整写入；`pkg/lightning/config/config_test.rs` 验证非法全局内容、post-restore 非法值与布尔/字符串映射、编码文本及 cron 编解码往返。
- Go 对照：`pkg/lightning/config/config.go` 的 `Config.LoadFromTOML`、`PostOpLevel.UnmarshalTOML`、`CheckpointKeepStrategy.UnmarshalTOML`、`MaxError.UnmarshalTOML`、`Duration.UnmarshalText` 以及重复键/压缩枚举的 `MarshalText`；`pkg/lightning/config/config_test.go` 的 `TestInvalidTOML`、`TestStringOrStringSlice`、`TestTOMLUnusedKeys`、`TestTomlPostRestore`、`TestCronEncodeDecode`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务规定的 `rg` 结构命令确认恰有 11 个固定二级标题，并人工复核唯一新增生产物、源码链接、边界描述与扩展测试位置。
