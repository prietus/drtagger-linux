import CryptoKit
import FLACKit
import Foundation
import LibraryKit
import SplitKit

// MacDump split --ffmpeg <dir> --into <scratch> <root>…
// Splits every CUE image album (the app's `isSplittable`) the way
// DiscService.split does, and dumps each outcome with the files' hashes.
func runSplit(_ arguments: [String]) {
    var roots: [URL] = []
    var ffmpegDir = ""
    var into = ""
    var args = arguments[...]
    while let a = args.popFirst() {
        if a == "--ffmpeg" { ffmpegDir = args.popFirst() ?? "" }
        else if a == "--into" { into = args.popFirst() ?? "" }
        else { roots.append(URL(fileURLWithPath: a)) }
    }
    let tool = FFmpegTool(ffmpeg: URL(fileURLWithPath: ffmpegDir + "/ffmpeg"), ffprobe: URL(fileURLWithPath: ffmpegDir + "/ffprobe"))
    let destination = URL(fileURLWithPath: into, isDirectory: true)
    let albums = LibraryScanner().scan(roots).albums.filter { $0.kind == .cueImage }
    final class Box: @unchecked Sendable { var out: [[String: Any]] = [] }
    let box = Box()
    let done = DispatchSemaphore(value: 0)
    Task { @Sendable in
        for album in albums {
            let discs = album.discs.filter { $0.cue != nil }.sorted { $0.number < $1.number }
            let multi = discs.count > 1
            for disc in discs {
                let context = ProvisionalTags.AlbumContext.from(cue: disc.cue!, discNumber: multi ? disc.number : nil, discTotal: multi ? discs.count : nil, toc: nil)
                var options = SplitOptions()
                options.overwriteExisting = true
                var entry: [String: Any] = ["album": album.id, "disc": disc.number]
                do {
                    let o = try await ImageSplitter(tool: tool).split(disc: disc, album: context, destinationRoot: destination, options: options)
                    entry["outcome"] = outcomeJSON(o, root: destination)
                } catch {
                    entry["error"] = error.localizedDescription
                }
                box.out.append(entry)
            }
        }
        done.signal()
    }
    done.wait()
    let data = try! JSONSerialization.data(withJSONObject: ["splits": box.out], options: [.sortedKeys, .withoutEscapingSlashes])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

// main.swift's globals are not initialised yet when a mode runs from the
// top of the script, so this file encodes with its own encoder.
func splitEncoded<T: Encodable>(_ value: T) -> Any {
    let e = JSONEncoder()
    e.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
    return try! JSONSerialization.jsonObject(with: e.encode(value), options: [.fragmentsAllowed])
}

func outcomeJSON(_ o: SplitOutcome, root: URL) -> [String: Any] {
    let rootPath = root.standardizedFileURL.path
    func relative(_ u: URL) -> String {
        let p = u.standardizedFileURL.path
        return p.hasPrefix(rootPath + "/") ? String(p.dropFirst(rootPath.count + 1)) : p
    }
    var d: [String: Any] = [
        "outputFolder": relative(o.outputFolder),
        "format": splitEncoded(o.format),
        "plan": splitEncoded(o.plan),
        "verified": o.verified,
        "warnings": o.warnings,
        "tracks": o.tracks.map { t -> [String: Any] in
            var tags: [[String]] = []
            if let f = try? FLACFile(url: t.url), let vc = f.vorbisComment {
                tags = vc.fields.map { [$0.name, $0.value] }
                tags.append(["(vendor)", vc.vendor])
            }
            var j: [String: Any] = [
                "number": t.number, "file": relative(t.url), "samples": t.samples, "md5Hex": t.md5Hex,
                "streamInfoMD5Matches": t.streamInfoMD5Matches, "crc32": t.crc32, "verified": t.verified,
                "sha": sha256(file: t.url), "tags": tags,
            ]
            if let title = t.title { j["title"] = title }
            if let c = t.ctdbCRC32 { j["ctdbCRC32"] = c }
            return j
        },
    ]
    if let toc = o.toc {
        d["toc"] = ["trackOffsets": toc.trackOffsets, "leadOut": toc.leadOut, "musicBrainzDiscID": toc.musicBrainzDiscID,
                    "freeDBDiscID": toc.freeDBDiscID, "ctdbTOCString": toc.ctdbTOCString]
    }
    if let c = o.ctdbDiscCRC32 { d["ctdbDiscCRC32"] = c }
    return d
}
