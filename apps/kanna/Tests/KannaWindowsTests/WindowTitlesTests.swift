import Testing
@testable import KannaWindows

struct WindowTitlesTests {
    let home = "/Users/me"

    @Test func aProjectIsNamedByItsFolder() {
        let names = WindowTitles.projectNames(["/Users/me/.kanna-v4/repos/tampopogk/kanna", "/Users/me/work/pencilword", "/Users/me", "/opt/notes"], home: home)
        #expect(names == ["/Users/me/.kanna-v4/repos/tampopogk/kanna": "kanna", "/Users/me/work/pencilword": "pencilword", "/Users/me": "~", "/opt/notes": "notes"])
    }

    @Test func twoProjectsOfOneNameShowTheirOwners() {
        let names = WindowTitles.projectNames(["/Users/me/.kanna-v4/repos/tampopogk/kanna", "/Users/me/.kanna-v4/repos/other/kanna", "/Users/me/work/pencilword"], home: home)
        #expect(names["/Users/me/.kanna-v4/repos/tampopogk/kanna"] == "tampopogk/kanna")
        #expect(names["/Users/me/.kanna-v4/repos/other/kanna"] == "other/kanna")
        #expect(names["/Users/me/work/pencilword"] == "pencilword")
    }

    @Test func pathsGrowUntilTheyDiffer() {
        let names = WindowTitles.projectNames(["/Users/me/a/src/app", "/Users/me/b/src/app"], home: home)
        #expect(names == ["/Users/me/a/src/app": "a/src/app", "/Users/me/b/src/app": "b/src/app"])
    }

    @Test func theHomeFolderKeepsItsTilde() {
        let names = WindowTitles.projectNames(["/Users/me/x/notes", "/Users/me/notes"], home: home)
        #expect(names == ["/Users/me/x/notes": "x/notes", "/Users/me/notes": "~/notes"])
    }

    @Test func theSameFolderOnAnotherMachineShowsTheMachine() {
        let names = WindowTitles.projectNames(["/Users/me/r/kanna", "studio:/Users/me/r/kanna"], home: home)
        #expect(names == ["/Users/me/r/kanna": "kanna", "studio:/Users/me/r/kanna": "studio:kanna"])
    }

    @Test func aTitleJoinsItsParts() {
        #expect(WindowTitles.title("kanna", "Fix login", nil) == "kanna — Fix login")
        #expect(WindowTitles.title("kanna", "Fix login", "store.rs") == "kanna — Fix login — store.rs")
    }
}
