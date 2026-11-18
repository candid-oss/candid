import CoreGraphics
import Darwin
import DesktopCore
import Foundation

/// Screenshot originals are WebP at quality 60 (data model v1 §9.1). ImageIO reads WebP but cannot write it, so the
/// frame goes to the core's statically linked libwebp: `candid encode-webp`, raw BGRA on stdin, WebP on stdout.
/// The App's helper binary is used directly; no running core is needed.
public enum DesktopWebPEncoder {
    public static let quality = 60

    /// `CANDID_CORE_BIN` (tests, development) or the App's own `Contents/Helpers/candid`.
    public static let executable: URL? = {
        if let path = ProcessInfo.processInfo.environment["CANDID_CORE_BIN"], FileManager.default.isExecutableFile(atPath: path) {
            return URL(fileURLWithPath: path)
        }
        let helper = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/candid")
        return FileManager.default.isExecutableFile(atPath: helper.path) ? helper : nil
    }()

    public static var isAvailable: Bool { executable != nil }

    public enum Failure: Error { case unavailable, pixels, encoder(String), timedOut }

    /// The frame drawn into sRGB 8-bit BGRA, then encoded. Throws when the helper is missing or fails; the caller
    /// keeps the screenshot as JPEG instead.
    public static func encode(_ image: CGImage, quality: Int = quality, timeout: TimeInterval = 10) throws -> Data {
        guard let executable else { throw Failure.unavailable }
        let (pixels, stride) = try bgra(image)
        return try run(executable, pixels: pixels, width: image.width, height: image.height, stride: stride,
                       quality: quality, timeout: timeout)
    }

    static func bgra(_ image: CGImage) throws -> (Data, Int) {
        let width = image.width, height = image.height
        guard width > 0, height > 0, width <= 16_383, height <= 16_383,
              let space = CGColorSpace(name: CGColorSpace.sRGB) else { throw Failure.pixels }
        let stride = width * 4
        var pixels = Data(count: stride * height)
        let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
            guard let context = CGContext(data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8,
                                          bytesPerRow: stride, space: space,
                                          bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
            else { return false }
            context.interpolationQuality = .none
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard drawn else { throw Failure.pixels }
        return (pixels, stride)
    }

    static func run(_ executable: URL, pixels: Data, width: Int, height: Int, stride: Int, quality: Int, timeout: TimeInterval) throws -> Data {
        let process = Process()
        process.executableURL = executable
        process.arguments = ["encode-webp", "--width", String(width), "--height", String(height),
                             "--stride", String(stride), "--quality", String(quality)]
        let input = Pipe(), output = Pipe(), errors = Pipe()
        process.standardInput = input; process.standardOutput = output; process.standardError = errors
        try process.run()
        let killer = DispatchWorkItem { if process.isRunning { process.terminate() } }
        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + timeout, execute: killer)
        defer { killer.cancel() }
        // An encoder that exits early must not raise SIGPIPE in the App.
        let fd = input.fileHandleForWriting.fileDescriptor
        _ = fcntl(fd, F_SETNOSIGPIPE, 1)
        let wrote = pixels.withUnsafeBytes { buffer -> Bool in
            var offset = 0
            while offset < buffer.count {
                let n = Darwin.write(fd, buffer.baseAddress! + offset, buffer.count - offset)
                if n < 0 { if errno == EINTR { continue }; return false }
                offset += n
            }
            return true
        }
        try? input.fileHandleForWriting.close()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        // The encoder writes at most one short line to stderr, well inside the pipe buffer.
        let errorText = errors.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        if process.terminationReason == .uncaughtSignal { throw Failure.timedOut }
        guard wrote, process.terminationStatus == 0, isWebP(data) else {
            throw Failure.encoder(String(decoding: errorText.prefix(500), as: UTF8.self))
        }
        return data
    }

    public static func isWebP(_ data: Data) -> Bool { DesktopImageFormat(data) == .webp }
}
