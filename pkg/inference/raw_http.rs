// Copyright 2026 AsterSQL.

//! Preserve escaped dot segments that WHATWG URLs normalize away. Ordinary
//! endpoints continue using the configured reqwest client.

use std::pin::Pin;
use std::task::{Context, Poll};

use http_body_util::BodyExt;
use hyper::Uri;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioExecutor;
use reqwest::header::HeaderMap;
use tower_service::Service;

use crate::base::{DEFAULT_HTTP_TIMEOUT, ProviderError};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
struct ForwardProxy<C> {
    connector: C,
    proxy: Uri,
}
struct ProxyStream<T>(T);
impl<T: Connection> Connection for ProxyStream<T> {
    fn connected(&self) -> Connected {
        self.0.connected().proxy(true)
    }
}
impl<T: Read + Unpin> Read for ProxyStream<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}
impl<T: Write + Unpin> Write for ProxyStream<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
impl<C> Service<Uri> for ForwardProxy<C>
where
    C: Service<Uri> + Clone + Send + 'static,
    C::Response: Read + Write + Connection + Unpin + Send + 'static,
    C::Future: Send + 'static,
    C::Error: Into<BoxError>,
{
    type Response = ProxyStream<C::Response>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.connector.poll_ready(cx).map_err(Into::into)
    }
    fn call(&mut self, _: Uri) -> Self::Future {
        let connecting = self.connector.call(self.proxy.clone());
        Box::pin(async move { connecting.await.map(ProxyStream).map_err(Into::into) })
    }
}

pub(crate) async fn post_json(
    endpoint: &str,
    payload: &serde_json::Value,
    headers: HeaderMap,
    max_bytes: i64,
    provider: &str,
) -> Result<(reqwest::StatusCode, Vec<u8>), ProviderError> {
    post_json_with_matcher(
        endpoint,
        payload,
        headers,
        max_bytes,
        provider,
        hyper_util::client::proxy::matcher::Matcher::from_env(),
        DEFAULT_HTTP_TIMEOUT,
    )
    .await
}

pub(crate) async fn post_json_with_matcher(
    endpoint: &str,
    payload: &serde_json::Value,
    mut headers: HeaderMap,
    max_bytes: i64,
    provider: &str,
    matcher: hyper_util::client::proxy::matcher::Matcher,
    timeout: std::time::Duration,
) -> Result<(reqwest::StatusCode, Vec<u8>), ProviderError> {
    let failed = |cause: BoxError| {
        ProviderError::redacted(format!("{provider} request failed"), cause.into())
    };
    let uri: Uri = endpoint.parse().map_err(|error| failed(Box::new(error)))?;
    let proxy = matcher.intercept(&uri);
    let connector = HttpsConnectorBuilder::new()
        .with_provider_and_webpki_roots(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .expect("ring TLS configuration")
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    let body = serde_json::to_vec(payload).map_err(|error| {
        ProviderError::redacted(
            "unexpected marshal request error",
            std::sync::Arc::new(error),
        )
    })?;
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    if let Some(proxy) = &proxy
        && uri.scheme_str() == Some("http")
        && let Some(auth) = proxy.basic_auth()
    {
        headers.insert(reqwest::header::PROXY_AUTHORIZATION, auth.clone());
    }
    let mut request = hyper::Request::post(uri.clone())
        .body(reqwest::Body::from(body))
        .map_err(|error| failed(Box::new(error)))?;
    *request.headers_mut() = headers;
    let operation = async {
        let response = if let Some(proxy) = proxy {
            if uri.scheme_str() == Some("https") {
                let mut tunnel = hyper_util::client::legacy::connect::proxy::Tunnel::new(
                    proxy.uri().clone(),
                    connector,
                );
                if let Some(auth) = proxy.basic_auth() {
                    tunnel = tunnel.with_auth(auth.clone());
                }
                let connector = HttpsConnectorBuilder::new()
                    .with_provider_and_webpki_roots(std::sync::Arc::new(
                        rustls::crypto::ring::default_provider(),
                    ))
                    .expect("ring TLS configuration")
                    .https_or_http()
                    .enable_http1()
                    .enable_http2()
                    .wrap_connector(tunnel);
                Client::builder(TokioExecutor::new())
                    .build::<_, reqwest::Body>(connector)
                    .request(request)
                    .await
            } else {
                let connector = ForwardProxy {
                    connector,
                    proxy: proxy.uri().clone(),
                };
                Client::builder(TokioExecutor::new())
                    .build::<_, reqwest::Body>(connector)
                    .request(request)
                    .await
            }
        } else {
            Client::builder(TokioExecutor::new())
                .build::<_, reqwest::Body>(connector)
                .request(request)
                .await
        }
        .map_err(|error| failed(Box::new(error)))?;
        let status = response.status();
        if max_bytes < 0 {
            return Err("maximum response body size must not be negative".into());
        }
        let mut incoming = response.into_body();
        let mut body = Vec::new();
        while let Some(frame) = incoming.frame().await {
            let frame = frame.map_err(|error| {
                ProviderError::redacted(error.to_string(), std::sync::Arc::new(error))
            })?;
            if let Ok(chunk) = frame.into_data() {
                let remaining =
                    (max_bytes.saturating_add(1) as u64).saturating_sub(body.len() as u64) as usize;
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if body.len() as u64 > max_bytes as u64 {
                    return Err(
                        format!("response body exceeds maximum size of {max_bytes} bytes").into(),
                    );
                }
            }
        }
        Ok((status, body))
    };
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|error| failed(Box::new(error)))?
}
