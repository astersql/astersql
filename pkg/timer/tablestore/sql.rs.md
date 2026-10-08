# `pkg/timer/tablestore/sql.rs`

## 文件定位

本文件是 `astersql-timer-tablestore` crate 的 SQL 表达层，位于 Timer API 数据模型与系统表存储执行器之间。模块入口 `pkg/timer/tablestore/lib.rs` 将 `sql` 声明为私有模块并通过 `pub use sql::*` 再导出；`pkg/timer/tablestore/Cargo.toml` 表明该 crate 直接依赖 `astersql-timer-api`，而实际会话执行依赖由相邻的 `store.rs` 通过 `astersql-session-syssession` 承担。

它不打开会话、不启动事务，也不发送 watch 通知；它只负责两件事：把 `api::TimerRecord`、`api::TimerUpdate` 和 `api::Cond` 转成带 `%?` 占位符的 SQL/`SqlArg`，以及把 `TIMER_EXT` 列中的 JSON 与 Rust 内存对象互转。生产调用链见 `pkg/timer/tablestore/store.rs`：`TableTimerStoreCore::{Create,List,Update,Delete}` 分别调用本文件的插入、查询、更新、删除构造器，`decode_timer` 调用 `decode_timer_ext`。

## 核心职责

1. `buildInsertTimerSQL`、`buildSelectTimerSQL`、`buildUpdateTimerSQL`、`buildDeleteTimerSQL` 生成 Timer 系统表 CRUD 语句，并保持 SQL 中占位符与 `Vec<SqlArg>` 的顺序严格一致。
2. `buildCondCriteria` 将 API 层的动态 `Cond` 分派为 `TimerCond` 或 `Operator`，后两者分别由 `buildTimerCondCriteria` 和 `buildOperatorCriteria` 生成叶子谓词与递归布尔表达式。
3. `buildUpdateCriteria` 将 `TimerUpdate` 的“未指定/指定值/显式清空”三态映射为 SET 子句，合并 `TIMER_EXT` 子字段，并无条件追加 `VERSION = VERSION + 1`。
4. `TimerExt`、`ManualRequestObj`、`EventExtObj` 以及编码/解码辅助函数维护 `TIMER_EXT` 的兼容 JSON 布局。手写 `JsonParser` 只实现该列需要的 JSON 子集，并返回 `api::TimerError`，避免给 crate 增加完整 JSON/serde 运行时依赖。
5. `SqlArg` 为会话执行边界提供类型化绑定值；本文件生成 SQL 和参数，但具体到会话层值的转换与执行发生在 `store.rs::executeSQL`。

## 主要符号

- `SqlArg`：SQL 参数联合类型，覆盖 `Null`、字符串、字节、布尔、有符号/无符号整数和已编码 JSON。SQL 构造器通过它显式区分普通字符串与 JSON 参数。
- `TimerExt`：`TIMER_EXT` 根对象，包含 `tags`、可选 `manual` 和可选 `event`。空字段在编码时省略，以对齐 Go 的 `omitempty`。
- `ManualRequestObj` / `EventExtObj`：JSON 中的可空字段布局。`newManualRequestObj`、`newEventExtObj` 从 API 对象压缩默认值，`ToManualRequest`、`ToEventExtra` 反向恢复默认值与时间戳。
- `indentString`：构造反引号限定的 `` `db`.`table` ``。调用者必须保证数据库名和表名来自可信配置；该函数不转义名称内的反引号。
- `buildInsertTimerSQL`：固定 16 列插入，`VERSION` 初值为 1；空 `EventStatus` 归一为 `api::SchedEventIdle`，可选时间通过 `timestamp_argument` 选择 `FROM_UNIXTIME(%?)` 或绑定 NULL。
- `buildSelectTimerSQL`：固定选择 `store.rs::decode_timer` 所需的 19 列，并把条件构造结果拼到 WHERE。
- `buildCondCriteria` / `buildTimerCondCriteria` / `buildOperatorCriteria`：条件树入口、叶子条件和 AND/OR/NOT 递归组合器。
- `buildUpdateTimerSQL` / `buildUpdateCriteria`：按 ID 更新；后者负责字段顺序、显式清空、`JSON_MERGE_PATCH` 和版本递增，前者最后追加 ID 参数。
- `encode_timer_ext` / `decode_timer_ext`：完整扩展对象的 JSON 边界。编码辅助函数包括 `encode_string`、`encode_string_array`、`encode_manual`、`encode_event`。
- `JsonValue` / `JsonParser`：私有轻量 AST 与递归下降解析器；支持 null、布尔、整数、字符串、数组、对象，不支持浮点数或指数形式。
- `buildDeleteTimerSQL`：生成按 ID 删除语句及唯一绑定参数。

