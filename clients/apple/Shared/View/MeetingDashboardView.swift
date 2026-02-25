import SwiftUI
import Network
#if canImport(AVFoundation)
import AVFoundation
#endif
#if os(macOS)
import CoreGraphics
#endif

struct MeetingDashboardView: View {
    @Environment(\.scenePhase) private var scenePhase
    @StateObject private var session = MeetingSession()
    @StateObject private var connectivity = NetworkConnectivityObserver()
    @StateObject private var audioSession = AudioSessionObserver()
    @StateObject private var screenCapture = ScreenCaptureCapabilityObserver()
    @State private var showFallbackSheet = false

    let platformTitle: String

    var body: some View {
        NavigationStack {
            ZStack {
                LinearGradient(
                    colors: [Color(red: 0.08, green: 0.12, blue: 0.22), Color(red: 0.04, green: 0.22, blue: 0.28)],
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                )
                .ignoresSafeArea()

                VStack(spacing: 16) {
                    header
                    configCard
                    sessionPolicyCard
                    controlRow
                    logCard
                }
                .padding(16)
            }
            .navigationTitle("Kaigi \(platformTitle)")
            .onChange(of: session.shouldShowFallback) { _, shouldShow in
                if shouldShow {
                    showFallbackSheet = true
                }
            }
            .onChange(of: scenePhase) { _, phase in
                switch phase {
                case .active:
                    screenCapture.refresh()
                    session.onScreenCaptureCapabilityChanged(
                        available: screenCapture.isAvailable,
                        source: "scene_active"
                    )
                    session.onAppForegrounded()
                case .background:
                    session.onAppBackgrounded()
                case .inactive:
                    break
                @unknown default:
                    break
                }
            }
            .onChange(of: connectivity.available) { _, available in
                session.onConnectivityChanged(available: available)
            }
            .onChange(of: audioSession.interruptionBeganToken) { _, token in
                guard token > 0 else { return }
                session.onAudioInterruptionBegan()
            }
            .onChange(of: audioSession.interruptionEndedToken) { _, token in
                guard token > 0 else { return }
                session.onAudioInterruptionEnded(shouldReconnect: audioSession.shouldReconnectAfterInterruption)
            }
            .onChange(of: audioSession.routeChangeToken) { _, token in
                guard token > 0 else { return }
                session.onAudioRouteChanged(reason: audioSession.lastRouteChangeReason)
            }
            .onChange(of: screenCapture.refreshToken) { _, token in
                guard token > 0 else { return }
                session.onScreenCaptureCapabilityChanged(
                    available: screenCapture.isAvailable,
                    source: "capability_refresh"
                )
            }
            .task {
                session.onConnectivityChanged(available: connectivity.available)
                screenCapture.refresh()
            }
            .sheet(isPresented: $showFallbackSheet, onDismiss: {
                if session.shouldShowFallback {
                    session.recoverFromFallback()
                }
            }) {
                if let url = session.config.fallbackURL {
                    NavigationStack {
                        WebFallbackView(url: url)
                    }
                }
            }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Direct Nexus Meeting Shell")
                .font(.system(size: 28, weight: .bold, design: .rounded))
                .foregroundStyle(.white)
            Text("Status: \(session.transportState)")
                .font(.system(size: 14, weight: .semibold, design: .rounded))
                .foregroundStyle(session.isConnected ? Color.green : Color.orange)
            if let error = session.lastErrorMessage {
                Text("Last Error: \(error)")
                    .font(.system(size: 12, weight: .regular, design: .rounded))
                    .foregroundStyle(.red.opacity(0.95))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var configCard: some View {
        VStack(spacing: 10) {
            editorField("Signaling URL", text: $session.config.signalingURLText)
            editorField("Fallback URL", text: $session.config.fallbackURLText)
            editorField("Room ID", text: $session.config.roomID)
            editorField("Participant", text: $session.config.participantName)
            editorField("Participant ID (optional)", text: participantIDBinding)
            policyToggle("Require Signed Moderation", value: $session.config.requireSignedModeration)
            policyToggle("Require Payment Settlement", value: $session.config.requirePaymentSettlement)
            policyToggle("Fallback On Policy Failure", value: $session.config.preferWebFallbackOnPolicyFailure)
        }
        .padding(14)
        .background(.white.opacity(0.12), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .strokeBorder(.white.opacity(0.18), lineWidth: 1)
        )
    }

    private func editorField(_ title: String, text: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.system(size: 12, weight: .semibold, design: .rounded))
                .foregroundStyle(.white.opacity(0.9))
#if os(iOS)
            TextField(title, text: text)
                .textFieldStyle(.plain)
                .padding(10)
                .background(.black.opacity(0.18), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .foregroundStyle(.white)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
#else
            TextField(title, text: text)
                .textFieldStyle(.plain)
                .padding(10)
                .background(.black.opacity(0.18), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .foregroundStyle(.white)
#endif
        }
    }

    private func policyToggle(_ title: String, value: Binding<Bool>) -> some View {
        Toggle(isOn: value) {
            Text(title)
                .font(.system(size: 12, weight: .semibold, design: .rounded))
                .foregroundStyle(.white.opacity(0.9))
        }
        .toggleStyle(.switch)
    }

    private var participantIDBinding: Binding<String> {
        Binding(
            get: { session.config.participantID ?? "" },
            set: { next in
                let trimmed = next.trimmingCharacters(in: .whitespacesAndNewlines)
                session.config.participantID = trimmed.isEmpty ? nil : trimmed
            }
        )
    }

    private var sessionPolicyCard: some View {
        let sessionState = session.sessionState
        return VStack(alignment: .leading, spacing: 6) {
            Text("Session Policy")
                .font(.system(size: 13, weight: .bold, design: .rounded))
                .foregroundStyle(.white)
            sessionLine("roomLocked=\(yesNo(sessionState.roomLocked)) waitingRoom=\(yesNo(sessionState.waitingRoomEnabled)) guestPolicy=\(sessionState.guestPolicy.rawValue)")
            sessionLine("e2eeRequired=\(yesNo(sessionState.e2eeRequired)) maxParticipants=\(sessionState.maxParticipants) policyEpoch=\(sessionState.policyEpoch)")
            sessionLine("recording=\(sessionState.recordingNotice.state.rawValue) media=\(sessionState.mediaProfile.preferredProfile.rawValue)/\(sessionState.mediaProfile.negotiatedProfile.rawValue)")
            sessionLine("paymentRequired=\(yesNo(sessionState.payment.required)) settlement=\(sessionState.payment.settlementStatus.rawValue)")
            if let destination = sessionState.payment.destination {
                sessionLine("paymentDestination=\(destination)")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(.black.opacity(0.24), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(.white.opacity(0.16), lineWidth: 1)
        )
    }

    private func sessionLine(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12, weight: .regular, design: .monospaced))
            .foregroundStyle(.white.opacity(0.9))
    }

    private var controlRow: some View {
        HStack(spacing: 10) {
            Button(session.isConnected ? "Reconnect" : "Connect") {
                session.connect()
            }
            .buttonStyle(.borderedProminent)
            .disabled(!session.config.isJoinable)

            Button("Ping") { session.sendPing() }
                .buttonStyle(.bordered)
                .disabled(!session.isConnected)

            Button("Disconnect") { session.disconnect() }
                .buttonStyle(.bordered)
                .disabled(!session.isConnected)

            Button("Open Web Fallback") { showFallbackSheet = true }
                .buttonStyle(.bordered)
                .disabled(session.config.fallbackURL == nil)
        }
        .tint(.mint)
    }

    private var logCard: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 6) {
                ForEach(session.logs) { log in
                    Text(log.formatted)
                        .font(.system(size: 12, weight: .regular, design: .monospaced))
                        .foregroundStyle(color(for: log.level))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.vertical, 1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.black.opacity(0.35), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .strokeBorder(.white.opacity(0.18), lineWidth: 1)
        )
    }

    private func color(for level: SessionLogLevel) -> Color {
        switch level {
        case .info: return .white
        case .warning: return .yellow
        case .error: return .red
        }
    }

    private func yesNo(_ value: Bool) -> String {
        value ? "yes" : "no"
    }
}

@MainActor
private final class NetworkConnectivityObserver: ObservableObject {
    @Published private(set) var available = true

    private let monitor: NWPathMonitor
    private let queue: DispatchQueue

    init(
        monitor: NWPathMonitor = NWPathMonitor(),
        queue: DispatchQueue = DispatchQueue(label: "io.sora.kaigi.apple.network")
    ) {
        self.monitor = monitor
        self.queue = queue
        monitor.pathUpdateHandler = { [weak self] path in
            Task { @MainActor in
                self?.available = (path.status == .satisfied)
            }
        }
        monitor.start(queue: queue)
    }

    deinit {
        monitor.cancel()
    }
}

@MainActor
private final class AudioSessionObserver: ObservableObject {
    @Published private(set) var interruptionBeganToken = 0
    @Published private(set) var interruptionEndedToken = 0
    @Published private(set) var shouldReconnectAfterInterruption = true
    @Published private(set) var routeChangeToken = 0
    @Published private(set) var lastRouteChangeReason = "unknown"

#if canImport(AVFoundation) && (os(iOS) || os(visionOS))
    private var observers: [NSObjectProtocol] = []
#endif

    init() {
#if canImport(AVFoundation) && (os(iOS) || os(visionOS))
        let center = NotificationCenter.default
        observers.append(
            center.addObserver(
                forName: AVAudioSession.interruptionNotification,
                object: nil,
                queue: .main
            ) { [weak self] notification in
                self?.handleInterruption(notification)
            }
        )
        observers.append(
            center.addObserver(
                forName: AVAudioSession.routeChangeNotification,
                object: nil,
                queue: .main
            ) { [weak self] notification in
                self?.handleRouteChange(notification)
            }
        )
#endif
    }

    deinit {
#if canImport(AVFoundation) && (os(iOS) || os(visionOS))
        let center = NotificationCenter.default
        for observer in observers {
            center.removeObserver(observer)
        }
#endif
    }

#if canImport(AVFoundation) && (os(iOS) || os(visionOS))
    private func handleInterruption(_ notification: Notification) {
        guard
            let value = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
            let type = AVAudioSession.InterruptionType(rawValue: value)
        else {
            return
        }

        switch type {
        case .began:
            interruptionBeganToken += 1
        case .ended:
            let optionsValue = notification.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
            let options = AVAudioSession.InterruptionOptions(rawValue: optionsValue)
            shouldReconnectAfterInterruption = options.contains(.shouldResume)
            interruptionEndedToken += 1
        @unknown default:
            break
        }
    }

    private func handleRouteChange(_ notification: Notification) {
        let value = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt
        let reason = value
            .flatMap { AVAudioSession.RouteChangeReason(rawValue: $0) }
            .map(routeChangeReasonLabel)
            ?? "unknown"
        lastRouteChangeReason = reason
        routeChangeToken += 1
    }

    private func routeChangeReasonLabel(_ reason: AVAudioSession.RouteChangeReason) -> String {
        switch reason {
        case .unknown: return "unknown"
        case .newDeviceAvailable: return "new_device_available"
        case .oldDeviceUnavailable: return "old_device_unavailable"
        case .categoryChange: return "category_change"
        case .override: return "override"
        case .wakeFromSleep: return "wake_from_sleep"
        case .noSuitableRouteForCategory: return "no_suitable_route"
        case .routeConfigurationChange: return "route_configuration_change"
        @unknown default: return "unknown"
        }
    }
#endif
}

@MainActor
private final class ScreenCaptureCapabilityObserver: ObservableObject {
    @Published private(set) var isAvailable = true
    @Published private(set) var refreshToken = 0

    func refresh() {
#if os(macOS)
        isAvailable = CGPreflightScreenCaptureAccess()
#else
        isAvailable = true
#endif
        refreshToken += 1
    }
}
