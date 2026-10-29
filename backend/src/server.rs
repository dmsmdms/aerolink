//! HTTP API and WebSocket glue (docs/08-backend.md). nginx terminates TLS, serves static files and
//! filters abuse; this process only sees `/api/` and `/ws/`.
//!
//! Single-threaded (current-thread runtime): shared state sits behind std mutexes that are never held
//! across an `.await`, so they are uncontended and cost next to nothing.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade, close_code};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};

use crate::auth;
use crate::config::Config;
use crate::db::{self, Db, User};
use crate::hub::{ConnId, Hub, IceServer, OperatorMsg, Out, Peer, RobotId, RobotMsg, ServerMsg, SessionId, Target, UserId};
use crate::release::{self, Release};

/// SDPs are a few KiB; anything near this is abuse.
const MAX_WS_MESSAGE: usize = 64 * 1024;
const MAX_BODY: usize = 16 * 1024;
/// Per-connection outgoing queue; a client this far behind is dropped.
const OUT_QUEUE: usize = 128;
/// Robots ping every 2–4 min (PWR-012); consoles every 30 s.
const ROBOT_IDLE: Duration = Duration::from_secs(10 * 60);
const OPERATOR_IDLE: Duration = Duration::from_secs(2 * 60);
/// Token bucket per connection: ICE gathering comes in bursts, steady traffic is tiny.
const RATE_BURST: f64 = 100.0;
const RATE_PER_SEC: f64 = 20.0;

#[derive(Clone, Copy)]
enum Who {
    Robot(RobotId),
    Operator(UserId),
}

struct Conn {
    tx: mpsc::Sender<Message>,
    who: Who,
}

struct Signaling {
    hub: Hub,
    conns: HashMap<ConnId, Conn>,
}

pub struct App {
    cfg: Config,
    db: Mutex<Db>,
    signaling: Mutex<Signaling>,
    next_conn: AtomicU64,
    /// Robot app release announced to robots (ROB-008).
    release: Mutex<Option<Release>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not take the whole service down with poisoned locks.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl App {
    pub fn new(cfg: Config, db: Db) -> rusqlite::Result<Arc<Self>> {
        let known = db.robots()?.into_iter().filter(|(_, _, disabled)| !disabled).map(|(id, name, _)| (id, name));
        let release = load_release(&cfg);
        Ok(Arc::new(App {
            cfg,
            signaling: Mutex::new(Signaling { hub: Hub::new(known), conns: HashMap::new() }),
            db: Mutex::new(db),
            next_conn: AtomicU64::new(1),
            release: Mutex::new(release),
        }))
    }

    /// SIGHUP (`systemctl reload aerolink`): reads the release again and announces a changed one to every
    /// connected robot. A broken release keeps the previous one.
    pub fn reload_release(&self) {
        let dir = &self.cfg.apk_dir;
        let release = match release::load(dir) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("app release in {}: {e}; keeping the previous one", dir.display());
                return;
            }
        };
        let mut current = lock(&self.release);
        if *current == release {
            tracing::info!("app release unchanged");
            return;
        }
        *current = release.clone();
        drop(current);
        let Some(release) = release else {
            tracing::info!("app release withdrawn");
            return;
        };
        tracing::info!(version = release.version_code, "app release {} announced", release.version_name);
        let mut sig = lock(&self.signaling);
        let robots: Vec<ConnId> =
            sig.conns.iter().filter(|(_, c)| matches!(c.who, Who::Robot(_))).map(|(&id, _)| id).collect();
        let out = robots.into_iter().map(|id| (Target::Conn(id), ServerMsg::Update(release.clone()))).collect();
        self.dispatch(&mut sig, out);
    }

    fn db(&self) -> MutexGuard<'_, Db> {
        lock(&self.db)
    }

    fn ice_servers(&self, who: Who) -> Vec<IceServer> {
        let mut servers = Vec::new();
        if !self.cfg.stun_urls.is_empty() {
            servers.push(IceServer { urls: self.cfg.stun_urls.clone(), username: None, credential: None });
        }
        if !self.cfg.turn_urls.is_empty() && !self.cfg.turn_secret.is_empty() {
            let user = match who {
                Who::Robot(id) => format!("robot{id}"),
                Who::Operator(id) => format!("op{id}"),
            };
            let (username, credential) = auth::turn_credentials(&self.cfg.turn_secret, &user, db::now());
            servers.push(IceServer { urls: self.cfg.turn_urls.clone(), username: Some(username), credential: Some(credential) });
        }
        servers
    }

    /// Delivers hub output. A full queue means a stuck client: dropping its sender closes it.
    fn dispatch(&self, sig: &mut Signaling, out: Out) {
        for (target, mut msg) in out {
            match target {
                Target::Conn(id) => {
                    let Some(conn) = sig.conns.get(&id) else { continue };
                    if let ServerMsg::Session { ice_servers, .. } = &mut msg {
                        *ice_servers = self.ice_servers(conn.who);
                    }
                    let text = encode(&msg);
                    send(sig, id, text);
                }
                Target::AllOperators => {
                    let text = encode(&msg);
                    let ids: Vec<ConnId> =
                        sig.conns.iter().filter(|(_, c)| matches!(c.who, Who::Operator(_))).map(|(&id, _)| id).collect();
                    for id in ids {
                        send(sig, id, text.clone());
                    }
                }
            }
        }
    }
}

