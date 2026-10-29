//! Signaling hub (docs/08-backend.md "Signaling"): who is online, which operator controls which robot
//! (ARCH-001), and relaying opaque WebRTC offers/answers/candidates between the two ends of a session.
//! Media and DRIVE never pass through here: they go peer-to-peer (or via TURN) over WebRTC.
//!
//! Admins (docs/08-backend.md "Fleet administration") get the robots of their fleets with the last known
//! state, location and events, and can end or take over sessions (ADM-005, ADM-012, ADM-014).
//!
//! Pure state machine: every input returns the messages to deliver, the WebSocket glue in `server.rs`
//! only serializes and sends them (and stores what has to survive a restart). No I/O, no clocks (the
//! caller passes `now`), so it is unit-tested directly.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type ConnId = u64;
pub type RobotId = i64;
pub type UserId = i64;
pub type SessionId = u64;
pub type FleetId = i64;

/// What an admin may see and act on (ADM-005).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// Every fleet, including ones created later.
    All,
    Fleets(BTreeSet<FleetId>),
}

impl Access {
    pub fn covers(&self, fleet: FleetId) -> bool {
        match self {
            Access::All => true,
            Access::Fleets(fleets) => fleets.contains(&fleet),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Fleet {
    pub id: FleetId,
    pub name: String,
}

/// Robot modes reported by the phone (ARCH-002).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RobotMode {
    Idle,
    Teleop,
    Auto,
    NeedsHelp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkKind {
    Wifi,
    Cellular,
    Other,
}

/// ADM-001: the phone's network, coarse enough not to change all the time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Network {
    #[serde(rename = "type")]
    pub kind: NetworkKind,
    /// Mobile operator name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    /// Signal level 0..4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<u8>,
}

/// Robot state for the operator list and the admin table. Sent by the robot only on change (PWR-012: no
/// chatter), so every field is coarse: battery in 5 % steps outside a session, no uptime counter (the start
/// time instead).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RobotStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<RobotMode>,
    /// Phone battery, %.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_battery: Option<u8>,
    /// Robot (hoverboard) battery, mV.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robot_battery_mv: Option<u32>,
    /// PWR-023: false without root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_control: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    /// Phone model, e.g. `SM-J710F`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    /// Android version, e.g. `6.0.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<Network>,
    /// ROB-008 state of the app update: `current`, `waiting`, `downloading`, `installing`, `blocked`, `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_update: Option<String>,
    /// ROB-009 state of the firmware update: `current`, `waiting`, `sending`, `installing`, `failed`, `no_image`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_update: Option<String>,
    /// PWR-014.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_owner: Option<bool>,
    /// When the robot service started, Unix seconds (the uptime without a ticking counter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<i64>,
}

/// ADM-002: last known position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub lat: f64,
    pub lon: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<f32>,
    /// Time of the fix, Unix seconds.
    pub at: i64,
}

impl Location {
    pub fn valid(&self) -> bool {
        self.lat.is_finite() && self.lon.is_finite() && self.lat.abs() <= 90.0 && self.lon.abs() <= 180.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLevel {
    Info,
    Warning,
    Error,
}

impl EventLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            EventLevel::Info => "info",
            EventLevel::Warning => "warning",
            EventLevel::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "error" => EventLevel::Error,
            "warning" => EventLevel::Warning,
            _ => EventLevel::Info,
        }
    }
}

/// ADM-003: an event as the robot sends it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct EventIn {
    pub level: EventLevel,
    pub code: String,
    #[serde(default)]
    pub message: String,
    /// When it happened on the robot, Unix seconds.
    pub at: i64,
}

pub const MAX_EVENT_CODE: usize = 64;
pub const MAX_EVENT_MESSAGE: usize = 2000;

impl EventIn {
    /// Robot text is untrusted: bounded before it reaches the database, the console or Telegram.
    pub fn sanitized(mut self) -> Self {
        fn clip(s: &mut String, max: usize) {
            if let Some((i, _)) = s.char_indices().nth(max) {
                s.truncate(i);
            }
        }
        clip(&mut self.code, MAX_EVENT_CODE);
        clip(&mut self.message, MAX_EVENT_MESSAGE);
        self
    }
}

/// A stored event.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RobotEvent {
    pub id: i64,
    pub level: EventLevel,
    pub code: String,
    pub message: String,
    pub at: i64,
    /// When the server got it (the robot may have been offline).
    pub received: i64,
}

/// A robot as stored, loaded at start and after admin changes.
#[derive(Clone, Debug, PartialEq)]
pub struct RobotRecord {
    pub id: RobotId,
    pub name: String,
    pub fleet: FleetId,
    pub disabled: bool,
    pub status: RobotStatus,
    pub location: Option<Location>,
    pub last_seen: Option<i64>,
    pub unread_errors: u32,
}

/// Messages from a robot.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RobotMsg {
    Status(RobotStatus),
    Location(Location),
    /// Stored by the glue first (it needs the id), then announced with [`Hub::robot_event`].
    Event(EventIn),
    Answer { session: SessionId, sdp: Value },
    Ice { session: SessionId, candidate: Value },
    /// The robot ends the session (e.g. the operator link died, SAFE-010).
    End { session: SessionId },
    Ping,
}

/// Messages from an operator console.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OperatorMsg {
    Take { robot: RobotId },
    Offer { session: SessionId, sdp: Value },
    Ice { session: SessionId, candidate: Value },
    Release { session: SessionId },
    Ping,
    /// Admins (ADM-012): end someone's session; the robot stops (SAFE-010).
    AdminEnd { session: SessionId },
    /// Admins (ADM-012): `take`, but a robot held by another operator is taken over.
    AdminTake { robot: RobotId },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RobotView {
    pub id: RobotId,
    pub name: String,
    pub online: bool,
    pub status: RobotStatus,
    /// Operator controlling the robot, if any.
    pub operator: Option<UserId>,
}

/// What admins see of a robot (ADM-010).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RobotAdminView {
    pub id: RobotId,
    pub name: String,
    pub fleet: FleetId,
    pub disabled: bool,
    pub online: bool,
    /// Last known, also while offline.
    pub status: RobotStatus,
    pub location: Option<Location>,
    /// When it was last connected (`None` while online or never seen).
    pub last_seen: Option<i64>,
    pub unread_errors: u32,
    pub session: Option<SessionId>,
    /// Who holds the robot.
    pub operator: Option<Peer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    Offline,
    Busy,
    NoSession,
    Forbidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Released,
    RobotEnded,
    RobotOffline,
    OperatorOffline,
    /// An admin ended the session or took the robot over (ADM-012).
    Admin,
}

