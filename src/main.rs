use anyhow::{anyhow, Context, Result};
use bollard::{query_parameters::EventsOptionsBuilder, Docker};
use futures_util::{FutureExt, StreamExt};
use hickory_proto::{
    op::{Message, MessageType, OpCode, ResponseCode},
    rr::{
        rdata::{A, AAAA, SOA},
        DNSClass, Name, RData, Record, RecordType,
    },
};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    process::Command,
    sync::{Mutex, RwLock, Semaphore},
    time::{sleep, timeout},
};

const ZONE: &str = "docker";
const TTL: u32 = 0;
const RETRY: Duration = Duration::from_secs(2);
const LISTEN: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5354);

#[derive(Default)]
struct Snapshot {
    ready: bool,
    records: HashMap<String, Vec<IpAddr>>,
}
#[derive(Debug)]
struct Entry {
    project: String,
    service: String,
    number: String,
    alias: Option<String>,
    global_alias: Option<String>,
    default: bool,
    addresses: Vec<IpAddr>,
}
struct Server {
    snapshot: RwLock<Snapshot>,
    rotation: Mutex<HashMap<(String, RecordType), usize>>,
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}
fn build_records(entries: Vec<Entry>) -> HashMap<String, Vec<IpAddr>> {
    let mut records = HashMap::<String, Vec<IpAddr>>::new();
    for entry in entries {
        if !valid_label(&entry.project)
            || !valid_label(&entry.service)
            || entry.number.parse::<u32>().map_or(true, |n| n == 0)
        {
            eprintln!("Skipping invalid Compose DNS labels: {entry:?}");
            continue;
        }
        let service = format!("{}.{}.{}.", entry.service, entry.project, ZONE).to_ascii_lowercase();
        let replica = format!(
            "{}-{}.{}.{}.",
            entry.service, entry.number, entry.project, ZONE
        )
        .to_ascii_lowercase();
        if Name::from_ascii(&service).is_err() || Name::from_ascii(&replica).is_err() {
            eprintln!("Skipping oversized DNS name: {service}");
            continue;
        }
        records
            .entry(format!("{}.{}.", entry.project, ZONE).to_ascii_lowercase())
            .or_default();
        let mut names = vec![service, replica];
        if entry.default {
            names.push(format!("{}.{}.", entry.project, ZONE).to_ascii_lowercase());
        }
        for (label, alias, scoped) in [
            ("dns", entry.alias, true),
            ("dns.global", entry.global_alias, false),
        ] {
            for alias in alias.iter().flat_map(|value| value.split(',')) {
                if alias.trim().is_empty() {
                    continue;
                }
                let mut alias = alias.trim().trim_end_matches('.').to_ascii_lowercase();
                if let Some(base) = alias.strip_suffix(&format!(".{ZONE}")) {
                    alias = base.to_owned();
                }
                let project = entry.project.to_ascii_lowercase();
                if scoped && alias != project && !alias.ends_with(&format!(".{project}")) {
                    alias.push_str(&format!(".{project}"));
                }
                alias.push_str(&format!(".{ZONE}"));
                if alias
                    .split('.')
                    .all(|label| label == "*" || valid_label(label))
                    && Name::from_ascii(&alias).is_ok()
                {
                    names.push(format!("{alias}."));
                } else {
                    eprintln!("Ignoring invalid {label} label: {alias}");
                }
            }
        }
        for name in names {
            records
                .entry(name)
                .or_default()
                .extend(entry.addresses.iter().copied());
        }
    }
    for addresses in records.values_mut() {
        addresses.sort();
        addresses.dedup();
    }
    records
}

