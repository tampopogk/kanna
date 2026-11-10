import Foundation

public struct FindQuery: Equatable, Sendable {
    public let text: String

    public init(_ text: String) {
        self.text = text
    }

    public var isEmpty: Bool { text.isEmpty }

    public var caseSensitive: Bool { text.contains { $0.isUppercase } }

    public func ranges(in string: String) -> [NSRange] {
        guard !text.isEmpty else { return [] }
        let haystack = string as NSString
        let options: NSString.CompareOptions = caseSensitive ? [.literal] : [.literal, .caseInsensitive]
        var found: [NSRange] = []
        var location = 0
        while location < haystack.length {
            let range = haystack.range(of: text, options: options, range: NSRange(location: location, length: haystack.length - location))
            guard range.location != NSNotFound, range.length > 0 else { break }
            found.append(range)
            location = NSMaxRange(range)
        }
        return found
    }

    public func matches(_ string: String) -> Bool {
        guard !text.isEmpty else { return false }
        return string.range(of: text, options: caseSensitive ? [.literal] : [.literal, .caseInsensitive]) != nil
    }
}
