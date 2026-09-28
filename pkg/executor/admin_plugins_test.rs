// Copyright 2026 AsterSQL.

use crate::admin_plugins::{AdminPluginsAction, AdminPluginsExec, PluginFlagFlusher};

#[derive(Default)]
struct RecordingFlusher {
    calls: Vec<(String, bool)>,
    fail_on: Option<String>,
}

impl PluginFlagFlusher for RecordingFlusher {
    type Error = &'static str;

    fn change_disable_flag_and_flush(
        &mut self,
        plugin_name: &str,
        disabled: bool,
    ) -> Result<(), Self::Error> {
        self.calls.push((plugin_name.to_owned(), disabled));
        if self.fail_on.as_deref() == Some(plugin_name) {
            Err("flush failed")
        } else {
            Ok(())
        }
    }
}

#[test]
fn enable_clears_the_flag_for_every_plugin_in_order() {
    let mut exec = AdminPluginsExec {
        BaseExecutor: (),
        Action: AdminPluginsAction::Enable,
        Plugins: vec!["audit".to_owned(), "auth".to_owned()],
        Flusher: RecordingFlusher::default(),
    };

    exec.Next((), &mut ()).unwrap();

    assert_eq!(
        exec.Flusher.calls,
        [("audit".to_owned(), false), ("auth".to_owned(), false)]
    );
}

#[test]
fn disable_stops_at_the_first_flush_error() {
    let mut exec = AdminPluginsExec {
        BaseExecutor: (),
        Action: AdminPluginsAction::Disable,
        Plugins: vec!["audit".to_owned(), "auth".to_owned(), "later".to_owned()],
        Flusher: RecordingFlusher {
            calls: Vec::new(),
            fail_on: Some("auth".to_owned()),
        },
    };

    assert_eq!(exec.Next((), &mut ()), Err("flush failed"));
    assert_eq!(
        exec.Flusher.calls,
        [("audit".to_owned(), true), ("auth".to_owned(), true)]
    );
}

#[test]
fn unknown_action_is_a_no_op() {
    let mut exec = AdminPluginsExec {
        BaseExecutor: (),
        Action: AdminPluginsAction::Unknown(99),
        Plugins: vec!["audit".to_owned()],
        Flusher: RecordingFlusher::default(),
    };

    exec.Next((), &mut ()).unwrap();

    assert!(exec.Flusher.calls.is_empty());
}
