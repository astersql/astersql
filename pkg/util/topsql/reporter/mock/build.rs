// Copyright 2026 AsterSQL.

// TopSQL mock crate 的 build 脚本：编译 `proto/topsql.proto` 生成 tonic 代码。
//
// 使用 vendored protoc，并声明对 proto 文件变更的重新运行条件。

/// 配置 PROTOC 并生成 gRPC server 端代码。
fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    // Cargo executes build scripts serially for this package.
    // 为本包设置 PROTOC，供 tonic_build 调用。
    unsafe { std::env::set_var("PROTOC", protoc) };
    tonic_build::configure()
        .build_server(true)
        .compile_protos(&["proto/topsql.proto"], &["proto"])
        .expect("compile TopSQL mock proto");
    println!("cargo:rerun-if-changed=proto/topsql.proto");
}
