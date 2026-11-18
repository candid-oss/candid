import AppKit
@preconcurrency import ApplicationServices
import Carbon
import CoreGraphics
import DesktopCore
import Foundation
import ImageIO
import OSLog
import CryptoKit
import ScreenCaptureKit
import UniformTypeIdentifiers

/// Single native request, including requests whose caller has already timed out.
/// A timeout releases the waiter, never the native request's occupied slot.
@MainActor
public final class DesktopCaptureProvider {
    private final class Request {
        let id: UUID
        let epoch: UInt64
        let focus: DesktopFocusReader.Focus
        let allowed: Set<String>
        let allowsMissingControlFocus: Bool
        var collectsAX = false
        var includeAXFocus = false
        var axEvidence: DesktopAXEvidence?
        var capturedAt: Date?
        var continuation: CheckedContinuation<CapturedWindow, any Error>?
        var timeout: Task<Void, Never>?
        var windowServerID: UInt32?
        var imageOptions = DesktopImageOptions()
        var deliversFrame = false
        var frame: CGImage?
        init(id: UUID, epoch: UInt64, focus: DesktopFocusReader.Focus, allowed: Set<String>, allowsMissingControlFocus: Bool, continuation: CheckedContinuation<CapturedWindow, any Error>) {
            self.id = id; self.epoch = epoch; self.focus = focus; self.allowed = allowed; self.continuation = continuation
            self.allowsMissingControlFocus = allowsMissingControlFocus
        }
    }
    private struct ContentResponse: @unchecked Sendable {
        let content: SCShareableContent?
        let error: (any Error)?
    }
    private struct ImageResponse: @unchecked Sendable {
        let image: CGImage?
        let error: (any Error)?
    }
    private typealias EncodedImage = DesktopEncodedImage
    private let windowLogger = Logger(subsystem: "io.github.candid-oss.candid", category: "WindowMatch")
    private var imageOptions = DesktopImageOptions()
    private var encodingCache: EncodedImage?
    private var encodingReuseEnabled = false
    private var rawFrameDelivery = false
    /// Hands the uncompressed frame to the caller with the encoded image, so OCR can read it before compression.
    public func setRawFrameDelivery(_ enabled: Bool) { rawFrameDelivery = enabled }
    public func setImageEncodingReuseEnabled(_ enabled: Bool) {
        encodingReuseEnabled = enabled
        if !enabled { encodingCache = nil }
    }
    public func setImageOptions(_ options: DesktopImageOptions) {
        if options.isValid, options != imageOptions { imageOptions = options; encodingCache = nil }
    }
    private var active: Request?
    private var preparingID: UUID?
    private var preparationCancelled = false
    public var isBusy: Bool { active != nil || preparingID != nil }
    public init() {}

