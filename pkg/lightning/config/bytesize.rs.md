# `pkg/lightning/config/bytesize.rs`

## 文件定位

本文件属于 `astersql-lightning-config` crate（`pkg/lightning/config/Cargo.toml`），定义 Lightning 配置层统一使用的字节容量值类型 `ByteSize`。crate 入口 `pkg/lightning/config/lib.rs` 通过 `pub mod bytesize` 声明模块并以 `pub use bytesize::*` 再导出该类型；因此同 crate 的配置结构和外部依赖方都可从 crate 根访问它。

它位于“配置文本/TOML/JSON”与“运行时字节数”之间：`pkg/lightning/config/config.rs` 用 `ByteSize` 表示 Mydumper 的读取/批次/Region 大小和 TiKV Importer 的发送批次、磁盘配额、缓存、限速等字段；`pkg/lightning/config/toml_codec.rs` 的 `load_config_from_toml` 经 `apply_mydumper`、`apply_importer` 调用 `ByteSize::from_toml_value`，将用户配置写入这些字段。本文件只负责容量值的表示和编解码，不负责默认值选择、字段间校验或实际资源分配。

## 核心职责

- 用 `ByteSize(pub i64)` 保存规范化后的字节数，同时保留复制、比较、排序、哈希和默认值等值类型能力。
- 用 `parse_ram_bytes` 接受纯数字及 RAM 风格后缀，后缀大小写不敏感，支持 `B`、`K/KB/KiB` 至 `P/PB/PiB`，所有单位按 1024 的幂换算。
- 为不同配置入口提供适配：`FromStr`/`unmarshal_text` 处理文本，`from_toml_value` 和 `Deserialize` 处理 TOML/Serde 值，`unmarshal_json` 复刻 Go 的数字专用 JSON 解码路径，`Serialize`/`marshal_json` 输出裸整数。
- 将底层 UTF-8、数字解析、Serde/TOML/JSON 错误归一为 `ConfigError::Parse` 或反序列化器的自定义错误，并尽量保持 Go 版本测试依赖的错误文本。

## 主要符号

- `pub struct ByteSize(pub i64)`：公开元组结构体，字段也公开。`Default` 为 `ByteSize(0)`；派生的 `Eq`、`Ord`、`Hash` 等都直接按内部字节数工作。
- `ByteSize::unmarshal_text(&mut self, bytes: &[u8])`：先验证 UTF-8，再调用 `parse_ram_bytes`；只有解析成功才覆盖 `self.0`。
- `ByteSize::unmarshal_json(&mut self, bytes: &[u8])`：通过 `serde_json::from_slice::<i64>` 仅接收 JSON 整数，成功后更新当前值。
- `ByteSize::marshal_json(&self)`：将内部 `i64` 编码为 JSON 数字字节串。
- `ByteSize::from_toml_value(&toml::Value)`：Lightning 自定义 TOML 装载链的主要入口。整数、浮点、字符串分别走边界检查、截断或 RAM 文本解析；布尔、日期、数组和表返回兼容性错误。
- `impl FromStr`：让 `"10MiB".parse::<ByteSize>()` 等文本调用复用 `parse_ram_bytes`。
- `impl Display`：输出十进制字节数而不是人类可读后缀。
- `impl From<i64>`：无检查地包装调用方提供的 `i64`；因此类型本身并不保证非负，非负约束只在解析入口执行。
- `impl Serialize`：始终调用 `serialize_i64`，所以 TOML/JSON 的 Serde 编码结果都是裸数字。
- `impl Deserialize` 与局部 `ByteSizeVisitor`：接受有符号整数、范围内无符号整数、有限非负浮点和字符串；显式拒绝负数、超出 `i64` 的无符号数、非有限浮点、布尔、序列与映射。
- `fn parse_ram_bytes(&str)`：私有核心解析器，完成数字/后缀拆分、`f64` 解析、单位选择、乘法和 `i64` 上界检查。

本文件没有模块级常量、条件编译项或独立 trait 定义。

## 执行流程

1. Lightning 的 `load_config_from_toml`（`pkg/lightning/config/toml_codec.rs`）先把 UTF-8 配置解析成 `toml::Value`，再遍历顶层配置段。
2. `apply_mydumper` 对 `read-block-size`、`batch-size`、`max-region-size`，`apply_importer` 对 `send-kv-size`、`region-split-size`、`disk-quota`、`block-size`、两个内存缓存大小、写带宽上限和逻辑导入批次大小调用 `ByteSize::from_toml_value`。
3. 对 TOML 整数，入口拒绝负值并直接包装；对有限非负浮点，以 Rust 浮点转整数规则截去小数部分；对字符串，进入 `parse_ram_bytes`。
4. `parse_ram_bytes` 从后向前寻找最后一个 ASCII 数字、小数点或空格。若找到空格，就把空格之前作为数值、之后作为后缀；否则把该字符包含在数值部分中并将余下部分视为后缀。
5. 数值部分按 `f64` 解析，拒绝解析失败、负数和非有限值；后缀转为 ASCII 小写，再映射到 0 至 5 次方的 1024 倍数。
6. 数值乘以单位倍数后若大于 `i64::MAX` 则报错，否则转换为 `i64`。该转换会截去正小数，因此 `2.5MB` 得到 `2.5 * 1024²`，无单位的 `256.9` 得到 256。
7. 编码方向不保留原始字符串或单位：Serde `Serialize` 和 `marshal_json` 都只输出最终字节整数，`Display` 同样只显示十进制整数。

