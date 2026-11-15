//! The account site, as the reader sees it: two reads of the player's own
//! data, made with the session the player signed in to.
//!
//! [`AccountSite`] is the seam: [`HttpAccountSite`] reads the real site,
//! and a scripted stand-in replays recorded responses in tests. Either way
//! an answer is one of three outcomes ([`SiteOutcome`]): the reading piece,
//! the site refusing the session, or a failure that may pass (the network,
//! a timeout, a response that is not the site's shape).
//!
//! The real reader is deliberately narrow. It speaks HTTPS to one fixed
//! host, never configurable at run time, and sends only the two read
//! requests the site's own account pages send. It follows no redirect (the
//! site redirects a request without a valid session to its login page, so
//! a redirect is read as the session having expired), gives each request
//! at most 15 seconds, reads at most 2 MB of a response before parsing it,
//! names itself truthfully, keeps no cookie jar, and parses every response
//! through the strict reading parser.
//!
//! The session cookie is a [`SessionSecret`]: it cannot be serialised,
//! printed, or copied, and it leaves this module only as the value of the
//! one request header it exists for.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::amount::Pec;
use super::reading::{balance_from_site, holdings_from_site, Holding, HoldingKey, SiteResponse};
use super::AccountError;

/// The account site. A constant, never configuration: the reader can reach
/// this host and no other.
pub const SITE_ORIGIN: &str = "https://account.entropiauniverse.com";

/// The item list: every item the player holds, in one flat list.
pub const ITEMS_PATH: &str = "/api/account/myitems";
/// The flat list, not grouped by container.
pub const ITEMS_BODY: &str = r#"{"groupByContainer":false}"#;
/// The PED Card balance.
pub const BALANCE_PATH: &str = "/api/account/getaccountbalance";
pub const BALANCE_BODY: &str = "{}";

/// The name of the cookie the site keeps the signed-in session in.
pub const SESSION_COOKIE: &str = "EntropiaSessionID";

/// The longest any one request may take, connection to last byte.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// The most of a response the reader reads before giving up on it.
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// The longest session value accepted, well above any the site issues.
const MAX_SECRET_LEN: usize = 4096;

/// The signed-in session's cookie value.
///
/// Held, never shown: it has no `Display`, its `Debug` prints nothing of
/// the value, it cannot be serialised, and it is not `Clone` (the few
/// places that must share it, the connection and its one request in
/// flight, share one value behind an `Arc`). The bytes are overwritten
/// when it is dropped, a best effort that cannot reach the copies made
/// elsewhere: the webview's, the request header the reader builds from
/// it, and the HTTP stack's.
pub struct SessionSecret(String);

/// Why a value was refused as a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretRefused {
    #[error("the session value is empty")]
    Empty,
    #[error("the session value is longer than any the site issues")]
    TooLong,
    /// A character a cookie value cannot carry, which would otherwise
    /// change the request header it is sent in.
    #[error("the session value holds a character a cookie cannot carry")]
    NotACookieValue,
}

impl SessionSecret {
    /// The session held in `value`, a cookie value as the site set it.
    pub fn new(value: String) -> Result<SessionSecret, SecretRefused> {
        let secret = SessionSecret(value);
        if secret.0.is_empty() {
            return Err(SecretRefused::Empty);
        }
        if secret.0.len() > MAX_SECRET_LEN {
            return Err(SecretRefused::TooLong);
        }
        // RFC 6265 cookie octets: visible ASCII less space, the double
        // quote, the comma, the semicolon and the backslash.
        let cookie_octet =
            |byte: u8| matches!(byte, 0x21 | 0x23..=0x2B | 0x2D..=0x3A | 0x3C..=0x5B | 0x5D..=0x7E);
        if !secret.0.bytes().all(cookie_octet) {
            return Err(SecretRefused::NotACookieValue);
        }
        Ok(secret)
    }

    /// The value, for the one request header it is sent in.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionSecret(..)")
    }
}

impl Drop for SessionSecret {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
        std::hint::black_box(&bytes);
    }
}

