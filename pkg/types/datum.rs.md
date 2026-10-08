# `pkg/types/datum.rs`

## 文件定位

`datum.rs` 是 AsterSQL Rust 类型系统的统一标量值实现，对应 Go 的 `pkg/types/datum.go`。它并非未接线的平行文件：内部 crate `pkg/types/internal/datum/lib.rs` 在末尾通过 `include!("../../datum.rs")` 编译本文件，根 crate `pkg/types/lib.rs` 再以 `pub use astersql_types_datum as datum` 暴露该模块。`pkg/types/Cargo.toml` 将这个内部 crate 声明为 `astersql_types_datum` 依赖，因此 executor、ranger、statistics 等消费者最终使用的就是这里的实现。

本文件处于 SQL 类型语义的基础层：上游把常量、行值、范围端点和中间计算结果包装成 `Datum`；本文件负责保存实际类型标签及载荷，并提供比较、转换、序列化、格式化和边界值操作。时间、DECIMAL、JSON、ENUM/SET、BIT、字符集/校对和向量的具体算法由相邻类型 crate 提供，本文件负责把它们组织为统一的分派入口。

## 核心职责

1. 用 `Datum` 和 `KindNull` 至 `KindVectorFloat32` 这 20 个标签统一承载 SQL 标量；通过 `k` 选择 `i`、`b`、`x`、`decimal`、`length`、`collation` 等字段的解释方式。
2. 通过成对的 `Get*`/`Set*`、`New*Datum`、`SetValue*` 和 `DatumValue` 完成值的装箱、取值与调试展示，并保留 Go `types.Datum` 的位级存储约定。
3. 通过 `Datum::Compare` 及 `compare*` 辅助函数执行跨 Kind 比较，处理 SQL `NULL`、`MinNotNull`、`MaxValue`、校对规则、数值互转、时间解析和 JSON/向量的专门语义。
4. 通过 `Datum::ConvertTo` 及 `convertTo*`/`To*` 方法完成由 `FieldType` 驱动的 MySQL 类型转换，并将长度、精度、标度、unsigned、字符集和语句上下文中的截断策略纳入结果。
5. 通过 `MarshalJSON`/`UnmarshalJSON`、`ToHashKey`、`DatumsToString*`、`CloneRow` 和内存估算函数支持持久化中间表示、哈希、诊断输出、复制与资源计量。
6. 通过 `GetMinValue`、`GetMaxValue` 和 `ChangeReverseResultByUpperLowerBound` 为范围推导和表达式反向求值提供类型边界。

## 主要符号

- `Datum { k, decimal, length, i, collation, b, x }`：核心容器。整数与浮点位模式复用 `i`；字符串、字节、二进制字面量、JSON 和向量序列化内容使用 `b`；`MyDecimal`、`Time` 和任意动态值使用 `Arc<dyn Any + Send + Sync>` 形式的 `x`。
- `Kind*` 常量：从 `KindNull = 0` 到 `KindVectorFloat32 = 19`，还包括范围构造使用的 `KindMinNotNull` 与 `KindMaxValue` 哨兵。`KindRaw` 仅表示原始字节，不能假定所有转换都接受它。
- `Clone` trait、`Datum::Clone`、`Datum::Copy`：trait clone 复制元数据和字节并共享 `x` 的 `Arc`；公开的 `Copy` 对 DECIMAL/Time 重新取值写回，`CloneRow` 逐项调用它。调用方需要区分 Rust 结构克隆和 Go 风格公开深拷贝入口。
- `Get*`/`Set*` 与 `New*Datum`：覆盖整数、浮点、字符串/字节、binary literal/BIT、DECIMAL、Duration、ENUM、SET、JSON、Time、vector 和 raw；setter 同时设置 Kind，并在需要时写入 collation、fsp 或序列化载荷。
- `Datum::Compare(ctx, ad, comparer)`：比较总入口。它按右值 Kind 分派，JSON 与非 JSON 时反向调用以维持 JSON 规则；字符串路径使用调用者提供的 `Collator`。
- `Datum::ConvertTo(ctx, target)`：转换总入口。根据 `FieldType::GetType()` 分派到整数、浮点、字符串、timestamp/date/datetime、duration、decimal、year、enum、bit、set、JSON 或 vector 转换。
- `ProduceFloatWithSpecifiedTp`、`ProduceStrWithSpecifiedTp`、`ProduceDecWithSpecifiedTp`：应用目标类型宽度、标度、unsigned、字符集及截断/告警策略的三个规范化辅助函数。
- `MarshalJSON`、`UnmarshalJSON` 与 `jsonDatum`：用 `k/decimal/length/i/collation/b/time/mydecimal` 形状保存 Datum；`b` 默认用 base64，反序列化也兼容数值数组。
- `Hash64ForDatum`：由 codec 层注入的全局函数指针，`Datum::Hash64` 通过 `unsafe` 调用；默认实现为空操作，初始化顺序属于外部契约。
- `SortDatums`、`DatumsToString*`、`GetMinValue`/`GetMaxValue`、`ChangeReverseResultByUpperLowerBound`、`EstimatedMemUsage`：分别服务排序、诊断输出、范围边界、反向推导和内存计量。