async fn refresh(docker: &Docker, server: &Server) -> Result<()> {
    let containers = docker.list_containers(None).await?;
    let mut priorities = HashMap::new();
    for network in docker.list_networks(None).await? {
        let Some(name) = network.name else { continue };
        let priority = match network
            .labels
            .as_ref()
            .and_then(|labels| labels.get("dns.priority"))
        {
            Some(value) => value.parse::<u32>().unwrap_or_else(|_| {
                eprintln!("Network {name}: invalid dns.priority {value:?}; using 500");
                500
            }),
            None => 500,
        };
        priorities.insert(name, priority);
    }
    let mut entries = Vec::new();
    for container in containers {
        let labels = container.labels.unwrap_or_default();
        let (Some(project), Some(service), Some(number)) = (
            labels.get("com.docker.compose.project"),
            labels.get("com.docker.compose.service"),
            labels.get("com.docker.compose.container-number"),
        ) else {
            continue;
        };
        if labels
            .get("com.docker.compose.oneoff")
            .is_some_and(|s| s.eq_ignore_ascii_case("true"))
        {
            continue;
        }
        let Some(networks) = container.network_settings.and_then(|n| n.networks) else {
            continue;
        };
        let primary = container.host_config.and_then(|h| h.network_mode);
        let selected = if let Some(name) = labels.get("dns.network") {
            let endpoint = networks.get(name);
            if endpoint.is_none() {
                eprintln!("Skipping {service}.{project}: dns.network {name:?} is not attached");
            }
            endpoint
        } else {
            networks
                .iter()
                .min_by_key(|(name, _)| {
                    (
                        priorities.get(*name).copied().unwrap_or(500),
                        primary.as_ref() != Some(*name),
                        *name,
                    )
                })
                .map(|(_, endpoint)| endpoint)
        };
        let Some(endpoint) = selected else {
            continue;
        };
        let addresses: Vec<IpAddr> = [
            endpoint.ip_address.as_deref(),
            endpoint.global_ipv6_address.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|s| s.parse().ok())
        .filter(|ip: &IpAddr| !ip.is_unspecified())
        .collect();
        if addresses.is_empty() {
            continue;
        }
        entries.push(Entry {
            project: project.clone(),
            service: service.clone(),
            number: number.clone(),
            alias: labels.get("dns").cloned(),
            global_alias: labels.get("dns.global").cloned(),
            default: labels
                .get("dns.default")
                .is_some_and(|value| value.eq_ignore_ascii_case("true")),
            addresses,
        });
    }
    let records = build_records(entries);
    let mut snapshot = server.snapshot.write().await;
    if !snapshot.ready || snapshot.records != records {
        eprintln!("Discovery refreshed: {} names", records.len());
    }
    server
        .rotation
        .lock()
        .await
        .retain(|(name, _), _| records.contains_key(name));
    *snapshot = Snapshot {
        ready: true,
        records,
    };
    Ok(())
}

// Never probe Docker through its activation socket to discover whether it is up.
async fn docker_active() -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", "docker.service"])
        .status()
        .await
        .is_ok_and(|status| status.success())
}

async fn watch(docker: &Docker, server: &Server, failures: &mut u32) -> Result<()> {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .to_string();
    // Replay events from before the snapshot so startup changes are not missed.
    let filters = HashMap::from([
        ("type", vec!["container", "network"]),
        (
            "event",
            vec!["start", "die", "destroy", "connect", "disconnect"],
        ),
    ]);
    let options = EventsOptionsBuilder::new()
        .since(&since)
        .filters(&filters)
        .build();
    refresh(docker, server).await?;
    let mut events = docker.events(Some(options));
    while let Some(event) = events.next().await {
        event?;
        // Drain already queued events: their changes share one fresh snapshot.
        loop {
            match events.next().now_or_never() {
                Some(Some(event)) => {
                    event?;
                }
                Some(None) => return Err(anyhow!("Docker event stream closed")),
                None => break,
            }
        }
        if !docker_active().await {
            return Ok(());
        }
        refresh(docker, server).await?;
        *failures = 0;
    }
    Err(anyhow!("Docker event stream closed"))
}

async fn discovery(server: Arc<Server>) {
    let mut failures = 0u32;
    loop {
        if !docker_active().await {
            *server.snapshot.write().await = Snapshot::default();
            server.rotation.lock().await.clear();
            failures = 0;
        } else {
            // A new client resets the transport on every reconnection.
            let connection = async {
                Docker::connect_with_unix_defaults()?
                    .negotiate_version()
                    .await
                    .map_err(anyhow::Error::from)
            }
            .await;
            let result = match &connection {
                Ok(docker) => watch(docker, &server, &mut failures).await,
                Err(error) => Err(anyhow!("{error}")),
            };
            if let Err(error) = result {
                failures = failures.saturating_add(1);
                if failures >= 3 {
                    eprintln!("Docker discovery failed {failures} times in a row: {error}");
                }
                // Reconcile after a failed stream before resetting its connection.
                let recovered = if docker_active().await {
                    match &connection {
                        Ok(docker) => refresh(docker, &server).await.is_ok(),
                        Err(_) => false,
                    }
                } else {
                    false
                };
                if !recovered {
                    *server.snapshot.write().await = Snapshot::default();
                    server.rotation.lock().await.clear();
                }
            } else {
                failures = 0;
            }
        }
        sleep(RETRY).await;
    }
}

