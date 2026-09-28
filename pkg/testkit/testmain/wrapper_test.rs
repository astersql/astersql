// Copyright 2026 AsterSQL.

use std::cell::Cell;

use super::{TestingM, WrapTestingM};

struct FakeTestingM {
    exit_code: i32,
    runs: Cell<usize>,
}

impl TestingM for FakeTestingM {
    fn run(&self) -> i32 {
        self.runs.set(self.runs.get() + 1);
        self.exit_code
    }
}

#[test]
fn callback_can_mutate_captured_state_across_runs() {
    let runner = FakeTestingM {
        exit_code: 7,
        runs: Cell::new(0),
    };
    let mut callback_runs = 0;

    {
        let wrapped = WrapTestingM(
            &runner,
            Some(Box::new(|exit_code| {
                callback_runs += 1;
                exit_code + callback_runs
            })),
        );

        assert_eq!(wrapped.run(), 8);
        assert_eq!(wrapped.run(), 9);
    }

    assert_eq!(runner.runs.get(), 2);
    assert_eq!(callback_runs, 2);
}
