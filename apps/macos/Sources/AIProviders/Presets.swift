import Foundation

public struct ProviderPreset: Sendable, Identifiable, Hashable {
    public var id: String
    public var name: String
    public var api: ProviderAPI
    public var baseURL: String
    public var destination: Destination
    public var needsKey: Bool
    public var hint: String

    public func makeProvider() -> AIProvider {
        let pid = id == "custom" ? UUID().uuidString.lowercased() : id + "-" + String(UUID().uuidString.prefix(6)).lowercased()
        return AIProvider(providerID: pid, name: name, api: api, baseURL: baseURL,
                          secretRef: needsKey ? "provider.\(pid)" : nil, destination: destination)
    }

    public static let all: [ProviderPreset] = [
        .init(id: "lmstudio", name: "LM Studio", api: .openaiChat, baseURL: "http://localhost:1234/v1", destination: .onDevice,
              needsKey: false, hint: "本机服务，在 LM Studio 里打开“本地服务器”"),
        .init(id: "ollama", name: "Ollama", api: .ollama, baseURL: "http://localhost:11434", destination: .onDevice,
              needsKey: false, hint: "本机服务，能读到工具和图片能力"),
        .init(id: "vllm", name: "vLLM / 自建服务", api: .openaiChat, baseURL: "http://localhost:8000/v1", destination: .selfHosted,
              needsKey: false, hint: "你自己的 Mac、NAS 或服务器"),
        .init(id: "openai", name: "OpenAI", api: .openaiChat, baseURL: "https://api.openai.com/v1", destination: .cloud,
              needsKey: true, hint: "云端，内容会发往 OpenAI"),
        .init(id: "anthropic", name: "Anthropic", api: .anthropicMessages, baseURL: "https://api.anthropic.com", destination: .cloud,
              needsKey: true, hint: "云端，内容会发往 Anthropic"),
        .init(id: "openrouter", name: "OpenRouter", api: .openaiChat, baseURL: "https://openrouter.ai/api/v1", destination: .cloud,
              needsKey: true, hint: "云端聚合，目录里带价格与能力"),
        .init(id: "custom", name: "自定义", api: .openaiChat, baseURL: "http://", destination: .selfHosted,
              needsKey: false, hint: "任何兼容 OpenAI 或 Anthropic 接口的服务")
    ]
}

/// An engine that runs inside Candid or its local library service rather than through a model API.
public struct BuiltinEngine: Sendable, Identifiable, Hashable {
    public var id: String
    public var title: String
    /// What actually runs, as shipped; shown so the user knows which model does the work.
    public var detail: String
    public var destination: Destination = .onDevice

    public static let appleText = BuiltinEngine(id: "apple-vision-text", title: "系统文字识别",
        detail: "macOS 自带的 Vision 文字识别，在这台电脑上运行，带每行位置")
    /// Model ids mirror `services/voice/asr/runtime.py` and `services/voice/asr/speaker_models.py`.
    public static let qwenASR = BuiltinEngine(id: "hub-qwen3-asr", title: "Qwen3-ASR 0.6B",
        detail: "mlx-community/Qwen3-ASR-0.6B-8bit，由本机资料库服务运行")
    public static let mossDiarization = BuiltinEngine(id: "hub-moss-campp", title: "MOSS 分段 + CAM++ 声纹",
        detail: "vanch007/mlx-MOSS-Transcribe-Diarize-8bit 与 Wespeaker CAM++，由本机资料库服务运行")
}

/// Model roles shown in settings. Ids follow timeline-v1 §4.9 `ai_roles.role`.
public struct ModelRoleInfo: Sendable, Identifiable, Hashable {
    public enum Group: String, Sendable { case understanding = "理解与整理", perception = "识别与转写" }

    public var id: String
    public var title: String
    public var detail: String
    public var group: Group = .understanding
    public var needsTools: Bool = false
    public var needsImages: Bool = false
    /// Built-in engines that can fill this role; the first is the default when nothing is chosen.
    public var engines: [BuiltinEngine] = []
    /// Whether a model from the sources may be chosen (false while only built-in engines are wired up).
    public var acceptsModels: Bool = true
    /// Whether the role sends a prompt the user may edit.
    public var hasPrompt: Bool = true
    /// Whether a feature in the app already uses this role.
    public var inUse: Bool = false

    public var defaultEngine: BuiltinEngine? { engines.first }

    public static let catalog: [ModelRoleInfo] = [
        .init(id: "vision", title: "图像理解", detail: "截图整理成 Markdown，以及按截图回答问题；需要图片输入",
              group: .perception, needsImages: true, inUse: true),
        .init(id: "ocr", title: "文字识别", detail: "截图文字提取。默认用系统识别；也可以交给支持图片的模型",
              group: .perception, needsImages: true, engines: [.appleText], inUse: true),
        .init(id: "asr", title: "语音转写", detail: "单人录音、口述和短语音转成文字",
              group: .perception, engines: [.qwenASR], acceptsModels: true, hasPrompt: false, inUse: true),
        .init(id: "diarization", title: "多人分段与声纹", detail: "会议和多人对话：分段、区分说话人、匹配已注册声纹",
              group: .perception, engines: [.mossDiarization], acceptsModels: true, hasPrompt: false, inUse: true),
        .init(id: "agent_chat", title: "对话 Agent", detail: "界面里的内置对话；需要工具调用", needsTools: true, inUse: true),
        .init(id: "title", title: "自动标题", detail: "根据转写生成可修改的标题", inUse: true),
        .init(id: "summary", title: "摘要", detail: "片段与每日回顾的文字整理", inUse: true),
        .init(id: "episode", title: "片段整理", detail: "为屏幕片段生成标题和摘要", inUse: true),
        .init(id: "relevance", title: "相关性初筛", detail: "录音和屏幕文字的保留/忽略判断"),
        .init(id: "embedding", title: "文本嵌入", detail: "语义检索用的向量模型", hasPrompt: false)
    ]

    public static func info(_ id: String) -> ModelRoleInfo? { catalog.first { $0.id == id } }
}
