//! Signaling hub (docs/08-backend.md "Signaling"): who is online, which operator controls which robot
//! (ARCH-001), and relaying opaque WebRTC offers/answers/candidates between the two ends of a session.
//! Media and DRIVE never pass through here: they go peer-to-peer (or via TURN) over WebRTC.
//!
//! Pure state machine: every input returns the messages to deliver, the WebSocket glue in `server.rs`
//! only serializes and sends them. No I/O, no clocks, so it is unit-tested directly.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type ConnId = u64;
pub type RobotId = i64;
pub type UserId = i64;
pub type SessionId = u64;

/// Robot modes reported by the phone (ARCH-002).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RobotMode {
    Idle,
    Teleop,
    Auto,
    NeedsHelp,
}

/// Coarse robot state for the operator list. Sent by the robot only on change (PWR-012: no chatter).
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
}

/// Messages from a robot.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RobotMsg {
    Status(RobotStatus),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    Offline,
    Busy,
    NoSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Released,
    RobotEnded,
    RobotOffline,
    OperatorOffline,
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
}

#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    Conn(ConnId),
    AllOperators,
}

pub type Out = Vec<(Target, ServerMsg)>;

struct RobotEntry {
    name: String,
    conn: Option<ConnId>,
    status: RobotStatus,
    session: Option<SessionId>,
}

struct OperatorConn {
    user: Peer,
}

struct Session {
    robot: RobotId,
    operator_conn: ConnId,
}

#[derive(Default)]
pub struct Hub {
    robots: BTreeMap<RobotId, RobotEntry>,
    robot_conns: HashMap<ConnId, RobotId>,
    operators: HashMap<ConnId, OperatorConn>,
    sessions: HashMap<SessionId, Session>,
    next_session: SessionId,
}