## 执行流程

插入路径从 `TableTimerStoreCore::Create` 进入。`buildInsertTimerSQL` 先把 `Watermark`、`EventStart` 转成 Unix 秒参数，补齐默认事件状态，再由 `TimerExt` 聚合 tags、手动请求和事件扩展；`encode_timer_ext` 产生 JSON 文本。函数随后按列顺序返回 INSERT 和 15 个参数，`store.rs::executeSQL` 才负责真正执行，之后 `Create` 查询 `@@last_insert_id` 并通知创建事件。

查询路径从 `TableTimerStoreCore::List` 进入。`buildSelectTimerSQL` 调用 `buildCondCriteria`：空条件或空 `TimerCond` 得到恒真 `1`；ID、namespace、key 按固定顺序生成谓词；`KeyPrefix` 使用 `LIKE value%`；非空 tags 同时要求 JSON 路径存在并用 `JSON_CONTAINS` 检查包含。`Operator` 对每个子条件递归构建并连续追加参数，多子项的非恒值表达式加括号，再用 AND/OR 连接；NOT 对 `1`/`0`直接化简，其余包成 `!(...)`。结果行随后由 `store.rs::decode_timer` 按固定列序读取，并通过 `decode_timer_ext` 恢复扩展字段。

更新路径从 `TableTimerStoreCore::Update` 进入。`store.rs` 先在事务内读取当前事件 ID、版本和调度策略并校验约束，之后 `buildUpdateCriteria` 按稳定顺序收集普通列。`EventStart`/`Watermark` 的外层 `OptionalVal` 未设置时跳过，设置为 `None` 时写 SQL NULL，设置为时间时写 `FROM_UNIXTIME(%?)`。tags/manual/event 先组成按键排序的 `BTreeMap`，再一次性绑定给 `JSON_MERGE_PATCH(TIMER_EXT, %?)`；空值编码成 JSON `null`，利用 merge-patch 删除相应键。末尾必定递增 VERSION，`buildUpdateTimerSQL` 再追加 `WHERE ID = %?` 及 ID 参数。

删除路径最短：`buildDeleteTimerSQL` 返回按 ID 删除的 SQL；`TableTimerStoreCore::Delete` 执行后查询 `ROW_COUNT()`，仅实际删除行时发送删除通知。

JSON 解码从 `decode_timer_ext` 开始：解析一个值，要求根节点为对象且消费全部输入；缺失或 null 的 tags 视为空数组，存在的 tags 必须全部为字符串；manual/event 缺失或 null 视为无对象，否则必须是对象且各字段类型匹配。解析成功后由对象的 `To*` 方法恢复 API 层结构。

## 数据与状态

本文件没有全局可变状态。每次 SQL 构造都新建字符串与参数向量；条件递归通过参数向量的所有权传递保持既有前缀，并按遍历顺序追加新参数。`buildUpdateCriteria` 使用局部 `BTreeMap` 排序 JSON patch 的 `event`、`manual`、`tags` 键，因此输出稳定，便于测试和跨语言比对。

时间在 SQL 边界按 Unix 秒传递。写入时非空值包在 MySQL/TiDB 的 `FROM_UNIXTIME` 中；JSON 扩展里的时间也是整数秒。`timestamp_from_unix` 以 `api::now_timestamp()` 的当前整秒为基准做加减，构造 API 时间戳；负的 manual timeout 不能转换为 `u64`，会在 `ToManualRequest` 中退回默认零时长。

`TimerUpdate` 的三态是重要不变量：未出现的字段不修改，出现的普通空字符串/空字节会写入空值，出现的可选时间 `None` 会写 SQL NULL；空 tags/default manual/default event 在 merge patch 中成为 JSON null，从现有对象删除对应键。`CheckEventID` 与 `CheckVersion` 不生成 SET 字段，它们由 `store.rs::checkUpdateConstraints` 在构造 UPDATE 之前处理。

