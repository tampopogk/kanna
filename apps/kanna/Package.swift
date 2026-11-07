// swift-tools-version: 6.0
import Foundation
import PackageDescription

let checkout = Context.packageDirectory + "/../.."
var gitIsFolder: ObjCBool = false
let inWorktree = FileManager.default.fileExists(atPath: checkout + "/.git", isDirectory: &gitIsFolder) && !gitIsFolder.boolValue
let rustProfile = Context.environment["KANNA_RUST_PROFILE"] ?? (inWorktree ? "debug" : "release")
let kannaFFI = LinkerSetting.unsafeFlags(["-L", Context.environment["KANNA_RUST_DIR"] ?? checkout + "/target/" + rustProfile])

let package = Package(
    name: "kanna",
    platforms: [.macOS(.v15)],
    products: [.executable(name: "Kanna", targets: ["Kanna"])],
    dependencies: [
        .package(url: "https://github.com/swiftlang/swift-markdown.git", exact: "0.7.3"),
        .package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.1"),
    ],
    targets: [
        .binaryTarget(name: "GhosttyKit", path: "../../target/ghosttykit/GhosttyKit.xcframework"),
        .systemLibrary(name: "KannaLayoutC", path: "Sources/KannaLayoutC"),
        .systemLibrary(name: "KannaMerman", path: "Sources/KannaMerman"),
        .systemLibrary(name: "KannaCallTreeC", path: "Sources/KannaCallTreeC"),
        .systemLibrary(name: "KannaSyntaxC", path: "Sources/KannaSyntaxC"),
        .systemLibrary(name: "KannaMachinesC", path: "Sources/KannaMachinesC"),
        .systemLibrary(name: "KannaCrashC", path: "Sources/KannaCrashC"),
        .target(name: "KannaDiagram", dependencies: ["KannaMerman"], linkerSettings: [kannaFFI] + ["AppKit", "CoreText"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaDiagramTests", dependencies: ["KannaDiagram"]),
        .target(name: "KannaDocs", linkerSettings: ["AppKit", "WebKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaDocsTests", dependencies: ["KannaDocs"]),
        .target(name: "KannaPerf"),
        .testTarget(name: "KannaPerfTests", dependencies: ["KannaPerf"]),
        .target(name: "KannaMarkdown", dependencies: ["KannaDocs", "KannaPerf", "KannaDiagram", "KannaOverview", "KannaCallTreeC", .product(name: "Markdown", package: "swift-markdown")],
                linkerSettings: [kannaFFI] + ["AppKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaMarkdownTests", dependencies: ["KannaMarkdown", "KannaOverview"]),
        .target(name: "KannaOverview", linkerSettings: ["AppKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaOverviewTests", dependencies: ["KannaOverview"]),
        .target(name: "KannaCode", dependencies: ["KannaSyntaxC", "KannaOverview"], linkerSettings: [kannaFFI] + ["AppKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaCodeTests", dependencies: ["KannaCode"]),
        .target(name: "KannaDiff", dependencies: ["KannaCode", "KannaOverview"], linkerSettings: ["AppKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaDiffTests", dependencies: ["KannaDiff"]),
        .target(name: "KannaLog", dependencies: ["KannaCode", "KannaOverview"], linkerSettings: ["AppKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaLogTests", dependencies: ["KannaLog"]),
        .target(name: "KannaSerial", dependencies: ["KannaOverview"], linkerSettings: ["AppKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaSerialTests", dependencies: ["KannaSerial"]),
        .target(name: "KannaLinks"),
        .target(name: "KannaDrop"),
        .target(name: "KannaStatus"),
        .target(name: "KannaMachines"),
        .target(name: "KannaUpdates"),
        .testTarget(name: "KannaUpdatesTests", dependencies: ["KannaUpdates"]),
        .target(name: "KannaWindows"),
        .testTarget(name: "KannaWindowsTests", dependencies: ["KannaWindows"]),
        .testTarget(name: "KannaMachinesTests", dependencies: ["KannaMachines"]),
        .testTarget(name: "KannaStatusTests", dependencies: ["KannaStatus"]),
        .testTarget(name: "KannaDropTests", dependencies: ["KannaDrop"]),
        .testTarget(name: "KannaLinksTests", dependencies: ["KannaLinks"]),
        .target(name: "SimBridge", linkerSettings: ["AppKit", "IOSurface", "QuartzCore"].map { .linkedFramework($0) }),
        .executableTarget(
            name: "Kanna",
            dependencies: ["GhosttyKit", "KannaLayoutC", "KannaMachinesC", "KannaCrashC", "KannaDiagram", "KannaDocs", "KannaMarkdown", "KannaCode", "KannaDiff", "KannaLog", "KannaSerial", "KannaOverview", "KannaLinks", "KannaDrop", "KannaStatus", "KannaMachines", "KannaUpdates", "KannaPerf", "KannaWindows", "SimBridge", .product(name: "Sparkle", package: "Sparkle")],
            linkerSettings: [.linkedLibrary("c++"), kannaFFI, .unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"])]
                + ["AppKit", "Carbon", "CoreText", "IOSurface", "Metal", "QuartzCore", "Security", "SystemConfiguration", "UniformTypeIdentifiers", "WebKit"].map { .linkedFramework($0) }
        ),
    ]
)
