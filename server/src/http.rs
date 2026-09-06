//! The HTTP surface.
//!
//! Thin on purpose: decode, call the logic that has its own tests, encode. No
//! decisions are made here that are not about HTTP itself.
//!
//! Bodies are postcard rather than JSON. Tills sync over patchy mobile networks
//! on prepaid data, and a batch of a hundred sales is roughly a third the size
//! encoded this way. The protocol version travels inside the body, so a
//! misrouted or stale client is refused with something specific rather than a
//! parse failure.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use openpos_core::protocol::{
    negotiate, EnrolRequest, EnrolResponse, LeaseRequest, LeaseResponse, ProtocolError,
    PullRequest, PullResponse, PushRequest,
};

use crate::auth::{bearer, Caller, EnrolmentCode, Token, TokenHash};
use crate::ingest::{self, IngestError};
use crate::repo::{RepoError, Repository};

/// Content type for postcard bodies, versioned so a future encoding can be
/// introduced without guessing what a client sent.
pub const CONTENT_TYPE: &str = "application/vnd.openpos.v1+postcard";

/// Shared state.
///
/// Generic over the repository rather than holding a trait object, because the
/// trait is asynchronous and an async method is not dyn compatible. Generics
/// also mean no lock around the server: a Postgres pool manages its own
/// concurrency, so two shops never wait on each other.
pub struct AppState<R> {
    pub repo: Arc<R>,
}

impl<R> Clone for AppState<R> {
    fn clone(&self) -> Self {
        Self {
            repo: Arc::clone(&self.repo),
        }
    }
}

impl<R: Repository> AppState<R> {
    #[must_use]
    pub fn new(repo: R) -> Self {
        Self {
            repo: Arc::new(repo),
        }
    }
}

/// Build the router.
pub fn router<R: Repository + 'static>(state: AppState<R>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/sync/push", post(push))
        .route("/v1/sync/pull", post(pull))
        .route("/v1/lease", post(lease))
        .route("/v1/enrol", post(enrol))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

/// Establish who is calling, from the credential rather than from the body.
///
/// Every handler starts here. The request still carries its own idea of which
/// tenant and terminal it is, and that is checked against the token rather than
/// trusted: a mismatch means a misconfigured device pointed at the wrong shop,
/// which is worth refusing loudly instead of quietly serving the wrong data.
async fn authenticate<R: Repository>(
    state: &AppState<R>,
    headers: &HeaderMap,
    claimed_tenant: u128,
    claimed_terminal: u128,
) -> std::result::Result<Caller, Response> {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    let Some(token) = bearer(presented) else {
        return Err(protocol_error(&ProtocolError::Unauthenticated));
    };

    let caller = match state.repo.authenticate(&TokenHash::of(token)).await {
        Ok(Some(caller)) => caller,
        Ok(None) => return Err(protocol_error(&ProtocolError::Unauthenticated)),
        Err(_) => return Err(unavailable()),
    };

    if caller.tenant != claimed_tenant || caller.terminal != claimed_terminal {
        return Err(protocol_error(&ProtocolError::UnknownTerminal));
    }
    Ok(caller)
}

/// Sales from a till.
async fn push<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = postcard::from_bytes::<PushRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    if let Err(refusal) =
        authenticate(&state, &headers, request.tenant, request.terminal).await
    {
        return refusal;
    }

    match ingest::push(state.repo.as_ref(), &request).await {
        Ok(response) => encoded(&response),
        Err(IngestError::Protocol(error)) => protocol_error(&error),
        // The till keeps its copy and retries. Telling it otherwise would let it
        // drop the only record of a sale that already happened.
        Err(IngestError::Storage) => unavailable(),
    }
}

/// Catalogue changes to a till.
async fn pull<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = postcard::from_bytes::<PullRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    // The tenant comes from the credential, never from the body.
    match state
        .repo
        .items_since(caller.tenant, request.cursor, request.limit)
        .await
    {
        Ok(page) => encoded(&PullResponse {
            protocol,
            cursor: page.cursor,
            upserts: page.upserts,
            tombstones: page.tombstones,
            more: page.more,
        }),
        Err(_) => unavailable(),
    }
}

