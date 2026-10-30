import AppKit
import KannaCode
import Testing
@testable import KannaDiff

struct DiffAsideTests {
    func shown(_ width: CGFloat, size: CGFloat = CodeTheme.fontSize) -> Bool {
        DiffAside.shown([.files], width: width, font: CodeTheme.font(size: size)).contains(.files)
    }

    func cutoff(_ size: CGFloat) -> CGFloat {
        DiffFileList.width + 1 + DiffAside.textColumns * DiffAside.columnWidth(CodeTheme.font(size: size))
    }

    @Test func theListShowsOnlyWhenTheTextKeepsItsColumns() {
        #expect(shown(cutoff(CodeTheme.fontSize)))
        #expect(!shown(cutoff(CodeTheme.fontSize) - 1))
    }

    @Test func theCutoffFollowsZoom() {
        let wide = cutoff(CodeTheme.fontSize)
        #expect(shown(wide, size: 9))
        #expect(!shown(wide, size: 16))
        #expect(cutoff(16) > cutoff(CodeTheme.fontSize) && cutoff(CodeTheme.fontSize) > cutoff(9))
    }

    @Test func aDeskFillingAMacBookScreenHidesIt() {
        for screen: CGFloat in [1512, 1728] {
            let total = screen - 1
            #expect(!shown(total - (total * 0.4).rounded()), "\(screen)")
        }
        #expect(shown(1920 - 1 - ((1920 - 1) * 0.4).rounded()))
    }
}