fn load_release(cfg: &Config) -> Option<Release> {
    match release::load(&cfg.apk_dir) {
        Ok(Some(r)) => {
            tracing::info!(version = r.version_code, "app release {}", r.version_name);
            Some(r)
        }
        Ok(None) => None,
        Err(e) => {
            tracing::error!("app release in {}: {e}", cfg.apk_dir.display());
            None
        }
    }
}

fn encode(msg: &ServerMsg) -> Utf8Bytes {
    serde_json::to_string(msg).expect("server messages always serialize").into()
}

fn send(sig: &mut Signaling, id: ConnId, text: Utf8Bytes) {
    let Some(conn) = sig.conns.get(&id) else { return };
    if let Err(mpsc::error::TrySendError::Full(_)) = conn.tx.try_send(Message::Text(text)) {
        tracing::warn!(conn = id, "client too slow, disconnecting");
        sig.conns.remove(&id);
    }
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/login", post(login))
        .route("/api/v1/logout", post(logout))
        .route("/api/v1/me", get(me))
        .route("/api/v1/robot/apk/{abi}", get(robot_apk))
        .route("/ws/operator", get(operator_ws))
        .route("/ws/robot", get(robot_ws))
        .fallback(|| async { ApiError::NotFound })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(app)
}

// --- errors ---

enum ApiError {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Internal,
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        tracing::error!("database: {e}");
        ApiError::Internal
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            ApiError::BadRequest => (StatusCode::BAD_REQUEST, "bad_request"),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        (status, Json(json!({ "error": code }))).into_response()
    }
}

/// Client address as seen by nginx (the socket peer is nginx itself).
fn client_ip(headers: &HeaderMap) -> &str {
    headers.get("x-real-ip").and_then(|v| v.to_str().ok()).unwrap_or("-")
}

fn user_json(u: &User) -> serde_json::Value {
    json!({ "id": u.id, "email": u.email, "name": u.name, "role": u.role })
}

fn session_user(app: &App, headers: &HeaderMap) -> Result<User, ApiError> {
    let token = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| auth::cookie(v, auth::cookie_name(app.cfg.secure_cookies)))
        .ok_or(ApiError::Unauthorized)?;
    app.db().session_user(&auth::token_hash(token))?.ok_or(ApiError::Unauthorized)
}

// --- HTTP ---

async fn health(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let sig = lock(&app.signaling);
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "operators": sig.hub.operator_count(),
        "robots_online": sig.hub.online_robot_count(),
    }))
}

#[derive(Deserialize)]
struct Login {
    email: String,
    password: String,
}

