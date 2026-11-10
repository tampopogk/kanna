import AppKit
import GhosttyKit

if !GhosttyResources.initialize(CommandLine.argc, CommandLine.unsafeArgv) {
    FileHandle.standardError.write(Data("kanna: ghostty_init failed\n".utf8))
    exit(1)
}
CrashHandlers.install()

let app = KannaApplication.shared
let controller = AppController()
app.delegate = controller
app.setActivationPolicy(.regular)
app.run()
