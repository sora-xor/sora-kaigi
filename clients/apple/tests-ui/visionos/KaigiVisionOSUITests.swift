import XCTest

final class KaigiVisionOSUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testDashboardFlow() throws {
        let app = XCUIApplication()
        app.launch()

        XCTAssertTrue(app.staticTexts["kaigi.header.title"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["kaigi.status.label"].waitForExistence(timeout: 10))

        let connect = app.buttons["kaigi.controls.connect"]
        XCTAssertTrue(connect.waitForExistence(timeout: 5))
        connect.tap()

        XCTAssertTrue(app.staticTexts["kaigi.session.e2ee_line"].waitForExistence(timeout: 5))
    }
}