async fn login(State(app): State<Arc<App>>, headers: HeaderMap, Json(req): Json<Login>) -> Result<Response, ApiError> {
    if req.email.len() > 254 || req.password.len() > 1024 {
        return Err(ApiError::BadRequest);
    }
    let found = app.db().user_for_login(&req.email)?;
    let user = match found {
        Some((user, hash)) if auth::verify_password(&req.password, &hash) => user,
        found => {
            if found.is_none() {
                auth::verify_dummy(&req.password);
            }
            tracing::warn!(ip = client_ip(&headers), email = %req.email, "login failed");
            return Err(ApiError::Unauthorized);
        }
    };
    let token = auth::new_token();
    app.db().add_session(&auth::token_hash(&token), user.id, db::now() + auth::SESSION_TTL_SECS)?;
    tracing::info!(ip = client_ip(&headers), user = user.id, "login");
    let cookie = HeaderValue::from_str(&auth::session_cookie(&token, app.cfg.secure_cookies)).expect("ascii cookie");
    Ok(([(header::SET_COOKIE, cookie)], Json(user_json(&user))).into_response())
}

async fn logout(State(app): State<Arc<App>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let name = auth::cookie_name(app.cfg.secure_cookies);
    for v in headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()) {
        if let Some(token) = auth::cookie(v, name) {
            app.db().delete_session(&auth::token_hash(token))?;
        }
    }
    let cookie = HeaderValue::from_str(&auth::clear_cookie(app.cfg.secure_cookies)).expect("ascii cookie");
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

async fn me(State(app): State<Arc<App>>, headers: HeaderMap) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(user_json(&session_user(&app, &headers)?)))
}

// --- WebSockets ---

async fn operator_ws(State(app): State<Arc<App>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Result<Response, ApiError> {
    // Cookies ride along on cross-site WebSocket handshakes: the Origin check is the CSRF defence.
    if !app.cfg.origin.is_empty() {
        let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
        if origin != Some(app.cfg.origin.as_str()) {
            tracing::warn!(ip = client_ip(&headers), origin = ?origin, "operator ws: foreign origin");
            return Err(ApiError::Forbidden);
        }
    }
    let user = session_user(&app, &headers)?;
    Ok(ws.max_message_size(MAX_WS_MESSAGE).max_frame_size(MAX_WS_MESSAGE).on_upgrade(move |socket| async move {
        let peer = Peer { id: user.id, name: user.name };
        run_conn(app, socket, Who::Operator(peer.id), OPERATOR_IDLE, |hub, conn| (hub.operator_connected(conn, peer), None))
            .await
    }))
}

/// `/ws/robot?session=<id>`: the robot reconnects while still in that session (SIG-007).
#[derive(Deserialize)]
struct RobotQuery {
    session: Option<SessionId>,
}

/// The robot of the `Authorization: Bearer <id>.<secret>` header.
fn robot_auth(app: &App, headers: &HeaderMap, what: &str) -> Result<(RobotId, String), ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(auth::parse_robot_token);
    let robot = match token {
        Some((id, secret)) => app.db().robot_for_login(id)?.and_then(|(name, hash)| {
            auth::constant_time_eq(&auth::token_hash(secret), &hash).then_some((id, name))
        }),
        None => None,
    };
    robot.ok_or_else(|| {
        tracing::warn!(ip = client_ip(headers), "{what}: bad token");
        ApiError::Unauthorized
    })
}

async fn robot_ws(
    State(app): State<Arc<App>>,
    Query(query): Query<RobotQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let (id, name) = robot_auth(&app, &headers, "robot ws")?;
    let resume = query.session;
    tracing::info!(ip = client_ip(&headers), robot = id, resume, "robot connected");
    let update = lock(&app.release).clone();
    Ok(ws.max_message_size(MAX_WS_MESSAGE).max_frame_size(MAX_WS_MESSAGE).on_upgrade(move |socket| {
        run_conn(app, socket, Who::Robot(id), ROBOT_IDLE, move |hub, conn| {
            let (mut out, replaced) = hub.robot_connected(conn, id, name, resume);
            out.extend(update.map(|r| (Target::Conn(conn), ServerMsg::Update(r))));
            (out, replaced)
        })
    }))
}

