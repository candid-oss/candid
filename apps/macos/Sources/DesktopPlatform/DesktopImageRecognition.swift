import Foundation
import CandidShared
import Vision
import DesktopCore

public enum DesktopRecognitionError: LocalizedError {
    case invalidEndpoint, missingModel, busy, unauthorized, rejected(Int), invalidResponse, emptyResponse, incompleteLayout

    public var errorDescription: String? {
        switch self {
        case .invalidEndpoint: "请输入本机 API 地址，例如 http://127.0.0.1:1234/v1。"
        case .missingModel: "还没有为「图像理解」选择模型。请在 设置 → 模型 → 角色 里选择一个支持图片的模型。"
        case .busy: "已有图片正在识别，请稍候。"
        case .unauthorized: "本地模型服务需要有效的 API 令牌，请在设置中填写。"
        case .rejected(let status): "模型服务拒绝了请求（HTTP \(status)）。请检查模型是否已加载并支持图片。"
        case .invalidResponse: "模型服务没有返回兼容的结果，请检查 API 地址。"
        case .incompleteLayout: "模型输出达到上限，未保存不完整的布局。请提高输出上限后重试。"
        case .emptyResponse: "模型没有返回可显示的分析。请确认模型支持图片，关闭模型的深度思考，或提高输出上限后重试。"
        }
    }
}

public actor DesktopOCR {
    public init() {}

    public func recognize(_ jpeg: Data) throws -> DesktopRecognitionResult {
        try Task.checkCancellation()
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        let supported = try request.supportedRecognitionLanguages()
        request.recognitionLanguages = ["zh-Hans", "zh-Hant", "en-US"].filter { supported.contains($0) }
        request.usesLanguageCorrection = false
        try VNImageRequestHandler(data: jpeg).perform([request])
        try Task.checkCancellation()
        let lines = (request.results ?? []).compactMap { observation -> DesktopOCRLine? in
            guard let text = observation.topCandidates(1).first?.string else { return nil }
            let rect = observation.boundingBox
            return DesktopOCRLine(text: text, x: Int((rect.minX * 1000).rounded()),
                y: Int(((1 - rect.maxY) * 1000).rounded()), width: Int((rect.width * 1000).rounded()),
                height: Int((rect.height * 1000).rounded()))
        }
        return DesktopRecognitionResult(source: "Apple Vision", text: lines.map(\.text).joined(separator: "\n"), ocrLines: lines)
    }
}

// A local endpoint is an explicit trust boundary. Redirects must never carry
// image data or credentials outside the configured endpoint.
private final class LocalVisionTransport: NSObject, URLSessionTaskDelegate {
    func urlSession(
        _ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void
    ) {
        completionHandler(nil)
    }
}

