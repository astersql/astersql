// Copyright 2026 AsterSQL.

use std::cell::Cell;

use super::{TestingM, benchmark_exit_code};

struct FakeTestingM {
    runs: Cell<usize>,
}

impl TestingM for FakeTestingM {
    fn run(&self) -> i32 {
        self.runs.set(self.runs.get() + 1);
        9
    }
}

#[test]
fn benchmark_uses_the_last_repeated_flag_value() {
    let runner = FakeTestingM { runs: Cell::new(0) };

    assert_eq!(
        benchmark_exit_code(
            &runner,
            ["tidb-test", "-test.bench=", "-test.bench=BenchmarkDDL"],
        ),
        Some(9)
    );
    assert_eq!(runner.runs.get(), 1);

    let runner = FakeTestingM { runs: Cell::new(0) };
    assert_eq!(
        benchmark_exit_code(
            &runner,
            ["tidb-test", "-test.bench=BenchmarkDDL", "-test.bench="],
        ),
        None
    );
    assert_eq!(runner.runs.get(), 0);
}

#[test]
fn benchmark_flag_after_argument_or_double_dash_is_not_parsed() {
    for args in [
        ["tidb-test", "suite-name", "-test.bench=BenchmarkDDL"],
        ["tidb-test", "--", "-test.bench=BenchmarkDDL"],
    ] {
        let runner = FakeTestingM { runs: Cell::new(0) };

        assert_eq!(benchmark_exit_code(&runner, args), None);
        assert_eq!(runner.runs.get(), 0);
    }
}

#[test]
fn benchmark_after_another_test_flag_value_is_parsed() {
    let runner = FakeTestingM { runs: Cell::new(0) };

    assert_eq!(
        benchmark_exit_code(
            &runner,
            [
                "tidb-test",
                "-test.run",
                "TestDDL",
                "-test.bench",
                "BenchmarkDDL",
            ],
        ),
        Some(9)
    );
    assert_eq!(runner.runs.get(), 1);
}
