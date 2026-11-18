import Foundation
import LibraryKit
import SACDKit

// MacDump sacd --into <scratch> <iso>…
// Reads each disc and extracts the stereo area with both pause policies and
// the multichannel area with the default one; every DSF is hashed, then
// deleted. Keep in step with crates/drtagger-cli/src/parity_sacd.rs.
func runSACD(_ arguments: [String]) {
    var isos: [URL] = []
    var into = ""
    var args = arguments[...]
    while let a = args.popFirst() {
        if a == "--into" { into = args.popFirst() ?? "" } else { isos.append(URL(fileURLWithPath: a)) }
    }
    let root = URL(fileURLWithPath: into, isDirectory: true)
    var out: [[String: Any]] = []
    for iso in isos {
        var entry: [String: Any] = ["iso": iso.lastPathComponent]
        do {
            let disc = try SACDDiscReader.read(url: iso)
            entry["areas"] = disc.areas.map { splitEncoded($0) }
            var runs: [[String: Any]] = []
            for area in disc.areas {
                let policies: [SACDExtractOptions.PausePolicy] = area.isMultichannel ? [.appendToPrevious] : [.appendToPrevious, .drop]
                for policy in policies {
                    var options = SACDExtractOptions()
                    options.pausePolicy = policy
                    options.overwriteExisting = true
                    var run: [String: Any] = ["multichannel": area.isMultichannel, "policy": policy.rawValue]
                    do {
                        let o = try SACDExtractor().extract(disc: disc, area: area, destinationRoot: root, options: options)
                        let rootPath = root.standardizedFileURL.path
                        run["outputFolder"] = String(o.outputFolder.standardizedFileURL.path.dropFirst(rootPath.count + 1))
                        run["droppedLeadIn"] = o.droppedLeadInFrames
                        run["droppedPause"] = o.droppedPauseFrames
                        run["verified"] = o.verified
                        run["warnings"] = o.warnings
                        run["tracks"] = o.tracks.map { t -> [String: Any] in
                            let j: [String: Any] = ["number": t.number, "file": t.url.lastPathComponent, "frames": t.frames,
                                                    "expectedFrames": t.expectedFrames, "sampleCount": t.sampleCount,
                                                    "fileSize": t.fileSize, "contiguous": t.contiguous, "sha": sha256(file: t.url)]
                            try? FileManager.default.removeItem(at: t.url)
                            return j
                        }
                    } catch {
                        run["error"] = error.localizedDescription
                    }
                    runs.append(run)
                }
            }
            entry["runs"] = runs
        } catch {
            entry["error"] = error.localizedDescription
        }
        out.append(entry)
    }
    let data = try! JSONSerialization.data(withJSONObject: ["discs": out], options: [.sortedKeys, .withoutEscapingSlashes])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}
