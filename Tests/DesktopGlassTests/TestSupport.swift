import Foundation
import XCTest

/// A scratch folder that's removed when the test ends. Symlinks in the path are resolved so paths from
/// FileManager enumerators (/private/var/…) compare equal to the ones the test builds.
final class TempDir {
    let url: URL

    init() throws {
        url = FileManager.default.temporaryDirectory
            .appendingPathComponent("DesktopGlassTests-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    deinit { try? FileManager.default.removeItem(at: url.resolvingSymlinksInPath()) }

    var root: URL { url.resolvingSymlinksInPath() }

    @discardableResult
    func file(_ path: String, _ contents: String = "") throws -> URL {
        try data(path, Data(contents.utf8))
    }

    @discardableResult
    func data(_ path: String, _ bytes: Data) throws -> URL {
        let u = root.appendingPathComponent(path)
        try FileManager.default.createDirectory(at: u.deletingLastPathComponent(), withIntermediateDirectories: true)
        try bytes.write(to: u)
        return u
    }

    @discardableResult
    func dir(_ path: String) throws -> URL {
        let u = root.appendingPathComponent(path, isDirectory: true)
        try FileManager.default.createDirectory(at: u, withIntermediateDirectories: true)
        return u
    }
}
