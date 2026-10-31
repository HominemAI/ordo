//! A stand-in for each child, for the supervisor tests. Standard library
//! only. It knows which child it plays from its environment: the console
//! (`ORDO_CONSOLE_LISTEN` is set), the identity service
//! (`ORDO_IDENTITY_LISTEN` is set), else the deployment. Its settings
//! carry that child's prefix, `P`: `ORDO_FAKE_` for the deployment,
//! `ORDO_CONSOLE_FAKE_`, `ORDO_IDENTITY_FAKE_`.
//!
//! - `<P>MODE`: `healthy` (the default) answers `/health/ready` 200;
//!   `unready` answers 503; `crash` exits 3 at once; `hang` never listens.
//! - `<P>CRASH_IF`: a file; while it exists, each start exits 6 at once.
//! - `<P>CHECK=refuse`: `check-config` refuses, as boot would.
//! - `<P>STARTS`: a file each start appends `<pid> <unix ms>` to.
//! - `<P>ENV_OUT`: a file the whole environment is written to, with the
//!   content of each `*_FILE` it names as `<NAME>_FILE_CONTENT`.
//!   `<P>ENV_PLAIN` set: the environment only, no secret's content.
//! - `<P>FORGET_OUT`: `forget-door-key` appends `USER=<user>` here and
//!   says `<P>FORGET_SAYS`; `<P>FORGET=fail` makes it fail.
//! - Where it listens: `ORDO_HOST_ADDR`, `ORDO_CONSOLE_LISTEN` or
//!   `ORDO_IDENTITY_LISTEN`.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn main() {
    let (prefix, listen, default) = if var("ORDO_CONSOLE_LISTEN").is_some() {
        (
            "ORDO_CONSOLE_FAKE_",
            "ORDO_CONSOLE_LISTEN",
            "127.0.0.1:8081",
        )
    } else if var("ORDO_IDENTITY_LISTEN").is_some() {
        (
            "ORDO_IDENTITY_FAKE_",
            "ORDO_IDENTITY_LISTEN",
            "127.0.0.1:8082",
        )
    } else {
        ("ORDO_FAKE_", "ORDO_HOST_ADDR", "127.0.0.1:8080")
    };
    let fake = |k: &str| var(&format!("{prefix}{k}"));
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("check-config") => {
            if fake("CHECK").as_deref() == Some("refuse") {
                eprintln!("{prefix}CHECK is `refuse`, which boot refuses. Unset it.");
                std::process::exit(1);
            }
            println!("settings ok");
            return;
        }
        Some("forget-door-key") => {
            if let Some(path) = fake("FORGET_OUT") {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = writeln!(f, "USER={}", var("USER").unwrap_or_default());
                }
            }
            if fake("FORGET").as_deref() == Some("fail") {
                eprintln!("the console database did not answer");
                std::process::exit(1);
            }
            println!(
                "{}",
                fake("FORGET_SAYS").unwrap_or_else(|| "no console key was held".into())
            );
            return;
        }
        _ => {}
    }
    if let Some(path) = fake("ENV_OUT") {
        let mut text = String::new();
        for (k, v) in std::env::vars() {
            text.push_str(&format!("{k}={v}\n"));
            // `<P>ENV_PLAIN`: the environment only, as a door would see it
            // (the console's end-to-end stack); no secret's content.
            if k.ends_with("_FILE") && fake("ENV_PLAIN").is_none() {
                if let Ok(content) = std::fs::read_to_string(&v) {
                    text.push_str(&format!("{k}_CONTENT={}\n", content.trim()));
                }
            }
        }
        let _ = std::fs::write(path, text);
    }
    if let Some(path) = fake("STARTS") {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{} {ms}", std::process::id());
        }
    }
    if let Some(path) = fake("CRASH_IF") {
        if std::path::Path::new(&path).exists() {
            eprintln!("fake child: {path} exists; crashing");
            std::process::exit(6);
        }
    }
    let mode = fake("MODE").unwrap_or_else(|| "healthy".into());
    println!("fake child {prefix}: starting in mode {mode}");
    match mode.as_str() {
        "crash" => {
            eprintln!("fake child: crashing");
            std::process::exit(3);
        }
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        _ => {}
    }
    let addr = var(listen).unwrap_or_else(|| default.into());
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("fake child: could not listen on {addr}: {e}");
            std::process::exit(4);
        }
    };
    let status = if mode == "unready" {
        "503 Service Unavailable"
    } else {
        "200 OK"
    };
    for mut stream in listener.incoming().flatten() {
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"
        );
    }
}
