# PostgreSQL 行构造消费者与复合类型边界

任务43复用 `ExplicitRow` / `ImplicitRow`、`record_type_in_context` 和 `value_with`，只处理已有直接投影和表达式消费者。依赖42提交为 `f3237799861b33b1ef2278aef2dfd277608cd957`；它的数组codec和18.6证据见 [jsonb数组记录](postgresql-pg05-array-jsonb-evidence.md)。本次没有新增复合类型子系统、外部依赖或标量/数组codec。

## 已接通的边界

`ROW(1)` 是一个字段的匿名 record，`(1)` 保持整数分组；`ROW(NULL)` 是非NULL record，包含OID705的NULL字段。`ROW(1,ROW(2,NULL))` 的外部字段OID为23、2249，内部为23、705，文本为 `(1,"(2,)")`。字段顺序、嵌套布局和NULL由独立类型信息与规范文本载体共同保留，不能仅凭字符串判断类型。

本计划的新族通过已有显式转换、已定型参数及标量子查询进入record：UUID2950、json114、jsonb3802，以及19个已开放数组族。json保留原文空白与数值拼写，jsonb保留规范键序、精确数字和二进制版本01。数组字段保留数组OID、形状、下界及外层/元素NULL；`NULL::jsonb[]` 不会变成空数组或含JSON null的数组。所有数组族覆盖typed NULL与无结果行时的稳定布局，四方向的具体非NULL样本覆盖int4[]和jsonb[]，不把这两个样本冒称所有数组族的record四方向证据。

参数消费沿真实PG扩展协议：`SELECT ROW($2,$1,$2)` 的参数描述仍有两个原始参数，绑定位置交换及重复字段的顺序持续保留；内部session接口使用三个已适配的 `?` 标记。协议测试使用CreateAnalyzeSession、ConcreteSessionDriver及PgService，实际发送Parse、statement Describe、Bind、portal Describe、Execute、Sync，断言原始DataRow与record内逐字段OID/长度/载荷，并覆盖text/binary输入×text/binary输出及NULL重绑定。出口OID2249、typlen=-1、typmod=-1与请求格式逐项断言。

## 范围外缺口及交接

以下为合法但当前未开放的路径，禁止以旧传输字符串或标量回退声称已支持。拥有者指建议接续的组件，不表示本任务已扩建它。

| 路径/可复现输入 | 当前行为与缺口 | 建议拥有者 |
| --- | --- | --- |
| `SELECT ROW(1)=ROW(1)` | 分析阶段0A000；不实现行比较及三值逻辑 | 后续表达式/行比较任务 |
| `SELECT ARRAY[ROW(1)]` | 分析阶段0A000；不实现record数组及元素复合布局 | 数组/复合类型任务 |
| `SELECT ROW(ARRAY[1,NULL])` | 既有ARRAY直接投影门禁返回0A000；可使用 `ROW('{1,NULL}'::int4[])` 或已定型数组参数。本任务没有改变ARRAY上下文资格 | session pg_array分析任务 |
| 扩展Parse `SELECT ROW($1)`，参数OID2249 | 分析阶段0A000；匿名record入参不属于既有标量/数组绑定器 | 参数/复合类型任务 |
| `SELECT (ROW(1))` | 当前record直接投影门禁0A000；普通标量 `(1)` 仍可用 | 行构造语法/上下文任务 |

命名复合类型、CREATE TYPE、复合类型表列、record持久化与完整隐式转换矩阵均范围外；本次没有对这些语法入口采集新的参考诊断，也不声称它们当前全部已返回0A000。零字段ROW、rowvalue.*、非本次覆盖的标量字段和会话显示设置另需专门验证；不从当前成功样本推导支持。现有SQL字符串反斜杠词法缺口仍由原任务负责。

非法UUID字段 `ROW('bad'::uuid)` 在求值阶段保持22P02，拒绝结果后同session执行 `ROW(1)` 正常。两项合法未开放操作在TCP测试中均无DataRow，后续SELECT1恢复。测试只使用内存分析会话；无Playground、用户数据库或持久化服务变更。自有参考服务均停止并删除临时数据库目录，TCP客户端/测试服务通过生命周期清理。

## PostgreSQL 18.6参考与原始观测

参考二进制 `/tmp/pg05-task14-reference/install/bin` 的 `postgres --version` 与SQL版本均为 PostgreSQL18.6，aarch64-apple-darwin25.1.0、Apple clang16、64bit；临时集群UTF8、无locale、trust、本地自有socket，`server_version_num=180006`。

