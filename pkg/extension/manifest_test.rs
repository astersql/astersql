// Copyright 2026 AsterSQL.

// Manifest option wiring and setup rollback parity tests.

use crate::{
    ExtensionError, FunctionDef, InstallDynamicPrivilegeHooks, InstallExtensionFunctionHooks,
    WithClose, WithCustomDynPrivs, WithCustomFunctions, WithCustomSysVariables,
    manifest::newManifestWithSetup, variable,
};
use serial_test::serial;
use std::sync::{Arc, Mutex};

fn event_recorder() -> (Arc<Mutex<Vec<String>>>, impl Fn(&str)) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorder = {
        let events = Arc::clone(&events);
        move |event: &str| events.lock().unwrap().push(event.to_string())
    };
    (events, recorder)
}

#[test]
#[serial]
fn manifest_options_register_resources_and_clear_in_go_order() {
    let (events, record) = event_recorder();
    let record = Arc::new(record);

    InstallDynamicPrivilegeHooks(
        {
            let record = Arc::clone(&record);
            Arc::new(move |privilege| {
                record(&format!("register-priv:{privilege}"));
                Ok(())
            })
        },
        {
            let record = Arc::clone(&record);
            Arc::new(move |privilege| {
                record(&format!("remove-priv:{privilege}"));
                true
            })
        },
    );
    InstallExtensionFunctionHooks(
        {
            let record = Arc::clone(&record);
            Arc::new(move |definition| {
                record(&format!("register-func:{}", definition.Name));
                Ok(())
            })
        },
        {
            let record = Arc::clone(&record);
            Arc::new(move |name| record(&format!("remove-func:{name}")))
        },
    );

    let sys_var_name = "aster_manifest_parity_var";
    variable::UnregisterSysVar(sys_var_name);
    let definition = Arc::new(FunctionDef {
        Name: "aster_manifest_parity_func".into(),
        ..FunctionDef::default()
    });
    let (manifest, clear) = newManifestWithSetup("manifest-parity".into(), || {
        Ok(vec![
            WithClose({
                let record = Arc::clone(&record);
                move || record("close")
            }),
            WithCustomDynPrivs(vec!["priv1".into(), "priv2".into()]),
            WithCustomSysVariables(vec![Some(Arc::new(variable::SysVar {
                Name: sys_var_name.into(),
                ..variable::SysVar::default()
            }))]),
            WithCustomFunctions(vec![Arc::clone(&definition)]),
        ])
    })
    .unwrap();

    assert_eq!(manifest.Name(), "manifest-parity");
    assert!(variable::GetSysVar(sys_var_name).is_some());
    assert_eq!(
        *events.lock().unwrap(),
        [
            "register-priv:priv1",
            "register-priv:priv2",
            "register-func:aster_manifest_parity_func",
        ]
    );

    clear();
    assert!(variable::GetSysVar(sys_var_name).is_none());
    assert_eq!(
        *events.lock().unwrap(),
        [
            "register-priv:priv1",
            "register-priv:priv2",
            "register-func:aster_manifest_parity_func",
            "close",
            "remove-priv:priv1",
            "remove-priv:priv2",
            "remove-func:aster_manifest_parity_func",
        ]
    );
}

#[test]
#[serial]
fn setup_error_rolls_back_only_resources_registered_before_the_error() {
    let (events, record) = event_recorder();
    let record = Arc::new(record);
    InstallDynamicPrivilegeHooks(
        {
            let record = Arc::clone(&record);
            Arc::new(move |privilege| {
                record(&format!("register:{privilege}"));
                if privilege == "bad" {
                    Err(ExtensionError::new("privilege is already registered"))
                } else {
                    Ok(())
                }
            })
        },
        {
            let record = Arc::clone(&record);
            Arc::new(move |privilege| {
                record(&format!("remove:{privilege}"));
                true
            })
        },
    );

    let error = newManifestWithSetup("manifest-rollback".into(), || {
        Ok(vec![
            WithClose({
                let record = Arc::clone(&record);
                move || record("close")
            }),
            WithCustomDynPrivs(vec!["good".into(), "bad".into(), "never".into()]),
        ])
    })
    .err()
    .expect("setup must fail on the rejected dynamic privilege");

    assert_eq!(error.to_string(), "privilege is already registered");
    assert_eq!(
        *events.lock().unwrap(),
        ["register:good", "register:bad", "close", "remove:good"]
    );
}

#[test]
#[serial]
fn invalid_system_variables_match_go_errors_and_rollback_prior_setup() {
    let (events, record) = event_recorder();
    let record = Arc::new(record);
    InstallDynamicPrivilegeHooks(Arc::new(|_| Ok(())), {
        let record = Arc::clone(&record);
        Arc::new(move |privilege| {
            record(&format!("remove:{privilege}"));
            true
        })
    });

    for (sys_var, expected) in [
        (None, "system var should not be nil"),
        (
            Some(Arc::new(variable::SysVar::default())),
            "system var name should not be empty",
        ),
    ] {
        events.lock().unwrap().clear();
        let error = newManifestWithSetup("manifest-invalid-sysvar".into(), || {
            Ok(vec![
                WithClose({
                    let record = Arc::clone(&record);
                    move || record("close")
                }),
                WithCustomDynPrivs(vec!["registered".into()]),
                WithCustomSysVariables(vec![sys_var]),
            ])
        })
        .err()
        .expect("invalid system variable must reject setup");

        assert_eq!(error.to_string(), expected);
        assert_eq!(*events.lock().unwrap(), ["close", "remove:registered"]);
    }
}
