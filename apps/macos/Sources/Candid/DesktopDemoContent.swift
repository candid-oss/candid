import AppKit
import DesktopCore

/// A synthetic day for `--mock`: system app identities for real icons, but
/// every title, text and image is generated here and labeled 合成样例. Nothing
/// is read from the user's screen or files.
enum DesktopDemoContent {
    struct Item { let record: DesktopRecord; let image: Data? }

    private struct Segment {
        let minutes: Double
        let bundle: String
        let app: String
        let title: String
        let text: String
        let dark: Bool
    }

    private static let lunchMinutes: Double = 57

    @MainActor static func items(now: Date) -> [Item] {
        let plan = plan()
        return items(plan: plan, now: now)
    }

    @MainActor private static func plan() -> [Segment?] {
        let editor = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.dt.Xcode") != nil
            ? ("com.apple.dt.Xcode", "Xcode") : ("com.apple.TextEdit", "文本编辑")
        func code(_ title: String, _ text: String, _ minutes: Double) -> Segment {
            Segment(minutes: minutes, bundle: editor.0, app: editor.1, title: title, text: text, dark: true)
        }
        let plan: [Segment?] = [
            Segment(minutes: 27, bundle: "com.apple.Notes", app: "备忘录", title: "晨间计划", text: "完成横向时间线；核对截图预算；下午整理会议纪要", dark: false),
            Segment(minutes: 4, bundle: "com.apple.mail", app: "邮件", title: "收件箱", text: "挂件第二版外壳打样寄到了，明天可以试装", dark: false),
            code("DesktopSession.swift", "let loaded = try await store.activities(from: day, to: end, query: searchText)", 39),
            Segment(minutes: 4, bundle: "com.apple.Terminal", app: "终端", title: "zsh — macos", text: "swift test --filter DesktopTimelineGroupingTests", dark: true),
            Segment(minutes: 8, bundle: "com.apple.Safari", app: "Safari", title: "ScreenCaptureKit", text: "Capture a single frame of shareable content as an image.", dark: false),
            Segment(minutes: 41, bundle: "com.apple.iCal", app: "日历", title: "周会 · 硬件与时间线", text: "SD 卡实时写入；时间轴横向；挂件下周试装", dark: false),
            code("TimelineScrubber.swift", "struct TimelineScrubber: View { @Binding var playhead: Date }", 42),
            Segment(minutes: 5, bundle: "com.apple.mail", app: "邮件", title: "周末安排", text: "周末回来吃饭吗", dark: false),
            Segment(minutes: 3, bundle: "com.apple.finder", app: "访达", title: "下载", text: "datasheet.pdf · 外壳_v2.step · 接线.heic", dark: false),
            Segment(minutes: 33, bundle: "com.apple.Safari", app: "Safari", title: "Gradient 文档", text: "A two-dimensional gradient defined by a grid of colors.", dark: false),
            nil, // lunch: recording paused
            Segment(minutes: 15, bundle: "com.apple.Music", app: "音乐", title: "每日推荐", text: "后摇合集", dark: true),
            Segment(minutes: 9, bundle: "com.apple.Notes", app: "备忘录", title: "时间线设计", text: "时间轴放底部；图标挂在细线上；细节按需展开", dark: false),
            code("HangingIcon.swift", "let scale = 1 + 0.75 * exp(-(d * d) / (2 * sigma * sigma))", 68),
            Segment(minutes: 4, bundle: "com.apple.Terminal", app: "终端", title: "zsh — build", text: "script/build_and_run.sh --mock", dark: true),
            Segment(minutes: 11, bundle: "com.apple.mail", app: "邮件", title: "项目进度", text: "文字识别在聊天窗口里的效果如何？", dark: false),
            Segment(minutes: 42, bundle: "com.apple.Notes", app: "备忘录", title: "会议纪要 · 周会", text: "SD 卡实时写入；时间轴横向；挂件试装下周", dark: false),
            Segment(minutes: 8, bundle: "com.apple.Safari", app: "Safari", title: "本地模型接口", text: "OpenAI compatible endpoints: /v1/models", dark: false),
            code("TimelineScrubber.swift", "if next.reveal(value, width: width, margin: 36, bounds: scene.bounds)", 17),
            Segment(minutes: 3, bundle: "com.apple.Preview", app: "预览", title: "外壳_v2.pdf", text: "外壳尺寸 42 × 28 × 14 mm", dark: false),
            // A short glance at another app inside one editing run, then a fragmented stretch of quick switches.
            code("DayRhythmView.swift", "RingArc(start: fraction(span.start), end: fraction(span.end))", 16),
            Segment(minutes: 2, bundle: "com.apple.MobileSMS", app: "信息", title: "硬件小组", text: "外壳样品到了吗", dark: false),
            code("DayRhythmView.swift", "let fraction = (date - dayStart) / 86_400", 18),
            Segment(minutes: 3, bundle: "com.apple.Safari", app: "Safari", title: "himalaya README", text: "CLI to manage emails", dark: false),
            Segment(minutes: 2, bundle: "com.apple.MobileSMS", app: "信息", title: "硬件小组", text: "到了，明天试装", dark: false),
            Segment(minutes: 4, bundle: "com.apple.Safari", app: "Safari", title: "pimalaya 文档", text: "email-lib: IMAP, SMTP, Maildir", dark: false),
            Segment(minutes: 1, bundle: "com.apple.mail", app: "邮件", title: "收件箱", text: "新邮件 2 封", dark: false),
            Segment(minutes: 2, bundle: "com.apple.finder", app: "访达", title: "下载", text: "外壳_v3.step", dark: false),
            Segment(minutes: 2, bundle: "com.apple.Notes", app: "备忘录", title: "待办", text: "邮件连接器：先读本机邮件", dark: false),
            Segment(minutes: 6, bundle: "com.apple.mail", app: "邮件", title: "样稿", text: "样稿今晚能看吗", dark: false),
            code("TimelineHomeView.swift", ".matchedGeometryEffect(id: episode.id, in: cards)", 32),
        ]
        return plan
    }

