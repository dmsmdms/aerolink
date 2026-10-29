//! AeroLink backend: operators, robot registry and WebRTC signaling (docs/08-backend.md).
//! Media and control go peer-to-peer between the console and the robot, never through this process.
//!
//! One process, one thread (tokio current-thread runtime): the load is a few WebSocket messages per
//! session; nginx does TLS, static files, compression and abuse filtering.

mod admin;
mod auth;
mod config;
mod db;
mod guard;
mod hub;
mod logging;
mod release;
mod server;
mod telegram;

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use config::Config;
use db::{Db, Role};

const USAGE: &str = "\
usage: aerolink <command> [args]

Commands:
  serve                 run the signaling service (default; systemd starts it)
  operator …            manage console users (operators and admins)
  robot …               manage robots and their tokens
  fleet …               manage fleets (groups of robots admins are responsible for)
  guard …               firewall bans from nginx's security log
  telegram-test         send a test message with the configured bot
  completion bash       print the bash completion script
  version               print the version
  help [command]        this text, or the details of a command (also: aerolink <command> --help)

Configuration: /etc/aerolink/aerolink.env (AEROLINK_CONFIG), see docs/08-backend.md.
Run management commands as the service user: sudo -u aerolink aerolink …";

const HELP_SERVE: &str = "\
usage: aerolink [serve]

Runs the backend: operators' console API and WebSocket signaling for robots and consoles. Listens on the
socket systemd passes (aerolink.socket), or on AEROLINK_LISTEN (unix:/path or host:port). Errors go to
AEROLINK_ERROR_LOG and Telegram. SIGHUP (systemctl reload aerolink) re-reads the robot app release in
AEROLINK_APK_DIR and announces it to the robots.";

const HELP_OPERATOR: &str = "\
usage: aerolink operator add <email> <name>     add an operator (password from stdin, at least 10 characters)
       aerolink operator passwd <email>         set a new password (from stdin); logs out every session
       aerolink operator disable <email>        block logins; logs out every session
       aerolink operator enable <email>
       aerolink operator role <email> admin|operator
       aerolink operator fleets <email> <fleet,…|all>
                                                fleets an admin sees and manages (ids or names); `all` also
                                                covers fleets created later and allows managing users
       aerolink operator list                   id, e-mail, name, role, fleets, disabled

Passwords are read from stdin without echo on a terminal, or as one line from a pipe. Role and fleet changes
apply to the user's next console connection (reload the page); the console's admin pages apply them at once.";

const HELP_ROBOT: &str = "\
usage: aerolink robot add <name> [fleet]        add a robot (to the default fleet unless given); prints its token
                                                once (enter it in the robot app)
       aerolink robot token <id>                issue a new token; the old one stops working
       aerolink robot disable <id>              reject the robot's connections (a connected robot keeps its
                                                connection until `systemctl restart aerolink`)
       aerolink robot enable <id>
       aerolink robot fleet <id> <fleet>        move the robot to another fleet (id or name)
       aerolink robot list                      id, name, fleet, disabled";

const HELP_FLEET: &str = "\
usage: aerolink fleet add <name>                add a fleet
       aerolink fleet list                      id, name

Every robot belongs to one fleet (fleet 1 `default` exists from the start); admins see the fleets given to
them with `aerolink operator fleets`.";

const HELP_GUARD: &str = "\
usage: aerolink guard [serve]                   receive nginx's security log and ban abusive addresses with
                                                nftables (aerolink-guard.service)
       aerolink guard list                      current bans with the time left (as root)
       aerolink guard unban <ip>                lift a ban (as root)

Never banned: loopback and AEROLINK_GUARD_ALLOW. See docs/09-deployment.md (OPS-013).";

const HELP_TELEGRAM: &str = "\
usage: aerolink telegram-test

Sends a test message with AEROLINK_TELEGRAM_TOKEN to AEROLINK_TELEGRAM_CHAT_ID.";

