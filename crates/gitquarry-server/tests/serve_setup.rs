//! The first-run wizard as the real `gitquarry` binary reaches it: a config
//! that already exists and connects, over a bucket with no admin yet.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_for(http: &reqwest::Client, url: &str) -> reqwest::Response {
    for _ in 0..200 {
        if let Ok(resp) = http.get(url).send().await {
            return resp;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("nothing answered at {url}");
}

/// The one-time token the wizard logs with its URL — the only way to get it,
/// same as an operator. Keeps draining stderr afterwards so the server never
/// blocks on a full pipe.
fn setup_token(server: &mut Server) -> String {
    let stderr = server.0.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some((_, rest)) = line.split_once("/_setup?token=") {
                let _ = tx.send(rest.chars().take_while(char::is_ascii_hexdigit).collect::<String>());
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(30))
        .expect("the wizard never logged its setup URL")
}

/// A local redb bucket takes an exclusive lock, so the wizard can only open
/// it for its round trip once startup has let go of the handle it used to
/// see that no admin exists yet.
#[tokio::test]
async fn a_prefilled_wizard_over_a_local_redb_bucket_completes_and_hands_off() {
    let dir = tempfile::tempdir().unwrap();
    let bucket = dir.path().join("bucket");
    std::fs::create_dir(&bucket).unwrap();
    let config = dir.path().join("gitquarry.toml");
    std::fs::write(
        &config,
        format!(
            "[buckets.main]\ndriver = \"local\"\nengine = \"redb\"\nbucket = {:?}\npassphrase = \"hunter2\"\n",
            bucket.to_string_lossy()
        ),
    )
    .unwrap();
    let (web, ssh) = (free_port(), free_port());
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_gitquarry"))
            .args([
                "--web-bind",
                &format!("127.0.0.1:{web}"),
                "--ssh-bind",
                &format!("127.0.0.1:{ssh}"),
            ])
            .arg("--mirrors-dir")
            .arg(dir.path().join("mirrors"))
            .env("GITQUARRY_CONFIG", &config)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let token = setup_token(&mut server);
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{web}");
    let form = wait_for(&http, &format!("{base}/_setup?token={token}")).await;
    assert_eq!(form.status().as_u16(), 200);

    let bucket = bucket.to_string_lossy().to_string();
    let fields = [
        ("token", token.as_str()),
        ("name", "main"),
        ("driver", "local"),
        ("local_bucket", bucket.as_str()),
        ("admin_username", "admin"),
        ("admin_password", "pw"),
        ("admin_password_confirm", "pw"),
    ];
    let done = http.post(format!("{base}/_setup")).form(&fields).send().await.unwrap();
    let status = done.status().as_u16();
    let body = done.text().await.unwrap();
    assert_eq!(status, 200, "{body}");

    // The real server takes over the same port: the wizard route is gone
    // and the login page answers.
    for _ in 0..200 {
        if let Ok(resp) = http.get(format!("{base}/_login")).send().await
            && resp.status().as_u16() == 200
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the server never took over from the wizard");
}
