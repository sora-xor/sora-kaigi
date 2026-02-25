import Foundation

enum MeetingTelemetryCategory: String, Equatable {
    case connectionLifecycle = "connection_lifecycle"
    case fallbackLifecycle = "fallback_lifecycle"
    case policyFailure = "policy_failure"
}

struct MeetingTelemetryEvent: Equatable {
    var category: MeetingTelemetryCategory
    var name: String
    var timestamp: Date
    var attributes: [String: String]

    init(
        category: MeetingTelemetryCategory,
        name: String,
        timestamp: Date = Date(),
        attributes: [String: String] = [:]
    ) {
        self.category = category
        self.name = name
        self.timestamp = timestamp
        self.attributes = attributes
    }
}

protocol MeetingTelemetrySink {
    func record(_ event: MeetingTelemetryEvent)
}

struct NoOpMeetingTelemetrySink: MeetingTelemetrySink {
    func record(_ event: MeetingTelemetryEvent) {}
}
