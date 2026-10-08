# [`pkg/types/json_path_expr.rs`](json_path_expr.rs)

## 文件定位

该文件实现一套与 Go `pkg/types/json_path_expr.go` 对齐的 JSON Path 解析器、规范化输出器和进程级解析缓存。它不是由 `pkg/types/lib.rs` 直接声明为普通模块，而是被 `pkg/types/internal/json_path/lib.rs` 通过 `include!("../../json_path_expr.rs")` 编译进独立 crate `astersql-types-json-path`；根 `astersql-types` crate 再以 `pub use types_json_path as json_path` 暴露它。`pkg/types/Cargo.toml` 中的 `types-json-path` 路径依赖和 `pkg/types/internal/json_path/Cargo.toml` 中的 `serde_json` 依赖共同界定了这一编译边界。

生产调用的一条直接证据位于 `pkg/expression/builtin.rs`：`formal_registry` 从 `types_dependency::json_path` 导入 `JSONPathExpression` 与 `ParseJSONPathExpr`，在 `CoreBuiltinKind::JsonExtract` 求值时解析 SQL 参数中的路径。仓库还存在 `pkg/types/internal/json_functions/lib.rs` 自己定义的另一套 `JSONPathExpression`/`ParseJSONPathExpr`，并被 `pkg/expression/builtin_json.rs` 等路径使用；两者类型与缓存互不共享，阅读调用链时不可混为一谈。

## 核心职责

- 将文本路径解析为 `JSONPathExpression { legs, flags }`，支持根 `$`、对象成员、数组单下标、`last[-n]`、闭区间 `start to end`、`*` 和递归下降 `**`（`parseJSONPathExpr`、`parseJSONPathMember`、`parseJSONPathArray`、`parseJSONPathWildcard`）。
- 用 `jsonPathLeg` 和 `jsonPathArraySelection` 保存结构化路径，并用三个标志位快速回答路径是否可能多选（`JSONPathExpression::CouldMatchMultipleValues`）。
- 将结构化路径规范化为字符串，供显示、往返检查和派生路径构造使用（`JSONPathExpression::String`、`jsonPathArrayIndex::String`、`quoteJSONString`）。
- 支持内部 JSON 遍历/修改算法对路径首尾 leg 的拆分，以及追加键或数组选择 leg；这些操作返回新值并维护标志位（`popOneLeg`、`popOneLastLeg`、`pushBackOneArraySelectionLeg`、`pushBackOneKeyLeg`）。
- 将成功解析的表达式缓存到容量 1,000 的全局 LRU 中，并通过克隆隔离缓存值和调用方持有值（`JSONPathExpressionCache`、`path_cache`、`ParseJSONPathExpr`）。失败结果不缓存。

## 主要符号

- `jsonPathArrayIndex = isize`：非负数表示从数组头部计数；负数编码相对末尾的位置，`-1` 表示 `last`，`-n-1` 表示 `last-n`。`JsonPathArrayIndexExt::getIndexFromStart` 结合元素数量解析实际下标，`String` 负责规范化输出。
- `jsonPathArraySelection::{Asterisk, Index, Range}`：数组选择的三种互斥形态；`getIndexRange` 产生闭区间，并把超过数组末尾的结束位置截到 `elem_count - 1`。起点可能仍越界，调用者需用 `start <= end` 判断是否命中，这与 Go 接口约定一致。
- `jsonPathLeg`：用 `typ` 区分键、数组选择和 `**`；数组 payload 放在 `arraySelection`，键 payload 放在 `dotKey`。它只在本文件内部可见。
- `JSONPathExpression`：主要公开数据类型；字段保持私有，对外公开 `CouldMatchMultipleValues`、`String`，并实现 `Display`。派生、拆分和克隆方法当前是 crate 内部实现细节。
- `JSONPathError`：公开解析错误类型，保存字符流位置；`position()` 返回位置，`Display` 生成与 Go 对齐的错误文案。
- `JSONPathExpressionCache`：公开缓存类型，但 `get`、`put` 与内部状态不公开；默认创建空缓存。`PE_CACHE: OnceLock<_>` 和 `path_cache()` 提供进程级单例。
- `jsonPathStream`：基于 `Vec<char>` 的解析游标，负责空白、固定关键字、十进制数字和 `last` 表达式的读取与失败回滚。
- `ParseJSONPathExpr`：本文件唯一的公开解析入口；先查缓存，再调用私有 `parseJSONPathExpr`，仅在成功时写缓存。

## 执行流程

