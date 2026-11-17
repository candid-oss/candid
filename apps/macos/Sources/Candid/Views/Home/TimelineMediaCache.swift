import AppKit
import DesktopCore
import ImageIO
import Observation

/// Bounded, on-demand thumbnails for the home timeline. Images are decoded
/// downsampled off the main actor; only what is on screen is ever requested.
@MainActor @Observable
final class TimelineMediaCache {
    private struct Decoded: @unchecked Sendable { let image: CGImage }
    /// Decoded pixels kept in memory (~64 MB RGBA). Reads refresh recency, so
    /// what is on screen is evicted last and never re-requested in a loop.
    static let pixelBudget = 16_000_000
    /// Sizes images are decoded at; a lookup falls back to another size of the
    /// same screenshot while the requested one loads, so nothing blinks.
    static let sizes = [1600, 1100, 760, 600]

    private(set) var images: [String: NSImage] = [:]
    private(set) var screenshots: [String: [DesktopRecord]] = [:]
    /// Oldest first; lists beyond 160 are dropped one at a time, never all at once (which made every frame blink).
    @ObservationIgnored private var screenshotOrder: [String] = []
    /// The last image shown per activity, kept while a growing live activity refetches its list.
    @ObservationIgnored private var lastBySpan: [String: NSImage] = [:]
    @ObservationIgnored private var order: [String] = []
    @ObservationIgnored private var pixels: [String: Int] = [:]
    @ObservationIgnored var pixelBudget = TimelineMediaCache.pixelBudget
    /// Decodes performed; a steady screen must not keep raising it.
    @ObservationIgnored private(set) var decodeCount = 0
    @ObservationIgnored private var inflight: Set<String> = []
    @ObservationIgnored private let session: DesktopSession
    @ObservationIgnored private let media: MediaWorkspaceStore?
    @ObservationIgnored private let previewDirectory = FileManager.default.temporaryDirectory
        .appendingPathComponent("candid-timeline-preview-" + UUID().uuidString, isDirectory: true)

    init(session: DesktopSession, media: MediaWorkspaceStore? = nil) { self.session = session; self.media = media }

    /// Library images (region screenshots, imports) by record ID.
    func mediaImage(id: String) -> NSImage? {
        if let image = images["media:" + id] { return image }
        guard let media, let record = media.records.first(where: { $0.id.uuidString == id }), record.kind == .image,
              inflight.insert("m:" + id).inserted else { return nil }
        let directory = previewDirectory
        Task {
            defer { inflight.remove("m:" + id) }
            guard let url = try? await media.preview(record, directory: directory) else { return }
            await decode(url, maxPixel: 1000, key: "media:" + id)
        }
        return nil
    }

    /// Key includes the sample count so a live activity refetches when it grows.
    static func key(_ span: TimelineSpan) -> String { span.id + "#" + String(span.sampleCount) }

    func screenshots(for span: TimelineSpan) -> [DesktopRecord]? { screenshots[Self.key(span)] }

    func requestScreenshots(for span: TimelineSpan) {
        guard span.hasScreenshot else { return }
        let key = Self.key(span)
        guard screenshots[key] == nil, inflight.insert("s:" + key).inserted else { return }
        Task {
            let records = await session.screenshotRecords(forEventID: span.id)
            screenshots[key] = records.filter { $0.imageHash != nil }
            inflight.remove("s:" + key)
            screenshotOrder.removeAll { $0 == key }; screenshotOrder.append(key)
            while screenshotOrder.count > 160 { screenshots[screenshotOrder.removeFirst()] = nil }
        }
    }

    static func key(_ id: String, maxPixel: Int) -> String { id + "@" + String(maxPixel) }

    /// The screenshot at `maxPixel` if decoded, else the closest other size.
    func image(for record: DesktopRecord, maxPixel: Int = 1100) -> NSImage? {
        let exact = Self.key(record.id, maxPixel: maxPixel)
        if let image = images[exact] { touch(exact); return image }
        let others = Self.sizes.filter { $0 != maxPixel }.sorted { abs($0 - maxPixel) < abs($1 - maxPixel) }
        for size in others {
            let key = Self.key(record.id, maxPixel: size)
            if let image = images[key] { touch(key); return image }
        }
        return nil
    }

    func hasImage(for record: DesktopRecord, maxPixel: Int) -> Bool { images[Self.key(record.id, maxPixel: maxPixel)] != nil }

    func requestImage(for record: DesktopRecord, maxPixel: Int = 1100) {
        let key = Self.key(record.id, maxPixel: maxPixel)
        guard let hash = record.imageHash, images[key] == nil, inflight.insert("i:" + key).inserted else { return }
        Task {
            defer { inflight.remove("i:" + key) }
            guard let url = await session.verifiedImageURL(hash: hash) else { return }
            await decode(url, maxPixel: maxPixel, key: key)
        }
    }

    private func touch(_ key: String) {
        guard order.last != key, let index = order.firstIndex(of: key) else { return }
        order.remove(at: index); order.append(key)
    }

    private func decode(_ url: URL, maxPixel: Int, key: String) async {
        let decoded = await Task.detached(priority: .userInitiated) { () -> Decoded? in
            guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
            let options: [CFString: Any] = [kCGImageSourceCreateThumbnailFromImageAlways: true,
                                            kCGImageSourceCreateThumbnailWithTransform: true,
                                            kCGImageSourceThumbnailMaxPixelSize: maxPixel]
            return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map(Decoded.init)
        }.value
        guard let decoded else { return }
        decodeCount += 1; TimelinePerf.tick(.decode)
        images[key] = NSImage(cgImage: decoded.image, size: NSSize(width: decoded.image.width, height: decoded.image.height))
        pixels[key] = decoded.image.width * decoded.image.height
        order.removeAll { $0 == key }; order.append(key)
        var total = pixels.values.reduce(0, +)
        while total > pixelBudget, order.count > 1 {
            let evicted = order.removeFirst()
            total -= pixels.removeValue(forKey: evicted) ?? 0
            images[evicted] = nil
        }
    }

    /// Latest screenshot of a span, requesting what is missing along the way.
    func latestImage(for span: TimelineSpan) -> NSImage? {
        guard span.hasScreenshot else { return nil }
        guard let list = screenshots(for: span) else { requestScreenshots(for: span); return lastBySpan[span.id] }
        guard let record = list.last else { return lastBySpan[span.id] }
        if let image = image(for: record, maxPixel: 760) {
            if !hasImage(for: record, maxPixel: 760) { requestImage(for: record, maxPixel: 760) }
            if lastBySpan[span.id] !== image {
                lastBySpan[span.id] = image
                if lastBySpan.count > 400, let other = lastBySpan.keys.first(where: { $0 != span.id }) { lastBySpan[other] = nil }
            }
            return image
        }
        requestImage(for: record, maxPixel: 760)
        return lastBySpan[span.id]
    }

    func reset() { images.removeAll(); screenshots.removeAll(); screenshotOrder.removeAll(); lastBySpan.removeAll(); order.removeAll(); pixels.removeAll() }
}
