use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[cfg(any(feature = "async", feature = "blocking"))]
use reqx::{
    advanced::TlsRootStore,
    prelude::{RedirectPolicy, RetryPolicy, StatusPolicy},
};

use crate::{
    auth::{Credentials, CredentialsSnapshot},
    error::Error,
};

#[derive(serde::Deserialize)]
struct MetadataCredentials {
    #[serde(rename = "AccessKeyId")]
    access_key_id: String,
    #[serde(rename = "SecretAccessKey")]
    secret_access_key: String,
    #[serde(rename = "Expiration")]
    expiration: Option<String>,
    #[serde(rename = "Token")]
    token: Option<String>,
}

impl MetadataCredentials {
    fn into_snapshot(self) -> Result<CredentialsSnapshot, Error> {
        let mut creds = Credentials::new(self.access_key_id, self.secret_access_key)?;
        if let Some(token) = self.token {
            creds = creds.with_session_token(token)?;
        }

        let expiration = self
            .expiration
            .ok_or_else(|| Error::decode("missing credentials expiration", None))?;
        let expires_at = parse_expiration(&expiration)?;
        Ok(CredentialsSnapshot::new(creds).with_expires_at(expires_at))
    }
}

fn parse_expiration(value: &str) -> Result<OffsetDateTime, Error> {
    if value.is_empty() {
        return Err(Error::decode("missing credentials expiration", None));
    }
    if value.trim() != value {
        return Err(Error::decode(
            "credentials expiration timestamp must not include leading or trailing whitespace",
            None,
        ));
    }
    OffsetDateTime::parse(value, &Rfc3339).map_err(|e| {
        Error::decode(
            "failed to parse credentials expiration timestamp",
            Some(Box::new(e)),
        )
    })
}

fn parse_container_credentials_full_uri(value: &str) -> Result<url::Url, Error> {
    if value
        .bytes()
        .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI must not contain ASCII control or whitespace characters",
        ));
    }
    let uri = url::Url::parse(value)
        .map_err(|_| Error::invalid_config("AWS_CONTAINER_CREDENTIALS_FULL_URI is invalid"))?;
    let scheme = uri.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI must use http or https",
        ));
    }

    if uri.host_str().is_none() {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI must include host",
        ));
    }
    if !uri.username().is_empty() || uri.password().is_some() {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI must not include user info",
        ));
    }
    if uri.fragment().is_some() {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI must not include a fragment",
        ));
    }

    if scheme == "http" && !is_allowed_http_container_credentials_host(&uri) {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI with http must target a loopback or 169.254.170.2 host",
        ));
    }

    Ok(uri)
}

fn parse_container_credentials_relative_uri(value: &str) -> Result<String, Error> {
    if value.is_empty() {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI must not be empty",
        ));
    }
    if value.trim() != value {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI must not include leading or trailing whitespace",
        ));
    }
    if value
        .bytes()
        .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI must not contain ASCII control or whitespace characters",
        ));
    }
    if !value.starts_with('/') {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI must start with '/'",
        ));
    }

    let full = format!("http://169.254.170.2{value}");
    let uri = url::Url::parse(&full)
        .map_err(|_| Error::invalid_config("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI is invalid"))?;
    if uri.host_str() != Some("169.254.170.2") {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI resolved to an unexpected host",
        ));
    }
    if uri.fragment().is_some() {
        return Err(Error::invalid_config(
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI must not include a fragment",
        ));
    }
    Ok(value.to_string())
}

fn is_allowed_http_container_credentials_host(uri: &url::Url) -> bool {
    let Some(host) = uri.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") || host == "169.254.170.2" {
        return true;
    }

    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

fn imds_v1_disabled_value(value: &str) -> bool {
    let value = value.trim();
    value == "1" || value.eq_ignore_ascii_case("true")
}

fn imds_v1_fallback_allowed() -> Result<bool, Error> {
    Ok(
        !crate::util::env::optional_var("AWS_EC2_METADATA_V1_DISABLED")?
            .as_deref()
            .is_some_and(imds_v1_disabled_value),
    )
}

