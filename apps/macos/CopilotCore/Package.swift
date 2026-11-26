// swift-tools-version:5.9
// Local package: Swift module `CopilotCore` (UniFFI bindings over the Rust static library).
// Outputs of `make core` (gitignored): CopilotCore.xcframework, Sources/CopilotCore/CopilotCore.swift.
import PackageDescription

let package = Package(
    name: "CopilotCore",
    platforms: [.macOS(.v14)],
    products: [.library(name: "CopilotCore", targets: ["CopilotCore"])],
    targets: [
        // Static lib + header + modulemap (clang module `warptalk_copilot_coreFFI`).
        .binaryTarget(name: "CopilotCoreFFI", path: "CopilotCore.xcframework"),
        .target(
            name: "CopilotCore",
            dependencies: ["CopilotCoreFFI"],
            path: "Sources/CopilotCore"
        ),
    ]
)
