// Copyright 2026 AsterSQL.

// RawKV 备份集成测试的二进制入口。
//
// 这里只把进程控制权交给同名库包，由库入口统一完成参数解析、场景执行和错误处理，
// 避免二进制包装层与可测试的库逻辑产生行为差异。

fn main() {
    astersql_br_tests_br_rawkv::main();
}