1. 调用者把字符串交给 `ParseJSONPathExpr`。函数以原始字符串（包括空白差异）作为缓存键，通过 `path_cache().get` 查询；命中时取得表达式克隆并返回。
2. 未命中时，`parseJSONPathExpr` 把输入按 Unicode 标量收集到 `jsonPathStream.pathExpr`。它先跳过空白并强制读取 `$`，随后预留 16 个 leg 的容量。
3. 主循环根据当前字符分派：`.` 进入 `parseJSONPathMember`，`[` 进入 `parseJSONPathArray`，`*` 进入 `parseJSONPathWildcard`。每个 leg 后允许空白；其他起始字符立即以当前游标生成 `JSONPathError`。
4. 成员解析接受 `.*`、JSON 引号包裹的键和未加引号的键。引号键先定位结束引号，再由 `decodePathMemberKey` 按 JSON 字符串规则解码；未引号键也先解码 `\uXXXX` 等 JSON 转义，再通过 `isEcmascriptIdentifier` 校验。
5. 数组解析接受 `[*]`、十进制下标、`last`、`last-n`，以及两端由空白和 `to` 分隔的闭区间。`validateIndexRange` 只在两端同号时比较先后；一端从头、一端从尾时，因为缺少实际数组长度而暂时接受。
6. `parseJSONPathWildcard` 只接受恰好两个星号，要求其后仍有字符且第三个字符不是 `*`；总解析结束后还会拒绝以 `**` 结尾的表达式。
7. 解析成功后，公开入口将一个克隆写入 LRU，再把原表达式返回。后续 `String` 从 `$` 开始逐 leg 输出；键名仅在不是 ECMAScript 标识符或含需转义字符时加 JSON 引号。

派生路径时，`pushBackOneArraySelectionLeg` 和 `pushBackOneKeyLeg` 先克隆原表达式，再追加 leg 并增量更新标志；`popOneLeg` 删除首 leg 后遍历剩余 leg 重算标志。`popOneLastLeg` 按 Go 修改路径的前置约束直接令父路径标志为零，因此只应在调用方已排除通配与范围时使用。

## 数据与状态

表达式状态由有序 `legs` 和 `u8` 标志位组成。`jsonPathExpressionContainsAsterisk` 记录 `.*`/`[*]`，`jsonPathExpressionContainsDoubleAsterisk` 记录 `**`，`jsonPathExpressionContainsRange` 记录数组范围；`CouldMatchMultipleValues` 对三者取逻辑或。标志是 legs 的派生缓存，不是独立事实来源，因此删除 leg 后必须调用 `recompute_flags`。

数组相对末尾索引采用负数编码，避免在解析阶段依赖具体 JSON 数组长度。`getIndexRange` 才在消费路径时传入 `elem_count`；空数组的 `[*]` 返回 `(0, -1)`，越界单下标可返回如 `(5, 2)`，这是让上层通过空区间判断“不命中”的有意设计，见 `pkg/types/json_path_expr_test.rs::test_path_boundaries_and_state`。

缓存内部是 `HashMap<String, JSONPathExpression>` 加 `VecDeque<String>`：map 保存值，deque 从最旧到最新维护访问顺序。`get` 会把命中键移到队尾；`put` 会去除旧位置、覆盖 map、追加队尾，并循环淘汰到不超过 `PATH_CACHE_CAPACITY`。全局状态由 `OnceLock` 延迟初始化，进程内持续存在，没有显式清空接口。

## 依赖与调用关系

上游装配链为 `pkg/types/internal/json_path/lib.rs` → `include!("../../json_path_expr.rs")` → `astersql-types-json-path`，再经 `pkg/types/lib.rs::json_path` 再导出。`pkg/expression/Cargo.toml` 把 `astersql-types` 命名为 `types-dependency`；`pkg/expression/builtin.rs::formal_registry` 导入本类型，并在 JSON_EXTRACT 的参数求值循环中调用本入口。这是 RustCodeGraph 对 `ParseJSONPathExpr` 给出的生产调用边之一。

本文件的直接标准库依赖是：`HashMap`/`VecDeque` 实现 LRU，`fmt` 实现错误与表达式显示，`Mutex`/`MutexGuard`/`OnceLock` 管理全局并发缓存。唯一外部库依赖是 `serde_json`，用于 `decodePathMemberKey` 的 JSON 字符串解码；它由 `pkg/types/internal/json_path/Cargo.toml` 声明，根 `pkg/types/Cargo.toml` 也声明了带 `arbitrary_precision`、`preserve_order` feature 的 `serde_json`，但目标文件在独立 json-path crate 中编译时采用前者的依赖配置。

