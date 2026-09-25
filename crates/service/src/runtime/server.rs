use std::env;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{
    handle_api_request_with_body_mut, reconcile_browser_sessions, ApiErrorBody, ApiResponse,
    ApiState, TaskSnapshot, TaskSnapshotFeed,
};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8765;
const INITIAL_REQUEST_BYTES: usize = 16 * 1024;
const MAX_REQUEST_HEADER_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

const TASK_EVENTS_PATH: &str = "/tasks/events";
const TASK_EVENT_POLL_INTERVAL_MS: u64 = 1_000;

pub fn run() -> io::Result<()> {
    let options = ServerOptions::from_args(env::args().skip(1));
    let listener = TcpListener::bind((options.host.as_str(), options.port))?;
    let state = ApiState::from_environment();
    let task_snapshot_feed = state.tasks.snapshot_feed();
    let state = Arc::new(Mutex::new(state));
    let maintenance_state = Arc::clone(&state);
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(TASK_EVENT_POLL_INTERVAL_MS));
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default();
        if let Ok(mut state) = maintenance_state.lock() {
            reconcile_browser_sessions(&mut state, now_unix);
        }
    });

    eprintln!(
        "overleaf-service-api listening on http://{}:{}",
        options.host, options.port
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = Arc::clone(&state);
                let task_snapshot_feed = task_snapshot_feed.clone();
                thread::spawn(move || {
                    if let Err(error) = handle_connection(stream, state, task_snapshot_feed) {
                        eprintln!("request failed: {error}");
                    }
                });
            }
            Err(error) => eprintln!("accept failed: {error}"),
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerOptions {
    host: String,
    port: u16,
}

impl ServerOptions {
    fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut host = env::var("OVERLEAF_SWITCHER_SERVICE_HOST")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_HOST.to_string());
        let mut port = env::var("OVERLEAF_SWITCHER_SERVICE_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);

        let mut args = args.into_iter().map(Into::into);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--host" => {
                    if let Some(value) = args.next() {
                        host = value;
                    }
                }
                "--port" => {
                    if let Some(value) = args.next().and_then(|value| value.parse::<u16>().ok()) {
                        port = value;
                    }
                }
                _ => {}
            }
        }

        Self { host, port }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    state: Arc<Mutex<ApiState>>,
    task_snapshot_feed: TaskSnapshotFeed,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let request = match read_http_request(&mut stream) {
        Ok(Some(request)) => request,
        Ok(None) => return Ok(()),
        Err(RequestReadError::BodyTooLarge { content_length }) => {
            return write_raw_response(
                &mut stream,
                413,
                "Payload Too Large",
                "application/json; charset=utf-8",
                &serde_json::to_string(&ApiErrorBody {
                    error: format!(
                        "request body too large: {content_length} bytes exceeds {MAX_REQUEST_BODY_BYTES}"
                    ),
                })
                .unwrap_or_else(|_| r#"{"error":"request body too large"}"#.to_string()),
            );
        }
        Err(RequestReadError::HeaderTooLarge) => {
            return write_raw_response(
                &mut stream,
                431,
                "Request Header Fields Too Large",
                "application/json; charset=utf-8",
                r#"{"error":"request headers too large"}"#,
            );
        }
        Err(RequestReadError::Io(error)) => return Err(error),
    };
    let Some((method, target)) = parse_request_line(&request) else {
        return write_raw_response(
            &mut stream,
            400,
            "Bad Request",
            "application/json; charset=utf-8",
            r#"{"error":"bad request"}"#,
        );
    };

    if let Some((status, reason)) = browser_request_rejection(&request, method) {
        return write_raw_response(
            &mut stream,
            status,
            reason,
            "application/json; charset=utf-8",
            &serde_json::json!({"error": reason}).to_string(),
        );
    }

    if is_task_events_target(target) {
        return handle_task_events_connection(&mut stream, method, task_snapshot_feed);
    }

    // Remote release checks must not hold the account/task state lock.
    if request_path(target) == "/runtime/updates" {
        let response = super::updates::response(method);
        return write_raw_response(
            &mut stream,
            response.status_code,
            response.status_text(),
            response.content_type,
            &response.body,
        );
    }

    if request_path(target) == "/accounts/projects" {
        let response = crate::api::projects::handle(&state, method, request_body(&request));
        return write_raw_response(
            &mut stream,
            response.status_code,
            response.status_text(),
            response.content_type,
            &response.body,
        );
    }

    let (response, background_jobs) = match ui_asset_response(method, target) {
        Some(response) => (response, Vec::new()),
        None => {
            let account_commit_lock = if requires_account_commit_lock(method, target) {
                Some(
                    state
                        .lock()
                        .map_err(|_| io::Error::other("api state lock poisoned"))?
                        .account_commit_lock(),
                )
            } else {
                None
            };
            let _account_commit_guard = account_commit_lock
                .as_ref()
                .map(|lock| {
                    lock.lock()
                        .map_err(|_| io::Error::other("account commit lock poisoned"))
                })
                .transpose()?;
            let mut guard = state
                .lock()
                .map_err(|_| io::Error::other("api state lock poisoned"))?;
            let response = handle_api_request_with_body_mut(
                &mut guard,
                method,
                target,
                request_body(&request),
                unix_now(),
            );
            let background_jobs = guard.take_background_jobs();
            (response, background_jobs)
        }
    };

    for job in background_jobs {
        let shared_state = Arc::clone(&state);
        let thread_name = format!("overleaf-api-{}", job.name());
        thread::Builder::new()
            .name(thread_name)
            .spawn(move || job.run(shared_state))
            .map_err(|error| io::Error::other(format!("spawn background job: {error}")))?;
    }

    write_raw_response(
        &mut stream,
        response.status_code,
        response.status_text(),
        response.content_type,
        &response.body,
    )
}

