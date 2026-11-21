import Foundation
import Testing
@testable import DesktopCore

/// Derived JSON originals (AX evidence, recognition results) are stored by the core as raw DEFLATE with a
/// `.json.deflate` suffix; older ones stay plain. Synthetic data only.
struct DerivedJSONTests {
    /// Written by the core's `json_blobs::compress_json` (flate2) for the JSON below.
    static let coreDeflate: [UInt8] = [
        0x8d, 0xca, 0xc1, 0x09, 0x80, 0x30, 0x0c, 0x05, 0xd0, 0x55, 0xca, 0x3f, 0xf7, 0xe0, 0xb9, 0x37, 0x71, 0x00, 0xc1, 0x0d,
        0x24, 0x09, 0xb4, 0x10, 0x1b, 0xb1, 0x11, 0x14, 0x71, 0x77, 0x1d, 0xa1, 0xc7, 0x07, 0xef, 0x01, 0xd9, 0xb6, 0xab, 0xb8,
        0xf0, 0xe8, 0x48, 0x43, 0x44, 0xb3, 0xf3, 0x20, 0x41, 0x42, 0xbb, 0xab, 0x67, 0xf1, 0x42, 0x61, 0x9e, 0x16, 0x44, 0xb8,
        0x5c, 0xff, 0x40, 0x16, 0x55, 0x0b, 0xb4, 0x56, 0x2e, 0x1c, 0xfa, 0x80, 0xf7, 0x03,
    ]
    static let json = #"{"completedAt":0,"source":"synthetic OCR","text":"hello candid hello candid hello candid hello candid"}"#

    private func directory() throws -> URL {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("derived-json-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    @Test func readsCoreCompressedFile() throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("abc.json.deflate")
        try Data(Self.coreDeflate).write(to: file)
        let data = try #require(DesktopEventMapping.derivedJSON(at: file))
        #expect(String(decoding: data, as: UTF8.self) == Self.json)
    }

    @Test func readsPlainFileAndRoundTrip() throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let plain = dir.appendingPathComponent("abc.json")
        try Data(Self.json.utf8).write(to: plain)
        #expect(DesktopEventMapping.derivedJSON(at: plain) == Data(Self.json.utf8))
        let evidence = Data(#"{"nodes":[{"role":"AXStaticText","value":"synthetic"}]}"#.utf8)
        let packed = dir.appendingPathComponent("def.json.deflate")
        try ((evidence as NSData).compressed(using: .zlib) as Data).write(to: packed)
        #expect(DesktopEventMapping.derivedJSON(at: packed) == evidence)
        let corrupt = dir.appendingPathComponent("bad.json.deflate")
        try Data("not deflate".utf8).write(to: corrupt)
        #expect(DesktopEventMapping.derivedJSON(at: corrupt) == nil)
        #expect(DesktopEventMapping.derivedJSON(at: dir.appendingPathComponent("missing.json.deflate")) == nil)
    }
}