fn response_base(query: &Message) -> Message {
    let mut response = Message::new();
    response
        .set_id(query.id())
        .set_message_type(MessageType::Response)
        .set_op_code(query.op_code())
        .set_recursion_desired(query.recursion_desired());
    response.add_queries(query.queries().iter().cloned());
    response
}
fn lookup_records<'a>(
    records: &'a HashMap<String, Vec<IpAddr>>,
    name: &str,
) -> Option<(&'a String, &'a Vec<IpAddr>)> {
    if let Some(record) = records.get_key_value(name) {
        return Some(record);
    }
    let labels: Vec<_> = name.trim_end_matches('.').split('.').collect();
    records
        .iter()
        .filter(|(pattern, _)| {
            let parts: Vec<_> = pattern.trim_end_matches('.').split('.').collect();
            parts.len() == labels.len()
                && parts
                    .iter()
                    .zip(&labels)
                    .all(|(part, label)| !label.is_empty() && (*part == "*" || part == label))
        })
        // Prefer fewer wildcards, then literal labels nearest the zone.
        .max_by_key(|(pattern, _)| {
            (
                std::cmp::Reverse(pattern.split('.').filter(|part| *part == "*").count()),
                pattern
                    .rsplit('.')
                    .map(|part| part != "*")
                    .collect::<Vec<_>>(),
                *pattern,
            )
        })
}

fn local_response(query: &Message, snapshot: &Snapshot, rotation: usize) -> Message {
    let mut response = response_base(query);
    response.set_authoritative(true);
    if !snapshot.ready {
        response.set_response_code(ResponseCode::ServFail);
        return response;
    }
    let question = &query.queries()[0];
    let key = question.name().to_ascii().to_ascii_lowercase();
    if let Some((_, addresses)) = lookup_records(&snapshot.records, &key) {
        let mut answers: Vec<_> = addresses
            .iter()
            .filter_map(|ip| match (question.query_type(), ip) {
                (RecordType::A, IpAddr::V4(ip)) => Some(RData::A(A(*ip))),
                (RecordType::AAAA, IpAddr::V6(ip)) => Some(RData::AAAA(AAAA(*ip))),
                _ => None,
            })
            .collect();
        if !answers.is_empty() {
            let len = answers.len();
            answers.rotate_left(rotation % len);
        }
        for data in answers {
            response.add_answer(Record::from_rdata(question.name().clone(), TTL, data));
        }
    } else {
        response.set_response_code(ResponseCode::NXDomain);
    }
    if response.answers().is_empty() {
        let zone = Name::from_ascii(format!("{}.", ZONE)).expect("fixed zone");
        let soa = SOA::new(
            zone.clone(),
            Name::from_ascii(format!("hostmaster.{}.", ZONE)).unwrap(),
            1,
            30,
            10,
            60,
            TTL,
        );
        response.add_name_server(Record::from_rdata(zone, TTL, RData::SOA(soa)));
    }
    response
}
fn is_local(name: &Name) -> bool {
    let name = name.to_ascii().to_ascii_lowercase();
    name == format!("{ZONE}.") || name.ends_with(&format!(".{ZONE}."))
}

