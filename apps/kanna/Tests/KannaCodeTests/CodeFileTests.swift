import Foundation
import Testing
@testable import KannaCode

struct CodeFileTests {
    @Test func linesStartAfterEachNewline() {
        let file = CodeFile(text: "fn main() {\n    run();\n}\n")
        #expect(file.lineStarts == [0, 12, 23])
        #expect(file.lineCount == 3)
        #expect(file.line(at: 0) == 1)
        #expect(file.line(at: 11) == 1)
        #expect(file.line(at: 12) == 2)
        #expect(file.line(at: 23) == 3)
    }

    @Test func aLastLineWithoutANewlineCounts() {
        #expect(CodeFile(text: "a\nb").lineCount == 2)
        #expect(CodeFile(text: "").lineCount == 1)
    }

    @Test func aLinePastTheEndIsTheLastLine() {
        let file = CodeFile(text: "a\nb\nc\n")
        #expect(file.start(ofLine: 99) == 4)
        #expect(file.start(ofLine: 0) == 0)
    }

    @Test func linesCountUTF16LikeTheTextView() {
        let file = CodeFile(text: "let 🦀 = 1;\nnext")
        #expect(file.lineStarts == [0, ("let 🦀 = 1;\n" as NSString).length])
    }

    @Test func changedLinesAreTheNewFilesInsertedOrEditedLines() {
        let old = CodeFile(text: "a\nb\nc\nd\n")
        #expect(CodeFile(text: "a\nB\nc\nd\n").changedLines(from: old) == [2])
        #expect(CodeFile(text: "a\nb\nnew\nc\nd\n").changedLines(from: old) == [3])
        #expect(CodeFile(text: "a\nc\nd\n").changedLines(from: old) == [])
        #expect(CodeFile(text: "a\nb\nc\nd\n").changedLines(from: old) == [])
    }
}
