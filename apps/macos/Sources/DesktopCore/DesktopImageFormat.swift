import Foundation

/// The stored format of a screenshot original, read from its first bytes. New captures are WebP (data model v1
/// §9.1); JPEG remains for older originals and for captures made while the WebP encoder was unavailable.
public enum DesktopImageFormat: String, Sendable {
    case webp, jpeg, png

    public init?(_ data: Data) {
        let head = [UInt8](data.prefix(12))
        if head.count == 12, head[0..<4].elementsEqual("RIFF".utf8), head[8..<12].elementsEqual("WEBP".utf8) { self = .webp }
        else if head.count >= 3, head[0] == 0xFF, head[1] == 0xD8, head[2] == 0xFF { self = .jpeg }
        else if head.count >= 8, head[0..<8].elementsEqual([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) { self = .png }
        else { return nil }
    }

    public init?(mimeType: String) {
        switch mimeType.lowercased() {
        case "image/webp": self = .webp
        case "image/jpeg", "image/jpg": self = .jpeg
        case "image/png": self = .png
        default: return nil
        }
    }

    public var mimeType: String { "image/" + rawValue }
    public var fileExtension: String { self == .jpeg ? "jpg" : rawValue }
}
