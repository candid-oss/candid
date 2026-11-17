import CoreGraphics
import DesktopCore
import Foundation
import ImageIO
import Testing
import UniformTypeIdentifiers
@testable import DesktopPlatform

@Suite struct DesktopImageEncoderTests {
    private func frame(width: Int = 1280, height: Int = 720, changed: Bool = false) throws -> CGImage {
        let context = try #require(CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                                            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        context.setFillColor(CGColor(red: 0.9, green: 0.8, blue: 0.7, alpha: 1)); context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        if changed { context.setFillColor(CGColor(gray: 0, alpha: 1)); context.fill(CGRect(x: 30, y: 30, width: 20, height: 20)) }
        return try #require(context.makeImage())
    }

    @Test func identicalPixelsReuseEncodingButChangedPixelsAndOptionsDoNot() throws {
        let first = try DesktopImageEncoder.encode(frame(), options: DesktopImageOptions())
        let repeatFrame = try frame()
        #expect(try DesktopImageEncoder.encode(repeatFrame, options: DesktopImageOptions(), cached: first) === first)
        let changed = try DesktopImageEncoder.encode(frame(changed: true), options: DesktopImageOptions(), cached: first)
        #expect(changed !== first)
        #expect(changed.data != first.data)
        var options = DesktopImageOptions(); options.jpegQuality = 0.5
        let quality = try DesktopImageEncoder.encode(repeatFrame, options: options, cached: first)
        #expect(quality !== first)
        #expect(quality.options == options)
        let resized = try DesktopImageEncoder.encode(frame(width: 640, height: 360), options: options, cached: quality)
        #expect(resized.width == 640 && resized.height == 360)
        #expect(resized !== quality)
        let direct = try DesktopImageEncoder.encode(repeatFrame, options: DesktopImageOptions(), cached: first, allowsReuse: false)
        #expect(direct !== first && direct.pixels == nil && direct.fingerprint == nil)
        #expect(direct.data == first.data)
    }

    @Test func repeatedFrameBenchmarkPreservesExactJPEG() throws {
        // A fixed synthetic frame; compare the same encoder and quality with/without the single-entry cache.
        let image = try frame(width: 1920, height: 1080)
        let options = DesktopImageOptions()
        let cached = try DesktopImageEncoder.encode(image, options: options)
        let originalStart = ContinuousClock.now
        for _ in 0..<20 {
            let bytes = NSMutableData()
            let output = try #require(CGImageDestinationCreateWithData(bytes, UTType.jpeg.identifier as CFString, 1, nil))
            CGImageDestinationAddImage(output, image, [kCGImageDestinationLossyCompressionQuality: options.jpegQuality] as CFDictionary)
            #expect(CGImageDestinationFinalize(output))
            #expect(bytes as Data == cached.data)
        }
        let original = originalStart.duration(to: .now)
        let start = ContinuousClock.now
        for _ in 0..<20 { #expect(try DesktopImageEncoder.encode(image, options: options).data == cached.data) }
        let uncached = start.duration(to: .now)
        let hitStart = ContinuousClock.now
        for _ in 0..<20 { #expect(try DesktopImageEncoder.encode(image, options: options, cached: cached) === cached) }
        let hits = hitStart.duration(to: .now)
        let pixels = try #require(image.dataProvider?.data) as Data
        let compareStart = ContinuousClock.now
        for _ in 0..<20 {
            let other = try #require(image.dataProvider?.data) as Data
            #expect(other == pixels)
            _ = image.colorSpace?.copyICCData()
        }
        let compare = compareStart.duration(to: .now)
        print("SYNTHETIC-ENCODING oldJPEG=\(original) compareAndJPEG=\(uncached) exactFrameHit=\(hits) exactPixelCompare=\(compare)")
        // Timings are reported, not asserted: host load must not turn correctness into a flaky test.
    }

    @Test func differentColourInterpretationCannotReuseAPreviousFrame() throws {
        let image = try frame()
        let first = try DesktopImageEncoder.encode(image, options: DesktopImageOptions())
        let converted = try #require(image.copy(colorSpace: CGColorSpace(name: CGColorSpace.displayP3)!))
        #expect(DesktopImageEncoder.fingerprint(image) != DesktopImageEncoder.fingerprint(converted))
        #expect(try DesktopImageEncoder.encode(converted, options: DesktopImageOptions(), cached: first) !== first)
    }

    @Test func largeFramesUseDirectEncodingWithoutRetainingPixelBuffers() throws {
        let image = try frame(width: 2048, height: 2049)
        let first = try DesktopImageEncoder.encode(image, options: DesktopImageOptions())
        #expect(first.pixels == nil)
        #expect(first.fingerprint == nil)
        #expect(try DesktopImageEncoder.encode(image, options: DesktopImageOptions(), cached: first) !== first)
    }
}
