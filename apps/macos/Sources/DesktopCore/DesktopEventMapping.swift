import CryptoKit
import CandidLocalization
import Foundation
import CandidClient

/// Errors of the capture's storage path (records, screenshots, the outbox and its reads from the core).
public enum DesktopStoreError: Error, LocalizedError, Sendable, Equatable {
    case invalidRecord, invalidHash, missingImage, imageTooLarge, quotaExceeded, deletedRecord, destinationExists
    /// The bounded outbox is full: capture stops until the core takes what is waiting.
    case outboxFull(Int)
    case unavailable(String)

    public var errorDescription: String? {
        switch self {
        case .invalidRecord: L("记录内容或参数无效。")
        case .invalidHash: L("图片校验标识无效。")
        case .missingImage: L("图片缺失或校验不通过。")
        case .imageTooLarge: L("图片超过 2 MiB 限制。")
        case .quotaExceeded: L("本地空间上限已到，已停止新增图片。")
        case .deletedRecord: L("记录已删除，已阻止迟到结果重新写入。")
        case .destinationExists: L("导出目标已存在，请选择新的目录。")
        case .outboxFull(let count): L("待写入时间线库的记录已达上限（%@ 条），采集已停止。时间线核心恢复后会自动补写。", count)
        case .unavailable(let detail): L("时间线库暂不可用：%@", detail)
        }
    }
}

/// Turns a capture record into the timeline event the ledger import makes of the same record (timeline-v1 §9), so
/// records captured directly and records imported from the old ledger read back the same way. The golden file
/// `core/fixtures/desktop-mapping.json`, written by the core's import, pins the shape for both sides.
public enum DesktopEventMapping {
    public static let maximumImageBytes = 2 * 1024 * 1024
    /// Seconds between 1970-01-01 and 2001-01-01, as the core computes record times.
    private static let referenceOffset = 978_307_200.0

    public static func milliseconds(_ date: Date) -> Int64 {
        Int64(((date.timeIntervalSinceReferenceDate + referenceOffset) * 1000).rounded())
    }

    public static func sha256(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }

    /// A derived JSON original from the core (AX evidence, recognition result). The core stores these as raw DEFLATE
    /// with a `.json.deflate` suffix (data model v1 §9.5); older ones are plain JSON. Nil when unreadable.
    public static func derivedJSON(at url: URL) -> Data? {
        guard let stored = try? Data(contentsOf: url) else { return nil }
        guard url.lastPathComponent.hasSuffix(".deflate") else { return stored }
        // Foundation's `.zlib` is raw DEFLATE (RFC 1951), the format the core writes.
        return try? (stored as NSData).decompressed(using: .zlib) as Data
    }

    /// The AX evidence file of a screenshot: the bytes whose hash is `payload.ax_evidence_ref`.
    public static func evidenceData(_ evidence: DesktopAXEvidence) throws -> Data {
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        return try encoder.encode(evidence)
    }