fn requires_account_commit_lock(method: &str, target: &str) -> bool {
    if method != "POST" {
        return false;
    }
    let path = request_path(target);
    path == "/accounts" || path.starts_with("/accounts/") || path == "/registration"
}

#[derive(Debug)]
enum RequestReadError {
    Io(io::Error),
    BodyTooLarge { content_length: usize },
    HeaderTooLarge,
}

impl From<io::Error> for RequestReadError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn read_http_request<R: Read>(reader: &mut R) -> Result<Option<String>, RequestReadError> {
    let mut request = Vec::with_capacity(INITIAL_REQUEST_BYTES);
    let mut buffer = [0_u8; INITIAL_REQUEST_BYTES];
    let bytes_read = reader.read(&mut buffer)?;
    if bytes_read == 0 {
        return Ok(None);
    }
    request.extend_from_slice(&buffer[..bytes_read]);

    loop {
        if let Some((header_end, body_start)) = request_header_boundary(&request) {
            if header_end > MAX_REQUEST_HEADER_BYTES {
                return Err(RequestReadError::HeaderTooLarge);
            }
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = request_content_length(&headers).unwrap_or(0);
            if content_length > MAX_REQUEST_BODY_BYTES {
                return Err(RequestReadError::BodyTooLarge { content_length });
            }

            let total_length = body_start.saturating_add(content_length);
            while request.len() < total_length {
                let remaining = total_length - request.len();
                let next_read = remaining.min(buffer.len());
                let bytes_read = reader.read(&mut buffer[..next_read])?;
                if bytes_read == 0 {
                    return Err(RequestReadError::Io(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "request body ended before Content-Length bytes were read",
                    )));
                }
                request.extend_from_slice(&buffer[..bytes_read]);
            }
            request.truncate(total_length);
            return Ok(Some(String::from_utf8_lossy(&request).into_owned()));
        }

        if request.len() > MAX_REQUEST_HEADER_BYTES {
            return Err(RequestReadError::HeaderTooLarge);
        }

        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(Some(String::from_utf8_lossy(&request).into_owned()));
        }
        request.extend_from_slice(&buffer[..bytes_read]);
    }
}

fn request_header_boundary(request: &[u8]) -> Option<(usize, usize)> {
    request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, index + 4))
        .or_else(|| {
            request
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| (index, index + 2))
        })
}

fn request_content_length(headers: &str) -> Option<usize> {
    request_header(headers, "content-length")?.parse().ok()
}

fn request_header<'a>(request: &'a str, header: &str) -> Option<&'a str> {
    request
        .lines()
        .take_while(|line| !line.is_empty())
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case(header)
                .then(|| value.trim())
        })
}

fn browser_request_rejection(request: &str, method: &str) -> Option<(u16, &'static str)> {
    let host = request_header(request, "host")
        .and_then(|host| reqwest::Url::parse(&format!("http://{host}")).ok());
    // This local-only API has no public authentication; reject DNS-rebinding hostnames.
    if !host
        .as_ref()
        .and_then(reqwest::Url::host_str)
        .is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok()
        })
    {
        return Some((403, "Forbidden"));
    }
    if let Some(origin) = request_header(request, "origin") {
        let same_origin = host
            .zip(reqwest::Url::parse(origin).ok())
            .is_some_and(|(host, origin)| host.origin() == origin.origin());
        if !same_origin && origin != "http://127.0.0.1:5173" {
            return Some((403, "Forbidden"));
        }
    }
    // CORS alone does not prevent cross-site form submissions from mutating local state.
    if method == "POST"
        && !request_header(request, "content-type").is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
    {
        return Some((415, "Unsupported Media Type"));
    }
    None
}

