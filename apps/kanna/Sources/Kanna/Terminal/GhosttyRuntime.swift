import AppKit
import GhosttyKit

@MainActor
final class GhosttyRuntime {
    nonisolated(unsafe) static var shared: GhosttyRuntime?

    let app: ghostty_app_t
    private(set) var config: ghostty_config_t
    private var live: [UInt: TerminalSurfaceView] = [:]
    private var timer: Timer?
    private var appearance: NSKeyValueObservation?
    let effects: TerminalEffects
    private(set) var settings: TerminalSettings
    var onSettings: ((TerminalSettings) -> Void)?
    var onAction: ((TerminalSurfaceView, ghostty_action_s) -> Bool)?
    var onTitle: ((TerminalSurfaceView, String) -> Void)?
    var onProgress: ((TerminalSurfaceView, ghostty_action_progress_report_s) -> Void)?
    var onClose: ((TerminalSurfaceView) -> Void)?

    init(recording: Bool = ProcessInfo.processInfo.environment["KANNA_TEST_EFFECTS"] == "1") throws {
        guard let cfg = Self.loadConfig() else { throw Self.error("ghostty_config_new failed") }

        var runtime = ghostty_runtime_config_s()
        runtime.supports_selection_clipboard = true
        runtime.wakeup_cb = { _ in DispatchQueue.main.async { MainActor.assumeIsolated { GhosttyRuntime.shared?.tick() } } }
        runtime.action_cb = { _, target, action in
            guard Thread.isMainThread else { return false }
            return MainActor.assumeIsolated { GhosttyRuntime.shared?.perform(target, action) ?? false }
        }
        runtime.read_clipboard_cb = { userdata, location, state, _, _, _ in
            let id = UInt(bitPattern: userdata)
            return MainActor.assumeIsolated { GhosttyRuntime.shared?.readClipboard(id, location, state: state) ?? GHOSTTY_CLIPBOARD_READ_UNSUPPORTED }
        }
        runtime.confirm_read_clipboard_cb = { userdata, confirm, state, request in
            let id = UInt(bitPattern: userdata)
            MainActor.assumeIsolated { GhosttyRuntime.shared?.confirmClipboard(id, confirm, state: state, request: request) }
        }
        runtime.write_clipboard_cb = { userdata, location, content, count, confirm in
            let id = UInt(bitPattern: userdata)
            MainActor.assumeIsolated { GhosttyRuntime.shared?.writeClipboard(id, location, content: content, count: count, confirm: confirm) }
        }
        runtime.close_surface_cb = { userdata, _ in
            let id = UInt(bitPattern: userdata)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let runtime = GhosttyRuntime.shared, let view = runtime.live[id] else { return }
                    runtime.onClose?(view)
                }
            }
        }
        effects = TerminalEffects(recording: recording)
        settings = TerminalSettings(cfg, dark: TerminalSettings.appearanceIsDark, cursor: nil)
        guard let value = ghostty_app_new(&runtime, cfg) else {
            ghostty_config_free(cfg)
            throw Self.error("ghostty_app_new failed")
        }
        app = value
        config = cfg
        ghostty_app_set_focus(value, true)
        Self.shared = self
        applyAppearance()
        appearance = NSApp.observe(\.effectiveAppearance) { _, _ in
            DispatchQueue.main.async { MainActor.assumeIsolated { GhosttyRuntime.shared?.applyAppearance() } }
        }
        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 60.0, repeats: true) { _ in
            MainActor.assumeIsolated { GhosttyRuntime.shared?.tick() }
        }
    }

    private static func loadConfig() -> ghostty_config_t? {
        guard let cfg = ghostty_config_new() else { return nil }
        ghostty_config_load_default_files(cfg)
        ghostty_config_load_recursive_files(cfg)
        ghostty_config_finalize(cfg)
        return cfg
    }

    static func options(for view: TerminalSurfaceView) -> ghostty_surface_config_s {
        var options = ghostty_surface_config_new()
        options.platform_tag = GHOSTTY_PLATFORM_MACOS
        options.platform = ghostty_platform_u(macos: ghostty_platform_macos_s(nsview: Unmanaged.passUnretained(view).toOpaque()))
        options.userdata = UnsafeMutableRawPointer(bitPattern: view.surfaceID)
        options.scale_factor = Double(NSScreen.main?.backingScaleFactor ?? 2)
        return options
    }

    func attach(_ view: TerminalSurfaceView, options: inout ghostty_surface_config_s) -> Bool {
        guard let surface = ghostty_surface_new(app, &options) else { return false }
        live[view.surfaceID] = view
        view.surface = surface
        ghostty_surface_set_color_scheme(surface, scheme)
        return true
    }

    func detach(_ view: TerminalSurfaceView) {
        live.removeValue(forKey: view.surfaceID)
        if let surface = view.surface { ghostty_surface_free(surface) }
        view.surface = nil
    }

    func trigger(for action: String) -> ghostty_input_trigger_s {
        ghostty_config_trigger(config, action, UInt(action.utf8.count))
    }

    private func tick() { ghostty_app_tick(app) }

    private var scheme: ghostty_color_scheme_e { TerminalSettings.appearanceIsDark ? GHOSTTY_COLOR_SCHEME_DARK : GHOSTTY_COLOR_SCHEME_LIGHT }

    private func applyAppearance() {
        ghostty_app_set_color_scheme(app, scheme)
        for view in live.values {
            if let surface = view.surface { ghostty_surface_set_color_scheme(surface, scheme) }
        }
        var next = settings
        next.dark = TerminalSettings.appearanceIsDark
        publish(next)
    }

    func reloadConfig() {
        guard let fresh = Self.loadConfig() else { return }
        ghostty_app_update_config(app, fresh)
        let old = config
        config = fresh
        ghostty_config_free(old)
    }

    private func refreshSettings(from source: ghostty_config_t) {
        let cursor = live.values.lazy.compactMap { view -> String? in
            guard let surface = view.surface else { return nil }
            var colors = ghostty_surface_colors_s()
            guard ghostty_surface_colors(surface, true, &colors), colors.has_cursor else { return nil }
            return TerminalSettings.hex(colors.cursor)
        }.first
        publish(TerminalSettings(source, dark: TerminalSettings.appearanceIsDark, cursor: cursor))
    }

    private func publish(_ next: TerminalSettings) {
        guard next != settings else { return }
        settings = next
        onSettings?(next)
    }

    private func perform(_ target: ghostty_target_s, _ action: ghostty_action_s) -> Bool {
        switch action.tag {
        case GHOSTTY_ACTION_RELOAD_CONFIG:
            if action.action.reload_config.soft {
                if target.tag == GHOSTTY_TARGET_SURFACE, let surface = target.target.surface {
                    ghostty_surface_update_config(surface, config)
                } else {
                    ghostty_app_update_config(app, config)
                }
            } else {
                reloadConfig()
            }
            return true
        case GHOSTTY_ACTION_CONFIG_CHANGE:
            if let changed = action.action.config_change.config { refreshSettings(from: changed) }
            return true
        default: break
        }
        guard target.tag == GHOSTTY_TARGET_SURFACE, let userdata = target.target.surface.flatMap({ ghostty_surface_userdata($0) }),
              let view = live[UInt(bitPattern: userdata)] else { return false }
        switch action.tag {
        case GHOSTTY_ACTION_PWD, GHOSTTY_ACTION_COLOR_CHANGE:
            return true
        case GHOSTTY_ACTION_SET_TITLE:
            let title = action.action.set_title.title.map { String(cString: $0) } ?? ""
            view.title = title
            onTitle?(view, title)
            return true
        case GHOSTTY_ACTION_OPEN_URL:
            let value = action.action.open_url
            guard let url = value.url else { return false }
            effects.open(String(decoding: UnsafeRawBufferPointer(start: url, count: Int(value.len)), as: UTF8.self), kind: value.kind, in: view.window)
            return true
        case GHOSTTY_ACTION_MOUSE_SHAPE:
            view.setMouseShape(action.action.mouse_shape)
            return true
        case GHOSTTY_ACTION_MOUSE_VISIBILITY:
            NSCursor.setHiddenUntilMouseMoves(action.action.mouse_visibility == GHOSTTY_MOUSE_HIDDEN)
            return true
        case GHOSTTY_ACTION_MOUSE_OVER_LINK:
            let value = action.action.mouse_over_link
            view.hoveredLink = value.len > 0 ? value.url.map { String(decoding: UnsafeRawBufferPointer(start: $0, count: value.len), as: UTF8.self) } : nil
            return true
        case GHOSTTY_ACTION_RING_BELL:
            effects.ring(settings.bell, in: view.window)
            return true
        case GHOSTTY_ACTION_DESKTOP_NOTIFICATION:
            guard settings.notifications else { return true }
            let value = action.action.desktop_notification
            effects.notify(title: value.title.map { String(cString: $0) } ?? "", body: value.body.map { String(cString: $0) } ?? "")
            return true
        case GHOSTTY_ACTION_PROGRESS_REPORT:
            view.progress = action.action.progress_report
            onProgress?(view, action.action.progress_report)
            return true
        case GHOSTTY_ACTION_SHOW_CHILD_EXITED:
            return false
        default:
            return onAction?(view, action) ?? false
        }
    }

    private func readClipboard(_ id: UInt, _ location: ghostty_clipboard_e, state: UnsafeMutableRawPointer?) -> ghostty_clipboard_read_result_e {
        guard let view = live[id], let surface = view.surface, let string = view.takeDropped() ?? effects.read(location) else { return GHOSTTY_CLIPBOARD_READ_UNAVAILABLE }
        let mime = strdup("text/plain")!
        let text = strdup(string)!
        defer { free(mime); free(text) }
        var item = ghostty_clipboard_content_s(mime: mime, data: text, len: strlen(text))
        withUnsafePointer(to: &item) { itemPointer in
            var result = ghostty_clipboard_complete_s(contents: itemPointer, contents_len: 1, available: nil, available_len: 0, confirmed: false, remember: false)
            ghostty_surface_complete_clipboard_request(surface, &result, state)
        }
        return GHOSTTY_CLIPBOARD_READ_STARTED
    }

    private func confirmClipboard(_ id: UInt, _ confirm: UnsafePointer<ghostty_clipboard_confirm_s>?, state: UnsafeMutableRawPointer?, request: ghostty_clipboard_request_e) {
        let items = confirm.map { ClipboardItems($0.pointee) }
        let token = UInt(bitPattern: state)
        DispatchQueue.main.async {
            MainActor.assumeIsolated { GhosttyRuntime.shared?.askToShare(id, items, request: request, state: token) }
        }
    }

    private func askToShare(_ id: UInt, _ items: ClipboardItems?, request: ghostty_clipboard_request_e, state: UInt) {
        guard let view = live[id], let surface = view.surface else { return }
        guard let items, let window = view.window, !effects.recording else {
            return ghostty_surface_deny_clipboard_request(surface, UnsafeMutableRawPointer(bitPattern: state))
        }
        let alert = NSAlert()
        if request == GHOSTTY_CLIPBOARD_REQUEST_PASTE {
            alert.messageText = "Allow terminal paste?"
            alert.informativeText = "The text you are pasting may run commands in the terminal."
            alert.addButton(withTitle: "Paste")
        } else {
            alert.messageText = "Allow the program to read the clipboard?"
            alert.informativeText = "A program in the terminal asked for what is on your clipboard."
            alert.addButton(withTitle: "Allow")
        }
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { response in
            MainActor.assumeIsolated {
                guard let surface = GhosttyRuntime.shared?.live[id]?.surface else { return }
                let state = UnsafeMutableRawPointer(bitPattern: state)
                if response == .alertFirstButtonReturn {
                    items.complete(surface, state: state)
                } else {
                    ghostty_surface_deny_clipboard_request(surface, state)
                }
            }
        }
    }

    private func writeClipboard(_ id: UInt, _ location: ghostty_clipboard_e, content: UnsafePointer<ghostty_clipboard_content_s>?, count: Int, confirm: Bool) {
        guard let content, count > 0 else { return }
        let items = (0..<count).map { content[$0] }
        guard let value = items.first(where: { $0.mime.map { String(cString: $0) } == "text/plain" }), let bytes = value.data else { return }
        let text = String(decoding: UnsafeRawBufferPointer(start: bytes, count: value.len), as: UTF8.self)
        guard confirm else { return effects.write(text, to: location) }
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                guard let runtime = GhosttyRuntime.shared, let window = runtime.live[id]?.window, !runtime.effects.recording else { return }
                let alert = NSAlert()
                alert.messageText = "Allow terminal clipboard write?"
                alert.informativeText = text
                alert.addButton(withTitle: "Allow")
                alert.addButton(withTitle: "Cancel")
                alert.beginSheetModal(for: window) { response in
                    if response == .alertFirstButtonReturn { MainActor.assumeIsolated { GhosttyRuntime.shared?.effects.write(text, to: location) } }
                }
            }
        }
    }

    private static func error(_ message: String) -> NSError { NSError(domain: "kanna", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
}

private struct ClipboardItems: Sendable {
    let contents: [(mime: String, data: Data)]
    let available: [String]

    init(_ value: ghostty_clipboard_confirm_s) {
        contents = (0..<value.contents_len).compactMap { index in
            guard let item = value.contents?[index], let mime = item.mime else { return nil }
            return (String(cString: mime), item.data.map { Data(bytes: $0, count: item.len) } ?? Data())
        }
        available = (0..<value.available_len).compactMap { index in value.available?[index].map { String(cString: $0) } }
    }

    @MainActor
    func complete(_ surface: ghostty_surface_t, state: UnsafeMutableRawPointer?) {
        let mimes = contents.map { strdup($0.mime) }
        let datas = contents.map { item -> UnsafeMutablePointer<CChar> in
            let buffer = UnsafeMutablePointer<CChar>.allocate(capacity: max(1, item.data.count))
            item.data.copyBytes(to: UnsafeMutableRawPointer(buffer).assumingMemoryBound(to: UInt8.self), count: item.data.count)
            return buffer
        }
        let names: [UnsafePointer<CChar>?] = available.map { UnsafePointer(strdup($0)) }
        defer {
            mimes.forEach { free($0) }
            datas.forEach { $0.deallocate() }
            names.forEach { free(UnsafeMutableRawPointer(mutating: $0)) }
        }
        let items = contents.indices.map { ghostty_clipboard_content_s(mime: mimes[$0], data: datas[$0], len: contents[$0].data.count) }
        items.withUnsafeBufferPointer { buffer in
            names.withUnsafeBufferPointer { available in
                var complete = ghostty_clipboard_complete_s(contents: buffer.baseAddress, contents_len: buffer.count, available: available.baseAddress,
                                                            available_len: available.count, confirmed: true, remember: false)
                ghostty_surface_complete_clipboard_request(surface, &complete, state)
            }
        }
    }
}
