# `pkg/ddl/label/errors.rs`

## 文件定位

本文件是 `astersql-ddl-label` crate 的错误定义模块，源码入口为 [`errors.rs`](errors.rs)。`lib.rs` 以 `pub mod errors` 暴露该模块，因此外部可通过 `astersql_ddl_label::errors::{Error, ErrInvalidAttributesFormat}` 访问；crate 根只重新导出了 `attributes` 和 `rule` 的公开项，并未把这里的符号直接提升到根命名空间。

它位于 DDL 的 Region Label 属性解析边界，不负责 DDL job 持久化、schema state 转换、回填或 schema version 同步。当前 Rust 调用链集中在同 crate 内：`rule::Rule::ApplyAttributesSpec` 解析 AST 属性，继而调用 `attributes::NewLabels` / `NewLabel` / `Add`，这些函数以本文件的 `Error` 表达失败。

`pkg/ddl/label/Cargo.toml` 声明 crate 名为 `astersql-ddl-label`，根 workspace 又以 `facade_ddl_label` 将其挂到 `pkg::ddl::label` 门面；`pkg/ddl`、`pkg/executor`、`pkg/domain/infosync` 和 `pkg/store/gcworker` 的 Cargo manifest 声明了对此 crate 的依赖。不过仓库内 Rust 源码搜索目前没有发现这些外部 crate 直接构造或匹配本文件错误的生产调用，实际可确认的生产消费者仅为同 crate 的 `attributes.rs` 和 `rule.rs`。

## 核心职责

本文件只承担三项职责：

1. 保留 Go `ErrInvalidAttributesFormat` 的稳定基础文案，供兼容代码或调用方引用。
2. 用一个可克隆、可比较且实现 `std::error::Error` 的 Rust 枚举区分属性格式、属性冲突和规格解析三类失败。
3. 通过 `thiserror` 的展示模板集中定义用户可见错误文本，使解析函数无需重复拼接文案。

它不解析输入、不修改 Label 集合，也不把内部错误转换成 MySQL 错误码。Go 主链在 `pkg/ddl/executor.go` 中会把 label 层错误包装为 `dbterror.ErrInvalidAttributesSpec`（错误码 8237）；当前 Rust 搜索未发现与该上层转换等价的已接线生产调用，因此不能把该转换归功于本文件。

## 主要符号

### `pub const ErrInvalidAttributesFormat: &str`

值固定为 `attributes should be in format 'key=value'`。它对应 Go `errors.go` 中可被 `%w` 包装的 sentinel error 的基础文案，但 Rust 中只是 `&'static str`，不是 `Error` 值，也不能用于错误源链或按类型判断。仓库搜索显示该 Rust 常量目前除定义处外没有引用；实际格式错误由下面的枚举变体直接生成相同前缀。

### `pub enum Error`

派生 `Clone`、`Debug`、`Eq`、`PartialEq` 和 `thiserror::Error`。这使测试和调用者可以按变体或完整载荷比较错误，也可用 `{}` / `to_string()` 得到规定文案。

- `InvalidAttributesFormat { attribute: String }`：保存原始单条属性文本，展示为 `attributes should be in format 'key=value': {attribute}`。`attributes::NewLabel` 在分割结果不恰好为两段、trim 后 key 为空或 value 为空时构造它。
- `ConflictingAttributes { new_label: String, existing_label: String }`：保存待加入标签和已存在标签的 `key=value` 表示，展示为 `'{new_label}' and '{existing_label}' are conflicted`。`attributes::Add` 发现同 key 不同 value 时构造它。
- `InvalidAttributesSpec(String)`：保存 `serde_yaml` 严格解析失败后的文本，展示时不加前缀。`rule::Rule::ApplyAttributesSpec` 在把 `AttributesSpec.Attributes` 包成 YAML 数组并反序列化失败时构造它。

文件中的 `#![allow(non_snake_case, dead_code)]` 允许保留 Go 风格常量名和暂未被 Rust 生产代码引用的兼容符号。

## 执行流程

本文件没有可执行函数；错误在相邻模块中按以下流程产生和传播：

1. `Rule::ApplyAttributesSpec` 收到 AST 的 `AttributesSpec`。若 `Default` 为真，它清空 Labels 并成功返回，不产生本文件错误。
2. 否则该方法将属性文本包成 YAML 数组，由 `serde_yaml::from_str` 解析。解析器拒绝输入时，经 `map_err` 转为 `Error::InvalidAttributesSpec(error.to_string())` 并立即返回。
3. YAML 成功得到 `Vec<String>` 后，`NewLabels` 逐项调用 `NewLabel`。每项必须经 `split('=')` 得到恰好两段，且 trim 后 key/value 都非空；失败即返回 `InvalidAttributesFormat`，`?` 会终止整批构建。
4. 每个合法标签交给 `Add`。不同 key 继续扫描，同 key 同 value 被视为重复并成功忽略；同 key 不同 value 返回 `ConflictingAttributes`。冲突发生在 `labels.push(label)` 之前，因此不会把冲突项写入集合。
5. `NewLabels` 和 `ApplyAttributesSpec` 用 `?` 原样向上返回同一个 `Error`，本文件不记录日志，也不做错误码转换。