/// Peer of a session as told to the other side.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Peer {
    pub id: i64,
    pub name: String,
}

/// `RTCIceServer` as the browser and libwebrtc take it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

/// Messages to robots and operators. `ice_servers` is filled in by the glue (per-session TURN credentials).
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome { you: Peer },
    Robots { robots: Vec<RobotView> },
    Robot { robot: RobotView },
    /// To operators: the robot was disabled and is no longer listed.
    RobotGone { robot: RobotId },
    /// Session granted (to the operator) / started (to the robot).
    Session { session: SessionId, robot: RobotId, peer: Peer, ice_servers: Vec<IceServer> },
    Offer { session: SessionId, sdp: Value },
    Answer { session: SessionId, sdp: Value },
    Ice { session: SessionId, candidate: Value },
    End { session: SessionId, reason: EndReason },
    /// To the operator: the robot came back on a new connection and kept the session (SIG-007); the media
    /// path is probably gone (new network), so the console restarts ICE with a new offer.
    Resumed { session: SessionId },
    Error { code: ErrorCode, #[serde(skip_serializing_if = "Option::is_none")] robot: Option<RobotId> },
    Pong,
    /// To robots, after `welcome` and whenever the release changes: the current app release (ROB-008). Sent by
    /// the server glue, not by the hub.
    Update(crate::release::Release),
    /// To admins after `welcome`, and again when fleets or a robot's fleet change: everything they may see.
    Fleet { fleets: Vec<Fleet>, robots: Vec<RobotAdminView> },
    /// To admins of the robot's fleet, on any change.
    FleetRobot { robot: RobotAdminView },
    /// To admins of the robot's fleet, for each new event.
    RobotEvent { robot: RobotId, event: RobotEvent },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    Conn(ConnId),
    AllOperators,
    /// Admins whose access covers the fleet.
    Admins(FleetId),
}

pub type Out = Vec<(Target, ServerMsg)>;

struct RobotEntry {
    name: String,
    fleet: FleetId,
    disabled: bool,
    conn: Option<ConnId>,
    status: RobotStatus,
    location: Option<Location>,
    last_seen: Option<i64>,
    unread_errors: u32,
    session: Option<SessionId>,
}

impl RobotEntry {
    fn new(r: RobotRecord) -> Self {
        RobotEntry {
            name: r.name,
            fleet: r.fleet,
            disabled: r.disabled,
            conn: None,
            status: r.status,
            location: r.location,
            last_seen: r.last_seen,
            unread_errors: r.unread_errors,
            session: None,
        }
    }
}

struct OperatorConn {
    user: Peer,
    admin: Option<Access>,
}

struct Session {
    robot: RobotId,
    operator_conn: ConnId,
}

#[derive(Default)]
pub struct Hub {
    fleets: BTreeMap<FleetId, String>,
    robots: BTreeMap<RobotId, RobotEntry>,
    robot_conns: HashMap<ConnId, RobotId>,
    operators: HashMap<ConnId, OperatorConn>,
    sessions: HashMap<SessionId, Session>,
    next_session: SessionId,
}

impl Hub {
    /// Registered robots, listed even while offline (disabled ones to admins only).
    pub fn new(fleets: impl IntoIterator<Item = Fleet>, robots: impl IntoIterator<Item = RobotRecord>) -> Self {
        let mut hub = Hub { next_session: 1, ..Default::default() };
        hub.fleets = fleets.into_iter().map(|f| (f.id, f.name)).collect();
        for r in robots {
            hub.robots.insert(r.id, RobotEntry::new(r));
        }
        hub
    }

    pub fn operator_count(&self) -> usize {
        self.operators.len()
    }

    pub fn online_robot_count(&self) -> usize {
        self.robot_conns.len()
    }

    /// A robot connected. A previous connection of the same robot is replaced (cellular reconnect);
    /// the caller must close the returned connection. [resume] is the session the robot still holds
    /// (SIG-007): if it is still this robot's session, it carries on and both ends are told; otherwise
    /// the robot hears `end` for it.
    pub fn robot_connected(
        &mut self,
        conn: ConnId,
        record: RobotRecord,
        resume: Option<SessionId>,
    ) -> (Out, Option<ConnId>) {
        let id = record.id;
        let mut out = Vec::new();
        let old = self.robots.get(&id).and_then(|r| r.conn);
        if let Some(old) = old {
            self.robot_conns.remove(&old);
        }
        let current = self.robots.get(&id).and_then(|r| r.session);
        let resumed = current.filter(|&s| resume == Some(s) && self.sessions.contains_key(&s));
        if resumed.is_none() {
            self.end_robot_session(id, EndReason::RobotOffline, &mut out);
        }
        let fleet_before = self.robots.get(&id).map(|r| r.fleet);
        let name = record.name.clone();
        let entry = self.robots.entry(id).or_insert_with(|| RobotEntry::new(record.clone()));
        // Name and fleet may have changed with the CLI meanwhile; the last known state in memory is current.
        entry.name = record.name;
        entry.fleet = record.fleet;
        entry.disabled = false;
        entry.conn = Some(conn);
        self.robot_conns.insert(conn, id);
        out.push((Target::Conn(conn), ServerMsg::Welcome { you: Peer { id, name } }));
        match (resume, resumed) {
            (_, Some(session)) => {
                let s = &self.sessions[&session];
                let operator_conn = s.operator_conn;
                let peer = self.operators[&operator_conn].user.clone();
                out.push((Target::Conn(conn), ServerMsg::Session { session, robot: id, peer, ice_servers: Vec::new() }));
                out.push((Target::Conn(operator_conn), ServerMsg::Resumed { session }));
            }
            (Some(session), None) => {
                out.push((Target::Conn(conn), ServerMsg::End { session, reason: EndReason::RobotOffline }));
            }
            (None, None) => {}
        }
        if fleet_before.is_some_and(|f| f != self.robots[&id].fleet) {
            self.fleet_snapshots(&mut out);
        }
        self.robot_update(id, &mut out);
        (out, old)
    }

    /// `admin`: what the user may administer (`None` for operators).
    pub fn operator_connected(&mut self, conn: ConnId, user: Peer, admin: Option<Access>) -> Out {
        let you = user.clone();
        self.operators.insert(conn, OperatorConn { user, admin });
        let robots = self.robots.iter().filter(|(_, r)| !r.disabled).map(|(&id, _)| self.view(id)).collect();
        let mut out = vec![
            (Target::Conn(conn), ServerMsg::Welcome { you }),
            (Target::Conn(conn), ServerMsg::Robots { robots }),
        ];
        out.extend(self.fleet_snapshot(conn));
        out
    }

