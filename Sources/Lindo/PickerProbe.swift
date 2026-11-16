import AppKit
import Foundation

/// Explicit development-only entry point. Opens and cancels our own folder dialog;
/// it never chooses a directory, starts a scan, or removes a file.
enum PickerProbe {
    static var requested: Bool { CommandLine.arguments.contains("--pickerprobe") }

    static func run(model: AppModel) {
        let env = ProcessInfo.processInfo.environment
        let localPrefix = FileManager.default.homeDirectoryForCurrentUser.appending(path: "Applications/Lindo-picker-probe-").path
        guard let temporary = env["APP_STORE"], temporary.hasPrefix("/tmp/"),
              let path = env["LINDO_PICKER_FIXTURE"],
              (path.contains("/Lindo/work/picker-probe-") || path.hasPrefix(localPrefix)), !model.isSample else {
            log("FAIL picker probe requires temporary APP_STORE and an explicit synthetic fixture")
            exit(1)
        }
        Task {
            try? await Task.sleep(for: .milliseconds(500))
            guard let owner = NSApp.windows.first(where: { $0.title == "Lindo" && $0.canBecomeMain && !($0 is NSPanel) }) else {
                log("FAIL picker probe has no main window")
                exit(1)
            }
            let fixture = URL(fileURLWithPath: path)
            model.sidebar = .duplicates
            model.duplicateRoot = fixture
            var beats = 0
            var maximumGap = 0.0
            var previous = ProcessInfo.processInfo.systemUptime
            let heartbeat = Task {
                while !Task.isCancelled {
                    let now = ProcessInfo.processInfo.systemUptime
                    maximumGap = max(maximumGap, (now - previous) * 1000)
                    previous = now
                    beats += 1
                    try? await Task.sleep(for: .milliseconds(10))
                }
            }
            var failures = 0
            try? await Task.sleep(for: .milliseconds(200))
            let idleResponsive = beats >= 5 && !model.isBusy && model.presentedFolderBrowser == nil
            log("\(idleResponsive ? "PASS" : "FAIL") cold startup idle heartbeat_ticks=\(beats), no picker and no scan")
            if !idleResponsive { failures += 1 }
            for pass in 1...3 {
                let before = beats
                let start = ProcessInfo.processInfo.systemUptime
                model.chooseFolder()
                let elapsed = (ProcessInfo.processInfo.systemUptime - start) * 1000
                let firstDialog = model.presentedFolderBrowser
                // The second request must not replace or duplicate the dialog.
                model.chooseFolder()
                try? await Task.sleep(for: .milliseconds(300))
                let dialog = model.presentedFolderBrowser
                let single = dialog != nil && firstDialog === dialog
                let responsive = model.choosingFolder && single && owner.attachedSheet == nil && beats - before >= 5 && elapsed < 100
                log("\(responsive ? "PASS" : "FAIL") picker pass=\(pass) return_ms=\(String(format: "%.2f", elapsed)) heartbeat_ticks=\(beats-before) single_inline_dialog=\(single)")
                if !responsive { failures += 1 }
                // Wait for an actual directory result: a live spinner alone is not a usable picker.
                let listingDeadline = ProcessInfo.processInfo.systemUptime + 6
                while dialog?.loading == true && ProcessInfo.processInfo.systemUptime < listingDeadline {
                    try? await Task.sleep(for: .milliseconds(20))
                }
                let listed = dialog?.loading == false && dialog?.error == nil && dialog?.canSelect == true
                log("\(listed ? "PASS" : "FAIL") picker directory listing completed and selection is enabled")
                if !listed { failures += 1 }
                model.cancelFolderChoice()
                let deadline = ProcessInfo.processInfo.systemUptime + 2
                while model.choosingFolder && ProcessInfo.processInfo.systemUptime < deadline {
                    try? await Task.sleep(for: .milliseconds(10))
                }
                let cancelled = !model.choosingFolder && !model.scanning && model.duplicateRoot == fixture && model.scanDates[.duplicates] == nil
                log("\(cancelled ? "PASS" : "FAIL") picker cancellation resets state and preserves folder without scanning")
                if !cancelled { failures += 1 }
                try? await Task.sleep(for: .milliseconds(150))
            }
            heartbeat.cancel()
            log("PICKER_METRICS heartbeat_max_gap_ms=\(String(format: "%.2f", maximumGap)) total_ticks=\(beats)")
            exit(failures == 0 ? 0 : 1)
        }
    }

    private static func log(_ message: String) {
        FileHandle.standardError.write(Data((message + "\n").utf8))
    }
}