fn should_fallback_to_imds_v1_from_token_error(err: &Error) -> bool {
    matches!(
        err.status(),
        Some(http::StatusCode::FORBIDDEN)
            | Some(http::StatusCode::NOT_FOUND)
            | Some(http::StatusCode::METHOD_NOT_ALLOWED)
    )
}

#[cfg(feature = "async")]
pub(crate) async fn load_async(tls_root_store: TlsRootStore) -> Result<CredentialsSnapshot, Error> {
    use std::time::Duration;

    let client = metadata_async_client(Duration::from_secs(2), tls_root_store)?;

    if let Some(full) =
        crate::util::env::optional_non_empty_var("AWS_CONTAINER_CREDENTIALS_FULL_URI")?
    {
        let full = parse_container_credentials_full_uri(&full)?;
        let headers = container_auth_headers()?;
        let body = http_get_text(&client, full.as_str(), headers).await?;
        let parsed: MetadataCredentials = serde_json::from_str(&body).map_err(|e| {
            Error::decode(
                "failed to parse container credentials JSON",
                Some(Box::new(e)),
            )
        })?;
        return parsed.into_snapshot();
    }

    if let Some(rel) =
        crate::util::env::optional_non_empty_var("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")?
    {
        let rel = parse_container_credentials_relative_uri(&rel)?;
        let url = format!("http://169.254.170.2{rel}");
        let headers = container_auth_headers()?;
        let body = http_get_text(&client, &url, headers).await?;
        let parsed: MetadataCredentials = serde_json::from_str(&body).map_err(|e| {
            Error::decode(
                "failed to parse container credentials JSON",
                Some(Box::new(e)),
            )
        })?;
        return parsed.into_snapshot();
    }

    let token = match fetch_imds_v2_token(&client).await {
        Ok(token) => Some(token),
        Err(err) if should_fallback_to_imds_v1_from_token_error(&err) => {
            if imds_v1_fallback_allowed()? {
                None
            } else {
                return Err(err);
            }
        }
        Err(err) => return Err(err),
    };
    let mut headers = http::HeaderMap::new();
    if let Some(token) = token.as_deref().filter(|v| !v.is_empty()) {
        let value = http::HeaderValue::from_str(token)
            .map_err(|_| Error::invalid_config("invalid IMDS token"))?;
        headers.insert("X-aws-ec2-metadata-token", value);
    }

    let role = http_get_text(
        &client,
        "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
        headers.clone(),
    )
    .await?;
    let role = imds_role_name_from_body(&role)?;
    let role = crate::util::encode::aws_percent_encode(&role);

    let url = format!("http://169.254.169.254/latest/meta-data/iam/security-credentials/{role}");
    let body = http_get_text(&client, &url, headers).await?;
    let parsed: MetadataCredentials = serde_json::from_str(&body)
        .map_err(|e| Error::decode("failed to parse IMDS credentials JSON", Some(Box::new(e))))?;
    parsed.into_snapshot()
}

#[cfg(feature = "async")]
async fn http_get_text(
    client: &reqx::Client,
    url: &str,
    headers: http::HeaderMap,
) -> Result<String, Error> {
    let mut req = client
        .request(http::Method::GET, url.to_string())
        .redirect_policy(RedirectPolicy::none());
    for (name, value) in headers {
        if let Some(name) = name {
            req = req.header(name, value);
        }
    }

    let resp = req
        .status_policy(StatusPolicy::Response)
        .send()
        .await
        .map_err(|e| crate::transport::map_reqx_error("request failed", e))?;
    let status = resp.status();
    if status.is_success() {
        return crate::util::text::decode_utf8_response_body(resp.body());
    }
    Err(crate::transport::response_error_from_body(
        status,
        resp.headers(),
        resp.body(),
    ))
}

