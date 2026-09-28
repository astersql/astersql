// Copyright 2026 AsterSQL.

use crate::{
    AwsDecryptClient, AwsDecryptError, Context, MasterKeyKms, NewAwsKmsWithClient, Provider,
};

struct ContextObservingAwsClient;

impl AwsDecryptClient for ContextObservingAwsClient {
    fn Decrypt(
        &self,
        ctx: &Context,
        _ciphertext: &[u8],
        _key_id: &str,
    ) -> Result<Vec<u8>, AwsDecryptError> {
        if ctx.is_cancelled() {
            return Err(AwsDecryptError {
                code: "KMS error".into(),
                message: "context canceled".into(),
            });
        }
        Ok(vec![1])
    }
}

#[test]
fn provider_propagates_cancellation_context_to_client() {
    let provider =
        NewAwsKmsWithClient(&MasterKeyKms::default(), ContextObservingAwsClient).unwrap();
    let ctx = Context::new();
    ctx.cancel();

    assert_eq!(
        Provider::DecryptDataKey(&provider, &ctx, b"ciphertext"),
        Err("KMS error: context canceled".into())
    );
}
