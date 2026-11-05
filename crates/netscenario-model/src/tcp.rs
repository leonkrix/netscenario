//! The TCP workloads: bulk transfers and requests with responses (WRK-04, WRK-05; D-086).
//!
//! They use the TCP of the machine's kernel, so a result about them is a result about that kernel (and the result says which).
//! Unlike the UDP workloads they are driven by feedback and are not packet-synchronous, so a run with one is not exact in the
//! way a seeded UDP run is; the result says so.

use schemars::JsonSchema;
use serde::{Serialize, Serializer};

use crate::ip::IpFamily;
use crate::units::{Duration, Name, Size};

/// The most connections of one TCP workload. Each is a thread and a socket in a node.
pub const MAX_CONNECTIONS: u32 = 1024;

/// The largest request or response, in bytes.
pub const MAX_MESSAGE_BYTES: u64 = 1 << 30;

/// The congestion control of a workload that does not name one (D-086): fixed and not the host's choice, so that one file means
/// one thing on two machines.
pub const DEFAULT_CONGESTION: &str = "cubic";

/// How long a request waits for its response when nothing else is said.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_nanos(30_000_000_000);

/// Data in one direction over one or more TCP connections, as fast as the network and the TCP of the kernel allow
/// (`tcp-bulk`): a download, an upload, a backup. Each connection sends `bytes`, or sends until the workload stops.
///
/// New keys are added to this type in later releases: use [`TcpBulk::new`] or read a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TcpBulk {
    /// The name of the workload. Metrics are named after it.
    pub id: Name,
    /// The node that sends.
    pub from: Name,
    /// The node that receives.
    pub to: Name,
    /// The payload that each connection sends. Without it each sends from `start` until `stop`.
    pub bytes: Option<Size>,
    /// How many connections. Default 1.
    pub connections: u32,
    /// The congestion control of the sending sockets, for example `cubic`, `reno` or `bbr`. Default [`DEFAULT_CONGESTION`]. A
    /// machine that lacks the one named refuses the scenario.
    pub cc: Option<Name>,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    pub start: Option<Duration>,
    /// When the workload stops. Default: the end of the measured phase. A connection that has `bytes` ends when it has sent
    /// them, if that is earlier; the data in flight at `stop` is still delivered, and the transfer counts as complete when the
    /// far end has read it.
    pub stop: Option<Duration>,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network, `v4` in a `dual` one.
    pub ip: Option<IpFamily>,
}

impl TcpBulk {
    /// A workload with one connection, the default congestion control and the whole measured phase.
    #[must_use]
    pub fn new(id: Name, from: Name, to: Name) -> Self {
        Self {
            id,
            from,
            to,
            bytes: None,
            connections: 1,
            cc: None,
            start: None,
            stop: None,
            ip: None,
        }
    }
}

/// Requests of one size, answered with responses of another (`request-response`): an API, a game server, a database. In a
/// closed loop each connection sends the next request when the response has arrived; with an `interval` requests are due at
/// fixed moments, whichever connection takes them, and the latency counts from the moment a request was due.
///
/// New keys are added to this type in later releases: use [`RequestResponse::new`] or read a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequestResponse {
    /// The name of the workload. Metrics are named after it.
    pub id: Name,
    /// The node that sends the requests.
    pub from: Name,
    /// The node that answers.
    pub to: Name,
    /// The size of a request.
    pub request: Size,
    /// The size of a response.
    pub response: Size,
    /// How many connections. Default 1.
    pub connections: u32,
    /// The time between two requests of the whole workload (an open loop). Without it the loop is closed.
    pub interval: Option<Duration>,
    /// How long a request waits for its response before it counts as failed. Default [`DEFAULT_TIMEOUT`].
    pub timeout: Option<Duration>,
    /// The congestion control of the sockets, for example `cubic`, `reno` or `bbr`. Default [`DEFAULT_CONGESTION`].
    pub cc: Option<Name>,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    pub start: Option<Duration>,
    /// When the workload stops: no request is started after it. Default: the end of the measured phase.
    pub stop: Option<Duration>,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network, `v4` in a `dual` one.
    pub ip: Option<IpFamily>,
}

