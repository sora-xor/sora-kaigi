import SwiftUI

@main
struct KaigiMacOSApp: App {
    var body: some Scene {
        WindowGroup {
            MeetingDashboardView(platformTitle: "macOS")
                .frame(minWidth: 980, minHeight: 680)
        }
    }
}
