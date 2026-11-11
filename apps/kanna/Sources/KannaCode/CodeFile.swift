import Foundation

public struct CodeFile: Equatable {
    public let text: String
    public let lineStarts: [Int]

    public init(text: String) {
        self.text = text
        var starts = [0]
        var offset = 0
        for unit in text.utf16 {
            offset += 1
            if unit == 0x0a { starts.append(offset) }
        }
        if starts.count > 1, starts.last == offset { starts.removeLast() }
        lineStarts = starts
    }

    public var lineCount: Int { lineStarts.count }

    public func line(at offset: Int) -> Int {
        var low = 0
        var high = lineStarts.count - 1
        while low < high {
            let mid = (low + high + 1) / 2
            if lineStarts[mid] <= offset { low = mid } else { high = mid - 1 }
        }
        return low + 1
    }

    public func start(ofLine line: Int) -> Int { lineStarts[min(max(line, 1), lineCount) - 1] }

    public var lines: [Substring] { text.split(separator: "\n", omittingEmptySubsequences: false) }

    public func changedLines(from old: CodeFile) -> Set<Int> {
        var changed = Set<Int>()
        for change in lines.difference(from: old.lines) {
            if case .insert(let offset, _, _) = change { changed.insert(offset + 1) }
        }
        return changed.filter { $0 <= lineCount }
    }
}