impl RequestResponse {
    /// A workload with one connection, a closed loop and the whole measured phase.
    #[must_use]
    pub fn new(id: Name, from: Name, to: Name, request: Size, response: Size) -> Self {
        Self {
            id,
            from,
            to,
            request,
            response,
            connections: 1,
            interval: None,
            timeout: None,
            cc: None,
            start: None,
            stop: None,
            ip: None,
        }
    }
}

/// The number of connections as a file writes it: the default is left out.
fn written(connections: u32) -> Option<u32> {
    (connections != 1).then_some(connections)
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum TcpBulkKind {
    TcpBulk,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields, rename = "TcpBulk")]
struct TcpBulkFile<'a> {
    /// The name of the workload. Metrics are named after it.
    id: &'a Name,
    /// `tcp-bulk`: data over TCP as fast as the network allows.
    kind: TcpBulkKind,
    /// The node that sends.
    from: &'a Name,
    /// The node that receives.
    to: &'a Name,
    /// The payload that each connection sends. Without it each sends from `start` until `stop`.
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: &'a Option<Size>,
    /// How many connections, from 1 to 1024. Default 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    connections: Option<u32>,
    /// The congestion control of the sending sockets: `cubic` (default), `reno`, `bbr` or another that the machine has.
    #[serde(skip_serializing_if = "Option::is_none")]
    cc: &'a Option<Name>,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    #[serde(skip_serializing_if = "Option::is_none")]
    start: &'a Option<Duration>,
    /// When the workload stops. Default: the end of the measured phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: &'a Option<Duration>,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network.
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: &'a Option<IpFamily>,
}

impl Serialize for TcpBulk {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        TcpBulkFile {
            id: &self.id,
            kind: TcpBulkKind::TcpBulk,
            from: &self.from,
            to: &self.to,
            bytes: &self.bytes,
            connections: written(self.connections),
            cc: &self.cc,
            start: &self.start,
            stop: &self.stop,
            ip: &self.ip,
        }
        .serialize(serializer)
    }
}

impl JsonSchema for TcpBulk {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TcpBulk".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        TcpBulkFile::json_schema(generator)
    }
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum RequestResponseKind {
    RequestResponse,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields, rename = "RequestResponse")]
struct RequestResponseFile<'a> {
    /// The name of the workload. Metrics are named after it.
    id: &'a Name,
    /// `request-response`: requests of one size answered with responses of another, over TCP.
    kind: RequestResponseKind,
    /// The node that sends the requests.
    from: &'a Name,
    /// The node that answers.
    to: &'a Name,
    /// The size of a request, from 1 byte to 1 GiB.
    request: &'a Size,
    /// The size of a response, from 1 byte to 1 GiB.
    response: &'a Size,
    /// How many connections, from 1 to 1024. Default 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    connections: Option<u32>,
    /// The time between two requests of the whole workload (an open loop). Without it each connection sends the next request
    /// when the response has arrived (a closed loop).
    #[serde(skip_serializing_if = "Option::is_none")]
    interval: &'a Option<Duration>,
    /// How long a request waits for its response before it counts as failed. Default `30s`.
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: &'a Option<Duration>,
    /// The congestion control of the sockets: `cubic` (default), `reno`, `bbr` or another that the machine has.
    #[serde(skip_serializing_if = "Option::is_none")]
    cc: &'a Option<Name>,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    #[serde(skip_serializing_if = "Option::is_none")]
    start: &'a Option<Duration>,
    /// When the workload stops: no request is started after it. Default: the end of the measured phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: &'a Option<Duration>,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network.
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: &'a Option<IpFamily>,
}

impl Serialize for RequestResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RequestResponseFile {
            id: &self.id,
            kind: RequestResponseKind::RequestResponse,
            from: &self.from,
            to: &self.to,
            request: &self.request,
            response: &self.response,
            connections: written(self.connections),
            interval: &self.interval,
            timeout: &self.timeout,
            cc: &self.cc,
            start: &self.start,
            stop: &self.stop,
            ip: &self.ip,
        }
        .serialize(serializer)
    }
}

impl JsonSchema for RequestResponse {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RequestResponse".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        RequestResponseFile::json_schema(generator)
    }
}
