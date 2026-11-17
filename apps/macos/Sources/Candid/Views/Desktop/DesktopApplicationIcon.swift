import SwiftUI
import CandidLocalization
import AppKit
import DesktopCore

struct DesktopApplicationIcon: View {
    let bundleID: String
    let fallback: String
    var size: CGFloat = 28
    var body: some View {
        Group {
            if let icon = DesktopIconCache.image(for: bundleID) {
                Image(nsImage: icon).resizable().interpolation(.high).scaledToFit()
            } else {
                Image(systemName: fallback).font(.system(size: size * 0.6)).foregroundStyle(.secondary)  // style-exempt: glyph scales with its container
            }
        }.frame(width: size, height: size).accessibilityHidden(true)
    }
}

@MainActor enum DesktopIconCache {
    private static let images: NSCache<NSString, NSImage> = {
        let cache = NSCache<NSString, NSImage>(); cache.countLimit = 128; return cache
    }()
    private static var missing: Set<String> = []
    static func image(for bundle: String) -> NSImage? {
        guard !bundle.isEmpty, !missing.contains(bundle) else { return nil }
        if let cached = images.object(forKey: bundle as NSString) { return cached }
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundle) else {
            if missing.count < 256 { missing.insert(bundle) }; return nil
        }
        let icon = NSWorkspace.shared.icon(forFile: url.path)
        images.setObject(icon, forKey: bundle as NSString); return icon
    }
}

enum DesktopRecordPresentation {
    static func triggerTitle(_ raw: String?) -> String? {
        switch raw {
        case "application": L("应用切换")
        case "window": L("窗口变化")
        case "focus": L("焦点变化")
        case "selection": L("选区变化")
        case "content": L("内容变化")
        case "timer": L("定时")
        case "manual": L("手动")
        case "system": L("系统状态")
        default: nil
        }
    }

    static func textStatus(_ raw: String?) -> String? {
        switch raw {
        case "composing": L("输入法尚未确认")
        case "partial": L("文字不完整")
        case "no_text": L("没有可读文字")
        case "permission_denied": L("需要辅助功能授权")
        case "protected": L("受保护")
        case "timeout": L("读取超时")
        case "unavailable": L("不可读取")
        case "not_allowed": L("未允许读取文字")
        default: nil
        }
    }

    static func focusedRole(_ raw: String?) -> String? {
        guard let raw, !raw.isEmpty else { return nil }
        switch raw {
        case "AXTextArea": return L("多行文本框")
        case "AXTextField": return L("文本框")
        case "AXSecureTextField": return L("受保护文本框")
        case "AXStaticText": return L("文本")
        case "AXButton": return L("按钮")
        case "AXCheckBox": return L("复选框")
        case "AXRadioButton": return L("单选项")
        case "AXRadioGroup": return L("单选组")
        case "AXPopUpButton": return L("弹出菜单")
        case "AXComboBox": return L("组合框")
        case "AXWebArea": return L("网页")
        case "AXLink": return L("链接")
        case "AXWindow": return L("窗口")
        case "AXScrollArea": return L("滚动区域")
        case "AXList": return L("列表")
        case "AXTable": return L("表格")
        case "AXRow": return L("行")
        case "AXCell": return L("单元格")
        case "AXOutline": return L("树状列表")
        case "AXMenuBar": return L("菜单栏")
        case "AXMenu": return L("菜单")
        case "AXMenuItem": return L("菜单项")
        case "AXTabGroup": return L("标签页")
        case "AXToolbar": return L("工具栏")
        case "AXImage": return L("图片")
        case "AXSlider": return L("滑块")
        case "AXGroup": return L("分组")
        case "AXSplitGroup": return L("分栏")
        case "AXSplitter": return L("分隔条")
        case "AXSheet": return L("面板")
        case "AXDialog": return L("对话框")
        case "AXProgressIndicator": return L("进度条")
        default: return raw
        }
    }
}
