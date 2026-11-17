import AppKit
import ApplicationServices
import CoreGraphics
import DesktopCore
import Foundation

/// AX is synchronous IPC. All calls run on this serial actor with a 50ms
/// per-message timeout and a 300ms read budget; callers bound queue admission.
actor DesktopAXWorker {
    static let shared = DesktopAXWorker()
    private var observer: AXObserver?
    private var registrations: [(AXUIElement, CFString)] = []
    private var ownerID: UUID?
    private var observedPID: Int32?
    private var observedFocus: UInt?
    private var observingText = false
    private var observingSelection = false
    private var observingBody = false
    private var observedProtection = false
    private var observedWindow = ""
    /// Chromium-based apps switch their accessibility tree off when they think
    /// no assistive client is around; see `wakeDormantWebContent`.
    private var wokenAt: [pid_t: TimeInterval] = [:]
    private var chromiumBundles: [String: Bool] = [:]

    private struct Budget {
        let deadline: TimeInterval
        init(seconds: TimeInterval = 0.3) { deadline = ProcessInfo.processInfo.systemUptime + seconds }
        var expired: Bool { ProcessInfo.processInfo.systemUptime >= deadline || Task.isCancelled }
        func check() throws { try Task.checkCancellation(); if expired { throw DesktopCaptureError.timedOut } }
    }

    func screenshotEvidence(pid: Int32, windowIdentity: UInt, frame: CGRect, includeFocus: Bool) -> DesktopAXEvidence? {
        let budget = Budget(seconds: 0.65)
        let observedAt = Date()
        guard AXIsProcessTrusted(), !frame.isEmpty else { return nil }
        let app = AXUIElementCreateApplication(pid)
        guard let root = element(app, kAXFocusedWindowAttribute, budget), CFHash(root) == windowIdentity,
              protectionAllowsEvidence(root, budget) else { return nil }
        var owner: pid_t = 0
        guard AXUIElementGetPid(root, &owner) == .success, owner == pid else { return nil }

        var queue: [(element: AXUIElement, parent: Int?, depth: Int, reportedVisible: Bool, reportedSelected: Bool)] = [(root, nil, 0, true, false)]
        var visited = Set<UInt>()
        var nodes: [DesktopAXEvidence.Node] = []
        var bytes = 64
        var partial = false
        let encoder = JSONEncoder()
        while !queue.isEmpty && !budget.expired && visited.count < DesktopAXEvidence.maximumNodes {
            let entry = queue.removeFirst()
            guard visited.insert(CFHash(entry.element)).inserted else { continue }
            var nodePID: pid_t = 0
            guard AXUIElementGetPid(entry.element, &nodePID) == .success, nodePID == pid else { continue }
            let node = entry.element
            guard let role = string(node, kAXRoleAttribute, maxBytes: 96, budget),
                  protectionAllowsEvidence(node, budget) else { continue }
            let rect = elementFrame(node, budget)
            let visibleRect = rect.map { $0.intersection(frame) }
            if let visibleRect, visibleRect.isEmpty { continue }
            let bounds = visibleRect.flatMap { Self.normalizedBounds($0, in: frame) }
            let visible = bounds != nil || entry.reportedVisible
            let readsContent = entry.depth > 0 && visible
            let subrole = string(node, kAXSubroleAttribute, maxBytes: 96, budget)
            let label = readsContent ? string(node, kAXTitleAttribute, maxBytes: 480, budget) : nil
            let detail = readsContent ? string(node, kAXDescriptionAttribute, maxBytes: 256, budget) : nil
            var value: String?
            if readsContent, !["AXTextField", "AXTextArea", "AXComboBox"].contains(role), boolean(node, "AXEditable", budget) != true, DesktopTextPolicy.permitsValue(role: role, subrole: subrole, protected: false),
               let range = range(node, kAXVisibleCharacterRangeAttribute, budget), range.length > 0 {
                var unsupported = false
                value = boundedRangeText(node, range: range, maxBytes: 1024, budget: budget,
                    limited: &partial, unsupported: &unsupported)
                partial = partial || unsupported
            }
            // Static UI labels and cells commonly offer only AXValue. Editable
            // fields and document-sized text areas never use this fallback.
            if readsContent, value == nil, ["AXStaticText", "AXCell", "AXHeading"].contains(role) {
                value = string(node, kAXValueAttribute, maxBytes: 1024, budget)
            }
            if readsContent, ["AXCheckBox", "AXRadioButton", "AXSlider", "AXProgressIndicator", "AXLevelIndicator"].contains(role),
               let number = attribute(node, kAXValueAttribute, budget) as? NSNumber {
                value = number.stringValue
            }
            let evidence = DesktopAXEvidence.Node(parent: entry.parent, role: role, subrole: subrole,
                label: label ?? detail, text: value == label || value == detail ? nil : value, bounds: bounds,
                selected: visible ? (entry.reportedSelected ? true : boolean(node, kAXSelectedAttribute, budget)) : nil,
                focused: visible && includeFocus ? boolean(node, kAXFocusedAttribute, budget) : nil,
                expanded: visible ? boolean(node, kAXExpandedAttribute, budget) : nil,
                enabled: visible ? boolean(node, kAXEnabledAttribute, budget) : nil,
                destination: readsContent && role == "AXLink" ? linkDestination(node, budget) : nil,
                detail: label != nil && detail != label ? detail : nil,
                placeholder: readsContent && ["AXTextField", "AXTextArea", "AXComboBox"].contains(role)
                    ? string(node, kAXPlaceholderValueAttribute, maxBytes: 160, budget) : nil,
                identifier: visible ? string(node, kAXIdentifierAttribute, maxBytes: 128, budget) : nil,
                reportedVisible: entry.reportedVisible ? true : nil)
            guard let encoded = try? encoder.encode(evidence),
                  bytes + encoded.count + 1 <= DesktopAXEvidence.maximumEncodedBytes else { partial = true; break }
            let index = nodes.count
            nodes.append(evidence); bytes += encoded.count + 1
            let capacity = max(0, DesktopAXEvidence.maximumNodes - visited.count - queue.count)
            let children = evidenceChildren(node, role: role, limit: entry.depth < 24 ? capacity : 0, budget)
            partial = partial || children.partial
            let selected = children.elements.isEmpty ? [] : selectedEvidenceChildren(node, role: role, budget)
            queue.append(contentsOf: children.elements.map { ($0, index, entry.depth + 1, children.visible, selected.contains(CFHash($0))) })
        }
        guard !Task.isCancelled else { return nil }
        return DesktopAXEvidence(nodes: nodes, partial: partial || budget.expired || !queue.isEmpty, observedAt: observedAt)
    }

    private func selectedEvidenceChildren(_ node: AXUIElement, role: String, _ budget: Budget) -> Set<UInt> {
        guard ["AXTable", "AXOutline", "AXList", "AXTabGroup", "AXBrowser"].contains(role) else { return [] }
        let key = ["AXTable", "AXOutline"].contains(role) ? kAXSelectedRowsAttribute : kAXSelectedChildrenAttribute
        guard !budget.expired else { return [] }
        var count = 0
        var values: CFArray?
        guard AXUIElementGetAttributeValueCount(node, key as CFString, &count) == .success, count > 0,
              AXUIElementCopyAttributeValues(node, key as CFString, 0, min(count, 32), &values) == .success,
              let values else { return [] }
        return Set((values as [AnyObject]).compactMap { value in
            CFGetTypeID(value) == AXUIElementGetTypeID() ? CFHash(value) : nil
        })
    }

    private func evidenceChildren(_ node: AXUIElement, role: String, limit: Int, _ budget: Budget)
        -> (elements: [AXUIElement], visible: Bool, partial: Bool) {
        let names = ["AXTable", "AXOutline"].contains(role)
            ? [kAXVisibleRowsAttribute, kAXVisibleChildrenAttribute, kAXChildrenAttribute, kAXRowsAttribute]
            : [kAXVisibleChildrenAttribute, kAXChildrenAttribute]
        var failed = false
        for name in names {
            guard !budget.expired else { return ([], false, true) }
            var count = 0
            let status = AXUIElementGetAttributeValueCount(node, name as CFString, &count)
            if status == .attributeUnsupported || status == .noValue { continue }
            guard status == .success else { failed = true; continue }
            if count == 0 { continue }
            guard limit > 0 else { return ([], false, true) }
            var values: CFArray?
            guard AXUIElementCopyAttributeValues(node, name as CFString, 0, min(count, limit), &values) == .success,
                  let values else { failed = true; continue }
            let elements = (values as [AnyObject]).compactMap { value -> AXUIElement? in
                guard CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
                return unsafeDowncast(value, to: AXUIElement.self)
            }
            return (elements, name == kAXVisibleRowsAttribute || name == kAXVisibleChildrenAttribute, failed || count > elements.count)
        }
        return ([], false, failed)
    }

    private func protectionAllowsEvidence(_ node: AXUIElement, _ budget: Budget) -> Bool {
        guard !budget.expired else { return false }
        AXUIElementSetMessagingTimeout(node, 0.05)
        for name in [kAXSubroleAttribute, NSAccessibility.Attribute.containsProtectedContent.rawValue] {
            var value: CFTypeRef?
            let status = AXUIElementCopyAttributeValue(node, name as CFString, &value)
            if status == .noValue || status == .attributeUnsupported { continue }
            guard status == .success, !budget.expired else { return false }
            if value as? String == kAXSecureTextFieldSubrole || (value as? NSNumber)?.boolValue == true { return false }
        }
        return true
    }

    private func boolean(_ node: AXUIElement, _ name: String, _ budget: Budget) -> Bool? {
        (attribute(node, name, budget) as? NSNumber)?.boolValue
    }

    private func elementFrame(_ node: AXUIElement, _ budget: Budget) -> CGRect? {
        guard let p = attribute(node, kAXPositionAttribute, budget), let s = attribute(node, kAXSizeAttribute, budget),
              CFGetTypeID(p) == AXValueGetTypeID(), CFGetTypeID(s) == AXValueGetTypeID() else { return nil }
        var point = CGPoint.zero, size = CGSize.zero
        guard AXValueGetValue(unsafeDowncast(p, to: AXValue.self), .cgPoint, &point),
              AXValueGetValue(unsafeDowncast(s, to: AXValue.self), .cgSize, &size),
              point.x.isFinite, point.y.isFinite, size.width.isFinite, size.height.isFinite,
              size.width > 0, size.height > 0 else { return nil }
        return CGRect(origin: point, size: size)
    }

    private static func normalizedBounds(_ rect: CGRect, in frame: CGRect) -> [Int]? {
        let values = [(rect.minX - frame.minX) / frame.width, (rect.minY - frame.minY) / frame.height,
                      (rect.maxX - frame.minX) / frame.width, (rect.maxY - frame.minY) / frame.height]
        guard values.allSatisfy(\.isFinite) else { return nil }
        let bounds = values.map { Int((min(1, max(0, $0)) * 1000).rounded()) }
        return bounds[0] < bounds[2] && bounds[1] < bounds[3] ? bounds : nil
    }

    private func linkDestination(_ node: AXUIElement, _ budget: Budget) -> String? {
        let raw = attribute(node, kAXURLAttribute, budget)
        let value = (raw as? URL)?.absoluteString ?? raw as? String
        guard let value, var url = URLComponents(string: value),
              let scheme = url.scheme?.lowercased(), ["http", "https"].contains(scheme), url.host != nil else { return nil }
        url.user = nil; url.password = nil; url.query = nil; url.fragment = nil
        guard let safe = url.string, safe.utf8.count <= 512 else { return nil }
        return safe
    }

    func read(application: DesktopApplication, includeText: Bool = false, minimalMetadata: Bool = false, contentOptions: DesktopContentOptions = DesktopContentOptions(), wakeDormantWeb: Bool = false) throws -> DesktopFocusReader.Focus {
        let budget = Budget()
        guard AXIsProcessTrusted() else { throw DesktopCaptureError.accessibilityPermissionDenied }
        let app = AXUIElementCreateApplication(application.pid)
        guard let window = element(app, kAXFocusedWindowAttribute, budget) else { throw DesktopCaptureError.noFocusedWindow }
        var windowPID: pid_t = 0
        guard AXUIElementGetPid(window, &windowPID) == .success, windowPID == application.pid else { throw DesktopCaptureError.focusChanged }
        guard (attribute(window, kAXMinimizedAttribute, budget) as? NSNumber)?.boolValue != true else { throw DesktopCaptureError.noFocusedWindow }
        guard let positionValue = attribute(window, kAXPositionAttribute, budget), let sizeValue = attribute(window, kAXSizeAttribute, budget),
              CFGetTypeID(positionValue) == AXValueGetTypeID(), CFGetTypeID(sizeValue) == AXValueGetTypeID() else { throw DesktopCaptureError.noFocusedWindow }
        var point = CGPoint.zero, size = CGSize.zero
        guard AXValueGetValue(unsafeDowncast(positionValue, to: AXValue.self), .cgPoint, &point),
              AXValueGetValue(unsafeDowncast(sizeValue, to: AXValue.self), .cgSize, &size), size.width > 0, size.height > 0 else { throw DesktopCaptureError.noFocusedWindow }
        let appFocus = focusedControl(app, budget: budget)
        if wakeDormantWeb, appFocus.element == nil { wakeDormantWebContent(app, application: application) }
        var focused = appFocus.element
        var controlFocusAbsent = false
        if minimalMetadata, focused == nil {
            let systemFocus = focusedControl(AXUIElementCreateSystemWide(), budget: budget)
            controlFocusAbsent = appFocus.absent && systemFocus.absent
            if let candidate = systemFocus.element {
                var pid: pid_t = 0
                if AXUIElementGetPid(candidate, &pid) == .success, pid == windowPID,
                   CFEqual(candidate, window) || element(candidate, kAXWindowAttribute, budget).map({ CFEqual($0, window) }) == true {
                    focused = candidate
                }
            }
        }
        let role = focused.flatMap { string($0, kAXRoleAttribute, maxBytes: 128, budget) }
        // Check protection before requesting labels, ranges, values or titles.
        // Unsupported protection attributes are never interpreted as "protected".
        let protected = isProtected(focused, budget: budget) || isProtected(window, budget: budget)
        try budget.check() // Never treat an unfinished protection check as safe.
        let title = protected ? "" : (string(window, kAXTitleAttribute, maxBytes: 4096, budget) ?? "")
        let composing = !protected ? focused.flatMap { markedTextState($0, budget: budget) } : nil
        let readsText = includeText && (contentOptions.selectedTextEnabled || contentOptions.readableTextEnabled || contentOptions.inputValueBundleIDs?.contains(application.bundleID) == true)
        var context = DesktopContextSnapshot(focusedRole: contentOptions.focusEnabled ? role : nil, textStatus: readsText ? "no_text" : "disabled", isProtected: protected, isComposing: composing)
        if protected {
            context.textStatus = "protected"
        } else if composing == true {
            // These public attributes are best-effort across AX providers. An
            // unsupported response remains unknown, never a false guarantee.
            context.textStatus = "composing"
        } else {
            if contentOptions.focusEnabled, !minimalMetadata, let focused { context.focusedLabel = string(focused, kAXTitleAttribute, maxBytes: 512, budget) ?? string(focused, kAXDescriptionAttribute, maxBytes: 512, budget) }
            if readsText {
                let inputAllowed = contentOptions.inputValueBundleIDs?.contains(application.bundleID) == true
                let editable = focused.map { boolean($0, "AXEditable", budget) == true || ["AXTextField", "AXTextArea", "AXComboBox"].contains(role ?? "") } ?? false
                var evidence = DesktopRecordEvidence(object: DesktopRecordEvidence.object(
                    document: string(window, kAXDocumentAttribute, maxBytes: 4096, budget),
                    url: objectURL(window, budget: budget), app: application.bundleID, title: title, window: String(CFHash(window))), inputStatus: inputAllowed ? "unavailable" : "disabled")
                if inputAllowed, editable, let focused {
                    evidence.field = "\(application.pid):\(CFHash(window)):\(role ?? "unknown"):\(CFHash(focused))"
                    // Request a bounded range, including an explicitly observed empty value.
                    if let count = attribute(focused, kAXNumberOfCharactersAttribute, budget) as? NSNumber {
                        var clipped = false, failed = false
                        var complete = "", offset = 0
                        if count.intValue < 0 || count.intValue > 65536 { clipped = true }
                        while offset < count.intValue, !clipped, !failed, !budget.expired {
                            let length = min(4096, count.intValue - offset)
                            guard let part = boundedRangeText(focused, range: CFRange(location: offset, length: length), maxBytes: 16384, budget: budget, limited: &clipped, unsupported: &failed) else { failed = true; break }
                            complete += part; offset += length
                            if complete.utf8.count > 65536 { clipped = true }
                        }
                        if offset == count.intValue && !clipped && !failed {
                            evidence.input_value = complete; evidence.input_status = "captured"
                        }
                    }
                    if evidence.input_value == nil, role == "AXTextField",
                       let value = completeValue(focused, budget: budget) {
                        evidence.input_value = value; evidence.input_status = "captured"
                    }
                }
                context.recordEvidence = evidence
                var limited = false, unsupported = false
                // Never read ordinary one-line input values, including selections.
                if !editable, contentOptions.selectedTextEnabled, let focused, DesktopTextPolicy.permitsValue(role: role ?? "", subrole: nil, protected: protected) {
                    if let range = range(focused, kAXSelectedTextRangeAttribute, budget), range.length > 0 {
                        context.selectedText = boundedRangeText(focused, range: range, maxBytes: 2048, budget: budget, limited: &limited, unsupported: &unsupported)
                    }
                }
                if contentOptions.readableTextEnabled {
                    var body: [String] = []
                    var bytes = 0, visited = 0
                    var queue: [(AXUIElement, Int)] = [(window, 0)]
                    // Focused text is preferred and avoids scanning a document tree
                    // when its public visible-character range is available.
                    if !editable, let focused, DesktopTextPolicy.permitsValue(role: role ?? "", subrole: nil, protected: protected) {
                        if let text = readableText(focused, maxBytes: 4096, budget: budget, limited: &limited, unsupported: &unsupported), !text.isEmpty {
                            body.append(text); bytes = text.utf8.count; queue.removeAll()
                        }
                    }
                    while body.isEmpty, !queue.isEmpty, visited < 24, !budget.expired {
                        let (node, depth) = queue.removeFirst(); visited += 1
                        let nodeRole = string(node, kAXRoleAttribute, maxBytes: 128, budget)
                        if isProtected(node, budget: budget) || (focused.map { CFEqual(node, $0) } == true && editable) || boolean(node, "AXEditable", budget) == true { continue }
                        if DesktopTextPolicy.permitsValue(role: nodeRole ?? "", subrole: nil, protected: false) {
                            if let text = readableText(node, maxBytes: 4096 - bytes, budget: budget, limited: &limited, unsupported: &unsupported), !text.isEmpty {
                                body.append(text); bytes += text.utf8.count
                            }
                        }
                        if depth < 4, bytes < 4096 {
                            queue.append(contentsOf: children(node, limit: max(0, 24 - visited - queue.count), budget: budget).map { ($0, depth + 1) })
                        }
                    }
                    var deep = false
                    if body.isEmpty, !budget.expired {
                        // Web content (Chromium/Electron/WebKit) keeps its text far below
                        // the window: Notion's page sits 8 levels down, its text to ~29.
                        let found = deepText(window, maxBytes: 4096, budget: budget, limited: &limited, unsupported: &unsupported, excluding: editable ? focused : nil)
                        if !found.isEmpty { body = found; deep = true }
                    }
                    context.readableText = body.isEmpty ? nil : DesktopTextPolicy.prefix(body.joined(separator: "\n"), maxBytes: 4096)
                    if budget.expired || (!deep && (visited >= 24 || !queue.isEmpty)) || unsupported { limited = true }
                }
                context.recordEvidence?.reading = (context.readableText ?? "").split(separator: "\n").map { .init(text: String($0)) }
                // Only the controlled source defines outgoing identity. A generic
                // AX label, a cleared composer or a click is never send proof.
                if application.bundleID == DesktopSourceConfirmation.fixtureBundleID, contentOptions.readableTextEnabled {
                    let messages = fixtureMessages(window, budget: budget)
                    context.recordEvidence?.reading = messages
                    context.readableText = messages.map(\.text).joined(separator: "\n")
                }
                let hasText = context.selectedText != nil || context.readableText != nil || context.recordEvidence?.input_value != nil
                context.textStatus = hasText ? (limited ? "partial" : "captured") : (budget.expired ? "timeout" : (unsupported ? "unavailable" : "no_text"))
            }
        }
        try Task.checkCancellation()
        return DesktopFocusReader.Focus(application: application, title: title, frame: CGRect(origin: point, size: size), context: context, focusIdentity: focused.map(CFHash) ?? 0, windowIdentity: CFHash(window), controlFocusAbsent: controlFocusAbsent)
    }

    private func completeValue(_ node: AXUIElement, budget: Budget) -> String? {
        guard let value = attribute(node, kAXValueAttribute, budget), CFGetTypeID(value) == CFStringGetTypeID() else { return nil }
        let raw = unsafeDowncast(value, to: CFString.self)
        let length = CFStringGetLength(raw)
        guard length <= 65536 else { return nil }
        var units = [UniChar](repeating: 0, count: length)
        if length > 0 { CFStringGetCharacters(raw, CFRange(location: 0, length: length), &units) }
        let result = String(decoding: units, as: UTF16.self)
        return result.utf8.count <= 65536 ? result : nil
    }

    private func fixtureMessages(_ window: AXUIElement, budget: Budget) -> [DesktopRecordEvidence.Reading] {
        var queue = [window], result: [DesktopRecordEvidence.Reading] = []
        var visited = Set<UInt>()
        while !queue.isEmpty, visited.count < 128, !budget.expired {
            let node = queue.removeFirst()
            guard visited.insert(CFHash(node)).inserted, protectionAllowsEvidence(node, budget) else { continue }
            if string(node, kAXRoleAttribute, maxBytes: 96, budget) == "AXStaticText",
               let id = string(node, kAXIdentifierAttribute, maxBytes: 128, budget),
               (id.hasPrefix("candid.outgoing.") || id.hasPrefix("candid.incoming.")),
               let text = completeValue(node, budget: budget) {
                result.append(.init(text: text, occurrence: id))
            }
            queue.append(contentsOf: children(node, limit: max(0, 128 - visited.count - queue.count), budget: budget))
        }
        return result
    }

    func observeAndRead(application: DesktopApplication, ownerID: UUID, includeText: Bool = false, observeText: Bool? = nil, minimalMetadata: Bool = false, contentOptions: DesktopContentOptions = DesktopContentOptions()) throws -> DesktopFocusReader.Focus {
        try Task.checkCancellation()
        let result = try read(application: application, includeText: includeText, minimalMetadata: minimalMetadata, contentOptions: contentOptions)
        let inputNotifications = contentOptions.inputValueBundleIDs?.contains(application.bundleID) == true
        let textNotifications = (observeText ?? includeText) && (contentOptions.readableTextEnabled || contentOptions.selectedTextEnabled || inputNotifications)
        let selectionNotifications = textNotifications && contentOptions.selectedTextEnabled
        let bodyNotifications = textNotifications && (contentOptions.readableTextEnabled || inputNotifications)
        let app = AXUIElementCreateApplication(application.pid)
        let budget = Budget()
        if self.ownerID != ownerID || observedPID != application.pid {
            removeObserver()
            var created: AXObserver?
            if AXObserverCreate(application.pid, { _, _, notification, _ in
                // This observer source is attached exclusively to the main loop.
                let name = notification as String
                let trigger: String
                if name == kAXFocusedUIElementChangedNotification as String { trigger = "focus" }
                else if name == kAXSelectedTextChangedNotification as String { trigger = "selection" }
                else if name == kAXValueChangedNotification as String || name == "AXLayoutChanged" { trigger = "content" }
                else { trigger = "window" }
                MainActor.assumeIsolated { DesktopSystemMonitor.receivedAXSignal(trigger: trigger) }
            }, &created) == .success, let created {
                observer = created; self.ownerID = ownerID; observedPID = application.pid
                add(app, kAXFocusedWindowChangedNotification)
                add(app, kAXFocusedUIElementChangedNotification)
                CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(created), .commonModes)
            }
        }
        // Rebind notifications when focus changes. Old element registrations are
        // removed, avoiding retention of closed windows and text views.
        let windowToken = "\(result.title)|\(result.frame)"
        if observedFocus != result.focusIdentity || observingText != textNotifications || observingSelection != selectionNotifications || observingBody != bodyNotifications || observedProtection != (result.context.isProtected == true) || observedWindow != windowToken || registrations.count <= 2 {
            removeElementRegistrations()
            if let window = element(app, kAXFocusedWindowAttribute, budget) {
                add(window, kAXTitleChangedNotification)
                if bodyNotifications, result.context.isProtected != true {
                    add(window, kAXValueChangedNotification); add(window, "AXLayoutChanged")
                }
            }
            if textNotifications, result.context.isProtected != true, let focused = element(app, kAXFocusedUIElementAttribute, budget) {
                // Role is still needed for safe subscriptions when the user
                // disables recording focus details; it is not emitted then.
                let role = result.context.focusedRole ?? string(focused, kAXRoleAttribute, maxBytes: 128, budget)
                if inputNotifications || DesktopTextPolicy.permitsValue(role: role ?? "", subrole: nil, protected: result.context.isProtected == true) {
                    if selectionNotifications { add(focused, kAXSelectedTextChangedNotification) }
                    if bodyNotifications { add(focused, kAXValueChangedNotification) }
                }
            }
            observedFocus = result.focusIdentity; observingText = textNotifications
            observingSelection = selectionNotifications; observingBody = bodyNotifications
            observedProtection = result.context.isProtected == true; observedWindow = windowToken
        }
        try Task.checkCancellation()
        return result
    }

    func clear(ownerID: UUID) { if self.ownerID == ownerID { removeObserver() } }

    private func add(_ element: AXUIElement, _ name: String) {
        guard let observer else { return }
        AXUIElementSetMessagingTimeout(element, 0.05)
        if AXObserverAddNotification(observer, element, name as CFString, nil) == .success { registrations.append((element, name as CFString)) }
    }
    private func removeElementRegistrations() {
        guard let observer else { return }
        // The two app-level notifications survive focus changes.
        let appNames = [kAXFocusedWindowChangedNotification as String, kAXFocusedUIElementChangedNotification as String]
        registrations.removeAll { element, name in
            guard !appNames.contains(name as String) else { return false }
            AXUIElementSetMessagingTimeout(element, 0.05)
            _ = AXObserverRemoveNotification(observer, element, name)
            return true
        }
    }
    private func removeObserver() {
        if let observer {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(observer), .commonModes)
            for (element, name) in registrations {
                AXUIElementSetMessagingTimeout(element, 0.05)
                _ = AXObserverRemoveNotification(observer, element, name)
            }
        }
        observer = nil; registrations = []; ownerID = nil; observedPID = nil; observedFocus = nil; observingText = false; observedProtection = false; observedWindow = ""
    }

    private func attribute(_ element: AXUIElement, _ name: String, _ budget: Budget) -> CFTypeRef? {
        guard !budget.expired else { return nil }
        AXUIElementSetMessagingTimeout(element, 0.05)
        var value: CFTypeRef?
        return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
    }
    private func element(_ parent: AXUIElement, _ name: String, _ budget: Budget) -> AXUIElement? {
        guard let value = attribute(parent, name, budget), CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        return unsafeDowncast(value, to: AXUIElement.self)
    }
    private func focusedControl(_ element: AXUIElement, budget: Budget) -> (element: AXUIElement?, absent: Bool) {
        guard !budget.expired else { return (nil, false) }
        AXUIElementSetMessagingTimeout(element, 0.05)
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(element, kAXFocusedUIElementAttribute as CFString, &value)
        if result == .noValue || result == .attributeUnsupported { return (nil, true) }
        guard result == .success, let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return (nil, false) }
        return (unsafeDowncast(value, to: AXUIElement.self), false)
    }
    private func string(_ element: AXUIElement, _ name: String, maxBytes: Int, _ budget: Budget) -> String? {
        guard let value = attribute(element, name, budget), CFGetTypeID(value) == CFStringGetTypeID() else { return nil }
        return boundedString(unsafeDowncast(value, to: CFString.self), maxBytes: maxBytes)
    }
    private func objectURL(_ root: AXUIElement, budget: Budget) -> String? {
        var queue = [root], visited = 0
        while !queue.isEmpty, visited < 32, !budget.expired {
            let node = queue.removeFirst(); visited += 1
            if string(node, kAXRoleAttribute, maxBytes: 96, budget) == "AXWebArea", let value = attribute(node, "AXURL", budget) {
                if let url = value as? URL, ["http", "https", "file"].contains(url.scheme ?? "") { return url.absoluteString }
                if let raw = value as? String, let url = URL(string: raw), ["http", "https", "file"].contains(url.scheme ?? "") { return raw }
            }
            queue.append(contentsOf: children(node, limit: max(0, 32 - visited - queue.count), budget: budget))
        }
        return nil
    }

    private func boundedString(_ value: CFString, maxBytes: Int) -> String? {
        let length = min(CFStringGetLength(value), maxBytes)
        guard length > 0 else { return "" }
        // Copy only a bounded prefix even if an AX provider returns a huge label.
        var units = [UniChar](repeating: 0, count: length)
        CFStringGetCharacters(value, CFRange(location: 0, length: length), &units)
        return DesktopTextPolicy.prefix(String(decoding: units, as: UTF16.self), maxBytes: maxBytes)
    }
    private func isProtected(_ element: AXUIElement?, budget: Budget) -> Bool {
        guard let element else { return false }
        if string(element, kAXSubroleAttribute, maxBytes: 128, budget) == kAXSecureTextFieldSubrole as String { return true }
        return (attribute(element, NSAccessibility.Attribute.containsProtectedContent.rawValue, budget) as? NSNumber)?.boolValue == true
    }
    private func markedTextState(_ element: AXUIElement, budget: Budget) -> Bool? {
        if let value = range(element, "AXTextInputMarkedRange" /* NSAccessibility.Attribute.textInputMarkedRangeAttribute, spelled out for older SDKs */, budget) {
            return value.length > 0
        }
        // Public AXWebConstants.h input-method marker range. Its opaque marker
        // is used only as a presence signal; no provisional text is copied.
        if let marker = attribute(element, "AXTextInputMarkedTextMarkerRange", budget), CFGetTypeID(marker) != CFNullGetTypeID() { return true }
        return nil
    }
    private func range(_ element: AXUIElement, _ name: String, _ budget: Budget) -> CFRange? {
        guard let value = attribute(element, name, budget), CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
        var result = CFRange()
        guard AXValueGetValue(unsafeDowncast(value, to: AXValue.self), .cfRange, &result), result.location >= 0, result.length >= 0 else { return nil }
        return result
    }
    private func boundedRangeText(_ element: AXUIElement, range: CFRange, maxBytes: Int, budget: Budget, limited: inout Bool, unsupported: inout Bool) -> String? {
        guard !budget.expired, maxBytes > 0 else { return nil }
        // UTF-16 length capped at bytes/4 guarantees the UTF-8 byte budget even
        // when a provider ignores its own visible range and returns emoji/CJK.
        var bounded = CFRange(location: range.location, length: DesktopTextPolicy.boundedUTF16Length(requested: range.length, maxBytes: maxBytes))
        if bounded.length < range.length { limited = true }
        guard bounded.length > 0, let parameter = AXValueCreate(.cfRange, &bounded) else { return nil }
        AXUIElementSetMessagingTimeout(element, 0.05)
        var value: CFTypeRef?
        guard AXUIElementCopyParameterizedAttributeValue(element, kAXStringForRangeParameterizedAttribute as CFString, parameter, &value) == .success,
              let value, CFGetTypeID(value) == CFStringGetTypeID() else { unsupported = true; return nil }
        return boundedString(unsafeDowncast(value, to: CFString.self), maxBytes: maxBytes)
    }
    private func readableText(_ element: AXUIElement, maxBytes: Int, budget: Budget, limited: inout Bool, unsupported: inout Bool) -> String? {
        if let visible = range(element, kAXVisibleCharacterRangeAttribute, budget), visible.length > 0 {
            return boundedRangeText(element, range: visible, maxBytes: maxBytes, budget: budget, limited: &limited, unsupported: &unsupported)
        }
        if let count = attribute(element, kAXNumberOfCharactersAttribute, budget) as? NSNumber {
            guard count.intValue > 0 else { return nil }
            return boundedRangeText(element, range: CFRange(location: 0, length: count.intValue), maxBytes: maxBytes, budget: budget, limited: &limited, unsupported: &unsupported)
        }
        // Do not request AXValue without a bounded parameterized text API. Some
        // providers expose an entire multi-megabyte document through that value.
        unsupported = true
        return nil
    }
    /// An allowed Chromium/Electron app that reports no focused control has
    /// usually turned its accessibility tree off (Codex: 12 elements instead of
    /// ~1300). Asking for the enhanced interface makes Chromium rebuild it; the
    /// request itself may return an error and still work. Sent at most every two
    /// minutes per process, only for apps the user allowed, never for native apps
    /// (where this attribute slows window animations).
    private func wakeDormantWebContent(_ app: AXUIElement, application: DesktopApplication) {
        let now = ProcessInfo.processInfo.systemUptime
        if let last = wokenAt[application.pid], now - last < 120 { return }
        guard let path = application.bundlePath else { return }
        let chromium = chromiumBundles[path] ?? Self.isChromiumBased(bundle: URL(fileURLWithPath: path, isDirectory: true))
        chromiumBundles[path] = chromium
        guard chromium else { return }
        wokenAt[application.pid] = now
        AXUIElementSetMessagingTimeout(app, 0.05)
        _ = AXUIElementSetAttributeValue(app, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
        _ = AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, kCFBooleanTrue)
    }

    /// Chromium and Electron apps ship renderer/GPU helper apps or the Electron
    /// framework inside Contents/Frameworks (Chrome, Notion, ChatGPT/Codex).
    nonisolated static func isChromiumBased(bundle: URL) -> Bool {
        func search(_ directory: URL, depth: Int) -> Bool {
            guard let items = try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]) else { return false }
            for item in items.prefix(200) {
                let name = item.lastPathComponent
                if name == "Electron Framework.framework" { return true }
                if item.pathExtension == "app" {
                    if name.contains("(Renderer)") || name.contains("(GPU)") { return true }
                    continue   // never look inside other bundled apps
                }
                let descends = item.pathExtension == "framework" || name == "Versions" || name == "Helpers"
                    || name == "Current" || name.first?.isNumber == true
                if descends, depth < 4, search(item, depth: depth + 1) { return true }
            }
            return false
        }
        return search(bundle.appendingPathComponent("Contents/Frameworks", isDirectory: true), depth: 0)
    }

    /// Roles whose subtrees are navigation or controls, not readable content.
    static let deepTextSkipped: Set<String> = [
        "AXOutline", "AXTabGroup", "AXToolbar", "AXMenuBar", "AXMenu", "AXMenuButton", "AXButton", "AXPopUpButton",
        "AXRadioButton", "AXCheckBox", "AXImage", "AXScrollBar", "AXTextField", "AXComboBox", "AXSearchField", "AXSlider",
    ]
    static let deepTextNodeLimit = 1500

    /// Document-order walk for pages whose text is deeper than the shallow pass
    /// reaches. Reads only static text and text areas through the same bounded
    /// range API; protection is checked on every node it reads.
    private func deepText(_ root: AXUIElement, maxBytes: Int, budget: Budget, limited: inout Bool, unsupported: inout Bool,
                          excluding: AXUIElement? = nil, trace: ((String, Int, Int) -> Void)? = nil) -> [String] {
        var texts: [String] = []
        var bytes = 0, visited = 0
        var stack: [(AXUIElement, Int)] = [(root, 0)]
        while let (node, depth) = stack.popLast(), visited < Self.deepTextNodeLimit, bytes < maxBytes, !budget.expired {
            visited += 1
            if excluding.map({ CFEqual(node, $0) }) == true || boolean(node, "AXEditable", budget) == true { continue }
            guard let role = string(node, kAXRoleAttribute, maxBytes: 96, budget) else { continue }
            if depth > 0, Self.deepTextSkipped.contains(role) { continue }
            if role == kAXStaticTextRole as String || role == kAXTextAreaRole as String {
                guard !isProtected(node, budget: budget) else { continue }
                let before = unsupported
                let isLeaf = role == kAXStaticTextRole as String
                var text = readableText(node, maxBytes: maxBytes - bytes, budget: budget, limited: &limited, unsupported: &unsupported)
                if text == nil, isLeaf {
                    // Chromium/WebKit text runs report zero characters and keep
                    // their text in AXValue. Allowed for leaf runs only (one DOM
                    // text node each), never text areas, and cut to the budget.
                    if let value = attribute(node, kAXValueAttribute, budget) as? String {
                        if value.utf8.count > maxBytes - bytes { limited = true }
                        text = DesktopTextPolicy.prefix(value, maxBytes: maxBytes - bytes)
                    }
                }
                if let text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty {
                    texts.append(text); bytes += text.utf8.count + 1
                    trace?(role, depth, text.utf8.count)
                    continue   // read whole; its children would repeat it
                } else if unsupported != before, !isLeaf {
                    // Chromium's editable page root refuses a range but its
                    // blocks are readable children, walked next; not a loss.
                    unsupported = before
                }
                if isLeaf { continue }
            }
            guard depth < 48 else { continue }
            var values: CFTypeRef?
            AXUIElementSetMessagingTimeout(node, 0.05)
            guard AXUIElementCopyAttributeValue(node, kAXChildrenAttribute as CFString, &values) == .success,
                  let children = values as? [AnyObject] else { continue }
            let elements = children.compactMap { value -> AXUIElement? in
                guard CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
                return unsafeDowncast(value, to: AXUIElement.self)
            }
            stack.append(contentsOf: elements.prefix(200).reversed().map { ($0, depth + 1) })
        }
        if !stack.isEmpty || bytes >= maxBytes || budget.expired { limited = true }
        return texts
    }

    private func children(_ parent: AXUIElement, limit: Int, budget: Budget) -> [AXUIElement] {
        guard limit > 0, !budget.expired else { return [] }
        AXUIElementSetMessagingTimeout(parent, 0.05)
        var values: CFArray?
        guard AXUIElementCopyAttributeValues(parent, kAXChildrenAttribute as CFString, 0, min(limit, 24), &values) == .success, let values else { return [] }
        return (values as [AnyObject]).compactMap { value in
            guard CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
            return unsafeDowncast(value, to: AXUIElement.self)
        }
    }
}