impl Server {
    async fn answer(&self, bytes: &[u8], tcp: bool) -> Result<Vec<u8>> {
        let query = Message::from_vec(bytes)?;
        let mut response = response_base(&query);
        if query.message_type() != MessageType::Query || query.queries().len() != 1 {
            response.set_response_code(ResponseCode::FormErr);
        } else if query.op_code() != OpCode::Query
            || query.queries()[0].query_class() != DNSClass::IN
        {
            response.set_response_code(ResponseCode::NotImp);
        } else if is_local(query.queries()[0].name()) {
            let snapshot = self.snapshot.read().await;
            let name = query.queries()[0].name().to_ascii().to_ascii_lowercase();
            if let Some((pattern, _)) = lookup_records(&snapshot.records, &name) {
                let key = (pattern.clone(), query.queries()[0].query_type());
                let mut rotations = self.rotation.lock().await;
                let cursor = rotations.entry(key).or_default();
                response = local_response(&query, &snapshot, *cursor);
                *cursor = cursor.wrapping_add(1);
            } else {
                response = local_response(&query, &snapshot, 0);
            }
        } else {
            response.set_response_code(ResponseCode::Refused);
        }
        let mut wire = response.to_vec()?;
        let limit = query
            .extensions()
            .as_ref()
            .map_or(512, |edns| edns.max_payload().clamp(512, 1232)) as usize;
        if !tcp && wire.len() > limit {
            response.set_truncated(true);
            response.answers_mut().clear();
            response.name_servers_mut().clear();
            response.additionals_mut().clear();
            wire = response.to_vec()?;
        }
        Ok(wire)
    }
}