/// `GET /api/v1/robot/apk/<abi>`: the APK of the current release (ROB-008), robots only. Behind nginx the file
/// is sent by nginx (`X-Accel-Redirect` to an `internal` location), so this thread never streams megabytes.
async fn robot_apk(State(app): State<Arc<App>>, Path(abi): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let (robot, _) = robot_auth(&app, &headers, "robot apk")?;
    let file = lock(&app.release).as_ref().and_then(|r| r.apks.get(&abi)).map(|apk| apk.file.clone());
    let Some(file) = file else { return Err(ApiError::NotFound) };
    tracing::info!(ip = client_ip(&headers), robot, abi, "app download");
    let apk_type = HeaderValue::from_static("application/vnd.android.package-archive");
    if !app.cfg.apk_accel.is_empty() {
        let target = HeaderValue::from_str(&format!("{}{file}", app.cfg.apk_accel)).map_err(|_| ApiError::Internal)?;
        return Ok(([(header::CONTENT_TYPE, apk_type), (HeaderName::from_static("x-accel-redirect"), target)]).into_response());
    }
    // Bench only: a few ms of this thread for a file in the page cache.
    let body = std::fs::read(app.cfg.apk_dir.join(&file)).map_err(|e| {
        tracing::error!("{file}: {e}");
        ApiError::Internal
    })?;
    Ok(([(header::CONTENT_TYPE, apk_type)], Body::from(body)).into_response())
}

async fn run_conn(
    app: Arc<App>,
    mut socket: WebSocket,
    who: Who,
    idle: Duration,
    join: impl FnOnce(&mut Hub, ConnId) -> (Out, Option<ConnId>),
) {
    let id = app.next_conn.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel(OUT_QUEUE);
    {
        let mut sig = lock(&app.signaling);
        sig.conns.insert(id, Conn { tx, who });
        let (out, replaced) = join(&mut sig.hub, id);
        if let Some(old) = replaced {
            sig.conns.remove(&old); // closes the stale connection
        }
        app.dispatch(&mut sig, out);
    }

    let mut tokens = RATE_BURST;
    let mut refilled = Instant::now();
    let reason = loop {
        tokio::select! {
            incoming = timeout(idle, socket.recv()) => {
                let text = match incoming {
                    Err(_) => break "idle timeout",
                    Ok(None | Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => break "closed",
                    Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
                    Ok(Some(Ok(Message::Binary(_)))) => break "binary frame",
                    Ok(Some(Ok(Message::Text(text)))) => text,
                };
                let now = Instant::now();
                tokens = (tokens + now.duration_since(refilled).as_secs_f64() * RATE_PER_SEC).min(RATE_BURST);
                refilled = now;
                if tokens < 1.0 {
                    break "rate limit";
                }
                tokens -= 1.0;
                let mut sig = lock(&app.signaling);
                let out = match who {
                    Who::Robot(_) => serde_json::from_str::<RobotMsg>(&text).map(|m| sig.hub.from_robot(id, m)),
                    Who::Operator(_) => serde_json::from_str::<OperatorMsg>(&text).map(|m| sig.hub.from_operator(id, m)),
                };
                match out {
                    Ok(out) => app.dispatch(&mut sig, out),
                    Err(_) => break "malformed message",
                }
            }
            outgoing = rx.recv() => match outgoing {
                Some(msg) => if socket.send(msg).await.is_err() { break "send failed" },
                None => break "replaced or too slow",
            }
        }
    };

    if reason != "closed" {
        let close = CloseFrame { code: close_code::POLICY, reason: reason.into() };
        let _ = socket.send(Message::Close(Some(close))).await;
    }
    let mut sig = lock(&app.signaling);
    sig.conns.remove(&id);
    let out = sig.hub.disconnected(id);
    app.dispatch(&mut sig, out);
    match who {
        Who::Robot(robot) => tracing::info!(robot, reason, "robot disconnected"),
        Who::Operator(user) => tracing::debug!(user, reason, "operator disconnected"),
    }
}

// --- listening ---

pub async fn serve(app: Arc<App>, listen: &str) -> io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut hangup = signal(SignalKind::hangup())?;
    let reloading = app.clone();
    tokio::spawn(async move {
        while hangup.recv().await.is_some() {
            reloading.reload_release();
        }
    });
    let router = router(app);
    if listen == "systemd" {
        let listener = systemd_listener()?;
        tracing::info!("listening on the systemd socket");
        axum::serve(listener, router).with_graceful_shutdown(shutdown_signal()).await
    } else if let Some(path) = listen.strip_prefix("unix:") {
        let _ = std::fs::remove_file(path);
        let listener = tokio::net::UnixListener::bind(path)?;
        tracing::info!("listening on {listen}");
        axum::serve(listener, router).with_graceful_shutdown(shutdown_signal()).await
    } else {
        let listener = tokio::net::TcpListener::bind(listen).await?;
        tracing::info!("listening on {}", listener.local_addr()?);
        axum::serve(listener, router).with_graceful_shutdown(shutdown_signal()).await
    }
}

