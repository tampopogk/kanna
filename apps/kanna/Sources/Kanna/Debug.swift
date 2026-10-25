import AppKit
import GhosttyKit
import KannaDocs
import KannaPerf

extension AppController {
    func installCommands() {
        guard debugCommands else { return }
        TerminalDebug.install(commands, runtime: runtime) { [weak self] id in
            guard let self else { throw KannaError("app_gone", "Kanna.app is closing") }
            return try self.terminal(id)
        }
        commands["debug.app"] = { [weak self] args in
            guard let self else { return [:] }
            switch args["appearance"]?.string {
            case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
            case "light": NSApp.appearance = NSAppearance(named: .aqua)
            default: break
            }
            var reply: [String: JSON] = [
                "app": "kanna", "pid": .number(Double(ProcessInfo.processInfo.processIdentifier)),
                "bundle": .string(Bundle.main.bundleIdentifier ?? ""),
                "panes": .array(keyWindow.map { WindowController.paneIDs($0["root"] ?? .null).map { .string($0) } } ?? []),
                "windows": .array(controllers.keys.sorted().map { .string($0) }),
            ]
            if let daemon {
                reply["daemon"] = .string(daemon.socketPath)
                reply["attached"] = .bool(daemon.attached)
            } else {
                reply["socket"] = .string(Self.echoSocketPath)
            }
            return reply
        }
        commands["debug.type"] = { [weak self] args in
            guard let self else { return [:] }
            if args["focused"]?.bool == true {
                guard let text = args["text"]?.string, let window = state["key"]?.string.flatMap({ controllers[$0]?.window }) else {
                    throw KannaError("bad_request", "debug.type needs text and a key window")
                }
                let events = SyntheticKey.typing(text, window: window)
                for event in events { window.sendEvent(event) }
                return ["keys": .number(Double(events.count / 2)), "responder": .string(window.firstResponder.map { String(describing: type(of: $0)) } ?? "")]
            }
            let view = try terminal(args["pane"]?.string)
            guard let text = args["text"]?.string, let window = view.window else { throw KannaError("bad_request", "debug.type needs text") }
            if window.firstResponder !== view { window.makeFirstResponder(view) }
            let events = SyntheticKey.typing(text, window: window)
            for event in events { window.sendEvent(event) }
            return ["keys": .number(Double(events.count / 2))]
        }
        commands["debug.key"] = { [weak self] args in try self?.pressKey(args) ?? [:] }
        commands["debug.click"] = { [weak self] args in try self?.click(args) ?? [:] }
        commands["debug.drag"] = { [weak self] args in try await self?.debugDrag(args) ?? [:] }
        commands["debug.menu"] = { [weak self] args in
            guard let self else { return [:] }
            if let title = args["item"]?.string { try choose(menuItem: title) }
            return ["items": .array(menuItems)]
        }
        commands["debug.perf"] = { args in
            if args["reset"]?.bool == true { Perf.reset() }
            let ms = { (value: Double) -> JSON in .number((value * 10).rounded() / 10) }
            let intervals = Perf.stats().map { stats -> JSON in
                ["name": .string(stats.name), "count": .number(Double(stats.count)), "recent": .number(Double(stats.recent)),
                 "p50": ms(stats.p50), "p95": ms(stats.p95), "max": ms(stats.max), "last": ms(stats.last), "detail": stats.detail.map { .string($0) } ?? .null]
            }
            let hangs = Perf.hangs().reversed().map { hang -> JSON in
                ["at": .string(ISO8601DateFormatter().string(from: hang.at)), "ms": ms(hang.ms),
                 "open": .array(hang.open.map { .string($0) }), "ran": .array(hang.ran.map { .string($0) })]
            }
            return ["intervals": .array(intervals), "hangs": .array(hangs), "open": .array(Perf.open.map { .string($0) })]
        }
        commands["debug.hang"] = { args in
            let ms = args["ms"]?.number ?? 300
            usleep(useconds_t(ms * 1000))
            return ["slept": .number(ms)]
        }
        commands["debug.stats"] = { _ in
            ["pid": .number(Double(ProcessInfo.processInfo.processIdentifier)), "footprint": .number(Double(memoryFootprint()))]
        }
        commands["debug.drop"] = { [weak self] args in
            guard let self else { return [:] }
            let view = try terminal(args["pane"]?.string)
            let pasteboard = NSPasteboard(name: NSPasteboard.Name("dev.kanna.debug-drop.\(UUID().uuidString)"))
            defer { pasteboard.releaseGlobally() }
            pasteboard.clearContents()
            if case .array(let list)? = args["files"] {
                pasteboard.writeObjects(list.compactMap { $0.string.map { NSURL(fileURLWithPath: $0) } })
            } else if let image = args["image"]?.string {
                guard let data = FileManager.default.contents(atPath: image) else { throw KannaError("bad_request", "no such image: \(image)") }
                pasteboard.setData(data, forType: image.lowercased().hasSuffix(".png") ? .png : .tiff)
            } else if let text = args["text"]?.string {
                pasteboard.setString(text, forType: .string)
            } else {
                throw KannaError("bad_request", "debug.drop needs files, image or text")
            }
            return ["dropped": .bool(view.drop(pasteboard))]
        }
        commands["debug.pinch"] = { [weak self] args in try await self?.pinch(args) ?? [:] }
        commands["debug.text"] = { [weak self] args in
            guard let self else { return [:] }
            let view = try terminal(args["pane"]?.string)
            var reply: [String: JSON] = ["text": .string(view.viewportText())]
            if let surface = view.surface {
                let size = ghostty_surface_size(surface)
                reply["columns"] = .number(Double(size.columns))
                reply["rows"] = .number(Double(size.rows))
            }
            return reply
        }
        commands["debug.view"] = { [weak self] _ in
            guard let self, case .object(let view) = state else { return [:] }
            return view
        }
        commands["debug.snapshot"] = { [weak self] args in
            guard let self else { return [:] }
            let id = args["window"]?.string ?? state["key"]?.string ?? controllers.keys.sorted().first
            guard let id, let controller = controllers[id], let window = window(id) else { throw KannaError("not_found", "no window \"\(args["window"]?.string ?? "")\"") }
            let shown: [(id: String, view: NSView)] = shownPaneViews(window).map { ($0.id, $0.view.terminal ?? $0.view) }
            var reply = try await Snapshot.run(controller.window, id: id, panes: shown, tabBar: controller.tabBar, path: args["path"]?.string, marker: args["marker"]?.string)
            if case .array(let list)? = reply["panes"] {
                reply["panes"] = .array(list.map { entry in
                    guard case .object(var fields) = entry, let pane = entry["id"]?.string, let message = panes[pane]?.message else { return entry }
                    fields["notice"] = .string(message)
                    return .object(fields)
                })
            }
            reply["tabs"] = window["tabs"].map { tabs in
                guard case .array(let list) = tabs else { return tabs }
                return .array(list.compactMap { $0["title"] })
            } ?? []
            return reply
        }
        commands["debug.fleet"] = { [weak self] _ in
            guard let self, let links else { return [:] }
            let remotes = links.remotes.keys.sorted().map { machine -> JSON in
                let link = links.remotes[machine]
                return ["machine": .string(machine), "attached": .bool(link?.attached == true), "path": links.path(of: machine),
                        "failure": links.failure(of: machine) ?? .null, "daemon": link?.daemonInfo ?? .null]
            }
            return ["name": links.name.map { .string($0) } ?? .null, "links": .array(remotes)]
        }
        commands["debug.desk"] = { [weak self] args in
            guard let self else { return [:] }
            let id = args["window"]?.string ?? state["key"]?.string ?? ""
            guard let controller = controllers[id] else { throw KannaError("not_found", "no window \"\(id)\"") }
            if case .array(let size)? = args["size"], size.count == 2, let width = size[0].number, let height = size[1].number {
                controller.window.setContentSize(NSSize(width: width, height: height))
            }
            if let y = args["scroll"]?.number { controller.shelfPage.scroll(toY: CGFloat(y)) }
            let page = controller.shelfPage
            let scrolled: JSON = ["top": .number(Double(page.scrollTop)), "height": .number(Double(page.contentHeight)),
                                  "visible": .number(Double(page.visibleHeight)), "backlog": .array(page.visibleBacklog.map { .string($0) }), "blocked": .array(page.blockedBacklog)]
            let search: JSON = ["shown": .bool(page.searchShown), "focused": .bool(page.searching), "query": .string(page.query),
                                "found": .array(page.found.map { .string($0) }), "selected": .number(Double(page.selected)),
                                "left": .number(Double(page.searchFrame.minX)), "right": .number(Double(page.searchFrame.maxX))]
            let tasks = controller.shelf.map { task -> JSON in ["task": .string(task.id), "title": .string(task.title), "status": task.status.map { .string($0) } ?? .null, "agent": task.agent.map { .string($0) } ?? .null] }
            return ["desk": .bool(controller.desk), "empty": .bool(controller.emptyDesk), "shelf": .array(tasks),
                    "strip": .bool(!controller.shelfStrip.isHidden), "page": .bool(!controller.shelfPage.isHidden), "title": .string(controller.window.title),
                    "header": controller.agentHeader.isHidden ? .null : .string(controller.agentHeader.text),
                    "backlog": .array(controller.backlog.map { .string($0.id) }), "agents": .array(controller.backlog.map { $0.agent.map { .string($0) } ?? .null }), "running": .number(Double(controller.running)),
                    "waiting": .string(controller.shelfStrip.waitingText), "stripLayout": controller.shelfStrip.geometry, "note": .string(controller.note), "cheer": .string(controller.cheer),
                    "lead": controller.leadShown.map { .string($0) } ?? .null, "window": window(id) ?? .null, "scroll": scrolled, "search": search,
                    "responder": .string(controller.window.firstResponder.map { String(describing: type(of: $0)) } ?? "")]
        }
        commands["debug.web"] = { [weak self] args in
            guard let self, let id = args["pane"]?.string, let web = contents.web(id) else { throw KannaError("wrong_type", "not a web pane") }
            if let script = args["js"]?.string {
                let value = try? await web.webView.evaluateJavaScript(script)
                let data = value.flatMap { try? JSONSerialization.data(withJSONObject: $0, options: [.fragmentsAllowed]) }
                return ["value": .string(data.map { String(decoding: $0, as: UTF8.self) } ?? "null"), "magnification": .number(Double(web.webView.magnification))]
            }
            let text = try? await web.webView.evaluateJavaScript("document.body ? document.body.innerText : ''") as? String
            var reply: [String: JSON] = ["url": web.webView.url.map { .string($0.absoluteString) } ?? .null, "text": .string(text ?? ""), "zoom": .number(Double(web.zoom))]
            guard let markup = web.markup else { return reply }
            let point = { (rect: NSRect) -> JSON in [.number(Double(rect.midX)), .number(Double(markup.bounds.height - rect.midY))] }
            if let selector = args["find"]?.string {
                guard let rect = await markup.rect(of: selector) else { throw KannaError("not_found", "nothing matches \(selector)") }
                return ["at": point(rect), "width": .number(Double(rect.width)), "height": .number(Double(rect.height))]
            }
            reply["cards"] = .object(markup.cards.mapValues { ["at": point($0.rect), "field": point($0.field), "reply": .bool($0.reply), "typing": .bool($0.typing), "waiting": $0.waiting.map { .string($0) } ?? .null] })
            reply["dot_typing"] = .array(markup.typingDots.map { .string($0) })
            reply["threads"] = .array(markup.threads.map { thread in
                var fields: [String: JSON] = ["id": .string(thread.id), "quote": .string(thread.quote), "line": .number(Double(thread.line)), "state": .string(thread.state),
                                              "status": thread.status.map { .string($0) } ?? .null, "error": thread.error.map { .string($0) } ?? .null,
                                              "mark": thread.mark.map { .string($0) } ?? .null,
                                              "messages": .array(thread.messages.map { .string("\($0.who): \($0.text)") })]
                if let page = thread.page {
                    fields["page"] = ["selector": .string(page.selector), "text": .string(page.text), "hash": .string(page.hash),
                                      "box": .array(page.box.map { .number(Double($0)) })]
                }
                return .object(fields)
            })
            reply["marks"] = .object(markup.markRects.mapValues(point))
            reply["areas"] = .object(markup.areaRects.mapValues(point))
            reply["box"] = markup.commentBoxThread.map { .string($0) } ?? .null
            reply["box_parts"] = Self.boxParts(markup.commentBoxParts)
            reply["box_shown"] = .bool(markup.commentBoxShown)
            reply["active"] = markup.activeThread.map { .string($0) } ?? .null
            reply["agent"] = markup.agent.map { .string($0) } ?? .null
            reply["hash"] = .string(markup.pageHash)
            return reply
        }
        commands["debug.md"] = { [weak self] args in
            guard let self, let id = args["pane"]?.string, let markdown = contents.markdown(id) else { throw KannaError("wrong_type", "not a running md pane") }
            if let step = args["zoom"]?.number { markdown.zoom(by: Int(step)) }
            if let y = args["scroll"]?.number { markdown.scroll(toY: CGFloat(y)) }
            if let step = args["change"]?.number { markdown.jump(by: Int(step)) }
            if case .array(let pick)? = args["select"], pick.count == 2, let at = pick[0].number, let length = pick[1].number {
                markdown.select(NSRange(location: Int(at), length: Int(length)))
            }
            if let id = args["mark"]?.string {
                guard let rect = markdown.markRect(id) else { throw KannaError("not_found", "no mark for \(id)") }
                return ["x": .number(Double(rect.midX)), "y": .number(Double(markdown.bounds.height - rect.midY))]
            }
            let pressed: JSON = args["tour"]?.string.map { .bool(markdown.pressTour($0)) } ?? .null
            let footer: JSON = markdown.tourFooter.map { ["thread": .string($0.thread), "at": .number(Double($0.at)), "of": .number(Double($0.of)), "back": .bool($0.back), "next": .string($0.next)] } ?? .null
            let threads: [JSON] = markdown.threads.map { thread in
                ["id": .string(thread.id), "quote": .string(thread.quote), "line": .number(Double(thread.line)), "state": .string(thread.state),
                 "status": thread.status.map { .string($0) } ?? .null, "error": thread.error.map { .string($0) } ?? .null,
                 "messages": .array(thread.messages.map { .string("\($0.who): \($0.text)") })]
            }
            let diagrams: [JSON] = markdown.drawnDiagrams.map { ["type": .string($0.type), "labels": .array($0.labels.map { .string($0) })] }
            return ["text": .string(markdown.text), "zoom": .number(Double(markdown.zoom)), "diagrams": .array(diagrams),
                    "render_ms": .number(markdown.renderMs), "parse_ms": .number(markdown.parseMs), "scroll": .number(Double(markdown.scrollTop)),
                    "layout_width": .number(Double(markdown.layoutWidth)), "threads": .array(threads),
                    "box": markdown.commentBoxThread.map { .string($0) } ?? .null, "box_text": markdown.commentBoxText.map { .string($0) } ?? .null,
                    "active": markdown.activeThread.map { .string($0) } ?? .null, "agent": markdown.agent.map { .string($0) } ?? .null,
                    "markup": .bool(markdown.markup), "diff": markdown.diffBase.map { .string($0) } ?? .null,
                    "changes": markdown.diffBase == nil ? .null : .number(Double(markdown.changes.count)), "diff_error": markdown.diffError.map { .string($0) } ?? .null,
                    "box_parts": Self.boxParts(markdown.commentBoxParts), "box_shown": .bool(markdown.commentBoxShown),
                    "cards": .object(markdown.cards.mapValues { ["typing": .bool($0.typing), "waiting": $0.waiting.map { .string($0) } ?? .null, "reply": .bool($0.reply)] }),
                    "tour": footer, "pressed": pressed]
        }
        commands["debug.code"] = { [weak self] args in
            guard let self, let id = args["pane"]?.string, let code = contents.code(id) else { throw KannaError("wrong_type", "not a running code pane") }
            if let line = args["reveal"]?.number { code.reveal(line: Int(line)) }
            if let line = args["pick"]?.number { code.reveal(line: Int(line), toTop: true) }
            if case .array(let pick)? = args["select"], pick.count == 3, let line = pick[0].number, let column = pick[1].number, let length = pick[2].number {
                code.select(line: Int(line), column: Int(column), length: Int(length))
            }
            var out: [String: JSON] = code.debugNumbers.mapValues { .number($0) }
            out["text"] = .string(code.file.text)
            out["path"] = .string(code.path)
            out["language"] = code.language.map { .string($0) } ?? .null
            out["comment_text"] = code.commentText.map { .string($0) } ?? .null
            if let to = args["tour"]?.string { out["pressed"] = .bool(code.pressTour(to)) }
            out["tour"] = code.tourFooter.map { ["at": .number(Double($0.at)), "of": .number(Double($0.of)), "back": .bool($0.back), "next": .string($0.next), "visible": .bool($0.visible)] } ?? .null
            out["pinned_rows"] = .array(code.pinnedMarks.map { line, marks in
                let ranges: ([NSRange]) -> JSON = { .array($0.map { .array([.number(Double($0.location)), .number(Double($0.length))]) }) }
                return ["line": .number(Double(line)), "selection": ranges(marks.selection), "matches": ranges(marks.matches), "current": .bool(marks.current),
                        "comment": .bool(marks.commented), "dimmed": .bool(marks.dimmed), "changed": .bool(marks.changed), "flashing": .bool(marks.flashing)]
            })
            out["captures"] = .array(code.highlights.spans.prefix(200).map { .array([.number(Double($0.range.location)), .number(Double($0.range.length)), .string($0.capture)]) })
            return out
        }
        commands["debug.diff"] = { [weak self] args in
            guard let self, let id = args["pane"]?.string, let diff = contents.diff(id) else { throw KannaError("wrong_type", "not a running diff pane") }
            if let keys = args["keys"]?.string { for key in keys { _ = diff.press(key == "~" ? "\r" : String(key)) } }
            if let view = args["view"]?.string { diff.setView(view) }
            if let step = args["zoom"]?.number { diff.zoom(by: Int(step)) }
            if let path = args["fold"]?.string { diff.toggle(file: path, all: false) }
            if let path = args["show"]?.string { diff.showFolded(path) }
            let rows: [JSON] = diff.debugRows.map { row in
                .object(row.mapValues { value in
                    if let text = value as? String { return .string(text) }
                    if let number = value as? Int { return .number(Double(number)) }
                    return .null
                })
            }
            let files: [JSON] = (diff.result?.files ?? []).map { file in
                ["path": .string(file.path), "status": .string(file.status), "added": .number(Double(file.added)), "removed": .number(Double(file.removed)),
                 "fold": file.fold.map { .string($0) } ?? .null, "untracked": .bool(file.untracked == true)]
            }
            return ["command": .string(diff.result?.command ?? ""), "error": diff.result?.error.map { .string($0) } ?? .null, "files": .array(files), "rows": .array(rows),
                    "current": diff.current.map { .number(Double($0)) } ?? .null, "view": .string(diff.view.rawValue), "updating": .bool(diff.updating),
                    "serial": .number(Double(diff.serial)), "fresh": .array(diff.fresh.sorted().map { .number(Double($0)) }), "zoom": .number(Double(diff.zoom)),
                    "highlighted": .number(Double(diff.highlightedFiles)), "list": .bool(diff.listShown),
                    "selected": diff.selectedFile.map { .string($0) } ?? .null, "sticky": diff.stickyFile.map { .string($0) } ?? .null,
                    "columns": .array(diff.debugColumns.map { .object($0.mapValues { .number($0) }) }),
                    "colors": .array((args["colors"]?.number).map { diff.colors(atRow: Int($0)).map { .string($0) } } ?? [])]
        }
        commands["debug.ios"] = { [weak self] args in
            guard let self, let id = args["pane"]?.string, let screen = contents.ios(id)?.screen else { throw KannaError("wrong_type", "not a running ios pane") }
            if let x = args["x"]?.number, let y = args["y"]?.number {
                screen.touch(.leftMouseDown, at: CGPoint(x: x, y: y))
                try await Task.sleep(for: .milliseconds(80))
                screen.touch(.leftMouseUp, at: CGPoint(x: x, y: y))
            }
            if args["home"]?.bool == true { screen.pressHome() }
            return ["width": .number(screen.pixelSize.width), "height": .number(screen.pixelSize.height)]
        }
    }

