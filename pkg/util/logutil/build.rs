// Copyright 2026 AsterSQL.

// Stable Rust does not expose libtest's test list to a TestMain wrapper. Keep
// the existing #[test] sources authoritative and register their unchanged
// bodies with the custom runner. Unsupported attributes fail the build rather
// than silently dropping a test or changing its contract.
use quote::{format_ident, quote};
use std::{env, fs, path::PathBuf};
use syn::{Item, ReturnType};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.");
    let mut files: Vec<_> = fs::read_dir(".")
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with("_test.rs"))
        .collect();
    files.sort();
    let mut modules = Vec::new();
    let mut notices = Vec::new();
    let mut trials = Vec::new();
    for path in files {
        let name = path.file_stem().unwrap().to_str().unwrap();
        let module = format_ident!("{name}");
        let source = fs::read_to_string(&path).unwrap();
        notices.push(
            source
                .lines()
                .take_while(|line| line.starts_with("//") || line.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let mut parsed = syn::parse_file(&source).unwrap();
        for item in &mut parsed.items {
            let Item::Fn(function) = item else { continue };
            if !function.attrs.iter().any(|a| a.path().is_ident("test")) {
                continue;
            }
            for attribute in &function.attrs {
                assert!(
                    attribute.path().is_ident("test") || attribute.path().is_ident("doc"),
                    "unsupported test attribute in {}: {}",
                    path.display(),
                    function.sig.ident
                );
            }
            assert!(
                matches!(function.sig.output, ReturnType::Default),
                "add Result-returning test support before registering {}",
                function.sig.ident
            );
            function.attrs.retain(|a| !a.path().is_ident("test"));
            function.vis = syn::parse_quote!(pub(super));
            let ident = &function.sig.ident;
            let label = format!("{name}::{ident}");
            trials.push(quote! {
                libtest_mimic::Trial::test(#label, || { #module::#ident(); Ok(()) })
            });
        }
        // Keep source license comments in generated files as well.
        modules.push(quote! { mod #module { #parsed } });
    }
    let generated = quote! {
        #(#modules)*
        fn registered_tests() -> Vec<libtest_mimic::Trial> { vec![#(#trials),*] }
    };
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("registered_tests.rs"),
        format!(
            "// Generated from adjacent test files.\n{}\n{generated}",
            notices.join("\n")
        ),
    )
    .unwrap();
}
