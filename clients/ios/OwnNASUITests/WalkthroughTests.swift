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

    /// Signs in to the test server, unless this device is already.
    private func signIn() {
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
    }

    func testWalkthrough() throws {
        signIn()

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

        // The Gallery (with On this day, when a photo was taken on today's date another year).
        app.navigationBars.buttons["More"].firstMatch.tap()
        app.buttons["Gallery"].tap()
        waitFor(app.buttons["Photos"])
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

    /// Backs the simulator's photos up to the server (whose media upload folder must be set).
    func testPhotoBackup() throws {
        signIn()
        waitFor(row("Docs"))
        backTo(app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Photo backup'")).firstMatch)
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Photo backup'")).firstMatch.tap()
        let server = app.buttons.matching(NSPredicate(format: "label CONTAINS '127.0.0.1'")).firstMatch
        waitFor(server)
        server.tap()
        // iOS asks once whether the app may see the photos.
        let allow = XCUIApplication(bundleIdentifier: "com.apple.springboard").buttons["Allow Full Access"]
        if allow.waitForExistence(timeout: 5) { allow.tap() }
        let done = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Backed up' OR label == 'Up to date'")).firstMatch
        waitFor(done, 90)
        shot("12-backup")
        app.swipeUp()
        shot("13-backup-recent")
    }

    /// The server in the Files app: browse to Docs and see notes.md there.
    func testFilesApp() throws {
        signIn()
        waitFor(row("Docs"))
        let files = XCUIApplication(bundleIdentifier: "com.apple.DocumentsApp")
        files.launch()
        // Files comes back where it was left: close what is open and go to Browse's top.
        for _ in 0..<10 where !files.staticTexts["Locations"].exists {
            if files.buttons["QLOverlayDoneButtonAccessibilityIdentifier"].firstMatch.exists {
                files.buttons["QLOverlayDoneButtonAccessibilityIdentifier"].firstMatch.tap()
            } else if files.buttons["Browse"].firstMatch.exists {
                files.buttons["Browse"].firstMatch.tap()
            }
            sleep(1)
        }
        sleep(2)
        shotOf(files, "14-files-locations")
        // With one server Files names the location after the app; with several, after each server.
        let location = files.staticTexts["library"].exists ? files.staticTexts["library"].firstMatch : files.staticTexts["OwnNAS"].firstMatch
        if !location.waitForExistence(timeout: 10) {
            shotOf(files, "failed-files")
            XCTFail("no OwnNAS location in Files:\n\(files.debugDescription)")
            return
        }
        location.tap()
        // The first time, Files asks to turn the new location on.
        let turnOn = files.buttons["Turn On"]
        if turnOn.waitForExistence(timeout: 5) { turnOn.tap() }
        let docs = files.staticTexts["Docs"].firstMatch
        if !docs.waitForExistence(timeout: 20) {
            shotOf(files, "failed-files-library")
            XCTFail("Docs not listed:\n\(files.debugDescription)")
            return
        }
        shotOf(files, "15-files-library")
        // The icon opens the folder; the name would start renaming it.
        docs.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: -3)).tap()
        let notes = files.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'notes'")).firstMatch
        if !notes.waitForExistence(timeout: 20) {
            shotOf(files, "failed-files-docs")
            XCTFail("notes.md not listed:\n\(files.debugDescription)")
            return
        }
        shotOf(files, "16-files-docs")

        // Open it: Files downloads it through the extension.
        notes.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: -3)).tap()
        sleep(4)
        shotOf(files, "17-files-open")
        XCTAssertTrue(files.staticTexts.matching(NSPredicate(format: "label CONTAINS 'hello'")).firstMatch.waitForExistence(timeout: 10) || files.textViews.firstMatch.exists, "notes.md did not open")
        files.buttons["QLOverlayDoneButtonAccessibilityIdentifier"].firstMatch.tap()
        sleep(2)

        // A new folder, made in Files, on the server.
        files.buttons["More"].firstMatch.tap()
        let newFolder = files.buttons["New Folder"].firstMatch
        if !newFolder.waitForExistence(timeout: 5) {
            shotOf(files, "failed-files-more")
            XCTFail("no New Folder:\n\(files.debugDescription)")
            return
        }
        newFolder.tap()
        sleep(1)
        files.typeText("From Files\n")
        XCTAssertTrue(files.staticTexts["From Files"].firstMatch.waitForExistence(timeout: 20), "the new folder did not appear")
        sleep(2)
        shotOf(files, "18-files-new-folder")

        // Delete it again: it goes to the server's Trash.
        files.staticTexts["From Files"].firstMatch.press(forDuration: 1.2)
        // Files says "Delete Now"; the server still keeps it in its Trash.
        let delete = files.buttons["Delete Now"].firstMatch
        if !delete.waitForExistence(timeout: 5) {
            shotOf(files, "failed-files-menu")
            XCTFail("no Delete:\n\(files.debugDescription)")
            return
        }
        delete.tap()
        let confirm = files.buttons.matching(NSPredicate(format: "label == 'Delete' OR label == 'Delete Now'")).firstMatch
        if confirm.waitForExistence(timeout: 3) { confirm.tap() }
        let gone = NSPredicate(format: "exists == false")
        expectation(for: gone, evaluatedWith: files.staticTexts["From Files"].firstMatch)
        waitForExpectations(timeout: 20)

        // Duplicate notes.md: Files uploads the copy through the extension.
        notes.press(forDuration: 1.2)
        let duplicate = files.buttons["Duplicate"].firstMatch
        XCTAssertTrue(duplicate.waitForExistence(timeout: 5), "no Duplicate")
        duplicate.tap()
        XCTAssertTrue(files.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'notes 2'")).firstMatch.waitForExistence(timeout: 20), "no copy")
        sleep(4)
        shotOf(files, "19-files-duplicate")
    }

    private func shotOf(_ other: XCUIApplication, _ name: String) {
        _ = other
        shot(name)
    }
}
