// use actix_http::StatusCode;
use http::request::Request;
use http::{header::FORWARDED, HeaderMap};
// use actix_web::{dev::Request, http::header::ContentType};
// use actix_web::{Response, ResponseBuilder, ResponseError};
use crate::errors::SimpleKeyExtractionError;
use axum::extract::ConnectInfo;
use forwarded_header_value::{ForwardedHeaderValue, Identifier};
use governor::clock::{Clock, DefaultClock, QuantaInstant};
use governor::NotUntil;
use std::fmt::Debug;
use std::net::SocketAddr;
use std::{hash::Hash, net::IpAddr};
use tower::BoxError;

/// Generic structure of what is needed to extract a rate-limiting key from an incoming request.
pub trait KeyExtractor: Clone {
    /// The type of the key.
    type Key: Clone + Hash + Eq;

    /// The type of the error that can occur if key extraction from the request fails.
    // type KeyExtractionError: Error;

    #[cfg(feature = "log")]
    /// Name of this extractor (only used in logs).
    fn name(&self) -> &'static str;

    /// Extraction method, will return [`KeyExtractionError`] response when the extract failed
    ///
    /// [`KeyExtractionError`]: KeyExtractor::KeyExtractionError
    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, BoxError>;

    /// The content you want to show it when the rate limit is exceeded.
    /// You can calculate the time at which a caller can expect the next positive rate-limiting result by using [`NotUntil`].
    /// The [`ResponseBuilder`] allows you to build a fully customized [`Response`] in case of an error.
    fn exceed_rate_limit_response(&self, negative: &NotUntil<QuantaInstant>) -> BoxError {
        let wait_time = negative
            .wait_time_from(DefaultClock::default().now())
            .as_secs();
        Box::new(SimpleKeyExtractionError::TooManyRequests(wait_time))
    }

    #[cfg(feature = "log")]
    /// Value of the extracted key (only used in logs).
    fn key_name(&self, _key: &Self::Key) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A [KeyExtractor] that allow to do rate limiting for all incoming requests. This is useful if you want to hard-limit the HTTP load your app can handle.
pub struct GlobalKeyExtractor;

impl KeyExtractor for GlobalKeyExtractor {
    type Key = ();
    // type KeyExtractionError = BoxError;

    #[cfg(feature = "log")]
    fn name(&self) -> &'static str {
        "global"
    }

    fn extract<T>(&self, _req: &Request<T>) -> Result<Self::Key, BoxError> {
        Ok(())
    }

    #[cfg(feature = "log")]
    fn key_name(&self, _key: &Self::Key) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A [KeyExtractor] that uses peer IP as key. **This is the default key extractor and [it may no do want you want](PeerIpKeyExtractor).**
///
/// **Warning:** this key extractor enforces rate limiting based on the **_peer_ IP address**.
///
/// This means that if your app is deployed behind a reverse proxy, the peer IP address will _always_ be the proxy's IP address.
/// In this case, rate limiting will be applied to _all_ incoming requests as if they were from the same user.
///
/// If this is not the behavior you want, you may:
/// - implement your own [KeyExtractor] that tries to get IP from the `Forwarded` or `X-Forwarded-For` headers that most reverse proxies set
/// - make absolutely sure that you only trust these headers when the peer IP is the IP of your reverse proxy (otherwise any user could set them to fake its IP)
pub struct PeerIpKeyExtractor;

impl KeyExtractor for PeerIpKeyExtractor {
    type Key = IpAddr;
    // type KeyExtractionError = BoxError;

    #[cfg(feature = "log")]
    fn name(&self) -> &'static str {
        "peer IP"
    }

    //type Key: Clone + Hash + Eq;
    //type Boxerror:  pub type BoxError = Box<dyn Error + Send + Sync>;
    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, BoxError> {
        req.extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip())
            .ok_or_else(|| -> BoxError { Box::new(SimpleKeyExtractionError::UnableToExtractKey) })

        // req.peer_addr()
        //     .map(|socket| socket.ip())
        //     .ok_or_else(|| SimpleKeyExtractionError::UnableToExtractKey)
    }

    #[cfg(feature = "log")]
    fn key_name(&self, key: &Self::Key) -> Option<String> {
        Some(key.to_string())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A [KeyExtractor] that uses peer IP as key. **This is the default key extractor and [it may no do want you want](PeerIpKeyExtractor).**
///
/// **Warning:** this key extractor enforces rate limiting based on the **_peer_ IP address**.
///
/// This means that if your app is deployed behind a reverse proxy, the peer IP address will _always_ be the proxy's IP address.
/// In this case, rate limiting will be applied to _all_ incoming requests as if they were from the same user.
///
/// If this is not the behavior you want, you may:
/// - implement your own [KeyExtractor] that tries to get IP from the `Forwarded` or `X-Forwarded-For` headers that most reverse proxies set
/// - make absolutely sure that you only trust these headers when the peer IP is the IP of your reverse proxy (otherwise any user could set them to fake its IP)
pub struct SmartIpKeyExtractor;

impl KeyExtractor for SmartIpKeyExtractor {
    type Key = IpAddr;
    // type KeyExtractionError = BoxError;

    #[cfg(feature = "log")]
    fn name(&self) -> &'static str {
        "smart IP"
    }

    //type Key: Clone + Hash + Eq;
    //type Boxerror:  pub type BoxError = Box<dyn Error + Send + Sync>;
    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, BoxError> {
        let headers = req.headers();

        maybe_x_forwarded_for(headers)
            .or_else(|| maybe_x_real_ip(headers))
            .or_else(|| maybe_forwarded(headers))
            .or_else(|| maybe_connect_info(req))
            .ok_or_else(|| -> BoxError { Box::new(SimpleKeyExtractionError::UnableToExtractKey) })
    }

    #[cfg(feature = "log")]
    fn key_name(&self, key: &Self::Key) -> Option<String> {
        Some(key.to_string())
    }
}

// Utility functions for the SmartIpExtractor
// Shamelessly snatched from the axum-client-ip crate here:
// https://crates.io/crates/axum-client-ip

const X_REAL_IP: &str = "x-real-ip";
const X_FORWARDED_FOR: &str = "x-forwarded-for";

/// Tries to parse the `x-real-ip` header
fn maybe_x_forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get(X_FORWARDED_FOR)
        .and_then(|hv| hv.to_str().ok())
        .and_then(|s| s.split(',').find_map(|s| s.trim().parse::<IpAddr>().ok()))
}

/// Tries to parse the `x-real-ip` header
fn maybe_x_real_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get(X_REAL_IP)
        .and_then(|hv| hv.to_str().ok())
        .and_then(|s| s.parse::<IpAddr>().ok())
}

/// Tries to parse `forwarded` headers
fn maybe_forwarded(headers: &HeaderMap) -> Option<IpAddr> {
    headers.get_all(FORWARDED).iter().find_map(|hv| {
        hv.to_str()
            .ok()
            .and_then(|s| ForwardedHeaderValue::from_forwarded(s).ok())
            .and_then(|f| {
                f.iter()
                    .filter_map(|fs| fs.forwarded_for.as_ref())
                    .find_map(|ff| match ff {
                        Identifier::SocketAddr(a) => Some(a.ip()),
                        Identifier::IpAddr(ip) => Some(*ip),
                        _ => None,
                    })
            })
    })
}

/// Looks in `ConnectInfo` extension
fn maybe_connect_info<T>(req: &Request<T>) -> Option<IpAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}