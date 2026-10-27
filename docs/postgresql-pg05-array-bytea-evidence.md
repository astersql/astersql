# bytea 数组 PostgreSQL 18.6 差分证据

任务29依赖批次28完整提交 `88d2e168611d8a43ccefaee2eebf45498135a3dd`。bytea标量复用 `76d0c59fc10808e2fe4f9b52260c8d80643dbf2d`、`79f5ecec486b22bd76a81a220d17111c1bb62dd8`、`b90fe805f1882d442aa9cb9ad53d08a17cdc6d06`、`a789e9b50e37d98dc7611c360c833a1f060c1ddf` 的现有输入/输出实现。本族元素OID17、数组OID1001、typlen均-1已由精确18.6目录实际核对；目录记录与行为验收分开。

## 实现与范围

规范 `PgArrayElement::Bytea(Vec<u8>)` 接入现有PgArray构造和text/binary分派，保留任意字节、shape、signed lower、元素typmod与元素NULL；外层SQL NULL独立。包括00、ff、非法UTF8原始字节、零长度、完整00–ff序列与ASCII原字节 `\x00`，不借Text(String)或JSON承载。容器的大小/形状/边界检查、整体失败原子性及行主序规则保持。

仅把原server `parameter(17)` 的hex/escape解析与原bytea hex输出最小提取到双方已依赖的 `astersql_types::pg_array::{bytea_text_input,bytea_text_output}`。server标量继续调用同一入口，session数组元素也调用它；binary数组接收委托现有 `binary_parameter(17)`，结果逐字节输出。不复制标量语法，不增加执行内核、外部依赖、features、vendor/third_party或本地patch。

FieldType/Restore、parser的数组类型分派、prepared声明/约束推导与binder、两处共享cast消费者、普通Parse/Bind及结果注册本族。OID17参数参与ARRAY构造时复用现有binary literal cast和字节载体还原函数；不按普通text前缀外观识别字节。新增路径均为本族闭环不可缺少局部接线，已获协调会话补充授权。十个生产Rust文件均同步同目录.rs.md，已有docs-manifest条目，无新生产模块。

跨族数组cast继续明确0A000；本任务不实现完整标量cast矩阵、其他数组族、数组运算、完整存储或目录子系统。合法但未开放的任意bytea表达式/自定义类型路径不在本次承诺内。

## 测试驱动证据

