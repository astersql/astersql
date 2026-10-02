// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aws_sdk_s3::config::SharedHttpClient;
use storeapi::aws_smithy_runtime_api::client::{
    http::{
        HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings, SharedHttpConnector,
    },
    orchestrator::{HttpRequest, HttpResponse},
    runtime_components::RuntimeComponents,
};

use crate::{NewS3Storage, backuppb};

#[derive(Clone, Debug)]
struct ObservedRequest {
    method: String,
    uri: String,
    authorization: String,
    accept_encoding: Vec<String>,
    invocation_ids: Vec<String>,
    sdk_request: String,
}

#[derive(Clone, Debug, Default)]
struct RecordingTransport(Arc<Mutex<Vec<ObservedRequest>>>, Arc<AtomicUsize>);

impl HttpConnector for RecordingTransport {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        self.0.lock().unwrap().push(ObservedRequest {
            method: request.method().to_string(),
            uri: request.uri().to_string(),
            authorization: request
                .headers()
                .get("authorization")
                .unwrap_or_default()
                .to_string(),
            invocation_ids: request
                .headers()
                .get_all("amz-sdk-invocation-id")
                .map(str::to_owned)
                .collect(),
            sdk_request: request
                .headers()
                .get("amz-sdk-request")
                .unwrap_or_default()
                .to_string(),
            accept_encoding: request
                .headers()
                .get_all("accept-encoding")
                .map(str::to_owned)
                .collect(),
        });
        let body = if request.method() == "GET" {
            "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>bucket</Name><KeyCount>0</KeyCount><MaxKeys>1</MaxKeys><IsTruncated>false</IsTruncated></ListBucketResult>"
        } else {
            ""
        };
        let mut response = HttpResponse::new(
            200.try_into().unwrap(),
            aws_sdk_s3::primitives::SdkBody::from(body),
        );
        response
            .headers_mut()
            .insert("content-type", "application/xml");
        response
            .headers_mut()
            .insert("x-amz-bucket-region", "us-east-1");
        if self
            .1
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            response = HttpResponse::new(
                500.try_into().unwrap(),
                aws_sdk_s3::primitives::SdkBody::from(
                    "<Error><Code>InternalError</Code><Message>retry</Message></Error>",
                ),
            );
        }
        HttpConnectorFuture::ready(Ok(response))
    }
}

impl HttpClient for RecordingTransport {
    fn http_connector(
        &self,
        _: &HttpConnectorSettings,
        _: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(self.clone())
    }
}

fn signed_headers(authorization: &str) -> &str {
    authorization
        .split(',')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("SignedHeaders="))
        .unwrap_or_default()
}

#[test]
fn gcs_factory_skips_region_discovery_and_signs_permission_requests() {
    for (provider, endpoint) in [
        ("", "https://storage.googleapis.com"),
        ("gcs", "http://127.0.0.1:9000"),
        ("aws", "https://storage.googleapis.com"),
    ] {
        let transport = RecordingTransport::default();
        let mut backend = backuppb::S3 {
            Bucket: "bucket".into(),
            Endpoint: endpoint.into(),
            Provider: provider.into(),
            ForcePathStyle: true,
            AccessKey: "access-key".into(),
            SecretAccessKey: "secret-access-key".into(),
            ..Default::default()
        };
        let options = storeapi::Options {
            CheckPermissions: vec![
                storeapi::Permission::AccessBuckets,
                storeapi::Permission::ListObjects,
            ],
            HTTPClient: Some(SharedHttpClient::new(transport.clone())),
            ..Default::default()
        };
        NewS3Storage(&storeapi::Context::default(), &mut backend, &options).unwrap();
        let requests = transport.0.lock().unwrap();
        assert_eq!(
            requests.len(),
            2,
            "provider={provider}, requests={requests:?}"
        );
        assert_eq!(requests[0].method, "HEAD");
        assert_eq!(requests[1].method, "GET");
        assert!(requests[1].uri.contains("list-type=2"));
        assert_eq!(backend.Region, "");
        for request in requests.iter() {
            let signed = signed_headers(&request.authorization);
            assert!(!signed.is_empty());
            assert!(!signed.contains("accept-encoding"));
            for header in [
                "amz-sdk-invocation-id",
                "amz-sdk-request",
                "host",
                "x-amz-content-sha256",
                "x-amz-date",
            ] {
                assert!(
                    signed.split(';').any(|value| value == header),
                    "{header}: {signed}"
                );
            }
        }
    }
}

