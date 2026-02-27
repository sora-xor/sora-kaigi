import XCTest
@testable import KaigiWatchOS

final class KaigiWatchOSTests: XCTestCase {
    func testMeetingConfigRequiresRoomID() {
        let config = MeetingConfig(
            signalingURLText: "wss://relay.example.com/ws",
            fallbackURLText: "https://fallback.example.com",
            roomID: "   ",
            participantName: "Watch User"
        )
        XCTAssertFalse(config.isJoinable)
    }

    func testProtocolCodecRejectsLegacyRawJoinFrame() {
        XCTAssertThrowsError(try MeetingProtocolCodec.decode("JOIN room=daily participant=Alice"))
    }

    @MainActor
    func testMeetingSessionConnectivityRestoreDefersWhileBackgrounded() async {
        let persistence = InMemoryMeetingSessionPersistence()
        let client = FakeMeetingProtocolClient()
        let session = MeetingSession(
            protocolClient: client,
            persistence: persistence,
            reconnectBackoffSeconds: [60.0]
        )

        session.onAppBackgrounded()
        session.onConnectivityChanged(available: true)
        await Task.yield()
        XCTAssertEqual(client.connectURLs.count, 0)

        session.onAppForegrounded()
        await Task.yield()
        XCTAssertEqual(client.connectURLs.count, 1)
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
