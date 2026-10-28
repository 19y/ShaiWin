// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "ShaiWin",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "ShaiWin", targets: ["ShaiWin"]),
        .library(name: "ShaiWinCore", targets: ["ShaiWinCore"]),
    ],
    targets: [
        // The C ABI from crates/ffi. The modulemap points at ../../include/shaiwin.h.
        .systemLibrary(name: "CShaiWin", path: "Sources/CShaiWin"),
        .target(name: "ShaiWinCore", dependencies: ["CShaiWin"], path: "Sources/ShaiWinCore"),
        .executableTarget(name: "ShaiWin", dependencies: ["ShaiWinCore"], path: "Sources/ShaiWin"),
    ]
)
