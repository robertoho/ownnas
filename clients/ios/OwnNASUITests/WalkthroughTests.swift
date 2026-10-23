import XCTest

/// Signs in to a test server and visits every screen, saving a screenshot of each.
///
///     TEST_RUNNER_OWNNAS_SERVER=127.0.0.1:8899 TEST_RUNNER_OWNNAS_USER=admin \
///     TEST_RUNNER_OWNNAS_PASSWORD=… TEST_RUNNER_OWNNAS_SHOTS=/tmp/shots \
///     xcodebuild test -scheme OwnNAS -destination 'platform=iOS Simulator,name=iPhone 17 Pro'
///
/// The library needs a folder "Docs" with a file containing "hello", and geotagged photos.
final class WalkthroughTests: XCTestCase {
    private var app: XCUIApplication!
    private var shots: URL?
    private var env: [String: String] { ProcessInfo.processInfo.environment }

    override func setUpWithError() throws {
        continueAfterFailure = false
        guard env["OWNNAS_SERVER"] != nil else { throw XCTSkip("Set TEST_RUNNER_OWNNAS_SERVER to run the walkthrough") }
        shots = env["OWNNAS_SHOTS"].map { URL(fileURLWithPath: $0) }
        app = XCUIApplication()
        app.launch()
    }

    private func shot(_ name: String) {
        let screenshot = XCUIScreen.main.screenshot()
        let attachment = XCTAttachment(screenshot: screenshot)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        if let shots { try? screenshot.pngRepresentation.write(to: shots.appendingPathComponent("\(name).png")) }
    }

    private func waitFor(_ element: XCUIElement, _ seconds: Double = 15, file: StaticString = #filePath, line: UInt = #line) {
        if !element.waitForExistence(timeout: seconds) {
            shot("failed-\(line)")
            XCTFail("missing: \(element)\n\(app.debugDescription)", file: file, line: line)
        }
    }

    /// Goes back until `element` shows.
    private func backTo(_ element: XCUIElement, file: StaticString = #filePath, line: UInt = #line) {
        for _ in 0..<4 where !element.waitForExistence(timeout: 2) {
            let back = app.navigationBars.buttons["BackButton"].exists ? app.navigationBars.buttons["BackButton"] : app.navigationBars.buttons.element(boundBy: 0)
            back.tap()
        }
        waitFor(element, file: file, line: line)
    }

    /// A list row, whose label starts with its title.
    private func row(_ title: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", title)).firstMatch
    }

    func testWalkthrough() throws {
        // Sign in.
        if app.buttons["Add server"].firstMatch.waitForExistence(timeout: 5) {
            app.buttons["Add server"].firstMatch.tap()
            let address = app.textFields["Server address"]
            waitFor(address)
            address.tap()
            address.typeText(env["OWNNAS_SERVER"]!)
            app.textFields["Username"].tap()
            app.textFields["Username"].typeText(env["OWNNAS_USER"] ?? "admin")
            app.secureTextFields["Password"].tap()
            app.secureTextFields["Password"].typeText(env["OWNNAS_PASSWORD"] ?? "")
            shot("01-sign-in")
            app.navigationBars.buttons["Sign in"].tap()
        } else {
            app.buttons.matching(NSPredicate(format: "label CONTAINS 'admin'")).firstMatch.tap()
        }

        // The library.
        waitFor(row("Docs"))
        shot("02-library")

        // Search inside files.
        app.navigationBars.buttons["More"].firstMatch.tap()
        app.buttons["Search"].tap()
        let field = app.searchFields.firstMatch
        waitFor(field)
        field.tap()
        field.typeText("hello\n")
        waitFor(row("notes.md"))
        shot("03-search")

        // Open the file in Quick Look.
        row("notes.md").tap()
        sleep(3)
        shot("04-quick-look")
        if app.buttons["Done"].exists { app.buttons["Done"].tap() } else { app.swipeDown() }
        backTo(row("Docs"))

        // The Gallery, with On this day.
        app.navigationBars.buttons["More"].firstMatch.tap()
        app.buttons["Gallery"].tap()
        waitFor(app.staticTexts["On this day"])
        sleep(2)
        shot("05-gallery")

        // Another workspace.
        app.buttons["Gallery"].firstMatch.tap()
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Family'")).firstMatch.tap()
        sleep(2)
        shot("06-workspace")

        // Places on a map.
        app.buttons["Places"].tap()
        sleep(4)
        shot("07-places")

        // Albums and tags.
        app.buttons["Albums"].tap()
        sleep(1)
        shot("08-albums")

        // A photo full screen.
        app.buttons["Photos"].tap()
        sleep(2)
        let photo = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'IMG_'")).firstMatch
        waitFor(photo)
        photo.tap()
        sleep(3)
        shot("09-photo")
        app.buttons["Details"].tap()
        sleep(2)
        shot("10-details")
        app.swipeDown(velocity: .fast)

        // A terminal (when the server has them on).
        backTo(row("Docs"))
        app.navigationBars.buttons["More"].firstMatch.tap()
        let terminals = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Terminals'")).firstMatch
        guard terminals.waitForExistence(timeout: 3) else { return }
        terminals.tap()
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'New terminal'")).firstMatch.tap()
        sleep(3)
        app.typeText("echo hello from the iPhone\n")
        sleep(2)
        shot("11-terminal")
    }
}