    /// The event for `record`. A screenshot's original is described by `imageHash` and size; its file and the AX
    /// evidence file are staged by the caller (`files` returns the evidence bytes by hash). Blob staging paths are
    /// left empty for the caller to fill.
    /// `imageFormat` is the screenshot original's stored format (WebP for new captures).
    public static func event(for r: DesktopRecord, imageFormat: DesktopImageFormat = .jpeg) throws -> (event: IngestEvent, files: [String: Data]) {
        var event = IngestEvent(source_key: r.id, kind: kind(r.kind), start_ms: milliseconds(r.timestamp))
        var files: [String: Data] = [:]
        var legacy: [String: CoreJSON] = ["run_id": .string(r.runID)]
        if let activity = r.activityID { legacy["activity_id"] = .string(activity) }
        if !r.detail.isEmpty { legacy["detail"] = .string(r.detail) }
        var payload: [String: CoreJSON] = ["legacy": .object(legacy)]
        event.app_bundle_id = r.bundleID.isEmpty ? nil : r.bundleID
        event.app_name = r.appName.isEmpty ? nil : r.appName
        event.title = r.windowTitle.isEmpty ? nil : r.windowTitle
        event.parent_key = r.parentID
        let optional: (String?) -> CoreJSON = { $0.map(CoreJSON.string) ?? .null }
        switch r.kind {
        case .application:
            payload["reason"] = .string(r.trigger ?? "switch")
        case .window:
            payload["window_token"] = .string(String(sha256(Data("\(r.runID)|\(r.bundleID)|\(r.windowTitle)".utf8)).prefix(16)))
            payload["coverage"] = .string("window")
        case .context:
            let c = r.context
            if let evidence = c?.recordEvidence, c?.isProtected != true, c?.isComposing != true {
                guard evidence.isValid else { throw DesktopStoreError.invalidRecord }
                payload["record_evidence"] = try JSONDecoder().decode(CoreJSON.self, from: JSONEncoder().encode(evidence))
                if let value = evidence.input_value, !value.isEmpty { event.text.append(IngestText(field: "message", content: value)) }
            }
            payload["trigger"] = .string(r.trigger ?? "unknown")
            payload["text_status"] = optional(c?.textStatus)
            payload["focused_role"] = optional(c?.focusedRole)
            payload["focused_label"] = optional(c?.focusedLabel)
            payload["is_composing"] = c?.isComposing.map(CoreJSON.bool) ?? .null
            payload["reader"] = .object(["version": .int(0), "deep_pass": .bool(false), "nodes": .int(Int64(c?.axEvidence?.nodes.count ?? 0))])
            if let v = c?.sessionActive { payload["session_active"] = .bool(v) }
            if let v = c?.onBattery { payload["on_battery"] = .bool(v) }
            if let v = c?.lowPower { payload["low_power"] = .bool(v) }
            if let v = c?.thermalState { payload["thermal_state"] = .string(v) }
            if let v = c?.idleSeconds { payload["idle_seconds"] = .number(v) }
            if c?.isProtected == true {
                event.sensitivity = "protected"
            } else {
                if let body = c?.readableText, !body.isEmpty { event.text.append(IngestText(field: "body", content: body)) }
                if let selection = c?.selectedText, !selection.isEmpty { event.text.append(IngestText(field: "selection", content: selection)) }
            }
        case .screenshot:
            payload["trigger"] = .string(r.trigger ?? "unknown")
            if let w = r.imageWidth { payload["pixel_w"] = .int(Int64(w)) }
            if let h = r.imageHeight { payload["pixel_h"] = .int(Int64(h)) }
            guard let hash = r.imageHash else { throw DesktopStoreError.missingImage }
            event.blobs.append(IngestBlob(staging_path: "", sha256: hash, mime_type: imageFormat.mimeType, pixel_w: r.imageWidth, pixel_h: r.imageHeight))
            if let evidence = r.context?.axEvidence {
                let data = try evidenceData(evidence)
                let sha = sha256(data)
                files[sha] = data
                event.blobs.append(IngestBlob(staging_path: "", sha256: sha, role: "derived", mime_type: "application/json"))
                payload["ax_evidence_ref"] = .string(sha)
            }
        case .marker:
            if !r.windowTitle.isEmpty { payload["label"] = .string(r.windowTitle) }
            if !r.detail.isEmpty { payload["note"] = .string(r.detail) }
        case .system:
            let d = r.detail.lowercased()
            let change = d.contains("wake") || d.contains("唤醒") ? "wake"
                : d.contains("sleep") || d.contains("睡眠") ? "sleep"
                : d.contains("unlock") || d.contains("解锁") ? "unlock"
                : d.contains("lock") || d.contains("锁") ? "lock"
                : d.contains("power") || d.contains("battery") || d.contains("电") ? "power" : "session"
            payload["change"] = .string(change)
            payload["value"] = .string(r.detail)
        case .gap:
            let d = r.detail.lowercased()
            let reason = d.contains("pause") || d.contains("暂停") ? "paused"
                : d.contains("permission") || d.contains("权限") ? "permission_denied"
                : d.contains("sleep") || d.contains("睡眠") ? "asleep"
                : d.contains("crash") || d.contains("崩溃") ? "crashed" : "unknown"
            payload["reason_code"] = .string(reason)
        }
        if r.kind == .context, r.context?.recordEvidence != nil, event.text.isEmpty {
            event.kind = "coverage.gap"; payload["reason_code"] = .string("text_unavailable")
        }
        event.payload = payload
        return (event, files)
    }

    static func kind(_ kind: DesktopRecordKind) -> String {
        switch kind {
        case .application: "app.focus"
        case .window: "window.focus"
        case .context: "screen.text"
        case .screenshot: "screen.shot"
        case .marker: "marker"
        case .system: "system.state"
        case .gap: "coverage.gap"
        }
    }