/// Why a read failed in a way that may pass on the next attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteFailure {
    /// The site could not be reached, or the connection failed.
    Network,
    /// The site did not answer within [`REQUEST_TIMEOUT`].
    Timeout,
    /// The site answered with a status that is neither success nor a
    /// refused session.
    Status(u16),
    /// The response was larger than [`MAX_RESPONSE_BYTES`].
    TooLarge,
    /// The response is not the shape the site sends.
    Malformed(SiteResponse),
}

impl SiteFailure {
    pub fn describe(self) -> &'static str {
        match self {
            SiteFailure::Network => "the account site could not be reached",
            SiteFailure::Timeout => "the account site did not answer in time",
            SiteFailure::Status(_) => "the account site answered with an unexpected status",
            SiteFailure::TooLarge => "the account site's answer was larger than any it sends",
            SiteFailure::Malformed(_) => "the account site's answer was not in its usual shape",
        }
    }
}

/// What one read of the site came to.
#[derive(Debug, Clone, PartialEq)]
pub enum SiteOutcome<T> {
    /// The read, parsed whole.
    Read(T),
    /// The site does not accept the session: it has expired or was never
    /// valid.
    Unauthorised,
    /// A failure that may pass on a later attempt.
    Failed(SiteFailure),
}

impl<T> SiteOutcome<T> {
    /// The outcome with its reading turned into another.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> SiteOutcome<U> {
        match self {
            SiteOutcome::Read(value) => SiteOutcome::Read(f(value)),
            SiteOutcome::Unauthorised => SiteOutcome::Unauthorised,
            SiteOutcome::Failed(failure) => SiteOutcome::Failed(failure),
        }
    }
}

/// A read in progress.
pub type SiteFuture<'a, T> = Pin<Box<dyn Future<Output = SiteOutcome<T>> + Send + 'a>>;

/// The two reads of the account site, with the session they are made in.
pub trait AccountSite: Send + Sync + 'static {
    /// The PED Card balance.
    fn read_balance<'a>(&'a self, session: &'a SessionSecret) -> SiteFuture<'a, Pec>;
    /// Every item held, keyed by holding.
    fn read_items<'a>(
        &'a self,
        session: &'a SessionSecret,
    ) -> SiteFuture<'a, BTreeMap<HoldingKey, Holding>>;
}

/// A shared site.
pub type SharedSite = Arc<dyn AccountSite>;

/// Read one response body through the strict parser: a refused session
/// the site reports inside a successful response reads as refused, and any
/// other refusal as a response not in the site's shape.
pub(crate) fn parse_body<T>(
    body: &[u8],
    response: SiteResponse,
    parse: impl FnOnce(&Value) -> Result<T, AccountError>,
) -> SiteOutcome<T> {
    let value = match serde_json::from_slice::<Value>(body) {
        Ok(value) => value,
        Err(_) => return SiteOutcome::Failed(SiteFailure::Malformed(response)),
    };
    match parse(&value) {
        Ok(read) => SiteOutcome::Read(read),
        Err(AccountError::SiteStatus { code: 401, .. }) => SiteOutcome::Unauthorised,
        Err(error) => {
            tracing::warn!(
                target: "eo::account",
                %error,
                "an account site response was refused by the reading parser"
            );
            SiteOutcome::Failed(SiteFailure::Malformed(response))
        }
    }
}

