import XCTest

final class KaigiWatchOSUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func waitUntil(
        timeout: TimeInterval,
        pollInterval: TimeInterval = 0.25,
        _ condition: () -> Bool
    ) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() {
                return true
            }

            RunLoop.current.run(until: Date().addingTimeInterval(pollInterval))
        }

        return condition()
    }

    func testDashboardFlow() throws {
        let app = XCUIApplication()
        app.launch()

        XCTAssertTrue(app.staticTexts["kaigi.header.title"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["kaigi.status.label"].waitForExistence(timeout: 10))

        let connect = app.buttons["kaigi.controls.connect"]
        XCTAssertTrue(connect.waitForExistence(timeout: 5))
        XCTAssertTrue(waitUntil(timeout: 5) { connect.exists && connect.isHittable })
        connect.tap()

        let e2eeLine = app.staticTexts["kaigi.session.e2ee_line"]
        XCTAssertTrue(waitUntil(timeout: 5) { e2eeLine.exists })

        let fallback = app.buttons["kaigi.controls.open_fallback"]
        XCTAssertTrue(fallback.waitForExistence(timeout: 5))
        XCTAssertFalse(fallback.isEnabled)
    }

    func testPolicyFailureFallbackDoesNotPresentUnsupportedSheet() throws {
        let app = XCUIApplication()
        app.launchEnvironment["KAIGI_UI_TEST_TRIGGER_POLICY_FAILURE"] = "1"
        app.launch()

        let fallbackNotice = app.staticTexts["kaigi.status.fallback_notice"]
        XCTAssertTrue(fallbackNotice.waitForExistence(timeout: 10))

        let fallback = app.buttons["kaigi.controls.open_fallback"]
        XCTAssertTrue(fallback.waitForExistence(timeout: 5))
        XCTAssertFalse(fallback.isEnabled)

        XCTAssertFalse(app.otherElements["kaigi.fallback.unsupported"].waitForExistence(timeout: 2))
        XCTAssertFalse(app.otherElements["kaigi.fallback.webview"].waitForExistence(timeout: 2))
    }
}
