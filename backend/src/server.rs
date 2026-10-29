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

use crate::admin;
use crate::auth;
use crate::config::Config;
use crate::db::{self, Db, User};
use crate::hub::{ConnId, Hub, IceServer, OperatorMsg, Out, Peer, RobotId, RobotMsg, RobotRecord, ServerMsg, SessionId, Target, UserId};
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

pub(crate) struct Signaling {
    pub(crate) hub: Hub,
    conns: HashMap<ConnId, Conn>,
}

impl Signaling {
    /// Closes connections (their tasks see the queue end and say goodbye).
    pub(crate) fn drop_conns(&mut self, ids: impl IntoIterator<Item = ConnId>) {
        for id in ids {
            self.conns.remove(&id);
        }
    }
}

/// Robot events with this target go to Telegram only: they are stored with the robot already (ADM-003), and
/// the server's error log stays about the server.
pub const ROBOT_EVENT_TARGET: &str = "aerolink::robot_event";

pub struct App {
    pub(crate) cfg: Config,
    db: Mutex<Db>,
    pub(crate) signaling: Mutex<Signaling>,
    next_conn: AtomicU64,
    /// Robot app release announced to robots (ROB-008).
    release: Mutex<Option<Release>>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not take the whole service down with poisoned locks.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl App {
    pub fn new(cfg: Config, db: Db) -> rusqlite::Result<Arc<Self>> {
        let hub = Hub::new(db.fleets()?, db.robots()?);
        let release = load_release(&cfg);
        Ok(Arc::new(App {
            cfg,
            signaling: Mutex::new(Signaling { hub, conns: HashMap::new() }),
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

    pub(crate) fn db(&self) -> MutexGuard<'_, Db> {
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
    pub(crate) fn dispatch(&self, sig: &mut Signaling, out: Out) {
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
                Target::Admins(fleet) => {
                    let text = encode(&msg);
                    let ids: Vec<ConnId> = sig.hub.admins_of(fleet).collect();
                    for id in ids {
                        send(sig, id, text.clone());
                    }
                }
            }
        }
    }
}

impl App {
    /// A robot message: what has to survive a restart is stored first (ADM-004), then the hub announces it.
    fn robot_message(&self, conn: ConnId, robot: RobotId, msg: RobotMsg) {
        let now = db::now();
        let stored = match &msg {
            RobotMsg::Status(status) => self.db().save_status(robot, status, now).map(|_| None),
            RobotMsg::Location(location) if location.valid() => self.db().save_location(robot, location, now).map(|_| None),
            RobotMsg::Event(event) => self.db().add_event(robot, &event.clone().sanitized(), now).map(Some),
            _ => Ok(None),
        };
        let event = match stored {
            Ok(event) => event,
            Err(e) => {
                tracing::error!("database: {e}");
                None
            }
        };
        let mut sig = lock(&self.signaling);
        // A replaced connection may still deliver a message or two; they are not this robot's any more.
        if sig.hub.robot_of(conn) != Some(robot) {
            return;
        }
        let mut out = sig.hub.from_robot(conn, msg);
        if let Some(event) = event {
            if event.level == crate::hub::EventLevel::Error {
                tracing::error!(target: ROBOT_EVENT_TARGET, robot, code = %event.code, "{}", event.message);
            }
            out.extend(sig.hub.robot_event(robot, event));
        }
        self.dispatch(&mut sig, out);
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
        .route("/api/v1/admin/robots", post(admin::add_robot))
        .route("/api/v1/admin/robots/{id}", post(admin::robot_action))
        .route("/api/v1/admin/robots/{id}/events", get(admin::robot_events))
        .route("/api/v1/admin/users", get(admin::users).post(admin::add_user))
        .route("/api/v1/admin/users/{id}", post(admin::user_action))
        .route("/api/v1/admin/fleets", post(admin::add_fleet))
        .route("/api/v1/admin/audit", get(admin::audit))
        .route("/ws/operator", get(operator_ws))
        .route("/ws/robot", get(robot_ws))
        .fallback(|| async { ApiError::NotFound })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(app)
}

// --- errors ---

pub(crate) enum ApiError {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    /// A name or e-mail that is taken.
    Conflict,
    Internal,
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(f, _) = &e {
            if f.code == rusqlite::ErrorCode::ConstraintViolation {
                return ApiError::Conflict;
            }
        }
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
            ApiError::Conflict => (StatusCode::CONFLICT, "conflict"),
            ApiError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        (status, Json(json!({ "error": code }))).into_response()
    }
}

/// Client address as seen by nginx (the socket peer is nginx itself).
pub(crate) fn client_ip(headers: &HeaderMap) -> &str {
    headers.get("x-real-ip").and_then(|v| v.to_str().ok()).unwrap_or("-")
}

pub(crate) fn user_json(u: &User) -> serde_json::Value {
    json!({ "id": u.id, "email": u.email, "name": u.name, "role": u.role, "all_fleets": u.all_fleets, "fleets": u.fleets })
}

pub(crate) fn session_user(app: &App, headers: &HeaderMap) -> Result<User, ApiError> {
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
        let admin = user.admin_access();
        let peer = Peer { id: user.id, name: user.name };
        run_conn(app, socket, Who::Operator(peer.id), OPERATOR_IDLE, |hub, conn| {
            (hub.operator_connected(conn, peer, admin), None)
        })
        .await
    }))
}