另有两条不经过 Lightning TOML 分派器的入口：`FromStr`/`unmarshal_text` 直接执行第 4 至 6 步；`unmarshal_json` 直接要求 JSON `i64`。通用 Serde `Deserialize` 则由 `ByteSizeVisitor` 根据反序列化器提供的值类型分派。

## 数据与状态

`ByteSize` 唯一状态是公开的 `i64` 字段，不保存原始格式、单位、解析来源或缓存。成功解析后所有等价表示都收敛为相同字节数，例如 `10k`、`10 KB` 和整数 `10240`；再次编码无法恢复原写法。

解析入口的更新具有“成功后提交”语义：`unmarshal_text` 和 `unmarshal_json` 都先得到局部结果，再赋给 `self.0`，所以失败不会改变原值。`from_toml_value`、`FromStr` 和 `Deserialize` 返回新值，不修改既有对象。

非负不是结构体不变量：公开字段、`ByteSize(-1)` 以及 `From<i64>` 都可以构造负值；只有文本、TOML 和 Serde 解码入口实施非负检查。配置默认值和哨兵值位于 `config.rs`/`const.rs`，例如 `UNLIMITED_QUOTA = ByteSize(i64::MAX)`，不由本文件管理。

## 依赖与调用关系

上游关系如下：

- `pkg/lightning/config/lib.rs` 声明并再导出模块；同文件把独立的 `bytesize_test.rs` 作为测试模块接入。
- `pkg/lightning/config/config.rs` 在 `MydumperRuntime` 和 `TikvImporter` 中持有共 13 个 `ByteSize` 字段，并用 `ByteSize(...)` 构造常量和默认值。
- `pkg/lightning/config/const.rs` 用该类型声明读取块、Region、默认批次和最大 Region 容量常量。
- `pkg/lightning/config/toml_codec.rs` 的 `apply_mydumper` 和 `apply_importer` 是已确认的直接运行时调用者；其 `load_config_from_toml` 是完整配置加载入口。

下游依赖只有标准库和当前 crate/声明依赖：`std::str::from_utf8`、`FromStr` 与格式化 trait；`serde` 的序列化/反序列化接口；`serde_json` 的 JSON 编解码；`toml::Value` 的类型分派；以及 `crate::ConfigError`。`Cargo.toml` 明确声明 `serde`（derive）、`serde_json` 和 `toml = 0.8`，测试使用 `regex` 校验错误文本。本文件不调用文件系统、网络、存储引擎或导入执行器。

RustCodeGraph 索引将本文件标为被 6 个文件使用，并明确列出 `bytesize_test.rs`、`config.rs`、`config_test.rs`、`toml_codec.rs`、`toml_codec_test.rs` 等；精确 `callers` 命令未在 30 秒查询窗口内返回，因此具体调用点由上述源码搜索逐项核验。

## 错误处理与边界

- 非 UTF-8 文本由 `unmarshal_text` 转成 `ConfigError::Parse`；数字语法错误使用接近 Go `strconv.ParseFloat` 的文本，未知后缀使用 `invalid suffix`，负数、非有限值和溢出使用 `invalid size`。
- 空串或完全没有 ASCII 数字、小数点、空格的输入无法找到分隔位置，直接报 `invalid size`。后缀仅接受空串、`b`、`k/m/g/t/p` 及其 `b`/`ib` 形式；大小写会被规范化，但不会任意清理空白。
- 为对齐 `docker/go-units`，前后空格和错位空格不是通用可忽略空白；独立测试确认 `" 32 "`、`"32m b"`、`"32  B"` 均失败，`"32bm"` 报非法后缀。
- 文本计算以 `f64` 为中间表示。超过 `i64::MAX` 的结果被拒绝；合法正小数最终向零截断。接近 `i64` 上界的值可能受 `f64` 精度影响，扩展单位或改变边界策略时必须专门回归。
- `from_toml_value` 对布尔和日期生成 Go 兼容错误；数组/表报告类型不兼容。Serde visitor 的 `visit_map` 为兼容现有 Go 测试而区分首键是否为 `size`，非 `size` 映射使用固定日期错误文本；这不是通用的映射诊断机制。
- `unmarshal_json` 明确只接受 JSON `i64`，但通过通用 `Deserialize` 调用 `serde_json::from_str::<ByteSize>` 时 visitor 仍可接受字符串或非负浮点。调用方若要求 Go `UnmarshalJSON` 的严格语义，应使用显式方法，不能把两条入口视为完全等价。
- 所有错误均通过 `Result` 返回；本文件没有 `panic!`、静默回退或部分写入。`ConfigError::Parse` 的 `Display` 直接输出内部消息（定义见 `config.rs`）。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、原子、通道、事务或外部资源。`ByteSize` 是 `Copy` 值类型，每次解析只创建短生命周期的字符串切片、一个小写后缀 `String` 和局部数值；没有跨调用缓存或共享可变状态。