## 执行流程

构造流程通常从 `New*Datum` 或 `NewDatum` 开始。构造器创建默认 `Datum` 后调用对应 setter；setter 写入 Kind，再把标量存入 `i`、把可复制字节存入 `b`，或把复杂对象放入 `x`。`SetValue` 对字符串、ENUM、SET 使用目标 `FieldType` 的 collation，其余类型回落到 `SetValueWithDefaultCollation` 的 Any downcast 分派。

比较流程从 `Compare` 开始：先处理 JSON 反向比较，再根据右操作数 Kind 选择 `compareInt64`、`compareUint64`、`compareFloat64`、`compareStringBytes` 等路径。每条路径首先处理 `NULL` 和两个边界哨兵，然后优先做同类比较；异类值按 Go 规则转换成数值、字符串、时间或 JSON 后比较。字符串/字节、ENUM、SET 和 binary literal 的文本比较由传入的 `Collator` 完成；vector 只允许与 vector 比较，否则明确报错。

转换流程从 `ConvertTo` 读取目标 MySQL 类型开始。具体 `convertTo*` 先按源 Kind 提取或解析值，再调用 `Convert*`、`ParseTime*`、`ParseDuration`、`ParseEnum/Set`、JSON/vector 辅助函数；最后由 `Produce*WithSpecifiedTp` 应用目标宽度、精度、补零和 unsigned 约束。`Context` 决定截断是忽略、追加 warning 还是返回 error，也向时间解析提供时区与日期容错标志。

序列化时，`jsonDatum::from_datum` 复制公共字段，并对 Time、DECIMAL 提取专门载荷；其他仍带 `x` 的 Kind 会被拒绝。`MarshalJSON` 省略多数零值字段但总是写出 `time`，字节字段以 base64 表示。`UnmarshalJSON` 恢复公共字段，并根据 Kind 重建 Time 或 DECIMAL 的 `x`。

范围反推时，`ChangeReverseResultByUpperLowerBound` 先把结果转换到目标类型；溢出返回空 Datum，其它错误继续传播。随后它按结果 Kind 构造临时 `FieldType`，与对应边界比较：命中边界则换成目标类型边界；Ceiling 情况下还会在未达上界时对整数、浮点或 DECIMAL 推进一个单位。

## 数据与状态

`Datum` 是“标签 + 多用途槽位”结构，正确性依赖 `k` 与载荷一致这一不变量。`i64` 同时承载有符号整数、无符号整数的位模式、浮点位模式、Duration 纳秒、ENUM/SET 数值和 JSON type code；`decimal` 同时用于小数位/FSP；`b` 的含义随 Kind 变化。绕过 setter 直接组合字段会破坏这一不变量。

`Default` 的 `k` 为零，因此默认 Datum 即 SQL NULL。`KindMinNotNull` 和 `KindMaxValue` 是排序/范围哨兵，不是普通 SQL 值。`collation` 影响字符串比较和 `ToHashKey`；调用者若构造字符串却遗漏校对规则，比较键可能与预期不同。

字节 getter 大多返回克隆，避免调用者修改内部缓冲。`x` 使用 `Arc` 让普通 Rust clone 保留 Go interface 浅复制形状，而公开 `Copy` 再对 DECIMAL/Time 做值复制。`MemUsage` 以结构常量、`b.capacity()` 和 collation 长度估算单值；`EstimatedMemUsage` 对复杂 Kind 使用固定或专用估算，并乘以行数，它是近似计量而非分配器精确统计。

`Hash64ForDatum` 是进程级可变静态状态，且本文件不负责安装实现。除此之外，比较和转换主要修改调用者提供的 warning store 或返回新 Datum，不维护跨调用缓存。

## 依赖与调用关系

crate 接线链为 `pkg/types/Cargo.toml` → `pkg/types/internal/datum` → `include!("../../datum.rs")` → `pkg/types/lib.rs` 的 `pub use astersql_types_datum as datum`。内部 crate 的 `lib.rs` 负责再导出本文件需要的 Context、FieldType、decimal、time、JSON、vector、charset、mysql、collate 和错误桥接符号。

