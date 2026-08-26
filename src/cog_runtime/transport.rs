//! Transport-independent Cog v1 client plus the loopback `ureq` transport.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::wire::*;

const MAX_JSON_BODY: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Put,
    Post,
    Delete,
}

impl HttpMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Post => "POST",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub path_and_query: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub struct StreamResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub reader: Box<dyn BufRead + Send>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError(pub String);

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TransportError {}

pub trait CogRuntimeTransport: Send + Sync + 'static {
    fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError>;
    fn open_stream(&self, request: HttpRequest) -> Result<StreamResponse, TransportError>;
}

#[derive(Clone)]
pub struct UreqTransport {
    base_url: Arc<str>,
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new(base_url: &str) -> Result<Self, TransportError> {
        let base_url = base_url.trim_end_matches('/');
        validate_loopback_base_url(base_url)?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(90))
            .timeout_write(Duration::from_secs(15))
            .build();
        Ok(Self {
            base_url: Arc::from(base_url),
            agent,
        })
    }

    fn send(&self, request: HttpRequest) -> Result<ureq::Response, TransportError> {
        let url = format!("{}{}", self.base_url, request.path_and_query);
        let mut outbound = self.agent.request(request.method.as_str(), &url);
        for (name, value) in request.headers {
            outbound = outbound.set(&name, &value);
        }
        let response = if request.body.is_empty() {
            outbound.call()
        } else {
            outbound.send_bytes(&request.body)
        };
        match response {
            Ok(response) | Err(ureq::Error::Status(_, response)) => Ok(response),
            Err(ureq::Error::Transport(error)) => {
                Err(TransportError(format!("Cog request failed: {error}")))
            }
        }
    }
}

impl CogRuntimeTransport for UreqTransport {
    fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let response = self.send(request)?;
        let status = response.status();
        let headers = response_headers(&response);
        let mut body = Vec::new();
        response
            .into_reader()
            .take(MAX_JSON_BODY + 1)
            .read_to_end(&mut body)
            .map_err(|e| TransportError(format!("reading Cog response failed: {e}")))?;
        if body.len() as u64 > MAX_JSON_BODY {
            return Err(TransportError("Cog JSON response exceeded 16 MiB".into()));
        }
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    fn open_stream(&self, request: HttpRequest) -> Result<StreamResponse, TransportError> {
        let response = self.send(request)?;
        let status = response.status();
        let headers = response_headers(&response);
        Ok(StreamResponse {
            status,
            headers,
            reader: Box::new(BufReader::new(response.into_reader())),
        })
    }
}

