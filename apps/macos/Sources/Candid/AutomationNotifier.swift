import Foundation
import UserNotifications

@MainActor protocol AutomationNotifying {
    func requestAccess() async -> Bool
    func deliver(_ run: AutomationRun, title: String) async -> Bool
}
@MainActor final class AutomationNotifier: NSObject, AutomationNotifying, UNUserNotificationCenterDelegate {
    static let shared = AutomationNotifier()
    private var center: UNUserNotificationCenter { .current() }
    func requestAccess() async -> Bool {
        guard !AIRuntime.isTesting && !QASnapshot.isActive else { return false }
        center.delegate = self
        return (try? await center.requestAuthorization(options: [.alert, .sound])) ?? false
    }
    func deliver(_ run: AutomationRun, title: String) async -> Bool {
        guard run.policy.notify, run.notify_ready == true, !run.notified, run.state == "done", !run.references.isEmpty,
              run.result?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false else { return false }
        guard !AIRuntime.isTesting && !QASnapshot.isActive else { return false }
        let settings = await center.notificationSettings()
        guard settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional else { return false }
        let identifier = "candid-skill-" + run.id
        if await center.deliveredNotifications().contains(where: { $0.request.identifier == identifier }) { return true }
        let content = UNMutableNotificationContent()
        content.title = title; content.body = "处理完成，点开查看"
        content.userInfo = ["run_id": run.id]
        do { try await center.add(UNNotificationRequest(identifier: identifier, content: content, trigger: nil)); return true }
        catch { return false }
    }
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        guard let id = response.notification.request.content.userInfo["run_id"] as? String else { return }
        await MainActor.run { NotificationCenter.default.post(name: .candidAutomationRun, object: id) }
    }
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions { [.banner] }
}
