public enum KeyWindow {
    public static func resolve(appKit: String?, front: [String], windows: [String]) -> String? {
        if let appKit, windows.contains(appKit) { return appKit }
        return front.first(where: windows.contains) ?? windows.last
    }
}