/// A block of receipt numbers for a till.
async fn lease<R: Repository>(
    State(state): State<AppState<R>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = postcard::from_bytes::<LeaseRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };
    let caller = match authenticate(&state, &headers, request.tenant, request.terminal).await {
        Ok(caller) => caller,
        Err(refusal) => return refusal,
    };

    match state
        .repo
        .issue_lease(caller.tenant, caller.terminal, request.count)
        .await
    {
        Ok(record) => encoded(&LeaseResponse {
            protocol,
            epoch: record.epoch,
            // Short and human readable, because it is printed on every receipt
            // and read aloud over the phone when something is disputed.
            prefix: format!("T{:X}", record.terminal & 0xFFFF),
            first: record.first,
            last: record.last,
        }),
        Err(RepoError::UnknownTerminal) => protocol_error(&ProtocolError::UnknownTerminal),
        Err(RepoError::Backend) => unavailable(),
    }
}

/// Trade a short code for a real credential.
///
/// The only route that takes no token, because it is how a device gets one. It
/// also takes no tenant and no terminal: both come from the code, so a device
/// cannot enrol itself into a shop it was not invited to.
///
/// Not yet rate limited. A code is eight characters, single use, and expires in
/// minutes, which makes guessing impractical rather than impossible; a limit on
/// attempts per address belongs here before this is exposed to the internet.
async fn enrol<R: Repository>(State(state): State<AppState<R>>, body: Bytes) -> Response {
    let Ok(request) = postcard::from_bytes::<EnrolRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };

    let caller = match state
        .repo
        .redeem_enrolment_code(&EnrolmentCode::hash_of(&request.code))
        .await
    {
        Ok(Some(caller)) => caller,
        // Unknown, expired and already used are one answer, so probing tells an
        // attacker nothing about which it was.
        Ok(None) => return protocol_error(&ProtocolError::Unauthenticated),
        Err(_) => return unavailable(),
    };

    let token = Token::generate();
    if state.repo.store_token(caller, &token.hash()).await.is_err() {
        return unavailable();
    }

    encoded(&EnrolResponse {
        protocol,
        tenant: caller.tenant,
        terminal: caller.terminal,
        token: token.into_string(),
    })
}