#[derive(Debug)]
struct AddAcceptEncoding;

impl aws_sdk_s3::config::Intercept for AddAcceptEncoding {
    fn name(&self) -> &'static str {
        "AddAcceptEncoding"
    }

    fn modify_before_signing(
        &self,
        context: &mut aws_sdk_s3::config::interceptors::BeforeTransmitInterceptorContextMut<'_>,
        _: &RuntimeComponents,
        _: &mut aws_sdk_s3::config::ConfigBag,
    ) -> Result<(), storeapi::aws_smithy_runtime_api::box_error::BoxError> {
        context
            .request_mut()
            .headers_mut()
            .append("accept-encoding", "gzip");
        context
            .request_mut()
            .headers_mut()
            .append("accept-encoding", "deflate");
        Ok(())
    }
}

#[test]
fn gcs_signer_excludes_accept_encoding_but_sends_all_values() {
    let transport = RecordingTransport::default();
    let mut builder = aws_sdk_s3::config::Builder::new()
        .behavior_version_latest()
        .region(aws_types::region::Region::new("us-east-1"))
        .credentials_provider(aws_credential_types::Credentials::new(
            "access-key",
            "secret-access-key",
            None,
            None,
            "test",
        ))
        .endpoint_url("https://storage.googleapis.com")
        .force_path_style(true)
        .http_client(SharedHttpClient::new(transport.clone()))
        .interceptor(AddAcceptEncoding);
    crate::gcs_s3_signer::configure_gcs_signer(&mut builder);
    let client = aws_sdk_s3::Client::from_conf(builder.build());
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(client.head_bucket().bucket("bucket").send())
        .unwrap();
    let requests = transport.0.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].accept_encoding, ["gzip", "deflate"]);
    let signed = signed_headers(&requests[0].authorization);
    assert!(
        !signed.split(';').any(|header| header == "accept-encoding"),
        "{signed}"
    );
    for header in [
        "host",
        "x-amz-content-sha256",
        "x-amz-date",
        "amz-sdk-invocation-id",
        "amz-sdk-request",
    ] {
        assert!(
            signed.split(';').any(|value| value == header),
            "{header}: {signed}"
        );
    }
}

#[test]
fn gcs_detection_matches_provider_and_endpoint_boundaries() {
    for (provider, endpoint, expected) in [
        ("gcs", "http://127.0.0.1:9000", true),
        ("GcS", "://bad-endpoint", true),
        ("gcſ", "", true),
        ("ceph", "https://storage.googleapis.com", true),
        ("", "https://storage.googleapis.com/", true),
        ("", "https://bucket.storage.googleapis.com", true),
        ("aws", "https://STORAGE.GOOGLEAPIS.COM:443", true),
        ("", "//storage.googleapis.com", true),
        ("ceph", "https://s3.example.com", false),
        ("", "://bad-endpoint", false),
        ("", "", false),
        ("", "https://evilstorage.googleapis.com", false),
        ("", "https://storage.googleapis.com.evil", false),
        ("", "https:storage.googleapis.com", false),
        ("", "storage.googleapis.com", false),
        ("", "https:///storage.googleapis.com", false),
        ("", "https://ｓｔｏｒａｇｅ.googleapis.com", false),
        ("", "https://storage.googleapis.com:99999", true),
        ("", "https://storage.googleapis.com/%zz", false),
        ("", "https://storage.googleapis.com/?q=%zz", true),
        ("", "https://storage.googleapis.com/#%zz", false),
        ("", "https://storage.googleapis.com\n", false),
    ] {
        assert_eq!(
            crate::is_gcs_s3_compatible(&backuppb::S3 {
                Provider: provider.into(),
                Endpoint: endpoint.into(),
                ..Default::default()
            }),
            expected,
            "{provider}: {endpoint}"
        );
    }
}