## 数据与状态

所有错误数据均由调用点按值拥有：三个 `String` 载荷分别保存原始输入、恢复后的新旧标签或 YAML 错误文本。枚举没有借用和生命周期参数，因而可越过普通函数边界返回；`Clone` 和 `Eq`/`PartialEq` 也使调用者能复制或精确断言。

本文件没有全局可变状态。唯一静态数据是只读字符串切片常量。错误构造不会改变 Rule、Label 或外部元数据；集合是否变化由 `attributes::Add` 的控制流决定。特别是冲突分支先生成两个展示字符串再返回错误，尚未执行追加；迁移测试明确断言错误前后的列表相等。

`InvalidAttributesSpec(String)` 只保存底层错误的格式化文本，不保存 `serde_yaml::Error` 本体，也未用 `#[source]` 建立错误链。这便于枚举保持 `Clone + Eq`，代价是调用者无法再向下转型到底层 YAML 错误或读取其结构化字段。

## 依赖与调用关系

### 下游依赖

本文件唯一直接代码依赖是 `thiserror`：`#[derive(thiserror::Error)]` 生成 `Display` 和 `std::error::Error` 实现。`Cargo.toml` 将其声明为 `thiserror = "2"`。标准库的 `String` 和派生 trait 由 prelude/编译器提供。

### 上游调用者

- `pkg/ddl/label/attributes.rs::NewLabel` 构造 `InvalidAttributesFormat`。
- `pkg/ddl/label/attributes.rs::Add` 构造 `ConflictingAttributes`。
- `pkg/ddl/label/attributes.rs::NewLabels` 通过 `?` 传播前两类错误。
- `pkg/ddl/label/rule.rs::Rule::ApplyAttributesSpec` 构造 `InvalidAttributesSpec`，并传播 `NewLabels` 的错误。

RustCodeGraph 将 `errors.rs` 识别为含 2 个符号的已索引文件；对具体枚举变体的名称查询没有建立节点，因此上述变体边由源码引用搜索补证。对整个仓库的精确搜索也表明，生产 Rust 中没有上述两个相邻模块之外的变体构造或匹配点。crate 可通过根门面访问，并不等于这些错误已经接入 Go DDL executor 的用户错误转换链。

## 错误处理与边界

- 格式校验是“恰好一个等号”：空串、没有等号、空 key、空 value、`a=b=c` 都产生 `InvalidAttributesFormat`；两端空白由 `NewLabel` 裁剪后再判空。
- 格式错误载荷保留调用者传入的原始字符串，因此展示文本可包含原始空白或多余等号；它不会转义或脱敏。若未来允许用户输入敏感值，向日志输出该错误前需重新评估。
- 冲突只定义为“同 key、不同 value”；完全重复的 `key=value` 不报错。错误文案中的 `new_label` 在前，首次遇到的 `existing_label` 在后，顺序是兼容行为的一部分。
- YAML 语法/类型错误和逐条 `key=value` 语义错误被分成不同变体，但 `InvalidAttributesSpec` 展示时直接透传 serde 文本，没有统一的 `Invalid attributes:` 前缀。
- `ErrInvalidAttributesFormat` 与 `Error::InvalidAttributesFormat` 不是同一个可比较错误对象。迁移时不能假设 Rust 支持 Go 的 `errors.Is(err, ErrInvalidAttributesFormat)` 语义；应匹配枚举变体。
- 没有兜底 `Other` 变体。新增错误类别会改变穷尽 `match` 的编译要求，这是有意的类型安全边界，也属于 API 兼容风险。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。错误值只包含拥有所有权的 `String`，其生命周期遵循 Rust 普通所有权：构造时分配/接收字符串，传播或克隆时移动/复制数据，离开作用域后自动释放。

