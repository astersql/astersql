# JSONB 真实客户端四方向差分证据

2026-10-27，批次16，已完成。依赖批次15提交
`c809242939b5492f3dd31e8a2788c24d7b4d7c8f`（结果编码、NULL与portal），
其本地3项通过记录已核对；批次14入参来自
`5df1642b66827604e3dc69d3ef927da8f3f17aeb`。
本批只补真实客户端测试、独立Python脚本和证据，无生产变更。

## 运行方式与断言

Rust `pg05_jsonb_libpq_four_directions` 启动真实共享执行消费者
CreateAnalyzeSession/ConcreteSessionDriver及同Server双协议端口，Python通过真实
libpq执行PQprepare、PQdescribePrepared、PQexecPrepared、PQclosePrepared。
脚本严格要求server_version_num=180006，不跳过或降级为18.4。

每一合法值与SQL NULL覆盖IT→OT、IT→OB、IB→OT、IB→OB。
输入输出记录原始十六进制载荷、参数OID、结果OID/typlen/typmod/格式和NULL。
参数3802，结果3802/-1/-1；binary为01加规范JSON文本，SQL NULL没有版本字节。
成功SQLSTATE写N/A，缺字段不会默认相等。

| 验收族 | 当前断言 |
| --- | --- |
| JSONB-DUP-ORDER | 最后重复键、解码后同键、嵌套重复、不同输入顺序及固定构建键序 |
| JSONB-NUMBER | 超u64精确整数、高精度小数、负零比例、尾零、指数展开、最大整数位/小数位及相邻越域 |
| JSONB-SCALAR-NEST | 全部根种类、空对象/数组、数组顺序与重复、嵌套NULL/对象/数组、256层合法嵌套 |
| JSONB-UNICODE | 直接中文/emoji、Unicode转义、正确代理对、非法/孤立代理、u0000、UTF8和binary原始NUL |
| JSONB-NULL-METADATA | SQL NULL/JSON null/字符串null、声明/推导/OID0、重绑定、空集、首行NULL及全NULL |
| JSONB-WIRE-INVALID | 缺版本08P01、仅版本22P02、00/02/ff实际XX000、垃圾/词法22P02、逐次Sync恢复 |
| JSONB-CAST-BRIDGE | text/json→jsonb重新校验，jsonb→json/text只得规范文本，同一输入的原文json仍保持原字节，转换失败后恢复 |

拒绝请求都是SELECT，无DML副作用，不返回DataRow。输入解码失败阶段为Bind，
显式桥接语义失败阶段为Execute。libpq不暴露portal字段与错误前帧序列；本轮另跑
既有真实TCP `pg05_jsonb_input_parse_bind_execute_and_recovery` 和
`pg05_jsonb_output_portal_formats_rebinding_and_empty_rows`，验证缺载荷帧08P01、
忽略错误后的Execute直至Sync、portal Describe/混合格式/重复Execute及稳定元数据。
受控非法内部身份夹具由本轮 `pg05_jsonb_output_invalid_carrier_writes_no_messages` 补证。
不支持MySQL扩展桥接依前批独立接口拒绝，不新增隐式转换或完整桥接子系统。

同Server MySQL原协议在PG请求前、合法/非法请求之间、请求后比较JSON(245)、
VARCHAR(253)、JSON null、字符串null及SQL NULL的实际值、原始列与行载荷。

## 精确参考与实际诊断

工具 `/tmp/pg05-task14-reference/install/bin`，本轮SQL version()返回
PostgreSQL 18.6 on aarch64-apple-darwin25.1.0，Apple clang 16.0.0，64-bit；
server_version_num=180006，libpq=180000，数据库/客户端UTF8，
standard_conforming_strings=on，DateStyle=ISO, MDY，TimeZone=Asia/Shanghai。
pg_config --configure完整结果保存在每次identity.json。
排序字节仅绑定此构建，不承诺跨版本存储格式。

