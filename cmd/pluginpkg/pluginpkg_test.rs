// Copyright 2026 AsterSQL.

use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};

use serde_json::json;

use crate::pluginpkg::{decode_manifest_toml, execute_code_template, run_with};
use crate::stubs::{Capture, FixedClock, Flags, MemFs, ScriptedRunner};

const SAMPLE_MANIFEST: &str = r#"
name = "conn_ip_example"
kind = "Audit"
description = "test"
version = "1"
license = ""
export = []
"#;

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    String::new()
}

#[test]
fn range_item_fields_do_not_fall_back_to_manifest_root() {
    let mut manifest = decode_manifest_toml(SAMPLE_MANIFEST).unwrap();
    manifest.insert("buildTime".into(), json!("t"));
    manifest.insert("extPoint".into(), json!("RootExtPoint"));
    manifest.insert("impl".into(), json!("RootImpl"));
    manifest.insert("export".into(), json!([{}]));

    let generated = execute_code_template(&manifest).unwrap();

    assert!(generated.contains("<no value>: <no value>,"));
    assert!(!generated.contains("RootExtPoint: RootImpl,"));
}

#[test]
fn template_failure_happens_after_gen_file_creation_and_keeps_partial_output() {
    let fs = MemFs::new();
    let pkg = "/plugins/conn_ip_example";
    fs.put(
        &format!("{pkg}/manifest.toml"),
        SAMPLE_MANIFEST.replace("export = []", "export = \"not iterable\""),
    );
    let runner = ScriptedRunner::new();
    let clock = FixedClock { value: "t".into() };
    let mut log = Capture::new();
    let mut stdout = Capture::new();

    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        run_with(
            Flags {
                pkg_dir: pkg.into(),
                out_dir: "/out".into(),
                ..Flags::default()
            },
            &fs,
            &runner,
            &clock,
            &mut log,
            &mut stdout,
            "pluginpkg",
        );
    }));

    assert!(
        result.is_err(),
        "Go text/template rejects range over a string"
    );
    assert!(
        log.string()
            .contains("generate code failure during generating code")
    );
    assert!(runner.commands().is_empty());
    let partial = fs
        .get_string(&format!("{pkg}/conn_ip_example.gen.go"))
        .expect("Go opens gen.go before template execution and os.Exit skips defer");
    assert!(
        partial.contains("package main"),
        "partial template output: {partial:?}"
    );
}

struct FilePresenceWriter {
    fs: MemFs,
    path: String,
    saw_write_while_present: bool,
}

impl Write for FilePresenceWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.saw_write_while_present |= self.fs.get(&self.path).is_some();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn deferred_remove_runs_after_manifest_output() {
    let fs = MemFs::new();
    let pkg = "/plugins/conn_ip_example";
    let gen_path = format!("{pkg}/conn_ip_example.gen.go");
    fs.put(&format!("{pkg}/manifest.toml"), SAMPLE_MANIFEST);
    let runner = ScriptedRunner::new();
    let clock = FixedClock { value: "t".into() };
    let mut log = Capture::new();
    let mut stdout = FilePresenceWriter {
        fs: fs.clone(),
        path: gen_path.clone(),
        saw_write_while_present: false,
    };

    run_with(
        Flags {
            pkg_dir: pkg.into(),
            out_dir: "/out".into(),
            ..Flags::default()
        },
        &fs,
        &runner,
        &clock,
        &mut log,
        &mut stdout,
        "pluginpkg",
    );

    assert!(
        stdout.saw_write_while_present,
        "Go defer runs only when main returns"
    );
    assert!(fs.get(&gen_path).is_none());
}

#[test]
fn non_string_manifest_name_panics_instead_of_using_controlled_exit() {
    let fs = MemFs::new();
    let pkg = "/plugins/conn_ip_example";
    fs.put(
        &format!("{pkg}/manifest.toml"),
        SAMPLE_MANIFEST.replace("name = \"conn_ip_example\"", "name = 1"),
    );
    let runner = ScriptedRunner::new();
    let clock = FixedClock { value: "t".into() };
    let mut log = Capture::new();
    let mut stdout = Capture::new();

    let payload = panic::catch_unwind(AssertUnwindSafe(|| {
        run_with(
            Flags {
                pkg_dir: pkg.into(),
                out_dir: "/out".into(),
                ..Flags::default()
            },
            &fs,
            &runner,
            &clock,
            &mut log,
            &mut stdout,
            "pluginpkg",
        );
    }))
    .expect_err("Go manifest[\"name\"].(string) panics");

    let message = panic_message(payload);
    assert!(message.contains("interface conversion"), "panic: {message}");
    assert!(!log.string().contains("plugin package must be same"));
}
