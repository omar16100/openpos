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

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use openpos_core::protocol::{
    negotiate, LeaseRequest, LeaseResponse, ProtocolError, PullRequest, PullResponse, PushRequest,
};

use crate::ingest::{self, IngestError};
use crate::repo::{RepoError, Repository};

/// Content type for postcard bodies, versioned so a future encoding can be
/// introduced without guessing what a client sent.
pub const CONTENT_TYPE: &str = "application/vnd.openpos.v1+postcard";

/// Shared state. One lock for now: the workload is a handful of tills per shop,
/// and a lock held for the length of a batch insert is not the bottleneck. It
/// becomes one when the Postgres repository lands, and disappears with it.
#[derive(Clone)]
pub struct AppState {
    pub repo: Arc<Mutex<dyn Repository + Send>>,
}

impl AppState {
    #[must_use]
    pub fn new(repo: impl Repository + Send + 'static) -> Self {
        Self {
            repo: Arc::new(Mutex::new(repo)),
        }
    }
}

/// Build the router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/sync/push", post(push))
        .route("/v1/sync/pull", post(pull))
        .route("/v1/lease", post(lease))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

/// Sales from a till.
async fn push(State(state): State<AppState>, body: Bytes) -> Response {
    let Ok(request) = postcard::from_bytes::<PushRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };

    let outcome = {
        let Ok(mut repo) = state.repo.lock() else {
            return unavailable();
        };
        ingest::push(&mut *repo, &request)
    };

    match outcome {
        Ok(response) => encoded(&response),
        Err(IngestError::Protocol(error)) => protocol_error(&error),
        // The till keeps its copy and retries. Telling it otherwise would let it
        // drop the only record of a sale that already happened.
        Err(IngestError::Storage) => unavailable(),
    }
}

/// Catalogue changes to a till.
async fn pull(State(state): State<AppState>, body: Bytes) -> Response {
    let Ok(request) = postcard::from_bytes::<PullRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };

    let Ok(repo) = state.repo.lock() else {
        return unavailable();
    };

    match repo.terminal_enrolled(request.tenant, request.terminal) {
        Ok(true) => {}
        Ok(false) => return protocol_error(&ProtocolError::UnknownTerminal),
        Err(_) => return unavailable(),
    }

    match repo.items_since(request.tenant, request.cursor, request.limit) {
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
async fn lease(State(state): State<AppState>, body: Bytes) -> Response {
    let Ok(request) = postcard::from_bytes::<LeaseRequest>(&body) else {
        return protocol_error(&ProtocolError::Malformed);
    };
    let protocol = match negotiate(request.protocol) {
        Ok(version) => version,
        Err(error) => return protocol_error(&error),
    };

    let Ok(mut repo) = state.repo.lock() else {
        return unavailable();
    };

    match repo.issue_lease(request.tenant, request.terminal, request.count) {
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

    fn app() -> Router {
        let mut repo = MemoryRepo::new();
        repo.enrol(TENANT, TERMINAL);
        repo.upsert_item(TENANT, item(1));
        repo.upsert_item(TENANT, item(2));
        repo.delete_item(TENANT, 1);
        router(AppState::new(repo))
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
    ) -> (StatusCode, Option<R>) {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, CONTENT_TYPE)
            .body(Body::from(postcard::to_allocvec(body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, postcard::from_bytes::<R>(&bytes).ok())
    }

    #[tokio::test]
    async fn reports_health() {
        let response = app()
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
        let (status, body) = post_to::<_, PullResponse>(app(), "/v1/sync/pull", &request).await;

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
        let (_, body) = post_to::<_, PullResponse>(app(), "/v1/sync/pull", &first).await;
        let page = body.unwrap();
        assert_eq!(page.upserts.len(), 2);
        assert!(page.more, "a till must know to ask again");

        let next = PullRequest {
            cursor: page.cursor,
            ..first
        };
        let (_, body) = post_to::<_, PullResponse>(app(), "/v1/sync/pull", &next).await;
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
        let (status, body) = post_to::<_, LeaseResponse>(app(), "/v1/lease", &request).await;

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
        let (status, body) = post_to::<_, ProtocolError>(app(), "/v1/lease", &request).await;

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
        let (status, body) = post_to::<_, ProtocolError>(app(), "/v1/sync/pull", &request).await;

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
        let (status, _) = post_to::<_, ProtocolError>(app(), "/v1/sync/pull", &request).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a terminal belongs to one tenant");
    }

    #[tokio::test]
    async fn rejects_a_body_that_is_not_a_request() {
        let request = Request::builder()
            .method("POST")
            .uri("/v1/sync/push")
            .body(Body::from(vec![0xFF_u8; 8]))
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