const HELP_COMPLETION: &str = "\
usage: aerolink completion bash

Prints the bash completion script. The Debian package installs it already
(/usr/share/bash-completion/completions/aerolink); elsewhere: source <(aerolink completion bash)";

const COMPLETION_BASH: &str = include_str!("../package/usr/share/bash-completion/completions/aerolink");

/// The details of a command, for `aerolink help <command>` and `aerolink <command> --help`.
fn help(command: &str) -> Option<&'static str> {
    Some(match command {
        "serve" => HELP_SERVE,
        "operator" => HELP_OPERATOR,
        "robot" => HELP_ROBOT,
        "fleet" => HELP_FLEET,
        "guard" => HELP_GUARD,
        "telegram-test" => HELP_TELEGRAM,
        "completion" => HELP_COMPLETION,
        "version" => "usage: aerolink version",
        "help" => USAGE,
        _ => return None,
    })
}

fn is_help(arg: &str) -> bool {
    matches!(arg, "help" | "--help" | "-h")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // Commands that need no configuration: they work for any user (the config file is 0640 root:aerolink).
    let early = match args.as_slice() {
        [h] if is_help(h) => Some(Ok(USAGE.to_owned())),
        [h, command] if is_help(h) => Some(help(command).map(str::to_owned).ok_or_else(|| USAGE.to_owned())),
        [command, .., h] if *h != "help" && is_help(h) => Some(help(command).map(str::to_owned).ok_or_else(|| USAGE.to_owned())),
        ["version" | "--version"] => Some(Ok(format!("aerolink {}", env!("CARGO_PKG_VERSION")))),
        ["completion", "bash"] => Some(Ok(COMPLETION_BASH.trim_end().to_owned())),
        ["completion", ..] => Some(Err(HELP_COMPLETION.to_owned())),
        _ => None,
    };
    if let Some(result) = early {
        return finish(result.map(|text| println!("{text}")));
    }
    let cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("config: {e}");
            return ExitCode::FAILURE;
        }
    };
    finish(match args.as_slice() {
        [] | ["serve"] => serve(cfg),
        ["telegram-test"] => telegram_test(&cfg),
        ["guard"] | ["guard", "serve"] => guard_serve(cfg),
        ["guard", "list"] => guard::Firewall::new(&cfg.guard_nft).list().map(|text| print!("{text}")),
        ["guard", "unban", ip] => guard_unban(&cfg, ip),
        ["operator" | "robot" | "fleet", ..] => manage(&cfg, &args),
        [command, ..] => Err(help(command).unwrap_or(USAGE).to_owned()),
    })
}

fn finish(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        // Only DNS lookups of the Telegram client end up here.
        .max_blocking_threads(1)
        .build()
        .expect("tokio runtime")
}

fn serve(cfg: Config) -> Result<(), String> {
    with_logging(&cfg, |cfg| async move {
        if cfg.origin.is_empty() {
            tracing::warn!("AEROLINK_ORIGIN is empty: operator WebSocket Origin is not checked (development only)");
        }
        run(cfg).await
    })
}

fn guard_serve(cfg: Config) -> Result<(), String> {
    guard::allow_list(&cfg)?;
    with_logging(&cfg, guard::run)
}

/// Sets up logging (stderr, error log, Telegram) and runs `main` on the single-threaded runtime. Errors
/// queued for Telegram are flushed before returning, including those of a failed start.
fn with_logging<F, Fut>(cfg: &Config, main: F) -> Result<(), String>
where
    F: FnOnce(Config) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let (telegram, tx) = if cfg.telegram_token.is_empty() || cfg.telegram_chat_id.is_empty() {
        (None, None)
    } else {
        let tg = telegram::Telegram::new(&cfg.telegram_token, &cfg.telegram_chat_id).map_err(|e| e.to_string())?;
        let (tx, rx) = tokio::sync::mpsc::channel(telegram::QUEUE);
        (Some((tg, rx)), Some(tx))
    };
    logging::init(&cfg.log_filter, cfg.error_log.clone(), tx);
    if telegram.is_none() {
        tracing::warn!("Telegram is not configured: errors go to {} only", cfg.error_log.display());
    }

    let cfg = cfg.clone();
    runtime().block_on(async move {
        let telegram = telegram.map(|(tg, rx)| {
            let (stop, stopped) = tokio::sync::oneshot::channel();
            (stop, tokio::spawn(tg.run(rx, stopped)))
        });
        let result = main(cfg).await;
        if let Some((stop, task)) = telegram {
            let _ = stop.send(());
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), task).await;
        }
        result
    })
}

