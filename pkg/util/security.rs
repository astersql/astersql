// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// TLS/安全相关工具：证书加载、客户端/服务端 rustls 配置与 HTTP/TCP 封装。
//
// 对应 Go `util/security`。支持 CA/证书路径或内存内容、Common Name 白名单、
// 最低 TLS 版本；可跳过主机名校验仅校验证书链，或完全不校验（测试/内网场景）。

use std::fmt;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, DistinguishedName, RootCertStore,
    ServerConfig, ServerConnection, SignatureScheme, StreamOwned,
    crypto::WebPkiSupportedAlgorithms,
};
use x509_parser::prelude::{FromDer, X509Certificate};

/// TLS 1.2 协议版本号。
const TLS12: u16 = 0x0303;
/// TLS 1.3 协议版本号。
const TLS13: u16 = 0x0304;
/// 允许 TLS 1.2 与 1.3。
static TLS12_AND_TLS13: &[&rustls::SupportedProtocolVersion] =
    &[&rustls::version::TLS13, &rustls::version::TLS12];
/// 仅允许 TLS 1.3。
static TLS13_ONLY: &[&rustls::SupportedProtocolVersion] = &[&rustls::version::TLS13];

#[derive(Clone, Default)]
/// `NewTLSConfig` 收集选项时的临时构建器。
struct TlsConfigBuilder {
    ca_path: Option<PathBuf>,
    cert_path: Option<PathBuf>,
    key_path: Option<PathBuf>,
    ca_content: Vec<u8>,
    cert_content: Vec<u8>,
    key_content: Vec<u8>,
    verify_cn: Vec<String>,
    min_tls_version: u16,
}

/// 构造 `TlsConfig` 的函数式选项。
pub enum TLSConfigOption {
    CAPath(String),
    CertAndKeyPath(String, String),
    VerifyCommonName(Vec<String>),
    CAContent(Vec<u8>),
    CertAndKeyContent(Vec<u8>, Vec<u8>),
    MinTLSVersion(u16),
}

/// 指定 CA 证书文件路径。
pub fn WithCAPath(path: String) -> TLSConfigOption {
    TLSConfigOption::CAPath(path)
}

/// 指定客户端/服务端证书与私钥路径。
pub fn WithCertAndKeyPath(cert_path: String, key_path: String) -> TLSConfigOption {
    TLSConfigOption::CertAndKeyPath(cert_path, key_path)
}

/// 要求对端证书 CN 落在给定白名单。
pub fn WithVerifyCommonName(common_names: Vec<String>) -> TLSConfigOption {
    TLSConfigOption::VerifyCommonName(common_names)
}

/// 直接提供 CA PEM 内容。
pub fn WithCAContent(content: Vec<u8>) -> TLSConfigOption {
    TLSConfigOption::CAContent(content)
}

/// 直接提供证书与私钥 PEM 内容。
pub fn WithCertAndKeyContent(cert: Vec<u8>, key: Vec<u8>) -> TLSConfigOption {
    TLSConfigOption::CertAndKeyContent(cert, key)
}

/// 设置允许的最低 TLS 版本（`TLS12`/`TLS13`）。
pub fn WithMinTLSVersion(version: u16) -> TLSConfigOption {
    TLSConfigOption::MinTLSVersion(version)
}

#[derive(Clone)]
/// 已解析的 TLS 配置，可生成 rustls 客户端/服务端 config。
pub struct TlsConfig {
    roots: Option<Arc<RootCertStore>>,
    cert_path: Option<PathBuf>,
    key_path: Option<PathBuf>,
    cert_content: Vec<u8>,
    key_content: Vec<u8>,
    verify_cn: Arc<Vec<String>>,
    min_tls_version: u16,
    skip_hostname_verification: bool,
}

impl fmt::Debug for TlsConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TlsConfig")
            .field("has_roots", &self.roots.is_some())
            .field("cert_path", &self.cert_path)
            .field("key_path", &self.key_path)
            .field("verify_cn", &self.verify_cn)
            .field("min_tls_version", &self.min_tls_version)
            .field(
                "skip_hostname_verification",
                &self.skip_hostname_verification,
            )
            .finish()
    }
}