async fn tcp_client(mut stream: TcpStream, server: Arc<Server>) -> Result<()> {
    loop {
        let size = match timeout(Duration::from_secs(30), stream.read_u16()).await {
            Ok(Ok(size)) => size as usize,
            _ => return Ok(()),
        };
        let mut bytes = vec![0; size];
        timeout(Duration::from_secs(5), stream.read_exact(&mut bytes)).await??;
        let response = server.answer(&bytes, true).await?;
        timeout(Duration::from_secs(5), async {
            stream.write_u16(response.len().try_into()?).await?;
            stream.write_all(&response).await?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let udp = Arc::new(
        UdpSocket::bind(LISTEN)
            .await
            .context("Bind UDP DNS listener")?,
    );
    let tcp = TcpListener::bind(LISTEN)
        .await
        .context("Bind TCP DNS listener")?;
    eprintln!("Listening on {} (UDP/TCP), zone {}", LISTEN, ZONE);
    let server = Arc::new(Server {
        snapshot: RwLock::new(Snapshot::default()),
        rotation: Mutex::new(HashMap::new()),
    });
    tokio::spawn(discovery(server.clone()));
    let udp_budget = Arc::new(Semaphore::new(128));
    let tcp_budget = Arc::new(Semaphore::new(64));
    let mut buffer = vec![0; 65535];
    loop {
        tokio::select! {
            result = udp.recv_from(&mut buffer) => {
                let (size, peer) = result?;
                if let Ok(permit) = udp_budget.clone().try_acquire_owned() {
                    let bytes = buffer[..size].to_vec();
                    let server = server.clone();
                    let socket = udp.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        if let Ok(response) = server.answer(&bytes, false).await {
                            let _ = socket.send_to(&response, peer).await;
                        }
                    });
                }
            }
            result = tcp.accept() => {
                let (stream, _) = result?;
                if let Ok(permit) = tcp_budget.clone().try_acquire_owned() {
                    let server = server.clone();
                    tokio::spawn(async move { let _permit = permit; let _ = tcp_client(stream, server).await; });
                }
            }
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::Query;
    fn entry(number: &str, ip: &str) -> Entry {
        Entry {
            project: "demo01".into(),
            service: "web".into(),
            number: number.into(),
            alias: None,
            global_alias: None,
            default: false,
            addresses: vec![ip.parse().unwrap()],
        }
    }
    fn query(name: &str, kind: RecordType) -> Message {
        let mut query = Message::new();
        query
            .set_id(42)
            .add_query(Query::query(Name::from_ascii(name).unwrap(), kind));
        query
    }
    #[test]
    fn replicas_and_rotating_service_answers() {
        let snapshot = Snapshot {
            ready: true,
            records: build_records(vec![
                entry("1", "172.30.53.10"),
                entry("3", "172.30.53.12"),
                entry("2", "172.30.53.11"),
            ]),
        };
        let query = query("WEB.demo01.docker.", RecordType::A);
        let first = local_response(&query, &snapshot, 0);
        let second = local_response(&query, &snapshot, 1);
        assert_eq!(first.answers().len(), 3);
        assert_ne!(first.answers()[0].data(), second.answers()[0].data());
        for number in 1..=3 {
            let query = super::tests::query(&format!("web-{number}.demo01.docker."), RecordType::A);
            assert_eq!(local_response(&query, &snapshot, 0).answers().len(), 1);
        }
    }
    #[test]
    fn ipv6_nodata_missing_and_unavailable_are_distinct() {
        let snapshot = Snapshot {
            ready: true,
            records: build_records(vec![entry("1", "172.30.53.10")]),
        };
        let nodata = local_response(&query("web.demo01.docker.", RecordType::AAAA), &snapshot, 0);
        assert_eq!(nodata.response_code(), ResponseCode::NoError);
        assert!(nodata.answers().is_empty());
        let project = local_response(&query("demo01.docker.", RecordType::A), &snapshot, 0);
        assert_eq!(project.response_code(), ResponseCode::NoError);
        assert!(project.answers().is_empty());
        assert_eq!(project.name_servers()[0].ttl(), 0);
        assert_eq!(
            local_response(
                &query("missing.demo01.docker.", RecordType::A),
                &snapshot,
                0
            )
            .response_code(),
            ResponseCode::NXDomain
        );
        assert_eq!(
            local_response(
                &query("web.demo01.docker.", RecordType::A),
                &Snapshot::default(),
                0
            )
            .response_code(),
            ResponseCode::ServFail
        );
        let ipv6 = Snapshot {
            ready: true,
            records: build_records(vec![entry("1", "fd00::1")]),
        };
        let response = local_response(&query("web.demo01.docker.", RecordType::AAAA), &ipv6, 0);
        assert_eq!(response.answers().len(), 1);
        assert!(Message::from_vec(&response.to_vec().unwrap()).is_ok());
    }
    #[test]
    fn replica_numbers_survive_gaps_and_reconciliation_removes_names() {
        let records = build_records(vec![entry("3", "172.30.53.12")]);
        assert!(records.contains_key("web-3.demo01.docker."));
        assert!(!records.contains_key("web-1.demo01.docker."));
        assert!(build_records(vec![]).is_empty());
        assert!(!is_local(&Name::from_ascii("contest.").unwrap()));
    }
    #[tokio::test]
    async fn large_rrset_truncates_udp_but_not_tcp() {
        let entries = (1..=80)
            .map(|i| entry(&i.to_string(), &format!("172.30.53.{i}")))
            .collect();
        let server = Server {
            snapshot: RwLock::new(Snapshot {
                ready: true,
                records: build_records(entries),
            }),
            rotation: Mutex::new(HashMap::new()),
        };
        let wire = query("web.demo01.docker.", RecordType::A).to_vec().unwrap();
        let udp = server.answer(&wire, false).await.unwrap();
        assert!(udp.len() <= 512);
        assert!(Message::from_vec(&udp).unwrap().truncated());
        let tcp = server.answer(&wire, true).await.unwrap();
        assert_eq!(Message::from_vec(&tcp).unwrap().answers().len(), 80);
    }
    #[tokio::test]
    async fn round_robin_is_independent_of_other_query_types() {
        let server = Server {
            snapshot: RwLock::new(Snapshot {
                ready: true,
                records: build_records(vec![
                    entry("1", "172.30.53.10"),
                    entry("2", "172.30.53.11"),
                ]),
            }),
            rotation: Mutex::new(HashMap::new()),
        };
        let wire = query("web.demo01.docker.", RecordType::A).to_vec().unwrap();
        let first = Message::from_vec(&server.answer(&wire, false).await.unwrap()).unwrap();
        server
            .answer(
                &query("web.demo01.docker.", RecordType::AAAA)
                    .to_vec()
                    .unwrap(),
                false,
            )
            .await
            .unwrap();
        let second = Message::from_vec(&server.answer(&wire, false).await.unwrap()).unwrap();
        assert_ne!(first.answers()[0].data(), second.answers()[0].data());
    }

    #[tokio::test]
    async fn external_queries_are_refused_without_recursion() {
        let server = Server {
            snapshot: RwLock::new(Snapshot::default()),
            rotation: Mutex::new(HashMap::new()),
        };
        let mut query = query("example.com.", RecordType::A);
        query.set_recursion_desired(true);
        for tcp in [false, true] {
            let response =
                Message::from_vec(&server.answer(&query.to_vec().unwrap(), tcp).await.unwrap())
                    .unwrap();
            assert_eq!(response.response_code(), ResponseCode::Refused);
            assert!(!response.recursion_available());
            assert!(response.answers().is_empty());
        }
    }
    #[tokio::test]
    async fn docker_snapshot_registers_all_projects_and_removes_stopped_containers() {
        let path = format!("/tmp/plop-dns-snapshot-{}.sock", std::process::id());
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let fixture = include_str!("../tests/fixtures/running.json");
        let selected_network = fixture.replace(
            "\"dns\": \"special.docker\"",
            "\"dns\": \"special.docker\", \"dns.network\": \"aaa-extra\"",
        );
        let missing_network = selected_network.replace(
            "\"dns.network\": \"aaa-extra\"",
            "\"dns.network\": \"missing\"",
        );
        let mock = tokio::spawn(async move {
            for body in [
                r#"{"ApiVersion":"1.53"}"#.to_owned(),
                fixture.to_owned(),
                "[]".to_owned(),
                fixture.to_owned(),
                r#"[{"Name":"aaa-extra","Labels":{"dns.priority":"1"}}]"#.to_owned(),
                fixture.to_owned(),
                r#"[{"Name":"compose-discovery","Labels":{"dns.priority":"999"}}]"#.to_owned(),
                fixture.to_owned(),
                r#"[{"Name":"aaa-extra","Labels":{"dns.priority":"invalid"}}]"#.to_owned(),
                selected_network,
                r#"[{"Name":"aaa-extra","Labels":{"dns.priority":"999"}}]"#.to_owned(),
                missing_network,
                "[]".to_owned(),
                "[]".to_owned(),
                "[]".to_owned(),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                }
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let docker = Docker::connect_with_unix(&path, 1, bollard::API_DEFAULT_VERSION).unwrap();
        let docker = docker.negotiate_version().await.unwrap();
        let server = Server {
            snapshot: RwLock::new(Snapshot::default()),
            rotation: Mutex::new(HashMap::new()),
        };
        refresh(&docker, &server).await.unwrap();
        {
            let snapshot = server.snapshot.read().await;
            assert!(snapshot.ready);
            assert_eq!(snapshot.records.len(), 10);
            assert_eq!(snapshot.records["web.demo01.docker."].len(), 3);
            assert_eq!(
                snapshot.records["web-1.demo01.docker."],
                vec!["172.30.53.10".parse::<IpAddr>().unwrap()]
            );
            assert!(snapshot.records.contains_key("web-3.demo01.docker."));
            assert!(!snapshot.records.contains_key("web-2.demo01.docker."));
            assert!(snapshot.records.contains_key("web.demo02.docker."));
            assert_eq!(snapshot.records["special.demo01.docker."].len(), 1);
            assert_eq!(snapshot.records["shared.docker."].len(), 2);
            assert_eq!(
                snapshot.records["demo01.docker."],
                vec!["172.30.53.10".parse::<IpAddr>().unwrap()]
            );
            assert!(snapshot.records["demo02.docker."].is_empty());
            assert!(!snapshot.records.contains_key("client.demo01.docker."));
        }
        // A priority of 1 wins; 999 on the primary network loses to default 500.
        for _ in 0..2 {
            refresh(&docker, &server).await.unwrap();
            assert_eq!(
                server.snapshot.read().await.records["web-1.demo01.docker."],
                vec![
                    "192.0.2.100".parse::<IpAddr>().unwrap(),
                    "fd00::100".parse().unwrap()
                ],
            );
        }
        // Invalid priority falls back to 500, preserving the primary-network tie break.
        refresh(&docker, &server).await.unwrap();
        assert_eq!(
            server.snapshot.read().await.records["web-1.demo01.docker."],
            vec!["172.30.53.10".parse::<IpAddr>().unwrap()],
        );
        // An explicit container override wins even when its network has priority 999.
        refresh(&docker, &server).await.unwrap();
        {
            let snapshot = server.snapshot.read().await;
            let selected: Vec<IpAddr> =
                vec!["192.0.2.100".parse().unwrap(), "fd00::100".parse().unwrap()];
            assert_eq!(snapshot.records["web-1.demo01.docker."], selected);
            assert_eq!(snapshot.records["demo01.docker."], selected);
            assert!(snapshot.records["web.demo01.docker."].contains(&selected[0]));
            assert!(snapshot.records["special.demo01.docker."].contains(&selected[1]));
            assert!(!snapshot.records["special.demo01.docker."]
                .contains(&"172.30.53.10".parse().unwrap()));
        }
        refresh(&docker, &server).await.unwrap();
        {
            let snapshot = server.snapshot.read().await;
            assert!(!snapshot.records.contains_key("web-1.demo01.docker."));
            assert!(snapshot.records["demo01.docker."].is_empty());
            assert_eq!(snapshot.records["web.demo01.docker."].len(), 2);
            assert!(!snapshot.records.contains_key("special.demo01.docker."));
            assert_eq!(snapshot.records["special.demo02.docker."].len(), 1);
            assert_eq!(snapshot.records["shared.docker."].len(), 1);
        }
        refresh(&docker, &server).await.unwrap();
        assert!(server.snapshot.read().await.records.is_empty());
        mock.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn custom_and_default_labels_add_records_and_share_round_robin() {
        let mut first = entry("1", "172.30.53.10");
        first.alias = Some("special".into());
        first.default = true;
        let mut second = entry("1", "172.30.53.11");
        second.service = "api".into();
        second.alias = Some("SPECIAL.DOCKER.".into());
        second.default = true;
        let records = build_records(vec![first, second]);
        for name in [
            "web.demo01.docker.",
            "web-1.demo01.docker.",
            "api.demo01.docker.",
            "api-1.demo01.docker.",
        ] {
            assert_eq!(records[name].len(), 1);
        }
        let server = Server {
            snapshot: RwLock::new(Snapshot {
                ready: true,
                records,
            }),
            rotation: Mutex::new(HashMap::new()),
        };
        for name in ["special.demo01.docker.", "demo01.docker."] {
            let wire = query(name, RecordType::A).to_vec().unwrap();
            let first = Message::from_vec(&server.answer(&wire, false).await.unwrap()).unwrap();
            let second = Message::from_vec(&server.answer(&wire, false).await.unwrap()).unwrap();
            assert_eq!(first.answers().len(), 2);
            assert_ne!(first.answers()[0].data(), second.answers()[0].data());
            assert!(first.answers().iter().all(|record| record.ttl() == 0));
        }
    }

    #[test]
    fn aliases_are_scoped_once_and_globals_are_shared_across_projects() {
        for alias in [
            "special",
            "special.docker",
            "special.demo01",
            "special.demo01.docker",
            " SPECIAL.DEMO01.DOCKER. ",
        ] {
            let mut first = entry("1", "172.30.53.10");
            first.alias = Some(alias.into());
            first.global_alias = Some("shared".into());
            let mut second = entry("1", "172.30.53.11");
            second.project = "demo02".into();
            second.alias = Some("special".into());
            second.global_alias = Some(" SHARED.DOCKER. ".into());
            let records = build_records(vec![first, second]);
            assert_eq!(records.len(), 9);
            assert_eq!(
                records["special.demo01.docker."],
                vec!["172.30.53.10".parse::<IpAddr>().unwrap()]
            );
            assert_eq!(
                records["special.demo02.docker."],
                vec!["172.30.53.11".parse::<IpAddr>().unwrap()]
            );
            assert_eq!(records["shared.docker."].len(), 2);
        }
    }

    #[tokio::test]
    async fn wildcard_aliases_match_each_label_and_rotate_answers() {
        for scoped in [false, true] {
            for (pattern, prefix) in [("*.x", "a.x"), ("*.*.y", "a.b.y")] {
                let suffix = if scoped { "demo01.docker." } else { "docker." };
                let mut first = entry("1", "172.30.53.10");
                if scoped {
                    first.alias = Some(format!("{pattern}.demo01.docker"));
                } else {
                    first.global_alias = Some(format!("{pattern}.docker"));
                }
                first.addresses.push("fd00::1".parse().unwrap());
                let mut second = entry("2", "172.30.53.11");
                if scoped {
                    second.alias = Some(pattern.into());
                } else {
                    second.global_alias = Some(pattern.into());
                }
                let server = Server {
                    snapshot: RwLock::new(Snapshot {
                        ready: true,
                        records: build_records(vec![first, second]),
                    }),
                    rotation: Mutex::new(HashMap::new()),
                };
                let name = format!("{prefix}.{suffix}");
                let wire = query(&name.to_ascii_uppercase(), RecordType::A)
                    .to_vec()
                    .unwrap();
                let first = Message::from_vec(&server.answer(&wire, false).await.unwrap()).unwrap();
                let second = Message::from_vec(&server.answer(&wire, true).await.unwrap()).unwrap();
                assert_eq!(first.answers().len(), 2);
                assert_ne!(first.answers()[0].data(), second.answers()[0].data());
                assert!(first
                    .answers()
                    .iter()
                    .all(|record| record.name().to_ascii().eq_ignore_ascii_case(&name)));
                let snapshot = server.snapshot.read().await;
                assert_eq!(
                    local_response(&query(&name, RecordType::AAAA), &snapshot, 0)
                        .answers()
                        .len(),
                    1
                );
                for prefix in ["x", "a.b.x", "a.y", "a.b.c.y"] {
                    assert_eq!(
                        local_response(
                            &query(&format!("{prefix}.{suffix}"), RecordType::A),
                            &snapshot,
                            0
                        )
                        .response_code(),
                        ResponseCode::NXDomain
                    );
                }
            }
        }
    }

    #[test]
    fn exact_records_and_more_specific_wildcards_take_precedence() {
        let mut broad = entry("1", "172.30.53.10");
        broad.global_alias = Some("*.*.y".into());
        let mut specific = entry("2", "172.30.53.11");
        specific.global_alias = Some("*.b.y".into());
        let mut exact = entry("3", "172.30.53.12");
        exact.global_alias = Some("a.b.y".into());
        let records = build_records(vec![broad, specific, exact]);
        for (name, ip) in [
            ("a.b.y.docker.", "172.30.53.12"),
            ("c.b.y.docker.", "172.30.53.11"),
            ("c.d.y.docker.", "172.30.53.10"),
        ] {
            assert_eq!(
                lookup_records(&records, name).unwrap().1,
                &vec![ip.parse::<IpAddr>().unwrap()]
            );
        }
        for invalid in ["a*", "**.x", "*.bad name"] {
            let mut container = entry("1", "172.30.53.10");
            container.alias = Some(invalid.into());
            container.global_alias = Some(invalid.into());
            assert_eq!(build_records(vec![container]).len(), 3);
        }
    }

    #[test]
    fn alias_lists_support_exact_wildcard_and_qualified_names() {
        let mut container = entry("1", "172.30.53.10");
        container.alias = Some("x, *.x, *.*.y, X.demo01.docker., z.demo01, bad name, ,".into());
        container.global_alias = Some("shared, *.shared, *.*.global, SHARED.DOCKER., a*, ,".into());
        let snapshot = Snapshot {
            ready: true,
            records: build_records(vec![container]),
        };
        assert_eq!(snapshot.records.len(), 10);
        for name in [
            "x.demo01.docker.",
            "a.x.demo01.docker.",
            "a.b.y.demo01.docker.",
            "z.demo01.docker.",
            "shared.docker.",
            "a.shared.docker.",
            "a.b.global.docker.",
            "web.demo01.docker.",
        ] {
            let response = local_response(&query(name, RecordType::A), &snapshot, 0);
            assert_eq!(response.response_code(), ResponseCode::NoError);
            assert_eq!(response.answers().len(), 1, "{name}");
        }
        for name in ["x.docker.", "a.b.x.demo01.docker.", "shared.demo01.docker."] {
            assert_eq!(
                local_response(&query(name, RecordType::A), &snapshot, 0).response_code(),
                ResponseCode::NXDomain
            );
        }
    }

    #[test]
    fn invalid_custom_alias_does_not_remove_automatic_records() {
        let mut container = entry("1", "172.30.53.10");
        container.alias = Some("bad name".into());
        let records = build_records(vec![container]);
        assert_eq!(records.len(), 3);
        assert!(records.contains_key("web.demo01.docker."));
        let mut container = entry("1", "172.30.53.10");
        container.alias = Some("special.notdocker".into());
        assert!(build_records(vec![container]).contains_key("special.notdocker.demo01.docker."));
    }
}