/// Why the real reader could not be built.
#[derive(Debug, thiserror::Error)]
pub enum SiteSetupError {
    #[error("the platform certificate verifier is unavailable: {0}")]
    Verifier(#[from] rustls::Error),
    #[error("the HTTP client could not be built: {0}")]
    Client(#[from] reqwest::Error),
}

/// The real account site, over HTTPS.
pub struct HttpAccountSite {
    client: reqwest::Client,
    /// Always [`SITE_ORIGIN`] outside this module's own tests.
    origin: &'static str,
}

impl HttpAccountSite {
    /// The reader, with its TLS and request policy fixed (see the module
    /// doc).
    pub fn new() -> Result<HttpAccountSite, SiteSetupError> {
        Ok(HttpAccountSite {
            client: HttpAccountSite::client(REQUEST_TIMEOUT)?
                .https_only(true)
                .build()?,
            origin: SITE_ORIGIN,
        })
    }

    /// A reader of a local stand-in for the site, over plain HTTP: only
    /// this module's tests can build one.
    #[cfg(test)]
    fn local(origin: &'static str, timeout: Duration) -> HttpAccountSite {
        HttpAccountSite {
            client: HttpAccountSite::client(timeout)
                .expect("the client policy builds")
                .build()
                .expect("the local client builds"),
            origin,
        }
    }

    fn client(timeout: Duration) -> Result<reqwest::ClientBuilder, SiteSetupError> {
        use rustls_platform_verifier::BuilderVerifierExt as _;

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_platform_verifier()?
            .with_no_client_auth();
        Ok(reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .no_proxy()
            .user_agent(concat!("EntropiaOrme/", env!("CARGO_PKG_VERSION"))))
    }

    /// Post `body` to `path` with the session, and read the answer's body.
    async fn post(
        &self,
        path: &'static str,
        body: &'static str,
        session: &SessionSecret,
    ) -> SiteOutcome<Vec<u8>> {
        use reqwest::header::{HeaderValue, CONTENT_TYPE, COOKIE};

        let mut cookie = String::with_capacity(SESSION_COOKIE.len() + 1 + session.expose().len());
        cookie.push_str(SESSION_COOKIE);
        cookie.push('=');
        cookie.push_str(session.expose());
        let mut cookie = match HeaderValue::try_from(cookie) {
            Ok(value) => value,
            // `SessionSecret::new` admits only cookie octets, so this
            // cannot happen; a session that cannot be sent is refused.
            Err(_) => return SiteOutcome::Unauthorised,
        };
        cookie.set_sensitive(true);
        let mut url = String::with_capacity(self.origin.len() + path.len());
        url.push_str(self.origin);
        url.push_str(path);
        let sent = self
            .client
            .post(url)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .header(COOKIE, cookie)
            .body(body)
            .send()
            .await;
        let mut response = match sent {
            Ok(response) => response,
            Err(error) => return SiteOutcome::Failed(transport_failure(&error)),
        };
        let status = response.status();
        // No session, or a session the site no longer accepts: it answers
        // 401, or redirects to its login page.
        if status == reqwest::StatusCode::UNAUTHORIZED || status.is_redirection() {
            return SiteOutcome::Unauthorised;
        }
        if !status.is_success() {
            return SiteOutcome::Failed(SiteFailure::Status(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return SiteOutcome::Failed(SiteFailure::TooLarge);
        }
        let mut read = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if read.len() + chunk.len() > MAX_RESPONSE_BYTES {
                        return SiteOutcome::Failed(SiteFailure::TooLarge);
                    }
                    read.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(error) => return SiteOutcome::Failed(transport_failure(&error)),
            }
        }
        SiteOutcome::Read(read)
    }
}

/// A transport error as a failure; its detail stays in the log.
fn transport_failure(error: &reqwest::Error) -> SiteFailure {
    tracing::warn!(target: "eo::account", %error, "an account site request failed");
    if error.is_timeout() {
        SiteFailure::Timeout
    } else {
        SiteFailure::Network
    }
}

impl AccountSite for HttpAccountSite {
    fn read_balance<'a>(&'a self, session: &'a SessionSecret) -> SiteFuture<'a, Pec> {
        Box::pin(async move {
            match self.post(BALANCE_PATH, BALANCE_BODY, session).await {
                SiteOutcome::Read(body) => {
                    parse_body(&body, SiteResponse::Balance, balance_from_site)
                }
                SiteOutcome::Unauthorised => SiteOutcome::Unauthorised,
                SiteOutcome::Failed(failure) => SiteOutcome::Failed(failure),
            }
        })
    }

    fn read_items<'a>(
        &'a self,
        session: &'a SessionSecret,
    ) -> SiteFuture<'a, BTreeMap<HoldingKey, Holding>> {
        Box::pin(async move {
            match self.post(ITEMS_PATH, ITEMS_BODY, session).await {
                SiteOutcome::Read(body) => {
                    parse_body(&body, SiteResponse::Items, holdings_from_site)
                }
                SiteOutcome::Unauthorised => SiteOutcome::Unauthorised,
                SiteOutcome::Failed(failure) => SiteOutcome::Failed(failure),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_shows_nothing_of_its_value() {
        let secret = SessionSecret::new("abc123DEF".to_string()).unwrap();
        assert_eq!(format!("{secret:?}"), "SessionSecret(..)");
        assert!(!format!("{:?}", Some(&secret)).contains("abc123"));
    }

    #[test]
    fn a_session_must_be_a_cookie_value() {
        assert_eq!(
            SessionSecret::new(String::new()).unwrap_err(),
            SecretRefused::Empty
        );
        for refused in ["a b", "a;b", "a\r\nX-Other: 1", "a\"b", "a,b", "a\\b", "é"] {
            assert_eq!(
                SessionSecret::new(refused.to_string()).unwrap_err(),
                SecretRefused::NotACookieValue,
                "{refused:?}"
            );
        }
        assert_eq!(
            SessionSecret::new("a".repeat(MAX_SECRET_LEN + 1)).unwrap_err(),
            SecretRefused::TooLong
        );
        assert!(SessionSecret::new("A1-_.~!#$%&'*+/:<=>?@[]^`{|}".to_string()).is_ok());
    }

    #[test]
    fn the_site_is_one_https_host() {
        let url = reqwest::Url::parse(SITE_ORIGIN).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("account.entropiauniverse.com"));
        assert_eq!(url.path(), "/");
    }

    #[test]
    fn a_refused_session_inside_a_successful_answer_reads_as_refused() {
        let body = br#"{"accountBalance": null, "statusCode": 401}"#;
        assert_eq!(
            parse_body(body, SiteResponse::Balance, balance_from_site),
            SiteOutcome::Unauthorised
        );
        let body = br#"{"accountBalance": 1.5, "statusCode": 200}"#;
        assert_eq!(
            parse_body(body, SiteResponse::Balance, balance_from_site),
            SiteOutcome::Read(Pec(150))
        );
        assert_eq!(
            parse_body(b"Unauthorized", SiteResponse::Balance, balance_from_site),
            SiteOutcome::Failed(SiteFailure::Malformed(SiteResponse::Balance))
        );
        let body = br#"{"accountBalance": 1.555, "statusCode": 200}"#;
        assert_eq!(
            parse_body(body, SiteResponse::Balance, balance_from_site),
            SiteOutcome::Failed(SiteFailure::Malformed(SiteResponse::Balance))
        );
    }

    #[test]
    fn the_real_reader_builds() {
        assert!(HttpAccountSite::new().is_ok());
    }

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// How a local stand-in for the site answers one request.
    enum Answer {
        /// These bytes, then the connection closes.
        Raw(Vec<u8>),
        /// A chunked body that never stops coming until the reader leaves.
        Endless,
        /// Headers promising a body that never arrives.
        Stalled,
    }

    fn reply(status: &str, body: &str) -> Answer {
        Answer::Raw(
            format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
        )
    }

    /// Serve one request on a local port with `answer`; the request's head
    /// and body come back through the handle.
    async fn serve(answer: Answer) -> (&'static str, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin: &'static str =
            Box::leak(format!("http://{}", listener.local_addr().unwrap()).into_boxed_str());
        let served = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            // The head, then as much body as it declares.
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= head_end + 4 + length || read == 0 {
                        break;
                    }
                }
            }
            match answer {
                Answer::Raw(bytes) => {
                    let _ = socket.write_all(&bytes).await;
                    let _ = socket.shutdown().await;
                }
                Answer::Endless => {
                    let _ = socket
                        .write_all(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n")
                        .await;
                    let chunk = format!("{:x}\r\n{}\r\n", 65_536, "x".repeat(65_536));
                    while socket.write_all(chunk.as_bytes()).await.is_ok() {}
                }
                Answer::Stalled => {
                    let _ = socket
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n{")
                        .await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            }
            String::from_utf8_lossy(&request).to_string()
        });
        (origin, served)
    }

    fn session() -> SessionSecret {
        SessionSecret::new("signed-in".to_string()).unwrap()
    }

    async fn balance_from(answer: Answer) -> SiteOutcome<Pec> {
        let (origin, _served) = serve(answer).await;
        HttpAccountSite::local(origin, Duration::from_millis(500))
            .read_balance(&session())
            .await
    }

    #[tokio::test]
    async fn a_reading_sends_the_one_cookie_and_nothing_more() {
        let (origin, served) = serve(reply(
            "200 OK",
            r#"{"accountBalance": 12.5, "statusCode": 200}"#,
        ))
        .await;
        let site = HttpAccountSite::local(origin, Duration::from_secs(5));
        assert_eq!(
            site.read_balance(&session()).await,
            SiteOutcome::Read(Pec(1_250))
        );
        let request = served.await.unwrap();
        let lower = request.to_ascii_lowercase();
        assert!(request.starts_with(&format!("POST {BALANCE_PATH} HTTP/1.1")));
        assert!(lower.contains("cookie: entropiasessionid=signed-in\r\n"));
        assert!(lower.contains("content-type: application/json"));
        assert!(lower.contains(&format!(
            "user-agent: entropiaorme/{}",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(request.ends_with(BALANCE_BODY));
    }

    #[tokio::test]
    async fn the_status_is_read_before_any_body() {
        let body = r#"{"accountBalance": 12.5, "statusCode": 200}"#;
        // A refused session, a redirect to the login page, and a refusal
        // inside a successful answer all read as refused.
        assert_eq!(
            balance_from(reply("401 Unauthorized", body)).await,
            SiteOutcome::Unauthorised
        );
        assert_eq!(
            balance_from(reply("401 Unauthorized", "Unauthorized")).await,
            SiteOutcome::Unauthorised
        );
        assert_eq!(
            balance_from(Answer::Raw(
                b"HTTP/1.1 302 Found\r\nlocation: /login\r\ncontent-length: 0\r\n\r\n".to_vec()
            ))
            .await,
            SiteOutcome::Unauthorised
        );
        assert_eq!(
            balance_from(reply(
                "200 OK",
                r#"{"accountBalance": null, "statusCode": 401}"#
            ))
            .await,
            SiteOutcome::Unauthorised
        );
        // A busy or failing site is a failure that may pass, whatever its
        // body says.
        assert_eq!(
            balance_from(reply("429 Too Many Requests", body)).await,
            SiteOutcome::Failed(SiteFailure::Status(429))
        );
        assert_eq!(
            balance_from(reply("503 Service Unavailable", body)).await,
            SiteOutcome::Failed(SiteFailure::Status(503))
        );
    }

    #[tokio::test]
    async fn a_body_over_the_cap_is_refused_declared_or_streamed() {
        let declared = Answer::Raw(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
                MAX_RESPONSE_BYTES + 1
            )
            .into_bytes(),
        );
        assert_eq!(
            balance_from(declared).await,
            SiteOutcome::Failed(SiteFailure::TooLarge)
        );
        let (origin, _served) = serve(Answer::Endless).await;
        let site = HttpAccountSite::local(origin, Duration::from_secs(20));
        assert_eq!(
            site.read_items(&session()).await,
            SiteOutcome::Failed(SiteFailure::TooLarge)
        );
    }

    #[tokio::test]
    async fn an_answer_that_never_completes_times_out() {
        assert_eq!(
            balance_from(Answer::Stalled).await,
            SiteOutcome::Failed(SiteFailure::Timeout)
        );
    }

    #[tokio::test]
    async fn the_real_reader_refuses_plain_http() {
        let (origin, _served) = serve(reply("200 OK", "{}")).await;
        let mut site = HttpAccountSite::new().unwrap();
        site.origin = origin;
        assert_eq!(
            site.read_balance(&session()).await,
            SiteOutcome::Failed(SiteFailure::Network)
        );
    }
}