impl TlsConfig {
    /// 当前配置的最低 TLS 版本。
    pub fn min_tls_version(&self) -> u16 {
        self.min_tls_version
    }

    /// 按配置的 CN 白名单校验证书。
    pub fn verify_common_name(&self, certificate: &CertificateDer<'_>) -> Result<()> {
        verify_common_name(certificate, &self.verify_cn)
    }

    /// Parses the first certificate in a PEM bundle and checks its Common
    /// Name against this configuration's allow-list.
    pub fn verify_common_name_pem(&self, certificate_pem: &[u8]) -> Result<()> {
        let certificate = rustls_pemfile::certs(&mut Cursor::new(certificate_pem))
            .next()
            .transpose()?
            .ok_or_else(|| anyhow!("no certificates found in PEM input"))?;
        self.verify_common_name(&certificate)
    }

    /// 从路径或内存内容加载证书链与私钥；未配置则返回 `None`。
    fn certificate_and_key(
        &self,
    ) -> Result<Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>> {
        let (cert, key) = match (&self.cert_path, &self.key_path) {
            (Some(cert_path), Some(key_path)) => (
                fs::read(cert_path).with_context(|| "could not load client key pair")?,
                fs::read(key_path).with_context(|| "could not load client key pair")?,
            ),
            _ if !self.cert_content.is_empty() && !self.key_content.is_empty() => {
                (self.cert_content.clone(), self.key_content.clone())
            }
            _ => return Ok(None),
        };
        Ok(Some(parse_key_pair(&cert, &key)?))
    }

    /// 构建 rustls 客户端配置（根证书、可选客户端证书、校验策略与 ALPN）。
    pub fn client_config(&self) -> Result<Arc<ClientConfig>> {
        let versions = protocol_versions(self.min_tls_version)?;
        let roots = match &self.roots {
            Some(roots) => roots.as_ref().clone(),
            None if !self.skip_hostname_verification => {
                let result = rustls_native_certs::load_native_certs();
                let mut roots = RootCertStore::empty();
                roots.add_parsable_certificates(result.certs);
                roots
            }
            None => RootCertStore::empty(),
        };
        let verifier_roots = Arc::new(roots.clone());
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(versions)?
            .with_root_certificates(roots);
        let mut config = match self.certificate_and_key()? {
            Some((certificates, key)) => builder
                .with_client_auth_cert(certificates, key)
                .context("could not load client key pair")?,
            None => builder.with_no_client_auth(),
        };

        // 无 CN 白名单时跳过主机名校验：有 CA 则只验链，无 CA 则完全信任。
        if self.skip_hostname_verification {
            if self.roots.is_some() {
                let signature_verifier = WebPkiServerVerifier::builder_with_provider(
                    Arc::clone(&verifier_roots),
                    Arc::clone(&provider),
                )
                .build()?;
                config.dangerous().set_certificate_verifier(Arc::new(
                    CertificateChainServerVerifier {
                        roots: verifier_roots,
                        signature_verifier,
                        algorithms: provider.signature_verification_algorithms,
                    },
                ));
            } else {
                config
                    .dangerous()
                    .set_certificate_verifier(Arc::new(NoCertificateVerifier));
            }
        } else {
            let verifier =
                WebPkiServerVerifier::builder_with_provider(verifier_roots, Arc::clone(&provider))
                    .build()?;
            if self.verify_cn.is_empty() {
                config.dangerous().set_certificate_verifier(verifier);
            } else {
                config
                    .dangerous()
                    .set_certificate_verifier(Arc::new(CommonNameServerVerifier {
                        inner: verifier,
                        common_names: Arc::clone(&self.verify_cn),
                    }));
            }
        }
        config.alpn_protocols = vec![b"http/1.1".to_vec(), b"h2".to_vec()];
        Ok(Arc::new(config))
    }

