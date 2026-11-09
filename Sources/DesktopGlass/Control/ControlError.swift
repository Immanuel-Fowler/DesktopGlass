import Foundation

/// An error a control-channel handler returns; its message goes back to the MCP client as `{"error": …}`.
struct ControlError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}
