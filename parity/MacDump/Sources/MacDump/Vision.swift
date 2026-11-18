// MacDump vision-scan --root <dir> [--threads n] <files.tsv>
//     Vision's artwork scan of the images listed as "album<TAB>file"
//     (relative to --root; drtagger-cli parity-vision-scan writes the same
//     list, as IdentifyKit's prioritised artwork is internal).
// MacDump vision-pairs --root <dir> <pairs.tsv>
//     Vision feature-print distances for lines "a<TAB>b".
// Keep the JSON shape in step with crates/drtagger-cli/src/parity_vision.rs.
import Foundation
import IdentifyKit

private func rows(_ path: String) -> [(String, String)] {
    let text = (try? String(contentsOfFile: path, encoding: .utf8)) ?? ""
    return text.split(separator: "\n").compactMap { line in
        let parts = line.split(separator: "\t", maxSplits: 1).map(String.init)
        return parts.count == 2 ? (parts[0], parts[1]) : nil
    }
}

private func emit(_ object: Any) {
    let data = try! JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write("\n".data(using: .utf8)!)
}

func runVisionScan(_ arguments: [String]) {
    var args = ArraySlice(arguments)
    var root = "", list = "", threads = 3
    while let a = args.popFirst() {
        switch a {
        case "--root": root = args.popFirst()!
        case "--threads": threads = Int(args.popFirst()!)!
        default: list = a
        }
    }
    let files = rows(list)
    let rootURL = URL(fileURLWithPath: root)
    var results = [Any](repeating: NSNull(), count: files.count)
    let lock = NSLock()
    var next = 0
    let group = DispatchGroup()
    for _ in 0..<threads {
        group.enter()
        Thread.detachNewThread {
            while true {
                lock.lock(); let i = next; next += 1; lock.unlock()
                guard i < files.count else { break }
                let (album, file) = files[i]
                let url = rootURL.appending(path: file)
                let start = Date()
                let sema = DispatchSemaphore(value: 0)
                nonisolated(unsafe) var result: ArtworkScanResult?
                Task.detached {
                    result = try? await ArtworkScanner().scan(url)
                    sema.signal()
                }
                sema.wait()
                let ms = Int(Date().timeIntervalSince(start) * 1000)
                FileHandle.standardError.write("[\(i + 1)/\(files.count)] \(ms) ms \(file)\n".data(using: .utf8)!)
                let r = result
                lock.lock()
                results[i] = [
                    "album": album, "file": file,
                    "barcodes": r?.barcodes ?? [], "catalogNumbers": r?.catalogNumbers ?? [],
                    "recognizedTexts": r?.recognizedTexts ?? [], "confidence": Double(r?.confidence ?? 0), "ms": ms,
                ] as [String: Any]
                lock.unlock()
            }
            group.leave()
        }
    }
    group.wait()
    emit(["images": results])
}

func runVisionPairs(_ arguments: [String]) {
    var args = ArraySlice(arguments)
    var root = "", list = ""
    while let a = args.popFirst() {
        if a == "--root" { root = args.popFirst()! } else { list = a }
    }
    let rootURL = URL(fileURLWithPath: root)
    var cache: [String: Data] = [:]
    func data(_ rel: String) -> Data? {
        if let d = cache[rel] { return d }
        let d = try? Data(contentsOf: rootURL.appending(path: rel))
        cache[rel] = d
        return d
    }
    var out: [Any] = []
    for (a, b) in rows(list) {
        var row: [String: Any] = ["a": a, "b": b]
        if let da = data(a), let db = data(b), let d = CoverCheck.distance(da, db) {
            row["distance"] = Double(d)
        } else {
            row["distance"] = NSNull()
        }
        out.append(row)
    }
    emit(["pairs": out])
}
