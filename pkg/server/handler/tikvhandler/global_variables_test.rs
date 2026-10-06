// Copyright 2026 AsterSQL.

use std::collections::BTreeMap;

use astersql_sessionctx_variable::{GetSysVar, SysVar, UnregisterSysVar, vardef};

use super::global_variables;

#[test]
fn masks_sensitive_values_and_redacts_cloud_storage_credentials() {
    astersql_sessionctx_variable::register_builtin_sysvars();
    let mut overrides = BTreeMap::from([
        (vardef::InitConnect.to_owned(), "set @secret = 1".to_owned()),
        (
            vardef::TiDBCloudStorageURI.to_owned(),
            "azure://bucket/path?endpoint=https%3A%2F%2Fhost%2F%3Fsig%3Dsecret&account-name=a"
                .to_owned(),
        ),
    ]);
    let custom_name = "test_http_sensitive_variable";
    astersql_sessionctx_variable::RegisterSysVar(SysVar {
        Name: custom_name.to_owned(),
        Scope: vardef::ScopeGlobal,
        IsSensitive: true,
        ..SysVar::default()
    });
    overrides.insert(custom_name.to_owned(), "custom-secret".to_owned());

    let values = global_variables(&overrides).unwrap();
    assert_eq!(values[vardef::InitConnect], vardef::MaskPwd);
    assert_eq!(values[custom_name], vardef::MaskPwd);
    assert_eq!(
        values[vardef::TiDBCloudStorageURI],
        "azure://bucket/path?account-name=a&endpoint=xxxxxx"
    );
    assert!(GetSysVar("init_slave").unwrap().IsSensitive);
    UnregisterSysVar(custom_name);
}

#[test]
fn preserves_empty_sensitive_values() {
    astersql_sessionctx_variable::register_builtin_sysvars();
    let values = global_variables(&BTreeMap::new()).unwrap();
    assert_eq!(values[vardef::InitConnect], "");
}