/// `/ws/robot?session=<id>`: the robot reconnects while still in that session (SIG-007).
#[derive(Deserialize)]
struct RobotQuery {
    session: Option<SessionId>,
}

/// The robot of the `Authorization: Bearer <id>.<secret>` header.
fn robot_auth(app: &App, headers: &HeaderMap, what: &str) -> Result<RobotRecord, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(auth::parse_robot_token);
    let robot = match token {
        Some((id, secret)) => app
            .db()
            .robot_for_login(id)?
            .and_then(|(record, hash)| auth::constant_time_eq(&auth::token_hash(secret), &hash).then_some(record)),
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
    let record = robot_auth(&app, &headers, "robot ws")?;
    let id = record.id;
    let resume = query.session;
    app.db().touch_robot(id, db::now())?;
    tracing::info!(ip = client_ip(&headers), robot = id, resume, "robot connected");
    let update = lock(&app.release).clone();
    Ok(ws.max_message_size(MAX_WS_MESSAGE).max_frame_size(MAX_WS_MESSAGE).on_upgrade(move |socket| {
        run_conn(app, socket, Who::Robot(id), ROBOT_IDLE, move |hub, conn| {
            let (mut out, replaced) = hub.robot_connected(conn, record, resume);
            out.extend(update.map(|r| (Target::Conn(conn), ServerMsg::Update(r))));
            (out, replaced)
        })
    }))
}

