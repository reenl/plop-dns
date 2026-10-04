use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    time::timeout,
};

struct TestChild(Child);

impl Drop for TestChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn request(listener: &UnixListener) -> (UnixStream, String) {
    let (mut stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        bytes.push(stream.read_u8().await.unwrap());
    }
    (stream, String::from_utf8(bytes).unwrap())
}

async fn reply(stream: &mut UnixStream, body: &str) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
}

async fn reply_snapshot(listener: &UnixListener, stream: &mut UnixStream) {
    reply(stream, "[]").await;
    let (mut networks, path) = request(listener).await;
    assert!(path.contains("/networks"));
    reply(&mut networks, "[]").await;
}

// Both scenarios run sequentially because the binary uses a fixed DNS port.
#[tokio::test]
async fn discovery_does_not_activate_or_poll_idle_docker() {
    let directory = std::env::temp_dir().join(format!("plop-dns-lazy-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let probe = directory.join("systemctl");
    fs::write(&probe, "#!/bin/sh\nexit 3\n").unwrap();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o755)).unwrap();
    let socket = directory.join("docker.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut child = TestChild(
        Command::new(env!("CARGO_BIN_EXE_plop-dns"))
            .env("DOCKER_HOST", format!("unix://{}", socket.display()))
            .env("PATH", format!("{}:/usr/bin:/bin", directory.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let contacted_socket = timeout(Duration::from_millis(3500), listener.accept())
        .await
        .is_ok();
    let exited = child.0.try_wait().unwrap().is_some();
    drop(child);

    fs::write(&probe, "#!/bin/sh\nexit 0\n").unwrap();
    let child = TestChild(
        Command::new(env!("CARGO_BIN_EXE_plop-dns"))
            .env("DOCKER_HOST", format!("unix://{}", socket.display()))
            .env("PATH", format!("{}:/usr/bin:/bin", directory.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (mut version, path) = request(&listener).await;
    assert!(path.starts_with("GET /version "));
    reply(&mut version, r#"{"ApiVersion":"1.53"}"#).await;
    let (mut snapshot, path) = request(&listener).await;
    assert!(path.contains("/containers/json"));
    reply_snapshot(&listener, &mut snapshot).await;
    let (mut events, path) = request(&listener).await;
    assert!(path.contains("/events?") && path.contains("since="));
    events.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
    let polled_idle = timeout(Duration::from_millis(3500), listener.accept())
        .await
        .is_ok();
    let event = "{\"Type\":\"container\",\"Action\":\"start\",\"Actor\":{\"ID\":\"fixture\"}}\n";
    let burst = event.repeat(20);
    events
        .write_all(format!("{:x}\r\n{burst}\r\n", burst.len()).as_bytes())
        .await
        .unwrap();
    let (mut refreshed, path) = request(&listener).await;
    reply_snapshot(&listener, &mut refreshed).await;
    assert!(
        timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_err(),
        "Queued events must share a single refresh"
    );
    // An abruptly closed stream must trigger a refresh and a new connection.
    drop(events);
    let (mut recovery, recovery_path) = request(&listener).await;
    assert!(recovery_path.contains("/containers/json"));
    reply_snapshot(&listener, &mut recovery).await;
    let (mut version, version_path) = request(&listener).await;
    assert!(version_path.starts_with("GET /version "));
    reply(&mut version, r#"{"ApiVersion":"1.53"}"#).await;
    let (mut snapshot, snapshot_path) = request(&listener).await;
    assert!(snapshot_path.contains("/containers/json"));
    reply_snapshot(&listener, &mut snapshot).await;
    let (_events, events_path) = request(&listener).await;
    assert!(events_path.contains("/events?"));
    drop(child);
    drop(listener);
    fs::remove_file(&socket).unwrap();
    fs::remove_file(&probe).unwrap();
    fs::remove_dir(&directory).unwrap();
    assert!(
        !exited,
        "Binary must stay running to exercise the discovery loops"
    );
    assert!(
        !contacted_socket,
        "Binary connected to Docker's activation socket while inactive"
    );
    assert!(
        !polled_idle,
        "Binary polled Docker while its event stream was idle"
    );
    assert!(
        path.contains("/containers/json"),
        "Container event must refresh records"
    );
}