    @discardableResult public func requestScreenPermission() -> Bool { CGRequestScreenCaptureAccess() }
    @discardableResult public func requestAccessibilityPermission() -> Bool {
        AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary)
    }
    public func cancelPendingCapture() {
        encodingCache = nil; encodingReuseEnabled = false
        if preparingID != nil { preparationCancelled = true }
        guard let active else { return }
        resolve(active, result: .failure(DesktopCaptureError.cancelled))
        // Keep the slot occupied until the OS callback/encoding work finishes.
    }

    public func captureFrontmost(allowedBundleIDs: Set<String>, windowCompatibilityBundleIDs: Set<String> = [],
                                 axTextBundleIDs: Set<String> = [], includeAXFocus: Bool = false) async throws -> CapturedWindow {
        guard !isBusy else { throw DesktopCaptureError.busy }
        guard DesktopSessionSafety.canCapture else { throw DesktopCaptureError.inactiveSession }
        guard CGPreflightScreenCaptureAccess() else { throw DesktopCaptureError.screenPermissionDenied }
        guard AXIsProcessTrusted() else { throw DesktopCaptureError.accessibilityPermissionDenied }
        guard let application = DesktopFocusReader.application(NSWorkspace.shared.frontmostApplication),
              allowedBundleIDs.contains(application.bundleID), application.bundleID != Bundle.main.bundleIdentifier,
              application.bundleID != "com.apple.loginwindow" else { throw DesktopCaptureError.excludedApplication }
        let requestID = UUID()
        preparingID = requestID; preparationCancelled = false
        defer { if preparingID == requestID { preparingID = nil; preparationCancelled = false } }
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            let focus = try await DesktopFocusReader.focusedWindow()
            guard focus.application.pid == application.pid, focus.application.bundleID == application.bundleID else { throw DesktopCaptureError.focusChanged }
            let allowsMissingControlFocus = windowCompatibilityBundleIDs.contains(application.bundleID)
            try Self.validateControlFocus(focus, allowsMissingControlFocus: allowsMissingControlFocus)
            try Task.checkCancellation()
            guard !preparationCancelled else { throw DesktopCaptureError.cancelled }
            guard DesktopSessionSafety.canCapture else { throw DesktopCaptureError.inactiveSession }
            guard allowedBundleIDs.contains(focus.application.bundleID),
                  focus.application.bundleID != Bundle.main.bundleIdentifier,
                  focus.application.bundleID != "com.apple.loginwindow" else { throw DesktopCaptureError.excludedApplication }
            return try await withCheckedThrowingContinuation { continuation in
                let request = Request(id: requestID, epoch: DesktopSessionSafety.epoch, focus: focus, allowed: allowedBundleIDs, allowsMissingControlFocus: allowsMissingControlFocus, continuation: continuation)
                request.imageOptions = imageOptions
                request.collectsAX = axTextBundleIDs.contains(application.bundleID)
                request.includeAXFocus = includeAXFocus
                request.deliversFrame = rawFrameDelivery
                active = request
                setTimeout(request, seconds: 2)
                SCShareableContent.getExcludingDesktopWindows(true, onScreenWindowsOnly: true) { [weak self] content, error in
                    let response = ContentResponse(content: content, error: error)
                    Task { @MainActor [weak self] in await self?.receivedContent(response, requestID: requestID) }
                }
            }
        } onCancel: {
            Task { @MainActor [weak self] in
                guard let self, self.active?.id == requestID || self.preparingID == requestID else { return }
                self.cancelPendingCapture()
            }
        }
    }

    private func receivedContent(_ response: ContentResponse, requestID: UUID) async {
        guard let request = active, request.id == requestID else { return }
        request.timeout?.cancel()
        guard request.continuation != nil else { finishNativeRequest(request); return }
        do {
            try await validate(request)
            guard response.error == nil, let content = response.content else { throw DesktopCaptureError.unavailable }
            let candidates = content.windows.compactMap { window -> DesktopWindowCandidate? in
                guard let application = window.owningApplication else { return nil }
                return DesktopWindowCandidate(id: window.windowID, pid: application.processID, title: window.title ?? "", frame: window.frame, isOnScreen: window.isOnScreen)
            }
            let windowID: UInt32
            do {
                windowID = try DesktopWindowMatcher.match(pid: request.focus.application.pid, title: request.focus.title, frame: request.focus.frame, candidates: candidates, windowServerCandidates: Self.windowServerCandidates(pid: request.focus.application.pid))
            } catch {
                logWindowMatch(request.focus, windows: content.windows)
                throw error
            }
            guard let window = content.windows.first(where: { $0.windowID == windowID }) else { throw DesktopCaptureError.ambiguousWindow }
            request.windowServerID = windowID
            windowLogger.notice("Resolved window app=\(request.focus.application.bundleID, privacy: .public) id=\(windowID, privacy: .public) titleEqual=\(window.title == request.focus.title, privacy: .public) via=WindowServer compatibility=\(request.focus.focusIdentity == 0, privacy: .public)")
            let filter = SCContentFilter(desktopIndependentWindow: window)
            let size = DesktopWindowMatcher.outputSize(width: window.frame.width, height: window.frame.height, scale: CGFloat(filter.pointPixelScale), maximumEdge: CGFloat(request.imageOptions.maximumEdge))
            let config = SCStreamConfiguration()
            config.width = size.width; config.height = size.height
            config.showsCursor = false
            config.capturesAudio = false
            config.ignoreShadowsSingleWindow = true
            setTimeout(request, seconds: 5)
            beginNativeScreenshot(filter: filter, config: config, requestID: requestID)
        } catch { resolve(request, result: .failure(error)); finishNativeRequest(request) }
    }

    private static func windowServerCandidates(pid: Int32) throws -> [DesktopWindowCandidate] {
        guard let entries = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else { throw DesktopCaptureError.unavailable }
        return entries.compactMap { entry in
            guard let owner = entry[kCGWindowOwnerPID as String] as? NSNumber, owner.int32Value == pid,
                  let number = entry[kCGWindowNumber as String] as? NSNumber,
                  let bounds = entry[kCGWindowBounds as String] as? [String: Any],
                  let frame = CGRect(dictionaryRepresentation: bounds as CFDictionary) else { return nil }
            return DesktopWindowCandidate(id: number.uint32Value, pid: owner.int32Value, title: "", frame: frame,
                isOnScreen: (entry[kCGWindowIsOnscreen as String] as? NSNumber)?.boolValue ?? true)
        }
    }

    /// Current allowed process only; no titles, text, URLs or other applications.
    private func logWindowMatch(_ focus: DesktopFocusReader.Focus, windows: [SCWindow]) {
        func rect(_ value: CGRect) -> [Double] { [value.minX, value.minY, value.width, value.height] }
        func titleInfo(_ title: String) -> [String: Any] {
            ["length": title.utf8.count, "hash": SHA256.hash(data: Data(title.utf8)).map { String(format: "%02x", $0) }.joined()]
        }
        let peers: [[String: Any]] = windows.filter { $0.owningApplication?.processID == focus.application.pid }.prefix(32).map { window in
            ["id": window.windowID, "frame": rect(window.frame), "onScreen": window.isOnScreen,
             "layer": window.windowLayer, "title": titleInfo(window.title ?? ""),
             "titleEqual": window.title == focus.title,
             "delta": [window.frame.minX - focus.frame.minX, window.frame.minY - focus.frame.minY,
                       window.frame.width - focus.frame.width, window.frame.height - focus.frame.height]]
        }
        let report: [String: Any] = ["app": focus.application.bundleID, "pid": focus.application.pid,
            "axFrame": rect(focus.frame), "axTitle": titleInfo(focus.title), "candidates": peers]
        if let data = try? JSONSerialization.data(withJSONObject: report, options: [.sortedKeys]), let value = String(data: data, encoding: .utf8) {
            windowLogger.error("Window matching rejected: \(value, privacy: .public)")
        }
    }

    private func beginNativeScreenshot(filter: SCContentFilter, config: SCStreamConfiguration, requestID: UUID) {
        // Callback form is deliberate: a cancelled Swift task must not hide an
        // outstanding framework request and admit a second native capture.
        SCScreenshotManager.captureImage(contentFilter: filter, configuration: config) { [weak self] image, error in
            let response = ImageResponse(image: image, error: error)
            Task { @MainActor [weak self] in await self?.receivedImage(response, requestID: requestID) }
        }
    }

    private func receivedImage(_ response: ImageResponse, requestID: UUID) async {
        guard let request = active, request.id == requestID else { return }
        request.timeout?.cancel()
        guard request.continuation != nil else { finishNativeRequest(request); return }
        request.capturedAt = Date()
        do {
            try await validate(request)
            guard response.error == nil, let image = response.image else { throw DesktopCaptureError.unavailable }
            if request.collectsAX, !IsSecureEventInputEnabled() {
                setTimeout(request, seconds: 3)
                let outline = await DesktopAXWorker.shared.screenshotEvidence(pid: request.focus.application.pid,
                    windowIdentity: request.focus.windowIdentity, frame: request.focus.frame, includeFocus: request.includeAXFocus)
                guard request.continuation != nil else { finishNativeRequest(request); return }
                if let observedAt = outline?.observedAt, let capturedAt = request.capturedAt,
                   abs(observedAt.timeIntervalSince(capturedAt)) <= 1 {
                    request.axEvidence = outline
                }
            }
            setTimeout(request, seconds: 3)
            if request.deliversFrame { request.frame = image }
            // CGImage is immutable; ImageIO encoding and native-size release stay
            // off the main actor. There is exactly one such task at any time.
            let payload = ImageResponse(image: image, error: nil)
            let imageOptions = request.imageOptions
            let cached = encodingCache
            let allowsReuse = encodingReuseEnabled
            Task.detached(priority: .utility) { [weak self] in
                let result: Result<EncodedImage, any Error>
                do {
                    guard let image = payload.image else { throw DesktopCaptureError.unavailable }
                    result = .success(try DesktopImageEncoder.encode(image, options: imageOptions, cached: cached, allowsReuse: allowsReuse))
                }
                catch { result = .failure(error) }
                await self?.receivedEncoding(result, requestID: requestID)
            }
        } catch { resolve(request, result: .failure(error)); finishNativeRequest(request) }
    }

    private func receivedEncoding(_ result: Result<EncodedImage, any Error>, requestID: UUID) async {
        guard let request = active, request.id == requestID else { return }
        defer { finishNativeRequest(request) }
        guard request.continuation != nil else { return }
        do {
            try await validate(request)
            if case .success(let image) = result { encodingCache = image }
            let encoded = try result.get()
            let output = CapturedWindow(data: encoded.data, application: request.focus.application, title: request.focus.title,
                                        width: encoded.width, height: encoded.height, capturedAt: request.capturedAt ?? Date(),
                                        windowIdentity: request.focus.windowIdentity,
                                        axEvidence: IsSecureEventInputEnabled() ? nil : request.axEvidence,
                                        frame: request.frame.map(DesktopRawFrame.init))
            resolve(request, result: .success(output))
        } catch { resolve(request, result: .failure(error)) }
    }

    private func validate(_ request: Request) async throws {
        guard DesktopSessionSafety.canCapture else { throw DesktopCaptureError.inactiveSession }
        guard request.epoch == DesktopSessionSafety.epoch else { throw DesktopCaptureError.focusChanged }
        guard CGPreflightScreenCaptureAccess() else { throw DesktopCaptureError.screenPermissionDenied }
        guard AXIsProcessTrusted() else { throw DesktopCaptureError.accessibilityPermissionDenied }
        guard let application = DesktopFocusReader.application(NSWorkspace.shared.frontmostApplication),
              application.pid == request.focus.application.pid, request.allowed.contains(application.bundleID) else { throw DesktopCaptureError.focusChanged }
        let focus = try await DesktopFocusReader.focusedWindow()
        try Self.validateControlFocus(focus, allowsMissingControlFocus: request.allowsMissingControlFocus)
        guard request.continuation != nil, request.epoch == DesktopSessionSafety.epoch, DesktopSessionSafety.canCapture else { throw DesktopCaptureError.focusChanged }
        guard focus.application.pid == request.focus.application.pid, focus.application.bundleID == request.focus.application.bundleID,
              focus.title == request.focus.title, focus.frame == request.focus.frame, focus.windowIdentity == request.focus.windowIdentity,
              request.allowed.contains(focus.application.bundleID) else { throw DesktopCaptureError.focusChanged }
        if let expectedID = request.windowServerID {
            let actualID = try DesktopWindowMatcher.resolveWindowServer(pid: focus.application.pid, frame: focus.frame,
                candidates: Self.windowServerCandidates(pid: focus.application.pid))
            guard actualID == expectedID else { throw DesktopCaptureError.focusChanged }
        }
    }
    static func validateControlFocus(_ focus: DesktopFocusReader.Focus, allowsMissingControlFocus: Bool,
                                     secureInputEnabled: () -> Bool = { IsSecureEventInputEnabled() }) throws {
        guard focus.context.isProtected != true else { throw DesktopCaptureError.protectedContent }
        guard focus.context.isComposing != true else { throw DesktopCaptureError.contentChanging }
        guard focus.focusIdentity == 0 else { return }
        guard allowsMissingControlFocus, focus.controlFocusAbsent else { throw DesktopCaptureError.noVerifiedFocus }
        guard !secureInputEnabled() else { throw DesktopCaptureError.protectedContent }
    }
    private func setTimeout(_ request: Request, seconds: Int) {
        request.timeout?.cancel()
        let id = request.id
        request.timeout = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(seconds)) } catch { return }
            guard let self, let current = self.active, current.id == id else { return }
            self.resolve(current, result: .failure(DesktopCaptureError.timedOut))
        }
    }
    private func resolve(_ request: Request, result: Result<CapturedWindow, any Error>) {
        request.timeout?.cancel(); request.timeout = nil
        let continuation = request.continuation; request.continuation = nil
        continuation?.resume(with: result)
    }
    private func finishNativeRequest(_ request: Request) {
        request.timeout?.cancel()
        if active?.id == request.id { active = nil }
    }
}
