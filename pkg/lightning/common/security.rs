// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// TLS/安全配置工具：为 Lightning 连接 PD、TiKV 及 HTTP API 组装证书与拨号选项。
//
// 支持从文件路径或内存字节加载 CA/证书/私钥（PEM），并转换为 PD/TiKV 安全配置结构。
// PD（Placement Driver）负责集群元数据与调度；TiKV 为分布式存储引擎。

use crate::{CommonError, Context};
use std::fs;
use std::sync::Arc;

/// TLS 材料：CA、客户端证书与私钥的原始字节（通常为 PEM）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TLSConfig {
    /// CA 证书内容。
    pub CA: Vec<u8>,
    /// 客户端/服务端证书内容。
    pub Cert: Vec<u8>,
    /// 私钥内容。
    pub Key: Vec<u8>,
}

/// gRPC 拨号选项：是否启用安全（TLS）连接。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DialOption {
    /// 为 true 时使用 TLS。
    pub Secure: bool,
}

/// PD 客户端安全选项：路径与字节二选一或同时保留，供下游 PD SDK 使用。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PDSecurityOption {
    /// CA 文件路径。
    pub CAPath: String,
    /// 证书文件路径。
    pub CertPath: String,
    /// 私钥文件路径。
    pub KeyPath: String,
    /// CA 证书字节。
    pub SSLCABytes: Vec<u8>,
    /// 证书字节。
    pub SSLCertBytes: Vec<u8>,
    /// 私钥字节。
    pub SSLKEYBytes: Vec<u8>,
}

/// TiKV 集群 TLS 配置：CA/证书/密钥路径及可选 CN 校验列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TiKVSecurityConfig {
    /// 集群 CA 路径。
    pub ClusterSSLCA: String,
    /// 集群证书路径。
    pub ClusterSSLCert: String,
    /// 集群私钥路径。
    pub ClusterSSLKey: String,
    /// 允许的证书 Common Name 列表。
    pub ClusterVerifyCN: Vec<String>,
}

/// 包装底层 Listener，并附带可选 TLS 配置。
pub struct TLSListener<L> {
    /// 原始监听器。
    pub Listener: L,
    /// 可选 TLS 配置；`None` 表示明文。
    pub Config: Option<TLSConfig>,
}

/// HTTP GET 回调类型：给定上下文与 URL，返回响应体字节或错误。
pub type HTTPGetter = Arc<dyn Fn(&Context, &str) -> Result<Vec<u8>, CommonError> + Send + Sync>;

/// 测试用 Mock TLS 服务端：提供 URL、可选 TLS 配置与注入的 HTTP 客户端。
#[derive(Clone)]
pub struct MockTLSServer {
    /// Mock 服务端 TLS 材料；`None` 表示 HTTP。
    pub TLS: Option<TLSConfig>,
    /// Mock 服务基址 URL。
    pub URL: String,
    /// 注入的 HTTP GET 实现。
    pub Client: HTTPGetter,
}

/// Lightning 侧 TLS 句柄：保存路径/字节、解析后的配置、HTTP 客户端与基址 URL。
#[derive(Clone)]
pub struct TLS {
    /// CA 文件路径。
    caPath: String,
    /// 证书文件路径。
    certPath: String,
    /// 私钥文件路径。
    keyPath: String,
    /// CA 原始字节（构造时传入，可能尚未读文件）。
    caBytes: Vec<u8>,
    /// 证书原始字节。
    certBytes: Vec<u8>,
    /// 私钥原始字节。
    keyBytes: Vec<u8>,
    /// 加载并校验后的 TLS 配置；全空则为 `None`（明文）。
    inner: Option<TLSConfig>,
    /// 可选 HTTP 传输（Mock 场景注入）。
    client: Option<HTTPGetter>,
    /// 当前基址，如 `https://host`。
    url: String,
}

/// Go `NewTLSConfig` gives a non-empty path precedence over in-memory content.
fn read_path_or_content(path: &str, content: &[u8]) -> Result<Vec<u8>, CommonError> {
    if !path.is_empty() {
        return fs::read(path).map_err(|error| CommonError::new("tls", error.to_string()));
    }
    Ok(content.to_vec())
}

/// 粗略判断字节是否像 PEM（含 `-----BEGIN ` 标记）。
fn looks_like_pem(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    text.contains("-----BEGIN ")
}

