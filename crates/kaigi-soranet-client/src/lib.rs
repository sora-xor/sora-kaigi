use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::Path,
    sync::Arc,
};

use anyhow::{Context as _, Result, anyhow};
use iroha_crypto::{
    KeyPair,
    soranet::handshake::{
        DEFAULT_CLIENT_CAPABILITIES, DEFAULT_DESCRIPTOR_COMMIT, DEFAULT_RELAY_CAPABILITIES,
        RuntimeParams, SessionSecrets, build_client_hello, client_handle_relay_hello,
    },
};
use quinn::crypto::rustls::QuicClientConfig;
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, SendStream};
use rand_core::{OsRng, TryRngCore as _};
use rustls::pki_types::{CertificateDer, pem::PemObject as _};
use serde::Deserialize;

const ROUTE_OPEN_FRAME_LEN: usize = 34;
const KAIGI_STREAM_TAG: u8 = 0x02;
const ROUTE_FLAG_AUTHENTICATED: u8 = 0x01;
const KAIGI_ROOM_DOMAIN_TAG: &[u8] = b"soranet.kaigi.room_id.v1";

/// Derive the same `room_id` that `soranet-relay` computes for Kaigi streams.
///
/// This is useful for debugging / local dev harnesses (hub adapters, route catalogs).
#[must_use]
pub fn derive_kaigi_room_id(
    channel_id: &[u8; 32],
    route_id: &[u8; 32],
    stream_id: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(KAIGI_ROOM_DOMAIN_TAG);
    hasher.update(channel_id);
    hasher.update(route_id);
    hasher.update(stream_id);
    *hasher.finalize().as_bytes()
}

#[derive(Clone, Debug)]
pub struct HandshakeParams {
    pub descriptor_commit: [u8; 32],
    pub client_capabilities: Vec<u8>,
    pub relay_capabilities: Vec<u8>,
    pub kem_id: u8,
    pub sig_id: u8,
    pub resume_hash: Option<Vec<u8>>,
}

impl HandshakeParams {
    pub fn fixture_defaults() -> Self {
        Self {
            descriptor_commit: DEFAULT_DESCRIPTOR_COMMIT,
            client_capabilities: DEFAULT_CLIENT_CAPABILITIES.to_vec(),
            relay_capabilities: DEFAULT_RELAY_CAPABILITIES.to_vec(),
            kem_id: 1,
            sig_id: 1,
            resume_hash: None,
        }
    }