    /// 构建 rustls 服务端配置；CA 启用可选客户端验签，CN 白名单将其提升为必需。
    pub fn server_config(&self) -> Result<Arc<ServerConfig>> {
        let (certificates, key) = self
            .certificate_and_key()?
            .ok_or_else(|| anyhow!("server TLS requires a certificate and private key"))?;
        let versions = protocol_versions(self.min_tls_version)?;
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let builder = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(versions)?;
        let mut config = if !self.verify_cn.is_empty() {
            let roots = self
                .roots
                .as_ref()
                .ok_or_else(|| anyhow!("Common Name verification requires a CA"))?;
            let verifier =
                WebPkiClientVerifier::builder_with_provider(Arc::clone(roots), provider).build()?;
            builder
                .with_client_cert_verifier(Arc::new(CommonNameClientVerifier {
                    inner: verifier,
                    common_names: Arc::clone(&self.verify_cn),
                }))
                .with_single_cert(certificates, key)?
        } else if let Some(roots) = self.roots.as_ref() {
            let verifier = WebPkiClientVerifier::builder_with_provider(
                Arc::clone(roots),
                Arc::clone(&provider),
            )
            .allow_unauthenticated()
            .build()?;
            builder
                .with_client_cert_verifier(verifier)
                .with_single_cert(certificates, key)?
        } else {
            builder
                .with_no_client_auth()
                .with_single_cert(certificates, key)?
        };
        config.alpn_protocols = vec![b"http/1.1".to_vec(), b"h2".to_vec()];
        Ok(Arc::new(config))
    }
}

/// 应用选项构造 `TlsConfig`；若未提供任何 CA/证书材料则返回 `None`。
pub fn NewTLSConfig(options: Vec<TLSConfigOption>) -> Result<Option<TlsConfig>> {
    let mut builder = TlsConfigBuilder::default();
    for option in options {
        match option {
            TLSConfigOption::CAPath(path) => {
                builder.ca_path = (!path.is_empty()).then(|| path.into());
            }
            TLSConfigOption::CertAndKeyPath(cert_path, key_path) => {
                builder.cert_path = (!cert_path.is_empty()).then(|| cert_path.into());
                builder.key_path = (!key_path.is_empty()).then(|| key_path.into());
            }
            TLSConfigOption::VerifyCommonName(names) => builder.verify_cn = names,
            TLSConfigOption::CAContent(content) => builder.ca_content = content,
            TLSConfigOption::CertAndKeyContent(cert, key) => {
                builder.cert_content = cert;
                builder.key_content = key;
            }
            TLSConfigOption::MinTLSVersion(version) => builder.min_tls_version = version,
        }
    }
    if builder.ca_path.is_none()
        && builder.ca_content.is_empty()
        && builder.cert_path.is_none()
        && builder.cert_content.is_empty()
        && builder.key_path.is_none()
        && builder.key_content.is_empty()
    {
        return Ok(None);
    }

    let ca_content = match &builder.ca_path {
        Some(path) => fs::read(path).context("could not read ca certificate")?,
        None => builder.ca_content,
    };
    let roots = if ca_content.is_empty() {
        None
    } else {
        Some(Arc::new(parse_ca(&ca_content)?))
    };

    if builder.cert_path.is_none()
        && !builder.cert_content.is_empty()
        && !builder.key_content.is_empty()
    {
        parse_key_pair(&builder.cert_content, &builder.key_content)?;
    }

    // Go 语义：未配置 verify-cn 时跳过主机名校验。
    let skip_hostname_verification = builder.verify_cn.is_empty();
    let config = TlsConfig {
        roots,
        cert_path: builder.cert_path,
        key_path: builder.key_path,
        cert_content: builder.cert_content,
        key_content: builder.key_content,
        verify_cn: Arc::new(
            builder
                .verify_cn
                .into_iter()
                .map(|name| name.trim().to_owned())
                .collect(),
        ),
        min_tls_version: if builder.min_tls_version == 0 {
            TLS12
        } else {
            builder.min_tls_version
        },
        skip_hostname_verification,
    };
    if !config.cert_content.is_empty() && !config.key_content.is_empty() {
        config.client_config()?;
    }
    Ok(Some(config))
}

/// 路径版便捷构造（不校验 CN，但保留主机名校验）。
pub fn ToTLSConfig(ca_path: &str, cert_path: &str, key_path: &str) -> Result<Option<TlsConfig>> {
    ToTLSConfigWithVerify(ca_path, cert_path, key_path, Vec::new())
}

