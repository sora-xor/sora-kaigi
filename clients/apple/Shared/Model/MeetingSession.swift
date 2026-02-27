import Foundation
import CryptoKit

enum SessionLogLevel: String {
    case info = "INFO"
    case warning = "WARN"
    case error = "ERROR"
}

struct SessionLogEntry: Identifiable {
    let id = UUID()
    let timestamp = Date()
    let level: SessionLogLevel
    let message: String

    var formatted: String {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return "[\(formatter.string(from: timestamp))] \(level.rawValue): \(message)"
    }
}

@MainActor
final class MeetingSession: ObservableObject {
    @Published var config: MeetingConfig = .default {
        didSet {
            apply(event: .configUpdated(config))
            participantID = Self.resolvedParticipantID(from: config)
        }
    }

    @Published private(set) var sessionState: MeetingSessionState
    @Published private(set) var isConnected = false
    @Published private(set) var transportState = SessionConnectionState.disconnected.label
    @Published private(set) var logs: [SessionLogEntry] = []
    @Published private(set) var shouldShowFallback = false
    @Published private(set) var lastErrorMessage: String?

    private let reducer: MeetingStateReducer
    private let protocolClient: MeetingProtocolClient
    private let persistence: MeetingSessionPersistence
    private let telemetrySink: MeetingTelemetrySink
    private let reconnectBackoffSeconds: [Double]

    private var reconnectAttempt = 0
    private var reconnectTask: Task<Void, Never>?
    private var userInitiatedDisconnect = false
    private var appInBackground = false
    private var participantID: String

    init(
        reducer: MeetingStateReducer = DefaultMeetingStateReducer(),
        protocolClient: MeetingProtocolClient = URLSessionMeetingProtocolClient(),
        persistence: MeetingSessionPersistence = UserDefaultsMeetingSessionPersistence(),
        telemetrySink: MeetingTelemetrySink = NoOpMeetingTelemetrySink(),
        reconnectBackoffSeconds: [Double] = [1.0, 2.0, 4.0, 8.0]
    ) {
        let restoredConfig = persistence.loadConfig() ?? .default
        let restoredResumeToken = persistence.loadResumeToken()
        let restoredFallbackActive = persistence.loadFallbackActive()
        let restoredFallbackReason = persistence.loadFallbackReason()
        var restoredState = MeetingSessionState.initial(config: restoredConfig)
        restoredState.resumeToken = restoredResumeToken
        if restoredFallbackActive {
            restoredState.fallback.active = true
            restoredState.fallback.reason = restoredFallbackReason
            restoredState.connectionState = .fallbackActive
        }

        self.reducer = reducer
        self.protocolClient = protocolClient
        self.persistence = persistence
        self.telemetrySink = telemetrySink
        self.reconnectBackoffSeconds = reconnectBackoffSeconds
        self.config = restoredConfig
        self.participantID = Self.resolvedParticipantID(from: restoredConfig)
        self.sessionState = restoredState
        self.isConnected = restoredState.connectionState.isOnline
        self.transportState = restoredState.connectionState.label
        self.shouldShowFallback = restoredState.fallback.active
        wireProtocolCallbacks()
    }

    func connect() {
        connectInternal(resetBackoff: true, source: "manual")
    }

    func onAppForegrounded() {
        guard !userInitiatedDisconnect else { return }
        appInBackground = false
        emitConnectionTelemetry(name: "app_foregrounded")
        let phase = sessionState.connectionState
        guard phase == .disconnected || phase == .degraded || phase == .error else { return }
        append(.info, "Foreground resume connect")
        connectInternal(resetBackoff: false, source: "foreground")
    }

    func onAppBackgrounded() {
        appInBackground = true
        append(.info, "App moved to background")
        emitConnectionTelemetry(name: "app_backgrounded")
        guard !userInitiatedDisconnect else { return }
        let phase = sessionState.connectionState
        if phase == .connected || phase == .connecting || phase == .degraded {
            protocolClient.disconnect(reason: "app_backgrounded")
            apply(event: .transportDisconnected(reason: "app_backgrounded"))
        }
    }

