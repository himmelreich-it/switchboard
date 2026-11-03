//! switchboard: the app's server, the Rust port of server.ts and the files it imports (see PORTING.md). This file is
//! server.ts's startup and shutdown.

mod acp;
mod agents;
mod apptools;
mod db;
mod helpers;
mod http;
mod runner;
mod server;
mod server_ops;
mod terms;
mod util;

use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use util::{home, join, J};

static STOPPING: AtomicBool = AtomicBool::new(false);

#[tokio::main]
async fn main() {
    let db_path = join(home(), "db.sqlite");
    let fresh = !util::exists(&db_path);
    if let Err(e) = db::open(&db_path) {
        eprintln!("switchboard: cannot open {db_path}: {e}");
        std::process::exit(1);
    }
    if let Err(e) = helpers::migrate_db(&db::conn()) {
        eprintln!("switchboard: cannot migrate {db_path}: {e}");
        std::process::exit(1);
    }
    if fresh {
        server_ops::migrate();
    }

    // A server already running on the port wins: this copy exits, and a window attaches to the live one.
    let listener = match tokio::net::TcpListener::bind((util::HOST, util::port())).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("switchboard: cannot listen on {}:{}: {e}", util::HOST, util::port());
            std::process::exit(1);
        }
    };
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let app = http::router();
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            })
            .await;
    });

    // Written only once the port is bound: a second copy that exits on EADDRINUSE never replaces the live server's token.
    let token_file = join(home(), "token");
    write_token(&token_file, http::token());
    println!("switchboard on http://{}:{}/#token={} (data in {})", util::HOST, util::port(), http::token(), home());

    // After the bind, like the reset below: a second copy that exits on EADDRINUSE must not move a live app's folders.
    server::move_artifact_folders();

    // A run cannot survive a restart. Queued messages stay queued until the user presses Deliver.
    // Only after the bind: a second copy that fails with EADDRINUSE must not touch the live app's TODOs.
    // Routing that ran or asked at the crash starts over at the next Start (801).
    for t in db::all("SELECT id FROM todos WHERE routing IS NOT NULL", params![]) {
        let id = t.i("id").unwrap_or(0);
        runner::cancel_routing(id);
        server::add_event(Some(id), "app restarted", "app", None, None);
    }
    for t in db::all("SELECT id, attention, attention_by, permission FROM todos WHERE status = 'running'", params![]) {
        let id = t.i("id").unwrap_or(0);
        // A permission request open at the crash died with its agent; the flag it raised drops with it.
        let attention = if t.truthy("permission") && t.s("attention_by") == Some("permission") { 0 } else { t.i("attention").unwrap_or(0) };
        server::set_todo(
            id,
            json!({
                "status": helpers::status_after_run(attention != 0), "attention": attention,
                "attention_by": if attention != 0 { t["attention_by"].clone() } else { serde_json::Value::Null }, "permission": null,
            }),
            true,
        );
        server::add_event(Some(id), "app restarted", "app", None, None);
    }
    util::spawn(async {
        loop {
            server_ops::refresh_all_prs().await;
            tokio::time::sleep(Duration::from_secs(120)).await;
        }
    });
    // Item 144: one catch-up run per scheduled TODO whose slot passed while the app was closed, after the reset above and
    // the bind (a second copy must not fire the live app's schedules); then the slots every 30 seconds.
    runner::fire_due(true);
    util::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            runner::fire_due(false);
        }
    });
    terms::start_reaper();

    // Ctrl-C, SIGTERM, or (started by electron/main.js) stdin closing when the window app quits or crashes.
    let parent = std::env::var("SWITCHBOARD_PARENT").is_ok();
    util::spawn(async move {
        let stdin_end = async {
            if parent {
                let _ = tokio::io::copy(&mut tokio::io::stdin(), &mut tokio::io::sink()).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        #[cfg(unix)]
        let term = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut s) => {
                    s.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        };
        #[cfg(not(unix))]
        let term = std::future::pending::<()>();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term => {}
            _ = stdin_end => {}
        }
        shutdown(stop_tx, token_file).await;
    });
    let _ = server.await;
    std::future::pending::<()>().await;
}

/// Interrupts every running agent so none keeps working unseen; a second Ctrl-C exits at once.
async fn shutdown(stop: tokio::sync::oneshot::Sender<()>, token_file: String) {
    STOPPING.store(true, Ordering::SeqCst);
    util::spawn(async {
        let _ = tokio::signal::ctrl_c().await;
        std::process::exit(1);
    });
    println!("stopping: interrupting running agents");
    // The port frees at once, so a window reopened during the 5 s below starts a new server instead of reading this
    // server's token and attaching to a server that is about to exit.
    let _ = stop.send(());
    terms::close_terms(None);
    let _ = tokio::time::timeout(Duration::from_secs(5), runner::stop_all()).await;
    // A run that did not end in the 5 s still has its agent process. One second for SIGTERM and SIGHUP, then SIGKILL to
    // every process of the terminals and agents still there.
    runner::close_agents();
    tokio::time::sleep(Duration::from_secs(1)).await;
    acp::kill_now();
    terms::kill_pending(Duration::ZERO);
    if std::fs::read_to_string(&token_file).is_ok_and(|t| t == http::token()) {
        let _ = std::fs::remove_file(&token_file);
    }
    std::process::exit(0);
}

/// Whether the server is shutting down (a run that ends now is not reported as an error).
pub fn stopping() -> bool {
    STOPPING.load(Ordering::SeqCst)
}

fn write_token(path: &str, token: &str) {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path) {
            let _ = f.write_all(token.as_bytes());
        }
        // mode above applies only when the file is new
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = std::fs::write(path, token);
}