/// 路径版构造，并附带 CN 白名单；空 CA 路径返回 `None`。
pub fn ToTLSConfigWithVerify(
    ca_path: &str,
    cert_path: &str,
    key_path: &str,
    verify_cn: Vec<String>,
) -> Result<Option<TlsConfig>> {
    if ca_path.is_empty() {
        return Ok(None);
    }
    let mut options = vec![
        WithCAPath(ca_path.to_owned()),
        WithVerifyCommonName(verify_cn),
    ];
    if !cert_path.is_empty() && !key_path.is_empty() {
        let cert = fs::read(cert_path).context("could not load client key pair")?;
        let key = fs::read(key_path).context("could not load client key pair")?;
        parse_key_pair(&cert, &key)?;
        options.push(WithCertAndKeyContent(cert, key));
    }
    let mut config = NewTLSConfig(options)?;
    if let Some(config) = &mut config {
        config.skip_hostname_verification = false;
    }
    Ok(config)
}

/// 解析 PEM CA 并填入根证书存储。
fn parse_ca(content: &[u8]) -> Result<RootCertStore> {
    let certificates = rustls_pemfile::certs(&mut Cursor::new(content))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut roots = RootCertStore::empty();
    let (added, _) = roots.add_parsable_certificates(certificates);
    if added == 0 {
        return Err(anyhow!("failed to append ca certs"));
    }
    Ok(roots)
}

/// 解析 PEM 证书链与私钥。
fn parse_key_pair(
    certificate: &[u8],
    key: &[u8],
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let certificates = rustls_pemfile::certs(&mut Cursor::new(certificate))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if certificates.is_empty() {
        return Err(anyhow!(
            "could not load client key pair: no certificates found"
        ));
    }
    let key = rustls_pemfile::private_key(&mut Cursor::new(key))?
        .ok_or_else(|| anyhow!("could not load client key pair: no private key found"))?;
    Ok((certificates, key))
}

/// 最低版本 → rustls 允许的协议版本切片。
fn protocol_versions(version: u16) -> Result<&'static [&'static rustls::SupportedProtocolVersion]> {
    match version {
        TLS12 => Ok(TLS12_AND_TLS13),
        TLS13 => Ok(TLS13_ONLY),
        other => Err(anyhow!("unsupported minimum TLS version: 0x{other:04x}")),
    }
}

/// 检查证书 Subject CN 是否命中白名单。
fn verify_common_name(certificate: &CertificateDer<'_>, allowed: &[String]) -> Result<()> {
    if allowed.is_empty() {
        return Ok(());
    }
    let (_, certificate) = X509Certificate::from_der(certificate.as_ref())
        .map_err(|error| anyhow!("invalid peer certificate: {error}"))?;
    let common_names = certificate
        .subject()
        .iter_common_name()
        .filter_map(|name| name.as_str().ok().map(str::to_owned))
        .collect::<Vec<_>>();
    if common_names.iter().any(|name| allowed.contains(name)) {
        return Ok(());
    }
    Err(anyhow!(
        "client certificate authentication failed. The Common Name from the client certificate {:?} was not found in the configuration cluster-verify-cn with value: {:?}",
        common_names,
        allowed
    ))
}

#[derive(Debug)]
/// 先走内层服务端校验，再额外检查 CN。
struct CommonNameServerVerifier {
    inner: Arc<dyn ServerCertVerifier>,
    common_names: Arc<Vec<String>>,
}

#[derive(Debug)]
/// 只校验证书链（不校验 server name / 主机名）。
struct CertificateChainServerVerifier {
    roots: Arc<RootCertStore>,
    signature_verifier: Arc<dyn ServerCertVerifier>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for CertificateChainServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let certificate = webpki::EndEntityCert::try_from(end_entity)
            .map_err(|error| rustls::Error::General(error.to_string()))?;
        certificate
            .verify_for_usage(
                self.algorithms.all,
                &self.roots.roots,
                intermediates,
                now,
                webpki::KeyUsage::server_auth(),
                None,
                None,
            )
            .map_err(|error| rustls::Error::General(error.to_string()))?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.signature_verifier
            .verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.signature_verifier
            .verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.signature_verifier.supported_verify_schemes()
    }
}