async fn run(cfg: Config) -> Result<(), String> {
    let db = Db::open(&cfg.database).map_err(|e| {
        tracing::error!("database {}: {e}", cfg.database.display());
        e.to_string()
    })?;
    let listen = cfg.listen.clone();
    let app = server::App::new(cfg, db).map_err(|e| e.to_string())?;
    tracing::info!("aerolink {} started", env!("CARGO_PKG_VERSION"));
    server::serve(app, &listen).await.map_err(|e| {
        tracing::error!("listen {listen}: {e}");
        e.to_string()
    })
}

fn guard_unban(cfg: &Config, ip: &str) -> Result<(), String> {
    let net = guard::Net::of(ip.parse().map_err(|_| format!("bad address: {ip}"))?);
    guard::Firewall::new(&cfg.guard_nft).unban(net)?;
    println!("{net} unbanned");
    Ok(())
}

fn telegram_test(cfg: &Config) -> Result<(), String> {
    if cfg.telegram_token.is_empty() || cfg.telegram_chat_id.is_empty() {
        return Err("AEROLINK_TELEGRAM_TOKEN and AEROLINK_TELEGRAM_CHAT_ID are not set".into());
    }
    let tg = telegram::Telegram::new(&cfg.telegram_token, &cfg.telegram_chat_id).map_err(|e| e.to_string())?;
    runtime().block_on(tg.send_test())?;
    println!("sent");
    Ok(())
}