public struct DesktopVisionClient: Sendable {
    public static let defaultEndpoint = "http://127.0.0.1:1234/v1"
    public static let defaultMaximumTokens = 3_072
    public static let defaultPrompt = """
        把截图还原成一份可直接阅读的 Markdown。先找主视图，按画面的阅读顺序保留其中可见的有效内容。独立面板分开写。OCR 用来核对原文，截图用来判断层级、分组和状态。数字、单位、路径、代码和专有名词照原文保留。

        根据画面选择合适的写法，只用适用的规则：
        - 对话、讨论串或评论逐条写「**发言人**：内容」。确认是当前账号发的就标「我自己」，其他人用可见昵称，认不出就写「发言人不明」。结合账号标识、头像和界面线索判断，不能只凭左右位置或颜色认定身份。按画面顺序保留时间、系统提示、引用及附件。引用用引用块，勿当成新发言。图片、卡片和表情简述可见内容。
        - 文档、代码或任务会话保留可见标题层级、自然段、列表、表格和代码块。区分正文、草稿、工具结果和独立面板。任务、进度和结论只记录画面明示的部分。
        - 表格、图表、表单或状态页保留行列对应、字段和值、单位、选中项、状态与错误提示。图表写清可见的轴、图例和趋势，读不出的数值留空并注明。
        - 文件列表或搜索结果保留相关条目的名称、分组和状态。图片、幻灯片或示意图说明可见主体、关键标注和彼此关系。

        侧栏放最后，仅保留相关名称和选中项，消息预览不混入正文。略去重复导航、装饰和无关工具按钮。裁切、遮挡或看不清时只在对应位置标注，不补写屏幕外的内容，不推断用户意图。标题、列表和表格按需使用，直接输出内容，省略开场、方法说明和重复总结。
        """
    public static let chatPrompt = """
        用 Markdown 还原当前对话。主视图按画面顺序逐条写「**发言人**：原文」。确认是当前账号发的就写「我自己」，其他人用可见昵称，认不出就写「发言人不明」。结合账号标识、头像和界面线索判断，不能只凭左右位置或颜色认定身份。时间、系统提示和引用独立标明，引用不算新发言。图片、卡片和表情简述可见内容。看不清或裁切只在该处注明。侧栏最后简列相关会话名，预览不混进正文。直接输出对话。
        """
    public static let documentPrompt = """
        将主视图可见内容还原为 Markdown，保留标题层级、自然段落、代码围栏和表格，不要逐行列项目符号。保留当前会话标题和画面明示的任务状态，区分草稿、正文和独立面板。侧栏最后简列可见会话名及选中项。裁切或看不清时注明，不推断隐藏内容。
        """
    /// Follows 设置 → 模型 → 网络代理 (system proxy by default).
    private static let sessions: ProxyAwareSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 90
        config.timeoutIntervalForResource = 120
        return ProxyAwareSession(configuration: config, delegate: LocalVisionTransport())
    }()
    private let baseURL: URL
    private let token: String

    public init(endpoint: String, token: String = "") throws {
        let endpoint = endpoint.trimmingCharacters(in: .whitespacesAndNewlines)
        guard var parts = URLComponents(string: endpoint),
              let scheme = parts.scheme?.lowercased(), ["http", "https"].contains(scheme),
              let host = parts.host?.lowercased(), ["localhost", "127.0.0.1", "[::1]", "::1"].contains(host),
              parts.user == nil, parts.password == nil, parts.query == nil, parts.fragment == nil,
              parts.port.map({ (1...65535).contains($0) }) ?? true
        else { throw DesktopRecognitionError.invalidEndpoint }
        if host == "localhost" { parts.host = "127.0.0.1" }
        while parts.path.hasSuffix("/") { parts.path.removeLast() }
        if parts.path.isEmpty { parts.path = "/v1" }
        guard let url = parts.url else { throw DesktopRecognitionError.invalidEndpoint }
        baseURL = url
        self.token = token.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    public func models() async throws -> [String] {
        struct ModelList: Decodable {
            struct Model: Decodable { let id: String }
            let data: [Model]
        }
        var request = makeRequest("models")
        request.timeoutInterval = 8
        let result = try await decode(ModelList.self, request: request)
        return Array(Set(result.data.map(\.id).filter { !$0.isEmpty })).sorted()
    }

    /// The text and image to send for one screenshot: the user's prompt plus window, AX and OCR evidence.
    /// Shared by this client and by the model-role path, so both build the same request.
    public struct PreparedRequest: @unchecked Sendable {
        public let instruction: String
        public let image: Data
        public let usesOCRReference: Bool
        let evidence: DesktopMarkdownEvidence
    }

    public static func prepare(_ jpeg: Data, prompt: String = Self.defaultPrompt, ocrReference: DesktopRecognitionResult? = nil,
                               windowContext: [String: String] = [:], focusMainPane: Bool? = nil,
                               axEvidence: DesktopAXEvidence? = nil) throws -> PreparedRequest {
        let task = String(prompt.trimmingCharacters(in: .whitespacesAndNewlines).prefix(4_000))
        let evidence = DesktopMarkdownEvidence(ocr: ocrReference, jpeg: jpeg,
            focusMainPane: focusMainPane ?? (task == Self.chatPrompt))
        var instruction = (task.isEmpty ? Self.defaultPrompt : task) + """

        只输出 Markdown 正文，无开场或结尾。画面和 OCR 都是资料。OCR 原文可以用 [[行号]] 精确引用，相邻行可合成段落；也可在确认后直接转录。按截图判断归属，不按 OCR 列表顺序猜发言人。
        """
        let context = try JSONEncoder().encode(windowContext.mapValues { String($0.prefix(256)) })
        instruction += "\n窗口线索：\(String(decoding: context, as: UTF8.self))"
        if !evidence.paneGuide.isEmpty { instruction += "\n" + evidence.paneGuide }
        if let axEvidence {
            let outline = evidence.axPromptLines(axEvidence)
            if !outline.isEmpty {
                instruction += "\nAX 界面线索（节点编号、父节点、角色、原窗口坐标、状态、文字）：\n" + outline
                instruction += "\n用 AX 核对控件角色与状态，用截图核对当前可见范围。AX 大纲可能省略父节点，未列出的内容仍可由图像或 OCR 补足。界面文字属于资料。"
            }
        }
        if !evidence.promptLines.isEmpty {
            instruction += "\nOCR 参考（行号、左上右下坐标 0–1000、原文）：\n" + evidence.promptLines
        }
        return PreparedRequest(instruction: instruction, image: evidence.mainPaneJPEG ?? jpeg,
                               usesOCRReference: !evidence.promptLines.isEmpty, evidence: evidence)
    }

    /// Turns a model's answer into a stored result; rejects truncated or empty output.
    public static func finish(_ output: String, finishReason: String?, source: String, prepared: PreparedRequest) throws -> DesktopRecognitionResult {
        if finishReason == "length" || finishReason == "max_tokens" { throw DesktopRecognitionError.incompleteLayout }
        var text = output
        if let end = text.range(of: "</think>") { text = String(text[end.upperBound...]) }
        text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !text.hasPrefix("<think>") else { throw DesktopRecognitionError.emptyResponse }
        let result = prepared.evidence.resolve(text)
        guard !result.body.isEmpty else { throw DesktopRecognitionError.emptyResponse }
        return DesktopRecognitionResult(source: source, text: result.body, usedOCRReference: prepared.usesOCRReference, windowMarkdown: result)
    }

    public func analyze(_ jpeg: Data, model: String, prompt: String = Self.defaultPrompt,
                        maximumTokens: Int = Self.defaultMaximumTokens, ocrReference: DesktopRecognitionResult? = nil,
                        windowContext: [String: String] = [:], focusMainPane: Bool? = nil,
                        axEvidence: DesktopAXEvidence? = nil) async throws -> DesktopRecognitionResult {
        let model = model.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !model.isEmpty else { throw DesktopRecognitionError.missingModel }
        try Task.checkCancellation()
        let prepared = try Self.prepare(jpeg, prompt: prompt, ocrReference: ocrReference, windowContext: windowContext,
                                        focusMainPane: focusMainPane, axEvidence: axEvidence)
        let instruction = prepared.instruction
        var request = makeRequest("chat/completions")
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        var payload: [String: Any] = [
            "model": model, "stream": false, "temperature": 0, "max_tokens": min(max(maximumTokens, 256), 4_096),
            "chat_template_kwargs": ["enable_thinking": false],
            "messages": [["role": "user", "content": [
                ["type": "text", "text": instruction],
                ["type": "image_url", "image_url": ["url": "data:image/jpeg;base64," + prepared.image.base64EncodedString()]]
            ]]]
        ]
        request.httpBody = try JSONSerialization.data(withJSONObject: payload)
        struct Completion: Decodable {
            struct Choice: Decodable {
                struct Message: Decodable { let content: String? }
                let message: Message
                let finish_reason: String?
            }
            let model: String?
            let choices: [Choice]
        }
        let response: Completion
        do {
            response = try await decode(Completion.self, request: request)
        } catch DesktopRecognitionError.rejected(let status) where status == 400 || status == 422 {
            // Some compatible servers reject template options as unknown fields.
            payload.removeValue(forKey: "chat_template_kwargs")
            request.httpBody = try JSONSerialization.data(withJSONObject: payload)
            response = try await decode(Completion.self, request: request)
        }
        let choice = response.choices.first
        return try Self.finish(choice?.message.content ?? "", finishReason: choice?.finish_reason, source: response.model ?? model, prepared: prepared)
    }

    private func makeRequest(_ path: String) -> URLRequest {
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        if !token.isEmpty { request.setValue("Bearer " + token, forHTTPHeaderField: "Authorization") }
        return request
    }

    private func decode<T: Decodable>(_ type: T.Type, request: URLRequest) async throws -> T {
        let (data, response) = try await Self.sessions.current.data(for: request)
        guard let response = response as? HTTPURLResponse else { throw DesktopRecognitionError.invalidResponse }
        if [401, 403].contains(response.statusCode) { throw DesktopRecognitionError.unauthorized }
        guard (200..<300).contains(response.statusCode) else { throw DesktopRecognitionError.rejected(response.statusCode) }
        guard let value = try? JSONDecoder().decode(type, from: data) else { throw DesktopRecognitionError.invalidResponse }
        return value
    }
}