`unmarshal_text`/`unmarshal_json` 需要调用方持有 `&mut self`，Rust 借用规则保证更新期间的独占访问；其他转换均使用不可变借用或按值返回。资源生命周期因此完全由栈值和输入借用范围决定，无需显式清理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/config/bytesize.go`。Go 的 `type ByteSize int64` 对应 Rust 的 `ByteSize(pub i64)`；Go `UnmarshalText` 调用 `github.com/docker/go-units.RAMInBytes`，Rust 则在 `parse_ram_bytes` 内复刻其常见 RAM 后缀、1024 进制、空白和错误语义。Go `UnmarshalJSON` 先解码到 `int64`，Rust 的同名方法也用 `serde_json::from_slice::<i64>` 保持数字专用行为。

Rust 比 Go 文件承担更多适配工作：它实现 `FromStr`、`Display`、`From<i64>`、Serde `Serialize`/`Deserialize`，并提供 `from_toml_value` 连接 Rust 自定义 TOML 装载器。Go 依靠 BurntSushi/toml 自动调用 `encoding.TextUnmarshaler`；Rust 手工分派 `toml::Value`，以复现整数、下划线整数、浮点截断、科学计数、布尔/日期/容器错误等行为。

`pkg/lightning/config/bytesize_test.rs` 移植了 Go `bytesize_test.go` 的 TOML 解码和 TOML/JSON 数字编码用例，并额外覆盖 Go `RAMInBytes` 的空白及非法后缀行为。当前证据表明这些测试意图一致；Rust 对通用 Serde map 的固定日期错误以及显式方法与 Serde JSON 的入口差异属于 Rust 适配细节，不能从 Go 类型本身推导为通用协议。

## 扩展指南

- 新增或修改容量后缀时，应改 `parse_ram_bytes` 的后缀映射，并在独立的 `pkg/lightning/config/bytesize_test.rs` 增加大小写、空白、小数、零、负数和溢出回归；不要把测试嵌入生产文件。同时核对 `docker/go-units.RAMInBytes` 和 `bytesize_test.go`，避免 Rust 接受范围无意偏离 Go。
- 调整 TOML 可接受类型或错误信息时，应同时审查 `ByteSize::from_toml_value` 和 `ByteSizeVisitor`，并运行 `bytesize_test.rs`、`toml_codec_test.rs` 中的完整配置字段用例。两条路径目前有意分别服务自定义加载器与通用 Serde，不能只改其中一条便宣称全局行为改变。
- 调整 JSON 语义时，应先决定目标是显式 `unmarshal_json` 的 Go 兼容接口，还是 Serde `Deserialize`。若要求两者统一，需要补充字符串、浮点、超界整数和负数的独立测试，并评估既有调用者兼容性。
- 新增 `ByteSize` 配置字段时，类型字段应放在 `config.rs`，默认值放在相应默认构造/`const.rs`，TOML 键接线放在 `toml_codec.rs` 的所属 `apply_*` 函数；再在 `toml_codec_test.rs` 或 `config_test.rs` 验证字段确实被加载，而不是只验证解析器。
- 若要强化“永不为负”的类型不变量，需要封装公开元组字段并审计所有直接构造点；这会影响常量、默认值和外部 API，属于兼容性变更。若只优化性能，应优先保持无分配的数字路径，并注意当前字符串路径仅为后缀小写化分配内存。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7,032 个 Rust 文件；`files --filter pkg/lightning/config` 确认 `bytesize.rs`、模块入口、Go 对照和独立测试均已索引；`node --file pkg/lightning/config/bytesize.rs --offset 1 --limit 260` 读取完整 260 行源码并报告 6 个使用文件；`query ByteSize --limit 30`、`query from_toml_value --kind function --limit 30`、`query parse_ram_bytes --kind function --limit 10` 确认符号位置。`callers`/`callees` 查询在 30 秒窗口内未返回，未据此推断未验证的图边。
- 源码：`pkg/lightning/config/bytesize.rs`（类型、方法、trait 实现、visitor 和私有解析器）；`pkg/lightning/config/lib.rs`（模块声明、再导出、独立测试接线）；`pkg/lightning/config/config.rs`（`ConfigError`、容量字段和默认值）；`pkg/lightning/config/const.rs`（容量常量）；`pkg/lightning/config/toml_codec.rs`（`load_config_from_toml`、`apply_mydumper`、`apply_importer` 直接调用点）。目标目录不存在 `doc.go`。
- crate 边界：`pkg/lightning/config/Cargo.toml`（crate 名、`lib.rs` 入口、Go 包映射以及 `serde`、`serde_json`、`toml`、`regex` 依赖）。
- 语义对照与测试：`pkg/lightning/config/bytesize.go`、`pkg/lightning/config/bytesize_test.go`、`pkg/lightning/config/bytesize_test.rs`、`pkg/lightning/config/toml_codec_test.rs`。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证只检查目标文档存在且恰有 11 个规定的二级标题，并人工复核上述源码与调用点。