类型本身没有内部可变性。其字段均由 `String` 构成，因而按标准库自动 trait 规则可在线程间移动和共享；不过当前文件没有显式声明并发协议，现有调用链也是同步的属性解析。错误产生不涉及 DDL owner、持久化 job、schema lock 或 PD 请求，重试与事务回滚均不由此模块管理。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/ddl/label/errors.go`、`attributes.go` 和 `rule.go`。

- Go `errors.go` 只定义 `ErrInvalidAttributesFormat = errors.New(...)`；Rust 保留同名字符串常量，同时增加统一的 `Error` 枚举以适配 `Result<T, E>`。
- Go `NewLabel` 使用 `fmt.Errorf("%w: %s", ErrInvalidAttributesFormat, attr)`，支持 `errors.Is` 沿 `%w` 找到 sentinel；Rust 用带 `attribute` 字段的枚举变体表达同一文案，不提供等价 source 链。
- Go `Add` 直接用 `fmt.Errorf` 生成冲突文本；Rust 将新旧标签保存为 `ConflictingAttributes` 的命名字段，最终文案与 Go 一致。
- Go `ApplyAttributesSpec` 直接返回 `yaml.UnmarshalStrict` 的错误；Rust 把 `serde_yaml::Error` 转为字符串后封装成 `InvalidAttributesSpec`。两者都在标签列表构建前终止，但 YAML 库的具体诊断文本不保证逐字一致，Rust 也丢失了底层类型/source。
- Go 上层 `pkg/ddl/executor.go` 会把 `ApplyAttributesSpec` 失败包装为 `dbterror.ErrInvalidAttributesSpec`。当前仓库没有找到对应的 Rust 生产调用接线，因此这里仅能确认 label crate 内部语义对齐，不能声称完整 SQL 错误码路径已经迁移。

## 扩展指南

新增或调整错误时，最小安全改动面通常是：

1. 在 `Error` 中新增语义明确的变体和稳定展示模板；若需要保留底层错误链，应评估 `#[source]` 与现有 `Clone + Eq` 派生是否还能成立，而不是继续把所有错误压成字符串。
2. 在真正检测该条件的 `attributes.rs` 或 `rule.rs` 调用点构造变体，保持“验证完成后才修改集合”的顺序。
3. 在独立测试文件中覆盖变体和状态不变量，不能把测试写进 `errors.rs`。格式/去重/冲突优先扩展 `attributes_test.rs` 或 `migration_aster_unit_test.rs`；AttributesSpec 解析优先扩展 `rule_test.rs` 或迁移测试。
4. 同步核对 Go 的 `errors.go`、`attributes.go`、`rule.go` 及对应 `*_test.go`，明确是保持 Go 文案/行为，还是记录有意差异。若错误最终越过 SQL 边界，还需核查 Rust 是否应接入 `pkg/util/dbterror/ddl_terror.rs::ErrInvalidAttributesSpec`。
5. 不要把展示字符串当作稳定的程序判断接口；Rust 调用者应匹配枚举变体。若外部 crate 需要匹配，需同时评估公开枚举新增变体对穷尽匹配的兼容影响。

主要正确性风险是错误分类或文案顺序漂移；兼容风险是破坏 Go sentinel/SQL 错误码预期；性能风险较低，仅需注意当前冲突路径会分配两个恢复字符串，而 `InvalidAttributesSpec` 会分配底层诊断文本。

## 验证依据

事实依据如下：

- 源文件：`pkg/ddl/label/errors.rs`（常量、枚举、派生和展示模板）。
- 直接生产调用：`pkg/ddl/label/attributes.rs::{NewLabel, NewLabels, Add}`、`pkg/ddl/label/rule.rs::Rule::ApplyAttributesSpec`。
- crate 与门面：`pkg/ddl/label/Cargo.toml`、`pkg/ddl/label/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`；相关依赖 manifest 为 `pkg/ddl/Cargo.toml`、`pkg/executor/Cargo.toml`、`pkg/domain/infosync/Cargo.toml`、`pkg/store/gcworker/Cargo.toml`。
- Go 对照：`pkg/ddl/label/errors.go`、`attributes.go`、`rule.go`；SQL 层包装证据为 `pkg/ddl/executor.go` 对 `dbterror.ErrInvalidAttributesSpec` 的调用。
- Rust 独立测试：`pkg/ddl/label/attributes_test.rs` 覆盖解析、去重和冲突；`rule_test.rs` 覆盖 AttributesSpec 合法/非法输入；`migration_aster_unit_test.rs` 额外断言非法格式文案、冲突文案以及冲突不修改集合。Go 对照测试为 `attributes_test.go` 和 `rule_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ddl/label` 找到 13 个相关文件；`node --file pkg/ddl/label/errors.rs --offset 1 --limit 240` 确认完整 46 行和 2 个符号；`query NewLabel --kind function` 同时定位 Go/Rust 实现。枚举变体查询无结果，故使用精确源码搜索补齐调用边。

本任务是纯文档分析，按计划不运行 Cargo。人工复核确认：文档区分了定义、直接调用与尚未接线的上层行为，没有把 DDL 框架概览写成该文件的实际职责，也没有建议把 Rust 测试内嵌到生产源文件。
