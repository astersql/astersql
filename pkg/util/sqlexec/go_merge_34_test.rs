// Copyright 2026 AsterSQL.

use crate::{ExecOptionWithSessionVarsSetup, GetExecOption, variable::SessionVars};

#[test]
fn go_merge_34_exec_option_carries_session_vars_setup() {
    let option = GetExecOption(vec![ExecOptionWithSessionVarsSetup(Box::new(|vars| {
        let original = vars.SelectLimit;
        vars.SelectLimit = 5;
        Box::new(move |vars| vars.SelectLimit = original)
    }))]);
    let mut vars = SessionVars::default();
    vars.SelectLimit = 99;
    let restore = option.SessionVarsSetup.unwrap()(&mut vars);
    assert_eq!(vars.SelectLimit, 5);
    restore(&mut vars);
    assert_eq!(vars.SelectLimit, 99);
}
