//! SQLite storage: operators, robots, login sessions. Queries are tiny and run inline on the single
//! runtime thread (no blocking pool); WAL lets the CLI write while the service runs.
//!
//! Schema changes: append a step to [`MIGRATIONS`], never edit a released one.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::hub::{RobotId, UserId};

const MIGRATIONS: &[&str] = &[
    // 1: stage 2 — operators, robots, sessions. `role` and `disabled` leave room for registration and
    // robot rental (owners, fleets) without reshaping these tables.
    "CREATE TABLE users (
        id       INTEGER PRIMARY KEY,
        email    TEXT NOT NULL UNIQUE COLLATE NOCASE,
        name     TEXT NOT NULL,
        password TEXT NOT NULL,
        role     TEXT NOT NULL DEFAULT 'operator',
        disabled INTEGER NOT NULL DEFAULT 0,
        created  INTEGER NOT NULL
    );
    CREATE TABLE robots (
        id         INTEGER PRIMARY KEY,
        name       TEXT NOT NULL UNIQUE,
        token_hash BLOB NOT NULL,
        disabled   INTEGER NOT NULL DEFAULT 0,
        created    INTEGER NOT NULL
    );
    CREATE TABLE sessions (
        token_hash BLOB PRIMARY KEY,
        user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        expires    INTEGER NOT NULL
    ) WITHOUT ROWID;
    CREATE INDEX sessions_user ON sessions(user_id);",
];

#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub name: String,
    pub role: String,
}

pub struct Db {
    conn: Connection,
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
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

    pub fn add_user(&self, email: &str, name: &str, password_hash: &str) -> rusqlite::Result<UserId> {
        self.conn.execute(
            "INSERT INTO users (email, name, password, created) VALUES (?1, ?2, ?3, ?4)",
            params![email, name, password_hash, now()],
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

    /// Enabled user and their password hash.
    pub fn user_for_login(&self, email: &str) -> rusqlite::Result<Option<(User, String)>> {
        self.conn
            .query_row(
                "SELECT id, email, name, role, password FROM users WHERE email = ?1 AND disabled = 0",
                [email],
                |r| Ok((User { id: r.get(0)?, email: r.get(1)?, name: r.get(2)?, role: r.get(3)? }, r.get(4)?)),
            )
            .optional()
    }

    pub fn users(&self) -> rusqlite::Result<Vec<(User, bool)>> {
        let mut stmt = self.conn.prepare("SELECT id, email, name, role, disabled FROM users ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok((User { id: r.get(0)?, email: r.get(1)?, name: r.get(2)?, role: r.get(3)? }, r.get(4)?))
        })?;
        rows.collect()
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
        self.conn
            .query_row(
                "SELECT u.id, u.email, u.name, u.role FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires > ?2 AND u.disabled = 0",
                params![token_hash, now()],
                |r| Ok(User { id: r.get(0)?, email: r.get(1)?, name: r.get(2)?, role: r.get(3)? }),
            )
            .optional()
    }

    pub fn delete_session(&self, token_hash: &[u8]) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])?;
        Ok(())
    }

    // --- robots ---

    pub fn add_robot(&self, name: &str, token_hash: &[u8]) -> rusqlite::Result<RobotId> {
        self.conn.execute(
            "INSERT INTO robots (name, token_hash, created) VALUES (?1, ?2, ?3)",
            params![name, token_hash, now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_robot_token(&self, id: RobotId, token_hash: &[u8]) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("UPDATE robots SET token_hash = ?2 WHERE id = ?1", params![id, token_hash])? == 1)
    }

    pub fn set_robot_disabled(&self, id: RobotId, disabled: bool) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("UPDATE robots SET disabled = ?2 WHERE id = ?1", params![id, disabled])? == 1)
    }

    /// Name and token hash of an enabled robot.
    pub fn robot_for_login(&self, id: RobotId) -> rusqlite::Result<Option<(String, Vec<u8>)>> {
        self.conn
            .query_row("SELECT name, token_hash FROM robots WHERE id = ?1 AND disabled = 0", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
    }

    /// `(id, name, disabled)`.
    pub fn robots(&self) -> rusqlite::Result<Vec<(RobotId, String, bool)>> {
        let mut stmt = self.conn.prepare("SELECT id, name, disabled FROM robots ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_and_sessions() {
        let db = Db::memory().unwrap();
        let id = db.add_user("A@x.org", "Alice", "hash").unwrap();
        let (user, hash) = db.user_for_login("a@X.org").unwrap().unwrap();
        assert_eq!((user.id, hash.as_str(), user.role.as_str()), (id, "hash", "operator"));

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
        let id = db.add_user("a@x.org", "A", "h1").unwrap();
        db.add_session(b"t", id, now() + 60).unwrap();
        assert!(db.set_password("a@x.org", "h2").unwrap());
        assert_eq!(db.session_user(b"t").unwrap(), None);
    }

    #[test]
    fn robots() {
        let db = Db::memory().unwrap();
        let id = db.add_robot("r1", b"h").unwrap();
        assert_eq!(db.robot_for_login(id).unwrap(), Some(("r1".into(), b"h".to_vec())));
        db.set_robot_disabled(id, true).unwrap();
        assert_eq!(db.robot_for_login(id).unwrap(), None);
        assert_eq!(db.robots().unwrap(), vec![(id, "r1".to_owned(), true)]);
    }
}
