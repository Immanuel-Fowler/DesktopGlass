import Foundation

/// Version and identity come from Info.plist, which build-app.sh fills in from app.conf.
enum AppInfo {
    static var version: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev" }
    static var build: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "0" }
    static var versionString: String { "\(version) (\(build))" }
}
