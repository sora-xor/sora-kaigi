import XCTest

final class KaigiTVOSUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testDashboardFlow() throws {
        let app = XCUIApplication()
        app.launch()

        XCTAssertTrue(app.staticTexts["kaigi.header.title"].waitForExistence(timeout: 10))
        let status = app.staticTexts["kaigi.status.label"]
        XCTAssertTrue(status.waitForExistence(timeout: 10))

        let connect = app.buttons["kaigi.controls.connect"]
        XCTAssertTrue(connect.waitForExistence(timeout: 5))

        XCUIRemote.shared.press(.playPause)
        XCTAssertTrue(connect.waitForExistence(timeout: 5))

        XCTAssertTrue(app.staticTexts["kaigi.session.e2ee_line"].waitForExistence(timeout: 5))

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
