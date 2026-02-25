import Foundation

protocol MeetingSessionPersistence {
    func loadConfig() -> MeetingConfig?
    func saveConfig(_ config: MeetingConfig)
    func loadResumeToken() -> String?
    func saveResumeToken(_ token: String?)
    func loadFallbackActive() -> Bool
    func saveFallbackActive(_ active: Bool)
    func loadFallbackReason() -> String?
    func saveFallbackReason(_ reason: String?)
}

final class UserDefaultsMeetingSessionPersistence: MeetingSessionPersistence {
    private let defaults: UserDefaults
    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func loadConfig() -> MeetingConfig? {
        guard let raw = defaults.data(forKey: Keys.configData) else {
            return nil
        }
        return try? decoder.decode(MeetingConfig.self, from: raw)
    }

    func saveConfig(_ config: MeetingConfig) {
        guard let encoded = try? encoder.encode(config) else {
            return
        }
        defaults.set(encoded, forKey: Keys.configData)
    }

    func loadResumeToken() -> String? {
        defaults.string(forKey: Keys.resumeToken)
    }

    func saveResumeToken(_ token: String?) {
        if let token {
            defaults.set(token, forKey: Keys.resumeToken)
        } else {
            defaults.removeObject(forKey: Keys.resumeToken)
        }
    }

    func loadFallbackActive() -> Bool {
        defaults.bool(forKey: Keys.fallbackActive)
    }

    func saveFallbackActive(_ active: Bool) {
        defaults.set(active, forKey: Keys.fallbackActive)
    }

    func loadFallbackReason() -> String? {
        defaults.string(forKey: Keys.fallbackReason)
    }

    func saveFallbackReason(_ reason: String?) {
        if let reason {
            defaults.set(reason, forKey: Keys.fallbackReason)
        } else {
            defaults.removeObject(forKey: Keys.fallbackReason)
        }
    }

    private enum Keys {
        static let configData = "kaigi.meeting.config"
        static let resumeToken = "kaigi.meeting.session.resume_token"
        static let fallbackActive = "kaigi.meeting.session.fallback_active"
        static let fallbackReason = "kaigi.meeting.session.fallback_reason"
    }
}
