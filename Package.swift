// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "DesktopGlass",
    platforms: [.macOS(.v13)],
    dependencies: [
        // Terminal emulator for the IDE's terminal panel (MIT)
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", from: "1.2.0"),
    ],
    targets: [
        .executableTarget(
            name: "DesktopGlass",
            dependencies: [.product(name: "SwiftTerm", package: "SwiftTerm")],
            path: "Sources/DesktopGlass"
        ),
        // `swift test` — the app's own code. description.md parsing and the Desktop model are tested in the engine
        // (engine/crates/dg-core, fixtures in engine/tests/golden).
        .testTarget(
            name: "DesktopGlassTests",
            dependencies: ["DesktopGlass"],
            path: "Tests/DesktopGlassTests"
        ),
    ]
)
