public enum KeyWindow {
    public static func resolve(appKit: String?, layout: String?, windows: [String]) -> String? {
        if let appKit, windows.contains(appKit) { return appKit }
        if let layout, windows.contains(layout) { return layout }
        return windows.last
    }
}
