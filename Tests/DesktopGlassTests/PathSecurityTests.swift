import XCTest
@testable import DesktopGlass

/// Which Desktop entries the app shows. (The artifact viewer's allow-list and path resolution moved to the engine:
/// engine/crates/dg-core/src/artifact_policy.rs has those tests; HTMLEngineTests checks the Swift matcher.)
final class PathSecurityTests: XCTestCase {
    func testDesktopLayoutIgnoresHiddenAndToolFolders() {
        for n in [".git", ".DS_Store", "~$report.docx", "node_modules", "_Organize Log", "description.md", "Icon\r"] {
            XCTAssertFalse(Layout.isVisible(n), n)
        }
        XCTAssertTrue(Layout.isVisible("Projects"))
    }
}
