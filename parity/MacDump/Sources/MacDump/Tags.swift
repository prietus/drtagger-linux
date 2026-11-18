import CryptoKit
import Foundation
import LibraryKit
import TagKit

func sha256(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func sha256(file: URL) -> String { (try? Data(contentsOf: file.onDisk)).map(sha256) ?? "unreadable" }

func sampleTags() -> TagSet {
    var t = TagSet()
    t.set(TagField.title, "So What"); t.set(TagField.artist, "Miles Davis"); t.set(TagField.album, "Kind of Blue")
    t.set(TagField.albumArtist, "Miles Davis"); t.set(TagField.date, "1997-03-25"); t.set(TagField.originalDate, "1959-08-17")
    t.set(TagField.trackNumber, "1"); t.set(TagField.trackTotal, "2"); t.set(TagField.discNumber, "1"); t.set(TagField.discTotal, "1")
    t[TagField.label] = ["Columbia", "Legacy"]; t.set(TagField.catalogNumber, "CK 64935"); t.set(TagField.barcode, "074646493526")
    t.set(TagField.mbAlbumID, "rel-1"); t.set(TagField.mbTrackID, "rec-1"); t.set(TagField.mbReleaseTrackID, "trk-1")
    t[TagField.performer] = ["Miles Davis (trumpet)", "Paul Chambers (bass)"]; t.set(TagField.producer, "Teo Macero")
    t.set(TagField.isrc, "USSM15900001"); t.set("REPLAYGAIN_TRACK_GAIN", "-6.10 dB"); t.set(TagField.comment, "Ünïcödé — ok")
    t[TagField.genre] = ["Jazz", "Modal"]; t.set(TagField.compilation, "1"); t.set("ORIGINALYEAR", "1959")
    t.set(TagField.conductor, "Nobody"); t.set(TagField.artistSort, "Davis, Miles"); t.set("MOOD", "Cool")
    return t
}

func pictureJSON(_ p: Picture) -> [String: Any] {
    ["kind": p.kind.rawValue, "mimeType": p.mimeType, "width": p.width, "height": p.height,
     "byteCount": p.data.count, "sha": sha256(p.data), "description": p.description]
}

func contentsJSON(_ c: ContainerContents) -> [String: Any] {
    ["tags": c.tags.fields, "pictures": c.pictures.map(pictureJSON), "rawSHA": sha256(c.rawMetadata),
     "rawLength": c.rawMetadata.count, "audioDigest": c.audioDigest,
     "preserved": c.preserved.map { ["id": $0.id, "sha": sha256($0.payload)] }, "opaqueSHA": sha256(c.opaque)]
}

func audioFiles(_ url: URL, into out: inout [URL]) {
    guard url.existsOnDisk else { return }
    if !url.isDirectoryOnDisk {
        if TagContainer.from(url: url) != nil { out.append(url) }
        return
    }
    let entries = (try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: nil)) ?? []
    for e in entries where !e.lastPathComponent.hasPrefix(".") { audioFiles(e, into: &out) }
}

func runTags(_ arguments: [String]) {
    var roots: [URL] = []
    var coverPath: String?
    var preview = false
    var args = arguments[...]
    while let a = args.popFirst() {
        if a == "--cover" { coverPath = args.popFirst() }
        else if a == "--preview" { preview = true }
        else { roots.append(URL(fileURLWithPath: a)) }
    }
    let coverData = try! Data(contentsOf: URL(fileURLWithPath: coverPath!))
    let dims = ArtworkProcessor.dimensions(of: coverData) ?? (0, 0)
    let cover = Picture(kind: .front, mimeType: ArtworkProcessor.mimeType(of: coverData) ?? "image/jpeg", data: coverData, width: dims.0, height: dims.1)

    var files: [URL] = []
    for r in roots { audioFiles(r, into: &files) }
    files.sort { $0.path < $1.path }
    var seenFolders = Set<String>()
    let scratch = FileManager.default.temporaryDirectory.appending(path: "macdump-tags-\(UUID().uuidString)")
    try! FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: scratch) }

    var out: [[String: Any]] = []
    for url in files {
        var entry: [String: Any] = ["path": url.path, "container": TagContainer.from(url: url)!.rawValue]
        // --preview: tags and pictures only (no audio hashing, no writes), for whole libraries.
        do { entry["read"] = contentsJSON(preview ? try TaggedFile.readForPreview(url) : try TaggedFile.read(url)) }
        catch { entry["readError"] = error.localizedDescription }
        // One write cycle per folder: write, keep pictures, strip them, restore.
        if !preview, seenFolders.insert(url.deletingLastPathComponent().path).inserted {
            let copy = scratch.appending(path: url.lastPathComponent)
            try? FileManager.default.removeItem(at: copy)
            do {
                try FileManager.default.copyItem(at: url.onDisk, to: copy)
                let original = sha256(file: copy)
                let backup = try TaggedFile.backup(copy)
                var cycle: [String: Any] = [:]
                try TaggedFile.write(copy, tags: sampleTags(), pictures: [cover])
                cycle["write"] = sha256(file: copy)
                cycle["afterWrite"] = contentsJSON(try TaggedFile.read(copy))
                var edited = sampleTags(); edited.set(TagField.title, "So What (edit)")
                try TaggedFile.write(copy, tags: edited)
                cycle["keep"] = sha256(file: copy)
                try TaggedFile.write(copy, tags: edited, pictures: [])
                cycle["strip"] = sha256(file: copy)
                try TaggedFile.restore(copy, from: backup)
                cycle["restoredOriginal"] = sha256(file: copy) == original
                entry["cycle"] = cycle
            } catch {
                entry["cycleError"] = error.localizedDescription
            }
            try? FileManager.default.removeItem(at: copy)
        }
        out.append(entry)
    }
    let data = try! JSONSerialization.data(withJSONObject: ["files": out], options: [.sortedKeys, .withoutEscapingSlashes])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}