首次参考运行揭示规划预期孤立代理22P05不准确，实际22P02；u0000实际22P05。
修正测试预期后，参考与Aster一致（批次14已经修正实现）。未知版本00/02/ff
实采XX000，没有猜成22P03或沿用临时0A000。非法词法22P02、numeric越域22003、
非法编码22021。首次Aster双端运行352项全部通过，属于已有正确实现补覆盖，
未制造无意义生产红灯。显式转换扩充后440项双端通过，另6项Server测试通过。

## 资源与验证边界

每次initdb自身TemporaryDirectory并领取空闲端口，finally关闭libpq/prepared、
停止并等待自身参考进程、验证端口关闭、删除临时集群；SIGTERM转换为异常走清理。
Rust guard释放检查点stdin并等待Python finally，关闭两协议端口并删除唯一数据库。
不操作用户服务，不启动Playground，不宣称RealTiKV或持久化通过。
Cargo阶段经Python原子mkdir依次尝试共享1–10槽位，绝对CARGO_TARGET_DIR继承给子进程，
结束后unlink自有owner并rmdir锁，保留所有缓存。未改Cargo manifest、依赖或features。

## Ready检查

最终差分目录 `target/pg05-jsonb-evidence/78728-54665/` 保存identity.json、
cases.jsonl、summary.json、mysql-before/during/after.json与rust-cleanup.json。
472项严格双端断言全部通过；20条statement Describe记录另计。
其中104项值/SQL NULL四方向、32项NULL形状/推导、40项显式桥接成功、
48项转换拒绝/恢复、248项输入拒绝/恢复；全部清理标志true。

| 确切验证命令 | 退出码 / 有效测试 |
| --- | --- |
| `python3 /tmp/pg05-task16-slot.py cargo fmt --all` | 0；每次Rust修改后先执行 |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin python3 /tmp/pg05-task16-slot.py cargo test -p astersql-server --lib pg05_jsonb_libpq -- --nocapture --test-threads=1` | 首次352断言、最终472断言均0；各1项Rust通过 |
| `PG18_BIN=/tmp/pg05-task14-reference/install/bin python3 /tmp/pg05-task16-slot.py cargo test -p astersql-server --lib pg05_jsonb -- --nocapture --test-threads=1` | 0；7项通过，其中差分440项；其余6项未被后续纯客户端扩充影响 |
| `python3 /tmp/pg05-task16-slot.py cargo fmt --all -- --check` | 0 |
| `make lint` | 0 |
| `git diff --check`、`git diff --cached --check` | 0 |

所有构建、格式化和测试动态取得slot-1，绝对CARGO_TARGET_DIR为
`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`；
最终格式检查与测试重叠时动态取得slot-2，绝对CARGO_TARGET_DIR为
`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`。
两个自有锁已释放，缓存保留；日志位于/tmp/pg05-task16-{red,green,final,fmt,fmt-check,lint}.log。
red日志实际是首次通过证据，不是行为失败。

参考诊断命令为 `PG18_BIN=/tmp/pg05-task14-reference/install/bin python3 pkg/server/testdata/pg05_jsonb_libpq.py --reference-only unused target/pg05-jsonb-evidence/reference-initial`，首次异常断言退出1（孤立代理实际22P02）；修正后reference-corrected和reference-bridges同命令分别退出0、352/440项通过，均passed=false，不冒充双端验收。

所有命令从仓库根执行；
`/tmp/pg05-task16-slot.py` 是本轮槽位包装器，无额外构建目录。
无Go/Bazel修改或其他prepare触发，不运行bazel_prepare；没有Go单测/failpoint操作，
未运行bazel_lint_changed。Ready技能目录缺失，按根AGENTS及testing-flow执行。

正确性/兼容性：本批无生产行为变化，断言绑定指定18.6与UTF8；其他编码、
不同构建、超256层资源极限、完整隐式转换、持久化和RealTiKV未验证。
性能：新增测试无线上开销，未做基准。下一步按计划推进批次17；前批待回归
由协调会话处理，不把本次子集对照扩称此前所有任务完成。
