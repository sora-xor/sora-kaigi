import XCTest
@testable import KaigiMacOS

final class KaigiMacOSTests: XCTestCase {
    func testMeetingConfigAcceptsValidWebSocketURL() {
        let config = MeetingConfig.default
        XCTAssertEqual(config.signalingURL?.scheme, "ws")
        XCTAssertTrue(config.isJoinable)
    }

    func testProtocolCodecRejectsLegacyRawJoinFrame() {
        XCTAssertThrowsError(try MeetingProtocolCodec.decode("JOIN room=daily participant=Alice"))
    }

    func testFallbackRecoveryTracksRto() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        state = reducer.reduce(
            state: state,
            event: .fallbackActivated(reason: "native-degraded"),
            now: Date(timeIntervalSince1970: 10)
        )

        state = reducer.reduce(
            state: state,
            event: .fallbackRecovered,
            now: Date(timeIntervalSince1970: 13)
        )

        XCTAssertFalse(state.fallback.active)
        XCTAssertEqual(state.fallback.lastRtoMs, 3_000)
        XCTAssertEqual(state.connectionState, .disconnected)
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
    var disconnectReasons: [String] = []

    func connect(to _: URL) {}

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
