// Copyright 2026 AsterSQL.

// S3 集成测试的二进制入口。
// 具体的导入流程和测试桩由同目录的测试 crate 维护，此处只负责启动统一入口。

fn main() {
    // 复用库入口，避免二进制包装层重复 S3 导入的初始化逻辑。
    astersql_dumpling_tests_s3::main();
}
