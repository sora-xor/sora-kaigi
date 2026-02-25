import XCTest
@testable import KaigiVisionOS

final class KaigiVisionOSTests: XCTestCase {
    func testMeetingConfigRequiresValidSignalingURL() {
        let config = MeetingConfig(
            signalingURLText: "http://relay.example.com/ws",
            fallbackURLText: "https://fallback.example.com",
            roomID: "vision-room",
            participantName: "Alice"
        )

        XCTAssertNil(config.signalingURL)
        XCTAssertFalse(config.isJoinable)
    }

    func testProtocolCodecRejectsLegacyRawJoinFrame() {
        XCTAssertThrowsError(try MeetingProtocolCodec.decode("JOIN room=daily participant=Alice"))
    }

    func testReducerPreservesFallbackStateAfterTransportFailure() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        state = reducer.reduce(
            state: state,
            event: .fallbackActivated(reason: "native-degraded"),
            now: Date(timeIntervalSince1970: 10)
        )
        state = reducer.reduce(
            state: state,
            event: .transportFailure(message: "network_down"),
            now: Date(timeIntervalSince1970: 11)
        )

        XCTAssertTrue(state.fallback.active)
        XCTAssertEqual(state.connectionState, .fallbackActive)
        XCTAssertEqual(state.lastError?.code, "fallback_activated")
    }

    func testReducerHandlesWaitingRoomAdmit() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)
        state.participants["host"] = MeetingParticipant(
            id: "host",
            displayName: "Host",
            role: .host,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: false
        )
        state.participants["guest"] = MeetingParticipant(
            id: "guest",
            displayName: "Guest",
            role: .participant,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: true
        )

        state = reducer.reduce(
            state: state,
            event: .frameReceived(
                MeetingProtocolFrame(
                    kind: .moderationSigned,
                    moderationSigned: ModerationSignedFrame(
                        sentAtMs: 1,
                        targetParticipantID: "guest",
                        action: .admitFromWaiting,
                        issuedBy: "host",
                        signature: "sig-vision"
                    )
                )
            ),
            now: Date(timeIntervalSince1970: 2)
        )

        XCTAssertEqual(state.participants["guest"]?.waitingRoom, false)
    }

    @MainActor
    func testMeetingSessionAudioInterruptionReconnectsWhenEnabled() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: [60.0]
        )

        session.connect()
        XCTAssertEqual(client.connectURLs.count, 1)

        client.emit(.connected)
        await Task.yield()

        session.onAudioInterruptionBegan()
        await Task.yield()
        XCTAssertEqual(session.sessionState.connectionState, .degraded)

        session.onAudioInterruptionEnded(shouldReconnect: true)
        await Task.yield()
        XCTAssertEqual(client.connectURLs.count, 2)
    }

    @MainActor
    func testMeetingSessionPolicyFailureActivatesFallbackWhenPreferred() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: []
        )

        var config = session.config
        config.preferWebFallbackOnPolicyFailure = true
        session.config = config

        client.emit(
            .frameReceived(
                .error(category: .policyFailure, code: "policy_reject", message: "blocked by policy")
            )
        )
        await Task.yield()

        XCTAssertTrue(session.shouldShowFallback)
        XCTAssertTrue(session.sessionState.fallback.active)
        XCTAssertEqual(session.sessionState.connectionState, .fallbackActive)
        XCTAssertEqual(client.disconnectReasons.last, "fallback_activated")
    }
}

private final class FakeMeetingProtocolClient: MeetingProtocolClient {
    var eventHandler: ((MeetingProtocolClientEvent) -> Void)?
    var connectURLs: [URL] = []
    var disconnectReasons: [String] = []

    func connect(to url: URL) {
        connectURLs.append(url)
    }

    func disconnect(reason: String) {
        disconnectReasons.append(reason)
    }

    func send(frame _: MeetingProtocolFrame) {}

    func emit(_ event: MeetingProtocolClientEvent) {
        eventHandler?(event)
    }
}

private final class InMemoryMeetingSessionPersistence: MeetingSessionPersistence {
    var config: MeetingConfig?
    var resumeToken: String?
    var fallbackActive = false
    var fallbackReason: String?

    func loadConfig() -> MeetingConfig? {
        config
    }

    func saveConfig(_ config: MeetingConfig) {
        self.config = config
    }

    func loadResumeToken() -> String? {
        resumeToken
    }

    func saveResumeToken(_ token: String?) {
        resumeToken = token
    }

    func loadFallbackActive() -> Bool {
        fallbackActive
    }

    func saveFallbackActive(_ active: Bool) {
        fallbackActive = active
    }

    func loadFallbackReason() -> String? {
        fallbackReason
    }

    func saveFallbackReason(_ reason: String?) {
        fallbackReason = reason
    }
}
