//! SQLite storage: operators, robots, login sessions. Queries are tiny and run inline on the single
//! runtime thread (no blocking pool); WAL lets the CLI write while the service runs.
//!
//! Schema changes: append a step to [`MIGRATIONS`], never edit a released one.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::hub::{Access, EventIn, EventLevel, Fleet, FleetId, Location, RobotEvent, RobotId, RobotRecord, RobotStatus, UserId};

const MIGRATIONS: &[&str] = &[
    // 1: operators and admins, fleets, robots with their last known state, events, audit, login sessions.
    // Until the first release this step is edited in place (docs/06-roadmap.md "Compatibility").
    "CREATE TABLE fleets (
        id      INTEGER PRIMARY KEY,
        name    TEXT NOT NULL UNIQUE COLLATE NOCASE,
        created INTEGER NOT NULL
    );
    INSERT INTO fleets (id, name, created) VALUES (1, 'default', 0);
    CREATE TABLE users (
        id         INTEGER PRIMARY KEY,
        email      TEXT NOT NULL UNIQUE COLLATE NOCASE,
        name       TEXT NOT NULL,
        password   TEXT NOT NULL,
        role       TEXT NOT NULL DEFAULT 'operator' CHECK (role IN ('operator', 'admin')),
        all_fleets INTEGER NOT NULL DEFAULT 0,
        disabled   INTEGER NOT NULL DEFAULT 0,
        created    INTEGER NOT NULL
    );
    CREATE TABLE user_fleets (
        user_id  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        fleet_id INTEGER NOT NULL REFERENCES fleets(id) ON DELETE CASCADE,
        PRIMARY KEY (user_id, fleet_id)
    ) WITHOUT ROWID;
    CREATE TABLE robots (
        id            INTEGER PRIMARY KEY,
        name          TEXT NOT NULL UNIQUE,
        token_hash    BLOB NOT NULL,
        fleet_id      INTEGER NOT NULL DEFAULT 1 REFERENCES fleets(id),
        disabled      INTEGER NOT NULL DEFAULT 0,
        status        TEXT,
        location      TEXT,
        last_seen     INTEGER,
        unread_errors INTEGER NOT NULL DEFAULT 0,
        created       INTEGER NOT NULL
    );
    CREATE TABLE robot_events (
        id       INTEGER PRIMARY KEY,
        robot_id INTEGER NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
        level    TEXT NOT NULL,
        code     TEXT NOT NULL,
        message  TEXT NOT NULL,
        at       INTEGER NOT NULL,
        received INTEGER NOT NULL
    );
    CREATE INDEX robot_events_robot ON robot_events(robot_id, id);
    CREATE TABLE audit (
        id       INTEGER PRIMARY KEY,
        at       INTEGER NOT NULL,
        user_id  INTEGER REFERENCES users(id) ON DELETE SET NULL,
        fleet_id INTEGER,
        action   TEXT NOT NULL,
        target   TEXT NOT NULL,
        detail   TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE sessions (
        token_hash BLOB PRIMARY KEY,
        user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        expires    INTEGER NOT NULL
    ) WITHOUT ROWID;
    CREATE INDEX sessions_user ON sessions(user_id);",
];

/// Robot events kept per robot (ADM-004); older ones are trimmed on insert.
pub const EVENTS_PER_ROBOT: i64 = 500;

#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub name: String,
    pub role: Role,
    /// ADM-005: an admin sees every fleet (including ones created later)…
    pub all_fleets: bool,
    /// …or only these.
    pub fleets: Vec<FleetId>,
}

impl User {
    /// What the user may administer: `None` for operators.
    pub fn admin_access(&self) -> Option<Access> {
        match self.role {
            Role::Operator => None,
            Role::Admin if self.all_fleets => Some(Access::All),
            Role::Admin => Some(Access::Fleets(self.fleets.iter().copied().collect())),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Operator,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "operator" => Some(Role::Operator),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }
}

/// An entry of the admin action log (ADM-004).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub at: i64,
    pub user: Option<UserId>,
    pub user_name: Option<String>,
    pub fleet: Option<FleetId>,
    pub action: String,
    pub target: String,
    pub detail: String,
}

