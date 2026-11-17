import Foundation

/// Candid's one proxy choice (设置 → 模型 → 网络代理). `system` follows the macOS network settings (the default);
/// `direct` connects without any proxy. Every HTTP client in the app — model sources, screenshot recognition, the
/// device console, the media library — and the `candid` command read it on each request, so a change applies at once.
public enum NetworkProxy: String, CaseIterable, Sendable {
    case system, direct

    public static let defaultsKey = "network.proxy"
    static let appDomain = "io.github.candid-oss.candid"

    /// Tests point this at a throwaway suite so they never read or write the app's settings.
    nonisolated(unsafe) public static var defaultsOverride: UserDefaults?

    /// The app reads its own defaults; a command-line process (no bundle) reads the app's domain.
    public static var current: NetworkProxy {
        let defaults = defaultsOverride
            ?? (Bundle.main.bundleIdentifier != nil ? UserDefaults.standard : (UserDefaults(suiteName: appDomain) ?? .standard))
        return NetworkProxy(rawValue: defaults.string(forKey: defaultsKey) ?? "") ?? .system
    }

    /// A session for clients without their own delegate or limits.
    public static func session() -> URLSession { current == .direct ? plain.direct : URLSession.shared }

    private static let plain = ProxyAwareSession(configuration: .default)
}

/// A pair of sessions with the same configuration and delegate, one following the system proxy and one direct;
/// `current` picks by the setting at the moment of the request.
public final class ProxyAwareSession: @unchecked Sendable {
    public let system: URLSession
    public let direct: URLSession

    public init(configuration: URLSessionConfiguration, delegate: (any URLSessionDelegate)? = nil) {
        let systemConfiguration = configuration.copy() as! URLSessionConfiguration
        let directConfiguration = configuration.copy() as! URLSessionConfiguration
        directConfiguration.connectionProxyDictionary = [:] // empty: no proxy at all; nil (the default) follows macOS
        system = URLSession(configuration: systemConfiguration, delegate: delegate, delegateQueue: nil)
        direct = URLSession(configuration: directConfiguration, delegate: delegate, delegateQueue: nil)
    }

    public var current: URLSession { NetworkProxy.current == .direct ? direct : system }
}
