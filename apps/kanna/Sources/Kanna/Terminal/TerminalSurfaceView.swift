import AppKit
import GhosttyKit
import QuartzCore

@MainActor
final class TerminalSurfaceView: NSView, @preconcurrency NSTextInputClient {
    var surface: ghostty_surface_t? { didSet { if surface != nil { syncSurfaceGeometry(); updateVisibility() } } }
    var isHandlingInputEvent = false
    private var dropped: String?
    var focused: (() -> Void)?
    var contextMenu: (() -> NSMenu?)?
    var title = ""
    var progress: ghostty_action_progress_report_s?
    var hoveredLink: String? { didSet { updateCursor() } }
    private var shape = NSCursor.iBeam
    static let ignoresOcclusion = ProcessInfo.processInfo.environment["KANNA_IGNORE_OCCLUSION"] == "1"
    private static var lastSurfaceID: UInt = 0
    let surfaceID: UInt
    private var occlusionObserver: NSObjectProtocol?
    private var trackingArea: NSTrackingArea?
    private var marked = NSMutableAttributedString()
    private var accumulating: [String]?
    private var suppressedKeyUps = Set<UInt16>()

    override var acceptsFirstResponder: Bool { true }

    override init(frame frameRect: NSRect) {
        Self.lastSurfaceID += 1
        surfaceID = Self.lastSurfaceID
        super.init(frame: frameRect)
        registerForDraggedTypes(Self.dropTypes)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let occlusionObserver { NotificationCenter.default.removeObserver(occlusionObserver) }
        occlusionObserver = window.map { window in
            NotificationCenter.default.addObserver(forName: NSWindow.didChangeOcclusionStateNotification, object: window, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.updateVisibility() }
            }
        }
        guard let surface else { return }
        ghostty_surface_set_focus(surface, window?.firstResponder === self)
        syncSurfaceGeometry()
        updateVisibility()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        if let window {
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            layer?.contentsScale = window.backingScaleFactor
            CATransaction.commit()
        }
        syncSurfaceGeometry()
    }

    private var visible: Bool { !isHiddenOrHasHiddenAncestor && window.map { Self.ignoresOcclusion || $0.occlusionState.contains(.visible) } ?? false }

    override func viewDidHide() {
        super.viewDidHide()
        updateVisibility()
    }

    override func viewDidUnhide() {
        super.viewDidUnhide()
        updateVisibility()
    }

    func updateVisibility() {
        guard let surface else { return }
        ghostty_surface_set_occlusion(surface, visible)
    }

    var hiddenFromGhostty: Bool { surface != nil && !visible }

    var presented: ObjectIdentifier? { (layer?.contents as AnyObject?).map(ObjectIdentifier.init) }

    func showToGhostty() {
        guard let surface else { return }
        ghostty_surface_set_occlusion(surface, true)
    }

    var pixels: (width: UInt32, height: UInt32)? {
        guard bounds.width > 0, bounds.height > 0 else { return nil }
        let backing = convertToBacking(bounds)
        return (UInt32(max(1, backing.width)), UInt32(max(1, backing.height)))
    }

    private func syncSurfaceGeometry() {
        guard let surface, let pixels else { return }
        let backing = convertToBacking(bounds)
        ghostty_surface_set_content_scale(surface, backing.width / bounds.width, backing.height / bounds.height)
        ghostty_surface_set_size(surface, pixels.width, pixels.height)
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        syncSurfaceGeometry()
    }

    override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        if let surface { ghostty_surface_set_focus(surface, true) }
        if result { focused?() }
        return result
    }

    override func resignFirstResponder() -> Bool {
        let result = super.resignFirstResponder()
        if let surface { ghostty_surface_set_focus(surface, false) }
        return result
    }

    func setMouseShape(_ value: ghostty_action_mouse_shape_e) {
        shape = switch value {
        case GHOSTTY_MOUSE_SHAPE_DEFAULT: .arrow
        case GHOSTTY_MOUSE_SHAPE_CONTEXT_MENU: .contextualMenu
        case GHOSTTY_MOUSE_SHAPE_POINTER: .pointingHand
        case GHOSTTY_MOUSE_SHAPE_CROSSHAIR, GHOSTTY_MOUSE_SHAPE_CELL: .crosshair
        case GHOSTTY_MOUSE_SHAPE_TEXT: .iBeam
        case GHOSTTY_MOUSE_SHAPE_VERTICAL_TEXT: .iBeamCursorForVerticalLayout
        case GHOSTTY_MOUSE_SHAPE_ALIAS: .dragLink
        case GHOSTTY_MOUSE_SHAPE_COPY: .dragCopy
        case GHOSTTY_MOUSE_SHAPE_NO_DROP, GHOSTTY_MOUSE_SHAPE_NOT_ALLOWED: .operationNotAllowed
        case GHOSTTY_MOUSE_SHAPE_GRAB: .openHand
        case GHOSTTY_MOUSE_SHAPE_GRABBING, GHOSTTY_MOUSE_SHAPE_MOVE, GHOSTTY_MOUSE_SHAPE_ALL_SCROLL: .closedHand
        case GHOSTTY_MOUSE_SHAPE_COL_RESIZE, GHOSTTY_MOUSE_SHAPE_EW_RESIZE, GHOSTTY_MOUSE_SHAPE_E_RESIZE, GHOSTTY_MOUSE_SHAPE_W_RESIZE: .resizeLeftRight
        case GHOSTTY_MOUSE_SHAPE_ROW_RESIZE, GHOSTTY_MOUSE_SHAPE_NS_RESIZE, GHOSTTY_MOUSE_SHAPE_N_RESIZE, GHOSTTY_MOUSE_SHAPE_S_RESIZE: .resizeUpDown
        default: .arrow
        }
        updateCursor()
    }

    var pointer: NSCursor { hoveredLink == nil ? shape : .pointingHand }

    private func updateCursor() {
        guard let window, window.isKeyWindow else { return }
        let point = convert(window.mouseLocationOutsideOfEventStream, from: nil)
        if bounds.contains(point) { pointer.set() }
    }

    override func cursorUpdate(with event: NSEvent) { pointer.set() }

    func zoom(by step: Int) {
        perform(step > 0 ? "increase_font_size:1" : step < 0 ? "decrease_font_size:1" : "reset_font_size")
    }

    @objc func copy(_ sender: Any?) { perform("copy_to_clipboard") }
    @objc func paste(_ sender: Any?) { perform("paste_from_clipboard") }
    @objc override func selectAll(_ sender: Any?) { perform("select_all") }

    private func perform(_ action: String) {
        guard let surface else { return }
        let handling = isHandlingInputEvent
        isHandlingInputEvent = true
        defer { isHandlingInputEvent = handling }
        _ = ghostty_surface_binding_action(surface, action, UInt(action.utf8.count))
    }

    @objc func validateMenuItem(_ item: NSMenuItem) -> Bool {
        guard let surface else { return false }
        return item.action == #selector(copy(_:)) ? ghostty_surface_has_selection(surface) : true
    }

    override func keyDown(with event: NSEvent) {
        guard let surface else {
            interpretKeyEvents([event])
            return
        }
        let mods = Self.mods(event.modifierFlags)
        let translated = ghostty_surface_key_translation_mods(surface, mods)
        let flags = Self.adjust(event.modifierFlags, basedOn: translated)
        let translatedEvent = flags == event.modifierFlags ? event : (NSEvent.keyEvent(with: .keyDown, location: event.locationInWindow, modifierFlags: flags, timestamp: event.timestamp, windowNumber: event.windowNumber, context: nil, characters: event.characters(byApplyingModifiers: flags) ?? "", charactersIgnoringModifiers: event.charactersIgnoringModifiers ?? "", isARepeat: event.isARepeat, keyCode: event.keyCode) ?? event)
        accumulating = []
        isHandlingInputEvent = true
        defer {
            accumulating = nil
            isHandlingInputEvent = false
        }
        interpretKeyEvents([translatedEvent])
        syncPreedit()
        if marked.length > 0 {
            suppressedKeyUps.insert(event.keyCode)
            return
        }
        if let texts = accumulating, !texts.isEmpty {
            for text in texts { sendKeyPress(event, mods: mods, text: text) }
            return
        }
        sendKeyPress(event, mods: mods, text: Self.keyEventText(translatedEvent))
    }

    override func keyUp(with event: NSEvent) {
        if suppressedKeyUps.remove(event.keyCode) != nil { return }
        guard let surface else { return }
        var input = ghostty_input_key_s()
        input.action = GHOSTTY_ACTION_RELEASE
        input.keycode = UInt32(event.keyCode)
        input.mods = Self.mods(event.modifierFlags)
        input.unshifted_codepoint = event.charactersIgnoringModifiers?.unicodeScalars.first?.value ?? 0
        _ = ghostty_surface_key(surface, input)
    }

    private func sendKeyPress(_ event: NSEvent, mods: ghostty_input_mods_e, text: String?) {
        guard let surface else { return }
        var input = ghostty_input_key_s()
        input.action = event.isARepeat ? GHOSTTY_ACTION_REPEAT : GHOSTTY_ACTION_PRESS
        input.keycode = UInt32(event.keyCode)
        input.mods = mods
        input.consumed_mods = ghostty_input_mods_e(rawValue: mods.rawValue & ~(GHOSTTY_MODS_CTRL.rawValue | GHOSTTY_MODS_SUPER.rawValue))
        input.unshifted_codepoint = event.charactersIgnoringModifiers?.unicodeScalars.first?.value ?? 0
        _ = Self.filteredText(text)?.withCString { input.text = $0; return ghostty_surface_key(surface, input) } ?? ghostty_surface_key(surface, input)
    }

    private static func keyEventText(_ event: NSEvent) -> String? {
        guard let characters = event.characters else { return nil }
        if characters.count == 1, let scalar = characters.unicodeScalars.first {
            if scalar.value < 0x20 { return event.characters(byApplyingModifiers: event.modifierFlags.subtracting(.control)) }
            if (0xF700...0xF8FF).contains(scalar.value) { return nil }
        }
        return characters
    }

    private static func filteredText(_ text: String?) -> String? {
        guard let text, let scalar = text.unicodeScalars.first else { return nil }
        return scalar.value < 0x20 || scalar.value == 0x7F ? nil : text
    }

    func viewportText() -> String {
        guard let surface else { return "" }
        let selection = ghostty_selection_s(
            top_left: ghostty_point_s(tag: GHOSTTY_POINT_VIEWPORT, coord: GHOSTTY_POINT_COORD_TOP_LEFT, x: 0, y: 0),
            bottom_right: ghostty_point_s(tag: GHOSTTY_POINT_VIEWPORT, coord: GHOSTTY_POINT_COORD_BOTTOM_RIGHT, x: 0, y: 0), rectangle: false)
        var result = ghostty_text_s()
        guard ghostty_surface_read_text(surface, selection, &result) else { return "" }
        defer { ghostty_surface_free_text(surface, &result) }
        return result.text.map { String(cString: $0) } ?? ""
    }

    func insertDropped(_ text: String) {
        dropped = text
        perform("paste_from_clipboard")
        dropped = nil
    }

    func takeDropped() -> String? {
        defer { dropped = nil }
        return dropped
    }

    private func sendText(_ text: String) {
        guard let surface else { return }
        text.withCString { ghostty_surface_text(surface, $0, UInt(text.utf8.count)) }
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        mouseButton(event, GHOSTTY_MOUSE_PRESS, GHOSTTY_MOUSE_LEFT)
    }
    override func mouseUp(with event: NSEvent) {
        mouseButton(event, GHOSTTY_MOUSE_RELEASE, GHOSTTY_MOUSE_LEFT)
        if let surface { ghostty_surface_mouse_pressure(surface, 0, 0) }
    }
    var paneMenu: NSMenu? {
        guard let surface, !ghostty_surface_mouse_captured(surface) else { return nil }
        return contextMenu?()
    }
    override func rightMouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        if let menu = paneMenu { return NSMenu.popUpContextMenu(menu, with: event, for: self) }
        mouseButton(event, GHOSTTY_MOUSE_PRESS, GHOSTTY_MOUSE_RIGHT)
    }
    override func rightMouseUp(with event: NSEvent) { mouseButton(event, GHOSTTY_MOUSE_RELEASE, GHOSTTY_MOUSE_RIGHT) }
    override func otherMouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        mouseButton(event, GHOSTTY_MOUSE_PRESS, Self.mouseButton(for: event.buttonNumber))
    }
    override func otherMouseUp(with event: NSEvent) { mouseButton(event, GHOSTTY_MOUSE_RELEASE, Self.mouseButton(for: event.buttonNumber)) }
    override func mouseMoved(with event: NSEvent) { mousePosition(event) }
    override func mouseDragged(with event: NSEvent) { mousePosition(event) }
    override func rightMouseDragged(with event: NSEvent) { mousePosition(event) }
    override func otherMouseDragged(with event: NSEvent) { mousePosition(event) }

    override func scrollWheel(with event: NSEvent) {
        guard let surface else { return }
        let scale = event.hasPreciseScrollingDeltas ? 2.0 : 1.0
        let mods = (event.hasPreciseScrollingDeltas ? 1 : 0) | (Self.momentum(event.momentumPhase) << 1)
        ghostty_surface_mouse_scroll(surface, event.scrollingDeltaX * scale, event.scrollingDeltaY * scale, ghostty_input_scroll_mods_t(mods))
    }

    override func pressureChange(with event: NSEvent) {
        if let surface { ghostty_surface_mouse_pressure(surface, UInt32(event.stage), Double(event.pressure)) }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let trackingArea { removeTrackingArea(trackingArea) }
        let area = NSTrackingArea(rect: bounds, options: [.mouseMoved, .cursorUpdate, .activeInKeyWindow, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(area)
        trackingArea = area
    }

    private func mouseButton(_ event: NSEvent, _ action: ghostty_input_mouse_state_e, _ button: ghostty_input_mouse_button_e) {
        guard let surface else { return }
        mousePosition(event)
        _ = ghostty_surface_mouse_button(surface, action, button, Self.mods(event.modifierFlags))
    }

    private func mousePosition(_ event: NSEvent) {
        guard let surface else { return }
        let point = convert(event.locationInWindow, from: nil)
        ghostty_surface_mouse_pos(surface, point.x, bounds.height - point.y, Self.mods(event.modifierFlags))
    }

    private static func mouseButton(for number: Int) -> ghostty_input_mouse_button_e {
        switch number {
        case 2: GHOSTTY_MOUSE_MIDDLE
        case 3: GHOSTTY_MOUSE_FOUR
        case 4: GHOSTTY_MOUSE_FIVE
        case 5: GHOSTTY_MOUSE_SIX
        case 6: GHOSTTY_MOUSE_SEVEN
        case 7: GHOSTTY_MOUSE_EIGHT
        case 8: GHOSTTY_MOUSE_NINE
        case 9: GHOSTTY_MOUSE_TEN
        case 10: GHOSTTY_MOUSE_ELEVEN
        default: GHOSTTY_MOUSE_UNKNOWN
        }
    }

    static func mods(_ flags: NSEvent.ModifierFlags) -> ghostty_input_mods_e {
        var value = GHOSTTY_MODS_NONE.rawValue
        if flags.contains(.shift) { value |= GHOSTTY_MODS_SHIFT.rawValue }
        if flags.contains(.control) { value |= GHOSTTY_MODS_CTRL.rawValue }
        if flags.contains(.option) { value |= GHOSTTY_MODS_ALT.rawValue }
        if flags.contains(.command) { value |= GHOSTTY_MODS_SUPER.rawValue }
        return ghostty_input_mods_e(rawValue: value)
    }

    private static func adjust(_ original: NSEvent.ModifierFlags, basedOn mods: ghostty_input_mods_e) -> NSEvent.ModifierFlags {
        var result = original
        for (flag, bit) in [(NSEvent.ModifierFlags.shift, GHOSTTY_MODS_SHIFT), (.control, GHOSTTY_MODS_CTRL), (.option, GHOSTTY_MODS_ALT), (.command, GHOSTTY_MODS_SUPER)] {
            if mods.rawValue & bit.rawValue != 0 { result.insert(flag) } else { result.remove(flag) }
        }
        return result
    }

    private static func momentum(_ phase: NSEvent.Phase) -> Int32 {
        if phase.contains(.mayBegin) { return 6 }
        if phase.contains(.began) { return 1 }
        if phase.contains(.stationary) { return 2 }
        if phase.contains(.changed) { return 3 }
        if phase.contains(.ended) { return 4 }
        if phase.contains(.cancelled) { return 5 }
        return 0
    }

    func characterIndex(for point: NSPoint) -> Int { NSNotFound }
    func hasMarkedText() -> Bool { marked.length > 0 }
    func markedRange() -> NSRange { marked.length > 0 ? NSRange(location: 0, length: marked.length) : NSRange(location: NSNotFound, length: 0) }
    func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

    func selectedRange() -> NSRange {
        guard let surface else { return NSRange(location: NSNotFound, length: 0) }
        var text = ghostty_text_s()
        guard ghostty_surface_read_selection(surface, &text) else { return NSRange(location: NSNotFound, length: 0) }
        defer { ghostty_surface_free_text(surface, &text) }
        return NSRange(location: Int(text.offset_start), length: Int(text.offset_len))
    }

    func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        if let value = string as? NSAttributedString { marked = NSMutableAttributedString(attributedString: value) }
        else if let value = string as? String { marked = NSMutableAttributedString(string: value) }
        syncPreedit()
    }

    func unmarkText() {
        marked = NSMutableAttributedString()
        syncPreedit()
    }

    func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? {
        guard let surface else { return nil }
        var text = ghostty_text_s()
        guard ghostty_surface_read_selection(surface, &text) else { return nil }
        defer { ghostty_surface_free_text(surface, &text) }
        guard let value = text.text else { return nil }
        actualRange?.pointee = NSRange(location: Int(text.offset_start), length: Int(text.offset_len))
        return NSAttributedString(string: String(cString: value))
    }

    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        guard let surface else { return .zero }
        var x = 0.0, y = 0.0, width = 0.0, height = 0.0
        ghostty_surface_ime_point(surface, &x, &y, &width, &height)
        let rect = convert(NSRect(x: x, y: bounds.height - y, width: width, height: height), to: nil)
        return window?.convertToScreen(rect) ?? rect
    }

    func insertText(_ string: Any, replacementRange: NSRange) {
        let text = (string as? NSAttributedString)?.string ?? (string as? String) ?? ""
        unmarkText()
        if var accumulating {
            accumulating.append(text)
            self.accumulating = accumulating
        } else {
            sendText(text)
        }
    }

    override func doCommand(by selector: Selector) {}

    private func syncPreedit() {
        guard let surface else { return }
        if marked.length == 0 { ghostty_surface_preedit(surface, nil, 0) }
        else { marked.string.withCString { ghostty_surface_preedit(surface, $0, UInt(marked.string.utf8.count)) } }
    }
}