    pub fn disconnected(&mut self, conn: ConnId, now: i64) -> Out {
        let mut out = Vec::new();
        if let Some(id) = self.robot_conns.remove(&conn) {
            self.end_robot_session(id, EndReason::RobotOffline, &mut out);
            if let Some(r) = self.robots.get_mut(&id) {
                r.conn = None;
                r.last_seen = Some(now);
            }
            self.robot_update(id, &mut out);
        } else if self.operators.remove(&conn).is_some() {
            let mine: Vec<SessionId> =
                self.sessions.iter().filter(|(_, s)| s.operator_conn == conn).map(|(&id, _)| id).collect();
            for session in mine {
                self.end_session(session, EndReason::OperatorOffline, &mut out);
            }
        }
        out
    }

    /// The robot of a robot connection.
    pub fn robot_of(&self, conn: ConnId) -> Option<RobotId> {
        self.robot_conns.get(&conn).copied()
    }

    pub fn from_robot(&mut self, conn: ConnId, msg: RobotMsg) -> Out {
        let Some(&id) = self.robot_conns.get(&conn) else { return Vec::new() };
        let mut out = Vec::new();
        match msg {
            RobotMsg::Status(status) => {
                let entry = self.robots.get_mut(&id).expect("connected robot is known");
                if entry.status != status {
                    entry.status = status;
                    self.robot_update(id, &mut out);
                }
            }
            RobotMsg::Location(location) => {
                let entry = self.robots.get_mut(&id).expect("connected robot is known");
                if location.valid() && entry.location.as_ref() != Some(&location) {
                    entry.location = Some(location);
                    self.admin_update(id, &mut out);
                }
            }
            RobotMsg::Event(_) => {}
            RobotMsg::Answer { session, sdp } => {
                if let Some(s) = self.robot_session(session, id) {
                    out.push((Target::Conn(s.operator_conn), ServerMsg::Answer { session, sdp }));
                }
            }
            RobotMsg::Ice { session, candidate } => {
                if let Some(s) = self.robot_session(session, id) {
                    out.push((Target::Conn(s.operator_conn), ServerMsg::Ice { session, candidate }));
                }
            }
            RobotMsg::End { session } => {
                if self.robot_session(session, id).is_some() {
                    self.end_session(session, EndReason::RobotEnded, &mut out);
                }
            }
            RobotMsg::Ping => out.push((Target::Conn(conn), ServerMsg::Pong)),
        }
        out
    }

    /// A stored robot event: shown to the admins of its fleet; errors count as unread.
    pub fn robot_event(&mut self, robot: RobotId, event: RobotEvent) -> Out {
        let Some(entry) = self.robots.get_mut(&robot) else { return Vec::new() };
        let mut out = Vec::new();
        let fleet = entry.fleet;
        if event.level == EventLevel::Error {
            entry.unread_errors += 1;
            self.admin_update(robot, &mut out);
        }
        out.push((Target::Admins(fleet), ServerMsg::RobotEvent { robot, event }));
        out
    }

    pub fn from_operator(&mut self, conn: ConnId, msg: OperatorMsg) -> Out {
        let Some(op) = self.operators.get(&conn) else { return Vec::new() };
        let user = op.user.clone();
        let admin = op.admin.clone();
        let mut out = Vec::new();
        let no_session = |out: &mut Out| {
            out.push((Target::Conn(conn), ServerMsg::Error { code: ErrorCode::NoSession, robot: None }))
        };
        match msg {
            OperatorMsg::Take { robot } => self.take(conn, user, robot, false, &mut out),
            OperatorMsg::Offer { session, sdp } => match self.operator_session_robot(session, conn) {
                Some(robot_conn) => out.push((Target::Conn(robot_conn), ServerMsg::Offer { session, sdp })),
                None => no_session(&mut out),
            },
            OperatorMsg::Ice { session, candidate } => match self.operator_session_robot(session, conn) {
                Some(robot_conn) => out.push((Target::Conn(robot_conn), ServerMsg::Ice { session, candidate })),
                None => no_session(&mut out),
            },
            OperatorMsg::Release { session } => {
                if self.sessions.get(&session).is_some_and(|s| s.operator_conn == conn) {
                    self.end_session(session, EndReason::Released, &mut out);
                }
            }
            OperatorMsg::Ping => out.push((Target::Conn(conn), ServerMsg::Pong)),
            OperatorMsg::AdminEnd { session } => {
                let fleet = self.sessions.get(&session).and_then(|s| self.robots.get(&s.robot)).map(|r| r.fleet);
                match (admin, fleet) {
                    (Some(access), Some(fleet)) if access.covers(fleet) => {
                        self.end_session(session, EndReason::Admin, &mut out)
                    }
                    (Some(_), None) => no_session(&mut out),
                    _ => out.push((Target::Conn(conn), ServerMsg::Error { code: ErrorCode::Forbidden, robot: None })),
                }
            }
            OperatorMsg::AdminTake { robot } => {
                let fleet = self.robots.get(&robot).map(|r| r.fleet);
                match (admin, fleet) {
                    (Some(access), Some(fleet)) if access.covers(fleet) => self.take(conn, user, robot, true, &mut out),
                    // Robots outside the admin's fleets do not exist for them.
                    (Some(_), _) => out.push((Target::Conn(conn), ServerMsg::Error { code: ErrorCode::NotFound, robot: Some(robot) })),
                    (None, _) => out.push((Target::Conn(conn), ServerMsg::Error { code: ErrorCode::Forbidden, robot: Some(robot) })),
                }
            }
        }
        out
    }

