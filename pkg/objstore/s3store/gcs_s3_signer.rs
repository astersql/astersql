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

use aws_sdk_s3::config::interceptors::{
    BeforeTransmitInterceptorContextMut, FinalizerInterceptorContextMut,
};
use aws_sdk_s3::config::{Builder, ConfigBag, Intercept};
use aws_smithy_types::config_bag::{Storable, StoreReplace};
use storeapi::aws_smithy_runtime_api::{
    box_error::BoxError, client::orchestrator::HttpRequest,
    client::runtime_components::RuntimeComponents,
};

#[derive(Debug, Default)]
struct SavedHeaders(Vec<(String, Vec<String>)>);

impl Storable for SavedHeaders {
    type Storer = StoreReplace<Self>;
}

/// GCS rejects Accept-Encoding in the canonical request. Save it per attempt,
/// exclude it while the SDK signs, and restore all values even on signing failure.
#[derive(Debug)]
struct GcsS3CompatibleSigner;

fn restore_headers(request: &mut HttpRequest, cfg: &mut ConfigBag) {
    if let Some(saved) = cfg.get_mut_from_interceptor_state::<SavedHeaders>() {
        for (key, values) in std::mem::take(&mut saved.0) {
            request.headers_mut().remove(&key);
            for value in values {
                request.headers_mut().append(key.clone(), value);
            }
        }
    }
}

impl Intercept for GcsS3CompatibleSigner {
    fn name(&self) -> &'static str {
        "GcsS3CompatibleSigner"
    }

    fn modify_before_signing(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        runtime: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        // Rust's SDK adds these two headers after signing, whereas Go signs
        // them. Reuse its native generators before signing, then preserve the
        // exact signed values after the native transmit hooks run again.
        for interceptor in runtime.interceptors() {
            if matches!(
                interceptor.name(),
                "InvocationIdInterceptor" | "RequestInfoInterceptor"
            ) {
                interceptor.modify_before_transmit(context, runtime, cfg)?;
            }
        }
        let request = context.request_mut();
        let mut saved = SavedHeaders::default();
        for key in [
            "accept-encoding",
            "amz-sdk-invocation-id",
            "amz-sdk-request",
        ] {
            let values: Vec<_> = request.headers().get_all(key).map(str::to_owned).collect();
            if !values.is_empty() {
                saved.0.push((key.to_owned(), values));
            }
        }
        request.headers_mut().remove("accept-encoding");
        cfg.interceptor_state().store_put(saved);
        Ok(())
    }

    fn modify_before_transmit(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        restore_headers(context.request_mut(), cfg);
        Ok(())
    }

    fn modify_before_attempt_completion(
        &self,
        context: &mut FinalizerInterceptorContextMut<'_>,
        _: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        if let Some(request) = context.request_mut() {
            restore_headers(request, cfg);
        }
        Ok(())
    }
}

pub(crate) fn configure_gcs_signer(builder: &mut Builder) {
    builder.push_interceptor(aws_sdk_s3::config::SharedInterceptor::new(
        GcsS3CompatibleSigner,
    ));
}
