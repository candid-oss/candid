import AVFoundation
import CandidLocalization
import Combine
import ConsoleCore

@MainActor
final class PlaybackController: NSObject, ObservableObject, AVAudioPlayerDelegate {
    @Published private(set) var isPlaying = false
    @Published private(set) var loading = false
    @Published private(set) var error: String?
    @Published private(set) var duration: TimeInterval = 0
    @Published private(set) var position: TimeInterval = 0
    private struct Part: Decodable {
        let path: String
        let sampleStart: Int64
        let sampleCount: Int64
        let sampleRate: Int
        var start: Double { Double(sampleStart) / Double(sampleRate) }
        var end: Double { Double(sampleStart + sampleCount) / Double(sampleRate) }
    }
    private struct Playlist: Decodable { let format: String; let parts: [Part] }
    private var parts: [Part] = []
    private var partIndex = 0
    private var offset: Double { parts.isEmpty ? 0 : parts[partIndex].start }
    private var player: AVAudioPlayer?
    private var loadedID: String?
    private var generation = UUID()
    private var ticker: Task<Void, Never>?

    func toggle(asset: AudioAsset, service: any ConsoleService) async {
        await toggle(id: asset.id) {
            let data = try await service.media(assetID: asset.id)
            return try AVAudioPlayer(data: data)
        }
    }

    func toggleFile(id: String, startAt: TimeInterval? = nil, url: () async throws -> URL) async {
        await toggle(id: id, startAt: startAt) {
            let file = try await url()
            if file.lastPathComponent.hasSuffix(".playlist.json") {
                let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
                let playlist = try decoder.decode(Playlist.self, from: Data(contentsOf: file))
                guard playlist.format == "candid-playlist/1", !playlist.parts.isEmpty else { throw ConsoleError.rejected(L("录音分段无法读取")) }
                self.parts = playlist.parts
                let time = startAt ?? 0
                self.partIndex = self.parts.firstIndex(where: { time < $0.end }) ?? self.parts.count - 1
                return try AVAudioPlayer(contentsOf: URL(fileURLWithPath: self.parts[self.partIndex].path))
            }
            return try AVAudioPlayer(contentsOf: file)
        }
    }

    private func toggle(id: String, startAt: TimeInterval? = nil, load: () async throws -> AVAudioPlayer) async {
        if loadedID == id, let player {
            if let startAt { seek(startAt) }
            if player.isPlaying && startAt == nil {
                player.pause(); isPlaying = false; ticker?.cancel()
            } else {
                isPlaying = player.play()
                if isPlaying { tick() }
            }
            return
        }
        stop(); loading = true; error = nil
        let request = generation
        do {
            let player = try await load()
            guard generation == request, !Task.isCancelled else { return }
            player.delegate = self; player.prepareToPlay()
            if let startAt { player.currentTime = max(0, min(startAt - offset, player.duration)) }
            guard player.play() else { throw ConsoleError.rejected(L("系统无法播放该录音")) }
            self.player = player; loadedID = id; duration = parts.last?.end ?? player.duration
            position = offset + player.currentTime
            loading = false; isPlaying = true; tick()
        } catch {
            guard generation == request else { return }
            loading = false; self.error = error.localizedDescription
        }
    }
    func seek(_ time: TimeInterval) {
        guard var player else { return }
        let time = max(0, min(time, duration))
        if !parts.isEmpty {
            let index = parts.firstIndex(where: { time < $0.end }) ?? parts.count - 1
            if index != partIndex {
                do {
                    let replacement = try AVAudioPlayer(contentsOf: URL(fileURLWithPath: parts[index].path))
                    player.stop(); partIndex = index; player = replacement; self.player = replacement
                    replacement.delegate = self; replacement.prepareToPlay()
                    if isPlaying { _ = replacement.play() }
                } catch { self.error = error.localizedDescription; return }
            }
        }
        let clamped = max(0, min(time - offset, player.duration))
        player.currentTime = clamped
        position = offset + clamped
    }
    func stop() {
        generation = UUID(); ticker?.cancel(); ticker = nil
        player?.stop(); player = nil; loadedID = nil; parts = []; partIndex = 0
        isPlaying = false; loading = false; position = 0; duration = 0; error = nil
    }
    private func tick() {
        ticker?.cancel()
        ticker = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                self.position = self.offset + (self.player?.currentTime ?? 0)
                do { try await Task.sleep(for: .milliseconds(200)) } catch { return }
            }
        }
    }
    nonisolated func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        let finished = ObjectIdentifier(player)
        Task { @MainActor [weak self] in
            guard let self, let active = self.player, ObjectIdentifier(active) == finished else { return }
            if flag, self.partIndex + 1 < self.parts.count {
                do {
                    self.partIndex += 1
                    let next = try AVAudioPlayer(contentsOf: URL(fileURLWithPath: self.parts[self.partIndex].path))
                    self.player = next; next.delegate = self; next.prepareToPlay()
                    self.position = self.offset; self.isPlaying = next.play(); self.tick()
                    return
                } catch { self.error = error.localizedDescription }
            }
            active.currentTime = 0
            isPlaying = false; ticker?.cancel(); position = 0
            if !flag { error = L("音频播放未能完成") }
        }
    }
}
