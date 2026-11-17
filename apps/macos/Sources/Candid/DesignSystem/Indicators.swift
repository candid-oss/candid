import AIProviders
import CandidLocalization
import SwiftUI

// Tags, status and feedback: small, fixed vocabulary, so the same state always looks the same.

/// A small capsule label. `neutral` for facts (默认, 尚未使用), `tinted` for meaning (已修改, 本机, 规划中).
struct Tag: View {
    let text: String
    var tint: Color?

    var body: some View {
        Text(text).typography(.caption, .semibold)
            .foregroundStyle(tint ?? .secondary)
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(Capsule().fill(tint.map { $0.opacity(0.13) } ?? Surface.fill.color))
            .contentTransition(.opacity)
    }
}

/// 「默认 / 已修改」 for anything the user can override.
struct StateTag: View {
    let isDefault: Bool
    var body: some View { Tag(text: isDefault ? L("默认") : L("已修改"), tint: isDefault ? nil : Palette.accent) }
}

extension Destination {
    var label: String {
        switch self { case .onDevice: L("本机"); case .selfHosted: L("自建"); case .cloud: L("云端") }
    }
    var tint: Color {
        switch self { case .onDevice: .green; case .selfHosted: .blue; case .cloud: .orange }
    }
    var symbol: String {
        switch self { case .onDevice: "laptopcomputer"; case .selfHosted: "server.rack"; case .cloud: "cloud" }
    }
}

/// Where content goes when this source or engine is used.
struct DestinationTag: View {
    let destination: Destination
    var body: some View {
        Tag(text: destination.label, tint: destination.tint)
            .help(destination == .cloud ? L("内容会发往第三方服务") : destination == .selfHosted ? L("内容只发往你自己登记的服务器") : L("内容留在这台电脑上"))
    }
}

/// A dot and a word for the state of a connection or capture.
struct StatusDot: View {
    enum State { case on, off, attention, planned }
    let state: State
    let text: String

    private var color: Color {
        switch state { case .on: Palette.success; case .off: .secondary; case .attention: Palette.warning; case .planned: .purple }
    }

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(text).typography(.secondary, .medium).foregroundStyle(state == .off ? Color.secondary : Color.primary)
        }
    }
}

/// A line that sits where the problem is: under the field, in the row. Not a banner.
struct InlineMessage: View {
    enum Kind { case error, warning, note }
    let kind: Kind
    let text: String

    var body: some View {
        Label {
            Text(text).typography(.secondary).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
        } icon: {
            Image(systemName: kind == .note ? "info.circle" : "exclamationmark.circle").typography(.secondary, .semibold)
        }
        .foregroundStyle(kind == .error ? Palette.danger : kind == .warning ? Palette.warning : .secondary)
    }
}

/// The floating notice under the chrome, for things that happened elsewhere (a capture failed, a save finished).
struct NoticeCapsule: View {
    let text: String
    var isError = false
    var onClose: (() -> Void)?

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: isError ? "exclamationmark.circle.fill" : "info.circle.fill")
                .foregroundStyle(isError ? Palette.warning : Palette.accent)
            Text(text).typography(.body).lineLimit(3).textSelection(.enabled)
            if let onClose {
                Button(action: onClose) { Image(systemName: "xmark").typography(.caption, .bold) }
                    .buttonStyle(.plain).foregroundStyle(.secondary).accessibilityLabel(L("关闭提示"))
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 9)
        .modifier(FloatingSurface(shape: Capsule()))
    }
}

/// Confirms an irreversible action that was started away from its own row (a context menu, a card):
/// floats under the chrome with the question, what will happen, and the two choices.
struct ConfirmCapsule: View {
    let title: String
    var message: String?
    let confirmTitle: String
    let onConfirm: () -> Void
    let onCancel: () -> Void

    var body: some View {
        HStack(spacing: Space.s) {
            Image(systemName: "exclamationmark.circle.fill").foregroundStyle(Palette.danger)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).typography(.body, .semibold)
                if let message { Text(message).typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true) }
            }
            Spacer(minLength: Space.s)
            QuietButton(title: L("取消"), action: onCancel).keyboardShortcut(.cancelAction)
            QuietButton(title: confirmTitle, role: .destructive, action: onConfirm)
        }
        .padding(.leading, Space.m).padding(.trailing, Space.xs).padding(.vertical, 10)
        .frame(maxWidth: 560)
        .floatingSurface(radius: Radius.large)
    }
}

extension View {
    /// Shows a `ConfirmCapsule` over the top of this view while `isPresented` is true.
    func confirmCapsule(_ title: String, isPresented: Binding<Bool>, message: String? = nil, confirmTitle: String = L("删除"),
                        action: @escaping () -> Void) -> some View {
        overlay(alignment: .top) {
            if isPresented.wrappedValue {
                ConfirmCapsule(title: title, message: message, confirmTitle: confirmTitle,
                               onConfirm: { confirmThenDismiss(action: action, dismiss: { isPresented.wrappedValue = false }) },
                               onCancel: { isPresented.wrappedValue = false })
                    .padding(.top, 58).padding(.horizontal, Space.page)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .animation(Motion.bouncy, value: isPresented.wrappedValue)
    }
}

/// Compact provider identity; destination colors are shared with DestinationTag.
struct SourceBadge: View {
    let title: String
    let destination: Destination
    var body: some View {
        Text(String(title.prefix(2))).typography(.headline, .semibold)
            .foregroundStyle(destination.tint)
            .frame(width: ControlHeight.field, height: ControlHeight.field)
            .background(destination.tint.opacity(0.12), in: RoundedRectangle(cornerRadius: Radius.small))
            .accessibilityHidden(true)
    }
}

/// A small proportion meter, with the numeric meaning supplied by adjacent labels.
struct ProportionMeter: View {
    let segments: [(Double, Color)]
    var body: some View {
        GeometryReader { bounds in
            HStack(spacing: 0) {
                ForEach(Array(segments.enumerated()), id: \.offset) { _, item in
                    item.1.frame(width: bounds.size.width * max(0, min(1, item.0)))
                }
                Spacer(minLength: 0)
            }
        }
        .frame(height: Space.xs).background(Surface.fill.color).clipShape(Capsule())
        .accessibilityHidden(true)
    }
}

/// Seven chronological daily call counts; no fabricated values when there were no calls.
struct ActivityBars: View {
    let values: [Int]
    var body: some View {
        HStack(alignment: .bottom, spacing: Space.xxs) {
            ForEach(Array(values.enumerated()), id: \.offset) { _, value in
                RoundedRectangle(cornerRadius: Radius.small / 2)
                    .fill(value > 0 ? Palette.accent.opacity(0.35) : Surface.fill.color)
                    .frame(height: max(1, CGFloat(value) / CGFloat(max(1, values.max() ?? 1)) * ControlHeight.button))
                    .frame(maxWidth: .infinity)
            }
        }
        .frame(height: ControlHeight.button, alignment: .bottom)
        .accessibilityElement(children: .ignore).accessibilityLabel(L("最近七天调用：") + values.map(String.init).joined(separator: L("、")))
    }
}

/// Capture item-based actions before dismissing their binding clears the selected item.
func confirmThenDismiss(action: () -> Void, dismiss: () -> Void) { action(); dismiss() }