#[cfg(feature = "async")]
async fn fetch_imds_v2_token(client: &reqx::Client) -> Result<String, Error> {
    let resp = client
        .request(
            http::Method::PUT,
            "http://169.254.169.254/latest/api/token".to_string(),
        )
        .redirect_policy(RedirectPolicy::none())
        .header(
            http::header::HeaderName::from_static("x-aws-ec2-metadata-token-ttl-seconds"),
            http::HeaderValue::from_static("21600"),
        )
        .status_policy(StatusPolicy::Response)
        .send()
        .await
        .map_err(|e| crate::transport::map_reqx_error("request failed", e))?;
    let status = resp.status();
    if status.is_success() {
        return imds_token_from_body(resp.body());
    }
    Err(crate::transport::response_error_from_body(
        status,
        resp.headers(),
        resp.body(),
    ))
}

#[cfg(any(feature = "async", feature = "blocking"))]
fn container_auth_headers() -> Result<http::HeaderMap, Error> {
    let mut headers = http::HeaderMap::new();
    if let Some(token) = crate::util::env::optional_var("AWS_CONTAINER_AUTHORIZATION_TOKEN")? {
        let value = container_authorization_header_value(
            "AWS_CONTAINER_AUTHORIZATION_TOKEN",
            &token,
            false,
        )?;
        headers.insert(http::header::AUTHORIZATION, value);
    } else if let Some(path) =
        crate::util::env::optional_var("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE")?
    {
        let token = std::fs::read_to_string(path)
            .map_err(|e| Error::invalid_config(format!("failed to read token file: {e}")))?;
        let value = container_authorization_header_value(
            "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
            &token,
            true,
        )?;
        headers.insert(http::header::AUTHORIZATION, value);
    }
    Ok(headers)
}

#[cfg(feature = "blocking")]
pub(crate) fn load_blocking(tls_root_store: TlsRootStore) -> Result<CredentialsSnapshot, Error> {
    use std::time::Duration;

    let client = metadata_blocking_client(Duration::from_secs(2), tls_root_store)?;

    if let Some(full) =
        crate::util::env::optional_non_empty_var("AWS_CONTAINER_CREDENTIALS_FULL_URI")?
    {
        let full = parse_container_credentials_full_uri(&full)?;
        let headers = container_auth_headers()?;
        let body = http_get_text_blocking(&client, full.as_str(), &headers)?;
        let parsed: MetadataCredentials = serde_json::from_str(&body).map_err(|e| {
            Error::decode(
                "failed to parse container credentials JSON",
                Some(Box::new(e)),
            )
        })?;
        return parsed.into_snapshot();
    }

    if let Some(rel) =
        crate::util::env::optional_non_empty_var("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")?
    {
        let rel = parse_container_credentials_relative_uri(&rel)?;
        let url = format!("http://169.254.170.2{rel}");
        let headers = container_auth_headers()?;
        let body = http_get_text_blocking(&client, &url, &headers)?;
        let parsed: MetadataCredentials = serde_json::from_str(&body).map_err(|e| {
            Error::decode(
                "failed to parse container credentials JSON",
                Some(Box::new(e)),
            )
        })?;
        return parsed.into_snapshot();
    }

    let token = match fetch_imds_v2_token_blocking(&client) {
        Ok(token) => Some(token),
        Err(err) if should_fallback_to_imds_v1_from_token_error(&err) => {
            if imds_v1_fallback_allowed()? {
                None
            } else {
                return Err(err);
            }
        }
        Err(err) => return Err(err),
    };
    let mut headers = http::HeaderMap::new();
    if let Some(token) = token.as_deref().filter(|v| !v.is_empty()) {
        let value = http::HeaderValue::from_str(token)
            .map_err(|_| Error::invalid_config("invalid IMDS token"))?;
        headers.insert("X-aws-ec2-metadata-token", value);
    }

    let role = http_get_text_blocking(
        &client,
        "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
        &headers,
    )?;
    let role = imds_role_name_from_body(&role)?;
    let role = crate::util::encode::aws_percent_encode(&role);

    let url = format!("http://169.254.169.254/latest/meta-data/iam/security-credentials/{role}");
    let body = http_get_text_blocking(&client, &url, &headers)?;
    let parsed: MetadataCredentials = serde_json::from_str(&body)
        .map_err(|e| Error::decode("failed to parse IMDS credentials JSON", Some(Box::new(e))))?;
    parsed.into_snapshot()
}

