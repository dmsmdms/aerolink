//! Admin REST API (docs/08-backend.md "Fleet administration", ADM-012..014). Every change is stored, written
//! to `audit` and handed to the hub, so every console sees it at once. Each handler checks the role and the
//! admin's fleets (ADM-005): robots outside them answer 404 as if they did not exist. Managing users and
//! creating fleets needs access to every fleet (an admin of some fleets could otherwise widen their own rights).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth;
use crate::db::{Role, User};
use crate::hub::{Access, Fleet, FleetId, RobotId, RobotRecord, UserId};
use crate::server::{ApiError, App, client_ip, lock, session_user, user_json};

/// Events and audit entries per page.
const PAGE: i64 = 50;
const MAX_NAME: usize = 64;

fn admin(app: &App, headers: &HeaderMap) -> Result<(User, Access), ApiError> {
    let user = session_user(app, headers)?;
    let access = user.admin_access().ok_or(ApiError::Forbidden)?;
    Ok((user, access))
}

fn admin_of_all(app: &App, headers: &HeaderMap) -> Result<User, ApiError> {
    match admin(app, headers)? {
        (user, Access::All) => Ok(user),
        _ => Err(ApiError::Forbidden),
    }
}

fn robot_in(app: &App, access: &Access, id: RobotId) -> Result<RobotRecord, ApiError> {
    app.db().robot(id)?.filter(|r| access.covers(r.fleet)).ok_or(ApiError::NotFound)
}

fn check_fleet(app: &App, access: &Access, fleet: FleetId) -> Result<(), ApiError> {
    let known = app.db().fleets()?.iter().any(|f| f.id == fleet);
    if known && access.covers(fleet) { Ok(()) } else { Err(ApiError::BadRequest) }
}

/// Names of robots, users and fleets: trimmed, 1..=64 characters, no control characters.
fn clean_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    let n = name.chars().count();
    if n == 0 || n > MAX_NAME || name.chars().any(char::is_control) {
        return Err(ApiError::BadRequest);
    }
    Ok(name.to_owned())
}

fn check_password(password: &str) -> Result<(), ApiError> {
    let n = password.chars().count();
    if n < auth::MIN_PASSWORD || password.len() > 1024 { Err(ApiError::BadRequest) } else { Ok(()) }
}

/// Hands a stored robot to the hub; a disabled robot or one with a new token is disconnected.
fn robot_changed(app: &App, id: RobotId, disconnect: bool) -> Result<(), ApiError> {
    let record = app.db().robot(id)?.ok_or(ApiError::NotFound)?;
    let mut sig = lock(&app.signaling);
    let kicked = if disconnect { sig.hub.robot_conn(id) } else { None };
    let (out, dropped) = sig.hub.robot_changed(record);
    sig.drop_conns(dropped.into_iter().chain(kicked));
    app.dispatch(&mut sig, out);
    Ok(())
}

#[derive(Deserialize)]
pub struct Page {
    before: Option<i64>,
}

/// `GET /api/v1/admin/robots/<id>/events?before=<event id>`: newest first.
pub async fn robot_events(
    State(app): State<Arc<App>>,
    Path(id): Path<RobotId>,
    Query(page): Query<Page>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let (_, access) = admin(&app, &headers)?;
    robot_in(&app, &access, id)?;
    let events = app.db().events(id, page.before, PAGE)?;
    Ok(Json(json!({ "events": events })))
}

/// `GET /api/v1/admin/audit?before=<entry id>`: newest first, entries about the admin's fleets (all of them for
/// admins of every fleet, including user management).
pub async fn audit(State(app): State<Arc<App>>, Query(page): Query<Page>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let (_, access) = admin(&app, &headers)?;
    let fleets: Option<Vec<FleetId>> = match access {
        Access::All => None,
        Access::Fleets(fleets) => Some(fleets.into_iter().collect()),
    };
    let entries = app.db().audit_log(fleets.as_deref(), page.before, PAGE)?;
    Ok(Json(json!({ "entries": entries })))
}

#[derive(Deserialize)]
pub struct NewRobot {
    name: String,
    fleet: FleetId,
}

