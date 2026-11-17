import SwiftUI

/// The AI page (agent-platform-v1 §4.2, mockup ④): 「今天」 on the left (300pt), the chat with Pi on the right. Narrower
/// than 900pt the columns become a 「今天 / 对话」 switch. The fixed-processing settings open in place from 「设置」.
struct AIHomeView: View {
    let session: DesktopSession
    @Binding private var requestedSkill: String?
    @ViewState private var model: AIPageModel
    @ViewState private var showingSkills = false
    @ViewState private var showingHistory = false

    /// Opens the timeline at a record's time (a record chip in Pi's answer).
    var showInTimeline: ((Date) -> Void)?

    init(session: DesktopSession, requestedSkill: Binding<String?> = .constant(nil), model: AIPageModel? = nil, showInTimeline: ((Date) -> Void)? = nil, showingSkills: Bool = false, showingHistory: Bool = false) {
        self.session = session; _requestedSkill = requestedSkill
        _showingSkills = ViewState(initialValue: showingSkills)
        _showingHistory = ViewState(initialValue: showingHistory)
        self.showInTimeline = showInTimeline
        let isolated = TimelineService.isolated(isDemo: session.isDemo)
        let scratch = URL(fileURLWithPath: session.dataLocation, isDirectory: true).appendingPathComponent("AI", isDirectory: true)
        _model = ViewState(wrappedValue: model ?? AIPageModel(
            agent: isolated ? PiAgent(directory: scratch.appendingPathComponent("pi", isDirectory: true)) : nil,
            store: isolated ? AIConversationStore(directory: scratch.appendingPathComponent("conversations", isDirectory: true)) : .standard))
    }

    var body: some View {
        GeometryReader { geometry in
            let split = AIPageLayout.isSplit(geometry.size.width)
            VStack(alignment: .leading, spacing: Space.m) {
                header(split: split)
                if showingHistory {
                    AIHistoryPanel(model: model, close: { withAnimation(Motion.spring) { showingHistory = false } })
                }
                if showingSkills {
                    AISkillsPanel(model: model, close: { withAnimation(Motion.spring) { showingSkills = false } })
                }
                if split {
                    HStack(alignment: .top, spacing: Space.l) {
                        AITodayColumn(model: model).frame(width: AIPageLayout.todayWidth)
                        AIChatColumn(model: model)
                    }
                } else if model.tab == .today {
                    AITodayColumn(model: model).transition(Motion.enter)
                } else {
                    AIChatColumn(model: model).transition(Motion.enter)
                }
            }
            .padding(.horizontal, Space.page).padding(.top, 62).padding(.bottom, Space.m)
            .frame(maxWidth: 1180, maxHeight: .infinity, alignment: .topLeading)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .onAppear { model.isSplit = split; model.showInTimeline = showInTimeline }
            .onChange(of: split) { _, value in model.isSplit = value }
        }
        .task { await model.refresh() }
        .task(id: requestedSkill) {
            if let id = requestedSkill, let skill = model.skills.first(where: { $0.id == id }) {
                await model.runSkill(skill); model.show(.chat); requestedSkill = nil
            }
        }
        .onChange(of: TimelineService.shared.dataVersion) { _, _ in Task { await model.refreshCore() } }
    }

    private static let dateFormat: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "zh_CN")
        f.dateFormat = "M月d日 EEEE"
        return f
    }()

    private func header(split: Bool) -> some View {
        HStack(spacing: Space.s) {
            Text("AI").typography(.title, .bold).foregroundStyle(Palette.ai)
            Text(Self.dateFormat.string(from: model.currentTime)).typography(.secondary, digits: true).foregroundStyle(.secondary)
            Spacer(minLength: Space.xs)
            Group {
                if !split {
                    PillSwitcher(items: [(AIPageTab.today, "今天"), (AIPageTab.chat, "对话")],
                                 selection: Binding(get: { model.tab }, set: { model.show($0) }))
                        .overlay(alignment: .topTrailing) {
                            if model.chatUnread || (model.turnRunning && model.tab == .today) {
                                Circle().fill(Palette.accent).frame(width: 6, height: 6).offset(x: -2, y: 2)
                                    .accessibilityLabel("对话有新内容")
                            }
                        }
                }
                ChipButton(title: "技能" + (model.skills.isEmpty ? "" : " · \(model.skills.count)"), systemImage: "wand.and.stars", emphasized: showingSkills) {
                    withAnimation(Motion.spring) { showingSkills.toggle(); if showingSkills { showingHistory = false } }
                }
                ChipButton(title: "历史对话" + (model.history.isEmpty ? "" : " · \(model.history.count)"), systemImage: "clock.arrow.circlepath", emphasized: showingHistory) {
                    withAnimation(Motion.spring) { showingHistory.toggle(); if showingHistory { showingSkills = false } }
                }
                ChipButton(title: "新对话", systemImage: "plus") { Task { await model.newConversation(); model.show(.chat) } }
            }
            ChromeButton(systemImage: "slider.horizontal.3", help: "AI 功能设置", active: false) {
                NotificationCenter.default.post(name: AIFunctionSettingsView.open, object: nil)
            }
        }
    }

}
