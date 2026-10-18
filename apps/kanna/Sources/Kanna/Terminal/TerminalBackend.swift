@MainActor
protocol TerminalBackend: AnyObject {
    func start(_ view: TerminalSurfaceView, terminal: String) -> Bool
    func stop(_ view: TerminalSurfaceView)
}