/// `POST /api/v1/admin/robots {name, fleet}` → `{id, token}`; the token is shown this once.
pub async fn add_robot(State(app): State<Arc<App>>, headers: HeaderMap, Json(req): Json<NewRobot>) -> Result<Json<Value>, ApiError> {
    let (user, access) = admin(&app, &headers)?;
    let name = clean_name(&req.name)?;
    check_fleet(&app, &access, req.fleet)?;
    let secret = auth::new_token();
    let id = app.db().add_robot(&name, &auth::token_hash(&secret), req.fleet)?;
    app.db().audit(user.id, Some(req.fleet), "robot_add", &format!("robot {id} ({name})"), "")?;
    tracing::info!(ip = client_ip(&headers), admin = user.id, robot = id, "robot added");
    robot_changed(&app, id, false)?;
    Ok(Json(json!({ "id": id, "token": auth::robot_token(id, &secret) })))
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RobotAction {
    Rename { name: String },
    Fleet { fleet: FleetId },
    Disable,
    Enable,
    /// A new token; the old one stops working and the robot is disconnected.
    Token,
    /// The admin has seen the errors.
    ReadErrors,
}

/// `POST /api/v1/admin/robots/<id> {action, …}` → `{}` or `{token}`.
pub async fn robot_action(
    State(app): State<Arc<App>>,
    Path(id): Path<RobotId>,
    headers: HeaderMap,
    Json(action): Json<RobotAction>,
) -> Result<Json<Value>, ApiError> {
    let (user, access) = admin(&app, &headers)?;
    let robot = robot_in(&app, &access, id)?;
    let (what, detail, reply, disconnect) = match action {
        RobotAction::ReadErrors => {
            app.db().ack_errors(id)?;
            let mut sig = lock(&app.signaling);
            let out = sig.hub.errors_read(id);
            app.dispatch(&mut sig, out);
            return Ok(Json(json!({})));
        }
        RobotAction::Rename { name } => {
            let name = clean_name(&name)?;
            app.db().rename_robot(id, &name)?;
            ("robot_rename", name, json!({}), false)
        }
        RobotAction::Fleet { fleet } => {
            check_fleet(&app, &access, fleet)?;
            app.db().set_robot_fleet(id, fleet)?;
            ("robot_fleet", fleet.to_string(), json!({}), false)
        }
        RobotAction::Disable => {
            app.db().set_robot_disabled(id, true)?;
            ("robot_disable", String::new(), json!({}), false)
        }
        RobotAction::Enable => {
            app.db().set_robot_disabled(id, false)?;
            ("robot_enable", String::new(), json!({}), false)
        }
        RobotAction::Token => {
            let secret = auth::new_token();
            app.db().set_robot_token(id, &auth::token_hash(&secret))?;
            ("robot_token", String::new(), json!({ "token": auth::robot_token(id, &secret) }), true)
        }
    };
    app.db().audit(user.id, Some(robot.fleet), what, &format!("robot {id} ({})", robot.name), &detail)?;
    tracing::info!(ip = client_ip(&headers), admin = user.id, robot = id, "{what}");
    robot_changed(&app, id, disconnect)?;
    Ok(Json(reply))
}

/// `GET /api/v1/admin/users`: everyone with what they are doing right now (ADM-013).
pub async fn users(State(app): State<Arc<App>>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    admin(&app, &headers)?;
    let users = app.db().users()?;
    let online = lock(&app.signaling).hub.online_users();
    let list: Vec<Value> = users
        .iter()
        .map(|(u, disabled)| {
            let mut v = user_json(u);
            v["disabled"] = json!(disabled);
            v["online"] = json!(online.contains_key(&u.id));
            v["robots"] = json!(online.get(&u.id).cloned().unwrap_or_default());
            v
        })
        .collect();
    Ok(Json(json!({ "users": list })))
}

#[derive(Deserialize)]
pub struct NewUser {
    email: String,
    name: String,
    password: String,
    role: Role,
}

/// `POST /api/v1/admin/users {email, name, password, role}` (admins of every fleet).
pub async fn add_user(State(app): State<Arc<App>>, headers: HeaderMap, Json(req): Json<NewUser>) -> Result<Json<Value>, ApiError> {
    let admin = admin_of_all(&app, &headers)?;
    let email = req.email.trim();
    if email.len() > 254 || !email.contains('@') || email.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ApiError::BadRequest);
    }
    let name = clean_name(&req.name)?;
    check_password(&req.password)?;
    let id = app.db().add_user(email, &name, &auth::hash_password(&req.password), req.role)?;
    app.db().audit(admin.id, None, "user_add", &format!("user {id} ({email})"), req.role.as_str())?;
    tracing::info!(ip = client_ip(&headers), admin = admin.id, user = id, "user added");
    let (user, _) = app.db().user(id)?.ok_or(ApiError::Internal)?;
    Ok(Json(user_json(&user)))
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum UserAction {
    Disable,
    Enable,
    Password { password: String },
    Role { role: Role },
    /// `all`: every fleet, including later ones; otherwise exactly `fleets`.
    Fleets { all: bool, fleets: Vec<FleetId> },
}