fn response_headers(response: &ureq::Response) -> BTreeMap<String, String> {
    response
        .headers_names()
        .into_iter()
        .filter_map(|name| {
            response
                .header(&name)
                .map(|value| (name.to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

fn validate_loopback_base_url(value: &str) -> Result<(), TransportError> {
    let authority = value
        .strip_prefix("http://")
        .ok_or_else(|| TransportError("Cog URL must use loopback http://".into()))?;
    let host_port = authority
        .split('/')
        .next()
        .ok_or_else(|| TransportError("Cog URL has no authority".into()))?;
    let host = host_port
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']').map(|(host, _)| host))
        .unwrap_or_else(|| host_port.split(':').next().unwrap_or(host_port));
    if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return Err(TransportError(
            "Cog runtime transport is installation-local and must use a loopback host".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum CapabilityProbe {
    Available(Capabilities),
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    Transport(String),
    Api {
        status: u16,
        error: ApiError,
    },
    UnexpectedStatus {
        status: u16,
        body: String,
    },
    InvalidContentType {
        expected: &'static str,
        actual: Option<String>,
    },
    Decode(String),
    Sse(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(message) | Self::Decode(message) | Self::Sse(message) => {
                f.write_str(message)
            }
            Self::Api { status, error } => {
                write!(f, "Cog HTTP {status} {:?}: {}", error.code, error.message)
            }
            Self::UnexpectedStatus { status, body } => {
                write!(f, "Cog HTTP {status}: {}", body.trim())
            }
            Self::InvalidContentType { expected, actual } => {
                write!(f, "expected Content-Type {expected}, got {actual:?}")
            }
        }
    }
}

impl std::error::Error for ClientError {}

impl From<TransportError> for ClientError {
    fn from(value: TransportError) -> Self {
        Self::Transport(value.0)
    }
}

#[derive(Clone)]
pub struct CogClient<T> {
    transport: Arc<T>,
}

impl<T: CogRuntimeTransport> CogClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport: Arc::new(transport),
        }
    }

    pub fn from_shared(transport: Arc<T>) -> Self {
        Self { transport }
    }

    pub fn probe_capabilities(&self) -> Result<CapabilityProbe, ClientError> {
        let response = self.transport.execute(self.request(
            HttpMethod::Get,
            "/v1/runtime-delivery/capabilities",
            None::<&()>,
        )?)?;
        if response.status == 404 && response.body.is_empty() {
            return Ok(CapabilityProbe::Unavailable);
        }
        self.decode_json(response).map(CapabilityProbe::Available)
    }

    pub fn get_host_lease(&self, host_id: &OpaqueId) -> Result<HostLeaseResponse, ClientError> {
        self.json(
            HttpMethod::Get,
            &format!("/v1/runtime-hosts/{}/lease", encode_path(host_id.as_str())),
            None::<&()>,
        )
    }

    pub fn acquire_host_lease(
        &self,
        host_id: &OpaqueId,
        body: &LeaseAcquireRequest,
    ) -> Result<HostLeaseResponse, ClientError> {
        self.json(
            HttpMethod::Put,
            &format!("/v1/runtime-hosts/{}/lease", encode_path(host_id.as_str())),
            Some(body),
        )
    }

    pub fn renew_host_lease(
        &self,
        host_id: &OpaqueId,
        body: &LeaseRenewRequest,
    ) -> Result<HostLeaseResponse, ClientError> {
        self.json(
            HttpMethod::Post,
            &format!(
                "/v1/runtime-hosts/{}/lease/renew",
                encode_path(host_id.as_str())
            ),
            Some(body),
        )
    }

    pub fn release_host_lease(
        &self,
        host_id: &OpaqueId,
        body: &LeaseReleaseRequest,
    ) -> Result<HostLeaseReleaseResponse, ClientError> {
        self.json(
            HttpMethod::Delete,
            &format!("/v1/runtime-hosts/{}/lease", encode_path(host_id.as_str())),
            Some(body),
        )
    }

    pub fn get_delivery_owner(
        &self,
        address_id: &OpaqueId,
    ) -> Result<DeliveryOwnerResponse, ClientError> {
        self.json(
            HttpMethod::Get,
            &format!(
                "/v1/addresses/{}/delivery-owner",
                encode_path(address_id.as_str())
            ),
            None::<&()>,
        )
    }

    pub fn put_delivery_owner(
        &self,
        address_id: &OpaqueId,
        body: &DeliveryOwnerPutRequest,
    ) -> Result<DeliveryOwnerResponse, ClientError> {
        self.json(
            HttpMethod::Put,
            &format!(
                "/v1/addresses/{}/delivery-owner",
                encode_path(address_id.as_str())
            ),
            Some(body),
        )
    }

    pub fn list_open_attempts(
        &self,
        host_id: &OpaqueId,
        limit: u64,
        after: Option<&PageCursor>,
    ) -> Result<AttemptListResponse, ClientError> {
        let mut path = format!(
            "/v1/runtime-hosts/{}/delivery-attempts?state=open&limit={}",
            encode_path(host_id.as_str()),
            limit
        );
        if let Some(after) = after {
            path.push_str("&after=");
            path.push_str(&encode_query(after.as_str()));
        }
        self.json(HttpMethod::Get, &path, None::<&()>)
    }

    pub fn get_attempt(&self, attempt_id: &OpaqueId) -> Result<AttemptResponse, ClientError> {
        self.json(
            HttpMethod::Get,
            &format!("/v1/delivery-attempts/{}", encode_path(attempt_id.as_str())),
            None::<&()>,
        )
    }

    pub fn claim(
        &self,
        host_id: &OpaqueId,
        body: &ClaimRequest,
    ) -> Result<ClaimResponse, ClientError> {
        self.json(
            HttpMethod::Post,
            &format!(
                "/v1/runtime-hosts/{}/delivery-attempts:claim",
                encode_path(host_id.as_str())
            ),
            Some(body),
        )
    }

    pub fn renew_attempt(
        &self,
        attempt_id: &OpaqueId,
        body: &AttemptRenewRequest,
    ) -> Result<AttemptMutationResponse, ClientError> {
        self.json(
            HttpMethod::Post,
            &format!(
                "/v1/delivery-attempts/{}/renew",
                encode_path(attempt_id.as_str())
            ),
            Some(body),
        )
    }

    pub fn release_attempt(
        &self,
        attempt_id: &OpaqueId,
        body: &AttemptReleaseRequest,
    ) -> Result<AttemptMutationResponse, ClientError> {
        self.json(
            HttpMethod::Post,
            &format!(
                "/v1/delivery-attempts/{}/release",
                encode_path(attempt_id.as_str())
            ),
            Some(body),
        )
    }

    pub fn complete_attempt(
        &self,
        attempt_id: &OpaqueId,
        body: &AttemptCompleteRequest,
    ) -> Result<AttemptMutationResponse, ClientError> {
        self.json(
            HttpMethod::Post,
            &format!(
                "/v1/delivery-attempts/{}/complete",
                encode_path(attempt_id.as_str())
            ),
            Some(body),
        )
    }

    pub fn fail_attempt(
        &self,
        attempt_id: &OpaqueId,
        body: &AttemptFailRequest,
    ) -> Result<AttemptMutationResponse, ClientError> {
        self.json(
            HttpMethod::Post,
            &format!(
                "/v1/delivery-attempts/{}/fail",
                encode_path(attempt_id.as_str())
            ),
            Some(body),
        )
    }

    pub fn open_wakes(
        &self,
        host_id: &OpaqueId,
        instance_id: ProtocolUuid,
        host_fence: DecimalU64,
        last_event_id: Option<DecimalU64>,
    ) -> Result<WakeStream, ClientError> {
        let mut request = self.request(
            HttpMethod::Get,
            &format!(
                "/v1/runtime-hosts/{}/delivery-wakes?follow=true",
                encode_path(host_id.as_str())
            ),
            None::<&()>,
        )?;
        request
            .headers
            .insert("accept".into(), SSE_MEDIA_TYPE.into());
        request
            .headers
            .insert("x-cog-instance-id".into(), instance_id.to_string());
        request
            .headers
            .insert("x-cog-host-fence".into(), host_fence.to_string());
        if let Some(cursor) = last_event_id {
            request
                .headers
                .insert("last-event-id".into(), cursor.to_string());
        }
        let response = self.transport.open_stream(request)?;
        if !(200..300).contains(&response.status) {
            return Err(ClientError::UnexpectedStatus {
                status: response.status,
                body: String::new(),
            });
        }
        require_content_type(&response.headers, SSE_MEDIA_TYPE)?;
        Ok(WakeStream::new(response.reader))
    }

    fn json<R: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: HttpMethod,
        path: &str,
        body: Option<&B>,
    ) -> Result<R, ClientError> {
        let request = self.request(method, path, body)?;
        let response = self.transport.execute(request)?;
        self.decode_json(response)
    }

    fn request<B: Serialize + ?Sized>(
        &self,
        method: HttpMethod,
        path: &str,
        body: Option<&B>,
    ) -> Result<HttpRequest, ClientError> {
        let mut headers = BTreeMap::new();
        headers.insert("accept".into(), MEDIA_TYPE.into());
        let body = if let Some(body) = body {
            headers.insert("content-type".into(), MEDIA_TYPE.into());
            serde_json::to_vec(body)
                .map_err(|e| ClientError::Decode(format!("encoding Cog request failed: {e}")))?
        } else {
            Vec::new()
        };
        Ok(HttpRequest {
            method,
            path_and_query: path.to_string(),
            headers,
            body,
        })
    }

    fn decode_json<R: DeserializeOwned>(&self, response: HttpResponse) -> Result<R, ClientError> {
        if (200..300).contains(&response.status) {
            require_content_type(&response.headers, MEDIA_TYPE)?;
            return serde_json::from_slice(&response.body)
                .map_err(|e| ClientError::Decode(format!("decoding Cog v1 response failed: {e}")));
        }
        if let Ok(envelope) = serde_json::from_slice::<ErrorEnvelope>(&response.body) {
            return Err(ClientError::Api {
                status: response.status,
                error: envelope.error,
            });
        }
        Err(ClientError::UnexpectedStatus {
            status: response.status,
            body: String::from_utf8_lossy(&response.body).into_owned(),
        })
    }
}

fn require_content_type(
    headers: &BTreeMap<String, String>,
    expected: &'static str,
) -> Result<(), ClientError> {
    let actual = headers.get("content-type").cloned();
    if actual.as_deref().is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case(expected))
    }) {
        Ok(())
    } else {
        Err(ClientError::InvalidContentType { expected, actual })
    }
}

