public enum Keyboard {
    public enum Holder: Equatable { case nothing, target, pane, text, other }

    public static func moves(from holder: Holder, keyed: Bool) -> Bool {
        switch holder {
        case .nothing, .pane: true
        case .target, .text: false
        case .other: !keyed
        }
    }
}
