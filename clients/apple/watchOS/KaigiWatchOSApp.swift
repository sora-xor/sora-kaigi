import SwiftUI

@main
struct KaigiWatchOSApp: App {
    var body: some Scene {
        WindowGroup {
            MeetingDashboardView(platformTitle: "watchOS")
        }
    }
}
