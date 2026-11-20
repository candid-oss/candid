import ConsoleCore
import CandidLocalization
import SwiftUI

enum HomePage: Hashable { case timeline, library, ai, connections, device, settings, specimen }

/// One continuous surface: no sidebar, no toolbar box. Navigation, search and
/// capture state float over the ambient background and stay out of the way.
struct HomeRootView: View {
    @Bindable var session: DesktopSession
    let media: MediaWorkspaceStore
    let deviceStore: ConsoleStore?
    let configurationError: String?
    var showMediaOnLaunch = false

    @AppStorage("desktop.settings.tab") private var settingsTab = "capture"
    @ViewState private var quickNote = QuickNote()
    @ViewState private var actionError: String?
    @ViewState private var showingNote = false
    @FocusState private var searchFocused: Bool
    @AppStorage("candid.floating") private var floating = false
    @ViewState private var page: HomePage = .timeline
    @ViewState private var ambientTint: Color = .accentColor
    @ViewState private var pendingJump: Date?
    @ViewState private var requestedSkill: String?
    @ViewState private var overlayID: String?
    /// Development reference (`--design-specimen`): adds a chrome button for the design specimen.
    private let showsSpecimen = ProcessInfo.processInfo.arguments.contains("--design-specimen")
    /// Explicit native-control QA stays on demo data and never enables capture.
    private var enablesQuickControls: Bool {
        !TimelineService.isolated(isDemo: session.isDemo)
            || (session.isDemo && ProcessInfo.processInfo.arguments.contains("--qa-snapshot-dir")
                && ProcessInfo.processInfo.arguments.contains("--qa-controls"))
    }

    var body: some View {
        ZStack(alignment: .top) {
            WindowBackdrop(tint: page == .timeline ? ambientTint : page == .ai ? MediaGlyph.tint(.audio) : .accentColor)
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            chrome
            VStack(spacing: 10) {
                DraftBar(media: media, openLibrary: { page = .library })
                feedback
            }
            .padding(.top, 58)
            .animation(Motion.bouncy, value: media.draft?.id)
        }
        .overlayPreferenceValue(WindowOverlayKey.self) { overlay in
            if let overlay { overlay.view.transition(.opacity) }
        }
        .animation(Motion.snappy, value: overlayID)
        .onPreferenceChange(WindowOverlayKey.self) { overlayID = $0?.id }
        .ignoresSafeArea()
        .visualStyleFromSettings()
        .onAppear { if showMediaOnLaunch { page = .library } else if showsSpecimen { page = .specimen } }
        .onAppear {
            if enablesQuickControls { GlobalHotkeys.shared.start(); FloatingControl.shared.setVisible(floating, session: session, media: media) }
        }
        .onChange(of: floating) { _, value in
            if enablesQuickControls { FloatingControl.shared.setVisible(value, session: session, media: media) }
        }
        .onReceive(NotificationCenter.default.publisher(for: .candidQuickAction)) { note in
            if note.object as? String == "settings" { page = .settings; NSApp.windows.first { $0.frame.width > 600 }?.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true); return }
            guard let action = note.object as? QuickAction else { return }
            if action == .search || action == .note {
                NSApp.windows.first { $0.contentView != nil && $0.frame.width > 600 }?.makeKeyAndOrderFront(nil)
            }
            switch action {
            case .search: requestSearch()
            case .compose: ComposePanel.shared.show()
            case .note: quickNote.target = nil; showingNote = true; NSApp.activate(ignoringOtherApps: true)
            case .marker: if !session.isDemo { session.addMarker() }
            case .screenshot: if !session.isDemo && media.canCapture { Task { await media.captureRegion() } }
            case .recording:
                if !session.isDemo { Task { if media.isRecording { await media.stopRecording() } else if media.canCapture { await media.startRecording() } } }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: AIFunctionSettingsView.open)) { _ in
            settingsTab = "ai-functions"; page = .settings
        }
        .onReceive(NotificationCenter.default.publisher(for: .candidRunSkill)) { note in
            requestedSkill = note.object as? String; page = .ai
        }
        .onReceive(NotificationCenter.default.publisher(for: .candidAutomationRun)) { note in
            UserDefaults.standard.set(note.object as? String, forKey: "automation.selectedRun")
            settingsTab = "ai-functions"; page = .settings
        }
        .onReceive(NotificationCenter.default.publisher(for: .candidOpenRecord)) { note in
            guard let id = note.object as? String, let channel = TimelineService.shared.channel else { return }
            Task {
                if let data = try? await channel.send("POST", "/v1/get_event", body: JSONSerialization.data(withJSONObject: ["event_id": id])).checked(),
                   let ref = PiToolResult.records("<candid_data>" + (String(data: data, encoding: .utf8) ?? "") + "</candid_data>").first,
                   let start = AgendaDate.parse(ref.start) { pendingJump = start; page = .timeline }
                else { actionError = L("这条来源已不可用") }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .candidActionError)) { actionError = $0.object as? String }
        .onReceive(NotificationCenter.default.publisher(for: .candidNoteReference)) { note in
            quickNote.target = note.object as? String; showingNote = true
        }
        .popover(isPresented: $showingNote) { QuickNoteView(model: quickNote) { showingNote = false } }
        .onChange(of: session.isRecording) { _, value in WidgetPublisher.updateFlags(isRecording: value, isMicRecording: media.isRecording) }
        .onChange(of: media.isRecording) { _, value in WidgetPublisher.updateFlags(isRecording: session.isRecording, isMicRecording: value) }
        .onReceive(NotificationCenter.default.publisher(for: QASnapshot.command)) { note in
            let pages: [String: HomePage] = ["timeline": .timeline, "library": .library, "ai": .ai,
                                             "connections": .connections, "device": .device, "settings": .settings, "specimen": .specimen]
            if let line = note.object as? String, line.hasPrefix("page "), let target = pages[String(line.dropFirst(5))] { page = target }
            // Self-test only: `search <text>` types into the search capsule (`search` alone clears it).
            if let line = note.object as? String, line == "search" || line.hasPrefix("search ") {
                session.searchText = String(line.dropFirst(min(7, line.count))); page = .timeline
            }
        }
    }

    // MARK: Pages

    @ViewBuilder private var content: some View {
        ZStack {
            switch page {
            case .timeline:
                TimelineHomeView(session: session, media: media, ambientTint: $ambientTint, pendingJump: $pendingJump, autofocus: !searchFocused)
                    .transition(pageTransition)
            case .library:
                MediaWorkspaceView(store: media, showInTimeline: { date in pendingJump = date; page = .timeline })
                    .transition(pageTransition)
            case .ai:
                AIHomeView(session: session, requestedSkill: $requestedSkill, showInTimeline: { date in pendingJump = date; page = .timeline })
                    .transition(pageTransition)
            case .connections:
                ConnectionsView(session: session, media: media, deviceStore: deviceStore,
                                open: { page = $0 })
                    .transition(pageTransition)
            case .device:
                Group {
                    if let deviceStore {
                        ConsoleView(store: deviceStore, openConnectionSettings: { settingsTab = "console"; page = .settings })
                    } else {
                        VStack(alignment: .leading, spacing: 6) {
                            Text(L("服务地址无效")).typography(.headline)
                            Text(configurationError ?? L("请检查启动参数")).typography(.secondary).foregroundStyle(.secondary)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, Space.page).padding(.top, Space.l)
                    }
                }
                .padding(.top, 52)
                .transition(pageTransition)
            case .settings:
                SettingsHomeView(session: session, deviceSettings: deviceStore.map { AnyView(ConsoleConnectionSettings(store: $0)) })
                    .transition(pageTransition)
            case .specimen:
                DesignSpecimenView()
                    .transition(pageTransition)
            }
        }
        .animation(Motion.spring, value: page)
    }

    private var pageTransition: AnyTransition {
        .asymmetric(insertion: Motion.enter, removal: .opacity)
    }

    // MARK: Chrome

    private var chrome: some View {
        HStack(spacing: 10) {
            Color.clear.frame(width: 70, height: 1) // traffic lights
            HomeNav(page: $page)
            Spacer(minLength: 12)
            HStack(spacing: 8) {
                SearchCapsule(text: $session.searchText, focused: $searchFocused, onSearch: requestSearch) { if page != .timeline { page = .timeline } }
                CaptureControls(session: session, media: media, openSettings: { settingsTab = "capture"; page = .settings })
                if showsSpecimen {
                    ChromeButton(systemImage: "swatchpalette", help: L("设计样张"), active: page == .specimen) { toggle(.specimen) }
                }
                ChromeButton(systemImage: "gearshape", help: L("设置"), active: page == .settings) { toggle(.settings) }
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 52)
    }

    private func requestSearch() {
        page = .timeline
        NSApp.windows.first { $0.contentView != nil && $0.frame.width > 600 }?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        searchFocused = true
    }
    private func toggle(_ target: HomePage) { page = page == target ? .timeline : target }

    @ViewBuilder private var feedback: some View {
        let message = actionError ?? session.error ?? media.error ?? session.notice
        if let message {
            let isError = actionError != nil || session.error != nil || media.error != nil
            NoticeCapsule(text: message, isError: isError,
                          onClose: isError ? { actionError = nil; session.dismissError(); media.error = nil } : nil)
            .frame(maxWidth: 620)
            .transition(.move(edge: .top).combined(with: .opacity))
            .animation(Motion.bouncy, value: message)
        }
    }
}

