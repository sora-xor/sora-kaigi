import Foundation

struct MeetingConfig: Codable, Equatable {
    var signalingURLText: String
    var fallbackURLText: String
    var roomID: String
    var participantID: String? = nil
    var participantName: String
    var walletIdentity: String? = nil
    var requireSignedModeration: Bool = true
    var requirePaymentSettlement: Bool = false
    var preferWebFallbackOnPolicyFailure: Bool = true
    var supportsHDRCapture: Bool? = nil
    var supportsHDRRender: Bool? = nil

    static let `default` = MeetingConfig(
        signalingURLText: "ws://127.0.0.1:9000",
        fallbackURLText: "https://127.0.0.1:8080",
        roomID: "daily-standup",
        participantID: nil,
        participantName: "Alice",
        walletIdentity: "nexus://wallet/alice",
        requireSignedModeration: true,
        requirePaymentSettlement: false,
        preferWebFallbackOnPolicyFailure: true,
        supportsHDRCapture: nil,
        supportsHDRRender: nil
    )

    var signalingURL: URL? {
        normalizedURL(signalingURLText)
    }

    var fallbackURL: URL? {
        normalizedURL(fallbackURLText)
    }

    var isJoinable: Bool {
        signalingURL != nil && !roomID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func normalizedURL(_ raw: String) -> URL? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let url = URL(string: trimmed), let scheme = url.scheme else {
            return nil
        }
        let allowed = ["ws", "wss", "http", "https"]
        guard allowed.contains(scheme.lowercased()) else {
            return nil
        }
        return url
    }
}