    func onConnectivityChanged(available: Bool) {
        guard !userInitiatedDisconnect else { return }
        if available {
            append(.info, "Connectivity restored")
            emitConnectionTelemetry(name: "network_available")
            if appInBackground {
                append(.info, "Connectivity restore deferred while app is backgrounded")
                return
            }
            let phase = sessionState.connectionState
            if phase != .connected && phase != .connecting {
                connectInternal(resetBackoff: false, source: "connectivity_restore")
            }
        } else {
            append(.warning, "Connectivity lost")
            emitConnectionTelemetry(name: "network_unavailable")
            apply(event: .transportFailure(message: "network_unavailable"))
            scheduleReconnect(trigger: "network_unavailable")
        }
    }

    func onAudioInterruptionBegan() {
        guard !userInitiatedDisconnect else { return }
        append(.warning, "Audio interruption began")
        emitConnectionTelemetry(name: "audio_interruption_began")
        apply(event: .transportFailure(message: "audio_interruption"))
    }

    func onAudioInterruptionEnded(shouldReconnect: Bool = true) {
        guard !userInitiatedDisconnect else { return }
        append(.info, "Audio interruption ended")
        emitConnectionTelemetry(
            name: "audio_interruption_ended",
            attributes: ["should_reconnect": shouldReconnect ? "true" : "false"]
        )

        guard shouldReconnect else { return }
        guard !appInBackground else { return }
        guard !sessionState.fallback.active else { return }
        let phase = sessionState.connectionState
        if phase != .connected && phase != .connecting {
            connectInternal(resetBackoff: false, source: "audio_interruption_end")
        }
    }

    func onAudioRouteChanged(reason: String) {
        append(.info, "Audio route changed: \(reason)")
        emitConnectionTelemetry(
            name: "audio_route_changed",
            attributes: ["reason": reason]
        )
    }

    func onScreenCaptureCapabilityChanged(available: Bool, source: String) {
        append(
            available ? .info : .warning,
            "Screen capture capability: \(available ? "available" : "unavailable") (\(source))"
        )
        emitConnectionTelemetry(
            name: "screen_capture_capability",
            attributes: [
                "available": available ? "true" : "false",
                "source": source
            ]
        )
    }

    private func connectInternal(resetBackoff: Bool, source: String) {
        guard !appInBackground else {
            append(.info, "Connect deferred while app is backgrounded")
            return
        }
        guard let url = config.signalingURL else {
            append(.error, "Invalid signaling URL")
            return
        }
        guard config.isJoinable else {
            append(.warning, "Room ID and signaling URL are required")
            return
        }

        userInitiatedDisconnect = false
        cancelReconnect()
        if resetBackoff {
            reconnectAttempt = 0
        }

        apply(event: .connectRequested)
        emitConnectionTelemetry(
            name: "connect_attempt",
            attributes: ["source": source]
        )
        if source == "manual" {
            append(.info, "Opening socket to \(url.absoluteString)")
        } else {
            append(.info, "Opening socket (\(source)) to \(url.absoluteString)")
        }
        protocolClient.connect(to: url)
    }

    func disconnect() {
        userInitiatedDisconnect = true
        cancelReconnect()
        protocolClient.disconnect(reason: "user_requested")
        apply(event: .manualDisconnected)
        emitConnectionTelemetry(name: "manual_disconnect")
        append(.info, "Disconnected")
    }

    func sendPing() {
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000)
        protocolClient.send(frame: .ping(at: nowMs))
        append(.info, "Sent ping")
    }

    func sendModeration(action: ModerationAction, targetParticipantID: String) {
        guard canIssueModeration() else {
            rejectPolicy(code: "moderation_not_authorized", message: "Host/co-host role is required for moderation actions")
            return
        }

        let sentAtMs = Int64(Date().timeIntervalSince1970 * 1000)
        let frame = MeetingProtocolFrame(
            kind: .moderationSigned,
            moderationSigned: ModerationSignedFrame(
                sentAtMs: sentAtMs,
                targetParticipantID: targetParticipantID,
                action: action,
                issuedBy: participantID,
                signature: makeFrameSignature(
                    "moderationSigned",
                    participantID,
                    targetParticipantID,
                    action.rawValue,
                    String(sentAtMs)
                )
            )
        )
        protocolClient.send(frame: frame)
        append(.info, "Moderation action sent: \(action.rawValue) -> \(targetParticipantID)")
    }

    func recoverFromFallback() {
        shouldShowFallback = false
        apply(event: .fallbackRecovered)
        connect()
    }