/// 从路径与/或字节构造 `TLS`；完整路径对优先于完整内容对，孤立的证书或密钥按 Go 逻辑忽略。
///
/// 全部材料为空时使用 `http://`；否则使用 `https://` 并填入 `inner`。
pub fn NewTLS(
    caPath: String,
    certPath: String,
    keyPath: String,
    host: String,
    caBytes: Vec<u8>,
    certBytes: Vec<u8>,
    keyBytes: Vec<u8>,
) -> Result<TLS, CommonError> {
    let has_any_material = !caPath.is_empty()
        || !certPath.is_empty()
        || !keyPath.is_empty()
        || !caBytes.is_empty()
        || !certBytes.is_empty()
        || !keyBytes.is_empty();
    let loaded_ca = read_path_or_content(&caPath, &caBytes)?;
    // Go only loads a key pair when both halves are present. Complete paths
    // override complete in-memory content; an incomplete pair is ignored.
    let (loaded_cert, loaded_key) = if !certPath.is_empty() && !keyPath.is_empty() {
        (fs::read(&certPath), fs::read(&keyPath))
    } else if !certBytes.is_empty() && !keyBytes.is_empty() {
        (Ok(certBytes.clone()), Ok(keyBytes.clone()))
    } else {
        (Ok(Vec::new()), Ok(Vec::new()))
    };
    let loaded_cert = loaded_cert.map_err(|error| CommonError::new("tls", error.to_string()))?;
    let loaded_key = loaded_key.map_err(|error| CommonError::new("tls", error.to_string()))?;
    // 已选中的证书/密钥必须均为 PEM；CA 非空时也须为 PEM。
    if (!loaded_cert.is_empty() || !loaded_key.is_empty())
        && (!looks_like_pem(&loaded_cert) || !looks_like_pem(&loaded_key))
    {
        return Err(CommonError::new(
            "tls",
            "tls: failed to find any PEM data in certificate input",
        ));
    }
    if !loaded_ca.is_empty() && !looks_like_pem(&loaded_ca) {
        return Err(CommonError::new(
            "tls",
            "tls: failed to find any PEM data in certificate input",
        ));
    }
    let inner = if !has_any_material {
        None
    } else {
        Some(TLSConfig {
            CA: loaded_ca,
            Cert: loaded_cert,
            Key: loaded_key,
        })
    };
    let scheme = if inner.is_some() { "https" } else { "http" };
    Ok(TLS {
        caPath,
        certPath,
        keyPath,
        caBytes,
        certBytes,
        keyBytes,
        inner,
        client: None,
        url: format!("{scheme}://{host}"),
    })
}

/// 从 Mock 服务端复制 TLS 配置与 HTTP Client，用于单元测试。
pub fn NewTLSFromMockServer(server: &MockTLSServer) -> TLS {
    TLS {
        caPath: String::new(),
        certPath: String::new(),
        keyPath: String::new(),
        caBytes: Vec::new(),
        certBytes: Vec::new(),
        keyBytes: Vec::new(),
        inner: server.TLS.clone(),
        client: Some(Arc::clone(&server.Client)),
        url: server.URL.clone(),
    }
}

/// 返回 Mock/当前 TLS 实例的基址 URL（测试断言用）。
pub fn GetMockTLSUrl(tls: &TLS) -> String {
    tls.url.clone()
}

impl TLS {
    /// 替换基址中的 host（可含路径）；去掉传入的 `http(s)://` 前缀后按是否启用 TLS 重拼 scheme。
    pub fn WithHost(&self, host: &str) -> TLS {
        let host = host
            .strip_prefix("http://")
            .or_else(|| host.strip_prefix("https://"))
            .unwrap_or(host);
        let mut result = self.clone();
        result.url = format!(
            "{}://{}",
            if self.inner.is_some() {
                "https"
            } else {
                "http"
            },
            host
        );
        result
    }

    /// 转为 gRPC 拨号选项（有 TLS 配置则 Secure=true）。
    pub fn ToGRPCDialOption(&self) -> DialOption {
        ToGRPCDialOption(self.inner.as_ref())
    }

    /// 用当前 TLS 配置包装监听器。
    pub fn WrapListener<L>(&self, listener: L) -> TLSListener<L> {
        TLSListener {
            Listener: listener,
            Config: self.inner.clone(),
        }
    }

    /// 通过已配置的 HTTP Client 请求 `url + path`，返回响应体。
    ///
    /// 未注入 Client 时返回配置错误（真实环境需另行挂载传输层）。
    pub fn GetJSON(&self, ctx: &Context, path: &str) -> Result<Vec<u8>, CommonError> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| CommonError::new("http", "no HTTP transport has been configured"))?;
        client(ctx, &format!("{}{}", self.url, path))
    }

    /// 导出为 PD 安全选项（保留路径与原始字节）。
    pub fn ToPDSecurityOption(&self) -> PDSecurityOption {
        PDSecurityOption {
            CAPath: self.caPath.clone(),
            CertPath: self.certPath.clone(),
            KeyPath: self.keyPath.clone(),
            SSLCABytes: self.caBytes.clone(),
            SSLCertBytes: self.certBytes.clone(),
            SSLKEYBytes: self.keyBytes.clone(),
        }
    }

    /// 导出为 TiKV 安全配置（路径侧；CN 列表暂为空）。
    pub fn ToTiKVSecurityConfig(&self) -> TiKVSecurityConfig {
        TiKVSecurityConfig {
            ClusterSSLCA: self.caPath.clone(),
            ClusterSSLCert: self.certPath.clone(),
            ClusterSSLKey: self.keyPath.clone(),
            ClusterVerifyCN: Vec::new(),
        }
    }

    /// 返回内部 TLS 配置引用；明文模式为 `None`。
    pub fn TLSConfig(&self) -> Option<&TLSConfig> {
        self.inner.as_ref()
    }
}

/// 由可选 `TLSConfig` 生成 gRPC 拨号选项：有配置则 Secure。
pub fn ToGRPCDialOption(tls_config: Option<&TLSConfig>) -> DialOption {
    DialOption {
        Secure: tls_config.is_some(),
    }
}
