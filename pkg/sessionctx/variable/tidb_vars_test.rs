// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use serial_test::serial;

use crate::{
    disable_global_resource_control, enable_global_resource_control,
    set_disable_global_resource_control_hook, set_enable_global_resource_control_hook,
};

#[test]
#[serial]
fn global_resource_control_hooks_can_replace_themselves() {
    let (enabled_tx, enabled_rx) = mpsc::channel();
    set_enable_global_resource_control_hook(Arc::new(move || {
        set_enable_global_resource_control_hook(Arc::new(|| {}));
        enabled_tx.send(()).unwrap();
    }));
    std::thread::spawn(enable_global_resource_control);
    enabled_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("enable hook deadlocked while replacing itself");

    let (disabled_tx, disabled_rx) = mpsc::channel();
    set_disable_global_resource_control_hook(Arc::new(move || {
        set_disable_global_resource_control_hook(Arc::new(|| {}));
        disabled_tx.send(()).unwrap();
    }));
    std::thread::spawn(disable_global_resource_control);
    disabled_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("disable hook deadlocked while replacing itself");
}