#[cfg(feature = "blocking")]
fn http_get_text_blocking(
    client: &reqx::blocking::Client,
    url: &str,
    headers: &http::HeaderMap,
) -> Result<String, Error> {
    let mut req = client
        .request(http::Method::GET, url.to_string())
        .redirect_policy(RedirectPolicy::none());
    for (name, value) in headers {
        req = req.header(name.clone(), value.clone());
    }

    let resp = req
        .status_policy(StatusPolicy::Response)
        .send()
        .map_err(|e| crate::transport::map_reqx_error("request failed", e))?;
    let status = resp.status();

    if status.is_success() {
        return crate::util::text::decode_utf8_response_body(resp.body());
    }

    Err(crate::transport::response_error_from_body(
        status,
        resp.headers(),
        resp.body(),
    ))
}

#[cfg(feature = "blocking")]
fn fetch_imds_v2_token_blocking(client: &reqx::blocking::Client) -> Result<String, Error> {
    let resp = client
        .request(
            http::Method::PUT,
            "http://169.254.169.254/latest/api/token".to_string(),
        )
        .redirect_policy(RedirectPolicy::none())
        .header(
            http::header::HeaderName::from_static("x-aws-ec2-metadata-token-ttl-seconds"),
            http::HeaderValue::from_static("21600"),
        )
        .status_policy(StatusPolicy::Response)
        .send()
        .map_err(|e| crate::transport::map_reqx_error("request failed", e))?;

    let status = resp.status();

    if status.is_success() {
        return imds_token_from_body(resp.body());
    }

    Err(crate::transport::response_error_from_body(
        status,
        resp.headers(),
        resp.body(),
    ))
}

fn imds_token_from_body(body: &bytes::Bytes) -> Result<String, Error> {
    let token = crate::util::text::decode_single_line_response_body("IMDSv2 token", body)?;
    if token.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err(Error::decode(
            "IMDSv2 token response must not contain ASCII whitespace",
            None,
        ));
    }
    http::HeaderValue::from_str(&token)
        .map_err(|_| Error::decode("IMDSv2 token response is not a valid header value", None))?;
    Ok(token)
}

fn imds_role_name_from_body(body: &str) -> Result<String, Error> {
    let role = crate::util::text::strip_trailing_line_ending(body);
    if role.is_empty() {
        return Err(Error::decode("missing IMDS role name", None));
    }
    if role.len() > 64
        || !role.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'=' | b',' | b'.' | b'@' | b'-')
        })
    {
        return Err(Error::decode("invalid IMDS role name", None));
    }
    Ok(role.to_string())
}

#[cfg(any(feature = "async", feature = "blocking"))]
fn container_authorization_header_value(
    source: &'static str,
    raw: &str,
    strip_file_line_ending: bool,
) -> Result<http::HeaderValue, Error> {
    let token = if strip_file_line_ending {
        crate::util::text::strip_trailing_line_ending(raw)
    } else {
        raw
    };
    if token.is_empty() {
        return Err(Error::invalid_config(format!("{source} must not be empty")));
    }
    if token.trim() != token {
        return Err(Error::invalid_config(format!(
            "{source} must not include leading or trailing whitespace"
        )));
    }
    if token.bytes().any(|b| b.is_ascii_control()) {
        return Err(Error::invalid_config(format!(
            "{source} must not contain ASCII control characters"
        )));
    }
    http::HeaderValue::from_str(token)
        .map_err(|_| Error::invalid_config("invalid container authorization token"))
}

