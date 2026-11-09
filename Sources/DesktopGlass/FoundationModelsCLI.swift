import Foundation
#if canImport(FoundationModels)
import FoundationModels
#endif

/// `DesktopGlass --foundation-models` reads {"system", "prompt"} JSON on stdin and prints the reply from
/// Apple's on-device model; `--foundation-models-check` reports availability. `dg reindex` calls these.
enum FoundationModelsCLI {
    struct Request: Decodable { let system: String; let prompt: String }

    private final class Box: @unchecked Sendable {
        var output = ""
        var code: Int32 = 0
    }

    static var isAvailable: Bool {
        #if canImport(FoundationModels)
        if #available(macOS 26.0, *) {
            if case .available = SystemLanguageModel.default.availability { return true }
        }
        #endif
        return false
    }

    static func run(check: Bool) -> Int32 {
        #if canImport(FoundationModels)
        if #available(macOS 26.0, *) {
            let availability = SystemLanguageModel.default.availability
            guard case .available = availability else {
                print("Apple's on-device model isn't available (\(availability)). Turn on Apple Intelligence in System Settings.")
                return 78
            }
            if check {
                print("Apple Foundation Models ready")
                return 0
            }
            let input = FileHandle.standardInput.readDataToEndOfFile()
            guard let request = try? JSONDecoder().decode(Request.self, from: input) else {
                print("Expected JSON {\"system\": …, \"prompt\": …} on stdin")
                return 2
            }
            let box = Box()
            let done = DispatchSemaphore(value: 0)
            Task.detached {
                do {
                    let session = LanguageModelSession(instructions: request.system)
                    let response = try await session.respond(to: request.prompt)
                    box.output = response.content
                } catch {
                    box.output = "Foundation Models error: \(error)"
                    box.code = 1
                }
                done.signal()
            }
            done.wait()
            print(box.output)
            return box.code
        }
        #endif
        print("Apple Foundation Models need macOS 26 with Apple Intelligence turned on.")
        return 78
    }
}