// MARK: - Diagnostics (`--ax-probe BUNDLE_ID OUT`)

extension DesktopAXWorker {
    /// Shape of an app's AX tree and what the reader gets from it. Records
    /// roles, counts, depths and lengths only, never text content.
    func probe(application: DesktopApplication) async -> [String] {
        var lines: [String] = ["app \(application.bundleID) pid \(application.pid) trusted \(AXIsProcessTrusted())"]
        let app = AXUIElementCreateApplication(application.pid)
        AXUIElementSetMessagingTimeout(app, 0.5)
        // Read-only: the probe never sets attributes on the inspected app.
        func flag(_ name: String) -> String {
            var value: CFTypeRef?
            let result = AXUIElementCopyAttributeValue(app, name as CFString, &value)
            return result == .success ? "\((value as? NSNumber)?.boolValue ?? false)" : "err \(result.rawValue)"
        }
        func readSummary(_ label: String) -> String {
            do {
                let focus = try read(application: application, includeText: true)
                return "\(label): status \(focus.context.textStatus ?? "nil") readable \(focus.context.readableText?.utf8.count ?? 0)B selected \(focus.context.selectedText?.utf8.count ?? 0)B role \(focus.context.focusedRole ?? "nil")"
            } catch { return "\(label): error \(error)" }
        }
        lines.append("AXManualAccessibility \(flag("AXManualAccessibility")) AXEnhancedUserInterface \(flag("AXEnhancedUserInterface"))")
        lines.append(readSummary("read before"))
        do {
            let b = Budget(seconds: 2)
            var windows: CFTypeRef?
            AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &windows)
            let focusedWindow = element(app, kAXFocusedWindowAttribute, b)
            for (i, w) in ((windows as? [AXUIElement]) ?? []).enumerated() {
                var size = CGSize.zero
                if let v = attribute(w, kAXSizeAttribute, b), CFGetTypeID(v) == AXValueGetTypeID() { AXValueGetValue(unsafeDowncast(v, to: AXValue.self), .cgSize, &size) }
                let kids = children(w, limit: 24, budget: b).count
                lines.append("window \(i): \(Int(size.width))x\(Int(size.height)) role \(string(w, kAXSubroleAttribute, maxBytes: 64, b) ?? "-") focused \(focusedWindow.map { CFEqual($0, w) } ?? false) minimized \(boolean(w, kAXMinimizedAttribute, b) ?? false) children \(kids) titleBytes \(string(w, kAXTitleAttribute, maxBytes: 512, b)?.utf8.count ?? 0)")
            }
            lines.append("app focused element role: \(element(app, kAXFocusedUIElementAttribute, b).flatMap { string($0, kAXRoleAttribute, maxBytes: 64, b) } ?? "none")")
        }

