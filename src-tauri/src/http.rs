//! The one way remote services (pull request hosts, Jira, model providers) are
//! reached: a request goes out, a status and a body come back. Clients build
//! requests and read answers; the transport only carries them, so tests swap
//! it for canned answers and nothing ever reaches the network.

use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, RETRY_AFTER};

use crate::error::{app_error, core_text, AppError, CoreText};

/// Longest a request may take before it counts as unanswered, unless the
/// request sets its own `timeout`.
const TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A secret value, such as an API key. Its `Debug` leaves the value out, so
/// it never ends up in a log or an error message.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A user name and a secret (an API token) for HTTP basic authentication.
/// Its `Debug` leaves the secret out, so it never ends up in a log or an
/// error message.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub username: String,
    pub secret: String,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("username", &self.username)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// How a request proves who sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// HTTP basic authentication.
    Basic(Credential),
    /// A header whose whole value is secret, such as `x-api-key: <key>` or
    /// `Authorization: Bearer <key>`.
    Header { name: &'static str, value: Secret },
    /// Nothing: a public endpoint, such as a Jira site's tenant info.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub auth: Auth,
    /// Headers that hold nothing secret, such as an API version.
    pub headers: Vec<(&'static str, String)>,
    /// Sent as JSON.
    pub body: Option<serde_json::Value>,
    /// Overrides the transport's default time limit, for slow answers such
    /// as a model's.
    pub timeout: Option<Duration>,
}

impl HttpRequest {
    pub fn new(method: Method, url: impl Into<String>, auth: Auth) -> Self {
        Self {
            method,
            url: url.into(),
            auth,
            headers: Vec::new(),
            body: None,
            timeout: None,
        }
    }

    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    pub fn json(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    /// The `Retry-After` header of a rate-limited answer.
    pub retry_after: Option<String>,
    /// The other headers clients read (`READ_HEADERS`), by their lowercase
    /// name, such as GitHub's `link` for the next page.
    pub headers: Vec<(&'static str, String)>,
}

/// The headers a response keeps besides `Retry-After`: GitHub pages with
/// `Link` and says how many requests are left with `X-RateLimit-*`.
pub const READ_HEADERS: &[&str] = &["link", "x-ratelimit-remaining", "x-ratelimit-reset"];

impl HttpResponse {
    /// The header `name` (lowercase, one of `READ_HEADERS`), when the answer
    /// had it.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| *header == name)
            .map(|(_, value)| value.as_str())
    }
}

pub trait Transport: Send + Sync {
    /// The service's answer, whatever its status. `Err` only when there was
    /// no answer at all, with a reason a person can read (no full stop: it
    /// ends another sentence).
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, CoreText>;
}

/// The real transport. The client is built on first use: building it starts
/// a thread, which has no business running in tests or before any service is
/// ever asked anything.
#[derive(Default)]
pub struct ReqwestTransport {
    client: OnceLock<Result<Client, CoreText>>,
}

impl ReqwestTransport {
    pub fn new() -> Self {
        Self::default()
    }

    fn client(&self) -> Result<&Client, CoreText> {
        self.client
            .get_or_init(|| {
                Client::builder()
                    .timeout(TIMEOUT)
                    .connect_timeout(CONNECT_TIMEOUT)
                    .user_agent(concat!("DiffLens/", env!("CARGO_PKG_VERSION")))
                    .build()
                    .map_err(|error| {
                        core_text!(
                            TransportClientFailed,
                            "the HTTP client could not start ({detail})",
                            detail = error
                        )
                    })
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

impl Transport for ReqwestTransport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, CoreText> {
        let client = self.client()?;
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Patch => reqwest::Method::PATCH,
            Method::Delete => reqwest::Method::DELETE,
        };
        let mut builder = client.request(method, &request.url);
        // A request may ask for a media type of its own, such as GitHub's.
        if !request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("accept"))
        {
            builder = builder.header(ACCEPT, "application/json");
        }
        builder = match &request.auth {
            Auth::Basic(credential) => {
                builder.basic_auth(&credential.username, Some(&credential.secret))
            }
            Auth::Header { name, value } => builder.header(*name, value.expose()),
            Auth::None => builder,
        };
        for (name, value) in &request.headers {
            builder = builder.header(*name, value);
        }
        if let Some(timeout) = request.timeout {
            builder = builder.timeout(timeout);
        }
        if let Some(body) = &request.body {
            builder = builder
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_string());
        }
        let response = builder.send().map_err(|error| unreachable_reason(&error))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let headers = READ_HEADERS
            .iter()
            .filter_map(|name| {
                let value = response.headers().get(*name)?.to_str().ok()?;
                Some((*name, value.to_string()))
            })
            .collect();
        let body = response.text().map_err(|error| {
            core_text!(
                TransportReadFailed,
                "the answer could not be read ({detail})",
                detail = error
            )
        })?;
        Ok(HttpResponse {
            status,
            body,
            retry_after,
            headers,
        })
    }
}

/// The errors pull request hosts and Jira answers share, by the host's name
/// in messages ("Bitbucket", "GitHub", "Jira").
pub fn host_credential_refused(host: &str) -> AppError {
    app_error!(
        AuthFailed,
        HostCredentialRefused,
        "{host} did not accept the email and API token. The token may have expired or been \
         revoked: create a new one and save it in Preferences.",
        host = host
    )
}

/// `retry_after` is the `Retry-After` header, used when it is a number of
/// seconds.
pub fn host_rate_limited(host: &str, retry_after: Option<&str>) -> AppError {
    match retry_after.and_then(|after| after.trim().parse::<u64>().ok()) {
        Some(seconds) => app_error!(
            RateLimited,
            HostRateLimitedRetryAfter,
            "{host} is limiting requests from this account. Try again in {seconds} seconds.",
            host = host,
            seconds = seconds
        ),
        None => app_error!(
            RateLimited,
            HostRateLimited,
            "{host} is limiting requests from this account. Try again in a minute.",
            host = host
        ),
    }
}

pub fn host_server_error(host: &str, status: u16) -> AppError {
    app_error!(
        RemoteFailed,
        HostServerError,
        "{host} had a problem answering (HTTP {status}). Try again in a moment.",
        host = host,
        status = status
    )
}

/// Any other refusal, with the host's own explanation when it gave one.
pub fn host_refused(host: &str, status: u16, detail: Option<String>) -> AppError {
    match detail {
        Some(detail) => app_error!(
            RemoteFailed,
            HostRefusedSaid,
            "{host} refused this: {detail}",
            host = host,
            detail = detail
        ),
        None => app_error!(
            RemoteFailed,
            HostRefusedStatus,
            "{host} refused this (HTTP {status}).",
            host = host,
            status = status
        ),
    }
}

/// A successful answer whose JSON is not what was expected.
pub fn host_unreadable(host: &str, error: &serde_json::Error) -> AppError {
    app_error!(
        RemoteFailed,
        HostUnreadableAnswer,
        "{host} answered with something DiffLens could not read ({detail}).",
        host = host,
        detail = error
    )
}

/// Why a request got no answer. URLs hold nothing secret (credentials travel
/// in headers), so reqwest's own text is safe to show.
fn unreachable_reason(error: &reqwest::Error) -> CoreText {
    if error.is_timeout() {
        core_text!(TransportTimeout, "it did not answer in time")
    } else if error.is_connect() {
        core_text!(
            TransportConnectFailed,
            "the connection failed (check that this machine is online)"
        )
    } else {
        CoreText::raw(error.to_string())
    }
}

#[cfg(test)]
pub mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use serde_json::Value;

    use super::*;

    /// Answers requests from a queue of canned responses, in order, and keeps
    /// every request it was given.
    #[derive(Default)]
    pub struct CannedTransport {
        answers: Mutex<VecDeque<Result<HttpResponse, String>>>,
        pub requests: Mutex<Vec<HttpRequest>>,
    }

    impl CannedTransport {
        pub fn answering(answers: Vec<(u16, Value)>) -> std::sync::Arc<Self> {
            let transport = Self::default();
            for (status, body) in answers {
                transport.push(status, body);
            }
            std::sync::Arc::new(transport)
        }

        pub fn push(&self, status: u16, body: Value) {
            self.answers.lock().unwrap().push_back(Ok(HttpResponse {
                status,
                body: body.to_string(),
                ..HttpResponse::default()
            }));
        }

        pub fn push_raw(&self, answer: Result<HttpResponse, String>) {
            self.answers.lock().unwrap().push_back(answer);
        }

        pub fn sent(&self) -> Vec<HttpRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Transport for CannedTransport {
        /// A canned `Err` reads as text from elsewhere, as reqwest's own is.
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, CoreText> {
            self.requests.lock().unwrap().push(request.clone());
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("no canned answer left".to_string()))
                .map_err(CoreText::raw)
        }
    }

    #[test]
    fn host_errors_name_the_host_and_keep_its_own_words_as_the_detail() {
        use crate::error::{ErrorCode, Param, Reason};

        let limited = host_rate_limited("Jira", Some(" 30 "));
        assert_eq!(limited.code, ErrorCode::RateLimited);
        assert_eq!(limited.reason, Some(Reason::HostRateLimitedRetryAfter));
        assert_eq!(limited.params.get("host"), Some(&Param::Text("Jira".into())));
        assert_eq!(limited.params.get("seconds"), Some(&Param::Number(30)));
        assert_eq!(
            host_rate_limited("Jira", Some("soon")).reason,
            Some(Reason::HostRateLimited)
        );

        let refused = host_refused("Bitbucket", 400, Some("bad anchor".to_string()));
        assert_eq!(refused.reason, Some(Reason::HostRefusedSaid));
        assert_eq!(refused.detail.as_deref(), Some("bad anchor"));
        assert_eq!(refused.message, "Bitbucket refused this: bad anchor");
        let quiet = host_refused("Bitbucket", 400, None);
        assert_eq!(quiet.reason, Some(Reason::HostRefusedStatus));
        assert_eq!(quiet.message, "Bitbucket refused this (HTTP 400).");
    }

    #[test]
    fn a_credential_never_prints_its_secret() {
        let credential = Credential {
            username: "dana@example.com".to_string(),
            secret: "ATATT-very-secret".to_string(),
        };
        let printed = format!(
            "{credential:?} {:?}",
            HttpRequest::new(
                Method::Get,
                "https://api.bitbucket.org/2.0/user",
                Auth::Basic(credential.clone()),
            )
        );
        assert!(!printed.contains("very-secret"), "{printed}");
        assert!(printed.contains("dana@example.com"));
    }

    #[test]
    fn a_header_secret_never_prints() {
        let request = HttpRequest::new(
            Method::Post,
            "https://api.example.com/v1/messages",
            Auth::Header {
                name: "x-api-key",
                value: Secret::new("sk-very-secret"),
            },
        )
        .header("anthropic-version", "2026-10-21");
        let printed = format!("{request:?}");
        assert!(!printed.contains("very-secret"), "{printed}");
        assert!(printed.contains("x-api-key"));
        assert!(printed.contains("2026-10-21"));
    }
}