impl ServerCertVerifier for CommonNameServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let verified = self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        )?;
        verify_common_name(end_entity, &self.common_names)
            .map_err(|error| rustls::Error::General(error.to_string()))?;
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[derive(Debug)]
/// 服务端校验客户端证书后再检查 CN。
struct CommonNameClientVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    common_names: Arc<Vec<String>>,
}

impl ClientCertVerifier for CommonNameClientVerifier {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }
    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        let verified = self
            .inner
            .verify_client_cert(end_entity, intermediates, now)?;
        verify_common_name(end_entity, &self.common_names)
            .map_err(|error| rustls::Error::General(error.to_string()))?;
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[derive(Debug)]
/// 完全跳过服务端证书校验（危险，仅特殊场景）。
struct NoCertificateVerifier;

impl ServerCertVerifier for NoCertificateVerifier {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Clone)]
/// 可选挂载 TLS 的阻塞 HTTP 客户端封装。
pub struct HttpClient {
    tls: Option<Arc<TlsConfig>>,
}

impl HttpClient {
    /// 发起 GET；若配置了 TLS 则使用预配置的 rustls `ClientConfig`。
    pub fn Get(&self, url: &str) -> Result<reqwest::blocking::Response> {
        let builder = reqwest::blocking::Client::builder();
        let client = match &self.tls {
            Some(config) => {
                // reqwest downcasts the preconfigured value to rustls::ClientConfig (not Arc).
                let tls = (*config.client_config()?).clone();
                builder.use_preconfigured_tls(tls).build()?
            }
            None => builder.build()?,
        };
        Ok(client.get(url).send()?)
    }
}

/// 用给定 TLS 配置构造 `HttpClient`。
pub fn ClientWithTLS(config: Arc<TlsConfig>) -> HttpClient {
    HttpClient { tls: Some(config) }
}

/// 聚合 TLS 配置、HTTP 客户端与 scheme://host URL。
pub struct TLS {
    pub inner: Option<Arc<TlsConfig>>,
    pub client: HttpClient,
    pub url: String,
}

/// 按路径与 CN 白名单构造 `TLS` 辅助对象。
pub fn NewTLS(
    ca_path: &str,
    cert_path: &str,
    key_path: &str,
    host: &str,
    verify_cn: Vec<String>,
) -> Result<TLS> {
    let inner = ToTLSConfigWithVerify(ca_path, cert_path, key_path, verify_cn)?.map(Arc::new);
    let client = HttpClient { tls: inner.clone() };
    Ok(TLS {
        url: format!(
            "{}://{host}",
            if inner.is_some() { "https" } else { "http" }
        ),
        inner,
        client,
    })
}

/// 明文或 TLS 包装的 `TcpListener`。
pub enum Listener {
    Plain(TcpListener),
    Tls(TcpListener, Arc<TlsConfig>),
}

/// 明文或 TLS 包装的已接受连接。
pub enum Connection {
    Plain(TcpStream),
    Tls(StreamOwned<ServerConnection, TcpStream>),
}

impl Listener {
    /// 接受连接；TLS 模式下完成服务端握手包装。
    pub fn accept(&self) -> Result<(Connection, SocketAddr)> {
        match self {
            Self::Plain(listener) => {
                let (stream, address) = listener.accept()?;
                Ok((Connection::Plain(stream), address))
            }
            Self::Tls(listener, config) => {
                let (stream, address) = listener.accept()?;
                let connection = ServerConnection::new(config.server_config()?)?;
                Ok((
                    Connection::Tls(StreamOwned::new(connection, stream)),
                    address,
                ))
            }
        }
    }
}

impl Read for Connection {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

impl TLS {
    /// 按是否配置 TLS 包装监听器。
    pub fn WrapListener(&self, listener: TcpListener) -> Listener {
        match &self.inner {
            Some(config) => Listener::Tls(listener, Arc::clone(config)),
            None => Listener::Plain(listener),
        }
    }
}

/// 客户端 TLS 流类型别名。
pub type ClientTlsStream = StreamOwned<ClientConnection, TcpStream>;
