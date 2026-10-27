# int4 数组容器 PostgreSQL 18.6 真实差分证据

任务24复用批次23提交 `ddafa21eee86cdcd1cc7d3c719b592aea7cb7d77` 保留的数组接口，仅增加客户端测试与证据，不修改生产语义。初次行为运行已通过，因此没有人为制造生产红灯；首次注册运行曾因脚本路径错误退出101，纠正后才取得行为证据。

## 入口与参考构建

`pkg/server/pg_client_integration_test.rs::pg05_array_shape_libpq_four_directions` 启动 CreateAnalyzeSession/ConcreteSessionDriver 的同Server PostgreSQL/MySQL内存夹具，运行 `pkg/server/testdata/pg05_array_shape_libpq.py`。脚本通过 ctypes 调用真实 libpq 18.0（PQlibVersion=180000），在独立临时目录 initdb，并启动、停止和清理自己的 PostgreSQL 18.6 服务。

本机参考为 `postgres (PostgreSQL) 18.6`，`server_version_num=180006`，aarch64-apple-darwin25.1.0、Apple clang 16.0.0、64位；配置为 without-icu/readline/zlib。`identity.json` 留存完整 version() 与 pg_config --configure。server/client encoding=UTF8，array_nulls=on，standard_conforming_strings=on，DateStyle=ISO, MDY，TimeZone=Asia/Shanghai。成功 SQLSTATE 为 N/A。

## 覆盖与观察

318个libpq场景包含186个合法执行和132个错误执行；每个场景分别断言参考端与AsterSQL，所有差分为空。合法输入覆盖 IT→OT、IT→OB、IB→OT、IB→OB，声明1007、上下文推导及OID0、空集、首行NULL、全NULL、重绑定。结果与 ParameterDescription 持续保留1007，RowDescription为1007/-1/-1，格式随portal选择。

值覆盖默认下界、二维行主序、负下界、int4两端边界、零维空数组、SQL NULL、元素NULL/全NULL、六维上限。额外验证flags=0含NULL、flags=1不含NULL以及零长度轴规范化。没有把目录数组当作普通数组证据。

样例 `[0:1][-2:-1]={{1,NULL},{-3,2147483647}}` 的二进制输入及输出均为：

    000000020000000100000017000000020000000000000002fffffffe
    0000000400000001ffffffff00000004fffffffd000000047fffffff

其中ndim=2、flags=1、元素OID=23，shape=[2,2]、lower=[0,-2]；结果列身份为数组OID1007。SQL NULL输出外层长度-1；`{NULL}`为非NULL数组且内层长度-1；`{}`为12字节的0,0,23头。

错误包含文本词法/矩形/前缀不匹配、非法元素及溢出/UTF8；二进制每个头截断位置、负维数/超维数、非法flags、内建与未知元素OID不符、负维长/乘积/边界溢出、所有元素长度头截断、长度<-1/超过剩余数据、过短/未消费完整标量、尾随垃圾。18.6实际SQLSTATE计数（每种输出格式均执行）如下：

| SQLSTATE | libpq错误场景数 |
| --- | ---: |
| 08P01 | 58 |
| 22P02 | 24 |
| 22P03 | 22 |
| 54000 | 18 |
| 22003 | 4 |
| 42804 | 4 |
| 22021 | 2 |

每项libpq错误后重新执行显式 `SELECT 1::int4 AS v`，断言结果23/4/-1、值1。libpq隐藏的阶段与portal字段由脚本中的真实TCP补证：双端共336个场景，其中72个合法portal Describe/Execute、264个错误与恢复。错误消息序列精确为ErrorResponse、ReadyForQuery；Bind错误后的建表Parse/Bind/Execute均被丢弃，没有DataRow或部分结果。Sync后int4查询恢复，再查被丢弃的表得到42P01，证明没有建表副作用。

同Server MySQL连接在前/中/后三次检查JSON列、普通VARCHAR及SQL NULL，比较原始列定义（245/253）与行载荷，全部一致。此为内存夹具的协议与共享消费者测试，不是RealTiKV或完整持久化验收。

## 验证命令与资源

从仓库根动态领取共享槽位1，所有Cargo及其子进程均继承：

    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1
    PG18_BIN=/tmp/pg05-task14-reference/install/bin

Ready用于交付测试代码，按根AGENTS.md选择，仓库verify-profile技能不存在。无Go/import/依赖/Bazel改动，也不是新工作区；不触发bazel_prepare，不运行bazel_lint_changed。

执行命令及最终结果见下表；默认features，无额外开关。

| 命令 | 退出码 / 有效测试数 |
| --- | --- |
| `cargo fmt --all` | 0 |
| `cargo fmt --all -- --check` | 0 |
| `cargo test -p astersql-server --lib pg05_array_shape_libpq -- --nocapture --test-threads=1` | WIP 0 / 1；Ready 0 / 1 |
| `cargo test -p astersql-server --lib pg05_int4_array -- --nocapture --test-threads=1` | 0 / 2 |
| `cargo test -p astersql-server --lib pg05_array_cast -- --nocapture --test-threads=1` | 0 / 1 |
| `cargo test -p astersql-session --lib pg05_array_cast -- --nocapture --test-threads=1` | 0 / 2 |
| `make lint` | 0 |
| `python3 -m py_compile pkg/server/testdata/pg05_array_shape_libpq.py` | 0 |

Ready共6项有效Rust测试，均0失败。最终原始记录在 `target/pg05-array-shape-evidence/84660-60292`；WIP完整记录在 `target/pg05-array-shape-evidence/83886-59824`，包含 identity.json、cases.jsonl、summary.json、MySQL三次快照与rust-cleanup.json。脚本另有reference-only诊断模式；该模式始终passed=false，不能作为双端通过。正常/异常路径通过Python finally及Rust Drop清理客户端、prepared、portal、临时数据库/服务；服务端口关闭、目录移除均有断言。槽位锁在整个验证阶段结束后释放，缓存保留。

正确性风险限于未覆盖的输入组合；兼容性证据限定上述18.6构建、UTF8和int4数组；性能仅测试小载荷，无基准结论。未验证其他数组族、完整cast矩阵、持久化和RealTiKV。后续继续按批次25逐族开放，不扩大本任务接口。