#[cfg(feature = "async")]
fn metadata_async_client(
    timeout: std::time::Duration,
    tls_root_store: TlsRootStore,
) -> Result<reqx::Client, Error> {
    reqx::Client::builder("http://localhost")
        .request_timeout(timeout)
        .retry_policy(RetryPolicy::disabled())
        .redirect_policy(RedirectPolicy::none())
        .default_status_policy(StatusPolicy::Response)
        .max_response_body_bytes(1024 * 1024)
        .tls_backend(crate::transport::default_tls_backend())
        .tls_root_store(tls_root_store)
        .client_name("s3-imds")
        .build()
        .map_err(|e| Error::transport("failed to build HTTP client", Some(Box::new(e))))
}

#[cfg(feature = "blocking")]
fn metadata_blocking_client(
    timeout: std::time::Duration,
    tls_root_store: TlsRootStore,
) -> Result<reqx::blocking::Client, Error> {
    reqx::blocking::Client::builder("http://localhost")
        .request_timeout(timeout)
        .retry_policy(RetryPolicy::disabled())
        .redirect_policy(RedirectPolicy::none())
        .default_status_policy(StatusPolicy::Response)
        .max_response_body_bytes(1024 * 1024)
        .tls_backend(crate::transport::default_tls_backend())
        .tls_root_store(tls_root_store)
        .client_name("s3-imds")
        .build()
        .map_err(|e| Error::transport("failed to build HTTP client", Some(Box::new(e))))
}