下游语义消费者依赖 `JSONPathExpression` 的结构和多选判断来决定 JSON_EXTRACT 是否包装数组、以及修改类函数是否允许通配/范围。需要注意，`pkg/types/json_binary_functions.rs` 展示的是另一套来自 `types_json_functions` 的路径类型；本文件对应类型的直接生产使用应沿 `types_dependency::json_path` 查询，不能凭同名符号推断跨 crate 类型兼容。

## 错误处理与边界

- 根字符缺失或不是 `$` 时固定报告位置 1；leg 解析失败报告当前 `jsonPathStream.pos`；末尾为 `**` 时报告输入末端位置。位置按 Rust `char` 序列计数，而不是 UTF-8 字节偏移。
- `tryReadIndexNumber` 只接收 ASCII 数字，且数值不能超过 `u32::MAX`；失败会恢复游标。测试验证 `4294967295` 合法、`4294967296` 非法。
- 范围关键字要求分隔空白，故 `[1 to 3]` 合法而 `[1to3]` 非法；同号范围必须有序，异号范围留待获得数组长度后解释。
- 单个 `*` 只能出现在成员或数组语法中；裸路径 leg 的递归下降必须是 `**`，不得是 `***`、不得位于表达式结尾。
- 未加引号键按 Go 的逐 UTF-8 字节分类行为移植：Latin-1 字母、`$`、`_` 和非首位数字可接受，因此 `µ` 可用；西里尔字符等必须加引号。这里不是完整 Unicode ECMAScript IdentifierName 实现。
- `decodePathMemberKey` 特意模拟 Go 对 UTF-16 代理项的行为：合法高低代理对解码为对应字符；某些错误的双代理组合归一化为 U+FFFD；孤立代理仍是错误。测试覆盖了这些分支以及尾部反斜杠的错误位置。
- `read`、`popOneLeg`、`popOneLastLeg` 内部使用直接索引；它们依赖调用前置条件，不能对耗尽流或空 legs 调用。未知 leg 类型和缺失数组 payload 在 `String` 中被视为内部不变量破坏并触发 `unreachable!`。
- 缓存互斥锁中毒时，`lock` 选择 `into_inner()` 继续使用已有状态，不把中毒传播给解析调用者。

## 并发与资源生命周期

`PE_CACHE` 用 `OnceLock` 保证全局缓存只初始化一次；`JSONPathExpressionCache.state` 的每次读取、LRU 刷新、写入和淘汰都在同一把 `Mutex` 下完成，因此 map 与 deque 的一致性由临界区保护。锁不跨解析过程持有：缓存未命中后先释放锁再执行解析，多个线程可能同时解析相同文本并先后覆盖为等价值；这避免了长时间持锁，代价是允许无害的重复计算。

缓存返回 `JSONPathExpression` 的深克隆，公开入口写入时也保存克隆；调用方对自己 legs 的后续派生或内部测试中的变更不会污染缓存。`pkg/types/json_path_expr_test.rs::test_cache_copy_eviction_and_concurrent_access` 用 8 个 scoped 线程重复解析并修改返回值，验证这一隔离，同时验证访问刷新和容量淘汰。

除全局缓存外，解析过程只拥有局部 `Vec<char>`、legs 和字符串，没有文件句柄、异步任务、通道或事务。缓存容量固定为 1,000，但每次 `get`/`put` 都线性扫描 deque 定位已有键；高并发且热点集合较大时，互斥竞争和 O(n) 顺序维护是主要性能风险。

## 与 Go 版本的对应关系

Rust 的 leg 类型码、三个 flag、负数形式的 `last-n` 编码、范围验证、`**` 末尾限制、1,000 容量缓存以及错误文案，均直接对应 `pkg/types/json_path_expr.go`。`pkg/types/json_path_expr_test.go` 的五组核心表（星号检测、合法性、String 往返、追加数组 leg、追加键 leg）在 `pkg/types/json_path_expr_test.rs` 与 `pkg/types/json_path_expr_8_aster_unit_test.rs` 中有对应覆盖。

实现形态上的差异包括：Go 用接口承载三种数组选择，Rust 用 enum；Go 缓存基于 `kvcache.SimpleLRUCache` 和显式 `sync.Mutex`，Rust在本文件中用 `HashMap + VecDeque + Mutex + OnceLock`；Go 的 `ErrInvalidJSONPath` 属于包级错误体系，Rust用独立 `JSONPathError`。Rust `String` 始终是合法 UTF-8，因此 Go `quoteJSONString` 针对非法 UTF-8/RuneError 的修复分支没有对应需求。