    /// `take`; with [take_over] a session of another operator is ended first (they and the robot hear
    /// `end admin`, the robot stops and disarms before the new session starts, SAFE-010).
    fn take(&mut self, conn: ConnId, user: Peer, robot: RobotId, take_over: bool, out: &mut Out) {
        let error = match self.robots.get(&robot) {
            None => Some(ErrorCode::NotFound),
            Some(r) if r.disabled => Some(ErrorCode::NotFound),
            Some(r) if r.conn.is_none() => Some(ErrorCode::Offline),
            Some(r) => match r.session.and_then(|s| self.sessions.get(&s)) {
                // Taking a robot you already hold is idempotent: re-announce the session.
                Some(s) if s.operator_conn == conn => None,
                Some(_) if take_over => {
                    self.end_robot_session(robot, EndReason::Admin, out);
                    None
                }
                Some(_) => Some(ErrorCode::Busy),
                None => None,
            },
        };
        if let Some(code) = error {
            out.push((Target::Conn(conn), ServerMsg::Error { code, robot: Some(robot) }));
            return;
        }
        let entry = self.robots.get_mut(&robot).expect("checked above");
        let robot_conn = entry.conn.expect("checked above");
        let robot_peer = Peer { id: robot, name: entry.name.clone() };
        let session = match entry.session {
            Some(s) => s,
            None => {
                let s = self.next_session;
                self.next_session += 1;
                entry.session = Some(s);
                self.sessions.insert(s, Session { robot, operator_conn: conn });
                out.push((Target::Conn(robot_conn), ServerMsg::Session { session: s, robot, peer: user, ice_servers: Vec::new() }));
                self.robot_update(robot, out);
                s
            }
        };
        out.push((Target::Conn(conn), ServerMsg::Session { session, robot, peer: robot_peer, ice_servers: Vec::new() }));
    }

    // --- changes made by admins or the CLI (stored by the glue first) ---

    pub fn fleet_added(&mut self, fleet: Fleet) -> Out {
        self.fleets.insert(fleet.id, fleet.name);
        let mut out = Vec::new();
        self.fleet_snapshots(&mut out);
        out
    }

    /// A robot was added, renamed, moved to another fleet, disabled or enabled. A disabled robot loses its
    /// session and its connection: the caller closes the returned one.
    pub fn robot_changed(&mut self, record: RobotRecord) -> (Out, Option<ConnId>) {
        let id = record.id;
        let mut out = Vec::new();
        let Some(entry) = self.robots.get_mut(&id) else {
            let listed = !record.disabled;
            self.robots.insert(id, RobotEntry::new(record));
            if listed {
                self.robot_update(id, &mut out);
            } else {
                self.admin_update(id, &mut out);
            }
            return (out, None);
        };
        let moved = entry.fleet != record.fleet;
        let was_disabled = entry.disabled;
        entry.name = record.name;
        entry.fleet = record.fleet;
        entry.disabled = record.disabled;
        let mut dropped = None;
        if record.disabled {
            // The connection is closed without `end`: the robot stops on its own (SAFE-010).
            if let Some(conn) = entry.conn.take() {
                self.robot_conns.remove(&conn);
                dropped = Some(conn);
            }
            self.end_robot_session(id, EndReason::Admin, &mut out);
        }
        if moved {
            self.fleet_snapshots(&mut out);
        }
        match (was_disabled, record.disabled) {
            (false, true) => {
                out.push((Target::AllOperators, ServerMsg::RobotGone { robot: id }));
                self.admin_update(id, &mut out);
            }
            _ => self.robot_update(id, &mut out),
        }
        (out, dropped)
    }

    pub fn errors_read(&mut self, robot: RobotId) -> Out {
        let mut out = Vec::new();
        if let Some(entry) = self.robots.get_mut(&robot) {
            if entry.unread_errors != 0 {
                entry.unread_errors = 0;
                self.admin_update(robot, &mut out);
            }
        }
        out
    }

    // --- lookups for the glue ---

    /// Operator user id of a session's peer, for TURN credentials.
    pub fn session_operator(&self, session: SessionId) -> Option<UserId> {
        let s = self.sessions.get(&session)?;
        Some(self.operators.get(&s.operator_conn)?.user.id)
    }

    pub fn robot_conn(&self, robot: RobotId) -> Option<ConnId> {
        self.robots.get(&robot)?.conn
    }