RustCodeGraph 将本文件标记为被 45 个文件使用；其符号调用证据包括：`pkg/util/ranger/points.rs` 调用 `Kind`、`GetInt64`、`SetInt64`、`Copy` 等构造和修正范围端点，`pkg/util/ranger/types.rs` 使用 Kind/格式化/边界判断，`pkg/executor/physical_plan_runtime.rs` 将 Datum 转为排序值，`pkg/statistics/histogram.rs` 与 runtime stats 路径使用 Datum 表达直方图边界，executor 聚合与写入测试也直接消费它。

下游依赖按职责分组：`types-group-1` 提供 `Context`、转换边界、字符串转数值和 binary literal；`types-field-group` 提供 `FieldType` 与目标类型裁剪；`types-time`、`types-decimal`、`types-json-binary`、`types-vector` 提供复杂值算法；`parser-charset`/`parser-mysql` 和 `collate` 提供 MySQL 类型标志、字符集与排序规则；`serde_json`/`base64` 提供 JSON 外层编码。

`Datum::Hash64` 与 codec 层存在反向注入关系：为避免 crate 循环依赖，本文件仅保存函数指针，真正的 Datum 哈希编码由 codec 包安装。修改哈希相关行为必须同时查找该安装点和相等性契约，不能只改 `Equals`。

## 错误处理与边界

大多数公共转换返回 `Result<_, errors::Error>`，并用 `invalidConv` 生成统一的源 Kind/目标类型错误。数值溢出、BIT 长度、字符串过长、DECIMAL 精度、非法 JSON charset、时间解析和 vector 维度错误保留各自的错误来源。内部 crate 的错误包装保存可选 typed cause，使 ranger 等调用者仍能按 `ErrOverflow` 身份分支。

`Context::Flags()` 是截断行为的关键边界：`IgnoreTruncateErr` 可吞掉部分长度错误，`TruncateAsWarning` 把错误写入 warning store，严格模式则返回错误。DECIMAL 的小数位舍入采用 `ModeHalfUp`；整数/浮点边界会饱和并携带 overflow。DATE 转换会清零时分秒；TIMESTAMP 路径还需时区上下文。

明确的 panic 边界有两处：未知目标 MySQL 类型在 `ConvertTo` 中被视为“不可能发生”而 panic；`GetVectorFloat32` 假设内部序列化载荷有效，反序列化失败会 panic。因此只能通过合法 setter/解码路径构造这些值。`SortDatums` 因 Rust 排序比较器不能返回错误，会暂存最后一次比较错误、临时返回 Equal，排序后再报错；错误结果下不应使用被部分重排的切片。

`truncateStringIfNeeded` 直接按字节位置切 `String`，调用者需注意长多字节文本在第 64 字节不是字符边界时存在 panic 风险；相对地，`ProduceStrWithSpecifiedTp` 使用 `utf8_safe_prefix_len`/`nth_rune_boundary` 避免非法 UTF-8 边界。`MarshalJSON` 不支持任意 `KindInterface` 载荷；`UnmarshalJSON` 对缺字段采用零值，并只重建 Time/DECIMAL 的动态载荷。

## 并发与资源生命周期

`Datum` 拥有 `String`、`Vec<u8>` 和 `Arc`，没有裸借用跨越 API，因此普通值可随所有者移动；`x` 限制为 `Any + Send + Sync`，使动态载荷具备线程边界所需约束。文件本身不启动任务、不创建通道、不持有锁，也没有事务生命周期。

资源释放依赖 Rust RAII：Datum 离开作用域后释放字符串/字节缓冲并递减 `Arc`。普通 `clone()` 会增加 `x` 的引用计数；公开 `Copy`/`CloneRow` 会复制字节，并对 DECIMAL/Time 重新装箱。大量行复制和 `GetBytes`/`GetRaw` 调用会产生实际分配，应在热路径评估成本。

唯一显著的并发风险是 `static mut Hash64ForDatum`：读写都缺少本地同步，且 `Hash64` 通过 unsafe 访问。系统必须在并发使用 Datum 哈希前完成一次性安装，并禁止运行期竞态替换。`Context` 的 warning store 可能具有内部可变性；转换方法虽然接收克隆的 Context，追加 warning 仍可能影响共享的语句级状态，这是预期语义。

## 与 Go 版本的对应关系

Rust 文件声明由 `pkg/types/datum.go` 迁移而来，Kind 编号、字段布局意图、构造器、比较矩阵、`ConvertTo` 分派、`Produce*WithSpecifiedTp`、JSON 中间结构、边界值和诊断格式均按同名 Go 符号组织。Go 的 `Datum` 用 `any` 保存复杂值，Rust 用受 `Send + Sync` 限制的 `Arc<dyn Any>`；Go 字节切片深拷贝对应 Rust `Vec` clone。