#[cfg(test)]
mod tests {
    use std::io::{ErrorKind, Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::thread::JoinHandle;
    use std::time::Duration;
    use std::time::Instant;

    use bytes::Bytes;

    use super::*;

    fn spawn_test_server(
        response: Vec<u8>,
    ) -> std::result::Result<(SocketAddr, JoinHandle<()>), Error> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| Error::transport("failed to bind test server", Some(Box::new(e))))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| Error::transport("failed to configure test server", Some(Box::new(e))))?;
        let addr = listener.local_addr().map_err(|e| {
            Error::transport("failed to read test server address", Some(Box::new(e)))
        })?;

        let handle = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        let mut request = Vec::new();
                        let mut buf = [0u8; 1024];
                        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                            match stream.read(&mut buf) {
                                Ok(0) => break,
                                Ok(n) => {
                                    request.extend_from_slice(&buf[..n]);
                                    if request.len() > 64 * 1024 {
                                        break;
                                    }
                                }
                                Err(err)
                                    if matches!(
                                        err.kind(),
                                        ErrorKind::WouldBlock | ErrorKind::TimedOut
                                    ) =>
                                {
                                    break;
                                }
                                Err(_) => break,
                            }
                        }
                        let _ = stream.write_all(&response);
                        let _ = stream.flush();
                        break;
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });

        Ok((addr, handle))
    }

    #[cfg(all(
        any(feature = "async", feature = "blocking"),
        feature = "native-tls",
        not(feature = "rustls")
    ))]
    fn assert_native_tls_webpki_error(err: Error) {
        match err {
            Error::Transport {
                source: Some(source),
                ..
            } => {
                assert!(
                    source.to_string().contains("TlsRootStore::WebPki"),
                    "unexpected source error: {source}"
                );
            }
            other => panic!("expected transport error, got {other:?}"),
        }
    }

    #[test]
    fn deserializes_and_converts_metadata_credentials() {
        let json = r#"
{
  "AccessKeyId": "AKIA_TEST",
  "Expiration": "2020-01-01T00:00:00Z",
  "SecretAccessKey": "SECRET_TEST",
  "Token": "TOKEN_TEST"
}
"#;
        let parsed: MetadataCredentials = serde_json::from_str(json).unwrap();
        let snapshot = parsed.into_snapshot().unwrap();
        let creds = snapshot.credentials();
        assert_eq!(creds.access_key_id(), "AKIA_TEST");
        assert_eq!(creds.secret_access_key(), "SECRET_TEST");
        assert_eq!(creds.session_token(), Some("TOKEN_TEST"));
        assert_eq!(
            snapshot.expires_at(),
            Some(parse_expiration("2020-01-01T00:00:00Z").unwrap())
        );
    }

    #[test]
    fn missing_token_is_ok() {
        let json = r#"
{
  "AccessKeyId": "AKIA_TEST",
  "Expiration": "2020-01-01T00:00:00Z",
  "SecretAccessKey": "SECRET_TEST"
}
"#;
        let parsed: MetadataCredentials = serde_json::from_str(json).unwrap();
        let snapshot = parsed.into_snapshot().unwrap();
        let creds = snapshot.credentials();
        assert!(creds.session_token().is_none());
        assert_eq!(
            snapshot.expires_at(),
            Some(parse_expiration("2020-01-01T00:00:00Z").unwrap())
        );
    }

    #[test]
    fn metadata_credentials_rejects_ambiguous_expiration() {
        let json = r#"
{
  "AccessKeyId": "AKIA_TEST",
  "Expiration": " 2020-01-01T00:00:00Z",
  "SecretAccessKey": "SECRET_TEST",
  "Token": "TOKEN_TEST"
}
"#;
        let parsed: MetadataCredentials = serde_json::from_str(json).unwrap();
        let err = parsed
            .into_snapshot()
            .expect_err("ambiguous expiration must be rejected");
        assert!(matches!(err, Error::Decode { .. }));
    }

    #[test]
    fn parse_container_credentials_full_uri_rejects_non_local_http_host() {
        let err = parse_container_credentials_full_uri("http://example.com/creds")
            .expect_err("non-local http host must be rejected");
        match err {
            Error::InvalidConfig { message } => {
                assert!(message.contains("must target a loopback or 169.254.170.2 host"));
            }
            other => panic!("expected invalid config, got {other:?}"),
        }
    }

    #[test]
    fn parse_container_credentials_full_uri_accepts_allowed_hosts() {
        assert!(
            parse_container_credentials_full_uri("http://169.254.170.2/creds").is_ok(),
            "ECS task role endpoint should be accepted",
        );
        assert!(
            parse_container_credentials_full_uri("http://127.0.0.1/creds").is_ok(),
            "loopback endpoint should be accepted",
        );
        assert!(
            parse_container_credentials_full_uri("https://example.com/creds").is_ok(),
            "https endpoint should be accepted",
        );
    }

    #[test]
    fn parse_container_credentials_full_uri_rejects_ambiguous_parts() {
        for uri in [
            "http://user@127.0.0.1/creds",
            "http://127.0.0.1/creds#fragment",
            "http://127.0.0.1/cre ds",
        ] {
            let err = parse_container_credentials_full_uri(uri)
                .expect_err("ambiguous full URI must be rejected");
            match err {
                Error::InvalidConfig { .. } => {}
                other => panic!("expected invalid config, got {other:?}"),
            }
        }
    }

    #[test]
    fn parse_container_credentials_relative_uri_rejects_missing_leading_slash() {
        let err = parse_container_credentials_relative_uri("v2/credentials/abc")
            .expect_err("relative URI must start with slash");
        match err {
            Error::InvalidConfig { message } => {
                assert!(message.contains("must start with '/'"));
            }
            other => panic!("expected invalid config, got {other:?}"),
        }
    }

    #[test]
    fn parse_container_credentials_relative_uri_rejects_outer_whitespace() {
        let err = parse_container_credentials_relative_uri(" /v2/credentials/abc")
            .expect_err("relative URI whitespace must be rejected");
        match err {
            Error::InvalidConfig { message } => assert!(message.contains("whitespace")),
            other => panic!("expected invalid config, got {other:?}"),
        }
    }

    #[test]
    fn parse_container_credentials_relative_uri_rejects_ambiguous_values() {
        for value in ["/v2/credentials/a bc", "/v2/credentials/abc#frag"] {
            let err = parse_container_credentials_relative_uri(value)
                .expect_err("ambiguous relative URI must be rejected");
            match err {
                Error::InvalidConfig { .. } => {}
                other => panic!("expected invalid config, got {other:?}"),
            }
        }
    }

    #[test]
    fn parse_container_credentials_relative_uri_accepts_valid_path() {
        let parsed = parse_container_credentials_relative_uri("/v2/credentials/abc")
            .expect("valid relative URI should pass");
        assert_eq!(parsed, "/v2/credentials/abc");
    }

    #[test]
    fn imds_token_from_body_rejects_empty_or_invalid_header_values() {
        assert_eq!(
            imds_token_from_body(&Bytes::from_static(b"token\n"))
                .expect("line ending should be trimmed"),
            "token"
        );
        assert!(imds_token_from_body(&Bytes::from_static(b"   ")).is_err());
        assert!(imds_token_from_body(&Bytes::from_static(b"tok\nen")).is_err());
        assert!(imds_token_from_body(&Bytes::from_static(b"token\n\n")).is_err());
        assert!(imds_token_from_body(&Bytes::from_static(b"tok en")).is_err());
    }

    #[test]
    fn imds_role_name_from_body_rejects_ambiguous_values() {
        assert_eq!(
            imds_role_name_from_body("demo-role\n").unwrap(),
            "demo-role"
        );
        assert!(imds_role_name_from_body("").is_err());
        assert!(imds_role_name_from_body("demo role").is_err());
        assert!(imds_role_name_from_body("path/demo").is_err());
        assert!(imds_role_name_from_body("demo\nignored").is_err());
        assert!(imds_role_name_from_body(&"a".repeat(65)).is_err());
    }

    #[cfg(any(feature = "async", feature = "blocking"))]
    #[test]
    fn container_authorization_header_value_is_strict_about_boundaries() {
        assert_eq!(
            container_authorization_header_value("token", "Bearer abc", false)
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer abc"
        );
        assert_eq!(
            container_authorization_header_value("token_file", "Bearer abc\n", true)
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer abc"
        );
        assert!(container_authorization_header_value("token", "", false).is_err());
        assert!(container_authorization_header_value("token", " Bearer abc", false).is_err());
        assert!(container_authorization_header_value("token", "Bearer abc\n", false).is_err());
        assert!(
            container_authorization_header_value("token_file", "Bearer abc\n\n", true).is_err()
        );
    }

    #[test]
    fn imds_v1_disabled_value_parsing_matches_expected_inputs() {
        assert!(imds_v1_disabled_value("true"));
        assert!(imds_v1_disabled_value("TRUE"));
        assert!(imds_v1_disabled_value("1"));
        assert!(!imds_v1_disabled_value("false"));
        assert!(!imds_v1_disabled_value("0"));
        assert!(!imds_v1_disabled_value("yes"));
    }

    #[test]
    fn imds_v1_fallback_only_allows_specific_token_error_statuses() {
        let allowed = [
            http::StatusCode::FORBIDDEN,
            http::StatusCode::NOT_FOUND,
            http::StatusCode::METHOD_NOT_ALLOWED,
        ];
        for status in allowed {
            let err = Error::Api {
                status,
                code: None,
                message: None,
                request_id: None,
                host_id: None,
                body_snippet: None,
            };
            assert!(should_fallback_to_imds_v1_from_token_error(&err));
        }

        let denied = [
            http::StatusCode::BAD_REQUEST,
            http::StatusCode::UNAUTHORIZED,
            http::StatusCode::TOO_MANY_REQUESTS,
            http::StatusCode::INTERNAL_SERVER_ERROR,
        ];
        for status in denied {
            let err = Error::Api {
                status,
                code: None,
                message: None,
                request_id: None,
                host_id: None,
                body_snippet: None,
            };
            assert!(!should_fallback_to_imds_v1_from_token_error(&err));
        }

        assert!(!should_fallback_to_imds_v1_from_token_error(
            &Error::transport("network timeout", None,)
        ));
    }

    #[cfg(feature = "async")]
    #[test]
    fn metadata_async_client_accepts_backend_default() {
        let client = metadata_async_client(Duration::from_secs(1), TlsRootStore::BackendDefault);
        let client = client.expect("async metadata client should build");
        assert_eq!(client.default_status_policy(), StatusPolicy::Response);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn http_get_text_maps_429_to_rate_limited_async() -> std::result::Result<(), Error> {
        let (addr, handle) = spawn_test_server(
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3\r\nx-amz-request-id: req-1\r\nContent-Length: 4\r\nConnection: close\r\n\r\nslow".to_vec(),
        )?;
        let client = metadata_async_client(Duration::from_secs(2), TlsRootStore::BackendDefault)?;
        let url = format!("http://{addr}/");

        let err = http_get_text(&client, &url, http::HeaderMap::new())
            .await
            .expect_err("expected non-success IMDS response to be mapped");
        handle
            .join()
            .map_err(|_| Error::transport("test server thread panicked", None))?;

        match err {
            Error::RateLimited {
                retry_after,
                request_id,
                ..
            } => {
                assert_eq!(retry_after, Some(Duration::from_secs(3)));
                assert_eq!(request_id.as_deref(), Some("req-1"));
            }
            other => panic!("expected rate-limited error, got {other:?}"),
        }

        Ok(())
    }

    #[cfg(all(feature = "async", feature = "rustls"))]
    #[test]
    fn metadata_async_client_accepts_webpki_on_rustls() {
        let client = metadata_async_client(Duration::from_secs(1), TlsRootStore::WebPki);
        assert!(client.is_ok(), "rustls should accept WebPki root store");
    }

    #[cfg(all(feature = "async", feature = "native-tls", not(feature = "rustls")))]
    #[test]
    fn metadata_async_client_rejects_webpki_on_native_tls() {
        let err = match metadata_async_client(Duration::from_secs(1), TlsRootStore::WebPki) {
            Ok(_) => panic!("native-tls should reject WebPki root store"),
            Err(err) => err,
        };
        assert_native_tls_webpki_error(err);
    }

    #[cfg(feature = "blocking")]
    #[test]
    fn metadata_blocking_client_accepts_backend_default() {
        let client = metadata_blocking_client(Duration::from_secs(1), TlsRootStore::BackendDefault);
        let client = client.expect("blocking metadata client should build");
        assert_eq!(client.default_status_policy(), StatusPolicy::Response);
    }

    #[cfg(feature = "blocking")]
    #[test]
    fn http_get_text_blocking_maps_429_to_rate_limited() -> std::result::Result<(), Error> {
        let (addr, handle) = spawn_test_server(
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3\r\nx-amz-request-id: req-1\r\nContent-Length: 4\r\nConnection: close\r\n\r\nslow".to_vec(),
        )?;
        let client =
            metadata_blocking_client(Duration::from_secs(2), TlsRootStore::BackendDefault)?;
        let url = format!("http://{addr}/");

        let err = http_get_text_blocking(&client, &url, &http::HeaderMap::new())
            .expect_err("expected non-success IMDS response to be mapped");
        handle
            .join()
            .map_err(|_| Error::transport("test server thread panicked", None))?;

        match err {
            Error::RateLimited {
                retry_after,
                request_id,
                ..
            } => {
                assert_eq!(retry_after, Some(Duration::from_secs(3)));
                assert_eq!(request_id.as_deref(), Some("req-1"));
            }
            other => panic!("expected rate-limited error, got {other:?}"),
        }

        Ok(())
    }

    #[cfg(all(feature = "blocking", feature = "rustls"))]
    #[test]
    fn metadata_blocking_client_accepts_webpki_on_rustls() {
        let client = metadata_blocking_client(Duration::from_secs(1), TlsRootStore::WebPki);
        assert!(client.is_ok(), "rustls should accept WebPki root store");
    }

    #[cfg(all(feature = "blocking", feature = "native-tls", not(feature = "rustls")))]
    #[test]
    fn metadata_blocking_client_rejects_webpki_on_native_tls() {
        let err = match metadata_blocking_client(Duration::from_secs(1), TlsRootStore::WebPki) {
            Ok(_) => panic!("native-tls should reject WebPki root store"),
            Err(err) => err,
        };
        assert_native_tls_webpki_error(err);
    }
}