## 依赖与调用关系

上游生产调用者均在 `pkg/timer/tablestore/store.rs`：

- `TableTimerStoreCore::Create` → `buildInsertTimerSQL` → `executeSQL`；
- `TableTimerStoreCore::List` → `buildSelectTimerSQL` → `buildCondCriteria`，查询行再经 `decode_timer` → `decode_timer_ext`；
- `TableTimerStoreCore::Update` → `buildUpdateTimerSQL` → `buildUpdateCriteria`；
- `TableTimerStoreCore::Delete` → `buildDeleteTimerSQL`；
- `store.rs::decode_timer` 还直接使用 `ManualRequestObj::ToManualRequest`、`EventExtObj::ToEventExtra`。

下游类型依赖只有标准库的 `BTreeMap`、`Duration` 和 `astersql_timer_api`。API crate 提供记录、更新、条件树、时间戳、错误与 `TimerResult`；SQL 的实际提交、事务、会话时区与结果单元格转换不在本文件内。`pkg/timer/tablestore/lib.rs` 的再导出让这些符号可从 crate 根访问，但当前核心生产路径仍是相邻 `store.rs`。

RustCodeGraph 的文件查询确认 `sql.rs` 已索引为 55 个符号，并定位同目录的 `store.rs`、`sql_test.rs`、Go 对照文件。对同名 Go/Rust 函数执行 `query` 时可区分 `sql.rs::buildInsertTimerSQL` 与 `sql.go::buildInsertTimerSQL`；`callers/callees` 查询在本次环境中超时，因此具体调用边另由已索引的 `store.rs` 节点和精确文本引用交叉核验。

## 错误处理与边界

SQL 构造函数返回 `api::TimerResult`，但大多数固定格式构造本身不会失败。明确错误包括：`buildCondCriteria` 收到非 `TimerCond`/`Operator` 的动态条件；`buildOperatorCriteria` 收到空 children；JSON 根不是对象、存在尾随数据、字段类型不符、token/转义/UTF-8/整数非法、数组或对象语法不完整。

`JsonParser` 有意只接受整数；因此即便是合法通用 JSON，浮点数和指数也会被拒绝。字符串解析支持常见转义和 `\uXXXX`，有效高低代理对会组合成补充平面字符，孤立代理项替换为 Unicode replacement character；未转义控制字符与尾随逗号会报错。未知对象字段被保留在临时 AST 中但解码时忽略，这与 Go `encoding/json` 对未知字段的默认兼容方式一致。

`indentString` 只加反引号、不做标识符转义，不适合直接接受用户输入。`KeyPrefix` 直接附加 `%`，没有转义输入值中的 `%` 或 `_`，所以它表达 SQL LIKE 前缀语义而不是逐字节前缀语义。参数值始终走 `%?` 绑定，普通数据不会直接拼入 SQL；固定列名和操作符由代码选择。

## 并发与资源生命周期

本文件全部函数是同步、无锁且无共享可变状态的纯构造/转换逻辑；没有线程、异步任务、通道、会话、事务或外部资源需要关闭。每个 `JsonParser` 只借用一次输入字节串，以 `offset` 单调推进；返回后不保存借用。

