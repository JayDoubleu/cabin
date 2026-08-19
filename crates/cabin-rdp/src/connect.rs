//! Establishing an RDP connection to a Cabin guest.
//!
//! `gnome-remote-desktop` refuses a plain TLS handshake and answers
//! `HYBRID_REQUIRED_BY_SERVER`, so CredSSP is mandatory. See DESIGN.md
//! section 12.4. The connector is configured accordingly: `enable_credssp` is
//! on, and `enable_tls` is off, which is what IronRDP recommends when the
//! server can do NLA.

use crate::session::Credentials;
use crate::{Error, Result};
use ironrdp::connector::{
    self, ClientConnector, ConnectionResult, Credentials as RdpCredentials, DesktopSize,
};
use ironrdp::pdu::{gcc, rdp::capability_sets};
use ironrdp_async::{connect_begin, connect_finalize, mark_as_upgraded, NetworkClient};
use ironrdp_tokio::TokioFramed;
use tokio::net::TcpStream;

/// Where to connect, and as whom.
#[derive(Debug, Clone)]
pub struct ConnectionParams {
    pub host: String,
    pub port: u16,
    pub credentials: Credentials,
    pub width: u32,
    pub height: u32,
}

impl ConnectionParams {
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// A connected session, plus what the server told us about it.
pub struct Connection {
    pub result: ConnectionResult,
    pub framed: TokioFramed<ironrdp_tls::TlsStream<TcpStream>>,
}

/// Kerberos is not used. Cabin authenticates with NTLM over CredSSP, so no
/// network requests to a KDC are ever needed. Returning an error here is
/// correct: if IronRDP ever asks, something has gone wrong.
struct NoKerberos;

impl NetworkClient for NoKerberos {
    async fn send(
        &mut self,
        _request: &connector::sspi::generator::NetworkRequest,
    ) -> connector::ConnectorResult<Vec<u8>> {
        Err(connector::custom_err!(
            "Kerberos",
            std::io::Error::other("Cabin authenticates with NTLM, not Kerberos")
        ))
    }
}

/// Connects to a guest and completes the whole RDP connection sequence.
pub async fn connect(params: &ConnectionParams) -> Result<Connection> {
    let address = params.address();
    let stream = TcpStream::connect(&address)
        .await
        .map_err(|source| Error::Connect {
            address: address.clone(),
            source,
        })?;

    let client_addr = stream.local_addr().map_err(|source| Error::Connect {
        address: address.clone(),
        source,
    })?;

    let mut framed = TokioFramed::new(stream);
    let mut connector = ClientConnector::new(build_config(params), client_addr);

    let should_upgrade = connect_begin(&mut framed, &mut connector)
        .await
        .map_err(|e| map_connector_error("during the connection request", e))?;

    // The TCP stream is handed to TLS here. Anything buffered by the framed
    // reader would be lost, so IronRDP requires taking the stream back out.
    let (initial_stream, leftover) = framed.into_inner();
    debug_assert!(
        leftover.is_empty(),
        "the connector should consume every byte before the TLS upgrade"
    );

    let (tls_stream, server_certificate) = ironrdp_tls::upgrade(initial_stream, &params.host)
        .await
        .map_err(|source| Error::Connect {
            address: address.clone(),
            source,
        })?;

    // CredSSP binds the authentication to this certificate's public key, which
    // is what stops a machine in the middle relaying the exchange.
    let server_public_key = ironrdp_tls::extract_tls_server_public_key(&server_certificate)
        .ok_or_else(|| Error::Closed("the server certificate carries no public key".into()))?
        .to_vec();

    let upgraded = mark_as_upgraded(should_upgrade, &mut connector);
    let mut framed = TokioFramed::new(tls_stream);

    let result = connect_finalize(
        upgraded,
        connector,
        &mut framed,
        &mut NoKerberos,
        connector::ServerName::new(&params.host),
        server_public_key,
        None,
    )
    .await
    .map_err(|e| map_connector_error("during authentication", e))?;

    Ok(Connection { result, framed })
}

fn build_config(params: &ConnectionParams) -> connector::Config {
    connector::Config {
        desktop_size: DesktopSize {
            width: params.width as u16,
            height: params.height as u16,
        },
        desktop_scale_factor: 0,
        // Off on purpose. The guest requires NLA, and IronRDP recommends
        // disabling the legacy TLS path when talking to an NLA-capable server.
        enable_tls: false,
        enable_credssp: true,
        credentials: RdpCredentials::UsernamePassword {
            username: params.credentials.username.clone(),
            password: params.credentials.password.clone(),
        },
        domain: if params.credentials.domain.is_empty() {
            None
        } else {
            Some(params.credentials.domain.clone())
        },
        client_build: 0,
        client_name: "cabin".to_string(),
        keyboard_type: gcc::KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout: 0,
        ime_file_name: String::new(),
        bitmap: None,
        dig_product_id: String::new(),
        client_dir: String::new(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        platform: capability_sets::MajorPlatformType::UNIX,
        hardware_id: None,
        request_data: None,
        autologon: true,
        enable_audio_playback: false,
        performance_flags: Default::default(),
        license_cache: None,
        timezone_info: Default::default(),
        compression_type: None,
        enable_server_pointer: false,
        pointer_software_rendering: true,
        multitransport_flags: None,
    }
}

/// Turns an IronRDP connector error into something a user can act on.
fn map_connector_error(stage: &str, error: connector::ConnectorError) -> Error {
    // Keep the whole chain. The top-level message is often just "decode
    // error", while the cause underneath names the field that failed.
    let mut text = error.to_string();
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&error);
    while let Some(cause) = source {
        text.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    // The server reports a rejected password as a generic access denied, so
    // match on the text. Getting this wrong only costs a less precise message.
    if text.contains("LOGON_FAILURE")
        || text.contains("ACCESS_DENIED")
        || text.to_lowercase().contains("authentication")
    {
        return Error::AuthenticationFailed;
    }
    if text.contains("HYBRID_REQUIRED") {
        return Error::NlaRequired;
    }
    Error::Closed(format!("{stage}: {text}"))
}