    private static func boxParts(_ parts: [String: NSRect]?) -> JSON {
        guard let parts else { return .null }
        return .object(parts.mapValues { .array([$0.minX, $0.minY, $0.width, $0.height].map { .number(Double($0)) }) })
    }

    private func terminal(_ reference: String?) throws -> TerminalSurfaceView {
        let shown = keyWindow.map { WindowController.shownPanes($0) } ?? []
        let id = try reference.map(resolve) ?? focusedPane.flatMap { panes.terminal($0) != nil ? $0 : nil } ?? shown.first { panes.terminal($0) != nil }
        guard let id, let view = panes[id] else { throw KannaError("not_found", "no pane \"\(reference ?? "")\" on screen") }
        guard let terminal = view.terminal else { throw KannaError("wrong_type", "\(id) is not a terminal") }
        guard terminal.surface != nil else { throw KannaError("not_found", "\(id)'s terminal is not running") }
        return terminal
    }

    private func pressKey(_ args: JSON) throws -> [String: JSON] {
        guard let name = args["key"]?.string else { throw KannaError("bad_request", "debug.key needs a key") }
        var window = state["key"]?.string.flatMap { controllers[$0]?.window }
        if let reference = args["pane"]?.string {
            let view = try terminal(reference)
            window = view.window
            if window?.firstResponder !== view { window?.makeFirstResponder(view) }
        }
        guard let window else { throw KannaError("not_found", "no window") }
        guard let event = SyntheticKey.event(name, window: window) else { throw KannaError("bad_request", "unknown key \"\(name)\"") }
        if !event.modifierFlags.intersection([.command, .control, .option]).isEmpty, window.performKeyEquivalent(with: event)
            || Self.performResponderItem(event, in: window) || NSApp.mainMenu?.performKeyEquivalent(with: event) == true {
            return ["characters": .string(event.characters ?? ""), "charactersIgnoringModifiers": .string(event.charactersIgnoringModifiers ?? ""), "menu": true]
        }
        window.sendEvent(event)
        return ["characters": .string(event.characters ?? ""), "charactersIgnoringModifiers": .string(event.charactersIgnoringModifiers ?? "")]
    }

