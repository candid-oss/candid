import AppKit
import ApplicationServices
import Foundation

// Read-only capability inventory. Scope comes from the installed capture's
// existing authorization; only numeric counts and capability flags leave IPC.
let preferences = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Candid/capture/preferences.json")
let raw = try JSONSerialization.jsonObject(with: Data(contentsOf: preferences)) as! [String: Any]
let allowed = Set(raw["textAllowedBundleIDs"] as? [String] ?? []).subtracting(raw["excludedBundleIDs"] as? [String] ?? [])
func attr(_ node: AXUIElement, _ key: String) -> CFTypeRef? {
    AXUIElementSetMessagingTimeout(node, 0.05)
    var v: CFTypeRef?; guard AXUIElementCopyAttributeValue(node, key as CFString, &v) == .success else { return nil }; return v
}
var rows: [[String: Any]] = []
for app in NSWorkspace.shared.runningApplications where allowed.contains(app.bundleIdentifier ?? "") {
    let root = AXUIElementCreateApplication(app.processIdentifier)
    guard AXIsProcessTrusted(), let win = attr(root, kAXFocusedWindowAttribute), CFGetTypeID(win) == AXUIElementGetTypeID() else {
        rows.append(["app": app.bundleIdentifier ?? "", "window": false]); continue
    }
    let window = unsafeDowncast(win, to: AXUIElement.self)
    let focus = attr(root, kAXFocusedUIElementAttribute)
    let deadline = ProcessInfo.processInfo.systemUptime + 0.6
    var queue = [window], visited = Set<UInt>(), staticNodes = 0, ranges = 0, urls = 0, selected = 0
    while !queue.isEmpty, visited.count < 128, ProcessInfo.processInfo.systemUptime < deadline {
        let node = queue.removeFirst(); guard visited.insert(CFHash(node)).inserted else { continue }
        if attr(node, kAXSubroleAttribute) as? String == kAXSecureTextFieldSubrole { continue }
        if (attr(node, "AXContainsProtectedContent") as? NSNumber)?.boolValue == true { continue }
        if attr(node, kAXRoleAttribute) as? String == "AXStaticText" { staticNodes += 1 }
        if attr(node, kAXVisibleCharacterRangeAttribute) != nil { ranges += 1 }
        if attr(node, kAXURLAttribute) != nil { urls += 1 }
        if (attr(node, kAXSelectedAttribute) as? NSNumber)?.boolValue == true { selected += 1 }
        queue += ((attr(node, kAXChildrenAttribute) as? [AXUIElement]) ?? []).prefix(max(0, 128 - visited.count - queue.count))
    }
    let focused = focus.flatMap { CFGetTypeID($0) == AXUIElementGetTypeID() ? unsafeDowncast($0, to: AXUIElement.self) : nil }
    rows.append(["app": app.bundleIdentifier ?? "", "window": true, "documentIdentity": attr(window, kAXDocumentAttribute) != nil,
                 "focusRole": focused.flatMap { attr($0, kAXRoleAttribute) as? String } ?? "unavailable", "staticNodes": staticNodes,
                 "visibleRanges": ranges, "urlNodes": urls, "selectedNodes": selected, "visited": visited.count,
                 "partial": !queue.isEmpty || visited.count >= 128, "sendConfirmation": "unconfirmed: no validated source adapter"])
}
let data = try JSONSerialization.data(withJSONObject: ["scope": "existing text-authorized running applications; no values or titles copied", "applications": rows], options: [.prettyPrinted, .sortedKeys])
FileHandle.standardOutput.write(data)
