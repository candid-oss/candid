import AppKit
import Foundation
import SwiftUI
import Testing
@testable import Candid

@Suite @MainActor struct RecordingSpeakerSettingsTests {
    @Test func wholeRecordingLabelsHidePhysicalSegmentsAndSettingsRender() throws {
        let object: [String:Any] = ["id":UUID().uuidString,"filename":"SYNTHETIC.wav","kind":"audio","source":"mac_microphone",
            "imported_at":"2026-11-16T00:00:00Z","processing_status":"ready","processing_mode":"conversation",
            "speaker_policy":["version":1,"preset":"balanced","max_speakers":0],"speaker_grouping":["speaker_count":3,"unresolved_segments":1],
            "segments":[["start_seconds":0,"end_seconds":3,"text":"SYNTHETIC","speaker":"recording-speaker:opaque","speaker_display_label":"说话人 1"]]]
        let decoder=JSONDecoder();decoder.dateDecodingStrategy = .iso8601;decoder.keyDecodingStrategy = .convertFromSnakeCase
        let item=try decoder.decode(MediaLibraryRecord.self,from:JSONSerialization.data(withJSONObject:object))
        #expect(item.segments.first?.shownSpeaker == "说话人 1")
        #expect(item.speakerPolicy?.maxSpeakers == 0 && item.speakerGrouping?.speakerCount == 3)
        guard let path=ProcessInfo.processInfo.environment["CANDID_AUDIO_RENDER_DIR"] else {return}
        let directory=URL(fileURLWithPath:path);try FileManager.default.createDirectory(at:directory,withIntermediateDirectories:true)
        let media=MediaWorkspaceStore(profile:directory,demoRecords:[item])
        for dark in [false,true] {
            let content=RecordingSpeakerSettings(item:item,store:media,close:{}).environment(\.colorScheme,dark ? .dark : .light)
            _ = NSApplication.shared
            let window=NSWindow(contentRect:NSRect(x:0,y:0,width:360,height:260),styleMask:[.borderless],backing:.buffered,defer:false)
            window.isReleasedWhenClosed=false
            window.appearance=NSAppearance(named:dark ? .darkAqua:.aqua)
            let host=NSHostingView(rootView:content.background(Color(nsColor:.windowBackgroundColor)))
            host.frame=NSRect(x:0,y:0,width:360,height:260);window.contentView=host
            host.layoutSubtreeIfNeeded()
            let bitmap=try #require(host.bitmapImageRepForCachingDisplay(in:host.bounds))
            host.cacheDisplay(in:host.bounds,to:bitmap)
            try #require(bitmap.representation(using:.png,properties:[:])).write(to:directory.appendingPathComponent(dark ? "speakers-dark.png":"speakers-light.png"))
            window.close()
        }
    }
}