#if DEBUG
    func simulatePolicyFailureForTesting(
        code: String = "policy_reject",
        message: String = "blocked by policy"
    ) {
        rejectPolicy(code: code, message: message)
    }
#endif

    private func wireProtocolCallbacks() {
        protocolClient.eventHandler = { [weak self] event in
            guard let self else { return }
            Task { @MainActor in
                self.handleProtocolEvent(event)
            }
        }
    }

    private func handleProtocolEvent(_ event: MeetingProtocolClientEvent) {
        switch event {
        case .connected:
            if appInBackground {
                protocolClient.disconnect(reason: "backgrounded_before_handshake")
                return
            }
            reconnectAttempt = 0
            apply(event: .transportConnected)
            emitConnectionTelemetry(name: "transport_connected")
            append(.info, "Socket connected")
            sendHandshakeFrames()

        case .disconnected(let reason):
            if userInitiatedDisconnect {
                return
            }
            apply(event: .transportDisconnected(reason: reason))
            emitConnectionTelemetry(
                name: "transport_disconnected",
                attributes: ["reason": reason]
            )
            append(.warning, "Socket disconnected: \(reason)")
            if appInBackground {
                append(.info, "Reconnect deferred while app is backgrounded")
                return
            }
            scheduleReconnect(trigger: reason)

        case .frameReceived(let frame):
            apply(event: .frameReceived(frame))
            if frame.kind == .ping {
                let nowMs = Int64(Date().timeIntervalSince1970 * 1000)
                protocolClient.send(frame: .pong(at: nowMs))
            }
            if frame.kind == .e2eeKeyEpoch,
               let epochFrame = frame.e2eeKeyEpoch,
               hasRequiredSignature(epochFrame.signature),
               sessionState.e2ee.currentEpoch >= epochFrame.epoch
            {
                let nowMs = Int64(Date().timeIntervalSince1970 * 1000)
                let ack = MeetingProtocolFrame(
                    kind: .keyRotationAck,
                    keyRotationAck: KeyRotationAckFrame(
                        participantID: participantID,
                        ackEpoch: epochFrame.epoch,
                        receivedAtMs: nowMs
                    )
                )
                protocolClient.send(frame: ack)
                apply(event: .frameReceived(ack))
            }
            if frame.kind == .error, let payload = frame.error {
                append(.error, "Protocol error [\(payload.code)]: \(payload.message)")
                if payload.category == .policyFailure && config.preferWebFallbackOnPolicyFailure {
                    activateFallback(reason: "Policy rejection: \(payload.message)")
                }
            } else {
                append(.info, "Recv frame: \(frame.kind.rawValue)")
            }

        case .rawTextReceived(let text):
            append(.info, "Recv raw: \(text)")

        case .sendFailed(let message):
            apply(event: .frameSendFailed(message: message))
            append(.error, "Send failed: \(message)")

        case .transportFailed(let message):
            if userInitiatedDisconnect {
                return
            }
            apply(event: .transportFailure(message: message))
            emitConnectionTelemetry(
                name: "transport_failed",
                attributes: ["message": message]
            )
            append(.error, "Transport failure: \(message)")
            if appInBackground {
                append(.info, "Reconnect deferred while app is backgrounded")
                return
            }
            scheduleReconnect(trigger: message)
        }
    }

    private func scheduleReconnect(trigger: String) {
        guard !userInitiatedDisconnect else { return }
        guard !appInBackground else { return }
        guard !sessionState.fallback.active else { return }
        guard reconnectTask == nil else { return }
        guard reconnectAttempt < reconnectBackoffSeconds.count else {
            activateFallback(reason: "Reconnect exhausted after \(reconnectAttempt) attempts: \(trigger)")
            return
        }

        let delay = reconnectBackoffSeconds[reconnectAttempt]
        let attemptNumber = reconnectAttempt + 1
        reconnectAttempt += 1

        append(.warning, "Scheduling reconnect attempt #\(attemptNumber) in \(String(format: "%.0f", delay))s")
        emitConnectionTelemetry(
            name: "reconnect_scheduled",
            attributes: [
                "attempt": String(attemptNumber),
                "delay_seconds": String(format: "%.0f", delay),
                "trigger": trigger
            ]
        )

        reconnectTask = Task { [weak self] in
            do {
                try await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            } catch {
                return
            }
            guard !Task.isCancelled else { return }
            await MainActor.run {
                guard let self else { return }
                self.reconnectTask = nil
                guard !Task.isCancelled else { return }
                guard !self.userInitiatedDisconnect else { return }
                guard !self.appInBackground else { return }
                guard let url = self.config.signalingURL else {
                    self.activateFallback(reason: "Reconnect aborted: invalid signaling URL")
                    return
                }
                self.apply(event: .connectRequested)
                self.emitConnectionTelemetry(
                    name: "reconnect_attempt",
                    attributes: ["attempt": String(attemptNumber)]
                )
                self.append(.info, "Reconnect attempt #\(attemptNumber)")
                self.protocolClient.connect(to: url)
            }
        }
    }

    private func activateFallback(reason: String) {
        guard !sessionState.fallback.active else { return }

        cancelReconnect()
        apply(event: .fallbackActivated(reason: reason))
        shouldShowFallback = true
        emitFallbackTelemetry(
            name: "fallback_activated",
            attributes: ["reason": reason]
        )
        append(.warning, "Fallback activated: \(reason)")
        protocolClient.disconnect(reason: "fallback_activated")
    }

    private func sendHandshakeFrames() {
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000)
        let preferredProfile = preferredProfileForRuntime()

        let handshake = HandshakeFrame(
            roomID: config.roomID,
            participantID: participantID,
            participantName: config.participantName,
            walletIdentity: config.walletIdentity,
            resumeToken: sessionState.resumeToken,
            preferredProfile: preferredProfile,
            hdrCapture: supportsHDRCapture(),
            hdrRender: supportsHDRRender(),
            sentAtMs: nowMs
        )

        protocolClient.send(frame: .handshake(handshake))

        let capability = DeviceCapabilityFrame(
            participantID: participantID,
            codecs: ["h264", "vp9"],
            hdrCapture: supportsHDRCapture(),
            hdrRender: supportsHDRRender(),
            maxStreams: 4,
            updatedAtMs: nowMs
        )

        protocolClient.send(frame: MeetingProtocolFrame(kind: .deviceCapability, deviceCapability: capability))

        if config.requirePaymentSettlement {
            let paymentPolicy = PaymentPolicyFrame(required: true, destinationAccount: "nexus://payment-policy")
            protocolClient.send(frame: MeetingProtocolFrame(kind: .paymentPolicy, paymentPolicy: paymentPolicy))
        }
    }

    private func preferredProfileForRuntime() -> MediaProfileKind {
        supportsHDRCapture() && supportsHDRRender() ? .hdr : .sdr
    }

    private func supportsHDRCapture() -> Bool {
        if let override = config.supportsHDRCapture {
            return override
        }
#if targetEnvironment(simulator)
        return false
#else
        return true
#endif
    }

    private func supportsHDRRender() -> Bool {
        if let override = config.supportsHDRRender {
            return override
        }
#if targetEnvironment(simulator)
        return false
#else
        return true
#endif
    }

    private func apply(event: MeetingSessionEvent) {
        let previous = sessionState
        sessionState = reducer.reduce(state: sessionState, event: event, now: Date())

        if previous.fallback.active && !sessionState.fallback.active, let rto = sessionState.fallback.lastRtoMs {
            append(.info, "Fallback recovered in \(rto) ms")
            emitFallbackTelemetry(
                name: "fallback_recovered",
                attributes: ["rto_ms": String(rto)]
            )
        }

        if previous.connectionState != sessionState.connectionState {
            emitConnectionTelemetry(
                name: "phase_transition",
                attributes: [
                    "from": previous.connectionState.label,
                    "to": sessionState.connectionState.label
                ]
            )
        }

        if let error = sessionState.lastError,
           error.category == .policyFailure,
           previous.lastError?.atMs != error.atMs
        {
            emitPolicyFailureTelemetry(code: error.code, message: error.message)
        }

        persistSessionSnapshot()

        if shouldTriggerFallbackFromPolicyFailure(previous: previous, current: sessionState) {
            activateFallback(reason: sessionState.lastError?.message ?? "policy_failure")
        }

        if sessionState.connectionState == .fallbackActive {
            shouldShowFallback = true
        }

        if let error = sessionState.lastError {
            lastErrorMessage = "\(error.code): \(error.message)"
        } else {
            lastErrorMessage = nil
        }

        isConnected = sessionState.connectionState.isOnline
        transportState = sessionState.connectionState.label
    }

    private func cancelReconnect() {
        reconnectTask?.cancel()
        reconnectTask = nil
    }

    private func append(_ level: SessionLogLevel, _ message: String) {
        logs.insert(SessionLogEntry(level: level, message: message), at: 0)
        if logs.count > 400 {
            logs.removeLast(logs.count - 400)
        }
    }

    private static func normalizedParticipantID(_ name: String) -> String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let base = trimmed.isEmpty ? "participant" : trimmed
        var normalized = String()
        normalized.reserveCapacity(base.count)

        for scalar in base.lowercased().unicodeScalars {
            let value = scalar.value
            if (value >= 97 && value <= 122) || (value >= 48 && value <= 57) || value == 45 || value == 95 {
                normalized.unicodeScalars.append(scalar)
            } else if CharacterSet.whitespacesAndNewlines.contains(scalar) {
                normalized.append("-")
            }
        }
        if normalized.isEmpty {
            return "participant"
        }
        return normalized
    }

    private static func resolvedParticipantID(from config: MeetingConfig) -> String {
        if let explicitID = config.participantID,
           !explicitID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        {
            return normalizedParticipantID(explicitID)
        }
        return normalizedParticipantID(config.participantName)
    }

    private func canIssueModeration() -> Bool {
        guard let actor = sessionState.participants[participantID] else {
            return false
        }
        return actor.role == .host || actor.role == .coHost
    }

    private func hasRequiredSignature(_ signature: String) -> Bool {
        if !config.requireSignedModeration {
            return true
        }
        return !signature.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func rejectPolicy(code: String, message: String) {
        apply(event: .frameReceived(.error(category: .policyFailure, code: code, message: message)))
        append(.error, "Policy reject [\(code)]: \(message)")
    }

    private func makeFrameSignature(_ components: String...) -> String {
        let payload = components.joined(separator: "|")
        let digest = SHA256.hash(data: Data(payload.utf8))
        return digest.map { String(format: "%02x", $0) }.joined()
    }

    private func shouldTriggerFallbackFromPolicyFailure(previous: MeetingSessionState, current: MeetingSessionState) -> Bool {
        guard config.preferWebFallbackOnPolicyFailure else { return false }
        guard !current.fallback.active else { return false }
        guard let error = current.lastError, error.category == .policyFailure else { return false }
        return previous.lastError?.atMs != error.atMs
    }

    private func persistSessionSnapshot() {
        persistence.saveConfig(config)
        persistence.saveResumeToken(sessionState.resumeToken)
        persistence.saveFallbackActive(sessionState.fallback.active)
        persistence.saveFallbackReason(sessionState.fallback.reason)
    }

    private func emitConnectionTelemetry(name: String, attributes: [String: String] = [:]) {
        telemetrySink.record(
            MeetingTelemetryEvent(
                category: .connectionLifecycle,
                name: name,
                attributes: attributes
            )
        )
    }

    private func emitFallbackTelemetry(name: String, attributes: [String: String] = [:]) {
        telemetrySink.record(
            MeetingTelemetryEvent(
                category: .fallbackLifecycle,
                name: name,
                attributes: attributes
            )
        )
    }

    private func emitPolicyFailureTelemetry(code: String, message: String) {
        telemetrySink.record(
            MeetingTelemetryEvent(
                category: .policyFailure,
                name: "policy_reject",
                attributes: [
                    "code": code,
                    "message": message
                ]
            )
        )
    }
}