    // MARK: Reading back

    public static func record(_ a: AppRecord) -> DesktopRecord {
        var result = DesktopRecord(id: a.id, timestamp: Date(timeIntervalSince1970: Double(a.ts_ms) / 1000), kind: DesktopRecordKind(rawValue: a.kind) ?? .gap,
                      appName: a.app_name, bundleID: a.bundle_id, windowTitle: a.window_title, detail: a.detail, imageHash: a.image_hash,
                      imageWidth: a.image_width, imageHeight: a.image_height, parentID: a.parent_id, runID: a.run_id, trigger: a.trigger,
                      context: a.context.map(context), activityID: a.activity_id)
        result.manualTitle = a.manual_title; result.manualNotes = a.manual_notes
        return result
    }

    static func context(_ c: AppContext) -> DesktopContextSnapshot {
        var result = DesktopContextSnapshot(focusedRole: c.focused_role, focusedLabel: c.focused_label, selectedText: c.selected_text, readableText: c.readable_text,
                               textStatus: c.text_status, isProtected: c.is_protected, sessionActive: c.session_active, onBattery: c.on_battery,
                               lowPower: c.low_power, thermalState: c.thermal_state, idleSeconds: c.idle_seconds, isComposing: c.is_composing)
        if let raw = c.record_evidence, let data = try? JSONEncoder().encode(raw) {
            result.recordEvidence = try? JSONDecoder().decode(DesktopRecordEvidence.self, from: data)
        }
        return result
    }

    public static func event(_ a: AppActivity) -> DesktopTimelineEvent {
        DesktopTimelineEvent(record: record(a.anchor), screenshots: a.screenshots.map(record), issues: a.issues.map(record),
                             observations: a.observations.map(record), observationOffset: a.observation_offset, sampleCount: a.sample_count,
                             latestRecord: record(a.latest), startDate: Date(timeIntervalSince1970: Double(a.start_ms) / 1000),
                             endDate: Date(timeIntervalSince1970: Double(a.end_ms) / 1000), hasScreenshot: a.has_screenshot, hasIssue: a.has_issue)
    }

    // MARK: Limits (the ledger's write checks, kept for direct capture)

    public static func validate(_ record: DesktopRecord) throws {
        if let context = record.context {
            if let ax = context.axEvidence {
                guard record.kind == .screenshot, context.isProtected != true, context.isComposing != true, ax.isValid else { throw DesktopStoreError.invalidRecord }
            }
            guard (context.readableText?.utf8.count ?? 0) <= 4096, (context.selectedText?.utf8.count ?? 0) <= 2048,
                  (context.focusedRole?.utf8.count ?? 0) <= 128, (context.focusedLabel?.utf8.count ?? 0) <= 512,
                  (context.textStatus?.utf8.count ?? 0) <= 128, (context.thermalState?.utf8.count ?? 0) <= 128 else { throw DesktopStoreError.invalidRecord }
            if let idle = context.idleSeconds, !idle.isFinite || idle < 0 { throw DesktopStoreError.invalidRecord }
            if context.isProtected == true, !(context.readableText ?? "").isEmpty || !(context.selectedText ?? "").isEmpty || !(context.focusedLabel ?? "").isEmpty {
                throw DesktopStoreError.invalidRecord
            }
            if context.isComposing == true, !(context.readableText ?? "").isEmpty || !(context.selectedText ?? "").isEmpty { throw DesktopStoreError.invalidRecord }
        }
        guard !record.id.isEmpty, record.id.utf8.count <= 200, record.timestamp.timeIntervalSince1970.isFinite,
              record.appName.utf8.count <= 2048, record.bundleID.utf8.count <= 1024,
              record.windowTitle.utf8.count <= 8192, record.detail.utf8.count <= 16_384, (record.trigger?.utf8.count ?? 0) <= 128,
              record.activityID == nil || (!(record.activityID ?? "").isEmpty && (record.activityID?.utf8.count ?? 0) <= 200) else {
            throw DesktopStoreError.invalidRecord
        }
        if let hash = record.imageHash { try validateHash(hash) }
    }

    public static func validateHash(_ hash: String) throws {
        guard hash.utf8.count == 64, hash.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else { throw DesktopStoreError.invalidHash }
    }
}
