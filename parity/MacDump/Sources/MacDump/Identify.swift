import ChromaprintKit
import CryptoKit
import FLACKit
import Foundation
import IdentifyKit
import LibraryKit
import ProviderKit
import SplitKit
import TagKit

// MacDump identify --ffmpeg <dir> --record <dir> --out <json> <root>…
// Identifies every album under the roots the way the app does (TOC and
// CUETools DB first for CUE discs). Every HTTP exchange is kept in
// <record dir>, for crates/drtagger-cli/src/parity_identify.rs to replay;
// requests already there are answered from it, so a rerun is offline.
// Keys come from ACOUSTID_KEY, DISCOGS_TOKEN and FANART_KEY. Artwork is not
// scanned (vision is a later phase).

// MARK: - Recording

// The request identity, as providerkit::http::canonical: method, URL and
// body with the API keys blanked.
func parityCanonical(method: String, url: String, body: String) -> String {
    func redact(_ s: String, _ name: String) -> String {
        s.replacingOccurrences(of: "(^|[?&])\(name)=[^&]*", with: "$1\(name)=<KEY>", options: .regularExpression)
    }
    return "\(method) \(redact(url, "api_key"))\n\(redact(body, "client"))"
}

final class RecordingProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var directory: URL!
    static let session = URLSession(configuration: .ephemeral)
    static let mark = "drtagger-parity-recorded"
    private var dataTask: URLSessionDataTask?

    override class func canInit(with request: URLRequest) -> Bool {
        URLProtocol.property(forKey: mark, in: request) == nil && ["http", "https"].contains(request.url?.scheme ?? "")
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    static func body(of request: URLRequest) -> Data {
        if let b = request.httpBody { return b }
        guard let stream = request.httpBodyStream else { return Data() }
        stream.open(); defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 65536)
        while stream.hasBytesAvailable {
            let n = stream.read(&buffer, maxLength: buffer.count)
            if n <= 0 { break }
            data.append(buffer, count: n)
        }
        return data
    }

    override func startLoading() {
        let body = Self.body(of: request)
        let canonKey = parityCanonical(method: request.httpMethod ?? "GET", url: request.url!.absoluteString, body: String(decoding: body, as: UTF8.self))
        let file = Self.directory.appendingPathComponent(SHA256.hash(data: Data(canonKey.utf8)).map { String(format: "%02x", $0) }.joined() + ".json")
        // Server errors are not answers: the client's retry goes to the network.
        if let data = try? Data(contentsOf: file), let rec = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           (rec["status"] as? Int ?? 200) < 500 {
            let payload = (rec["response"] as? String).map { Data($0.utf8) } ?? (rec["responseBase64"] as? String).flatMap { Data(base64Encoded: $0) } ?? Data()
            let response = HTTPURLResponse(url: request.url!, statusCode: rec["status"] as? Int ?? 200, httpVersion: "HTTP/1.1", headerFields: [:])!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: payload)
            client?.urlProtocolDidFinishLoading(self)
            return
        }
        guard let mutable = (request as NSURLRequest).mutableCopy() as? NSMutableURLRequest else { return }
        mutable.httpBody = body.isEmpty ? nil : body
        URLProtocol.setProperty(true, forKey: Self.mark, in: mutable)
        let sent = mutable as URLRequest
        dataTask = Self.session.dataTask(with: sent) { [weak self] data, response, error in
            guard let self else { return }
            if let error {
                self.client?.urlProtocol(self, didFailWithError: error)
                return
            }
            let data = data ?? Data()
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            let canon = parityCanonical(method: sent.httpMethod ?? "GET", url: sent.url!.absoluteString, body: String(decoding: body, as: UTF8.self))
            let key = SHA256.hash(data: Data(canon.utf8)).map { String(format: "%02x", $0) }.joined()
            let parts = canon.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
            let head = String(parts[0])
            var record: [String: Any] = [
                "method": sent.httpMethod ?? "GET",
                "url": String(head.drop { $0 != " " }.dropFirst()),
                "body": parts.count > 1 ? String(parts[1]) : "",
                "status": status,
            ]
            if let text = String(data: data, encoding: .utf8) { record["response"] = text } else { record["responseBase64"] = data.base64EncodedString() }
            let json = try! JSONSerialization.data(withJSONObject: record, options: [.sortedKeys, .prettyPrinted, .withoutEscapingSlashes])
            try? json.write(to: Self.directory.appendingPathComponent("\(key).json"))
            if let response { self.client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed) }
            self.client?.urlProtocol(self, didLoad: data)
            self.client?.urlProtocolDidFinishLoading(self)
        }
        dataTask?.resume()
    }

    override func stopLoading() { dataTask?.cancel() }
}

