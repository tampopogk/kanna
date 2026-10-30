import Foundation
import KannaCode

extension AppController {
    func installAppCommands() {
        commands["app.machine.confirm"] = { args in
            ["confirmed": .bool(await MachineAlert.confirm(args))]
        }
        commands["app.snap"] = { args in
            guard let path = args["path"]?.string, !path.isEmpty, let line = args["line"]?.number.map({ Int($0) }), line > 0 else {
                throw KannaError("bad_request", "app.snap needs a path and a line")
            }
            let last = args["last"]?.number.map { Int($0) }
            let snapped = try await Task.detached { () throws -> CodeScope in
                let file: CodeFile
                switch CodeFile.read(path) {
                case .success(let read): file = read
                case .failure(let failure): throw KannaError("not_found", failure.message)
                }
                guard line <= file.lineCount else { throw KannaError("bad_request", "\(path) has \(file.lineCount) lines") }
                let firstLine = String(file.text.prefix { $0 != "\n" })
                let scopes = CodeSyntax.language(for: path, firstLine: firstLine).map { CodeSyntax.highlight(file.text, language: $0).scopes } ?? []
                return CodeScopes.snap(line: line, last: last, in: scopes, lineCount: file.lineCount)
            }.value
            return ["first": .number(Double(snapped.first)), "last": .number(Double(snapped.last))]
        }
    }
}
