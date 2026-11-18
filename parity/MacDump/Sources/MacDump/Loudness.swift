import Foundation
import LoudnessKit
import SplitKit
import TagKit

// MacDump loudness --ffmpeg <dir> <album folder>…
// Each folder's audio files (sorted) measured as one album, with the
// ReplayGain tags the app would write. Keep in step with
// crates/drtagger-cli/src/parity_loudness.rs.
func runLoudness(_ arguments: [String]) {
    var folders: [URL] = []
    var ffmpegDir = ""
    var args = arguments[...]
    while let a = args.popFirst() {
        if a == "--ffmpeg" { ffmpegDir = args.popFirst() ?? "" } else { folders.append(URL(fileURLWithPath: a)) }
    }
    let albumFolders = folders
    let tool = FFmpegTool(ffmpeg: URL(fileURLWithPath: ffmpegDir + "/ffmpeg"), ffprobe: URL(fileURLWithPath: ffmpegDir + "/ffprobe"))
    final class Box: @unchecked Sendable { var out: [[String: Any]] = [] }
    let box = Box()
    let done = DispatchSemaphore(value: 0)
    Task { @Sendable in
        for folder in albumFolders {
            let files = ((try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)) ?? [])
                .filter { !$0.lastPathComponent.hasPrefix(".") && TagContainer.from(url: $0) != nil }
                .sorted { $0.lastPathComponent < $1.lastPathComponent }
            var entry: [String: Any] = ["folder": folder.lastPathComponent]
            do {
                let album = try await LoudnessAnalyzer(tool: tool).analyzeAlbum(files)
                entry["albumLUFS"] = album.integratedLUFS ?? NSNull()
                entry["albumPeak"] = album.truePeakDBTP
                entry["tracks"] = zip(files, album.tracks).map { url, t -> [String: Any] in
                    let dsd = ["dsf", "dff"].contains(url.pathExtension.lowercased())
                    return ["file": url.lastPathComponent, "lufs": t.integratedLUFS ?? NSNull(), "truePeak": t.truePeakDBTP,
                            "samplePeak": t.samplePeakDBFS, "blocks": t.blockPowers.count, "duration": t.durationSeconds,
                            "sampleRate": t.sampleRate, "channels": t.channels,
                            "tags": ReplayGain.tags(track: t, album: album, dsd: dsd).map { [$0.0, $0.1] }]
                }
            } catch {
                entry["error"] = error.localizedDescription
            }
            box.out.append(entry)
        }
        done.signal()
    }
    done.wait()
    let data = try! JSONSerialization.data(withJSONObject: ["albums": box.out], options: [.sortedKeys, .withoutEscapingSlashes])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}