/// Search that grows when focused; typing always lands on the timeline.
private struct SearchCapsule: View {
    @Binding var text: String
    @FocusState.Binding var focused: Bool
    let onSearch: () -> Void
    let onEdit: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").typography(.secondary, .medium).foregroundStyle(.secondary)
            TextField(L("搜索"), text: $text)
                .opacity(focused || !text.isEmpty ? 1 : 0)
                .textFieldStyle(.plain).typography(.body)
                .focused($focused)
                .accessibilityIdentifier("candid.search")
                .onChange(of: text) { _, _ in onEdit() }
                .onSubmit(onEdit)
            if !text.isEmpty {
                Button { text = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary) }
                    .buttonStyle(.plain).accessibilityLabel(L("清除搜索"))
            }
        }
        .padding(.horizontal, focused || !text.isEmpty ? 11 : 8)
        .frame(width: focused || !text.isEmpty ? 220 : 30, height: 30)
        .contentShape(Capsule())
        .onTapGesture(perform: onSearch)
        .background(Capsule().fill(focused ? Surface.hover.color : Surface.fill.color))
        .animation(Motion.bouncy, value: focused)
        .background {
            Button(L("搜索"), action: onSearch).keyboardShortcut("f", modifiers: .command).opacity(0).allowsHitTesting(false)
        }
    }
}


/// Something a page shows over the whole window (a zoomed screenshot). Compared by id only.
struct WindowOverlay: Equatable, @unchecked Sendable {
    let id: String
    let view: AnyView
    static func == (a: Self, b: Self) -> Bool { a.id == b.id }
}
struct WindowOverlayKey: PreferenceKey {
    static let defaultValue: WindowOverlay? = nil
    static func reduce(value: inout WindowOverlay?, nextValue: () -> WindowOverlay?) { value = nextValue() ?? value }
}
