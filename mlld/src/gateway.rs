use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Json;
use axum::body::{Body, HttpBody};
use axum::extract::{Request, State};
use axum::extract::rejection::JsonRejection;
use axum::http::{Uri, StatusCode};
use axum::http::uri;
use axum::response::{IntoResponse, Response};
use futures::Stream;
use http_body_util::{BodyDataStream, BodyExt};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use mll_core::config;
use pin_project_lite::pin_project;
use serde::{Serialize, Deserialize};

use crate::ctxt::DaemonCtxt;
use crate::engine::{EngineState, EngineRequestResponderHandle};

pin_project! {
    struct ProxiedRequestEngineResponseStream<B> {
        #[pin]
        stream: BodyDataStream<B>,
        engine_request_responder_handle: Option<EngineRequestResponderHandle>,
    }
}

impl<B: HttpBody> ProxiedRequestEngineResponseStream<B> {
    pub fn new(stream: BodyDataStream<B>, engine_request_responder_handle: EngineRequestResponderHandle) -> Self {
        Self { stream, engine_request_responder_handle: Some(engine_request_responder_handle) }
    }
}

impl<B: HttpBody> Stream for ProxiedRequestEngineResponseStream<B> {
    type Item = Result<B::Data, B::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match this.stream.poll_next(cx) {
            Poll::Ready(None) => {
                // NOTE: End of engine response stream, drop the pending engine request handle to singal completion.
                drop(this.engine_request_responder_handle.take());
                Poll::Ready(None)
            }
            p => p,
        }
    }
}

#[derive(Clone)]
struct ServerState {
    dcx: Arc<DaemonCtxt>,
    client: Client<HttpConnector, Body>,
}

pub(crate) fn setup_routes(dcx: Arc<DaemonCtxt>) -> axum::Router {
    let mut http_connector = HttpConnector::new();
    http_connector.set_nodelay(true);
    http_connector.set_keepalive(Some(Duration::from_secs(60)));
    http_connector.set_connect_timeout(Some(Duration::from_secs(10)));
    http_connector.set_reuse_address(true);
    http_connector.enforce_http(false);

    let client = Client::builder(hyper_util::rt::TokioExecutor::new())
        .pool_idle_timeout(Duration::from_secs(60))
        .pool_max_idle_per_host(32)
        .retry_canceled_requests(true)
        .set_host(true)
        .build(http_connector);

    let state = ServerState {
        dcx,
        client,
    };

    axum::Router::new()
        .route("/health", axum::routing::get(handle_health_request))
        .route("/v1/models", axum::routing::get(handle_models_request))
        .route("/v1/completions", axum::routing::post(handle_proxied_engine_request))
        .route("/v1/chat/completions", axum::routing::post(handle_proxied_engine_request))
        .route("/v1/embeddings", axum::routing::post(handle_proxied_engine_request))
        .fallback(handle_unknown_request)
        .with_state(state)
}

async fn handle_unknown_request() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "404")
}

async fn handle_health_request() -> StatusCode {
    StatusCode::OK
}

#[derive(Serialize)]
#[serde(tag = "object", rename = "list")]
struct ModelsResponse {
    data: Vec<ModelObject>,
}

#[derive(Serialize)]
#[serde(tag = "object", rename = "model")]
struct ModelObject {
    id: String,
    owned_by: &'static str,
    created: u64,
    // Additional fields:
    max_model_len: usize,
}

async fn handle_models_request(State(state): State<ServerState>) -> Json<ModelsResponse> {
    let model_objects = state.dcx.engine_instances.read().iter()
        .filter_map(|engine_instance| {
            if *engine_instance.engine_state.read() != EngineState::Running { return None; }

            let engine_name = match engine_instance.engine_config.kind {
                config::EngineKind::Vllm => "vllm",
            };

            Some(ModelObject {
                id: engine_instance.model_name().to_owned(),
                owned_by: engine_name,
                created: 0,
                max_model_len: engine_instance.model_config.max_context_tokens,
            })
        })
        .collect::<Vec<_>>();

    Json(ModelsResponse { data: model_objects })
}

async fn handle_proxied_engine_request(State(state): State<ServerState>, request: Request) -> Result<Response, JsonRejection> {
    let (mut parts, body) = request.into_parts();

    // NOTE: This replicates the behavior of the axum::body::Bytes extractor impl.
    let body = match body.collect().await {
        Ok(v) => v.to_bytes(),
        Err(_error) => { return Ok((StatusCode::BAD_REQUEST, "Failed to buffer the request body").into_response()); }
    };

    #[derive(Deserialize)]
    struct RequestModelRoutingPart {
        model: String,
    }
    let Json(request_model_routing_part) = Json::<RequestModelRoutingPart>::from_bytes(&body)?;

    let Some(engine_instance) = state.dcx.engine_instances.read().iter().find(|engine_instance| {
        engine_instance.model_name() == request_model_routing_part.model && *engine_instance.engine_state.read() == EngineState::Running
    }).cloned() else {
        return Ok((StatusCode::SERVICE_UNAVAILABLE, format!("model `{}` not loaded", request_model_routing_part.model)).into_response())
    };

    let pending_engine_request = engine_instance.track_pending_engine_request();

    let engine_port = engine_instance.engine_port;

    let proxied_request = {
        let mut uri_parts = parts.uri.into_parts();
        uri_parts.scheme = Some(uri::Scheme::HTTP);
        uri_parts.authority = Some(uri::Authority::from_str(&format!("127.0.0.1:{}", engine_port)).expect("invalid proxied URI"));
        parts.uri = Uri::from_parts(uri_parts).expect("invalid proxied URI");
        Request::from_parts(parts, Body::from(body))
    };

    match state.client.request(proxied_request).await {
        Ok(response) => {
            // Convert the response's body type from `hyper::body::Incoming` into `axum::body::Body`.
            let (parts, body) = response.into_parts();
            // NOTE: Keep the pending engine request handle from dropping until the response stream from the proxied engine request is fully received.
            let response_body_data_stream = ProxiedRequestEngineResponseStream::new(body.into_data_stream(), pending_engine_request);
            let body = Body::from_stream(response_body_data_stream);
            let response = Response::from_parts(parts, body);

            Ok(response)
        }
        Err(error) => {
            Ok((StatusCode::BAD_GATEWAY, format!("failed to connect to model inference server: {}", error)).into_response())
        }
    }
}
