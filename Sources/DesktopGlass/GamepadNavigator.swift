import Foundation
import GameController

/// D-pad / left stick move, A opens, B goes back, Y searches — only while the overlay is showing,
/// so a controller never steers the overlay while you're playing.
@MainActor
final class GamepadNavigator {
    static let shared = GamepadNavigator()

    var isActive: () -> Bool = { false }
    var move: (Int, Int) -> Void = { _, _ in }
    var activate: () -> Void = {}
    var back: () -> Void = {}
    var search: () -> Void = {}

    private var heldDirection: (Int, Int)?
    private var repeatTask: Task<Void, Never>?

    func start() {
        GCController.shouldMonitorBackgroundEvents = true
        observer = NotificationCenter.default.addObserver(forName: .GCControllerDidConnect, object: nil, queue: .main) { [weak self] note in
            guard let c = note.object as? GCController else { return }
            MainActor.assumeIsolated { self?.attach(c) }
        }
        GCController.controllers().forEach(attach)
    }

    private var observer: NSObjectProtocol?

    private func attach(_ c: GCController) {
        c.handlerQueue = .main
        guard let pad = c.extendedGamepad else { return }
        pad.valueChangedHandler = { [weak self] pad, element in
            MainActor.assumeIsolated { self?.handle(pad, element) }
        }
    }

    private func handle(_ pad: GCExtendedGamepad, _ element: GCControllerElement) {
        guard isActive() else { stopRepeat(); return }
        if element === pad.buttonA, pad.buttonA.isPressed { activate(); return }
        if element === pad.buttonB, pad.buttonB.isPressed { back(); return }
        if element === pad.buttonY, pad.buttonY.isPressed { search(); return }
        guard element === pad.dpad || element === pad.leftThumbstick else { return }
        let x = pad.dpad.xAxis.value != 0 ? pad.dpad.xAxis.value : pad.leftThumbstick.xAxis.value
        let y = pad.dpad.yAxis.value != 0 ? pad.dpad.yAxis.value : pad.leftThumbstick.yAxis.value
        var dir: (Int, Int)? = nil
        if abs(x) > 0.5 || abs(y) > 0.5 {
            dir = abs(x) > abs(y) ? (x > 0 ? 1 : -1, 0) : (0, y > 0 ? -1 : 1)    // stick up = row above
        }
        if let d = dir, heldDirection.map({ $0 != d }) ?? true {
            heldDirection = d
            move(d.0, d.1)
            stopRepeat()
            // Held direction: a pause, then keep moving — like keyboard key repeat.
            repeatTask = Task { @MainActor [weak self] in
                try? await Task.sleep(nanoseconds: 380_000_000)
                while !Task.isCancelled {
                    guard let self, let h = self.heldDirection, self.isActive() else { return }
                    self.move(h.0, h.1)
                    try? await Task.sleep(nanoseconds: 110_000_000)
                }
            }
        } else if dir == nil {
            heldDirection = nil
            stopRepeat()
        }
    }

    private func stopRepeat() {
        repeatTask?.cancel()
        repeatTask = nil
    }
}