    /// The synthetic day normally ends now; shortly after midnight that would put it in yesterday,
    /// so it then starts at the beginning of today instead and the shown day keeps its content.
    static func dayStart(total minutes: Double, now: Date, calendar: Calendar = .current) -> Date {
        max(now.addingTimeInterval(-minutes * 60), calendar.startOfDay(for: now).addingTimeInterval(60))
    }

    /// Start of each planned segment (nil entries are the paused lunch).
    private static func starts(_ plan: [Segment?], now: Date) -> [Date] {
        let total = plan.reduce(0.0) { $0 + ($1?.minutes ?? lunchMinutes) }
        var cursor = dayStart(total: total, now: now), result: [Date] = []
        for segment in plan { result.append(cursor); cursor = cursor.addingTimeInterval((segment?.minutes ?? lunchMinutes) * 60) }
        return result
    }

    /// Synthetic library items placed inside the same synthetic day: a meeting
    /// recording, two voice memos and one region screenshot.
    @MainActor static func mediaRecords(now: Date) -> [MediaLibraryRecord] {
        let starts = starts(plan(), now: now)
        func audio(_ name: String, at start: Date, lines: [(Double, Double, String?, String)]) -> MediaLibraryRecord {
            MediaLibraryRecord(id: UUID(), filename: name, kind: .audio, source: .macMicrophone, occurredAt: start, importedAt: start,
                               processingStatus: .ready,
                               segments: lines.map { .init(startSeconds: $0.0, endSeconds: $0.1, text: $0.3 + "（合成样例）", speaker: $0.2, identity: nil) },
                               error: nil, processingMode: "conversation")
        }
        return [
            audio("周会录音.m4a", at: starts[5].addingTimeInterval(60), lines: [
                (0, 240, "说话人 1", "SD 卡这边建议直接实时写 Opus，断电最多丢一秒。"),
                (300, 620, "说话人 2", "可以。时间轴想改成横向，放在底部，图标挂在线上。"),
                (700, 1100, "说话人 1", "悬停放大要克制，别晃。"),
                (1200, 1600, "说话人 2", "详细信息默认收起，需要时再展开。"),
                (1700, 2280, "说话人 1", "挂件外壳下周试装，到时候一起看收音。")
            ]),
            audio("语音备忘.m4a", at: starts[13].addingTimeInterval(25 * 60), lines: [
                (0, 70, nil, "卡片闪烁是因为悬停区域和卡片本身重叠了。"),
                (75, 150, nil, "提示卡应该浮在整条时间轴上方。")
            ]),
            audio("节律视图想法.m4a", at: starts[18].addingTimeInterval(6 * 60), lines: [
                (0, 60, nil, "节律用 24 小时的环，夜里空着本身就是信息。"),
                (62, 100, nil, "右边只放应用排行，别的都不要。")
            ]),
            MediaLibraryRecord(id: UUID(), filename: "框选截图.png", kind: .image, source: .macRegion, occurredAt: starts[15].addingTimeInterval(180),
                               importedAt: starts[15].addingTimeInterval(180), processingStatus: .idle, segments: [], error: nil, processingMode: nil),
        ]
    }