fn parse_request_line(request: &str) -> Option<(&str, &str)> {
    let first_line = request.lines().next()?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let _version = parts.next()?;
    Some((method, target))
}

fn request_body(request: &str) -> &str {
    request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .or_else(|| request.split_once("\n\n").map(|(_, body)| body))
        .unwrap_or("")
}

fn ui_asset_response(method: &str, target: &str) -> Option<ApiResponse> {
    let path = request_path(target);
    let path = match path {
        "/ui" | "/ui/index.html" => "/ui/",
        other => other,
    };
    let asset = crate::api::EMBEDDED_UI_ASSETS
        .iter()
        .find(|asset| asset.path == path)?;
    let content_type = asset.content_type;
    let body = asset.body;

    if method != "GET" && method != "HEAD" {
        return Some(ApiResponse {
            status_code: 405,
            content_type: "application/json; charset=utf-8",
            body: serde_json::to_string(&ApiErrorBody {
                error: "method not allowed".to_string(),
            })
            .unwrap_or_else(|_| r#"{"error":"method not allowed"}"#.to_string()),
        });
    }

    Some(ApiResponse {
        status_code: 200,
        content_type,
        body: if method == "HEAD" {
            String::new()
        } else {
            body.to_string()
        },
    })
}

fn handle_task_events_connection(
    stream: &mut TcpStream,
    method: &str,
    task_snapshot_feed: TaskSnapshotFeed,
) -> io::Result<()> {
    if method != "GET" && method != "HEAD" {
        return write_raw_response(
            stream,
            405,
            "Method Not Allowed",
            "application/json; charset=utf-8",
            &serde_json::to_string(&ApiErrorBody {
                error: "method not allowed".to_string(),
            })
            .unwrap_or_else(|_| r#"{"error":"method not allowed"}"#.to_string()),
        );
    }

    write_event_stream_headers(stream)?;
    if method == "HEAD" {
        return Ok(());
    }

    let mut last_payload = String::new();
    loop {
        let payload = task_event_frame(&task_snapshot_feed.list_snapshots(), &last_payload)?;

        stream.write_all(payload.as_bytes())?;
        stream.flush()?;
        if !payload.starts_with(':') {
            last_payload = payload;
        }
        thread::sleep(Duration::from_millis(TASK_EVENT_POLL_INTERVAL_MS));
    }
}

fn is_task_events_target(target: &str) -> bool {
    request_path(target) == TASK_EVENTS_PATH
}

fn request_path(target: &str) -> &str {
    target
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or(target)
}

fn write_event_stream_headers(stream: &mut TcpStream) -> io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\n\
          Content-Type: text/event-stream; charset=utf-8\r\n\
          Cache-Control: no-store\r\n\
          Access-Control-Allow-Origin: http://127.0.0.1:5173\r\n\
          Access-Control-Allow-Methods: GET, HEAD, POST, OPTIONS\r\n\
          Access-Control-Allow-Headers: Accept, Content-Type, Last-Event-ID\r\n\
          X-Accel-Buffering: no\r\n\
          Connection: keep-alive\r\n\
          \r\n",
    )
}

fn task_event_payload(snapshots: &[TaskSnapshot]) -> io::Result<String> {
    let latest_sequence = latest_task_sequence(snapshots);
    let data = serde_json::to_string(snapshots)
        .map_err(|error| io::Error::other(format!("serialize task snapshots: {error}")))?;
    Ok(format!(
        "id: {latest_sequence}\nevent: tasks\ndata: {data}\n\n"
    ))
}

fn task_event_frame(snapshots: &[TaskSnapshot], last_payload: &str) -> io::Result<String> {
    let payload = task_event_payload(snapshots)?;
    if payload == last_payload {
        Ok(task_event_heartbeat())
    } else {
        Ok(payload)
    }
}

fn latest_task_sequence(snapshots: &[TaskSnapshot]) -> u64 {
    snapshots
        .iter()
        .map(|snapshot| snapshot.last_sequence)
        .max()
        .unwrap_or(0)
}

fn task_event_heartbeat() -> String {
    ": heartbeat\n\n".to_string()
}

fn write_raw_response(
    stream: &mut TcpStream,
    status_code: u16,
    status_text: &str,
    content_type: &str,
    body: &str,
) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status_code} {status_text}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Access-Control-Allow-Origin: http://127.0.0.1:5173\r\n\
         Access-Control-Allow-Methods: GET, HEAD, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Accept, Content-Type\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(response.as_bytes())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
