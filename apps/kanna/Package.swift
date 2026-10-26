// swift-tools-version: 6.0
import PackageDescription

let kannaFFI = LinkerSetting.unsafeFlags(["-L", Context.environment["KANNA_RUST_RELEASE_DIR"] ?? Context.packageDirectory + "/../../target/release"])

let package = Package(
    name: "kanna",
    platforms: [.macOS(.v15)],
    products: [.executable(name: "Kanna", targets: ["Kanna"])],
    dependencies: [.package(url: "https://github.com/swiftlang/swift-markdown.git", exact: "0.7.3")],
    targets: [
        .binaryTarget(name: "GhosttyKit", path: "../../target/ghosttykit/GhosttyKit.xcframework"),
        .systemLibrary(name: "KannaLayoutC", path: "Sources/KannaLayoutC"),
        .systemLibrary(name: "KannaMerman", path: "Sources/KannaMerman"),
        .systemLibrary(name: "KannaSyntaxC", path: "Sources/KannaSyntaxC"),
        .systemLibrary(name: "KannaMachinesC", path: "Sources/KannaMachinesC"),
        .target(name: "KannaDiagram", dependencies: ["KannaMerman"], linkerSettings: [kannaFFI] + ["AppKit", "CoreText"].map { .linkedFramework($0) }),
        .target(name: "KannaDocs", linkerSettings: ["AppKit", "WebKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaDocsTests", dependencies: ["KannaDocs"]),
        .target(name: "KannaPerf"),
        .testTarget(name: "KannaPerfTests", dependencies: ["KannaPerf"]),
        .target(name: "KannaMarkdown", dependencies: ["KannaDocs", "KannaPerf", "KannaDiagram", .product(name: "Markdown", package: "swift-markdown")],
                linkerSettings: ["AppKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaMarkdownTests", dependencies: ["KannaMarkdown"]),
        .target(name: "KannaCode", dependencies: ["KannaSyntaxC"], linkerSettings: [kannaFFI] + ["AppKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaCodeTests", dependencies: ["KannaCode"]),
        .target(name: "KannaDiff", dependencies: ["KannaCode"], linkerSettings: ["AppKit"].map { .linkedFramework($0) }),
        .testTarget(name: "KannaDiffTests", dependencies: ["KannaDiff"]),
        .target(name: "KannaLinks"),
        .target(name: "KannaDrop"),
        .target(name: "KannaStatus"),
        .testTarget(name: "KannaStatusTests", dependencies: ["KannaStatus"]),
        .testTarget(name: "KannaDropTests", dependencies: ["KannaDrop"]),
        .testTarget(name: "KannaLinksTests", dependencies: ["KannaLinks"]),
        .target(name: "SimBridge", linkerSettings: ["AppKit", "IOSurface", "QuartzCore"].map { .linkedFramework($0) }),
        .executableTarget(
            name: "Kanna",
            dependencies: ["GhosttyKit", "KannaLayoutC", "KannaMachinesC", "KannaDiagram", "KannaDocs", "KannaMarkdown", "KannaCode", "KannaDiff", "KannaLinks", "KannaDrop", "KannaStatus", "KannaPerf", "SimBridge"],
            linkerSettings: [.linkedLibrary("c++"), kannaFFI]
                + ["AppKit", "Carbon", "CoreText", "IOSurface", "Metal", "QuartzCore", "Security", "SystemConfiguration", "UniformTypeIdentifiers", "WebKit"].map { .linkedFramework($0) }
        ),
    ]
)
