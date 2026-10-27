# text 数组 PostgreSQL 18.6 差分证据

任务25接续批次24提交 `927a9e893d7abc225e452969ab9bd382493ba7f3`，只开放text数组（元素OID25、数组OID1009）。沿现有PgArray与共享prepared/query消费者注册本族，没有移植标量子系统或改动外部依赖。参考pg_type实查25/0/-1与1009/25/-1（oid/typelem/typlen），不以目录结果代替普通SQL支持。

## 实现与行为

解析器PgArrayType注册text身份，FieldType还原TEXT[]；control/planning保留声明、推断与OID0上下文的1009，query/relational_value委派既有数组求值。规范容器保留维度、下界、元素身份、typmod=-1、元素NULL和外层SQL NULL。Bind逐元素调用既有parameter(25)/binary_parameter(25)，结果沿同一规范容器输出。共享构造调用pg_text_cast，非法元素拒绝整个数组；跨text与整数数组cast仍为0A000，其他数组族未开放。

首次新增TCP测试可编译且执行1项，在Parse OID1009收到0A000而非ParseComplete，退出101，日志`/tmp/pg05-text-red.log`。实施后同断言通过。首次差分误用默认参考18.4，版本门禁按预期拒绝，未计为18.6证据；显式PG18_BIN后WIP与最终Ready差分均通过。

## 参考与实际覆盖

本机精确PostgreSQL18.6/180006，aarch64-apple-darwin25.1.0、Apple clang16.0.0、64位，without-icu/readline/zlib；真实libpq18.0/PQlibVersion180000。UTF8客户端及服务端编码，array_nulls=on、standard_conforming_strings=on、DateStyle=ISO, MDY、TimeZone=Asia/Shanghai。原始identity.json保留version()、pg_config配置及目录OID实查。

`pg05_array_text_libpq_four_directions`启动CreateAnalyzeSession/ConcreteSessionDriver同Server双协议内存夹具，Python ctypes调用真实libpq，并独立initdb/启动/停止本任务参考集群。最终原始记录为`target/pg05-array-text-evidence/1864-63262`（identity.json、cases.jsonl、summary.json、MySQL三次快照、rust-cleanup.json）。

364个libpq场景含246成功、118拒绝；所有合法载荷覆盖IT→OT、IT→OB、IB→OT、IB→OB，声明/推断/OID0、空集、首行NULL及重绑定。12类载荷覆盖二维行主序、默认/负下界、六维、空数组、SQL NULL、元素NULL/全NULL，以及NULL字样、空串、逗号、引号、反斜线、Unicode、组合字符和花括号。原始输入/输出为十六进制，参数及结果持续1009，结果typlen/typmod=-1/-1，格式独立选择。成功SQLSTATE为N/A。flags非规范输入规范化输出。

双端332个TCP场景含96个合法portal执行与236个错误及恢复，补证libpq未暴露的statement/portal Describe元数据与Bind阶段。拒绝项包括文本词法/矩形/前缀、UTF8、维数/flags/OID、shape乘积和下界溢出、所有头截断位置、长度头/载荷不足、长度<-1、尾随字节以及先合法元素再非法UTF8或NUL元素。SQLSTATE由18.6实际采集，双端比较无差异：

| SQLSTATE | libpq拒绝场景数 |
| --- | ---: |
| 22P02 | 18 |
| 22021 | 8 |
| 54000 | 18 |
| 22P03 | 20 |
| 42804 | 4 |
| 08P01 | 50 |

每次拒绝无DataRow，TCP精确断言ErrorResponse→ReadyForQuery；错误后流水线建表被丢弃，Sync后SELECT 1::int4恢复，再查询被丢弃的表得到42P01，证明无副作用。所有差异为空。MySQL前/中/后三次JSON/VARCHAR/SQL NULL原始值、列定义与行载荷一致。该夹具不代表RealTiKV或完整持久化支持。

Python finally及Rust guards清理prepared、portal、客户端、本任务服务和数据库，等待进程退出；最终reference_directory_removed/reference_port_closed/clients_closed/raw_clients_closed与Rust数据库删除、双端口关闭全部为true。

## 验证与边界

验证采用Ready，因交付Rust代码执行格式、跨crate聚焦行为回归和make lint。仓库verify-profile技能不存在，直接遵守根AGENTS与testing-flow。未改Go/import/依赖/Bazel且非新工作区，不触发bazel_prepare；不运行bazel_lint_changed或Playground。

全部Cargo阶段动态原子领取slot-1，继承绝对路径：

    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1
    PG18_BIN=/tmp/pg05-task14-reference/install/bin

阶段结束用trap删除自己的owner与锁目录，保留缓存。命令及最终结果见以下交付记录。

正确性风险限于未覆盖输入组合；兼容性仅承诺此18.6构建和UTF8配置，不承诺其他数组族、完整cast矩阵或存储。性能为小载荷行为检查，无吞吐、超大分配或性能基准结论；RealTiKV、32位平台和持久化未验证。

| 最终命令（仓库根目录、默认features） | 退出码 / 有效测试数 |
| --- | --- |
| `cargo fmt --all` | 0 |
| `cargo fmt --all -- --check` | 0 |
| `cargo test -p astersql-server --lib pg05_array_text -- --nocapture --test-threads=1` | 0 / 2 |
| `cargo test -p astersql-parser --lib pg05_array_text -- --nocapture --test-threads=1` | 0 / 1 |
| `cargo test -p astersql-parser-types --lib pg05_array_text -- --nocapture --test-threads=1` | 0 / 1 |
| `cargo test -p astersql-server --lib pg_scalar_21 -- --nocapture --test-threads=1` | 0 / 1 |
| `make lint` | 0 |
| `python3 -m py_compile pkg/server/testdata/pg05_array_text_libpq.py` | 0 |
| `git diff --check` | 0 |
| `cargo test -p astersql-session --lib pg05_array_text -- --nocapture --test-threads=1` | 0 / 2 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser -p astersql-parser-types --lib pg05_int4_array -- --nocapture --test-threads=1` | 0 / 8 |
| `cargo test -p astersql-server -p astersql-session -p astersql-parser --lib pg05_array_cast -- --nocapture --test-threads=1` | 0 / 4 |

新增runtime测试曾因误用query方法未编译，修正为既有execute后补跑session与受影响回归，全部通过；不把编译失败算作行为红灯。make lint检查Go及dashboard，测试方法修正不影响其覆盖，按根规则复用该通过证据。

目录text数组结果路径受本次规范编码器分派影响，额外运行 `cargo test -p astersql-server --lib pg_introspection_parameters_array_metadata -- --nocapture --test-threads=1`，退出0、1项通过。最终聚焦Rust测试合计20项，0失败；格式检查、Python语法检查和make lint均退出0。