/// `GET /api/v1/robot/apk/<abi>`: the APK of the current release (ROB-008), robots only. Behind nginx the file
/// is sent by nginx (`X-Accel-Redirect` to an `internal` location), so this thread never streams megabytes.
async fn robot_apk(State(app): State<Arc<App>>, Path(abi): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let robot = robot_auth(&app, &headers, "robot apk")?.id;
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
                let handled = match who {
                    Who::Robot(robot) => serde_json::from_str::<RobotMsg>(&text).map(|m| app.robot_message(id, robot, m)),
                    Who::Operator(_) => serde_json::from_str::<OperatorMsg>(&text).map(|m| {
                        let mut sig = lock(&app.signaling);
                        let out = sig.hub.from_operator(id, m);
                        app.dispatch(&mut sig, out);
                    }),
                };
                if handled.is_err() {
                    break "malformed message";
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
    let now = db::now();
    let mut sig = lock(&app.signaling);
    sig.conns.remove(&id);
    let current = sig.hub.robot_of(id).is_some();
    let out = sig.hub.disconnected(id, now);
    app.dispatch(&mut sig, out);
    drop(sig);
    match who {
        Who::Robot(robot) => {
            // A replaced connection is not the robot going offline.
            if current {
                if let Err(e) = app.db().touch_robot(robot, now) {
                    tracing::error!("database: {e}");
                }
            }
            tracing::info!(robot, reason, "robot disconnected")
        }
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
        let hash = auth::hash_password("password123");
        db.add_user("op@x.org", "Op", &hash, db::Role::Operator).unwrap();
        db.add_user("admin@x.org", "Admin", &hash, db::Role::Admin).unwrap();
        db.set_user_fleets("admin@x.org", true, &[]).unwrap();
        let north = db.add_fleet("north").unwrap();
        db.add_user("north@x.org", "North", &hash, db::Role::Admin).unwrap();
        db.set_user_fleets("north@x.org", false, &[north]).unwrap();
        let secret = auth::new_token();
        let robot = db.add_robot("r1", &auth::token_hash(&secret), 1).unwrap();
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
        login_as(addr, "OP@x.org", password).await
    }

    async fn login_as(addr: &str, email: &str, password: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("http://{addr}/api/v1/login"))
            .json(&json!({ "email": email, "password": password }))
            .send()
            .await
            .unwrap()
    }

    async fn cookie_of(addr: &str, email: &str) -> String {
        let r = login_as(addr, email, "password123").await;
        r.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_owned()
    }

    /// `(status, body)` of an admin API call with the given cookie.
    async fn api(addr: &str, cookie: &str, method: &str, path: &str, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
        let client = reqwest::Client::new();
        let url = format!("http://{addr}{path}");
        let mut req = if method == "GET" { client.get(url) } else { client.post(url) };
        req = req.header("cookie", cookie);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let r = req.send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(serde_json::Value::Null))
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

    #[tokio::test]
    async fn fleet_administration() {
        let (addr, robot_token, _) = start_app(|_| {}).await;
        let op = cookie_of(&addr, "op@x.org").await;
        let admin = cookie_of(&addr, "admin@x.org").await;
        let north = cookie_of(&addr, "north@x.org").await;
        let me = api(&addr, &admin, "GET", "/api/v1/me", None).await.1;
        assert_eq!((me["role"].as_str(), me["all_fleets"].as_bool()), (Some("admin"), Some(true)));

        // The admin gets the fleet view after the usual robot list; an operator does not.
        let mut op_ws = connect(&addr, "/ws/operator", &[("origin", ORIGIN), ("cookie", &op)]).await.unwrap();
        assert_eq!(recv(&mut op_ws).await["type"], "welcome");
        assert_eq!(recv(&mut op_ws).await["type"], "robots");
        let mut admin_ws = connect(&addr, "/ws/operator", &[("origin", ORIGIN), ("cookie", &admin)]).await.unwrap();
        assert_eq!(recv(&mut admin_ws).await["type"], "welcome");
        assert_eq!(recv(&mut admin_ws).await["type"], "robots");
        let fleet = recv(&mut admin_ws).await;
        assert_eq!((fleet["type"].as_str(), fleet["fleets"].as_array().unwrap().len()), (Some("fleet"), 2));
        assert_eq!(fleet["robots"][0]["name"], "r1");

        // The robot's state, location and events reach the admin; the state survives in the database.
        let bearer = format!("Bearer {robot_token}");
        let mut robot = connect(&addr, "/ws/robot", &[("authorization", &bearer)]).await.unwrap();
        assert_eq!(recv(&mut robot).await["type"], "welcome");
        assert_eq!(recv(&mut op_ws).await["robot"]["online"], true);
        assert_eq!(recv_of(&mut admin_ws, "fleet_robot").await["robot"]["online"], true);
        send(&mut robot, json!({ "type": "location", "lat": 50.45, "lon": 30.52, "accuracy_m": 9, "at": 100 })).await;
        assert_eq!(recv_of(&mut admin_ws, "fleet_robot").await["robot"]["location"]["lat"], 50.45);
        send(&mut robot, json!({ "type": "event", "level": "error", "code": "base_fault", "message": "motor", "at": 101 })).await;
        assert_eq!(recv_of(&mut admin_ws, "fleet_robot").await["robot"]["unread_errors"], 1);
        assert_eq!(recv_of(&mut admin_ws, "robot_event").await["event"]["code"], "base_fault");
        let (status, events) = api(&addr, &admin, "GET", "/api/v1/admin/robots/1/events", None).await;
        assert_eq!((status, events["events"][0]["message"].as_str()), (200, Some("motor")));

        // Operators cannot use admin messages or the admin API; an admin of another fleet sees nothing of r1.
        send(&mut op_ws, json!({ "type": "take", "robot": 1 })).await;
        let session = recv(&mut robot).await["session"].clone();
        recv_of(&mut op_ws, "session").await;
        send(&mut op_ws, json!({ "type": "admin_end", "session": session })).await;
        assert_eq!(recv(&mut op_ws).await["code"], "forbidden");
        assert_eq!(api(&addr, &op, "GET", "/api/v1/admin/users", None).await.0, 403);
        assert_eq!(api(&addr, &op, "POST", "/api/v1/admin/robots/1", Some(json!({ "action": "disable" }))).await.0, 403);
        assert_eq!(api(&addr, &north, "GET", "/api/v1/admin/robots/1/events", None).await.0, 404);
        assert_eq!(api(&addr, &north, "POST", "/api/v1/admin/robots/1", Some(json!({ "action": "disable" }))).await.0, 404);
        let user = json!({ "email": "x@x.org", "name": "X", "password": "long enough!", "role": "admin" });
        assert_eq!(api(&addr, &north, "POST", "/api/v1/admin/users", Some(user)).await.0, 403);

        // Take-over: the operator and the robot hear `end admin`, the robot then gets the admin's session.
        while recv_of(&mut admin_ws, "fleet_robot").await["robot"]["operator"]["name"] != "Op" {}
        send(&mut admin_ws, json!({ "type": "admin_take", "robot": 1 })).await;
        let ended = recv(&mut robot).await;
        assert_eq!((ended["type"].as_str(), ended["reason"].as_str()), (Some("end"), Some("admin")));
        let taken = recv(&mut robot).await;
        assert_eq!((taken["type"].as_str(), taken["peer"]["name"].as_str()), (Some("session"), Some("Admin")));
        assert_eq!(recv_of(&mut op_ws, "end").await["reason"], "admin");

        // The operators page knows who holds what.
        let users = api(&addr, &admin, "GET", "/api/v1/admin/users", None).await.1;
        let admin_row = users["users"].as_array().unwrap().iter().find(|u| u["name"] == "Admin").unwrap().clone();
        assert_eq!((admin_row["online"].as_bool(), &admin_row["robots"]), (Some(true), &json!([1])));

        // Disabling drops the robot at once; a new robot comes with a token shown once.
        let (status, _) = api(&addr, &admin, "POST", "/api/v1/admin/robots/1", Some(json!({ "action": "disable" }))).await;
        assert_eq!(status, 200);
        assert!(tokio::time::timeout(Duration::from_secs(2), async { while robot.next().await.is_some_and(|m| m.is_ok_and(|m| !m.is_close())) {} }).await.is_ok());
        assert_eq!(status_of(connect(&addr, "/ws/robot", &[("authorization", &bearer)]).await), 401);
        let (status, added) = api(&addr, &admin, "POST", "/api/v1/admin/robots", Some(json!({ "name": "r2", "fleet": 2 }))).await;
        assert_eq!(status, 200);
        assert!(added["token"].as_str().unwrap().starts_with(&format!("{}.", added["id"])));
        let (status, _) = api(&addr, &admin, "POST", "/api/v1/admin/robots", Some(json!({ "name": "r2", "fleet": 1 }))).await;
        assert_eq!(status, 409);
        let (status, events) = api(&addr, &north, "GET", &format!("/api/v1/admin/robots/{}/events", added["id"]), None).await;
        assert_eq!((status, events["events"].as_array().map(Vec::len)), (200, Some(0)));

        // Users: an admin cannot demote themselves; the audit shows what happened, per fleet.
        let (status, _) = api(&addr, &admin, "POST", "/api/v1/admin/users/2", Some(json!({ "action": "role", "role": "operator" }))).await;
        assert_eq!(status, 403);
        let (status, user) = api(&addr, &admin, "POST", "/api/v1/admin/users", Some(json!({
            "email": "new@x.org", "name": "New", "password": "long enough!", "role": "operator"
        }))).await;
        assert_eq!((status, user["role"].as_str()), (200, Some("operator")));
        let short = json!({ "action": "password", "password": "short" });
        assert_eq!(api(&addr, &admin, "POST", &format!("/api/v1/admin/users/{}", user["id"]), Some(short)).await.0, 400);
        let audit = api(&addr, &admin, "GET", "/api/v1/admin/audit", None).await.1;
        let actions: Vec<&str> = audit["entries"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
        assert_eq!(actions, ["user_add", "robot_add", "robot_disable"]);
        let audit = api(&addr, &north, "GET", "/api/v1/admin/audit", None).await.1;
        assert_eq!(audit["entries"].as_array().unwrap().len(), 1);
    }

    /// The next message of this type; others are skipped.
    async fn recv_of(ws: &mut Ws, ty: &str) -> serde_json::Value {
        loop {
            let m = recv(ws).await;
            if m["type"] == ty {
                return m;
            }
        }
    }

    fn status_of(r: Result<Ws, tungstenite::Error>) -> u16 {
        match r {
            Ok(_) => panic!("not refused"),
            Err(e) => status(e),
        }
    }
}