    private static func performResponderItem(_ event: NSEvent, in window: NSWindow) -> Bool {
        func find(_ menu: NSMenu?) -> NSMenuItem? {
            for item in menu?.items ?? [] {
                if let found = find(item.submenu) { return found }
                let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
                if item.target == nil, item.action != nil, !item.keyEquivalent.isEmpty, item.keyEquivalent == event.charactersIgnoringModifiers,
                   item.keyEquivalentModifierMask.intersection([.command, .control, .option, .shift]) == modifiers { return item }
            }
            return nil
        }
        guard let item = find(NSApp.mainMenu), let action = item.action, let responder = window.firstResponder else { return false }
        return responder.tryToPerform(action, with: item)
    }

    private func click(_ args: JSON) throws -> [String: JSON] {
        let window: NSWindow, point: NSPoint
        if let reference = args["pane"]?.string {
            let id = try resolve(reference)
            guard let view = panes[id], let paneWindow = view.window else { throw KannaError("not_found", "pane \"\(id)\" is not on screen") }
            var at = [view.bounds.midX, view.bounds.midY]
            if case .array(let list)? = args["at"] { at = list.compactMap(\.number).map { CGFloat($0) } }
            guard at.count == 2 else { throw KannaError("bad_request", "at is [x, y]") }
            window = paneWindow
            point = view.convert(NSPoint(x: at[0], y: view.bounds.height - at[1]), to: nil)
        } else if let task = args["shelf"]?.string {
            let id = args["window"]?.string ?? state["key"]?.string ?? ""
            guard let controller = controllers[id] else { throw KannaError("not_found", "no window \"\(id)\"") }
            let spots = controller.emptyDesk ? controller.shelfPage.rows : controller.shelfStrip.items
            guard let spot = spots.first(where: { $0.task == task }) else { throw KannaError("not_found", "\(task) is not on \(id)'s Shelf") }
            window = controller.window
            point = spot.center
        } else if let task = args["backlog"]?.string {
            let id = args["window"]?.string ?? state["key"]?.string ?? ""
            guard let controller = controllers[id], let spot = controller.shelfPage.backlogRows.first(where: { $0.task == task }) else { throw KannaError("not_found", "\(task) is not in \(id)'s backlog") }
            guard controller.shelfPage.visibleBacklog.contains(task) else { throw KannaError("not_found", "\(task) is scrolled out of \(id)'s view") }
            window = controller.window
            point = spot.center
        } else if let id = args["park"]?.string {
            guard let controller = controllers[id], !controller.agentHeader.isHidden else { throw KannaError("not_found", "\(id) shows no agent header") }
            window = controller.window
            point = controller.agentHeader.parkCenter
        } else {
            let id = args["tab"]?.string ?? ""
            guard let state = windowOf(tab: id), let controller = state["id"]?.string.flatMap({ controllers[$0] }),
                  let center = controller.tabBar.center(of: id) else { throw KannaError("not_found", "no tab \"\(id)\" on screen") }
            window = controller.window
            point = center
        }
        if let button = args["button"]?.number {
            let type: NSEvent.EventType = button == 1 ? .rightMouseDown : .otherMouseDown
            guard let cg = NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                              windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)?.cgEvent
            else { throw KannaError("bad_request", "can't make that mouse event") }
            cg.setIntegerValueField(.mouseEventButtonNumber, value: Int64(button))
            guard let down = NSEvent(cgEvent: cg) else { throw KannaError("bad_request", "can't make that mouse event") }
            if button == 1 {
                var view = window.contentView?.hitTest(window.contentView?.convert(point, from: nil) ?? point)
                var menu: NSMenu?
                while let current = view, menu == nil {
                    if let terminal = current as? TerminalSurfaceView {
                        menu = terminal.paneMenu ?? NSMenu()
                    } else {
                        menu = current.menu(for: down)
                    }
                    view = current.superview
                }
                let items = (menu?.items ?? []).filter { !$0.isSeparatorItem }.map { JSON.string($0.title) }
                return ["menu": .array(items)]
            }
            window.sendEvent(down)
            return [:]
        }
        PageMarkupView.scriptedCommand = args["command"]?.bool == true
        var drag: NSPoint?
        if case .array(let list)? = args["to"], let reference = args["pane"]?.string, let view = panes[try resolve(reference)] {
            let to = list.compactMap(\.number).map { CGFloat($0) }
            guard to.count == 2 else { throw KannaError("bad_request", "to is [x, y]") }
            drag = view.convert(NSPoint(x: to[0], y: view.bounds.height - to[1]), to: nil)
        }
        for count in 1...max(1, Int(args["clicks"]?.number ?? 1)) {
            func event(_ type: NSEvent.EventType, at location: NSPoint? = nil) -> NSEvent? {
                NSEvent.mouseEvent(with: type, location: location ?? point, modifierFlags: args["command"]?.bool == true ? [.command] : [],
                                   timestamp: ProcessInfo.processInfo.systemUptime,
                                   windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: count, pressure: 1)
            }
            guard let down = event(.leftMouseDown), let up = event(.leftMouseUp, at: drag) else { continue }
            if let drag {
                for step in 1...4 {
                    let t = CGFloat(step) / 4
                    if let moved = event(.leftMouseDragged, at: NSPoint(x: point.x + (drag.x - point.x) * t, y: point.y + (drag.y - point.y) * t)) {
                        NSApp.postEvent(moved, atStart: false)
                    }
                }
            }
            NSApp.postEvent(up, atStart: false)
            if window.isKeyWindow {
                NSApp.sendEvent(down)
            } else if let content = window.contentView, let view = content.hitTest(content.convert(point, from: nil)) {
                if view.acceptsFirstResponder { window.makeFirstResponder(view) }
                view.mouseDown(with: down)
            }
        }
        return [:]
    }

    private func pinch(_ args: JSON) async throws -> [String: JSON] {
        let id = try resolve(args["pane"]?.string ?? focusedPane ?? "")
        guard let view = panes.keyView(id), let window = view.window else { throw KannaError("wrong_type", "pane is not on screen") }
        if args["outline"]?.bool == true { panes[id]?.focused = true }
        var steps = [0.1, 0.1, 0.1, 0.1, 0.1]
        if case .array(let list)? = args["steps"] { steps = list.compactMap(\.number) }
        let area = view.visibleRect.intersection(view.bounds)
        let point = view.convert(NSPoint(x: area.midX, y: area.midY), to: nil)
        let height = NSScreen.screens.first?.frame.height ?? 0
        let gesture = CGEventType(rawValue: 29)!
        let gestureKind = CGEventField(rawValue: 110)!
        let magnification = CGEventField(rawValue: 113)!
        let phaseField = CGEventField(rawValue: 132)!
        let zoomGesture: Int64 = 8
        var zooms: [JSON] = [], renders: [JSON] = []
        let phases = [(1, 0.0)] + steps.map { (2, $0) } + [(4, 0.0)]
        for (phase, value) in phases {
            guard let cg = CGEvent(source: nil) else { continue }
            cg.type = gesture
            cg.location = CGPoint(x: point.x, y: height - point.y)
            cg.setIntegerValueField(gestureKind, value: zoomGesture)
            cg.setDoubleValueField(magnification, value: value)
            cg.setIntegerValueField(phaseField, value: Int64(phase))
            guard let event = NSEvent(cgEvent: cg) else { continue }
            window.sendEvent(event)
            try await Task.sleep(for: .milliseconds(40))
            if phase == 2, let markdown = contents.markdown(id) {
                zooms.append(.number(Double(markdown.zoom)))
                renders.append(.number(markdown.renderMs))
            } else if phase == 2, let web = contents.web(id) {
                zooms.append(.number(Double(web.zoom)))
            }
        }
        return ["zooms": .array(zooms), "render_ms": .array(renders)]
    }
}

private func memoryFootprint() -> UInt64 {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
    let result = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count) }
    }
    return result == KERN_SUCCESS ? info.phys_footprint : 0
}
