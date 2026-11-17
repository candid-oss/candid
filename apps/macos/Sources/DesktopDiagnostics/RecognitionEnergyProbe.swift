import CoreGraphics
import CoreText
import Darwin
import DesktopPlatform
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// Offline measurements of the actual product OCR and VLM request preparation.
/// Generates only marked synthetic images; never opens a screen, library, keychain or network connection.
enum RecognitionEnergyProbe {
    private static let markers = ["SYNTHETIC-A1", "TOTAL 128.50", "DEADLINE 2026-11-18", "STATUS READY", "42", "09:30", "项目状态", "屏幕证据", "FINE-19X", "备注明细"]

    static func run(root: URL, iterations: Int) async throws {
        let ocr = DesktopOCR()
        try emit(["stage": "probe_start", "pid": getpid(), "at_ms": milliseconds(), "iterations": iterations,
                  "input": "synthetic_only", "fixture_version": "recognition-energy/2-fine-print", "network": false,
                  "ocr": "product DesktopOCR / Apple Vision accurate zh-Hans zh-Hant en-US"])
        // Separates framework cold initialization from subsequent requests and gives a matched idle window.
        try await Task.sleep(for: .seconds(3))
        for width in [1920, 1280, 960] {
            for index in 0..<iterations {
                let image = try syntheticImage(width: width, variant: index)
                if index == 0 { try image.write(to: root.appendingPathComponent("SYNTHETIC-\(width).jpg")) }
                let begin = milliseconds(), cpu = cpuSeconds()
                let result = try await ocr.recognize(image)
                let end = milliseconds(), elapsedCPU = cpuSeconds() - cpu
                let folded = result.text.replacingOccurrences(of: " ", with: "").lowercased()
                let found = markers.filter { folded.contains($0.replacingOccurrences(of: " ", with: "").lowercased()) }
                try emit(["stage": "ocr", "width": width, "iteration": index, "cold_first_call": width == 1920 && index == 0,
                          "start_ms": begin, "end_ms": end, "cpu_seconds": elapsedCPU, "jpeg_bytes": image.count,
                          "marker_count": markers.count, "markers_found": found.count, "lines": result.ocrLines?.count ?? 0])
                let preparationStart = milliseconds(), preparationCPU = cpuSeconds()
                let prepared = try DesktopVisionClient.prepare(image, ocrReference: result)
                let bodyImage = prepared.image.base64EncodedString()
                if index == 0 {
                    let message: [String: Any] = ["messages": [["role": "user", "content": [
                        ["type": "text", "text": prepared.instruction],
                        ["type": "image_url", "image_url": ["url": "data:image/jpeg;base64," + bodyImage]]
                    ]]]]
                    try JSONSerialization.data(withJSONObject: message).write(to: root.appendingPathComponent("SYNTHETIC-vlm-\(width).json"))
                }
                try emit(["stage": "vlm_prepare", "width": width, "iteration": index, "start_ms": preparationStart,
                          "end_ms": milliseconds(), "cpu_seconds": cpuSeconds() - preparationCPU,
                          "image_bytes": prepared.image.count, "base64_bytes": bodyImage.utf8.count,
                          "instruction_bytes": prepared.instruction.utf8.count, "network": false])
            }
            try await Task.sleep(for: .seconds(3))
        }
        try emit(["stage": "probe_end", "at_ms": milliseconds()])
    }

    private static func milliseconds() -> Int64 { Int64(Date().timeIntervalSince1970 * 1000) }

    private static func cpuSeconds() -> Double {
        var usage = rusage()
        getrusage(RUSAGE_SELF, &usage)
        return Double(usage.ru_utime.tv_sec + usage.ru_stime.tv_sec)
            + Double(usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) / 1_000_000
    }

    private static func emit(_ value: [String: Any]) throws {
        var data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
        data.append(0x0A)
        try FileHandle.standardOutput.write(contentsOf: data)
    }

    private static func syntheticImage(width: Int, variant: Int) throws -> Data {
        try autoreleasepool {
            let height = width * 1080 / 1920, scale = CGFloat(width) / 1920
            guard let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
                                          bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                                          bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { throw ProbeError.image }
            context.setFillColor(CGColor(gray: 1, alpha: 1)); context.fill(CGRect(x: 0, y: 0, width: width, height: height))
            context.scaleBy(x: scale, y: scale)
            context.setFillColor(CGColor(gray: 0.93, alpha: 1)); context.fill(CGRect(x: 0, y: 0, width: 270, height: 1080))
            let rows: [(String, CGFloat, CGFloat, CGFloat)] = [
                ("SYNTHETIC-A1 · 能耗评测", 320, 975, 36), ("项目状态 / 屏幕证据", 320, 902, 28),
                ("09:30 Review captured evidence and preserve source links.", 320, 820, 26),
                ("TOTAL 128.50", 320, 725, 26), ("DEADLINE 2026-11-18", 320, 655, 26),
                ("STATUS READY", 320, 585, 26), ("42 tasks verified with synthetic data only.", 320, 515, 26),
                ("if status == ready: save_record()", 320, 420, 24),
                ("No personal content. No microphone. No screen pixels.", 320, 330, 24),
                ("Synthetic revision \(variant)", 320, 220, 24), ("FINE-19X 2087", 320, 150, 16),
                ("备注明细：保持原文与数字", 320, 100, 16),
                ("Navigation", 30, 980, 24), ("Document", 30, 900, 24)
            ]
            for (text, x, y, size) in rows {
                let font = CTFontCreateWithName("PingFangSC-Regular" as CFString, size, nil)
                let attributes = [kCTFontAttributeName as NSAttributedString.Key: font,
                                  kCTForegroundColorAttributeName as NSAttributedString.Key: CGColor(gray: 0.08, alpha: 1)] as [NSAttributedString.Key: Any]
                let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
                context.textPosition = CGPoint(x: x, y: y)
                CTLineDraw(line, context)
            }
            guard let image = context.makeImage() else { throw ProbeError.image }
            let data = NSMutableData()
            guard let destination = CGImageDestinationCreateWithData(data, UTType.jpeg.identifier as CFString, 1, nil) else { throw ProbeError.image }
            CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: 0.8] as CFDictionary)
            guard CGImageDestinationFinalize(destination) else { throw ProbeError.image }
            return data as Data
        }
    }

    private enum ProbeError: Error { case image }
}