默认features、共享动态slot-1、绝对 `CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。Rust改动后先cargo fmt --all，再运行测试。

`cargo test -p astersql-server --lib pg05_array_bytea -- --nocapture --test-threads=1` 首次编译成功执行2项，标量前置通过，数组Parse因OID1001未开放返回0A000，1通过/1失败、退出101，日志 `/tmp/pg05-bytea-red.log`。最小接线后同命令参数描述为25而非1001，日志 `/tmp/pg05-bytea-wip.log`，补原planning声明白名单及cast约束后通过。不存在把编译错误或零测试当红灯的情况。

`cargo test -p astersql-session --lib pg05_array_bytea_scalar_parameter_constructor -- --nocapture --test-threads=1` 实际1项、退出101，原OID17 binder生成的binary literal cast未被数组分析识别而报0A000；日志 `/tmp/pg05-bytea-constructor-red.log`。沿既有binary_cast_literal_value/binary_runtime_bytes接线后同输入通过，日志 `/tmp/pg05-bytea-constructor-green.log`。带括号参数补充回归在 `/tmp/pg05-bytea-parentheses-red.log` 实际执行1项、退出101，输出内部载体标记内容而非00ffc3285c。沿既有Parentheses规则解包后同一测试通过，并加入恰似内部标记的普通text断言防止按外观误识别。

Python reference-only运行明确 `mode=reference-only, passed=false`，仅校验测试输入和参考诊断，不当作AsterSQL验收。未借用18.4或此前int4/int8证据代替本族当前行为。

## 真实双端差分

`PG18_BIN=/tmp/pg05-task14-reference/install/bin` 当前工具输出PostgreSQL18.6，server_version_num180006；aarch64-apple-darwin25.1.0、Apple clang16.0.0、64位，libpq180000。identity.json记录version/pg_config构建选项、UTF8双端编码、array_nulls=on、standard_conforming_strings=on、bytea_output=hex、DateStyle=ISO, MDY、TimeZone=Asia/Shanghai。

使用CreateAnalyzeSession/ConcreteSessionDriver同Server双协议内存夹具与真实ctypes libpq。本任务脚本独立initdb并启动参考服务；348数组场景包括218合法与130拒绝，覆盖IT→OT、IT→OB、IB→OT、IB→OB、声明/推断/OID0、重复绑定、二维/六维、默认/负下界、空数组、零行、首行NULL、全NULL及外层SQL NULL。成功逐项记录原始输入、参数OID、结果OID/typlen/typmod/format、值及NULL，SQLSTATE=N/A。独立规范值测试另验证元素typmod7完整保留。

20标量场景与20标量参数构造数组场景同样覆盖四方向及全字节、空/NULL和非UTF8原始字节。hex大小写/完整字节间空白、escape八进制及反斜线沿同一标量codec归规范hex输出；数组自身转义层独立。没有把binary中的ASCII `\x00` 当作hex输入再次解码。

348双端TCP场景（80合法portal执行、268拒绝恢复）补证statement/portal Describe、Bind阶段、无DataRow、丢弃直到Sync及恢复。非法输入包括数组词法、非矩形/前缀、非法第二元素、bytea hex/escape、维数/flags/OID、shape乘积/下界溢出、全部头截断、元素长度头/载荷不足、length<-1与尾随垃圾。130项libpq拒绝的18.6 SQLSTATE为08P01×50、22P02×26、22P03×24、54000×18、22023×6、42804×4、22021×2。raw TCP另验证两个含原始NUL的完整text载荷、两种结果格式、双端共8场景，均为Bind 22021；libpq text参数在NUL处截断，故不假称由libpq验证完整NUL。

每次错误后的CREATE消息被丢弃，Sync后SELECT 1恢复，再查询被跳过的表得到42P01，证明无副作用。普通text若恰似内部字节标记仍按text处理。MySQL在同Server前/中/后三次验证JSON/VARCHAR/SQL NULL及列定义/原始行载荷完全一致；bytea原有测试另核对VARBINARY原字节。

Python finally及Rust Drop guard关闭libpq、raw sockets、prepared/portal及双方服务，等待参考子进程后删除自有临时数据库目录；Rust guard删除唯一测试数据库并核验PG/MySQL端口关闭。不触碰用户服务或共享参考工具安装。Cargo阶段以原子mkdir动态领取slot1–10，写plan/task/session/PID，所有Cargo及子进程继承绝对CARGO_TARGET_DIR；正常/失败/中断清理自己的锁，保留编译缓存。

## 最终验证与限制

采用Ready：格式检查、本族跨crate行为、已开放数组聚焦回归、bytea标量和binary结果回归及make lint。verify-profile技能缺失，按根AGENTS/testing-flow执行。仅Rust/测试/说明变更，非新工作区，无Go文件/import/Go测试、依赖或Bazel变化，不触发bazel_prepare；不运行Go单元测试，无failpoint状态变更。未运行bazel_lint_changed、Playground或RealTiKV。

正确性风险限于未覆盖组合；兼容性只承诺记录的18.6/UTF8/hex构建及路径。未验证其他PG版本、32位、escape会话配置的数组输出、任意表达式/完整cast矩阵、持久化/RealTiKV、超大真实分配或性能基准。没有生产性能改动承诺；时间/空间随字节数和元素数增长，既有容器上限检查保留。

最终原始证据：`target/pg05-array-bytea-evidence/82376-63583`，summary.passed=true、mismatches为空，全部Python/Rust清理标志true；MySQL三次原始记录完全相同。下表命令均从仓库根运行，默认features，退出码全部0。共75项有效Rust行为测试，不重复计数；格式/语法/lint不计行为测试。

| 确切命令 | 有效测试数 | 日志 |
| --- | ---: | --- |
| `cargo fmt --all` | 格式化 | `/tmp/pg05-bytea-fmt-final.log` |
| `cargo fmt --all -- --check` | 格式检查 | `/tmp/pg05-bytea-fmt-check.log` |
| `cargo test -p astersql-server --lib pg05_array_bytea -- --nocapture --test-threads=1` | 3 | `/tmp/pg05-bytea-server-final.log` |
| `cargo test -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib pg05_array_bytea -- --nocapture --test-threads=1` | 7（session3/parser1/parser-types1/types2） | `/tmp/pg05-bytea-cross-final.log` |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types -p astersql-types --lib pg05_array -- --skip pg05_array_bytea --nocapture --test-threads=1` | 60（parser4/parser-types4/server13/session13/types26） | `/tmp/pg05-bytea-array-regression.log` |
| `cargo test -p astersql-server --lib pg_scalar_25 -- --nocapture --test-threads=1` | 1 | `/tmp/pg05-bytea-scalar25.log` |
| `cargo test -p astersql-server --lib pg_scalar_26 -- --nocapture --test-threads=1` | 1 | `/tmp/pg05-bytea-scalar26.log` |
| `cargo test -p astersql-server --lib pg_scalar_27 -- --nocapture --test-threads=1` | 2 | `/tmp/pg05-bytea-scalar27.log` |
| `cargo test -p astersql-server --lib pg_binary_native_results -- --nocapture --test-threads=1` | 1 | `/tmp/pg05-bytea-binary-native.log` |
| `make lint` | Go/dashboard检查 | `/tmp/pg05-bytea-lint.log` |
| `PYTHONPYCACHEPREFIX=/tmp/pg05-bytea-pycache python3 -m py_compile pkg/server/testdata/pg05_array_bytea_libpq.py` | Python语法检查 | 退出0 |
| `git diff --check -- pkg docs/postgresql-pg05-array-bytea-evidence.md`、`git diff --cached --check` | 差异检查 | 退出0 |

Cargo与make lint所有阶段实际均动态取得slot-1，绝对CARGO_TARGET_DIR如上；日志保留每次领取、命令和释放记录。最终 `target/rust-slot-locks` 无本任务锁，缓存保留。没有遗留本任务参考数据库、服务或客户端；没有临时上游依赖副本。

交付文件共28个：10个生产Rust、10份对应.rs.md、6个独立Rust测试、1个libpq脚本、本文证据。生产文件为 `pkg/parser/parser_actions/expression.rs`、`pkg/parser/types/field_type.rs`、`pkg/server/pg_extended.rs`、`pkg/server/pg_result.rs`、`pkg/session/runtime/{control,pg_array,planning,query,relational_value}.rs`、`pkg/types/pg_array.rs`；测试为 `pkg/parser/parser_test.rs`、`pkg/parser/types/field_type_test.rs`、`pkg/server/{pg_extended_test,pg_client_integration_test}.rs`、`pkg/session/runtime/pg_array_test.rs`、`pkg/types/pg_array_test.rs`；脚本为 `pkg/server/testdata/pg05_array_bytea_libpq.py`。保留PingCAP许可与AsterSQL版权。只提交本任务文件，排除所有计划文件及原有其他任务变更；plan.md保持只读。
