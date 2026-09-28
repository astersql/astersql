// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};

use crate::mirror::{DownloadedModule, ListedModule, dump_new_deps_bzl, mirror_with};
use crate::stubs::{Error, MemFs, ScriptedEnv};

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed stdout"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed stdout"))
    }
}

#[test]
fn deps_bzl_output_errors_are_ignored_like_go_fmt_print() {
    let mut listed = HashMap::new();
    let mut downloaded = HashMap::new();
    for (path, version) in [
        ("github.com/grpc-ecosystem/grpc-gateway", "v1.16.0"),
        ("github.com/tikv/pd", "v1.2.3"),
    ] {
        listed.insert(
            path.to_string(),
            ListedModule {
                Path: path.to_string(),
                Version: version.to_string(),
                Replace: None,
            },
        );
        downloaded.insert(
            path.to_string(),
            DownloadedModule {
                Path: path.to_string(),
                Sum: format!("h1:{path}"),
                Version: version.to_string(),
                Zip: format!("/zip/{path}"),
            },
        );
    }

    let bazel = ScriptedEnv {
        runfiles_root: "/runfiles".into(),
        ..ScriptedEnv::default()
    };
    let fs = MemFs::new();
    fs.put("/runfiles/build/patches/com_github_tikv_pd.patch", b"diff");

    let result = dump_new_deps_bzl(&bazel, &fs, &mut FailingWriter, &listed, &downloaded);

    assert!(result.is_ok(), "Go ignores fmt.Print* errors: {result:?}");

    downloaded.remove("github.com/grpc-ecosystem/grpc-gateway");
    let err = dump_new_deps_bzl(&bazel, &fs, &mut FailingWriter, &listed, &downloaded)
        .expect_err("business errors must still be returned");
    assert!(
        err.msg.contains(
            "could not find downloaded module for github.com/grpc-ecosystem/grpc-gateway@v1.16.0"
        ),
        "stdout errors must not mask the later business error: {err:?}"
    );
}

#[test]
fn cleanup_panic_uses_go_error_text_instead_of_rust_debug_fields() {
    let mut bazel = ScriptedEnv::default();
    bazel.runfiles.insert("go.mod".into(), "/ws/go.mod".into());
    bazel.runfiles.insert("go.sum".into(), "/ws/go.sum".into());
    bazel.runfiles.insert("bin/go".into(), "/bin/go".into());
    bazel.list_json = br#"{
        "Path": "example.com/module",
        "Version": "v1.0.0"
    }
    "#
    .to_vec();
    bazel.download_json = br#"{
        "Path": "example.com/module",
        "Version": "v1.0.0",
        "Sum": "h1:sum"
    }
    "#
    .to_vec();

    let fs = MemFs::new();
    fs.put("/ws/go.mod", b"module root\n");
    fs.put("/ws/go.sum", b"sum root\n");
    fs.put("/ws/pkg/parser/go.mod", b"module parser\n");
    fs.put("/ws/pkg/parser/go.sum", b"sum parser\n");
    *fs.fail_remove.borrow_mut() = Some(Error::new("rm failed"));

    let panic = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = mirror_with(&bazel, &bazel, &fs, &mut Vec::new());
    }))
    .expect_err("cleanup failure must panic like Go");

    assert_eq!(panic_message(panic), "rm failed");
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    format!("{payload:?}")
}
