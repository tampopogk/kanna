import AppKit
import GhosttyKit

if ghostty_init(UInt(CommandLine.argc), CommandLine.unsafeArgv) != GHOSTTY_SUCCESS {
    FileHandle.standardError.write(Data("kanna: ghostty_init failed\n".utf8))
    exit(1)
}
CrashHandlers.install()

let app = KannaApplication.shared
let controller = AppController()
app.delegate = controller
app.setActivationPolicy(.regular)
app.run()
