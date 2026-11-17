import Foundation
import Testing
@testable import Candid

@MainActor @Test func originalActionsResolveActualPartsInsteadOfOpeningThePlaylistAsAnOriginal() throws {
    let single = try MediaOriginalActions.files(from: Data(#"{"path":"/SYNTHETIC/original.wav"}"#.utf8))
    #expect(single.map(\.path) == ["/SYNTHETIC/original.wav"])
    let parts = try MediaOriginalActions.files(from: Data(#"{"format":"candid-playlist/1","parts":[{"path":"/SYNTHETIC/first.wav"},{"path":"/SYNTHETIC/second.wav"}]}"#.utf8))
    #expect(parts.map(\.path) == ["/SYNTHETIC/first.wav", "/SYNTHETIC/second.wav"])
    for raw in [#"{"path":"relative.wav"}"#, #"{"format":"candid-playlist/99","parts":[{"path":"/SYNTHETIC/first.wav"}]}"#, #"{"format":"candid-playlist/1","parts":[]}"#] {
        #expect(throws: (any Error).self) { try MediaOriginalActions.files(from: Data(raw.utf8)) }
    }
}