        let budget = Budget(seconds: 8)
        guard let window = element(app, kAXFocusedWindowAttribute, budget) else { lines.append("no focused window"); return lines }
        do {
            let started = ProcessInfo.processInfo.systemUptime
            var limited = false, unsupported = false, trace: [String] = []
            let texts = deepText(window, maxBytes: 4096, budget: Budget(), limited: &limited, unsupported: &unsupported) { trace.append("\($0.dropFirst(2))@\($1):\($2)B") }
            let ms = Int((ProcessInfo.processInfo.systemUptime - started) * 1000)
            lines.append("deep: \(texts.count) texts \(texts.reduce(0) { $0 + $1.utf8.count })B in \(ms)ms limited \(limited) unsupported \(unsupported)")
            lines.append("deep order: " + trace.prefix(40).joined(separator: " "))
        }
        var queue: [(AXUIElement, Int)] = [(window, 0)]
        var roles: [String: Int] = [:]
        var visited = 0, maxDepth = 0
        var webAreaDepth: Int?, webAreaParams: [String] = []
        var staticText = 0, withCount = 0, withVisible = 0, withValue = 0, valueBytes = 0, textAreas = 0
        // Static text bytes by the nearest ancestor role the deep pass skips ("-" = reachable).
        var hiddenBy: [String: (count: Int, bytes: Int)] = [:]
        var skippedAncestor: [UInt: String] = [:]
        while !queue.isEmpty, visited < 5000, !budget.expired {
            let (peek, _) = queue[0]
            let inherited = skippedAncestor[CFHash(peek)] ?? "-"
            if let role = string(peek, kAXRoleAttribute, maxBytes: 96, budget), role == kAXStaticTextRole as String {
                let length = (attribute(peek, kAXNumberOfCharactersAttribute, budget) as? NSNumber)?.intValue ?? 0
                hiddenBy[inherited, default: (0, 0)].count += 1; hiddenBy[inherited, default: (0, 0)].bytes += length
            } else if let role = string(peek, kAXRoleAttribute, maxBytes: 96, budget) {
                let mark = inherited == "-" && Self.deepTextSkipped.contains(role) ? role : inherited
                var values: CFTypeRef?
                if AXUIElementCopyAttributeValue(peek, kAXChildrenAttribute as CFString, &values) == .success, let children = values as? [AXUIElement] {
                    for child in children { skippedAncestor[CFHash(child)] = mark }
                }
            }
            let (node, depth) = queue.removeFirst(); visited += 1
            maxDepth = max(maxDepth, depth)
            let role = string(node, kAXRoleAttribute, maxBytes: 96, budget) ?? "?"
            roles[role, default: 0] += 1
            if role == "AXWebArea", webAreaDepth == nil {
                webAreaDepth = depth
                var names: CFArray?
                if AXUIElementCopyParameterizedAttributeNames(node, &names) == .success { webAreaParams = (names as? [String]) ?? [] }
            }
            if role == kAXStaticTextRole as String || role == kAXTextAreaRole as String {
                if role == kAXTextAreaRole as String { textAreas += 1 } else { staticText += 1 }
                if attribute(node, kAXNumberOfCharactersAttribute, budget) is NSNumber { withCount += 1 }
                if range(node, kAXVisibleCharacterRangeAttribute, budget) != nil { withVisible += 1 }
                if let value = attribute(node, kAXValueAttribute, budget) as? String, !value.isEmpty { withValue += 1; valueBytes += value.utf8.count }
            }
            var values: CFTypeRef?
            AXUIElementSetMessagingTimeout(node, 0.1)
            if AXUIElementCopyAttributeValue(node, kAXChildrenAttribute as CFString, &values) == .success, let children = values as? [AXUIElement] {
                queue.append(contentsOf: children.map { ($0, depth + 1) })
            }
        }
        lines.append("walk: visited \(visited) maxDepth \(maxDepth) expired \(budget.expired) remaining \(queue.count)")
        lines.append("webArea depth \(webAreaDepth.map(String.init) ?? "none") params \(webAreaParams.sorted().joined(separator: ","))")
        lines.append("text nodes: static \(staticText) textArea \(textAreas) withCharCount \(withCount) withVisibleRange \(withVisible) withValue \(withValue) valueBytes \(valueBytes)")
        lines.append("static text by skipped ancestor: " + hiddenBy.sorted { $0.value.bytes > $1.value.bytes }.map { "\($0.key)=\($0.value.count)/\($0.value.bytes)ch" }.joined(separator: " "))
        lines.append("roles: " + roles.sorted { $0.value > $1.value }.prefix(20).map { "\($0.key)=\($0.value)" }.joined(separator: " "))
        return lines
    }
}

/// `--ax-probe BUNDLE_ID OUT`: runs before the app opens any data, writes the
/// probe report to OUT and exits. Used to check what a given app exposes.
public enum DesktopAXProbe {
    public static func runIfRequested(arguments: [String] = ProcessInfo.processInfo.arguments) {
        guard let index = arguments.firstIndex(of: "--ax-probe"), index + 2 < arguments.count else { return }
        let bundleID = arguments[index + 1], output = URL(fileURLWithPath: arguments[index + 2])
        Task.detached {
            var lines: [String]
            if let running = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).first {
                let application = DesktopApplication(bundleID: bundleID, name: running.localizedName ?? bundleID, pid: running.processIdentifier, bundlePath: running.bundleURL?.path)
                lines = await DesktopAXWorker.shared.probe(application: application)
            } else {
                lines = ["\(bundleID) is not running"]
            }
            try? (lines.joined(separator: "\n") + "\n").write(to: output, atomically: true, encoding: .utf8)
            exit(0)
        }
        dispatchMain()
    }
}
