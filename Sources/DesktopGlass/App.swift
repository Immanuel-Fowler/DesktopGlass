import AppKit

@main
enum DesktopGlassApp {
    @MainActor
    static func main() {
        let args = CommandLine.arguments
        if args.contains("--foundation-models-check") { exit(FoundationModelsCLI.run(check: true)) }
        if args.contains("--foundation-models") { exit(FoundationModelsCLI.run(check: false)) }
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.accessory)
        app.run()
    }
}