fn encoded<T: serde::Serialize>(value: &T) -> Response {
    match postcard::to_allocvec(value) {
        Ok(bytes) => ([(header::CONTENT_TYPE, CONTENT_TYPE)], bytes).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// A refusal the client can act on.
///
/// The reason travels in the body in the same encoding as everything else, so a
/// till can tell "you are too old, upgrade" apart from "that terminal is not
/// yours", rather than seeing an opaque 400 and retrying forever.
fn protocol_error(error: &ProtocolError) -> Response {
    let status = match error {
        ProtocolError::UnsupportedVersion { .. } => StatusCode::UPGRADE_REQUIRED,
        ProtocolError::UnknownTerminal => StatusCode::FORBIDDEN,
        ProtocolError::Unauthenticated => StatusCode::UNAUTHORIZED,
        ProtocolError::Malformed => StatusCode::BAD_REQUEST,
    };
    match postcard::to_allocvec(error) {
        Ok(bytes) => (status, [(header::CONTENT_TYPE, CONTENT_TYPE)], bytes).into_response(),
        Err(_) => status.into_response(),
    }
}

/// Temporary failure. Distinct from a refusal on purpose: a till must retry this
/// one, and must not retry a refusal.
fn unavailable() -> Response {
    StatusCode::SERVICE_UNAVAILABLE.into_response()
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing
    )]

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use openpos_core::protocol::{ItemWire, PROTOCOL_VERSION};
    use tower::ServiceExt;

    use super::*;
    use crate::repo::MemoryRepo;

    const TENANT: u128 = 42;
    const TERMINAL: u128 = 7;

    /// A shop, a terminal, a small catalogue, and the terminal's credential.
    fn app() -> (Router, String) {
        let repo = MemoryRepo::new();
        let token = repo.enrol_with_token(TENANT, TERMINAL);
        repo.upsert_item(TENANT, item(1));
        repo.upsert_item(TENANT, item(2));
        repo.delete_item(TENANT, 1);
        (router(AppState::new(repo)), token.into_string())
    }

    fn item(id: u128) -> ItemWire {
        ItemWire {
            id,
            code: format!("SKU{id:03}"),
            name_en: "Rice Miniket 5kg".to_owned(),
            name_bn: "মিনিকেট চাল ৫ কেজি".to_owned(),
            unit: "Nos".to_owned(),
            price_minor: 43_000,
            cost_minor: 38_000,
            vat_bp: 1_500,
            price_inclusive: false,
            barcodes: vec![format!("869000000{id:04}")],
            on_hand_milli: 40_000,
            active: true,
        }
    }

    async fn post_to<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        app: Router,
        path: &str,
        body: &T,
        token: Option<&str>,
    ) -> (StatusCode, Option<R>) {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, CONTENT_TYPE);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = builder
            .body(Body::from(postcard::to_allocvec(body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, postcard::from_bytes::<R>(&bytes).ok())
    }

    #[tokio::test]
    async fn reports_health_without_a_credential() {
        // Health is the one unauthenticated route: a load balancer has no token
        // and needs to know whether the process is alive.
        let (app, _) = app();
        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn pulls_catalogue_changes_from_a_cursor() {
        let request = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 10,
        };
        let (app, token) = app();
        let (status, body) =
            post_to::<_, PullResponse>(app, "/v1/sync/pull", &request, Some(&token)).await;

        assert_eq!(status, StatusCode::OK);
        let page = body.unwrap();
        assert_eq!(page.upserts.len(), 2);
        assert_eq!(page.tombstones, vec![1]);
        assert_eq!(page.cursor, 3);
        assert!(!page.more);
    }

    #[tokio::test]
    async fn pages_a_large_catalogue() {
        let first = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            cursor: 0,
            limit: 2,
        };
        let (app, token) = app();
        let (_, body) =
            post_to::<_, PullResponse>(app.clone(), "/v1/sync/pull", &first, Some(&token)).await;
        let page = body.unwrap();
        assert_eq!(page.upserts.len(), 2);
        assert!(page.more, "a till must know to ask again");

        let next = PullRequest {
            cursor: page.cursor,
            ..first
        };
        let (_, body) =
            post_to::<_, PullResponse>(app, "/v1/sync/pull", &next, Some(&token)).await;
        let page = body.unwrap();
        assert_eq!(page.tombstones, vec![1]);
        assert!(!page.more);
    }

    #[tokio::test]
    async fn issues_a_lease_block() {
        let request = LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 500,
        };
        let (app, token) = app();
        let (status, body) =
            post_to::<_, LeaseResponse>(app, "/v1/lease", &request, Some(&token)).await;

        assert_eq!(status, StatusCode::OK);
        let lease = body.unwrap();
        assert_eq!((lease.first, lease.last), (1, 500));
        assert_eq!(lease.epoch, 1);
        assert_eq!(lease.prefix, "T7");
    }

    #[tokio::test]
    async fn tells_an_old_client_to_upgrade_rather_than_failing_opaquely() {
        let request = LeaseRequest {
            protocol: 99,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 10,
        };
        let (app, token) = app();
        let (status, body) =
            post_to::<_, ProtocolError>(app, "/v1/lease", &request, Some(&token)).await;

        assert_eq!(status, StatusCode::UPGRADE_REQUIRED);
        assert!(matches!(
            body,
            Some(ProtocolError::UnsupportedVersion { requested: 99, .. })
        ));
    }

    #[tokio::test]
    async fn refuses_a_terminal_that_is_not_enrolled() {
        let request = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: 999,
            cursor: 0,
            limit: 10,
        };
        let (app, token) = app();
        let (status, body) =
            post_to::<_, ProtocolError>(app, "/v1/sync/pull", &request, Some(&token)).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, Some(ProtocolError::UnknownTerminal));
    }

    #[tokio::test]
    async fn a_different_shop_sees_nothing() {
        let request = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: 999,
            terminal: TERMINAL,
            cursor: 0,
            limit: 10,
        };
        let (app, token) = app();
        let (status, _) =
            post_to::<_, ProtocolError>(app, "/v1/sync/pull", &request, Some(&token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a terminal belongs to one tenant");
    }

    #[tokio::test]
    async fn a_new_tablet_trades_a_code_for_a_credential() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let code = EnrolmentCode::generate();
        repo.issue_enrolment_code(
            Caller { tenant: TENANT, terminal: TERMINAL },
            &code.hash(),
            std::time::Duration::from_secs(900),
        )
        .await
        .unwrap();
        let app = router(AppState::new(repo));

        // Typed by a person, with the grouping and case they actually use.
        let typed = format!("{} {}", &code.as_str()[..4], code.as_str()[4..].to_lowercase());
        let request = EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code: typed,
        };
        let (status, body) =
            post_to::<_, EnrolResponse>(app.clone(), "/v1/enrol", &request, None).await;

        assert_eq!(status, StatusCode::OK);
        let enrolled = body.unwrap();
        assert_eq!(enrolled.tenant, TENANT);
        assert_eq!(enrolled.terminal, TERMINAL);
        assert_eq!(enrolled.token.len(), 64);

        // The credential it was handed actually works.
        let lease = LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 10,
        };
        let (status, _) =
            post_to::<_, LeaseResponse>(app, "/v1/lease", &lease, Some(&enrolled.token)).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn an_enrolment_code_works_exactly_once() {
        let repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        let code = EnrolmentCode::generate();
        repo.issue_enrolment_code(
            Caller { tenant: TENANT, terminal: TERMINAL },
            &code.hash(),
            std::time::Duration::from_secs(900),
        )
        .await
        .unwrap();
        let app = router(AppState::new(repo));

        let request = EnrolRequest {
            protocol: PROTOCOL_VERSION,
            code: code.as_str().to_owned(),
        };
        let (first, _) =
            post_to::<_, EnrolResponse>(app.clone(), "/v1/enrol", &request, None).await;
        let (second, _) = post_to::<_, ProtocolError>(app, "/v1/enrol", &request, None).await;

        assert_eq!(first, StatusCode::OK);
        assert_eq!(second, StatusCode::UNAUTHORIZED, "a code is single use");
    }

    #[tokio::test]
    async fn a_revoked_credential_stops_working() {
        let repo = MemoryRepo::new();
        let token = repo.enrol_with_token(TENANT, TERMINAL);
        let hash = token.hash();
        let token = token.into_string();

        let request = LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 10,
        };

        let app = router(AppState::new(repo));
        let (before, _) =
            post_to::<_, LeaseResponse>(app.clone(), "/v1/lease", &request, Some(&token)).await;
        assert_eq!(before, StatusCode::OK);

        // The tablet is lost, so the shop withdraws its credential.
        let repo = MemoryRepo::new();
        let replacement = repo.enrol_with_token(TENANT, TERMINAL);
        repo.revoke_token(&hash).await.unwrap();
        let app = router(AppState::new(repo));

        let (after, _) =
            post_to::<_, ProtocolError>(app.clone(), "/v1/lease", &request, Some(&token)).await;
        assert_eq!(after, StatusCode::UNAUTHORIZED);

        // And the replacement device carries on.
        let (still_working, _) = post_to::<_, LeaseResponse>(
            app,
            "/v1/lease",
            &request,
            Some(replacement.as_str()),
        )
        .await;
        assert_eq!(still_working, StatusCode::OK);
    }

    #[tokio::test]
    async fn refuses_a_request_with_no_credential() {
        let request = LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 10,
        };
        let (app, _) = app();
        let (status, body) = post_to::<_, ProtocolError>(app, "/v1/lease", &request, None).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, Some(ProtocolError::Unauthenticated));
    }

    #[tokio::test]
    async fn refuses_a_credential_it_does_not_know() {
        let request = LeaseRequest {
            protocol: PROTOCOL_VERSION,
            tenant: TENANT,
            terminal: TERMINAL,
            count: 10,
        };
        let (app, _) = app();
        let (status, _) =
            post_to::<_, ProtocolError>(app, "/v1/lease", &request, Some("not a real token")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// The reason this exists: before authentication, a body could claim to be
    /// any shop and the server believed it.
    #[tokio::test]
    async fn a_valid_credential_cannot_be_used_to_claim_another_shop() {
        let repo = MemoryRepo::new();
        let intruder = repo.enrol_with_token(TENANT, TERMINAL);
        repo.enrol(999, 888);
        repo.upsert_item(999, item(7));
        let app = router(AppState::new(repo));

        let request = PullRequest {
            protocol: PROTOCOL_VERSION,
            tenant: 999,
            terminal: 888,
            cursor: 0,
            limit: 100,
        };
        let (status, body) = post_to::<_, ProtocolError>(
            app,
            "/v1/sync/pull",
            &request,
            Some(intruder.as_str()),
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, Some(ProtocolError::UnknownTerminal));
    }

    #[tokio::test]
    async fn rejects_a_body_that_is_not_a_request() {
        let (app, _) = app();
        let request = Request::builder()
            .method("POST")
            .uri("/v1/sync/push")
            .body(Body::from(vec![0xFF_u8; 8]))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
