# 21. Carry HTTP failures as HttpError

Date: 2026-11-12

## Status

Accepted

Amends [12. HTTP client trait for testability](0012-http-client-trait-for-testability.md)

## Context

`HttpClient` returned `reqwest::Error`, and the error types of the library held it as their source. `reqwest::Error` has no public constructor, so a test double of the trait cannot fail the way a real client does, and ADR 12 names that gap: error handling paths for network failures and HTTP errors could not be tested through the trait. Whoever wanted to know why a request failed had to read the text of the error chain.

## Decision

**The trait returns `HttpError`.** It holds a kind, the source error and the URL the failing request went to, which after a redirect is not the URL that was asked for. `HttpClient::get_bytes`, `HttpClient::get_stream`, the items of `ByteStream`, `FeedError::FetchFailed` and `DownloadError::{HttpFailed, StreamFailed}` use it. The text of an `HttpError` is the text of its source.

**The kind is `HttpErrorKind`:** `Dns`, `Refused`, `TimedOut`, `Tls`, `Connect`, `Status(code)` or `Other`.

**One conversion decides the kind.** `From<reqwest::Error> for HttpError` in `src/http.rs` is the only place that knows reqwest. It reads the failure as plain facts and applies the first rule that matches:

1. `reqwest::Error::is_dns()` gives `Dns`.
2. An `io::Error` of kind `ConnectionRefused` below the request error gives `Refused`.
3. A request timeout or an `io::Error` of kind `TimedOut` gives `TimedOut`.
4. A status gives `Status`.
5. A connect error whose chain holds an `io::Error` of kind `InvalidData`, or a link whose text names a certificate, TLS, a handshake or a TLS alert, gives `Tls`. The text of the request error itself holds the request URL and is not read.
6. Any other connect error gives `Connect`.
7. Anything else gives `Other`.

The rules work on a `Facts` value, so each rule is tested with plain values. Real failures are tested against a closed local port, a local server that is not a TLS server, a local server that never answers and a name under `.invalid`.

## Consequences

A test double can return an `HttpError` of any kind. The text of a failure is unchanged: the display of an `HttpError` is the display of its source.
