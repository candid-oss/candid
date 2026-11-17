import CoreGraphics
import CryptoKit
import DesktopCore
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// One immutable encoded frame, retained only until the next capture or cancellation.
final class DesktopEncodedImage: Sendable {
    let data: Data
    let width: Int
    let height: Int
    let fingerprint: Data?
    let pixels: Data?
    let options: DesktopImageOptions
    init(data: Data, image: CGImage, fingerprint: Data?, pixels: Data?, options: DesktopImageOptions) {
        self.data = data; width = image.width; height = image.height
        self.fingerprint = fingerprint; self.pixels = pixels; self.options = options
    }
}

enum DesktopImageEncoder {
    /// Pixel interpretation, including colour profile/decode. The pixels themselves are compared byte-for-byte.
    static func fingerprint(_ image: CGImage) -> Data? {
        var hash = SHA256()
        let metadata = [image.width, image.height, image.bitsPerComponent, image.bitsPerPixel, image.bytesPerRow,
                        Int(image.bitmapInfo.rawValue), Int(image.alphaInfo.rawValue), Int(image.renderingIntent.rawValue),
                        image.shouldInterpolate ? 1 : 0, image.isMask ? 1 : 0,
                        image.colorSpace == nil ? 0 : 1, image.decode == nil ? 0 : 1]
        hash.update(data: Data(metadata.map(String.init).joined(separator: ",").utf8))
        if let space = image.colorSpace {
            if let profile = space.copyICCData() {
                hash.update(data: Data(";profile:\(CFDataGetLength(profile));".utf8)); hash.update(data: profile as Data)
            }
            else if let name = space.name {
                let value = name as String
                hash.update(data: Data(";space:\(value.utf8.count);\(value)".utf8))
            }
            else { return nil }
            if let decode = image.decode {
                hash.update(data: Data(bytes: decode, count: 2 * space.numberOfComponents * MemoryLayout<CGFloat>.size))
            }
        }
        return Data(hash.finalize())
    }

    static func encode(_ image: CGImage, options: DesktopImageOptions, cached: DesktopEncodedImage? = nil, allowsReuse: Bool = true) throws -> DesktopEncodedImage {
        try autoreleasepool {
            // A native 1920px frame fits; large/HDR frames use the original direct encoder without retaining pixels.
            let maximumPixelBytes = 16 * 1024 * 1024
            let bounded = allowsReuse && image.bytesPerRow > 0 && image.height <= maximumPixelBytes / image.bytesPerRow
            let key = bounded ? fingerprint(image) : nil
            let copied = key == nil ? nil : image.dataProvider?.data as Data?
            let pixels = copied.flatMap { $0.count <= maximumPixelBytes ? $0 : nil }
            if let key, let pixels, let cached, key == cached.fingerprint, pixels == cached.pixels, cached.options == options { return cached }
            let data = NSMutableData()
            guard let destination = CGImageDestinationCreateWithData(data, UTType.jpeg.identifier as CFString, 1, nil) else { throw DesktopCaptureError.unavailable }
            CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: options.jpegQuality] as CFDictionary)
            guard CGImageDestinationFinalize(destination) else { throw DesktopCaptureError.unavailable }
            if data.length <= 2 * 1024 * 1024 { return DesktopEncodedImage(data: data as Data, image: image, fingerprint: key, pixels: pixels, options: options) }
            guard options.reduceOversizedImages else { throw DesktopCaptureError.imageTooLarge }
            let smaller = NSMutableData()
            guard let retry = CGImageDestinationCreateWithData(smaller, UTType.jpeg.identifier as CFString, 1, nil) else { throw DesktopCaptureError.unavailable }
            CGImageDestinationAddImage(retry, image, [kCGImageDestinationLossyCompressionQuality: min(0.55, options.jpegQuality)] as CFDictionary)
            guard CGImageDestinationFinalize(retry), smaller.length <= 2 * 1024 * 1024 else { throw DesktopCaptureError.imageTooLarge }
            return DesktopEncodedImage(data: smaller as Data, image: image, fingerprint: key, pixels: pixels, options: options)
        }
    }
}