`SELECT row(1)::text,encode(record_send(row(1)),'hex')` 结果为 `(1)`、`00000001000000170000000400000001`。
`row(NULL)` 为 `()`、`00000001000002c1ffffffff`，证明NULL字段仍有OID705且长度-1。
`row(1,row(2,NULL))` 为 `(1,"(2,)")`、`00000002000000170000000400000001000008c90000001800000002000000170000000400000002000002c1ffffffff`。
这些独立采样字节直接用作Rust TCP测试夹具，没有调用生产codec构造期望。

参考另用libpq对UUID、json、jsonb、int4[]、jsonb[]执行上述重复参数SQL，逐族覆盖四种格式组合和两个NULL位置，共40个场景。记录原始十六进制载荷、元数据与成功SQLSTATE N/A，断言OID2249/-1/-1/请求格式。与本地TCP使用同一组独立载荷；jsonb[] `{null,NULL}` 的未引用null会成为SQL NULL，测试使用 `{"null",NULL}` 区分JSON null与SQL NULL。参考与本地各自验证真实返回数据，不以历史18.4或仅编译替代验收。

当前临时原始日志：`/tmp/pg05-record-reference.log`（文本/record_send），`/tmp/pg05-record-ref-directions.log`（40项libpq观测）。采样命令为 `/tmp/pg05-record-reference.sh` 和 `python3 /tmp/pg05-record-ref-directions.py`，脚本使用仓库既有libpq API声明，全部退出0并清理自有临时集群；这些临时路径是本次运行证据位置，长期复现应运行已提交Rust测试及上文SQL。

## 验证与风险

Ready按本任务相关Rust/文档范围执行。所有Cargo阶段均经 `/tmp/pg05-record-cargo.sh` 从1至10原子mkdir动态领取，共同实际使用slot-1；`CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。owner记录task43/session/PID，EXIT/INT/TERM清理自有锁，子进程结束才释放；所有阶段均已释放slot-1，保留构建缓存。

| 仓库根确切命令 | 退出码/有效测试数 |
| --- | --- |
| `cargo fmt --all` | 0；Rust修改后先执行 |
| `cargo fmt --all -- --check` | 0 |
| `cargo test -p astersql-session --lib pg05_record_consumer -- --nocapture --test-threads=1` | 原实现101：6项中5失败1通过；最终0：6项全通过 |
| `cargo test -p astersql-session --lib pg_dialect_11 -- --nocapture --test-threads=1` | 0：2项通过 |
| `cargo test -p astersql-session --lib pg_dialect_12 -- --nocapture --test-threads=1` | 0：1项通过 |
| `cargo test -p astersql-server --lib pg05_record_consumer -- --nocapture --test-threads=1` | 0：1项通过，含40组四方向/NULL重绑定及10组直接/嵌套record结果格式断言、4组简单协议拒绝/恢复和1组扩展Parse拒绝/Sync恢复 |
| `make lint` | 0 |
| `git diff --check -- pkg docs` | 0 |

最终相关验收为10项Rust测试（session6+2+1、server1），不把TCP场景数当Rust测试数。日志 `/tmp/pg05-record-accepted.log`；红灯 `/tmp/pg05-record-red-final.log`；lint `/tmp/pg05-record-lint.log`。参考40项与本地40项独立四方向结果均满足相同值/字节/元数据断言。

宽筛选 `cargo test -p astersql-session --lib pg_record_test -- --nocapture --test-threads=1` 退出101，12项中11过1失败；未改动的 `pg_dialect_26_escape_result_type` 报既有“PostgreSQL ESCAPE expression evaluation is unsupported”。暂用HEAD原生产pg_record.rs复核 `cargo test -p astersql-session --lib pg_dialect_26_escape_result_type -- --nocapture --test-threads=1` 同样退出101、1项失败（日志 `/tmp/pg05-record-baseline-escape.log`），随后恢复本任务生产文件并取得上表最终通过证据。该ESCAPE功能不消费record，列为原表达式任务范围外建议；宽筛选不报告全绿，不递归修复或缩减原测试。旧server pg_dialect_11/12外部双参考总套件未运行，本次重跑原session11/12行为并用18.6采样及真实TCP新测试补出口验收。

正确性和兼容性承诺限定上面的既有消费者与样本；不覆盖完整复合类型或所有数组族的非NULL四方向、持久化/RealTiKV及其他平台。沿现有字段遍历、元数据布局和codec复用，没有改变标量算法；未做吞吐、超宽record或内存基准。没有Go/Bazel/依赖改动，不触发bazel_prepare；不运行bazel_lint_changed。verify-profile技能当前缺失，Ready依据根AGENTS.md和testing-flow执行，不能报告该技能通过。

本次交付文件：`pkg/session/runtime/pg_record.rs`、同目录`pg_record_test.rs`和`pg_record.rs.md`、`pkg/server/pg_types_test.rs`、`docs/docs-manifest.md`及本文。自审只含record身份局部接线、独立测试和边界说明；其余任务记录与只读plan.md未纳入提交。
