import DesktopCore
import Foundation

/// An on-demand inventory of application bundle metadata. Call from a utility
/// task; this never launches applications, scans documents, or grants capture.
public enum DesktopApplicationCatalog {
    public static func scan() -> [DesktopApplication] {
        scan(roots: [URL(fileURLWithPath: "/Applications", isDirectory: true),
                     FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications", isDirectory: true),
                     URL(fileURLWithPath: "/System/Applications", isDirectory: true)])
    }

    /// The running-process inventory uses the same filter as installed apps.
    /// A helper can advertise a regular activation policy and the parent's name.
    /// LSUIElement alone is not a helper identity: standalone menu-bar apps stay
    /// selectable, while application/extension containment rejects nested tools.
    public static func isUserApplication(url: URL) -> Bool {
        descriptor(url: url) != nil
    }

    static func scan(roots: [URL], maximumApplications: Int = 512, maximumDepth: Int = 4) -> [DesktopApplication] {
        guard maximumApplications > 0, maximumDepth >= 0 else { return [] }
        let manager = FileManager.default
        var known: [String: DesktopApplication] = [:]
        var visitedDirectories = Set<String>()
        var inspectedEntries = 0
        // Both bounds apply independently: malformed installations or a large
        // directory of non-app files cannot cause an unbounded walk.
        let maximumEntries = 8192
        for root in roots {
            var pending: [(URL, Int)] = [(root, 0)]
            while let (directory, depth) = pending.popLast(), known.count < maximumApplications, inspectedEntries < maximumEntries {
                guard !Task.isCancelled,
                      visitedDirectories.insert(directory.standardizedFileURL.path).inserted,
                      let children = try? manager.contentsOfDirectory(at: directory,
                          includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey, .isPackageKey],
                          options: [.skipsHiddenFiles]) else { continue }
                for child in children.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
                    guard known.count < maximumApplications, inspectedEntries < maximumEntries, !Task.isCancelled else { break }
                    inspectedEntries += 1
                    if child.pathExtension.lowercased() == "app" {
                        if let app = descriptor(url: child), known[app.bundleID] == nil { known[app.bundleID] = app }
                        continue // Never enumerate an app's Contents or helpers.
                    }
                    guard depth < maximumDepth,
                          let values = try? child.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey, .isPackageKey]),
                          values.isDirectory == true, values.isSymbolicLink != true, values.isPackage != true,
                          child.pathExtension.lowercased() != "appex" else { continue }
                    pending.append((child, depth + 1))
                }
            }
            if known.count >= maximumApplications || inspectedEntries >= maximumEntries || Task.isCancelled { break }
        }
        return known.values.sorted {
            let result = $0.name.localizedStandardCompare($1.name)
            return result == .orderedSame ? $0.bundleID < $1.bundleID : result == .orderedAscending
        }
    }

    public static func descriptor(url: URL) -> DesktopApplication? {
        let resolved = url.resolvingSymlinksInPath().standardizedFileURL
        guard resolved.pathExtension.lowercased() == "app" else { return nil }
        let parents = resolved.deletingLastPathComponent().pathComponents
        guard !parents.contains(where: { component in
            let suffix = (component as NSString).pathExtension.lowercased()
            return suffix == "app" || suffix == "appex"
        }), let bundle = Bundle(url: resolved),
              let info = bundle.infoDictionary,
              info["CFBundlePackageType"] as? String == "APPL",
              let identifier = bundle.bundleIdentifier, !identifier.isEmpty,
              info["NSExtension"] == nil,
              !flag(info["LSBackgroundOnly"]) else { return nil }
        // A recorder's own supported language must not force the whole app
        // catalogue to English. Resolve each bundle using the user's languages.
        let languages = UserDefaults.standard.stringArray(forKey: "AppleLanguages") ?? Locale.preferredLanguages
        let language = Bundle.preferredLocalizations(from: bundle.localizations, forPreferences: languages).first
        let localized: [String: Any]?
        if let language, let strings = bundle.url(forResource: "InfoPlist", withExtension: "strings", subdirectory: nil, localization: language),
           let data = try? Data(contentsOf: strings), data.count <= 65536,
           let values = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any] { localized = values }
        else { localized = bundle.localizedInfoDictionary }
        let name = localized?["CFBundleDisplayName"] as? String
            ?? info["CFBundleDisplayName"] as? String
            ?? localized?["CFBundleName"] as? String
            ?? info["CFBundleName"] as? String
            ?? resolved.deletingPathExtension().lastPathComponent
        return DesktopApplication(bundleID: identifier, name: name, pid: 0, bundlePath: resolved.path)
    }

    private static func flag(_ value: Any?) -> Bool {
        if let number = value as? NSNumber { return number.boolValue }
        if let string = value as? String { return ["1", "true", "yes"].contains(string.lowercased()) }
        return false
    }
}
