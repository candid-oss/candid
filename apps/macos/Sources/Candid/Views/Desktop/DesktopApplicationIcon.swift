import SwiftUI
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
        case "application": "应用切换"
        case "window": "窗口变化"
        case "focus": "焦点变化"
        case "selection": "选区变化"
        case "content": "内容变化"
        case "timer": "定时"
        case "manual": "手动"
        case "system": "系统状态"
        default: nil
        }
    }

    static func textStatus(_ raw: String?) -> String? {
        switch raw {
        case "composing": "输入法尚未确认"
        case "partial": "文字不完整"
        case "no_text": "没有可读文字"
        case "permission_denied": "需要辅助功能授权"
        case "protected": "受保护"
        case "timeout": "读取超时"
        case "unavailable": "不可读取"
        case "not_allowed": "未允许读取文字"
        default: nil
        }
    }

    static func focusedRole(_ raw: String?) -> String? {
        guard let raw, !raw.isEmpty else { return nil }
        switch raw {
        case "AXTextArea": return "多行文本框"
        case "AXTextField": return "文本框"
        case "AXSecureTextField": return "受保护文本框"
        case "AXStaticText": return "文本"
        case "AXButton": return "按钮"
        case "AXCheckBox": return "复选框"
        case "AXRadioButton": return "单选项"
        case "AXRadioGroup": return "单选组"
        case "AXPopUpButton": return "弹出菜单"
        case "AXComboBox": return "组合框"
        case "AXWebArea": return "网页"
        case "AXLink": return "链接"
        case "AXWindow": return "窗口"
        case "AXScrollArea": return "滚动区域"
        case "AXList": return "列表"
        case "AXTable": return "表格"
        case "AXRow": return "行"
        case "AXCell": return "单元格"
        case "AXOutline": return "树状列表"
        case "AXMenuBar": return "菜单栏"
        case "AXMenu": return "菜单"
        case "AXMenuItem": return "菜单项"
        case "AXTabGroup": return "标签页"
        case "AXToolbar": return "工具栏"
        case "AXImage": return "图片"
        case "AXSlider": return "滑块"
        case "AXGroup": return "分组"
        case "AXSplitGroup": return "分栏"
        case "AXSplitter": return "分隔条"
        case "AXSheet": return "面板"
        case "AXDialog": return "对话框"
        case "AXProgressIndicator": return "进度条"
        default: return raw
        }
    }
}
