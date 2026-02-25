use kaigi_linux_client::{ConnectionPhase, MeetingConfig, SessionRuntime};

fn main() {
    let mut runtime = SessionRuntime::new(MeetingConfig::default());
    runtime.connect_requested(0);
    let outbound = runtime.on_transport_connected(1);

    println!(
        "kaigi-linux-client phase={:?} outbound_handshake_frames={}",
        runtime.state().connection_phase,
        outbound.len()
    );
    assert_eq!(
        runtime.state().connection_phase,
        ConnectionPhase::Connecting
    );
}
