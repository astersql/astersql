// Copyright 2026 AsterSQL.

use std::path::Path;

#[test]
fn generated_control_files_match_go_generator_outputs() {
    let production = crate::control_vec::generate_dot_go().unwrap();
    let tests = crate::control_vec::generate_test_dot_go().unwrap();

    assert_eq!(
        production,
        include_bytes!("../builtin_control_vec_generated.go"),
        "production output must retain every branch of control_vec.go",
    );
    assert_eq!(
        tests,
        include_bytes!("../builtin_control_vec_generated_test.go"),
        "test output must retain the complete Go test matrix",
    );
}

#[test]
fn generate_one_file_preserves_go_write_order_and_paths() {
    let outputs = crate::control_vec::generate_one_file(Path::new("out/control")).unwrap();
    assert_eq!(outputs[0].0, Path::new("out/control.go"));
    assert_eq!(outputs[1].0, Path::new("out/control_test.go"));
    assert_eq!(outputs[0].1, crate::control_vec::generate_dot_go().unwrap());
    assert_eq!(
        outputs[1].1,
        crate::control_vec::generate_test_dot_go().unwrap()
    );
}