    /// Admin connections whose access covers the fleet.
    pub fn admins_of(&self, fleet: FleetId) -> impl Iterator<Item = ConnId> + '_ {
        self.operators.iter().filter(move |(_, o)| o.admin.as_ref().is_some_and(|a| a.covers(fleet))).map(|(&c, _)| c)
    }

    /// Console connections of a user (closed when their rights change).
    pub fn user_conns(&self, user: UserId) -> Vec<ConnId> {
        self.operators.iter().filter(|(_, o)| o.user.id == user).map(|(&c, _)| c).collect()
    }

    /// ADM-013: connected users and the robots each one holds.
    pub fn online_users(&self) -> HashMap<UserId, Vec<RobotId>> {
        let mut users: HashMap<UserId, Vec<RobotId>> = HashMap::new();
        for op in self.operators.values() {
            users.entry(op.user.id).or_default();
        }
        for s in self.sessions.values() {
            if let Some(op) = self.operators.get(&s.operator_conn) {
                users.entry(op.user.id).or_default().push(s.robot);
            }
        }
        users
    }

    fn robot_session(&self, session: SessionId, robot: RobotId) -> Option<&Session> {
        self.sessions.get(&session).filter(|s| s.robot == robot)
    }

    fn operator_session_robot(&self, session: SessionId, conn: ConnId) -> Option<ConnId> {
        let s = self.sessions.get(&session).filter(|s| s.operator_conn == conn)?;
        self.robots.get(&s.robot)?.conn
    }

    /// Ends the robot's session without a robot update: the caller announces the robot once itself.
    fn end_robot_session(&mut self, robot: RobotId, reason: EndReason, out: &mut Out) {
        if let Some(session) = self.robots.get(&robot).and_then(|r| r.session) {
            self.close_session(session, reason, out);
        }
    }

    fn end_session(&mut self, session: SessionId, reason: EndReason, out: &mut Out) {
        if let Some(robot) = self.close_session(session, reason, out) {
            self.robot_update(robot, out);
        }
    }

    /// Both ends hear about it, except the side whose connection is gone (it gets nothing).
    fn close_session(&mut self, session: SessionId, reason: EndReason, out: &mut Out) -> Option<RobotId> {
        let s = self.sessions.remove(&session)?;
        let robot_conn = self.robots.get_mut(&s.robot).and_then(|r| {
            r.session = None;
            r.conn
        });
        if reason != EndReason::OperatorOffline {
            out.push((Target::Conn(s.operator_conn), ServerMsg::End { session, reason }));
        }
        if reason != EndReason::RobotOffline {
            if let Some(rc) = robot_conn {
                out.push((Target::Conn(rc), ServerMsg::End { session, reason }));
            }
        }
        Some(s.robot)
    }

    fn view(&self, id: RobotId) -> RobotView {
        let r = &self.robots[&id];
        RobotView {
            id,
            name: r.name.clone(),
            online: r.conn.is_some(),
            status: r.status.clone(),
            operator: r.session.and_then(|s| self.session_operator(s)),
        }
    }

    fn admin_view(&self, id: RobotId) -> RobotAdminView {
        let r = &self.robots[&id];
        let operator = r.session.and_then(|s| self.sessions.get(&s)).and_then(|s| self.operators.get(&s.operator_conn));
        RobotAdminView {
            id,
            name: r.name.clone(),
            fleet: r.fleet,
            disabled: r.disabled,
            online: r.conn.is_some(),
            status: r.status.clone(),
            location: r.location.clone(),
            last_seen: if r.conn.is_some() { None } else { r.last_seen },
            unread_errors: r.unread_errors,
            session: r.session,
            operator: operator.map(|o| o.user.clone()),
        }
    }

    /// Operators see enabled robots; admins of the fleet see everything about it.
    fn robot_update(&self, id: RobotId, out: &mut Out) {
        if !self.robots[&id].disabled {
            out.push((Target::AllOperators, ServerMsg::Robot { robot: self.view(id) }));
        }
        self.admin_update(id, out);
    }

    fn admin_update(&self, id: RobotId, out: &mut Out) {
        out.push((Target::Admins(self.robots[&id].fleet), ServerMsg::FleetRobot { robot: self.admin_view(id) }));
    }

    fn fleet_snapshot(&self, conn: ConnId) -> Option<(Target, ServerMsg)> {
        let access = self.operators.get(&conn)?.admin.as_ref()?;
        let fleets = self
            .fleets
            .iter()
            .filter(|(id, _)| access.covers(**id))
            .map(|(&id, name)| Fleet { id, name: name.clone() })
            .collect();
        let robots =
            self.robots.iter().filter(|(_, r)| access.covers(r.fleet)).map(|(&id, _)| self.admin_view(id)).collect();
        Some((Target::Conn(conn), ServerMsg::Fleet { fleets, robots }))
    }

    fn fleet_snapshots(&self, out: &mut Out) {
        let mut admins: Vec<ConnId> = self.operators.iter().filter(|(_, o)| o.admin.is_some()).map(|(&c, _)| c).collect();
        admins.sort_unstable();
        out.extend(admins.into_iter().filter_map(|c| self.fleet_snapshot(c)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R1: RobotId = 1;
    const ROBOT_CONN: ConnId = 100;
    const OP_A: ConnId = 200;
    const OP_B: ConnId = 201;

    fn raw(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    fn rec(id: RobotId, name: &str) -> RobotRecord {
        rec_in(id, name, 1)
    }

    fn rec_in(id: RobotId, name: &str, fleet: FleetId) -> RobotRecord {
        RobotRecord {
            id,
            name: name.into(),
            fleet,
            disabled: false,
            status: RobotStatus::default(),
            location: None,
            last_seen: None,
            unread_errors: 0,
        }
    }

    fn fleets() -> Vec<Fleet> {
        vec![Fleet { id: 1, name: "default".into() }, Fleet { id: 2, name: "north".into() }]
    }

    fn peer(id: i64, name: &str) -> Peer {
        Peer { id, name: name.into() }
    }

    fn hub() -> Hub {
        let mut hub = Hub::new(fleets(), [rec(R1, "r1"), rec(2, "r2")]);
        hub.operator_connected(OP_A, peer(10, "alice"), None);
        hub.operator_connected(OP_B, peer(11, "bob"), None);
        hub.robot_connected(ROBOT_CONN, rec(R1, "r1"), None);
        hub
    }

    fn take(hub: &mut Hub, conn: ConnId) -> SessionId {
        let out = hub.from_operator(conn, OperatorMsg::Take { robot: R1 });
        match out.iter().find(|(t, _)| *t == Target::Conn(conn)) {
            Some((_, ServerMsg::Session { session, .. })) => *session,
            other => panic!("expected session, got {other:?}"),
        }
    }

    fn to(out: &Out, conn: ConnId) -> Vec<&ServerMsg> {
        out.iter().filter(|(t, _)| *t == Target::Conn(conn)).map(|(_, m)| m).collect()
    }

    #[test]
    fn operator_gets_snapshot_with_offline_robots() {
        let mut hub = Hub::new(fleets(), [rec(R1, "r1"), rec(2, "r2")]);
        let out = hub.operator_connected(OP_A, peer(10, "alice"), None);
        assert_eq!(out.len(), 2, "no fleet view for operators");
        let ServerMsg::Robots { robots } = &out[1].1 else { panic!() };
        assert_eq!(robots.len(), 2);
        assert!(robots.iter().all(|r| !r.online));
    }

    #[test]
    fn robot_online_is_broadcast() {
        let mut hub = Hub::new(fleets(), []);
        let (out, replaced) = hub.robot_connected(ROBOT_CONN, rec(7, "new"), None);
        assert_eq!(replaced, None);
        assert!(matches!(&out[1], (Target::AllOperators, ServerMsg::Robot { robot }) if robot.online && robot.id == 7));
    }

    #[test]
    fn take_starts_session_on_both_ends() {
        let mut hub = hub();
        let out = hub.from_operator(OP_A, OperatorMsg::Take { robot: R1 });
        assert!(matches!(to(&out, ROBOT_CONN)[..], [ServerMsg::Session { peer: Peer { id: 10, .. }, .. }]));
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::Session { robot: R1, .. }]));
        assert!(out.iter().any(|(t, m)| *t == Target::AllOperators
            && matches!(m, ServerMsg::Robot { robot } if robot.operator == Some(10))));
    }

    #[test]
    fn one_operator_per_robot() {
        let mut hub = hub();
        let session = take(&mut hub, OP_A);
        let out = hub.from_operator(OP_B, OperatorMsg::Take { robot: R1 });
        assert!(matches!(to(&out, OP_B)[..], [ServerMsg::Error { code: ErrorCode::Busy, robot: Some(R1) }]));
        // Taking it again from the same console re-announces the same session.
        assert_eq!(take(&mut hub, OP_A), session);
    }

    #[test]
    fn take_errors() {
        let mut hub = hub();
        let out = hub.from_operator(OP_A, OperatorMsg::Take { robot: 2 });
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::Error { code: ErrorCode::Offline, .. }]));
        let out = hub.from_operator(OP_A, OperatorMsg::Take { robot: 99 });
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::Error { code: ErrorCode::NotFound, .. }]));
    }

    #[test]
    fn signaling_is_relayed_only_within_the_session() {
        let mut hub = hub();
        let session = take(&mut hub, OP_A);
        let out = hub.from_operator(OP_A, OperatorMsg::Offer { session, sdp: raw("\"o\"") });
        assert!(matches!(to(&out, ROBOT_CONN)[..], [ServerMsg::Offer { sdp, .. }] if sdp == "o"));
        let out = hub.from_robot(ROBOT_CONN, RobotMsg::Answer { session, sdp: raw("\"a\"") });
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::Answer { .. }]));
        let out = hub.from_robot(ROBOT_CONN, RobotMsg::Ice { session, candidate: raw("{}") });
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::Ice { .. }]));

        // Another operator cannot inject into a session that is not theirs.
        let out = hub.from_operator(OP_B, OperatorMsg::Offer { session, sdp: raw("\"x\"") });
        assert!(to(&out, ROBOT_CONN).is_empty());
        assert!(matches!(to(&out, OP_B)[..], [ServerMsg::Error { code: ErrorCode::NoSession, .. }]));
        // A robot cannot answer a session that is not its own.
        hub.robot_connected(101, rec(2, "r2"), None);
        let out = hub.from_robot(101, RobotMsg::Answer { session, sdp: raw("\"x\"") });
        assert!(out.is_empty());
    }

    #[test]
    fn release_frees_the_robot() {
        let mut hub = hub();
        let session = take(&mut hub, OP_A);
        let out = hub.from_operator(OP_A, OperatorMsg::Release { session });
        assert!(matches!(to(&out, ROBOT_CONN)[..], [ServerMsg::End { reason: EndReason::Released, .. }]));
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::End { .. }]));
        assert_ne!(take(&mut hub, OP_B), session);
    }

    #[test]
    fn operator_disconnect_ends_its_sessions() {
        let mut hub = hub();
        take(&mut hub, OP_A);
        let out = hub.disconnected(OP_A, 0);
        assert!(matches!(to(&out, ROBOT_CONN)[..], [ServerMsg::End { reason: EndReason::OperatorOffline, .. }]));
        assert!(to(&out, OP_A).is_empty());
        assert_eq!(hub.operator_count(), 1);
        take(&mut hub, OP_B);
    }

    #[test]
    fn robot_disconnect_ends_session_and_goes_offline() {
        let mut hub = hub();
        take(&mut hub, OP_A);
        let out = hub.disconnected(ROBOT_CONN, 0);
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::End { reason: EndReason::RobotOffline, .. }]));
        let updates: Vec<_> = out.iter().filter_map(|(_, m)| if let ServerMsg::Robot { robot } = m { Some(robot) } else { None }).collect();
        assert!(matches!(updates[..], [r] if !r.online && r.operator.is_none()));
        assert_eq!(hub.online_robot_count(), 0);
    }

    #[test]
    fn robot_reconnect_replaces_old_connection() {
        let mut hub = hub();
        take(&mut hub, OP_A);
        let (out, replaced) = hub.robot_connected(102, rec(R1, "r1"), None);
        assert_eq!(replaced, Some(ROBOT_CONN));
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::End { .. }]));
        // The stale connection closing later must not take the new one offline.
        assert!(hub.disconnected(ROBOT_CONN, 0).is_empty());
        assert_eq!(hub.online_robot_count(), 1);
    }

    #[test]
    fn robot_reconnect_resumes_its_session() {
        let mut hub = hub();
        let session = take(&mut hub, OP_A);
        let (out, replaced) = hub.robot_connected(102, rec(R1, "r1"), Some(session));
        assert_eq!(replaced, Some(ROBOT_CONN));
        assert!(matches!(to(&out, 102)[..], [ServerMsg::Welcome { .. }, ServerMsg::Session { session: s, peer: Peer { id: 10, .. }, .. }] if *s == session));
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::Resumed { session: s }] if *s == session));
        assert!(out.iter().any(|(t, m)| *t == Target::AllOperators
            && matches!(m, ServerMsg::Robot { robot } if robot.online && robot.operator == Some(10))));
        assert!(hub.disconnected(ROBOT_CONN, 0).is_empty());
        // The new connection carries the session: the restart offer reaches it.
        let out = hub.from_operator(OP_A, OperatorMsg::Offer { session, sdp: raw("\"o\"") });
        assert!(matches!(to(&out, 102)[..], [ServerMsg::Offer { .. }]));
    }

    #[test]
    fn robot_reconnect_with_a_stale_session_hears_end() {
        let mut hub = hub();
        let session = take(&mut hub, OP_A);
        // The server noticed the old connection die first: the session is over.
        hub.disconnected(ROBOT_CONN, 0);
        let (out, _) = hub.robot_connected(102, rec(R1, "r1"), Some(session));
        assert!(matches!(to(&out, 102)[..], [ServerMsg::Welcome { .. }, ServerMsg::End { reason: EndReason::RobotOffline, .. }]));
        assert!(to(&out, OP_A).is_empty());
        // Not the current session (e.g. the app restarted meanwhile): the current one ends as well.
        let session = take(&mut hub, OP_A);
        let (out, _) = hub.robot_connected(103, rec(R1, "r1"), Some(session + 100));
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::End { reason: EndReason::RobotOffline, .. }]));
        assert!(matches!(to(&out, 103)[..], [ServerMsg::Welcome { .. }, ServerMsg::End { .. }]));
    }

    #[test]
    fn robot_ends_session() {
        let mut hub = hub();
        let session = take(&mut hub, OP_A);
        let out = hub.from_robot(ROBOT_CONN, RobotMsg::End { session });
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::End { reason: EndReason::RobotEnded, .. }]));
    }

    #[test]
    fn status_broadcast_only_on_change() {
        let mut hub = hub();
        let status = RobotStatus { mode: Some(RobotMode::NeedsHelp), phone_battery: Some(55), ..Default::default() };
        let out = hub.from_robot(ROBOT_CONN, RobotMsg::Status(status.clone()));
        assert!(matches!(&out[..], [(Target::AllOperators, ServerMsg::Robot { .. }), (Target::Admins(1), ServerMsg::FleetRobot { .. })]));
        assert!(hub.from_robot(ROBOT_CONN, RobotMsg::Status(status)).is_empty());
    }

    #[test]
    fn messages_parse_from_json() {
        let m: OperatorMsg = serde_json::from_str(r#"{"type":"offer","session":3,"sdp":{"type":"offer","sdp":"v=0"}}"#).unwrap();
        assert!(matches!(m, OperatorMsg::Offer { session: 3, .. }));
        let m: RobotMsg = serde_json::from_str(r#"{"type":"status","mode":"NEEDS_HELP","phone_battery":50,"future":1}"#).unwrap();
        assert!(matches!(m, RobotMsg::Status(RobotStatus { mode: Some(RobotMode::NeedsHelp), .. })));
        let json = serde_json::to_string(&ServerMsg::End { session: 1, reason: EndReason::Released }).unwrap();
        assert_eq!(json, r#"{"type":"end","session":1,"reason":"released"}"#);
    }

    // --- fleet administration ---

    const ADMIN: ConnId = 300;
    const NORTH_ADMIN: ConnId = 301;

    /// r1 in the default fleet (online, `hub()`), r2 in the default fleet, r3 in "north"; an all-fleet admin and
    /// a "north" admin.
    fn admin_hub() -> Hub {
        let mut hub = Hub::new(fleets(), [rec(R1, "r1"), rec(2, "r2"), rec_in(3, "r3", 2)]);
        hub.operator_connected(OP_A, peer(10, "alice"), None);
        hub.operator_connected(OP_B, peer(11, "bob"), None);
        hub.operator_connected(ADMIN, peer(20, "root"), Some(Access::All));
        hub.operator_connected(NORTH_ADMIN, peer(21, "nora"), Some(Access::Fleets([2].into())));
        hub.robot_connected(ROBOT_CONN, rec(R1, "r1"), None);
        hub
    }

    fn fleet_robots(out: &Out, conn: ConnId) -> Vec<RobotId> {
        match to(out, conn)[..] {
            [.., ServerMsg::Fleet { robots, .. }] => robots.iter().map(|r| r.id).collect(),
            _ => panic!("no fleet snapshot"),
        }
    }

    #[test]
    fn admins_see_only_their_fleets() {
        let mut hub = admin_hub();
        let out = hub.operator_connected(400, peer(20, "root"), Some(Access::All));
        assert_eq!(fleet_robots(&out, 400), [1, 2, 3]);
        let out = hub.operator_connected(401, peer(21, "nora"), Some(Access::Fleets([2].into())));
        assert_eq!(fleet_robots(&out, 401), [3]);
        let ServerMsg::Fleet { fleets, .. } = to(&out, 401)[2] else { panic!() };
        assert_eq!(fleets, &[Fleet { id: 2, name: "north".into() }]);
        let mut admins: Vec<_> = hub.admins_of(1).collect();
        admins.sort();
        assert_eq!(admins, [ADMIN, 400]);
    }

    #[test]
    fn admin_updates_follow_the_fleet() {
        let mut hub = admin_hub();
        let status = RobotStatus { phone_battery: Some(80), ..Default::default() };
        let out = hub.from_robot(ROBOT_CONN, RobotMsg::Status(status));
        assert!(out.iter().any(|(t, m)| *t == Target::Admins(1)
            && matches!(m, ServerMsg::FleetRobot { robot } if robot.status.phone_battery == Some(80))));

        // Location goes to admins only, and only when it changed or makes sense.
        let at = Location { lat: 50.45, lon: 30.52, accuracy_m: Some(5.0), at: 1 };
        let out = hub.from_robot(ROBOT_CONN, RobotMsg::Location(at.clone()));
        assert!(matches!(&out[..], [(Target::Admins(1), ServerMsg::FleetRobot { robot })] if robot.location == Some(at.clone())));
        assert!(hub.from_robot(ROBOT_CONN, RobotMsg::Location(at)).is_empty());
        let nowhere = Location { lat: 91.0, lon: 0.0, accuracy_m: None, at: 2 };
        assert!(hub.from_robot(ROBOT_CONN, RobotMsg::Location(nowhere)).is_empty());
    }

    #[test]
    fn errors_are_counted_until_read() {
        let mut hub = admin_hub();
        let event = |id, level| RobotEvent { id, level, code: "c".into(), message: String::new(), at: 0, received: 0 };
        let out = hub.robot_event(R1, event(1, EventLevel::Info));
        assert!(matches!(&out[..], [(Target::Admins(1), ServerMsg::RobotEvent { robot: R1, .. })]));
        let out = hub.robot_event(R1, event(2, EventLevel::Error));
        assert!(matches!(&out[0], (Target::Admins(1), ServerMsg::FleetRobot { robot }) if robot.unread_errors == 1));
        let out = hub.errors_read(R1);
        assert!(matches!(&out[..], [(_, ServerMsg::FleetRobot { robot })] if robot.unread_errors == 0));
        assert!(hub.errors_read(R1).is_empty());
    }

    #[test]
    fn admin_ends_a_session() {
        let mut hub = admin_hub();
        let session = take(&mut hub, OP_A);
        // Operators cannot; admins of other fleets cannot.
        let out = hub.from_operator(OP_B, OperatorMsg::AdminEnd { session });
        assert!(matches!(to(&out, OP_B)[..], [ServerMsg::Error { code: ErrorCode::Forbidden, .. }]));
        let out = hub.from_operator(NORTH_ADMIN, OperatorMsg::AdminEnd { session });
        assert!(matches!(to(&out, NORTH_ADMIN)[..], [ServerMsg::Error { code: ErrorCode::Forbidden, .. }]));
        assert!(to(&out, ROBOT_CONN).is_empty());

        let out = hub.from_operator(ADMIN, OperatorMsg::AdminEnd { session });
        assert!(matches!(to(&out, ROBOT_CONN)[..], [ServerMsg::End { reason: EndReason::Admin, .. }]));
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::End { reason: EndReason::Admin, .. }]));
        let out = hub.from_operator(ADMIN, OperatorMsg::AdminEnd { session });
        assert!(matches!(to(&out, ADMIN)[..], [ServerMsg::Error { code: ErrorCode::NoSession, .. }]));
    }

    #[test]
    fn admin_takes_over() {
        let mut hub = admin_hub();
        let old = take(&mut hub, OP_A);
        let out = hub.from_operator(NORTH_ADMIN, OperatorMsg::AdminTake { robot: R1 });
        assert!(matches!(to(&out, NORTH_ADMIN)[..], [ServerMsg::Error { code: ErrorCode::NotFound, .. }]));
        let out = hub.from_operator(OP_B, OperatorMsg::AdminTake { robot: R1 });
        assert!(matches!(to(&out, OP_B)[..], [ServerMsg::Error { code: ErrorCode::Forbidden, .. }]));

        let out = hub.from_operator(ADMIN, OperatorMsg::AdminTake { robot: R1 });
        // The robot hears `end` (it stops and disarms) before the new session starts.
        match to(&out, ROBOT_CONN)[..] {
            [ServerMsg::End { session, reason: EndReason::Admin }, ServerMsg::Session { session: new, peer, .. }] => {
                assert_eq!(*session, old);
                assert_ne!(*new, old);
                assert_eq!(peer.id, 20);
            }
            ref other => panic!("{other:?}"),
        }
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::End { reason: EndReason::Admin, .. }]));
        assert!(matches!(to(&out, ADMIN)[..], [ServerMsg::Session { robot: R1, .. }]));
        assert!(out.iter().any(|(_, m)| matches!(m, ServerMsg::FleetRobot { robot }
            if robot.operator.as_ref().is_some_and(|p| p.id == 20))));
        // The operator lost it for good.
        let out = hub.from_operator(OP_A, OperatorMsg::Take { robot: R1 });
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::Error { code: ErrorCode::Busy, .. }]));
        // A free robot is taken like with `take`; offline ones are offline.
        let out = hub.from_operator(ADMIN, OperatorMsg::AdminTake { robot: 2 });
        assert!(matches!(to(&out, ADMIN)[..], [ServerMsg::Error { code: ErrorCode::Offline, .. }]));
    }

    #[test]
    fn disabling_a_robot_drops_it() {
        let mut hub = admin_hub();
        take(&mut hub, OP_A);
        let (out, dropped) = hub.robot_changed(RobotRecord { disabled: true, ..rec(R1, "r1") });
        assert_eq!(dropped, Some(ROBOT_CONN));
        assert!(to(&out, ROBOT_CONN).is_empty());
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::End { reason: EndReason::Admin, .. }]));
        assert!(out.iter().any(|(t, m)| *t == Target::AllOperators && matches!(m, ServerMsg::RobotGone { robot: R1 })));
        assert!(out.iter().any(|(_, m)| matches!(m, ServerMsg::FleetRobot { robot } if robot.disabled && !robot.online)));
        assert!(hub.disconnected(ROBOT_CONN, 5).is_empty());
        assert_eq!(hub.online_robot_count(), 0);
        let out = hub.from_operator(OP_A, OperatorMsg::Take { robot: R1 });
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::Error { code: ErrorCode::NotFound, .. }]));
        // New operators do not see it, admins do.
        let out = hub.operator_connected(402, peer(12, "carl"), None);
        assert!(matches!(to(&out, 402)[1], ServerMsg::Robots { robots } if robots.iter().all(|r| r.id != R1)));
        let out = hub.operator_connected(403, peer(20, "root"), Some(Access::All));
        assert_eq!(fleet_robots(&out, 403), [1, 2, 3]);
        // Enabled again: listed again.
        let (out, _) = hub.robot_changed(rec(R1, "r1"));
        assert!(out.iter().any(|(t, m)| *t == Target::AllOperators && matches!(m, ServerMsg::Robot { robot } if robot.id == R1)));
    }

    #[test]
    fn moving_a_robot_refreshes_admin_views() {
        let mut hub = admin_hub();
        let (out, dropped) = hub.robot_changed(rec_in(R1, "r1", 2));
        assert_eq!(dropped, None);
        assert_eq!(fleet_robots(&out, NORTH_ADMIN), [1, 3]);
        assert_eq!(fleet_robots(&out, ADMIN), [1, 2, 3]);
        // A new robot and a new fleet show up as well.
        let (out, _) = hub.robot_changed(rec_in(9, "r9", 2));
        assert!(out.iter().any(|(t, m)| *t == Target::Admins(2) && matches!(m, ServerMsg::FleetRobot { robot } if robot.id == 9)));
        let out = hub.fleet_added(Fleet { id: 3, name: "south".into() });
        assert!(matches!(to(&out, ADMIN)[..], [ServerMsg::Fleet { fleets, .. }] if fleets.len() == 3));
        assert!(matches!(to(&out, NORTH_ADMIN)[..], [ServerMsg::Fleet { fleets, .. }] if fleets.len() == 1));
    }

    #[test]
    fn last_known_state_survives_going_offline() {
        let mut hub = admin_hub();
        let status = RobotStatus { phone_battery: Some(33), ..Default::default() };
        hub.from_robot(ROBOT_CONN, RobotMsg::Status(status));
        let out = hub.disconnected(ROBOT_CONN, 1234);
        let view = out.iter().find_map(|(_, m)| if let ServerMsg::FleetRobot { robot } = m { Some(robot) } else { None }).unwrap();
        assert_eq!((view.online, view.last_seen, view.status.phone_battery), (false, Some(1234), Some(33)));
    }

    #[test]
    fn online_users_and_their_robots() {
        let mut hub = admin_hub();
        take(&mut hub, OP_A);
        let users = hub.online_users();
        assert_eq!(users[&10], [R1]);
        assert!(users[&11].is_empty());
        assert_eq!(hub.user_conns(20), [ADMIN]);
    }

    #[test]
    fn robot_messages_parse() {
        let m: RobotMsg = serde_json::from_str(
            r#"{"type":"status","phone":"SM-J710F","network":{"type":"cellular","operator":"kyivstar","signal":3},"device_owner":true}"#,
        )
        .unwrap();
        assert!(matches!(m, RobotMsg::Status(RobotStatus { network: Some(Network { kind: NetworkKind::Cellular, signal: Some(3), .. }), .. })));
        let m: RobotMsg = serde_json::from_str(r#"{"type":"location","lat":50.4,"lon":30.5,"at":7}"#).unwrap();
        assert!(matches!(m, RobotMsg::Location(Location { accuracy_m: None, at: 7, .. })));
        let m: RobotMsg = serde_json::from_str(r#"{"type":"event","level":"error","code":"crash","message":"boom","at":9}"#).unwrap();
        assert!(matches!(m, RobotMsg::Event(EventIn { level: EventLevel::Error, .. })));
        let long = EventIn { level: EventLevel::Info, code: "ж".repeat(100), message: "x".repeat(5000), at: 0 }.sanitized();
        assert_eq!((long.code.chars().count(), long.message.len()), (MAX_EVENT_CODE, MAX_EVENT_MESSAGE));
        let m: OperatorMsg = serde_json::from_str(r#"{"type":"admin_take","robot":3}"#).unwrap();
        assert!(matches!(m, OperatorMsg::AdminTake { robot: 3 }));
    }
}