fn signer_client(transport: &RecordingTransport, with_encoding: bool) -> aws_sdk_s3::Client {
    let mut builder = aws_sdk_s3::config::Builder::new()
        .behavior_version_latest()
        .region(aws_types::region::Region::new("us-east-1"))
        .credentials_provider(aws_credential_types::Credentials::new(
            "access-key",
            "secret-access-key",
            Some("session-token".into()),
            None,
            "test",
        ))
        .endpoint_url("https://storage.googleapis.com")
        .force_path_style(true)
        .retry_config(
            aws_sdk_s3::config::retry::RetryConfig::standard()
                .with_max_attempts(2)
                .with_initial_backoff(std::time::Duration::ZERO),
        )
        .http_client(SharedHttpClient::new(transport.clone()));
    if with_encoding {
        builder = builder.interceptor(AddAcceptEncoding);
    }
    crate::gcs_s3_signer::configure_gcs_signer(&mut builder);
    aws_sdk_s3::Client::from_conf(builder.build())
}

#[test]
fn gcs_signer_restores_multivalue_headers_on_retry_and_preserves_sdk_metadata() {
    let transport = RecordingTransport::default();
    transport.1.store(1, Ordering::SeqCst);
    let client = signer_client(&transport, true);
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(client.head_bucket().bucket("bucket").send())
        .unwrap();
    let requests = transport.0.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].invocation_ids.len(), 1);
    assert_eq!(requests[1].invocation_ids, requests[0].invocation_ids);
    assert!(requests[0].sdk_request.contains("attempt=1"));
    assert!(requests[1].sdk_request.contains("attempt=2"));
    for request in requests.iter() {
        assert_eq!(request.accept_encoding, ["gzip", "deflate"]);
        let signed = signed_headers(&request.authorization);
        assert!(!signed.contains("accept-encoding"));
        assert!(signed.contains("x-amz-security-token"));
        assert!(signed.contains("amz-sdk-invocation-id"));
        assert!(signed.contains("amz-sdk-request"));
    }
}

#[test]
fn gcs_signer_does_not_add_missing_accept_encoding() {
    let transport = RecordingTransport::default();
    let client = signer_client(&transport, false);
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(client.head_bucket().bucket("bucket").send())
        .unwrap();
    let requests = transport.0.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].accept_encoding.is_empty());
    assert!(!signed_headers(&requests[0].authorization).contains("accept-encoding"));
}

#[derive(Debug)]
struct FailSigning;

impl aws_sdk_s3::config::Intercept for FailSigning {
    fn name(&self) -> &'static str {
        "FailSigning"
    }
    fn read_before_signing(
        &self,
        _: &aws_sdk_s3::config::interceptors::BeforeTransmitInterceptorContextRef<'_>,
        _: &RuntimeComponents,
        _: &mut aws_sdk_s3::config::ConfigBag,
    ) -> Result<(), storeapi::aws_smithy_runtime_api::box_error::BoxError> {
        Err("injected signing failure".into())
    }
}

#[derive(Clone, Debug, Default)]
struct ObserveFailedAttempt(Arc<Mutex<Vec<String>>>);

impl aws_sdk_s3::config::Intercept for ObserveFailedAttempt {
    fn name(&self) -> &'static str {
        "ObserveFailedAttempt"
    }
    fn read_after_attempt(
        &self,
        context: &aws_sdk_s3::config::interceptors::FinalizerInterceptorContextRef<'_>,
        _: &RuntimeComponents,
        _: &mut aws_sdk_s3::config::ConfigBag,
    ) -> Result<(), storeapi::aws_smithy_runtime_api::box_error::BoxError> {
        *self.0.lock().unwrap() = context
            .request()
            .unwrap()
            .headers()
            .get_all("accept-encoding")
            .map(str::to_owned)
            .collect();
        Ok(())
    }
}

#[test]
fn gcs_signer_restores_headers_when_signing_fails_and_keeps_error() {
    let transport = RecordingTransport::default();
    let observer = ObserveFailedAttempt::default();
    let client = signer_client(&transport, true);
    let mut builder = client
        .config()
        .to_builder()
        .interceptor(FailSigning)
        .interceptor(observer.clone());
    builder.set_retry_config(Some(aws_sdk_s3::config::retry::RetryConfig::disabled()));
    let client = aws_sdk_s3::Client::from_conf(builder.build());
    let error = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(client.head_bucket().bucket("bucket").send())
        .unwrap_err();
    assert!(format!("{error:?}").contains("injected signing failure"));
    assert!(transport.0.lock().unwrap().is_empty());
    assert_eq!(*observer.0.lock().unwrap(), ["gzip", "deflate"]);
}