/// Socket activation (`aerolink.socket`): fd 3 is a listening Unix socket owned by systemd, so restarts
/// never refuse connections and nginx needs no TCP port.
fn systemd_listener() -> io::Result<tokio::net::UnixListener> {
    use std::os::fd::FromRawFd;
    let fd = systemd_fd("AEROLINK_LISTEN")?;
    // SAFETY: systemd passed exactly this one fd and nothing else owns it.
    let std_listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
    std_listener.set_nonblocking(true)?;
    tokio::net::UnixListener::from_std(std_listener)
}

/// The single socket systemd passed to this process (fd 3, SD_LISTEN_FDS_START). `key` names the
/// setting that asked for it, for the error message.
pub(crate) fn systemd_fd(key: &str) -> io::Result<std::os::fd::RawFd> {
    let ours = std::env::var("LISTEN_PID").ok().and_then(|p| p.parse::<u32>().ok()) == Some(std::process::id());
    let fds = std::env::var("LISTEN_FDS").ok().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
    if !ours || fds != 1 {
        return Err(io::Error::other(format!("{key}=systemd but no socket was passed (LISTEN_PID/LISTEN_FDS)")));
    }
    Ok(3)
}

pub(crate) async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

    type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

    const ORIGIN: &str = "https://console.test";

    async fn start() -> (String, String) {
        let (addr, token, _) = start_app(|_| {}).await;
        (addr, token)
    }

    async fn start_app(tweak: impl FnOnce(&mut Config)) -> (String, String, Arc<App>) {
        let db = Db::memory().unwrap();
        db.add_user("op@x.org", "Op", &auth::hash_password("password123")).unwrap();
        let secret = auth::new_token();
        let robot = db.add_robot("r1", &auth::token_hash(&secret)).unwrap();
        let mut cfg = Config::from_lookup(|_| None);
        cfg.origin = ORIGIN.into();
        cfg.secure_cookies = false;
        cfg.stun_urls = vec!["stun:stun.test:3478".into()];
        cfg.turn_urls = vec!["turn:turn.test:3478".into()];
        cfg.turn_secret = "s".into();
        cfg.apk_dir = "/nonexistent".into();
        tweak(&mut cfg);
        let app = App::new(cfg, db).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let serving = app.clone();
        tokio::spawn(async move { axum::serve(listener, router(serving)).await.unwrap() });
        (addr, auth::robot_token(robot, &secret), app)
    }

    async fn login(addr: &str, password: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("http://{addr}/api/v1/login"))
            .json(&json!({ "email": "OP@x.org", "password": password }))
            .send()
            .await
            .unwrap()
    }

    async fn connect(addr: &str, path: &str, headers: &[(&'static str, &str)]) -> Result<Ws, tungstenite::Error> {
        let mut req = format!("ws://{addr}{path}").into_client_request().unwrap();
        for (k, v) in headers {
            req.headers_mut().insert(*k, v.parse().unwrap());
        }
        tokio_tungstenite::connect_async(req).await.map(|(ws, _)| ws)
    }

    async fn recv(ws: &mut Ws) -> serde_json::Value {
        let msg = tokio::time::timeout(Duration::from_secs(2), ws.next()).await.expect("message").unwrap().unwrap();
        serde_json::from_str(msg.to_text().unwrap()).unwrap()
    }

    async fn send(ws: &mut Ws, v: serde_json::Value) {
        ws.send(tungstenite::Message::text(v.to_string())).await.unwrap();
    }

    fn status(err: tungstenite::Error) -> u16 {
        match err {
            tungstenite::Error::Http(r) => r.status().as_u16(),
            e => panic!("unexpected {e}"),
        }
    }

    #[tokio::test]
    async fn login_and_me() {
        let (addr, _) = start().await;
        assert_eq!(login(&addr, "wrong-password").await.status(), 401);
        let r = login(&addr, "password123").await;
        assert_eq!(r.status(), 200);
        let cookie = r.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_owned();
        assert!(cookie.starts_with("aerolink="));
        let me = reqwest::Client::new().get(format!("http://{addr}/api/v1/me")).header("cookie", &cookie).send().await.unwrap();
        assert_eq!(me.json::<serde_json::Value>().await.unwrap()["name"], "Op");
        let anon = reqwest::get(format!("http://{addr}/api/v1/me")).await.unwrap();
        assert_eq!(anon.status(), 401);
        let missing = reqwest::get(format!("http://{addr}/api/v1/nope")).await.unwrap();
        assert_eq!(missing.status(), 404);
    }

    #[tokio::test]
    async fn signaling_end_to_end() {
        let (addr, robot_token) = start().await;
        let r = login(&addr, "password123").await;
        let cookie = r.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_owned();

        // Rejections: no cookie, foreign origin, bad robot token.
        assert_eq!(status(connect(&addr, "/ws/operator", &[("origin", ORIGIN)]).await.unwrap_err()), 401);
        let evil = [("origin", "https://evil.test"), ("cookie", cookie.as_str())];
        assert_eq!(status(connect(&addr, "/ws/operator", &evil).await.unwrap_err()), 403);
        let bad = format!("Bearer {}x", robot_token);
        assert_eq!(status(connect(&addr, "/ws/robot", &[("authorization", &bad)]).await.unwrap_err()), 401);

        let mut op = connect(&addr, "/ws/operator", &[("origin", ORIGIN), ("cookie", &cookie)]).await.unwrap();
        assert_eq!(recv(&mut op).await["type"], "welcome");
        let robots = recv(&mut op).await;
        assert_eq!(robots["robots"][0]["online"], false);

        let bearer = format!("Bearer {robot_token}");
        let mut robot = connect(&addr, "/ws/robot", &[("authorization", &bearer)]).await.unwrap();
        assert_eq!(recv(&mut robot).await["you"]["name"], "r1");
        assert_eq!(recv(&mut op).await["robot"]["online"], true);

        send(&mut robot, json!({ "type": "status", "mode": "NEEDS_HELP", "phone_battery": 52 })).await;
        assert_eq!(recv(&mut op).await["robot"]["status"]["mode"], "NEEDS_HELP");

        send(&mut op, json!({ "type": "take", "robot": 1 })).await;
        let started = recv(&mut robot).await;
        assert_eq!(started["type"], "session");
        assert_eq!(started["peer"]["name"], "Op");
        assert!(started["ice_servers"][1]["username"].as_str().unwrap().ends_with(":robot1"));
        let session = started["session"].clone();
        assert_eq!(recv(&mut op).await["robot"]["operator"], 1);
        let granted = recv(&mut op).await;
        assert_eq!(granted["type"], "session");
        assert!(granted["ice_servers"][1]["username"].as_str().unwrap().ends_with(":op1"));

        let sdp = json!({ "type": "offer", "sdp": "v=0\r\n" });
        send(&mut op, json!({ "type": "offer", "session": session, "sdp": sdp })).await;
        assert_eq!(recv(&mut robot).await["sdp"], sdp);
        send(&mut robot, json!({ "type": "answer", "session": session, "sdp": "a" })).await;
        assert_eq!(recv(&mut op).await["type"], "answer");

        send(&mut op, json!({ "type": "ping" })).await;
        assert_eq!(recv(&mut op).await["type"], "pong");

        // Network change: the robot comes back on a new connection and keeps the session (SIG-007).
        let mut robot = connect(&addr, &format!("/ws/robot?session={session}"), &[("authorization", &bearer)]).await.unwrap();
        assert_eq!(recv(&mut robot).await["type"], "welcome");
        let resumed = recv(&mut robot).await;
        assert_eq!((resumed["type"].as_str(), &resumed["session"]), (Some("session"), &session));
        assert!(resumed["ice_servers"][1]["username"].as_str().unwrap().ends_with(":robot1"));
        let notice = recv(&mut op).await;
        assert_eq!((notice["type"].as_str(), &notice["session"]), (Some("resumed"), &session));
        assert_eq!(recv(&mut op).await["robot"]["operator"], 1);
        send(&mut op, json!({ "type": "offer", "session": session, "sdp": sdp })).await;
        assert_eq!(recv(&mut robot).await["type"], "offer");

        // Garbage closes the connection; the session ends for the operator and the robot goes offline.
        robot.send(tungstenite::Message::text("{nope")).await.unwrap();
        let ended = recv(&mut op).await;
        assert_eq!((ended["type"].as_str(), ended["reason"].as_str()), (Some("end"), Some("robot_offline")));
        assert_eq!(recv(&mut op).await["robot"]["online"], false);
    }

    #[tokio::test]
    async fn app_release() {
        let dir = release::temp_dir("server-release");
        release::write_test_release(&dir, 2, b"apk v2");
        let apk_dir = dir.clone();
        let (addr, robot_token, app) = start_app(move |cfg| cfg.apk_dir = apk_dir).await;
        let bearer = format!("Bearer {robot_token}");

        // Announced right after welcome.
        let mut robot = connect(&addr, "/ws/robot", &[("authorization", &bearer)]).await.unwrap();
        assert_eq!(recv(&mut robot).await["type"], "welcome");
        let update = recv(&mut robot).await;
        assert_eq!((update["type"].as_str(), update["version_code"].as_u64()), (Some("update"), Some(2)));
        assert_eq!(update["apks"]["arm64-v8a"]["size"], 6);
        assert!(update["apks"]["arm64-v8a"].get("file").is_none());

        // Download: robots only, known ABIs only; without nginx the backend sends the file.
        let get = |path: &str, auth: Option<&str>| {
            let mut req = reqwest::Client::new().get(format!("http://{addr}{path}"));
            if let Some(auth) = auth {
                req = req.header("authorization", auth);
            }
            req.send()
        };
        assert_eq!(get("/api/v1/robot/apk/arm64-v8a", None).await.unwrap().status(), 401);
        assert_eq!(get("/api/v1/robot/apk/x86", Some(&bearer)).await.unwrap().status(), 404);
        let apk = get("/api/v1/robot/apk/arm64-v8a", Some(&bearer)).await.unwrap();
        assert_eq!(apk.status(), 200);
        assert_eq!(apk.bytes().await.unwrap().as_ref(), b"apk v2");

        // A reload announces a new release to connected robots; an unchanged or broken one is not announced.
        app.reload_release();
        release::write_test_release(&dir, 3, b"apk v3");
        app.reload_release();
        let update = recv(&mut robot).await;
        assert_eq!((update["type"].as_str(), update["version_code"].as_u64()), (Some("update"), Some(3)));
        std::fs::write(dir.join("a.apk"), b"truncated").unwrap();
        app.reload_release();
        send(&mut robot, json!({ "type": "ping" })).await;
        assert_eq!(recv(&mut robot).await["type"], "pong");
        let apk = get("/api/v1/robot/apk/arm64-v8a", Some(&bearer)).await.unwrap();
        assert_eq!(apk.status(), 200, "the previous release stays");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn app_download_behind_nginx() {
        let dir = release::temp_dir("server-accel");
        release::write_test_release(&dir, 2, b"apk");
        let apk_dir = dir.clone();
        let (addr, robot_token, _) = start_app(move |cfg| {
            cfg.apk_dir = apk_dir;
            cfg.apk_accel = "/_apk/".into();
        })
        .await;
        let apk = reqwest::Client::new()
            .get(format!("http://{addr}/api/v1/robot/apk/arm64-v8a"))
            .header("authorization", format!("Bearer {robot_token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(apk.headers()["x-accel-redirect"], "/_apk/a.apk");
        assert!(apk.bytes().await.unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn health() {
        let (addr, _) = start().await;
        let v: serde_json::Value = reqwest::get(format!("http://{addr}/api/v1/health")).await.unwrap().json().await.unwrap();
        assert_eq!(v["status"], "ok");
    }
}
