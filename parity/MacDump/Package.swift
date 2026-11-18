// swift-tools-version: 6.0
import Foundation
import PackageDescription

// Dumps what the Mac app's packages compute for a set of samples, as JSON
// that parity/compare.py diffs against `drtagger-cli parity`. Builds
// against the Mac checkout in place (MACTAGGER_DIR, default ~/mactagger);
// nothing in that repository changes.
let mactagger = ProcessInfo.processInfo.environment["MACTAGGER_DIR"]
    ?? (NSHomeDirectory() as NSString).appendingPathComponent("mactagger")

let package = Package(
    name: "MacDump",
    platforms: [.macOS(.v15)],
    dependencies: [
        .package(path: mactagger + "/Packages/LibraryKit"),
        .package(path: mactagger + "/Packages/TagKit"),
        .package(path: mactagger + "/Packages/SplitKit"),
        .package(path: mactagger + "/Packages/SACDKit"),
        .package(path: mactagger + "/Packages/LoudnessKit"),
        .package(path: mactagger + "/Packages/FLACKit"),
        .package(path: mactagger + "/Packages/ProviderKit"),
        .package(path: mactagger + "/Packages/IdentifyKit"),
        .package(path: mactagger + "/Packages/Chromaprint"),
    ],
    targets: [
        .executableTarget(
            name: "MacDump",
            dependencies: [
                .product(name: "LibraryKit", package: "LibraryKit"),
                .product(name: "TagKit", package: "TagKit"),
                .product(name: "SplitKit", package: "SplitKit"),
                .product(name: "SACDKit", package: "SACDKit"),
                .product(name: "LoudnessKit", package: "LoudnessKit"),
                .product(name: "FLACKit", package: "FLACKit"),
                .product(name: "ProviderKit", package: "ProviderKit"),
                .product(name: "IdentifyKit", package: "IdentifyKit"),
                .product(name: "ChromaprintKit", package: "Chromaprint"),
            ]
        ),
    ]
)
