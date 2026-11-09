import Foundation

/// JavaScript shared with other DesktopGlass UIs (shared/web/*.js in the repo, Contents/Resources/web in the app
/// bundle — scripts/build-app.sh copies them). `swift run` / `swift test` read them from the source tree.
enum WebScripts {
    private static var cache: [String: String] = [:]
    private static let lock = NSLock()

    /// shared/web in the source tree (for builds that aren't an app bundle).
    static let sourceDirectory = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("shared/web", isDirectory: true)

    /// The script `name` (without ".js"); empty if it can't be found (logged once).
    static func source(_ name: String) -> String {
        lock.lock()
        defer { lock.unlock() }
        if let s = cache[name] { return s }
        let candidates = [Bundle.main.url(forResource: name, withExtension: "js", subdirectory: "web"),
                          sourceDirectory.appendingPathComponent(name + ".js")]
        let text = candidates.compactMap { $0 }.lazy.compactMap { try? String(contentsOf: $0, encoding: .utf8) }.first
        if text == nil { NSLog("DesktopGlass: missing web script \(name).js") }
        cache[name] = text ?? ""
        return text ?? ""
    }

    /// A call of the function-expression script `name` with JSON-encoded arguments: `(<script>)(a, b)`.
    static func call(_ name: String, _ args: [Any]) throws -> String {
        let encoded = try args.map { arg -> String in
            let data = try JSONSerialization.data(withJSONObject: arg, options: .fragmentsAllowed)
            return String(data: data, encoding: .utf8) ?? "null"
        }
        return "(\n" + source(name) + "\n)(" + encoded.joined(separator: ", ") + ")"
    }
}
