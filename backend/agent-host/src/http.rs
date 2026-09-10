use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::ws::{rejection::WebSocketUpgradeRejection, WebSocketUpgrade};
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;

use crate::server::Host;

pub(crate) struct HttpServer {
    inner: std::sync::Mutex<Option<Serving>>,
}

struct Serving {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct HttpState {
    host: Arc<Host>,
    token: Arc<str>,
    web_root: Option<Arc<Path>>,
}

#[derive(serde::Deserialize)]
struct AuthQuery {
    tkn: Option<String>,
}

impl HttpServer {
    pub(crate) fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn url(&self) -> Option<String> {
        self.inner
            .lock()
            .ok()?
            .as_ref()
            .map(|serving| serving.url.clone())
    }

    pub(crate) fn stop(&self) -> bool {
        let Some(serving) = self.inner.lock().ok().and_then(|mut held| held.take()) else {
            return false;
        };
        serving.task.abort();
        true
    }
}

pub(crate) fn resolve_web_root(cli: Option<&Path>) -> Option<PathBuf> {
    if let Some(root) = cli {
        return Some(root.to_owned());
    }
    if let Ok(root) = std::env::var("HIMARK_WEB_ROOT") {
        return Some(PathBuf::from(root));
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_owned();
    let parent = dir.parent()?;
    [
        dir.join("web"),
        parent.join("Resources").join("web"),
        parent.join("web"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
}

pub(crate) async fn start(
    host: &Arc<Host>,
    addr: &str,
    web_root: Option<PathBuf>,
) -> std::io::Result<String> {
    host.http.stop();
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    let token: Arc<str> = mint_token().into();

    let advertised = match local.ip().is_unspecified() {
        true => format!(
            "{}:{}",
            lan_ip().unwrap_or_else(|| "127.0.0.1".to_owned()),
            local.port()
        ),
        false => local.to_string(),
    };
    let url = format!("http://{advertised}/?tkn={token}");
    let web_root: Option<Arc<Path>> = web_root.map(Into::into);

    tracing::info!(target: "ahp_http", host = %host.trace.tag, %url, "serving http");

    eprintln!(
        "[agent-host] browser url: http://localhost:{}/?tkn={token}",
        local.port()
    );
    let state = HttpState {
        host: Arc::clone(host),
        token,
        web_root: web_root.clone(),
    };
    let mut app = Router::new()
        .route("/", get(index_or_websocket))
        .route("/index.html", get(index_or_websocket))
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }));
    if let Some(root) = web_root {
        app = app.fallback_service(ServeDir::new(root).precompressed_br().precompressed_gzip());
    }
    let app = app
        .layer(middleware::from_fn_with_state(
            state.clone(),
            authenticate_and_set_headers,
        ))
        .with_state(state);
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            tracing::error!(target: "ahp_http", %error, "http server stopped");
        }
    });
    if let Ok(mut held) = host.http.inner.lock() {
        *held = Some(Serving {
            url: url.clone(),
            task,
        });
    }
    Ok(url)
}

fn lan_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

fn mint_token() -> String {
    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
        .is_err()
    {
        tracing::warn!(target: "ahp_http", "no /dev/urandom — minting a weak token");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or(0);
        bytes[..16].copy_from_slice(&now.to_le_bytes());
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn authenticate_and_set_headers(
    State(state): State<HttpState>,
    request: Request,
    next: Next,
) -> Response {
    let immutable_asset = is_content_hashed_asset(request.uri().path());
    let query_token = Query::<AuthQuery>::try_from_uri(request.uri())
        .ok()
        .and_then(|query| query.0.tkn);
    let cookie_token = request
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .find_map(|cookie| cookie.trim().strip_prefix("himark_tkn="));
    if query_token.as_deref() != Some(state.token.as_ref())
        && cookie_token != Some(state.token.as_ref())
    {
        return (StatusCode::FORBIDDEN, "missing or wrong token").into_response();
    }

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if immutable_asset {
            "private, max-age=31536000, immutable"
        } else {
            "no-cache"
        }),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-embedder-policy"),
        HeaderValue::from_static("require-corp"),
    );
    response
}

fn is_content_hashed_asset(path: &str) -> bool {
    let Some(name) = path.rsplit('/').next() else {
        return false;
    };
    let Some(stem) = name
        .strip_suffix(".wasm")
        .or_else(|| name.strip_suffix(".scm"))
    else {
        return false;
    };
    let Some((stable_name, hash)) = stem.rsplit_once('-') else {
        return false;
    };
    if stable_name.is_empty() {
        return false;
    }
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn index_or_websocket(
    State(state): State<HttpState>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let asked_to_upgrade = headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    match upgrade {
        Ok(upgrade) => {
            let host = Arc::clone(&state.host);
            upgrade
                .on_upgrade(move |socket| async move {
                    let _ = host.serve_ws(socket).await;
                })
                .into_response()
        }
        Err(rejection) if asked_to_upgrade => rejection.into_response(),
        Err(_) => serve_index(&state, &headers).await,
    }
}

async fn serve_index(state: &HttpState, headers: &HeaderMap) -> Response {
    let Some(root) = &state.web_root else {
        return (StatusCode::NOT_FOUND, "no web root configured").into_response();
    };
    let bytes = match tokio::fs::read(root.join("index.html")).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    let authority = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("127.0.0.1");
    let url = format!("ws://{authority}/?tkn={}", state.token);
    let encoded_url = serde_json::to_string(&url).expect("a string encodes");
    let inject = format!("<head><script>window.HIMARK_AHP_URL={encoded_url};</script>");
    let text = String::from_utf8_lossy(&bytes).replacen("<head>", &inject, 1);
    let mut response = Html(text).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "himark_tkn={}; HttpOnly; SameSite=Strict",
            state.token
        ))
        .expect("the hex token is a header value"),
    );
    response
}