并发正确性由输出协议间接参与：`buildUpdateCriteria` 每次更新都追加 `VERSION = VERSION + 1`，但版本匹配与事务隔离由 `TableTimerStoreCore::Update`/`runInTxn` 负责，本文件本身不提供 compare-and-swap。通知器生命周期也由 `store.rs` 和 `notifier.rs` 管理。扩展本文件时不应在 SQL 构造器里取得会话或引入全局缓存，否则会破坏当前可重入、易测试的边界。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/timer/tablestore/sql.go`，测试基线是 `pkg/timer/tablestore/sql_test.go`。Rust 保留了 Go 的函数划分、SQL 文本、列顺序、参数顺序、空状态默认为 IDLE、条件树括号规则、JSON tag 查询、merge-patch 清空语义和每次更新递增 VERSION 的行为。

主要实现差异来自语言边界。Go 使用 `[]any` 与 `json.RawMessage`，Rust 使用 `SqlArg`；Go 的零值 `time.Time` 对应 Rust 的 `Option<Timestamp>`；Go 用 `encoding/json`，Rust 以 `encode_*` 和 `JsonParser` 复现当前 `TIMER_EXT` 所需行为。Rust 的 `BTreeMap` 明确固定 patch 键顺序，而 Go map 的 JSON 编码器同样按键排序产生现有测试期望。Rust 独立测试 `test_timer_ext_json_matches_go_encoding_edges` 额外锁定 Go JSON 对 `<`、`>`、`&`、U+2028/U+2029 的转义、代理对和尾随逗号行为。

一个需保持可见的差异是 Go 的 `buildOperatorCriteria` 对未知 operator 有显式错误分支；当前 Rust `api::OperatorTp` 是只含 AND/OR 的封闭枚举，match 已穷尽，因此无需运行时“未知操作符”分支。若 API 枚举未来扩展，编译器会迫使这里同步处理。

## 扩展指南

新增普通表列时，应同步修改对应 CRUD 构造器、`SqlArg` 参数顺序、`store.rs::decode_timer` 的 SELECT 列下标以及独立测试；不能只修改 SQL 文本。新增可更新字段时在 `buildUpdateCriteria` 中保持“字段片段与参数同时追加”，并覆盖未设置、设置为空、设置为非空三种情况。新增时间列可复用 `timestamp_argument` 的插入语义，但 `append_optional_timestamp` 当前只硬编码 `EVENT_START` 与默认 `WATERMARK`，扩展前应改成不会静默映射错列的明确实现。

新增查询条件时，先在 API crate 的条件模型中定义语义，再扩展 `buildTimerCondCriteria` 或 `buildCondCriteria` 的分派，并在 `pkg/timer/tablestore/sql_test.rs` 添加独立测试，检查已有参数前缀不被覆盖、占位符数量和嵌套优先级。LIKE 条件若要支持字面前缀，需要同时定义转义字符和 SQL `ESCAPE` 规则，不能只转义 Rust 字符串。

扩展 `TIMER_EXT` 时，应同时更新 `TimerExt`、编码、解码、Go 对照结构及边界测试。若引入浮点数、任意 JSON 或更复杂 schema，现有私有 `JsonValue::Number(i64)` 不够；应评估使用通用 JSON 库，而不是让手写解析器逐渐承担完整规范。所有 Rust 测试应继续放在独立的 `pkg/timer/tablestore/sql_test.rs`，不要内嵌进生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含目标；`files --filter pkg/timer/tablestore` 列出 `sql.rs`、`store.rs`、`sql_test.rs` 及 Go 对照；`node --file pkg/timer/tablestore/sql.rs --offset 1 --limit 400` 与后续 400 行节点读取覆盖全部 861 行；`query buildInsertTimerSQL --kind function --json` 区分出 Rust 符号 `sql.rs::buildInsertTimerSQL`。调用图命令在当前环境中未返回，未将其结果冒充证据。
- 源码：`pkg/timer/tablestore/sql.rs`，核对 55 个索引符号、四类 CRUD 构造、条件递归、更新时间三态、JSON 编解码及错误路径。
- crate/模块：`pkg/timer/tablestore/Cargo.toml`、`pkg/timer/tablestore/lib.rs`，核对 crate 名、API 与会话依赖、模块声明和再导出边界。
- 生产调用：`pkg/timer/tablestore/store.rs` 的导入及 `TableTimerStoreCore::{Create,List,Update,Delete}`、`decode_timer`，核对生成结果的真实消费位置。
- Go 对照：`pkg/timer/tablestore/sql.go`、`pkg/timer/tablestore/store.go`，核对 SQL 文本、零值、条件、merge patch、版本与 CRUD 接线。
- 独立测试：`pkg/timer/tablestore/sql_test.rs` 的 `test_build_insert_timer_sql`、`test_build_cond_criteria`、`test_build_select_timer_sql`、`test_build_update_criteria`、`test_build_update_timer_sql`、`test_build_delete_timer_sql`、`test_timer_ext_json_matches_go_encoding_edges`；并对照 `pkg/timer/tablestore/sql_test.go` 的同类测试。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只执行任务规定的 11 章节结构检查，并人工复核本文未把会话、事务、通知等相邻职责归入本文件。