    @MainActor private static func items(plan: [Segment?], now: Date) -> [Item] {
        let lunch = lunchMinutes
        let total = plan.reduce(0.0) { $0 + ($1?.minutes ?? lunch) }
        var cursor = dayStart(total: total, now: now)
        var items: [Item] = []
        let run = "synthetic-demo"
        for (index, segment) in plan.enumerated() {
            guard let segment else {
                items.append(Item(record: DesktopRecord(timestamp: cursor, kind: .gap, detail: "合成样例：午饭时暂停记录", runID: run), image: nil))
                cursor = cursor.addingTimeInterval(lunch * 60); continue
            }
            let activity = "demo-\(index)"
            let observationStep = max(1, min(4, segment.minutes / 2)) * 60
            var t = cursor, first = true, lastObservation: DesktopRecord?
            var nextShot = cursor.addingTimeInterval(20)
            var shot = 0
            while t < cursor.addingTimeInterval(segment.minutes * 60 - 20) {
                let observation = DesktopRecord(
                    timestamp: t, kind: first ? .application : .context, appName: segment.app, bundleID: segment.bundle,
                    windowTitle: segment.title, detail: "合成样例", runID: run, trigger: first ? "application" : "timer",
                    context: DesktopContextSnapshot(readableText: segment.text + "（合成样例）", textStatus: "partial"),
                    activityID: activity)
                items.append(Item(record: observation, image: nil))
                lastObservation = observation; first = false
                if t >= nextShot, let parent = lastObservation {
                    let record = DesktopRecord(timestamp: t.addingTimeInterval(2), kind: .screenshot, appName: segment.app,
                                               bundleID: segment.bundle, windowTitle: segment.title, detail: "合成样例截图",
                                               imageWidth: 1280, imageHeight: 800, parentID: parent.id, runID: run,
                                               trigger: shot == 0 ? "application" : "timer", activityID: activity)
                    items.append(Item(record: record, image: autoreleasepool { render(segment, variant: shot) }))
                    shot += 1
                    nextShot = t.addingTimeInterval(9 * 60)
                }
                t = t.addingTimeInterval(observationStep)
            }
            if index == 5 || index == 13 {
                items.append(Item(record: DesktopRecord(timestamp: cursor.addingTimeInterval(segment.minutes * 25), kind: .marker,
                                                        detail: index == 5 ? "合成样例：这里定了方案" : "合成样例：复现了一个问题", runID: run), image: nil))
            }
            cursor = cursor.addingTimeInterval(segment.minutes * 60)
        }
        return items
    }

    // MARK: Synthetic window images