impl Hub {
    /// Registered robots, listed even while offline.
    pub fn new(known: impl IntoIterator<Item = (RobotId, String)>) -> Self {
        let mut hub = Hub { next_session: 1, ..Default::default() };
        for (id, name) in known {
            hub.robots.insert(id, RobotEntry { name, conn: None, status: RobotStatus::default(), session: None });
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
        id: RobotId,
        name: String,
        resume: Option<SessionId>,
    ) -> (Out, Option<ConnId>) {
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
        let entry = self.robots.entry(id).or_insert_with(|| RobotEntry {
            name: name.clone(),
            conn: None,
            status: RobotStatus::default(),
            session: None,
        });
        entry.name = name.clone();
        entry.conn = Some(conn);
        entry.status = RobotStatus::default();
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
        out.push(self.robot_update(id));
        (out, old)
    }

    pub fn operator_connected(&mut self, conn: ConnId, user: Peer) -> Out {
        let you = user.clone();
        self.operators.insert(conn, OperatorConn { user });
        let robots = self.robots.keys().map(|&id| self.view(id)).collect();
        vec![
            (Target::Conn(conn), ServerMsg::Welcome { you }),
            (Target::Conn(conn), ServerMsg::Robots { robots }),
        ]
    }

    pub fn disconnected(&mut self, conn: ConnId) -> Out {
        let mut out = Vec::new();
        if let Some(id) = self.robot_conns.remove(&conn) {
            self.end_robot_session(id, EndReason::RobotOffline, &mut out);
            if let Some(r) = self.robots.get_mut(&id) {
                r.conn = None;
                r.status = RobotStatus::default();
            }
            out.push(self.robot_update(id));
        } else if self.operators.remove(&conn).is_some() {
            let mine: Vec<SessionId> =
                self.sessions.iter().filter(|(_, s)| s.operator_conn == conn).map(|(&id, _)| id).collect();
            for session in mine {
                self.end_session(session, EndReason::OperatorOffline, &mut out);
            }
        }
        out
    }

    pub fn from_robot(&mut self, conn: ConnId, msg: RobotMsg) -> Out {
        let Some(&id) = self.robot_conns.get(&conn) else { return Vec::new() };
        let mut out = Vec::new();
        match msg {
            RobotMsg::Status(status) => {
                let entry = self.robots.get_mut(&id).expect("connected robot is known");
                if entry.status != status {
                    entry.status = status;
                    out.push(self.robot_update(id));
                }
            }
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

    pub fn from_operator(&mut self, conn: ConnId, msg: OperatorMsg) -> Out {
        let Some(op) = self.operators.get(&conn) else { return Vec::new() };
        let user = op.user.clone();
        let mut out = Vec::new();
        let no_session = |out: &mut Out| {
            out.push((Target::Conn(conn), ServerMsg::Error { code: ErrorCode::NoSession, robot: None }))
        };
        match msg {
            OperatorMsg::Take { robot } => {
                let error = match self.robots.get(&robot) {
                    None => Some(ErrorCode::NotFound),
                    Some(r) if r.conn.is_none() => Some(ErrorCode::Offline),
                    Some(r) => match r.session.and_then(|s| self.sessions.get(&s)) {
                        // Taking a robot you already hold is idempotent: re-announce the session.
                        Some(s) if s.operator_conn == conn => None,
                        Some(_) => Some(ErrorCode::Busy),
                        None => None,
                    },
                };
                if let Some(code) = error {
                    out.push((Target::Conn(conn), ServerMsg::Error { code, robot: Some(robot) }));
                    return out;
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
                        out.push(self.robot_update(robot));
                        s
                    }
                };
                out.push((Target::Conn(conn), ServerMsg::Session { session, robot, peer: robot_peer, ice_servers: Vec::new() }));
            }
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
        }
        out
    }

    /// Operator user id of a session's peer, for TURN credentials.
    pub fn session_operator(&self, session: SessionId) -> Option<UserId> {
        let s = self.sessions.get(&session)?;
        Some(self.operators.get(&s.operator_conn)?.user.id)
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
            out.push(self.robot_update(robot));
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

    fn robot_update(&self, id: RobotId) -> (Target, ServerMsg) {
        (Target::AllOperators, ServerMsg::Robot { robot: self.view(id) })
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

    fn hub() -> Hub {
        let mut hub = Hub::new([(R1, "r1".to_owned()), (2, "r2".to_owned())]);
        hub.operator_connected(OP_A, Peer { id: 10, name: "alice".into() });
        hub.operator_connected(OP_B, Peer { id: 11, name: "bob".into() });
        hub.robot_connected(ROBOT_CONN, R1, "r1".into(), None);
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
        let mut hub = Hub::new([(R1, "r1".to_owned()), (2, "r2".to_owned())]);
        let out = hub.operator_connected(OP_A, Peer { id: 10, name: "alice".into() });
        let ServerMsg::Robots { robots } = &out[1].1 else { panic!() };
        assert_eq!(robots.len(), 2);
        assert!(robots.iter().all(|r| !r.online));
    }

    #[test]
    fn robot_online_is_broadcast() {
        let mut hub = Hub::new([]);
        let (out, replaced) = hub.robot_connected(ROBOT_CONN, 7, "new".into(), None);
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
        hub.robot_connected(101, 2, "r2".into(), None);
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
        let out = hub.disconnected(OP_A);
        assert!(matches!(to(&out, ROBOT_CONN)[..], [ServerMsg::End { reason: EndReason::OperatorOffline, .. }]));
        assert!(to(&out, OP_A).is_empty());
        assert_eq!(hub.operator_count(), 1);
        take(&mut hub, OP_B);
    }

    #[test]
    fn robot_disconnect_ends_session_and_goes_offline() {
        let mut hub = hub();
        take(&mut hub, OP_A);
        let out = hub.disconnected(ROBOT_CONN);
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::End { reason: EndReason::RobotOffline, .. }]));
        let updates: Vec<_> = out.iter().filter_map(|(_, m)| if let ServerMsg::Robot { robot } = m { Some(robot) } else { None }).collect();
        assert!(matches!(updates[..], [r] if !r.online && r.operator.is_none()));
        assert_eq!(hub.online_robot_count(), 0);
    }

    #[test]
    fn robot_reconnect_replaces_old_connection() {
        let mut hub = hub();
        take(&mut hub, OP_A);
        let (out, replaced) = hub.robot_connected(102, R1, "r1".into(), None);
        assert_eq!(replaced, Some(ROBOT_CONN));
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::End { .. }]));
        // The stale connection closing later must not take the new one offline.
        assert!(hub.disconnected(ROBOT_CONN).is_empty());
        assert_eq!(hub.online_robot_count(), 1);
    }

    #[test]
    fn robot_reconnect_resumes_its_session() {
        let mut hub = hub();
        let session = take(&mut hub, OP_A);
        let (out, replaced) = hub.robot_connected(102, R1, "r1".into(), Some(session));
        assert_eq!(replaced, Some(ROBOT_CONN));
        assert!(matches!(to(&out, 102)[..], [ServerMsg::Welcome { .. }, ServerMsg::Session { session: s, peer: Peer { id: 10, .. }, .. }] if *s == session));
        assert!(matches!(to(&out, OP_A)[..], [ServerMsg::Resumed { session: s }] if *s == session));
        assert!(out.iter().any(|(t, m)| *t == Target::AllOperators
            && matches!(m, ServerMsg::Robot { robot } if robot.online && robot.operator == Some(10))));
        assert!(hub.disconnected(ROBOT_CONN).is_empty());
        // The new connection carries the session: the restart offer reaches it.
        let out = hub.from_operator(OP_A, OperatorMsg::Offer { session, sdp: raw("\"o\"") });
        assert!(matches!(to(&out, 102)[..], [ServerMsg::Offer { .. }]));
    }

    #[test]
    fn robot_reconnect_with_a_stale_session_hears_end() {
        let mut hub = hub();
        let session = take(&mut hub, OP_A);
        // The server noticed the old connection die first: the session is over.
        hub.disconnected(ROBOT_CONN);
        let (out, _) = hub.robot_connected(102, R1, "r1".into(), Some(session));
        assert!(matches!(to(&out, 102)[..], [ServerMsg::Welcome { .. }, ServerMsg::End { reason: EndReason::RobotOffline, .. }]));
        assert!(to(&out, OP_A).is_empty());
        // Not the current session (e.g. the app restarted meanwhile): the current one ends as well.
        let session = take(&mut hub, OP_A);
        let (out, _) = hub.robot_connected(103, R1, "r1".into(), Some(session + 100));
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
        assert_eq!(out.len(), 1);
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
}
