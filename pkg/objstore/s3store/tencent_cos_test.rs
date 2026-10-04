// Copyright 2026 AsterSQL.

use super::*;
use std::error::Error;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::SystemTime;

use crate::store::{TencentCredential, TencentCvmRoleCredentialsProvider};
use anyhow::Result;
use aws_credential_types::provider::ProvideCredentials;

#[test]
fn tencent_cos_endpoints_use_cvm_role_credentials() {
    for endpoint in [
        "https://cos.ap-beijing.myqcloud.com",
        "https://bucket.cos.ap-beijing.myqcloud.com",
        "https://cos.ap-beijing.tencentcos.cn",
        "https://bucket.cos-internal.ap-beijing.tencentcos.cn",
    ] {
        let options = backuppb::S3 {
            Endpoint: endpoint.to_owned(),
            ..Default::default()
        };
        assert!(is_tencent_cos_endpoint(endpoint));
        assert_eq!(
            credential_source(&options),
            CredentialSource::TencentCvmRole
        );
    }

    let other = backuppb::S3 {
        Endpoint: "https://s3.example.com".to_owned(),
        ..Default::default()
    };
    assert!(!is_tencent_cos_endpoint(&other.Endpoint));
    assert_eq!(credential_source(&other), CredentialSource::DefaultChain);
}

#[test]
fn explicit_credentials_take_precedence_over_tencent_cvm_role() {
    let options = backuppb::S3 {
        Endpoint: "https://cos.ap-beijing.myqcloud.com".to_owned(),
        AccessKey: "explicit-id".to_owned(),
        SecretAccessKey: "explicit-key".to_owned(),
        SessionToken: "explicit-token".to_owned(),
        ..Default::default()
    };
    assert_eq!(credential_source(&options), CredentialSource::Static);
    let credentials = autoNewCred(&options).unwrap().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let credentials = runtime.block_on(credentials.provide_credentials()).unwrap();
    assert_eq!(credentials.access_key_id(), "explicit-id");
    assert_eq!(credentials.secret_access_key(), "explicit-key");
    assert_eq!(credentials.session_token(), Some("explicit-token"));
}

#[derive(Debug)]
struct RotatingTencentCredential {
    calls: AtomicUsize,
}

impl TencentCredential for RotatingTencentCredential {
    fn get_credential(&self) -> Result<(String, String, String)> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok((
            format!("temporary-id-{call}"),
            format!("temporary-key-{call}"),
            format!("temporary-token-{call}"),
        ))
    }
}

#[test]
fn tencent_cvm_role_provider_expires_outer_cache_and_observes_rotation() {
    let credential = Arc::new(RotatingTencentCredential {
        calls: AtomicUsize::new(0),
    });
    let provider = TencentCvmRoleCredentialsProvider {
        credential: credential.clone(),
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let first = runtime.block_on(provider.provide_credentials()).unwrap();
    assert_eq!(first.access_key_id(), "temporary-id-1");
    assert_eq!(first.secret_access_key(), "temporary-key-1");
    assert_eq!(first.session_token(), Some("temporary-token-1"));
    assert!(
        first
            .expiry()
            .is_some_and(|expiry| expiry <= SystemTime::now())
    );

    let second = runtime.block_on(provider.provide_credentials()).unwrap();
    assert_eq!(second.access_key_id(), "temporary-id-2");
    assert_eq!(credential.calls.load(Ordering::SeqCst), 2);
}

#[derive(Debug)]
struct IncompleteTencentCredential;

impl TencentCredential for IncompleteTencentCredential {
    fn get_credential(&self) -> Result<(String, String, String)> {
        Ok(("id".to_owned(), String::new(), "token".to_owned()))
    }
}

#[test]
fn tencent_cvm_role_provider_rejects_incomplete_credentials() {
    let provider = TencentCvmRoleCredentialsProvider {
        credential: Arc::new(IncompleteTencentCredential),
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let error = runtime
        .block_on(provider.provide_credentials())
        .unwrap_err();
    assert_eq!(
        error.source().unwrap().to_string(),
        "tencent CVM role returned incomplete credentials"
    );
}