// MARK: - Dump

let parityUA = "drtagger-parity/0.1 (+https://drtagger.priet.us)"

func jsonObject<T: Encodable>(_ value: T) -> Any {
    let e = JSONEncoder()
    e.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
    return try! JSONSerialization.jsonObject(with: e.encode(value), options: [.fragmentsAllowed])
}

func discTOC(_ album: DetectedAlbum, tool: FFmpegTool) async -> DiscTOC? {
    guard album.discs.count == 1, let disc = album.discs.first, let cue = disc.cue else { return nil }
    var frames: [Int] = []
    for file in cue.files {
        let url = file.url(relativeTo: disc.folder)
        guard let info = try? await tool.probe(url) else { return nil }
        let samples: Int64
        if url.pathExtension.lowercased() == "flac", let flac = try? FLACFile(url: url) {
            samples = Int64(flac.streamInfo.totalSamples)
        } else {
            guard let format = try? info.pcmFormat, let (_, n) = try? await tool.rawChecksum(url, format: format) else { return nil }
            samples = n
        }
        let spf = info.sampleRate / CueTime.framesPerSecond
        guard spf > 0, samples % Int64(spf) == 0 else { return nil }
        frames.append(Int(samples / Int64(spf)))
    }
    return try? DiscTOC(cue: cue, fileFrames: frames)
}

func dumpResult(_ r: IdentificationResult) -> [String: Any] {
    var signals = jsonObject(r.signals) as! [String: Any]
    signals["tracks"] = r.signals.tracks.map { t -> [String: Any] in
        var d = jsonObject(t) as! [String: Any]
        d["url"] = t.url?.lastPathComponent
        return d
    }
    signals.removeValue(forKey: "artworkScans")
    var out: [String: Any] = [
        "signals": signals,
        "candidates": r.candidates.map { jsonObject($0) },
        "discogsCandidates": r.discogsCandidates.map { jsonObject($0) },
        "log": r.log,
        "isConfident": r.isConfident,
        "isClearFingerprintMatch": r.isClearFingerprintMatch,
        "isClearBarcodeMatch": r.isClearBarcodeMatch,
        "coverContenders": r.coverContenders.map(\.id),
    ]
    if let f = r.fingerprints { out["fingerprints"] = jsonObject(f) }
    if let x = r.runnerUp { out["runnerUp"] = x.id }
    if let x = r.albumOnlyPick { out["albumOnlyPick"] = x.id }
    return out
}