/// `POST /api/v1/admin/users/<id> {action, …}` (admins of every fleet). Changed rights apply at once: the user's
/// consoles are disconnected and reconnect with the new ones (a disabled user or a new password logs out).
pub async fn user_action(
    State(app): State<Arc<App>>,
    Path(id): Path<UserId>,
    headers: HeaderMap,
    Json(action): Json<UserAction>,
) -> Result<Json<Value>, ApiError> {
    let admin = admin_of_all(&app, &headers)?;
    let (target, _) = app.db().user(id)?.ok_or(ApiError::NotFound)?;
    // An admin cannot lock themselves out; another admin of every fleet can.
    if target.id == admin.id && !matches!(action, UserAction::Password { .. }) {
        return Err(ApiError::Forbidden);
    }
    let email = target.email.as_str();
    let (what, detail) = match action {
        UserAction::Disable => {
            app.db().set_user_disabled(email, true)?;
            ("user_disable", String::new())
        }
        UserAction::Enable => {
            app.db().set_user_disabled(email, false)?;
            ("user_enable", String::new())
        }
        UserAction::Password { password } => {
            check_password(&password)?;
            app.db().set_password(email, &auth::hash_password(&password))?;
            ("user_password", String::new())
        }
        UserAction::Role { role } => {
            app.db().set_role(email, role)?;
            ("user_role", role.as_str().to_owned())
        }
        UserAction::Fleets { all, fleets } => {
            let known = app.db().fleets()?;
            if !fleets.iter().all(|f| known.iter().any(|k| k.id == *f)) {
                return Err(ApiError::BadRequest);
            }
            app.db().set_user_fleets(email, all, &fleets)?;
            let detail = if all { "all".to_owned() } else { fleets.iter().map(|f| f.to_string()).collect::<Vec<_>>().join(",") };
            ("user_fleets", detail)
        }
    };
    app.db().audit(admin.id, None, what, &format!("user {id} ({email})"), &detail)?;
    tracing::info!(ip = client_ip(&headers), admin = admin.id, user = id, "{what}");
    if what != "user_enable" {
        let mut sig = lock(&app.signaling);
        let conns = sig.hub.user_conns(id);
        sig.drop_conns(conns);
    }
    let (user, disabled) = app.db().user(id)?.ok_or(ApiError::NotFound)?;
    let mut v = user_json(&user);
    v["disabled"] = json!(disabled);
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct NewFleet {
    name: String,
}

/// `POST /api/v1/admin/fleets {name}` (admins of every fleet) → `{id, name}`.
pub async fn add_fleet(State(app): State<Arc<App>>, headers: HeaderMap, Json(req): Json<NewFleet>) -> Result<Json<Value>, ApiError> {
    let admin = admin_of_all(&app, &headers)?;
    let name = clean_name(&req.name)?;
    let id = app.db().add_fleet(&name)?;
    app.db().audit(admin.id, Some(id), "fleet_add", &format!("fleet {id} ({name})"), "")?;
    let mut sig = lock(&app.signaling);
    let out = sig.hub.fleet_added(Fleet { id, name: name.clone() });
    app.dispatch(&mut sig, out);
    Ok(Json(json!({ "id": id, "name": name })))
}