主要语言适配包括：Go type switch 改为 `Any::downcast_ref`；Go 的任意字节字符串与 Rust UTF-8 `String` 不完全等价，因此字符串裁剪显式寻找字符边界；Go `sort.Slice` 式错误闭包在 Rust 中通过外部 `Option<Error>` 暂存；Go JSON marshal/unmarshal 由手工构造 `serde_json::Value` 实现；Go interface 的动态打印由 `DatumValue` 和专门格式化补齐。

`pkg/types/datum_test.rs` 与 `pkg/types/datum_test.go` 的同名用例证明当前移植意图：二者都覆盖布尔/整数/浮点转换、JSON、NULL、字节输出、Clone、内存估算、反向边界修正、BIT、Marshal、DECIMAL 精度、NULL 比较、行格式化和可打印判断。仍需注意测试并非逐断言完全等价：例如 Go 的 unsigned 溢出用例还检查饱和值，而 Rust 当前测试只断言错误；Go DECIMAL 测试区分 warning 与 overflow，Rust 对部分 case 只检查 error presence 与结果字符串。扩展时应以 Go 行为和两侧测试共同校准，而不能因 Rust 现有覆盖较窄而简化语义。

## 扩展指南

新增一种 Datum Kind 时，应按完整链路修改：分配稳定 Kind 编号；扩展字段存取与 `DatumValue`；补齐 `GetValue`、调试格式、`Compare` 和必要的 `compare*`；补齐 `ConvertTo`、`ToBool`、`ToString`、`ToMysqlJSON` 等允许的转换；决定 Clone/Equals、Marshal/Unmarshal、内存估算、上下界和哈希编码语义；最后在独立的 `pkg/types/datum_test.rs` 增加与 `pkg/types/datum_test.go` 对齐的用例。测试不得内嵌进生产文件。

新增目标类型转换优先接入 `ConvertTo` 分派和一个聚焦的 `convertTo*`，复用 `Context`、`FieldType` 及现有 `Produce*`，不要绕过 warning/typed error 机制。字符串相关修改必须同时验证 binary/non-binary charset、按字节与按 rune 长度、CHAR/VARCHAR padding、非法编码及校对键一致性。数值修改必须覆盖 signed/unsigned、NaN/Inf、上下界、DECIMAL 舍入和错误值是否仍携带饱和结果。

修改 JSON 序列化要维持 Go 字段名与零值兼容，并验证历史的 base64 字节表示及数组兼容输入。修改 Clone 或内存估算时应分别验证 `b` 的所有权、`x` 的共享/重装箱、vector/decimal/time 的专门计量。修改 `Hash64ForDatum` 契约时必须联动 codec 注入点，并确保 `Equals` 与哈希保持一致。

性能风险主要在跨类型解析、反复克隆 `Vec`/`String`、collator 构造、JSON 编解码和行级格式化。兼容风险主要在 Kind 编号、比较全序、错误/告警分类、时区、字符集、JSON wire shape 与 Go 的任意字节字符串语义。

## 验证依据

- 源实现：`pkg/types/datum.rs`，通过 RustCodeGraph 按行读取全部 3005 行；重点核验 `Datum`、`Compare`、`ConvertTo`、`Produce*WithSpecifiedTp`、JSON、边界值和内存估算实现。
- 编译接线：`pkg/types/internal/datum/lib.rs:677` 的 `include!("../../datum.rs")`；`pkg/types/internal/datum/Cargo.toml` 的依赖与测试目标；`pkg/types/lib.rs:10` 的 crate 再导出；`pkg/types/Cargo.toml` 的 `astersql_types_datum` 依赖。
- 调用图：`rustcodegraph status` 显示索引含 11467 文件、307296 节点、1848419 边；`node --file pkg/types/datum.rs` 显示本文件被 45 个文件使用；`explore`/call graph 识别出 ranger、executor、statistics 等调用者，以及本文件向转换、时间、decimal、JSON、vector 与 collate 辅助函数的调用边。
- Go 对照：完整读取 `pkg/types/datum.go` 的类型/核心入口，并以同名符号和布局核对迁移；完整读取 `pkg/types/datum_test.go` 的 827 行用例。
- Rust 测试：完整读取独立文件 `pkg/types/datum_test.rs` 的 762 行；确认覆盖存取、转换、比较、clone、内存、边界修正、BIT、JSON 往返、DECIMAL 和诊断格式。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证仅执行任务指定的 11 章节结构检查，并人工复核所有结论均能回指上述源码、接线、调用图或测试证据。