fn manage(cfg: &Config, args: &[&str]) -> Result<(), String> {
    let db = Db::open(&cfg.database).map_err(|e| format!("{}: {e}", cfg.database.display()))?;
    let found = |ok: bool| if ok { Ok(()) } else { Err("not found".to_owned()) };
    let id = |s: &str| s.parse::<hub::RobotId>().map_err(|_| format!("bad robot id: {s}"));
    let sql = |e: rusqlite::Error| e.to_string();
    let fleet = |key: &str| db.find_fleet(key).map_err(sql)?.ok_or_else(|| format!("no such fleet: {key}"));
    match args {
        ["operator", "add", email, name] => {
            let hash = auth::hash_password(&read_password()?);
            let id = db.add_user(email, name, &hash, Role::Operator).map_err(sql)?;
            println!("operator {id} added");
            Ok(())
        }
        ["operator", "passwd", email] => found(db.set_password(email, &auth::hash_password(&read_password()?)).map_err(sql)?),
        ["operator", "disable", email] => found(db.set_user_disabled(email, true).map_err(sql)?),
        ["operator", "enable", email] => found(db.set_user_disabled(email, false).map_err(sql)?),
        ["operator", "role", email, role] => {
            let role = Role::parse(role).ok_or_else(|| format!("bad role: {role} (admin or operator)"))?;
            found(db.set_role(email, role).map_err(sql)?)
        }
        ["operator", "fleets", email, "all"] => found(db.set_user_fleets(email, true, &[]).map_err(sql)?),
        ["operator", "fleets", email, list] => {
            let fleets = list.split(',').filter(|f| !f.is_empty()).map(fleet).collect::<Result<Vec<_>, _>>()?;
            found(db.set_user_fleets(email, false, &fleets).map_err(sql)?)
        }
        ["operator", "list"] => {
            for (u, disabled) in db.users().map_err(sql)? {
                let fleets = match (u.role, u.all_fleets) {
                    (Role::Operator, _) => "-".to_owned(),
                    (Role::Admin, true) => "all".to_owned(),
                    (Role::Admin, false) => u.fleets.iter().map(|f| f.to_string()).collect::<Vec<_>>().join(","),
                };
                let disabled = if disabled { "\tdisabled" } else { "" };
                println!("{}\t{}\t{}\t{}\t{fleets}{disabled}", u.id, u.email, u.name, u.role.as_str());
            }
            Ok(())
        }
        ["robot", "add", name, rest @ ..] if rest.len() <= 1 => {
            let fleet = rest.first().map(|f| fleet(f)).transpose()?.unwrap_or(1);
            let secret = auth::new_token();
            let id = db.add_robot(name, &auth::token_hash(&secret), fleet).map_err(sql)?;
            println!("robot {id} added; token (shown once, enter it in the robot app):");
            println!("{}", auth::robot_token(id, &secret));
            restart_hint();
            Ok(())
        }
        ["robot", "token", robot] => {
            let robot = id(robot)?;
            let secret = auth::new_token();
            found(db.set_robot_token(robot, &auth::token_hash(&secret)).map_err(sql)?)?;
            println!("{}", auth::robot_token(robot, &secret));
            Ok(())
        }
        ["robot", "disable", robot] => {
            found(db.set_robot_disabled(id(robot)?, true).map_err(sql)?)?;
            restart_hint();
            Ok(())
        }
        ["robot", "enable", robot] => found(db.set_robot_disabled(id(robot)?, false).map_err(sql)?),
        ["robot", "fleet", robot, to] => {
            let to = fleet(to)?;
            found(db.set_robot_fleet(id(robot)?, to).map_err(sql)?)?;
            restart_hint();
            Ok(())
        }
        ["robot", "list"] => {
            for r in db.robots().map_err(sql)? {
                println!("{}\t{}\t{}{}", r.id, r.name, r.fleet, if r.disabled { "\tdisabled" } else { "" });
            }
            Ok(())
        }
        ["fleet", "add", name] => {
            let id = db.add_fleet(name).map_err(sql)?;
            println!("fleet {id} added");
            Ok(())
        }
        ["fleet", "list"] => {
            for f in db.fleets().map_err(sql)? {
                println!("{}\t{}", f.id, f.name);
            }
            Ok(())
        }
        [command, ..] => Err(help(command).unwrap_or(USAGE).to_owned()),
        [] => Err(USAGE.to_owned()),
    }
}

/// The running service learns about CLI changes when the robot connects again; the console's admin pages
/// apply them at once.
fn restart_hint() {
    eprintln!("note: a connected robot keeps its connection until it reconnects or `systemctl restart aerolink`");
}

/// One line from stdin; on a terminal the echo is switched off with `stty`.
fn read_password() -> Result<String, String> {
    let tty = std::io::stdin().is_terminal();
    let stty = |arg: &str| {
        let _ = std::process::Command::new("stty").arg(arg).stdin(std::process::Stdio::inherit()).status();
    };
    if tty {
        eprint!("password: ");
        let _ = std::io::stderr().flush();
        stty("-echo");
    }
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if tty {
        stty("echo");
        eprintln!();
    }
    read.map_err(|e| e.to_string())?;
    let password = line.trim_end_matches(['\r', '\n']).to_owned();
    if password.chars().count() < auth::MIN_PASSWORD {
        return Err(format!("password must be at least {} characters", auth::MIN_PASSWORD));
    }
    Ok(password)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_completed_command_has_help() {
        let line = COMPLETION_BASH.lines().find_map(|l| l.trim().strip_prefix("local commands=\"")).unwrap();
        for command in line.trim_end_matches('"').split_whitespace() {
            assert!(help(command).is_some(), "{command}");
            assert!(USAGE.contains(&format!("\n  {command}")), "{command}");
        }
    }
}