func runIdentify(_ arguments: [String]) {
    var roots: [URL] = []
    var ffmpegDir = ""
    var recordDir = ""
    var outFile = ""
    var args = arguments[...]
    while let a = args.popFirst() {
        switch a {
        case "--ffmpeg": ffmpegDir = args.popFirst() ?? ""
        case "--record": recordDir = args.popFirst() ?? ""
        case "--out": outFile = args.popFirst() ?? ""
        default: roots.append(URL(fileURLWithPath: a))
        }
    }
    let env = ProcessInfo.processInfo.environment
    RecordingProtocol.directory = URL(fileURLWithPath: recordDir)
    try? FileManager.default.createDirectory(at: RecordingProtocol.directory, withIntermediateDirectories: true)
    URLProtocol.registerClass(RecordingProtocol.self)

    let tool = FFmpegTool(ffmpeg: URL(fileURLWithPath: ffmpegDir + "/ffmpeg"), ffprobe: URL(fileURLWithPath: ffmpegDir + "/ffprobe"))
    let albums = LibraryScanner().scan(roots).albums.sorted { $0.url.lastPathComponent < $1.url.lastPathComponent }
    final class Box: @unchecked Sendable { var out: [[String: Any]] = [] }
    let box = Box()
    let done = DispatchSemaphore(value: 0)
    let acoustIDKey = env["ACOUSTID_KEY"], discogsToken = env["DISCOGS_TOKEN"], fanartKey = env["FANART_KEY"]
    Task { @Sendable in
        for album in albums {
            FileHandle.standardError.write(Data("identify \(album.url.lastPathComponent)\n".utf8))
            var entry: [String: Any] = ["folder": album.url.lastPathComponent]
            let toc = await discTOC(album, tool: tool)
            var ctdb: [CUEToolsDBClient.Metadata] = []
            if let toc {
                entry["toc"] = ["discID": toc.musicBrainzDiscID, "ctdb": toc.ctdbTOCString, "freedb": toc.freeDBDiscID]
                do {
                    let r = try await CUEToolsDBClient(userAgent: parityUA).lookup(toc: toc.ctdbTOCString)
                    ctdb = r.metadata
                    entry["ctdb"] = jsonObject(r)
                } catch {
                    entry["ctdbError"] = error.localizedDescription
                }
            }
            var config = IdentifierConfig(userAgent: parityUA, acoustIDKey: acoustIDKey, discogsToken: discogsToken)
            config.scanArtwork = false
            let result = await Identifier(config: config, tool: tool).identify(album: album, toc: toc, ctdb: ctdb)
            entry["result"] = dumpResult(result)

            // Raw fingerprints, the way FingerprintService computes them.
            var raws: [Any] = []
            for t in result.signals.tracks {
                guard let url = t.url, url.pathExtension.lowercased() != "iso",
                      let snippet = try? await tool.decodeSnippet(url, start: t.imageStart ?? 0, seconds: 120), !snippet.samples.isEmpty else {
                    raws.append(NSNull()); continue
                }
                let duration = Int((t.durationSeconds ?? Double(snippet.samples.count / snippet.channels) / Double(snippet.sampleRate)).rounded())
                let fp = try? await Fingerprinter().fingerprint(samples: snippet.samples, sampleRate: snippet.sampleRate, numChannels: snippet.channels, durationSeconds: max(1, duration))
                raws.append(fp.map { ["sampleRate": snippet.sampleRate, "channels": snippet.channels, "duration": $0.durationSeconds, "raw": $0.rawFingerprint] as [String: Any] } ?? NSNull())
            }
            entry["rawFingerprints"] = raws

            if let best = result.best {
                var ctx = TagContext()
                ctx.discogs = result.discogsCandidates.first
                ctx.musicBrainzDiscID = toc?.musicBrainzDiscID
                ctx.freedbDiscID = toc?.freeDBDiscID
                ctx.acoustIDs = result.fingerprints?.acoustIDs ?? [:]
                let media = MatchScorer.relevantMedia(best.candidate, localFormat: result.signals.localFormat)
                // By disc position, as TagService maps the files.
                var seen: [Int: Int] = [:]
                entry["tags"] = result.signals.tracks.map { t -> Any in
                    let track = seen[t.discNumber, default: 0]
                    seen[t.discNumber] = track + 1
                    return PicardMapper.trackTags(best.candidate, media: media, disc: t.discNumber, track: track, context: ctx)
                        .map { $0.pairs.map { [$0.name, $0.value] } } ?? NSNull()
                }
                let fetcher = CoverArtFetcher(userAgent: parityUA, fanartKey: fanartKey)
                let options = await fetcher.options(for: best.candidate, discogs: result.discogsCandidates.first, album: album)
                entry["artwork"] = options.map { ["source": $0.source.rawValue, "url": $0.isLocal ? $0.url.lastPathComponent : $0.url.absoluteString, "label": $0.label] }
            }
            box.out.append(entry)
        }
        done.signal()
    }
    done.wait()
    // The Mac AcoustID client prints debug lines on stdout: the dump goes to a file.
    let data = try! JSONSerialization.data(withJSONObject: ["albums": box.out], options: [.sortedKeys, .withoutEscapingSlashes])
    try! data.write(to: URL(fileURLWithPath: outFile))
}