fn encode_path(value: &str) -> String {
    percent_encode(value.as_bytes())
}

fn encode_query(value: &str) -> String {
    percent_encode(value.as_bytes())
}

fn percent_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0xf) as usize] as char);
        }
    }
    out
}

pub struct WakeStream {
    reader: Box<dyn BufRead + Send>,
    last_event_id: Option<DecimalU64>,
}

impl WakeStream {
    fn new(reader: Box<dyn BufRead + Send>) -> Self {
        Self {
            reader,
            last_event_id: None,
        }
    }

    pub fn last_event_id(&self) -> Option<DecimalU64> {
        self.last_event_id
    }

    pub fn next_wake(&mut self) -> Result<Option<DeliveryWake>, ClientError> {
        loop {
            let mut event_name = None;
            let mut id = None;
            let mut data = String::new();
            let mut saw_field = false;
            loop {
                let mut line = String::new();
                let read = self
                    .reader
                    .read_line(&mut line)
                    .map_err(|e| ClientError::Sse(format!("reading Cog wake SSE failed: {e}")))?;
                if read == 0 {
                    if !saw_field {
                        return Ok(None);
                    }
                    break;
                }
                let line = line.trim_end_matches(['\r', '\n']);
                if line.is_empty() {
                    if saw_field {
                        break;
                    }
                    continue;
                }
                if line.starts_with(':') {
                    continue;
                }
                saw_field = true;
                let (field, value) = line.split_once(':').unwrap_or((line, ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "event" => event_name = Some(value.to_string()),
                    "id" => {
                        id = Some(
                            serde_json::from_str::<DecimalU64>(&format!("\"{value}\"")).map_err(
                                |e| ClientError::Sse(format!("invalid wake SSE id: {e}")),
                            )?,
                        );
                    }
                    "data" => {
                        if !data.is_empty() {
                            data.push('\n');
                        }
                        data.push_str(value);
                    }
                    "retry" => {}
                    other => {
                        return Err(ClientError::Sse(format!(
                            "unknown Cog wake SSE field {other}"
                        )));
                    }
                }
            }
            // SSE permits a standalone `retry` control frame. It changes the
            // caller's reconnect policy but is not a delivery wake.
            if event_name.is_none() && id.is_none() && data.is_empty() {
                continue;
            }
            if event_name.as_deref() != Some("delivery-ready") {
                return Err(ClientError::Sse(format!(
                    "unexpected Cog wake SSE event {event_name:?}"
                )));
            }
            let id = id.ok_or_else(|| ClientError::Sse("Cog wake SSE frame has no id".into()))?;
            let wake: DeliveryWake = serde_json::from_str(&data)
                .map_err(|e| ClientError::Sse(format!("invalid Cog wake SSE data: {e}")))?;
            if wake.wake_id != id {
                return Err(ClientError::Sse(
                    "Cog wake SSE id differs from data.wake_id".into(),
                ));
            }
            self.last_event_id = Some(id);
            return Ok(Some(wake));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::{self, Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    #[derive(Default)]
    struct ScriptedTransport {
        requests: Mutex<Vec<HttpRequest>>,
        responses: Mutex<VecDeque<HttpResponse>>,
        streams: Mutex<VecDeque<StreamResponse>>,
    }

    impl ScriptedTransport {
        fn push_json(&self, status: u16, value: serde_json::Value) {
            self.responses.lock().unwrap().push_back(HttpResponse {
                status,
                headers: BTreeMap::from([("content-type".into(), MEDIA_TYPE.into())]),
                body: serde_json::to_vec(&value).unwrap(),
            });
        }

        fn push_stream(&self, text: &str) {
            self.streams.lock().unwrap().push_back(StreamResponse {
                status: 200,
                headers: BTreeMap::from([("content-type".into(), SSE_MEDIA_TYPE.into())]),
                reader: Box::new(BufReader::new(io::Cursor::new(text.as_bytes().to_vec()))),
            });
        }
    }

    impl CogRuntimeTransport for ScriptedTransport {
        fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| TransportError("no scripted response".into()))
        }

        fn open_stream(&self, request: HttpRequest) -> Result<StreamResponse, TransportError> {
            self.requests.lock().unwrap().push(request);
            self.streams
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| TransportError("no scripted stream".into()))
        }
    }

    fn timestamp() -> &'static str {
        "2026-08-25T18:02:03.123456789Z"
    }