    @MainActor private static func render(_ segment: Segment, variant: Int) -> Data? {
        let width = 1280, height = 800
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height, bitsPerSample: 8,
                                         samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0, bitsPerPixel: 0),
              let context = NSGraphicsContext(bitmapImageRep: rep) else { return nil }
        NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = context
        defer { NSGraphicsContext.restoreGraphicsState() }
        let w = CGFloat(width), h = CGFloat(height)
        let accent = accentColor(segment.bundle)
        let background = segment.dark ? NSColor(white: 0.12, alpha: 1) : NSColor(white: 0.985, alpha: 1)
        let ink = segment.dark ? NSColor(white: 0.92, alpha: 1) : NSColor(white: 0.12, alpha: 1)
        let muted = segment.dark ? NSColor(white: 0.32, alpha: 1) : NSColor(white: 0.88, alpha: 1)
        background.setFill(); NSRect(x: 0, y: 0, width: w, height: h).fill()
        // Title bar.
        (segment.dark ? NSColor(white: 0.18, alpha: 1) : NSColor(white: 0.93, alpha: 1)).setFill()
        NSRect(x: 0, y: h - 52, width: w, height: 52).fill()
        for (i, color) in [NSColor.systemRed, .systemYellow, .systemGreen].enumerated() {
            color.setFill(); NSBezierPath(ovalIn: NSRect(x: 22 + CGFloat(i) * 24, y: h - 33, width: 14, height: 14)).fill()
        }
        draw("\(segment.app) — \(segment.title)", at: NSPoint(x: w / 2 - 180, y: h - 37), size: 17, weight: .semibold, color: ink.withAlphaComponent(0.7))
        // Sidebar.
        accent.withAlphaComponent(segment.dark ? 0.16 : 0.08).setFill()
        NSRect(x: 0, y: 0, width: 250, height: h - 52).fill()
        for row in 0..<9 {
            let y = h - 100 - CGFloat(row) * 44
            if row == (variant + 2) % 9 { accent.withAlphaComponent(0.85).setFill(); NSBezierPath(roundedRect: NSRect(x: 14, y: y - 10, width: 222, height: 34), xRadius: 8, yRadius: 8).fill() }
            muted.setFill(); NSBezierPath(roundedRect: NSRect(x: 30, y: y, width: CGFloat(90 + (row * 37 + variant * 13) % 100), height: 12), xRadius: 6, yRadius: 6).fill()
        }
        // Content.
        draw(segment.title, at: NSPoint(x: 300, y: h - 140), size: 40, weight: .bold, color: ink)
        draw(segment.text, at: NSPoint(x: 300, y: h - 200), size: 22, weight: .regular, color: ink.withAlphaComponent(0.8))
        accent.setFill()
        NSBezierPath(roundedRect: NSRect(x: 300, y: h - 470 + CGFloat(variant % 3) * 10, width: w - 360, height: 220), xRadius: 18, yRadius: 18).fill()
        NSGradient(starting: accent, ending: accent.blended(withFraction: 0.5, of: .white) ?? accent)?
            .draw(in: NSBezierPath(roundedRect: NSRect(x: 300, y: h - 470 + CGFloat(variant % 3) * 10, width: w - 360, height: 220), xRadius: 18, yRadius: 18), angle: 20)
        for line in 0..<5 {
            muted.setFill()
            NSBezierPath(roundedRect: NSRect(x: 300, y: h - 520 - CGFloat(line) * 38, width: CGFloat(420 + (line * 131 + variant * 57) % 480), height: 14), xRadius: 7, yRadius: 7).fill()
        }
        draw("合成样例", at: NSPoint(x: w - 150, y: 24), size: 20, weight: .semibold, color: ink.withAlphaComponent(0.35))
        return rep.representation(using: .jpeg, properties: [.compressionFactor: 0.72])
    }

    private static func draw(_ text: String, at point: NSPoint, size: CGFloat, weight: NSFont.Weight, color: NSColor) {
        (text as NSString).draw(at: point, withAttributes: [.font: NSFont.systemFont(ofSize: size, weight: weight), .foregroundColor: color])
    }

    private static func accentColor(_ bundle: String) -> NSColor {
        switch bundle {
        case "com.apple.Safari": .systemBlue
        case "com.apple.Notes": .systemYellow
        case "com.apple.mail": .systemTeal
        case "com.apple.iCal": .systemRed
        case "com.apple.Music": .systemPink
        case "com.apple.Terminal": .systemGreen
        case "com.apple.finder": .systemCyan
        case "com.apple.Preview": .systemOrange
        default: .systemIndigo
        }
    }
}