    fn as_runtime_params(&self) -> RuntimeParams<'_> {
        RuntimeParams {
            descriptor_commit: self.descriptor_commit.as_slice(),
            client_capabilities: self.client_capabilities.as_slice(),
            relay_capabilities: self.relay_capabilities.as_slice(),
            kem_id: self.kem_id,
            sig_id: self.sig_id,
            resume_hash: self.resume_hash.as_deref(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RelayConnectOptions {
    pub relay_addr: SocketAddr,
    /// TLS SNI name to use for the QUIC connection.
    pub server_name: String,
    /// If set, accept any TLS certificate (dev-only).
    pub insecure: bool,
    /// Optional PEM bundle to trust as a root CA.
    pub ca_cert_pem_path: Option<std::path::PathBuf>,
    /// Optional admission token OR pow ticket frame, sent as the first handshake frame.
    pub handshake_prelude_frame: Option<Vec<u8>>,
    pub handshake: HandshakeParams,
}

pub struct RelaySession {
    _endpoint: Endpoint,
    pub connection: Connection,
    pub secrets: SessionSecrets,
}

pub async fn connect_and_handshake(opts: RelayConnectOptions) -> Result<RelaySession> {
    let bind_addr = match opts.relay_addr.ip() {
        IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    };
    let mut endpoint = Endpoint::client(bind_addr).context("create QUIC endpoint")?;

    let client_config = build_quic_client_config(opts.insecure, opts.ca_cert_pem_path.as_deref())
        .context("build QUIC client config")?;
    endpoint.set_default_client_config(client_config);

    let connecting = endpoint
        .connect(opts.relay_addr, &opts.server_name)
        .with_context(|| {
            format!(
                "connect to relay {} (sni={})",
                opts.relay_addr, opts.server_name
            )
        })?;
    let connection = connecting.await.context("await QUIC connection")?;

    let secrets = perform_soranet_handshake(
        &connection,
        &opts.handshake,
        opts.handshake_prelude_frame.as_deref(),
    )
    .await
    .context("perform SoraNet application handshake")?;

    Ok(RelaySession {
        _endpoint: endpoint,
        connection,
        secrets,
    })
}

pub async fn open_kaigi_stream(
    connection: &Connection,
    channel_id: [u8; 32],
    authenticated: bool,
) -> Result<(SendStream, RecvStream)> {
    if channel_id.iter().all(|&b| b == 0) {
        return Err(anyhow!("channel_id must not be all zeros"));
    }

    let (mut send, recv) = connection.open_bi().await.context("open QUIC stream")?;

    let mut header = [0u8; ROUTE_OPEN_FRAME_LEN];
    header[0] = KAIGI_STREAM_TAG;
    header[1] = if authenticated {
        ROUTE_FLAG_AUTHENTICATED
    } else {
        0
    };
    header[2..34].copy_from_slice(&channel_id);

    send.write_all(&header)
        .await
        .context("send RouteOpenFrame")?;

    Ok((send, recv))
}

pub fn decode_hex_32(s: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(s.trim()).context("decode hex")?;
    if bytes.len() != 32 {
        return Err(anyhow!("expected 32 bytes, got {}", bytes.len()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn decode_hex_vec(s: &str) -> Result<Vec<u8>> {
    hex::decode(s.trim()).context("decode hex")
}

pub async fn fetch_handshake_params_from_torii(torii_url: &str) -> Result<HandshakeParams> {
    #[derive(Debug, Deserialize)]
    struct ConfigGetDTO {
        network: Network,
    }
    #[derive(Debug, Deserialize)]
    struct Network {
        soranet_handshake: SoranetHandshake,
    }
    #[derive(Debug, Deserialize)]
    struct SoranetHandshake {
        descriptor_commit_hex: String,
        client_capabilities_hex: String,
        relay_capabilities_hex: String,
        kem_id: u8,
        sig_id: u8,
        resume_hash_hex: Option<String>,
    }

    let base = torii_url.trim_end_matches('/');
    let url = format!("{base}/v1/config");
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .context("GET /v1/config")?
        .error_for_status()
        .context("GET /v1/config status")?;
    let dto: ConfigGetDTO = resp.json().await.context("decode /v1/config json")?;

    let descriptor_commit = decode_hex_32(&dto.network.soranet_handshake.descriptor_commit_hex)
        .context("descriptor_commit_hex")?;
    let client_capabilities =
        decode_hex_vec(&dto.network.soranet_handshake.client_capabilities_hex)
            .context("client_capabilities_hex")?;
    let relay_capabilities = decode_hex_vec(&dto.network.soranet_handshake.relay_capabilities_hex)
        .context("relay_capabilities_hex")?;
    let resume_hash = dto
        .network
        .soranet_handshake
        .resume_hash_hex
        .as_deref()
        .map(decode_hex_vec)
        .transpose()
        .context("resume_hash_hex")?;

    Ok(HandshakeParams {
        descriptor_commit,
        client_capabilities,
        relay_capabilities,
        kem_id: dto.network.soranet_handshake.kem_id,
        sig_id: dto.network.soranet_handshake.sig_id,
        resume_hash,
    })
}

async fn perform_soranet_handshake(
    connection: &Connection,
    handshake: &HandshakeParams,
    prelude_frame: Option<&[u8]>,
) -> Result<SessionSecrets> {
    let runtime_params = handshake.as_runtime_params();
    let mut rng = OsRng.unwrap_err();
    let (client_hello, client_state) =
        build_client_hello(&runtime_params, &mut rng).context("build ClientHello")?;

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .context("open handshake bi-stream")?;
    if let Some(frame) = prelude_frame {
        write_handshake_frame(&mut send, frame)
            .await
            .context("write handshake prelude frame")?;
    }
    write_handshake_frame(&mut send, &client_hello)
        .await
        .context("write ClientHello frame")?;

    let relay_hello = read_handshake_frame(&mut recv)
        .await
        .context("read RelayHello frame")?;

    let client_key_pair = KeyPair::random();
    let (client_finish, secrets) = client_handle_relay_hello(
        client_state,
        &relay_hello,
        &client_key_pair,
        &runtime_params,
        &mut rng,
    )
    .context("handle RelayHello")?;

    if let Some(finish) = client_finish.as_deref() {
        write_handshake_frame(&mut send, finish)
            .await
            .context("write ClientFinish")?;
    }
    send.finish().context("finish handshake send stream")?;

    Ok(secrets)
}

async fn read_handshake_frame(recv: &mut RecvStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 2];
    recv.read_exact(&mut len_buf)
        .await
        .context("read handshake length")?;
    let len = u16::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload)
        .await
        .context("read handshake payload")?;
    Ok(payload)
}

async fn write_handshake_frame(send: &mut SendStream, payload: &[u8]) -> Result<()> {
    let len = u16::try_from(payload.len()).map_err(|_| anyhow!("handshake payload too large"))?;
    send.write_all(&len.to_be_bytes())
        .await
        .context("write handshake length")?;
    send.write_all(payload)
        .await
        .context("write handshake payload")?;
    Ok(())
}

fn build_quic_client_config(insecure: bool, ca_cert_path: Option<&Path>) -> Result<ClientConfig> {
    let mut crypto = if insecure {
        build_insecure_rustls_config().context("build insecure rustls config")?
    } else {
        build_rustls_config_with_ca(ca_cert_path).context("build rustls config with CA")?
    };

    // quinn needs TLS 1.3
    crypto.enable_early_data = true;

    Ok(ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(crypto).context("wrap rustls config for QUIC")?,
    )))
}

fn build_rustls_config_with_ca(ca_cert_path: Option<&Path>) -> Result<rustls::ClientConfig> {
    let Some(path) = ca_cert_path else {
        return Err(anyhow!(
            "TLS verification enabled but no --ca-cert-pem-path provided"
        ));
    };
    let data =
        std::fs::read(path).with_context(|| format!("read CA PEM bundle at {}", path.display()))?;

    let mut root_store = rustls::RootCertStore::empty();
    for entry in CertificateDer::pem_slice_iter(&data) {
        let cert = entry.context("parse PEM certificate")?.into_owned();
        root_store
            .add(cert)
            .context("add certificate to root store")?;
    }
    if root_store.is_empty() {
        return Err(anyhow!("no certificates found in CA bundle"));
    }

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Ok(config)
}

fn build_insecure_rustls_config() -> Result<rustls::ClientConfig> {
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

    /// Dev-only verifier: accept any certificate *chain* but still validate the TLS handshake
    /// signatures against whatever leaf cert was presented (matches quinn's own insecure example).
    #[derive(Debug)]
    struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

    impl SkipServerVerification {
        fn new() -> Arc<Self> {
            Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
        }
    }

    impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp: &[u8],
            _now: UnixTime,
        ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error>
        {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            rustls::crypto::verify_tls12_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            rustls::crypto::verify_tls13_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }

    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();
    Ok(config)
}