Rust 额外测试明确锁定了移植细节：Unicode 转义后的未引号键校验、错误位置、u32 边界、UTF-16 代理项替换、缓存防御性复制和并发访问。当前 Rust 的 `isEcmascriptIdentifier` 有意保留 Go 对 UTF-8 字节逐个转 rune 的现有行为，而不是“修正”为更宽泛的 Unicode 规则；若 Go 后续改变，Rust与测试应同步更新。

## 扩展指南

- 新增 JSON Path 语法时，先扩展 `jsonPathLeg`/`jsonPathArraySelection` 表达能力，再更新 `parseJSONPathExpr` 的分派和具体解析函数、`String` 规范化输出、`recompute_flags` 与 `CouldMatchMultipleValues`。新增多选语法若未进入 flags，会使提取结果形状或修改路径校验出错。
- 修改数组下标/范围规则时，同步检查 `tryReadIndexNumber`、`tryParseArrayIndex`、`validateIndexRange`、三个 `getIndexRange` 实现及负数 `String`；尤其保持“解析时未知数组长度”和“消费时解析实际位置”的边界分工。
- 修改键语法或转义时，必须同时审查 `parseJSONPathMember`、`decodePathMemberKey`、`isEcmascriptIdentifier`、`quoteJSONString`，确保 parse→String→parse 往返及 Go 的 UTF-8/UTF-16 特例不漂移。
- 修改缓存时，保持成功结果才缓存、返回值与缓存值隔离、命中刷新 LRU、容量为 1,000 这四项契约；若引入更高效的数据结构，应继续覆盖锁中毒策略和并发访问。
- Rust 单元测试必须继续放在独立文件中，不要内嵌进本源文件。优先扩展 `pkg/types/json_path_expr_test.rs`；Go 对照表变化时同步检查 `pkg/types/json_path_expr_test.go`，移植契约补充可放入 `pkg/types/json_path_expr_8_aster_unit_test.rs`。
- 因该文件由 `include!` 编译进 `pkg/types/internal/json_path`，新增外部 crate 依赖应写入该子 crate 的 `Cargo.toml`，不能只写根 `pkg/types/Cargo.toml`。若要让新 API 进入 SQL 表达式主链，还需确认 `pkg/types/lib.rs` 再导出和 `pkg/expression/builtin.rs` 的使用点。
- 仓库存在同名的 `pkg/types/internal/json_functions` 路径实现。任何语义扩展都应先确认目标调用链；若两套实现都需要保持一致，应分别修改并分别测试，不能假设改动本文件会自动影响 `builtin_json.rs`/`builtin_json_vec.rs`。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`node --file pkg/types/json_path_expr.rs --offset 1 --limit 500` 与 `--offset 501 --limit 500` 读取了目标文件全部 899 行；`query ParseJSONPathExpr --kind function --limit 20 --json` 区分了本文件、Go 文件及 `internal/json_functions` 的同名符号；`explore "pkg/types/json_path_expr.rs JSONPathExpression PathLeg parse_json_path_expr contains_any_asterisk extract_json"` 给出了 `ParseJSONPathExpr`、flags、push/pop、缓存和 JSON 二进制调用关系。
- 源码与装配：`pkg/types/json_path_expr.rs`、`pkg/types/internal/json_path/lib.rs`、`pkg/types/internal/json_path/Cargo.toml`、`pkg/types/Cargo.toml`、`pkg/types/lib.rs`。
- 直接调用证据：`pkg/expression/Cargo.toml`、`pkg/expression/builtin.rs`；同名但独立实现的边界证据：`pkg/types/internal/json_functions/lib.rs`、`pkg/expression/builtin_json.rs`、`pkg/expression/builtin_json_vec.rs`。
- Rust 测试：`pkg/types/json_path_expr_test.rs`、`pkg/types/json_path_expr_8_aster_unit_test.rs`；邻接集成证据：`pkg/types/migration_aster_unit_test.rs`。
- Go 对照：`pkg/types/json_path_expr.go`、`pkg/types/json_path_expr_test.go`。
- 本任务是纯文档分析，依任务约束未运行 Cargo。最终使用任务指定的 `test -f` 与 11 个固定标题计数命令做结构验证，并人工复核本文能回答文件为何存在、解析如何运行、状态/错误/并发边界以及安全扩展位置。
