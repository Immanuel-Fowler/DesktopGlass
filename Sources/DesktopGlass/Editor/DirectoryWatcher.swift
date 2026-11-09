import CoreServices
import Foundation

/// Watches a folder tree with FSEvents and calls `onChange` on the main queue when something in it changes, so files
/// an agent creates from the terminal show up in the editor without pressing Refresh. Changes FSEvents reports within
/// `latency` seconds are delivered as one call. Churn inside folders the tree and Quick Open ignore anyway (.git,
/// node_modules, build caches) doesn't count.
final class DirectoryWatcher {
    private var stream: FSEventStreamRef?
    private let onChange: () -> Void

    static let ignoredComponents: Set<String> = [".git", "node_modules", ".build", "DerivedData", "__pycache__",
                                                  ".next", ".venv", "venv", ".turbo", ".cache", ".gradle", "Pods"]

    init?(url: URL, latency: TimeInterval = 0.4, onChange: @escaping () -> Void) {
        self.onChange = onChange
        var context = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(),
                                           retain: nil, release: nil, copyDescription: nil)
        let callback: FSEventStreamCallback = { _, info, _, paths, _, _ in
            guard let info else { return }
            let watcher = Unmanaged<DirectoryWatcher>.fromOpaque(info).takeUnretainedValue()
            let changed = (unsafeBitCast(paths, to: NSArray.self) as? [String]) ?? []
            if changed.contains(where: { !DirectoryWatcher.isIgnored($0) }) { watcher.onChange() }
        }
        let flags = FSEventStreamCreateFlags(kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagWatchRoot)
        guard let s = FSEventStreamCreate(nil, callback, &context, [url.path] as CFArray,
                                          FSEventStreamEventId(kFSEventStreamEventIdSinceNow), latency, flags) else {
            return nil
        }
        stream = s
        FSEventStreamSetDispatchQueue(s, .main)
        FSEventStreamStart(s)
    }

    static func isIgnored(_ path: String) -> Bool {
        path.split(separator: "/").contains { ignoredComponents.contains(String($0)) }
    }

    deinit {
        guard let s = stream else { return }
        FSEventStreamStop(s)
        FSEventStreamInvalidate(s)
        FSEventStreamRelease(s)
    }
}
