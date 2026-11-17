import Foundation
import Testing
import CandidShared

@Suite(.serialized)
struct NetworkProxyTests {
    @Test func defaultFollowsSystemAndDirectDropsEveryProxy() {
        let key = NetworkProxy.defaultsKey
        let suite = "candid.tests.proxy." + UUID().uuidString
        let defaults = UserDefaults(suiteName: suite)!
        NetworkProxy.defaultsOverride = defaults
        defer { NetworkProxy.defaultsOverride = nil; defaults.removePersistentDomain(forName: suite) }

        #expect(NetworkProxy.current == .system)
        #expect(NetworkProxy.session() === URLSession.shared)

        let pair = ProxyAwareSession(configuration: .ephemeral)
        #expect(pair.system.configuration.connectionProxyDictionary == nil) // nil = macOS network settings
        #expect(pair.direct.configuration.connectionProxyDictionary?.isEmpty == true)
        #expect(pair.current === pair.system)

        defaults.set("direct", forKey: key)
        #expect(NetworkProxy.current == .direct)
        #expect(pair.current === pair.direct)
        #expect(NetworkProxy.session() !== URLSession.shared)

        defaults.set("unknown", forKey: key)
        #expect(NetworkProxy.current == .system)
    }
}
