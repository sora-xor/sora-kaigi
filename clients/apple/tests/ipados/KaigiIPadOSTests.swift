import XCTest
@testable import KaigiIPadOS

final class KaigiIPadOSTests: XCTestCase {
    func testMeetingConfigNeedsRoomID() {
        let config = MeetingConfig(
            signalingURLText: "wss://relay.example.com/ws",
            fallbackURLText: "https://fallback.example.com",
            roomID: "   ",
            participantName: "Alice"
        )
        XCTAssertNotNil(config.signalingURL)
        XCTAssertFalse(config.isJoinable)
    }

    func testProtocolCodecRejectsLegacyRawJoinFrame() {
        XCTAssertThrowsError(try MeetingProtocolCodec.decode("JOIN room=daily participant=Alice"))
    }

    func testPresenceDeltaSequenceIsMonotonic() {
        let reducer = DefaultMeetingStateReducer()
        var state = MeetingSessionState.initial(config: .default)

        let joined = MeetingParticipant(
            id: "p1",
            displayName: "Alice",
            role: .participant,
            muted: false,
            videoEnabled: true,
            shareEnabled: false,
            waitingRoom: false
        )

        let first = MeetingProtocolFrame(
            kind: .participantPresenceDelta,
            presenceDelta: ParticipantPresenceDeltaFrame(joined: [joined], left: [], roleChanges: [], sequence: 5)
        )

        let stale = MeetingProtocolFrame(
            kind: .participantPresenceDelta,
            presenceDelta: ParticipantPresenceDeltaFrame(joined: [], left: ["p1"], roleChanges: [], sequence: 4)
        )

        state = reducer.reduce(state: state, event: .frameReceived(first), now: Date())
        state = reducer.reduce(state: state, event: .frameReceived(stale), now: Date())

        XCTAssertEqual(state.presenceSequence, 5)
        XCTAssertNotNil(state.participants["p1"])
    }

    func testModerationDenyRemovesParticipant() {
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
        state.participants["p1"] = MeetingParticipant(
            id: "p1",
            displayName: "Alice",
            role: .participant,
            muted: false,
            videoEnabled: true,
            shareEnabled: true,
            waitingRoom: true
        )

        let moderation = MeetingProtocolFrame(
            kind: .moderationSigned,
            moderationSigned: ModerationSignedFrame(
                sentAtMs: 1,
                targetParticipantID: "p1",
                action: .denyFromWaiting,
                issuedBy: "host",
                signature: "sig-ipados"
            )
        )

        state = reducer.reduce(state: state, event: .frameReceived(moderation), now: Date())
        XCTAssertNil(state.participants["p1"])
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