pub struct Db {
    conn: Connection,
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn user_row(r: &rusqlite::Row) -> rusqlite::Result<User> {
    let role: String = r.get(3)?;
    Ok(User {
        id: r.get(0)?,
        email: r.get(1)?,
        name: r.get(2)?,
        role: Role::parse(&role).unwrap_or(Role::Operator),
        all_fleets: r.get(4)?,
        fleets: Vec::new(),
    })
}

const USER_COLUMNS: &str = "u.id, u.email, u.name, u.role, u.all_fleets";

/// Stored JSON of the last known state; a row the current code cannot read is treated as unknown.
fn json_column<T: serde::de::DeserializeOwned>(text: Option<String>) -> Option<T> {
    text.and_then(|t| serde_json::from_str(&t).ok())
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 2000;",
        )?;
        let version: usize = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, step) in MIGRATIONS.iter().enumerate().skip(version) {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(step)?;
            tx.pragma_update(None, "user_version", i + 1)?;
            tx.commit()?;
        }
        Ok(Db { conn })
    }

    // --- users ---

    pub fn add_user(&self, email: &str, name: &str, password_hash: &str, role: Role) -> rusqlite::Result<UserId> {
        self.conn.execute(
            "INSERT INTO users (email, name, password, role, created) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![email, name, password_hash, role.as_str(), now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_password(&self, email: &str, password_hash: &str) -> rusqlite::Result<bool> {
        let n = self.conn.execute("UPDATE users SET password = ?2 WHERE email = ?1", params![email, password_hash])?;
        // Changing the password logs out every session.
        self.conn.execute("DELETE FROM sessions WHERE user_id = (SELECT id FROM users WHERE email = ?1)", [email])?;
        Ok(n == 1)
    }

    pub fn set_user_disabled(&self, email: &str, disabled: bool) -> rusqlite::Result<bool> {
        let n = self.conn.execute("UPDATE users SET disabled = ?2 WHERE email = ?1", params![email, disabled])?;
        if disabled {
            self.conn.execute("DELETE FROM sessions WHERE user_id = (SELECT id FROM users WHERE email = ?1)", [email])?;
        }
        Ok(n == 1)
    }

    pub fn set_role(&self, email: &str, role: Role) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("UPDATE users SET role = ?2 WHERE email = ?1", params![email, role.as_str()])? == 1)
    }

    /// ADM-005: `all` = every fleet, including later ones; otherwise exactly `fleets`.
    pub fn set_user_fleets(&self, email: &str, all: bool, fleets: &[FleetId]) -> rusqlite::Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let Some(id) = tx.query_row("SELECT id FROM users WHERE email = ?1", [email], |r| r.get::<_, UserId>(0)).optional()?
        else {
            return Ok(false);
        };
        tx.execute("UPDATE users SET all_fleets = ?2 WHERE id = ?1", params![id, all])?;
        tx.execute("DELETE FROM user_fleets WHERE user_id = ?1", [id])?;
        for fleet in fleets {
            tx.execute("INSERT OR IGNORE INTO user_fleets (user_id, fleet_id) VALUES (?1, ?2)", params![id, fleet])?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn with_fleets(&self, mut user: User) -> rusqlite::Result<User> {
        let mut stmt = self.conn.prepare_cached("SELECT fleet_id FROM user_fleets WHERE user_id = ?1 ORDER BY fleet_id")?;
        user.fleets = stmt.query_map([user.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(user)
    }

    /// Enabled user and their password hash.
    pub fn user_for_login(&self, email: &str) -> rusqlite::Result<Option<(User, String)>> {
        let found = self
            .conn
            .query_row(
                &format!("SELECT {USER_COLUMNS}, u.password FROM users u WHERE u.email = ?1 AND u.disabled = 0"),
                [email],
                |r| Ok((user_row(r)?, r.get(5)?)),
            )
            .optional()?;
        found.map(|(user, hash)| Ok((self.with_fleets(user)?, hash))).transpose()
    }

    /// `(user, disabled)`.
    pub fn user(&self, id: UserId) -> rusqlite::Result<Option<(User, bool)>> {
        let found = self
            .conn
            .query_row(&format!("SELECT {USER_COLUMNS}, u.disabled FROM users u WHERE u.id = ?1"), [id], |r| {
                Ok((user_row(r)?, r.get(5)?))
            })
            .optional()?;
        found.map(|(user, disabled)| Ok((self.with_fleets(user)?, disabled))).transpose()
    }

    /// `(user, disabled)`.
    pub fn users(&self) -> rusqlite::Result<Vec<(User, bool)>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {USER_COLUMNS}, u.disabled FROM users u ORDER BY u.id"))?;
        let rows = stmt.query_map([], |r| Ok((user_row(r)?, r.get(5)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|(user, disabled)| Ok((self.with_fleets(user)?, disabled))).collect()
    }

    // --- login sessions ---

    pub fn add_session(&self, token_hash: &[u8], user: UserId, expires: i64) -> rusqlite::Result<()> {
        // Housekeeping piggybacks on logins: no timers.
        self.conn.execute("DELETE FROM sessions WHERE expires < ?1", [now()])?;
        self.conn.execute(
            "INSERT INTO sessions (token_hash, user_id, expires) VALUES (?1, ?2, ?3)",
            params![token_hash, user, expires],
        )?;
        Ok(())
    }

    pub fn session_user(&self, token_hash: &[u8]) -> rusqlite::Result<Option<User>> {
        let found = self
            .conn
            .query_row(
                &format!(
                    "SELECT {USER_COLUMNS} FROM sessions s JOIN users u ON u.id = s.user_id
                     WHERE s.token_hash = ?1 AND s.expires > ?2 AND u.disabled = 0"
                ),
                params![token_hash, now()],
                user_row,
            )
            .optional()?;
        found.map(|user| self.with_fleets(user)).transpose()
    }

    pub fn delete_session(&self, token_hash: &[u8]) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])?;
        Ok(())
    }

    // --- fleets ---

    pub fn add_fleet(&self, name: &str) -> rusqlite::Result<FleetId> {
        self.conn.execute("INSERT INTO fleets (name, created) VALUES (?1, ?2)", params![name, now()])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn fleets(&self) -> rusqlite::Result<Vec<Fleet>> {
        let mut stmt = self.conn.prepare("SELECT id, name FROM fleets ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok(Fleet { id: r.get(0)?, name: r.get(1)? }))?;
        rows.collect()
    }

    /// A fleet by id or name (CLI arguments).
    pub fn find_fleet(&self, key: &str) -> rusqlite::Result<Option<FleetId>> {
        self.conn
            .query_row("SELECT id FROM fleets WHERE id = ?1 OR name = ?1 ORDER BY id = ?1 DESC LIMIT 1", [key], |r| {
                r.get(0)
            })
            .optional()
    }

    // --- robots ---

    pub fn add_robot(&self, name: &str, token_hash: &[u8], fleet: FleetId) -> rusqlite::Result<RobotId> {
        self.conn.execute(
            "INSERT INTO robots (name, token_hash, fleet_id, created) VALUES (?1, ?2, ?3, ?4)",
            params![name, token_hash, fleet, now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_robot_token(&self, id: RobotId, token_hash: &[u8]) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("UPDATE robots SET token_hash = ?2 WHERE id = ?1", params![id, token_hash])? == 1)
    }

    pub fn set_robot_disabled(&self, id: RobotId, disabled: bool) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("UPDATE robots SET disabled = ?2 WHERE id = ?1", params![id, disabled])? == 1)
    }

    pub fn rename_robot(&self, id: RobotId, name: &str) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("UPDATE robots SET name = ?2 WHERE id = ?1", params![id, name])? == 1)
    }

    pub fn set_robot_fleet(&self, id: RobotId, fleet: FleetId) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("UPDATE robots SET fleet_id = ?2 WHERE id = ?1", params![id, fleet])? == 1)
    }

    /// An enabled robot and its token hash.
    pub fn robot_for_login(&self, id: RobotId) -> rusqlite::Result<Option<(RobotRecord, Vec<u8>)>> {
        let found = self
            .conn
            .query_row(
                &format!("SELECT {ROBOT_COLUMNS}, token_hash FROM robots WHERE id = ?1 AND disabled = 0"),
                [id],
                |r| Ok((robot_row(r)?, r.get(9)?)),
            )
            .optional()?;
        Ok(found.map(|((record, status, location), hash)| (record.with_json(status, location), hash)))
    }

    pub fn robot(&self, id: RobotId) -> rusqlite::Result<Option<RobotRecord>> {
        let found = self
            .conn
            .query_row(&format!("SELECT {ROBOT_COLUMNS} FROM robots WHERE id = ?1"), [id], robot_row)
            .optional()?;
        Ok(found.map(|(record, status, location)| record.with_json(status, location)))
    }

    pub fn robots(&self) -> rusqlite::Result<Vec<RobotRecord>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {ROBOT_COLUMNS} FROM robots ORDER BY id"))?;
        let rows = stmt.query_map([], robot_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().map(|(record, status, location)| record.with_json(status, location)).collect())
    }

    /// ADM-004: the last known state, written only when the robot sends something.
    pub fn save_status(&self, id: RobotId, status: &RobotStatus, at: i64) -> rusqlite::Result<()> {
        let json = serde_json::to_string(status).expect("status serializes");
        self.conn.execute("UPDATE robots SET status = ?2, last_seen = ?3 WHERE id = ?1", params![id, json, at])?;
        Ok(())
    }

    pub fn save_location(&self, id: RobotId, location: &Location, at: i64) -> rusqlite::Result<()> {
        let json = serde_json::to_string(location).expect("location serializes");
        self.conn.execute("UPDATE robots SET location = ?2, last_seen = ?3 WHERE id = ?1", params![id, json, at])?;
        Ok(())
    }

    pub fn touch_robot(&self, id: RobotId, at: i64) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE robots SET last_seen = ?2 WHERE id = ?1", params![id, at])?;
        Ok(())
    }

    /// Stores an event from the robot; errors count as unread until an admin acknowledges them.
    pub fn add_event(&self, robot: RobotId, event: &EventIn, received: i64) -> rusqlite::Result<RobotEvent> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO robot_events (robot_id, level, code, message, at, received) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![robot, event.level.as_str(), event.code, event.message, event.at, received],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "DELETE FROM robot_events WHERE robot_id = ?1 AND id <= (SELECT id FROM robot_events WHERE robot_id = ?1
             ORDER BY id DESC LIMIT 1 OFFSET ?2)",
            params![robot, EVENTS_PER_ROBOT],
        )?;
        if event.level == EventLevel::Error {
            tx.execute("UPDATE robots SET unread_errors = unread_errors + 1 WHERE id = ?1", [robot])?;
        }
        tx.execute("UPDATE robots SET last_seen = ?2 WHERE id = ?1", params![robot, received])?;
        tx.commit()?;
        Ok(RobotEvent {
            id,
            level: event.level,
            code: event.code.clone(),
            message: event.message.clone(),
            at: event.at,
            received,
        })
    }

    /// Newest first, older than `before` (an event id) when given.
    pub fn events(&self, robot: RobotId, before: Option<i64>, limit: i64) -> rusqlite::Result<Vec<RobotEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, level, code, message, at, received FROM robot_events
             WHERE robot_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![robot, before.unwrap_or(i64::MAX), limit], |r| {
            let level: String = r.get(1)?;
            Ok(RobotEvent {
                id: r.get(0)?,
                level: EventLevel::parse(&level),
                code: r.get(2)?,
                message: r.get(3)?,
                at: r.get(4)?,
                received: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn ack_errors(&self, robot: RobotId) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE robots SET unread_errors = 0 WHERE id = ?1", [robot])?;
        Ok(())
    }

    // --- audit ---

    pub fn audit(&self, user: UserId, fleet: Option<FleetId>, action: &str, target: &str, detail: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO audit (at, user_id, fleet_id, action, target, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![now(), user, fleet, action, target, detail],
        )?;
        Ok(())
    }

    /// Newest first. `fleets`: only entries about these fleets (`None` = everything, for all-fleet admins).
    pub fn audit_log(&self, fleets: Option<&[FleetId]>, before: Option<i64>, limit: i64) -> rusqlite::Result<Vec<AuditEntry>> {
        let filter = match fleets {
            None => String::new(),
            Some(ids) => {
                let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
                format!("AND a.fleet_id IN ({})", ids.join(","))
            }
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT a.id, a.at, a.user_id, u.name, a.fleet_id, a.action, a.target, a.detail
             FROM audit a LEFT JOIN users u ON u.id = a.user_id
             WHERE a.id < ?1 {filter} ORDER BY a.id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![before.unwrap_or(i64::MAX), limit], |r| {
            Ok(AuditEntry {
                id: r.get(0)?,
                at: r.get(1)?,
                user: r.get(2)?,
                user_name: r.get(3)?,
                fleet: r.get(4)?,
                action: r.get(5)?,
                target: r.get(6)?,
                detail: r.get(7)?,
            })
        })?;
        rows.collect()
    }
}

const ROBOT_COLUMNS: &str = "id, name, fleet_id, disabled, status, location, last_seen, unread_errors, created";

type RobotRow = (RobotRecord, Option<String>, Option<String>);

fn robot_row(r: &rusqlite::Row) -> rusqlite::Result<RobotRow> {
    Ok((
        RobotRecord {
            id: r.get(0)?,
            name: r.get(1)?,
            fleet: r.get(2)?,
            disabled: r.get(3)?,
            status: RobotStatus::default(),
            location: None,
            last_seen: r.get(6)?,
            unread_errors: r.get(7)?,
        },
        r.get(4)?,
        r.get(5)?,
    ))
}

impl RobotRecord {
    fn with_json(mut self, status: Option<String>, location: Option<String>) -> Self {
        self.status = json_column(status).unwrap_or_default();
        self.location = json_column(location);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_and_sessions() {
        let db = Db::memory().unwrap();
        let id = db.add_user("A@x.org", "Alice", "hash", Role::Operator).unwrap();
        let (user, hash) = db.user_for_login("a@X.org").unwrap().unwrap();
        assert_eq!((user.id, hash.as_str(), user.role), (id, "hash", Role::Operator));
        assert_eq!(user.admin_access(), None);

        db.add_session(b"t1", id, now() + 60).unwrap();
        db.add_session(b"old", id, now() - 1).unwrap();
        assert_eq!(db.session_user(b"t1").unwrap().unwrap().name, "Alice");
        assert_eq!(db.session_user(b"old").unwrap(), None);

        db.set_user_disabled("a@x.org", true).unwrap();
        assert_eq!(db.session_user(b"t1").unwrap(), None);
        assert_eq!(db.user_for_login("a@x.org").unwrap(), None);
    }

    #[test]
    fn password_change_logs_out() {
        let db = Db::memory().unwrap();
        let id = db.add_user("a@x.org", "A", "h1", Role::Operator).unwrap();
        db.add_session(b"t", id, now() + 60).unwrap();
        assert!(db.set_password("a@x.org", "h2").unwrap());
        assert_eq!(db.session_user(b"t").unwrap(), None);
    }

    #[test]
    fn admins_and_fleets() {
        let db = Db::memory().unwrap();
        let north = db.add_fleet("north").unwrap();
        assert_eq!(db.find_fleet("North").unwrap(), Some(north));
        assert_eq!(db.find_fleet("1").unwrap(), Some(1));
        assert_eq!(db.find_fleet("nope").unwrap(), None);
        assert!(db.add_fleet("NORTH").is_err());

        let id = db.add_user("a@x.org", "A", "h", Role::Admin).unwrap();
        db.add_session(b"t", id, now() + 60).unwrap();
        assert_eq!(db.session_user(b"t").unwrap().unwrap().admin_access(), Some(Access::Fleets(Default::default())));
        assert!(db.set_user_fleets("a@x.org", false, &[1, north]).unwrap());
        let user = db.session_user(b"t").unwrap().unwrap();
        assert_eq!(user.admin_access(), Some(Access::Fleets([1, north].into())));
        assert!(db.set_user_fleets("a@x.org", true, &[]).unwrap());
        assert_eq!(db.user(id).unwrap().unwrap().0.admin_access(), Some(Access::All));
        assert!(db.set_role("a@x.org", Role::Operator).unwrap());
        assert_eq!(db.users().unwrap()[0].0.admin_access(), None);
        assert!(!db.set_user_fleets("b@x.org", true, &[]).unwrap());
        assert!(db.add_user("b@x.org", "B", "h", Role::Admin).is_ok());
        assert!(db.conn.execute("UPDATE users SET role = 'root' WHERE id = ?1", [id]).is_err());
    }

    #[test]
    fn robots() {
        let db = Db::memory().unwrap();
        let id = db.add_robot("r1", b"h", 1).unwrap();
        let (record, hash) = db.robot_for_login(id).unwrap().unwrap();
        assert_eq!((record.name.as_str(), record.fleet, hash), ("r1", 1, b"h".to_vec()));
        db.set_robot_disabled(id, true).unwrap();
        assert_eq!(db.robot_for_login(id).unwrap(), None);
        assert!(db.robots().unwrap()[0].disabled);
        assert!(db.set_robot_fleet(id, 99).is_err(), "unknown fleet");
        assert!(db.rename_robot(id, "r2").unwrap());
        assert_eq!(db.robot(id).unwrap().unwrap().name, "r2");
    }

    #[test]
    fn last_known_state() {
        let db = Db::memory().unwrap();
        let id = db.add_robot("r1", b"h", 1).unwrap();
        let status = RobotStatus { phone_battery: Some(40), ..Default::default() };
        db.save_status(id, &status, 100).unwrap();
        db.save_location(id, &Location { lat: 50.45, lon: 30.52, accuracy_m: Some(8.0), at: 90 }, 120).unwrap();
        let r = db.robot(id).unwrap().unwrap();
        assert_eq!((r.status, r.last_seen), (status, Some(120)));
        assert_eq!(r.location.unwrap().lat, 50.45);
        // A row from an older format is ignored, not an error.
        db.conn.execute("UPDATE robots SET status = '{\"mode\":\"FLYING\"}'", []).unwrap();
        assert_eq!(db.robot(id).unwrap().unwrap().status, RobotStatus::default());
    }

    #[test]
    fn events_are_trimmed_and_count_errors() {
        let db = Db::memory().unwrap();
        let id = db.add_robot("r1", b"h", 1).unwrap();
        let event = |level, n: i64| EventIn { level, code: "c".into(), message: format!("m{n}"), at: n };
        for n in 0..EVENTS_PER_ROBOT + 10 {
            db.add_event(id, &event(EventLevel::Info, n), n).unwrap();
        }
        let e = db.add_event(id, &event(EventLevel::Error, 999), 999).unwrap();
        let count: i64 = db.conn.query_row("SELECT count(*) FROM robot_events", [], |r| r.get(0)).unwrap();
        assert_eq!(count, EVENTS_PER_ROBOT);
        let page = db.events(id, None, 3).unwrap();
        assert_eq!(page[0], e);
        let older = db.events(id, Some(page[2].id), 2).unwrap();
        assert_eq!(older[0].message, format!("m{}", EVENTS_PER_ROBOT + 7));
        assert_eq!(db.robot(id).unwrap().unwrap().unread_errors, 1);
        db.ack_errors(id).unwrap();
        assert_eq!(db.robot(id).unwrap().unwrap().unread_errors, 0);
    }

    #[test]
    fn audit_is_filtered_by_fleet() {
        let db = Db::memory().unwrap();
        let admin = db.add_user("a@x.org", "A", "h", Role::Admin).unwrap();
        let north = db.add_fleet("north").unwrap();
        db.audit(admin, Some(1), "disable", "robot 1", "").unwrap();
        db.audit(admin, Some(north), "rename", "robot 2", "x").unwrap();
        db.audit(admin, None, "role", "user 3", "admin").unwrap();
        assert_eq!(db.audit_log(None, None, 10).unwrap().len(), 3);
        let mine = db.audit_log(Some(&[north]), None, 10).unwrap();
        assert_eq!((mine.len(), mine[0].user_name.as_deref()), (1, Some("A")));
        assert!(db.audit_log(Some(&[]), None, 10).unwrap().is_empty());
    }
}
