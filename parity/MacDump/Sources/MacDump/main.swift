// MacDump <root>… [--names <file>]        scan, CUE and name parsing
// MacDump tags --cover <jpg> <root>…        tag read / write / restore
// MacDump identify --ffmpeg <dir> --record <dir> <root>…   identification
// MacDump vision-scan / vision-pairs                       Vision benchmark (Vision.swift)
// Keep the JSON shape in step with crates/drtagger-cli/src/parity.rs.
import Foundation
import LibraryKit
import SplitKit

if CommandLine.arguments.dropFirst().first == "tags" {
    runTags(Array(CommandLine.arguments.dropFirst(2)))
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "loudness" {
    runLoudness(Array(CommandLine.arguments.dropFirst(2)))
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "sacd" {
    runSACD(Array(CommandLine.arguments.dropFirst(2)))
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "identify" {
    runIdentify(Array(CommandLine.arguments.dropFirst(2)))
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "vision-scan" {
    runVisionScan(Array(CommandLine.arguments.dropFirst(2)))
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "vision-pairs" {
    runVisionPairs(Array(CommandLine.arguments.dropFirst(2)))
    exit(0)
}
if CommandLine.arguments.dropFirst().first == "split" {
    runSplit(Array(CommandLine.arguments.dropFirst(2)))
    exit(0)
}

var roots: [URL] = []
var namesFile: String?
var args = CommandLine.arguments.dropFirst()
while let a = args.popFirst() {
    if a == "--names" { namesFile = args.popFirst() } else { roots.append(URL(fileURLWithPath: a)) }
}

let encoder = JSONEncoder()
encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]

func encoded<T: Encodable>(_ value: T) -> Any {
    try! JSONSerialization.jsonObject(with: encoder.encode(value), options: [.fragmentsAllowed])
}

func position(_ p: (number: Int, total: Int?)) -> [String: Any] {
    var d: [String: Any] = ["number": p.number]
    if let t = p.total { d["total"] = t }
    return d
}

func derived(_ a: DetectedAlbum) -> [String: Any] {
    var d: [String: Any] = [
        "id": a.id,
        "trackCount": a.trackCount,
        "folderName": a.folderName,
        "setBaseName": a.setBaseName,
        "formats": a.formats.map(\.rawValue).sorted(),
    ]
    if let t = a.titleHint { d["titleHint"] = t }
    if let t = a.artistHint { d["artistHint"] = t }
    if let t = a.yearHint { d["yearHint"] = t }
    if let p = a.discPosition { d["discPosition"] = position(p) }
    return d
}

// Path strings decompose accents (see LibraryKit's DiskPaths): existence is
// checked on the exact bytes, as the scanner does.
func findCues(_ url: URL, into out: inout [URL]) {
    guard url.existsOnDisk else { return }
    if !url.isDirectoryOnDisk {
        if FileRules.isCue(url) { out.append(url) }
        return
    }
    let entries = (try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: [.isDirectoryKey, .isRegularFileKey])) ?? []
    for e in entries where !FileRules.isJunk(name: e.lastPathComponent) {
        let v = try? e.resourceValues(forKeys: [.isDirectoryKey, .isRegularFileKey])
        if v?.isDirectory == true { findCues(e, into: &out) }
        else if v?.isRegularFile == true, FileRules.isCue(e) { out.append(e) }
    }
}

func cue(_ url: URL) -> [String: Any] {
    let sheet: CueSheet
    do { sheet = try CueSheet.parse(url: url) } catch {
        return ["path": url.path, "error": error.localizedDescription]
    }
    let maxIndex = sheet.tracks.flatMap { $0.indexes.map(\.time.frames) }.max() ?? 0
    let frames = Array(repeating: maxIndex + 10_000, count: sheet.files.count)
    var toc: [String: Any]
    do {
        let t = try DiscTOC(cue: sheet, fileFrames: frames)
        toc = ["firstTrack": t.firstTrack, "lastTrack": t.lastTrack, "trackOffsets": t.trackOffsets,
               "leadOut": t.leadOut, "musicBrainzDiscID": t.musicBrainzDiscID, "freeDBDiscID": t.freeDBDiscID,
               "ctdbTOCString": t.ctdbTOCString, "musicBrainzTOCString": t.musicBrainzTOCString]
    } catch { toc = ["error": error.localizedDescription] }
    var plan: Any
    do {
        plan = encoded(try CueSplitPlan.make(cue: sheet, fileSampleCounts: frames.map { Int64($0 * 588) }, sampleRate: 44100))
    } catch { plan = ["error": error.localizedDescription] }
    var d: [String: Any] = ["path": url.path, "sheet": encoded(sheet), "catalogNumberHints": sheet.catalogNumberHints,
                            "toc": toc, "plan": plan]
    if let b = sheet.barcodeHint { d["barcodeHint"] = b }
    if let p = sheet.trackOnePregap { d["trackOnePregap"] = ["frames": p.frames] }
    return d
}

func parsed(_ p: FolderNameParser.Parsed) -> [String: Any] {
    var d: [String: Any] = ["notes": p.notes]
    if let v = p.artist { d["artist"] = v }
    if let v = p.title { d["title"] = v }
    if let v = p.year { d["year"] = v }
    if let v = p.editionYear { d["editionYear"] = v }
    if let v = p.edition { d["edition"] = v }
    if let v = p.source { d["source"] = v.rawValue }
    if let v = p.country { d["country"] = v }
    if let v = p.displayTitle { d["displayTitle"] = v }
    return d
}

func name(_ n: String) -> [String: Any] {
    var d: [String: Any] = [
        "name": n,
        "parse": parsed(FolderNameParser.parse(n)),
        "albumTitle": parsed(FolderNameParser.parseAlbumTitle(n)),
        "stripped": FileRules.strippingDiscToken(n),
        "ascii": PathTemplate.asciiSafe(n),
        "sanitized": PathTemplate.sanitizeComponent(n),
        "rendered": PathTemplate.render("{albumartist}/{album} ({year})/{track} - {title}",
                                        values: ["albumartist": n, "album": n, "year": "", "track": "07", "title": n], ascii: true),
    ]
    if let p = FileRules.discNumber(fromFileName: n) { d["discNumber"] = position(p) }
    if let f = FileRules.discNumber(fromFolderName: n) { d["folderDisc"] = f }
    return d
}

let result = LibraryScanner().scan(roots)
var cueURLs: [URL] = []
for r in roots { findCues(r, into: &cueURLs) }
let names = namesFile.flatMap { try? String(contentsOfFile: $0, encoding: .utf8) }?
    .split(separator: "\n").map(String.init).filter { !$0.isEmpty } ?? []

let out: [String: Any] = [
    "scan": [
        "albums": result.albums.map { ["album": encoded($0), "derived": derived($0)] },
        "issues": encoded(result.issues),
        "foldersVisited": result.foldersVisited,
    ],
    "cues": cueURLs.map(cue),
    "names": names.map(name),
]
let data = try JSONSerialization.data(withJSONObject: out, options: [.sortedKeys, .withoutEscapingSlashes])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data("\n".utf8))