    #[test]
    fn capability_404_empty_is_inert_not_a_decode_error() {
        let transport = ScriptedTransport::default();
        transport.responses.lock().unwrap().push_back(HttpResponse {
            status: 404,
            headers: BTreeMap::new(),
            body: Vec::new(),
        });
        let client = CogClient::new(transport);
        assert_eq!(
            client.probe_capabilities().unwrap(),
            CapabilityProbe::Unavailable
        );
    }

    #[test]
    fn production_transport_sends_exact_capability_request_over_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut chunk).unwrap();
                assert_ne!(count, 0, "client closed before completing HTTP headers");
                request.extend_from_slice(&chunk[..count]);
            }
            request_tx
                .send(String::from_utf8(request).unwrap())
                .unwrap();

            let body = serde_json::json!({
                "schema_version":"1",
                "protocol_versions":["1"],
                "source_kinds":["mail","chat"],
                "provider_kinds":["codex","claude"],
                "features":REQUIRED_FEATURES,
                "limits":{
                    "host_lease_seconds":{"min":"5","max":"300"},
                    "attempt_lease_seconds":{"min":"5","max":"300"},
                    "max_claim_attempts":"8",
                    "max_claim_entries":"128",
                    "max_claim_content_bytes":"1048576"
                },
                "server_time":timestamp()
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {MEDIA_TYPE}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });

        let transport = UreqTransport::new(&format!("http://{address}")).unwrap();
        let client = CogClient::new(transport);
        let CapabilityProbe::Available(capabilities) = client.probe_capabilities().unwrap() else {
            panic!("mock server advertised capabilities");
        };
        assert_eq!(
            capabilities.compatibility_error(&[ProviderKind::Codex]),
            None
        );
        let request = request_rx.recv().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /v1/runtime-delivery/capabilities http/1.1\r\n"));
        assert!(request.contains(&format!("accept: {MEDIA_TYPE}").to_ascii_lowercase()));
        server.join().unwrap();
    }

    #[test]
    fn typed_claim_uses_media_type_string_u64_and_encoded_host() {
        let transport = ScriptedTransport::default();
        transport.push_json(
            200,
            serde_json::json!({
                "schema_version":"1", "attempts":[], "remaining_due":false,
                "remaining_incompatible":false, "server_time":timestamp()
            }),
        );
        let host = OpaqueId::new("host/one").unwrap();
        let request = ClaimRequest {
            instance_id: ProtocolUuid::from_uuid(uuid::Uuid::nil()),
            host_fence: DecimalU64(u64::MAX),
            available_addresses: vec![OpaqueId::new("addr").unwrap()],
            max_attempts: DecimalU64(1),
            max_entries: DecimalU64(2),
            max_content_bytes: DecimalU64(3),
            attempt_lease_seconds: DecimalU64(30),
        };
        let client = CogClient::new(transport);
        client.claim(&host, &request).unwrap();
        let sent = client.transport.requests.lock().unwrap();
        assert_eq!(
            sent[0].path_and_query,
            "/v1/runtime-hosts/host%2Fone/delivery-attempts:claim"
        );
        assert_eq!(sent[0].headers["accept"], MEDIA_TYPE);
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(body["host_fence"], u64::MAX.to_string());
        assert!(body["host_fence"].is_string());
    }

    #[test]
    fn open_attempt_pagination_preserves_opaque_cursor() {
        let transport = ScriptedTransport::default();
        transport.push_json(
            200,
            serde_json::json!({
                "schema_version":"1", "attempts":[], "next_after":null,
                "server_time":timestamp()
            }),
        );
        let client = CogClient::new(transport);
        client
            .list_open_attempts(
                &OpaqueId::new("host").unwrap(),
                25,
                Some(&PageCursor::new("next page/+?").unwrap()),
            )
            .unwrap();
        let sent = client.transport.requests.lock().unwrap();
        assert_eq!(
            sent[0].path_and_query,
            "/v1/runtime-hosts/host/delivery-attempts?state=open&limit=25&after=next%20page%2F%2B%3F"
        );
    }

    #[test]
    fn api_error_union_is_preserved() {
        let transport = ScriptedTransport::default();
        transport.push_json(
            409,
            serde_json::json!({"error":{
                "code":"retired_address", "message":"retired", "retryable":false, "details":{}
            }}),
        );
        let client = CogClient::new(transport);
        let err = client
            .get_delivery_owner(&OpaqueId::new("a").unwrap())
            .unwrap_err();
        assert!(matches!(
            err,
            ClientError::Api {
                status: 409,
                error: ApiError {
                    code: ErrorCode::RetiredAddress,
                    ..
                }
            }
        ));
    }

    #[test]
    fn wake_stream_resumes_and_requires_matching_frame_id() {
        let transport = ScriptedTransport::default();
        let data = format!(
            "{{\"schema_version\":\"1\",\"wake_id\":\"9007199254740993\",\"host_id\":\"h\",\"due_since\":\"{}\",\"due_addresses\":[\"a\"],\"reason\":\"both\"}}",
            timestamp()
        );
        transport.push_stream(&format!(
            ": keepalive\n\nevent: delivery-ready\nid: 9007199254740993\ndata: {data}\n\n"
        ));
        let client = CogClient::new(transport);
        let mut stream = client
            .open_wakes(
                &OpaqueId::new("h").unwrap(),
                ProtocolUuid::from_uuid(uuid::Uuid::nil()),
                DecimalU64(7),
                Some(DecimalU64(6)),
            )
            .unwrap();
        let wake = stream.next_wake().unwrap().unwrap();
        assert_eq!(wake.wake_id.0, 9_007_199_254_740_993);
        assert_eq!(stream.last_event_id(), Some(wake.wake_id));
        assert!(stream.next_wake().unwrap().is_none());
        let sent = client.transport.requests.lock().unwrap();
        assert_eq!(sent[0].headers["last-event-id"], "6");
    }

    #[test]
    fn incompatible_content_type_fails_before_decode() {
        let transport = ScriptedTransport::default();
        transport.responses.lock().unwrap().push_back(HttpResponse {
            status: 200,
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            body: b"{}".to_vec(),
        });
        let client = CogClient::new(transport);
        assert!(matches!(
            client.probe_capabilities(),
            Err(ClientError::InvalidContentType { .. })
        ));
    }

    #[test]
    fn production_transport_rejects_non_loopback_urls() {
        assert!(UreqTransport::new("http://127.0.0.1:7666").is_ok());
        assert!(UreqTransport::new("http://localhost:7666").is_ok());
        assert!(UreqTransport::new("https://example.com").is_err());
        assert!(UreqTransport::new("http://10.0.0.1:7666").is_err());
    }
}
