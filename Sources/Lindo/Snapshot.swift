import AppKit
import IOKit.pwr_mgt
import LindoKit
import SwiftUI

enum Snapshot {
    private static var started = false
    private static var captureFailures = 0

    static func runIfRequested(model: AppModel) {
        guard !started else { return }
        started = true
        let env = ProcessInfo.processInfo.environment
        if model.isSample, let look = env["APP_TEST_APPEARANCE"], ["light", "dark"].contains(look) {
            NSApp.appearance = NSAppearance(named: look == "dark" ? .darkAqua : .aqua)
        }
        if PickerProbe.requested { PickerProbe.run(model: model); return }
        if CommandLine.arguments.contains("--selftest") {
            Task { await runSelfTest(model) }
            return
        }
        guard let directory = env["APP_SNAPSHOT"] else { return }
        guard isTemporary(env["APP_STORE"]), isTemporary(directory), model.isSample else {
            log("SNAPSHOT refused: APP_SAMPLE=1 and APP_STORE/APP_SNAPSHOT in /tmp required")
            exit(1)
        }
        Task {
            var activity: IOPMAssertionID = 0
            IOPMAssertionDeclareUserActivity("Lindo snapshots" as CFString, kIOPMUserActiveLocal, &activity)
            do { try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true) }
            catch { log("SNAPSHOT failed to create output directory: \(error.localizedDescription)"); exit(1) }
            if let window = NSApp.windows.first(where: { $0.title == "Lindo" }) {
                window.setContentSize(env["APP_SNAPSHOT_SIZE"] == "compact"
                    ? NSSize(width: 980, height: 680) : NSSize(width: 1140, height: 760))
            }
            try? await Task.sleep(for: .seconds(1))
            for mode in ["light", "dark"] {
                NSApp.appearance = NSAppearance(named: mode == "light" ? .aqua : .darkAqua)
                for page in SidebarItem.allCases {
                    model.sidebar = page
                    try? await Task.sleep(for: .milliseconds(600))
                    await capture("\(directory)/\(page.rawValue)-\(mode).png")
                }
                await captureUXStates(model: model, directory: directory, mode: mode)
                await captureReviewStates(model: model, directory: directory, mode: mode)
                model.sidebar = .duplicates
                try? await Task.sleep(for: .milliseconds(300))
                model.selectDuplicateCopies()
                await capture("\(directory)/duplicates-selected-\(mode).png")
                model.reviewSelection()
                try? await Task.sleep(for: .milliseconds(500))
                await capture("\(directory)/duplicate-confirmation-\(mode).png")
                model.showConfirmation = false
                model.selected = []
                if let group = model.duplicateGroups.first, let copy = group.files.first(where: { !model.isKeeper($0.id) }) {
                    model.selected = [copy.id]
                    model.reviewSelection()
                    try? await Task.sleep(for: .milliseconds(500))
                    await capture("\(directory)/duplicate-partial-confirmation-\(mode).png")
                    model.showConfirmation = false
                    model.cleaning = true
                    try? await Task.sleep(for: .milliseconds(400))
                    await capture("\(directory)/trash-progress-\(mode).png")
                    model.sidebar = .overview
                    try? await Task.sleep(for: .milliseconds(400))
                    await capture("\(directory)/trash-progress-overview-\(mode).png")
                    model.cleaning = false
                    model.selected = []
                }
                model.sidebar = .largeFiles
                try? await Task.sleep(for: .milliseconds(300))
                let saved = model.items[.largeFiles]
                let date = model.scanDates[.largeFiles]
                model.items[.largeFiles] = nil
                model.scanDates[.largeFiles] = nil
                try? await Task.sleep(for: .milliseconds(400))
                await capture("\(directory)/empty-\(mode).png")
                model.items[.largeFiles] = saved
                model.scanDates[.largeFiles] = date
                model.selected = Set(saved?.prefix(2).map(\.id) ?? [])
                model.reviewSelection()
                try? await Task.sleep(for: .milliseconds(500))
                await capture("\(directory)/confirmation-\(mode).png")
                model.showConfirmation = false
                model.selected = []
                for page in [SidebarItem.cleanup, .duplicates, .applications] {
                    model.sidebar = page
                    let savedItems = model.items[page]
                    let savedDate = model.scanDates[page]
                    let savedWarnings = model.warnings[page]
                    let savedGroups = model.duplicateGroups
                    model.items[page] = nil
                    model.scanDates[page] = nil
                    model.warnings[page] = nil
                    if page == .duplicates { model.duplicateGroups = [] }
                    try? await Task.sleep(for: .milliseconds(400))
                    await capture("\(directory)/empty-\(page.rawValue)-\(mode).png")
                    model.items[page] = savedItems
                    model.scanDates[page] = savedDate
                    model.warnings[page] = savedWarnings
                    model.duplicateGroups = savedGroups
                }
                try? await Task.sleep(for: .milliseconds(300))
                let settings = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 560, height: 560), styleMask: [.titled, .closable], backing: .buffered, defer: false)
                settings.title = "Impostazioni di Lindo"
                settings.isReleasedWhenClosed = false
                settings.contentView = NSHostingView(rootView: SettingsView().tint(Palette.accent))
                settings.center()
                settings.makeKeyAndOrderFront(nil)
                try? await Task.sleep(for: .milliseconds(400))
                await capture("\(directory)/settings-\(mode).png", window: settings, probeAreas: ["settings"])
                settings.orderOut(nil)

                let browser = FolderBrowserModel(initialURL: URL(fileURLWithPath: "/Cartella di esempio"), loader: { _ in
                    throw NSError(domain: "LindoPreview", code: 1, userInfo: [NSLocalizedDescriptionKey: "Controlla il consenso di macOS oppure scegli una cartella locale."])
                })
                let chooser = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 660, height: 500), styleMask: [.titled, .closable], backing: .buffered, defer: false)
                chooser.title = "Scegli una cartella"
                chooser.isReleasedWhenClosed = false
                chooser.contentView = NSHostingView(rootView: FolderBrowserView(model: browser, onCancel: {}, onSelect: { _ in }).tint(Palette.accent))
                chooser.center()
                chooser.makeKeyAndOrderFront(nil)
                try? await Task.sleep(for: .milliseconds(400))
                await capture("\(directory)/folder-chooser-\(mode).png", window: chooser, probeAreas: ["folder-browser"])
                browser.stop()
                chooser.orderOut(nil)
            }
            guard captureFailures == 0 else { log("SNAPSHOT failed: \(captureFailures) missing or invalid images"); exit(1) }
            log("SNAPSHOT done \(directory)")
            NSApp.terminate(nil)
        }
    }

    private static func isTemporary(_ path: String?) -> Bool {
        guard let path else { return false }
        let normalized = URL(fileURLWithPath: path).standardizedFileURL.path
        return normalized.hasPrefix("/tmp/") || normalized.hasPrefix("/private/tmp/")
    }

    private static func captureUXStates(model: AppModel, directory: String, mode: String) async {
        model.sidebar = .largeFiles
        try? await Task.sleep(for: .milliseconds(300))
        model.selected = Set(model.currentItems.prefix(1).map(\.id))
        model.query = "no-matching-file"
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/filtered-selection-\(mode).png")
        model.clearSearch()
        model.selected = []
        model.sidebar = .duplicates
        try? await Task.sleep(for: .milliseconds(300))
        model.selectDuplicateCopies()
        model.query = "Presentazione"
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/filtered-duplicates-\(mode).png")
        model.clearSearch()
        model.selected = []

        // Simulate presentation only: sample mode never starts filesystem work.
        model.activePage = .duplicates
        model.scanning = true
        model.progressText = "Confronto dei file: 12 elementi esaminati"
        model.sidebar = .overview
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/scan-navigation-\(mode).png")
        model.sidebar = .duplicates
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/scan-refresh-\(mode).png")
        let groups = model.duplicateGroups
        let date = model.scanDates[.duplicates]
        model.duplicateGroups = []
        model.scanDates[.duplicates] = nil
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/scan-first-use-\(mode).png")
        model.scanCancelling = true
        model.progressText = "Interruzione in corso…"
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/scan-cancelling-\(mode).png")
        model.duplicateGroups = groups
        model.scanDates[.duplicates] = date
        model.scanning = false
        model.scanCancelling = false
        model.activePage = nil
        model.progressText = ""
    }

    private static func captureReviewStates(model: AppModel, directory: String, mode: String) async {
        let savedItems = model.items, savedDates = model.scanDates, savedWarnings = model.warnings
        let savedOutcomes = model.scanOutcomes, savedGroups = model.duplicateGroups
        let savedKeepers = model.duplicateKeepers, savedDisk = model.disk, savedMap = model.spaceReport
        for page in [SidebarItem.cleanup, .duplicates] {
            model.sidebar = page
            model.items[page] = []
            if page == .duplicates { model.duplicateGroups = [] }
            model.scanDates[page] = Date()
            model.scanOutcomes[page] = .interrupted
            model.warnings[page] = ["Analisi interrotta dall’utente."]
            try? await Task.sleep(for: .milliseconds(300))
            await capture("\(directory)/interrupted-\(page.rawValue)-\(mode).png")
        }
        model.scanOutcomes[.duplicates] = .partial
        model.warnings[.duplicates] = ["Una cartella non è accessibile."]
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/partial-empty-duplicates-\(mode).png")
        model.sidebar = .overview
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/partial-overview-\(mode).png")
        model.disk = DiskSpace()
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/unknown-disk-\(mode).png")
        model.disk = savedDisk
        model.sidebar = .spaceMap
        let root = model.spaceRoot
        model.spaceReport = SpaceReport(root: root, entries: [
            SpaceEntry(url: root.appending(path: "original.bin"), allocatedBytes: 4096, logicalBytes: 4096, fileCount: 1, isDirectory: false, isPartial: false),
            SpaceEntry(url: root.appending(path: "altro-nome.bin"), allocatedBytes: 0, logicalBytes: 0, fileCount: 0, isDirectory: false, isPartial: false, alreadyCountedLinks: 1)
        ], allocatedBytes: 4096, logicalBytes: 4096, fileCount: 1, inspectedCount: 2, warnings: [], isPartial: false, cancelled: false, duration: 0)
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/hardlinks-map-\(mode).png")
        let files = (0..<10_000).map { index in
            ScanItem(id: "stress-\(index)", url: (model.duplicateRoot ?? URL(fileURLWithPath: "/Users/demo/Documents")).appending(path: "Copia-\(index)/Documento.pdf"), title: "Documento.pdf", bytes: 1024, category: .largeFiles)
        }
        let group = DuplicateGroup(id: "stress", files: files, keeperID: files[0].id, digest: "sample")
        model.sidebar = .duplicates
        model.duplicateGroups = [group]
        model.duplicateKeepers = [group.id: group.keeperID]
        model.items[.duplicates] = files
        model.warnings[.duplicates] = []
        model.scanOutcomes[.duplicates] = .complete
        model.selected = []
        model.query = ""
        model.appearedDuplicateRows = []
        let started = ProcessInfo.processInfo.systemUptime
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/ten-thousand-duplicates-\(mode).png")
        let count = model.appearedDuplicateRows.count
        log("LAZY_ROWS \(mode): \(count) appeared of 10000; capture elapsed \(ProcessInfo.processInfo.systemUptime - started)s")
        if count == 0 || count >= 200 { captureFailures += 1; log("LAZY_ROWS failed viewport bound") }
        model.items = savedItems
        model.scanDates = savedDates
        model.warnings = savedWarnings
        model.scanOutcomes = savedOutcomes
        model.duplicateGroups = savedGroups
        model.duplicateKeepers = savedKeepers
        model.spaceReport = savedMap
        model.pageChanged()
    }

    private static func capture(_ path: String, window supplied: NSWindow? = nil, probeAreas: Set<String>? = nil) async {
        guard let window = supplied ?? NSApp.windows.first(where: { $0.canBecomeMain && !$0.isSheet }) else {
            log("SNAPSHOT missing window")
            captureFailures += 1
            return
        }
        // Ask LaunchServices to activate this already-running QA bundle. Merely
        // calling NSApp.activate() can leave captures inactive on current macOS.
        for _ in 0..<3 {
            if Bundle.main.bundleURL.pathExtension == "app" {
                let configuration = NSWorkspace.OpenConfiguration()
                configuration.activates = true
                configuration.addsToRecentItems = false
                configuration.createsNewApplicationInstance = false
                _ = try? await NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration)
            }
            if NSApp.isHidden { NSApp.unhideWithoutActivation() }
            NSApp.activate()
            (window.attachedSheet ?? window).makeKeyAndOrderFront(nil)
            try? await Task.sleep(for: .milliseconds(600))
            if NSApp.isActive && (window.isKeyWindow || window.attachedSheet?.isKeyWindow == true) { break }
        }
        // A failed capture must never be mistaken for an older successful image.
        if FileManager.default.fileExists(atPath: path) {
            do { try FileManager.default.removeItem(atPath: path) }
            catch { log("SNAPSHOT cannot replace old image: \(path)"); captureFailures += 1; return }
        }
        // Auxiliary windows share the probe registry, but never the viewport.
        let mainAreas = Set(GridProbes.shared.probes.values.map(\.area))
            .subtracting(["settings", "folder-browser"])
        let areas = probeAreas ?? mainAreas
        GridProbes.shared.write(to: path.replacingOccurrences(of: ".png", with: "-layout.json"), window: window.frame.size, areas: areas)
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        process.arguments = ["-x", "-o", "-l", String(window.windowNumber), path]
        do { try process.run(); process.waitUntilExit() } catch { log(error.localizedDescription) }
        if let image = NSImage(contentsOfFile: path), image.isValid {
            log("SNAPSHOT \(path)")
            log("SNAPSHOT_STATE active=\(NSApp.isActive) key=\(window.isKeyWindow) sheetKey=\(window.attachedSheet?.isKeyWindow == true)")
            return
        }
        guard let view = window.contentView?.superview ?? window.contentView,
              let representation = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
            log("SNAPSHOT bitmap unavailable: \(path)"); captureFailures += 1; return
        }
        view.cacheDisplay(in: view.bounds, to: representation)
        if let data = representation.representation(using: .png, properties: [:]) {
            do {
                try data.write(to: URL(fileURLWithPath: path), options: .atomic)
                if let image = NSImage(contentsOfFile: path), image.isValid {
                    log("SNAPSHOT \(path) (AppKit fallback)")
                    return
                }
            } catch { log("SNAPSHOT write failed: \(error.localizedDescription)") }
        }
        log("SNAPSHOT missing or invalid image: \(path)")
        captureFailures += 1
    }

    private static func runSelfTest(_ model: AppModel) async {
        guard isTemporary(ProcessInfo.processInfo.environment["APP_STORE"]), model.isSample else {
            log("SELFTEST refused: temporary sample mode required")
            exit(1)
        }
        var failures = 0
        func check(_ condition: Bool, _ name: String) {
            if !condition { failures += 1 }
            log("\(condition ? "PASS" : "FAIL") \(name)")
        }
        check(model.selected.isEmpty, "No automatic selection")
        check(model.disk.available > 0 && model.disk.fraction > 0, "Disk metrics")
        for (name, passed) in UXSelfTest.run() { check(passed, name) }
        for (name, passed) in FolderPickerSelfTest.run() { check(passed, name) }
        for (name, passed) in await FolderPickerSelfTest.runLoading() { check(passed, name) }
        let processes = ProcessInventory.capture()
        check(processes.isComplete && !processes.paths.isEmpty, "Complete executable inventory on this Mac")
        for page in SidebarItem.allCases {
            check(NSImage(systemSymbolName: page.symbol, accessibilityDescription: nil) != nil, "Symbol \(page.title)")
        }
        model.sidebar = .spaceMap
        model.pageChanged()
        check(model.currentItems.isEmpty && model.selectedItems.isEmpty && model.spaceReport != nil, "Space map has read-only sample data")
        model.reviewSelection()
        check(!model.showConfirmation && !model.spaceCanGoBack, "Space map has no selectable items or parent above preset")
        model.startScan()
        check(!model.scanning, "Sample space map refuses live scan")
        model.sidebar = .largeFiles
        check(model.currentItems.count == 3, "Sample large file results")
        if let first = model.currentItems.first {
            model.toggle(first)
            check(model.selectedItems.count == 1 && model.selectedBytes == first.bytes, "Explicit selection and byte total")
        }
        model.query = "Workshop"
        check(model.visibleItems.count == 1, "Search results")
        model.pageChanged()
        check(model.selected.isEmpty && model.query.isEmpty, "Page change clears selection")
        if let item = model.currentItems.first {
            model.toggle(item)
            model.reviewSelection()
            let reviewedIDs = model.reviewItems.map(\.id)
            model.sidebar = .applications
            model.pageChanged()
            check(!reviewedIDs.isEmpty && model.reviewItems.map(\.id) == reviewedIDs && model.reviewPage == .largeFiles, "Review freezes original page and selection")
            model.showConfirmation = false
        }
        model.startScan()
        check(!model.scanning, "Sample mode refuses live scan")
        model.trashSelection()
        check(!model.cleaning, "Sample mode refuses removal")
        check(model.reviewSyncStatus == .local && !model.terminateAfterCleaning, "Sample review has local items and no pending quit")
        let quitGuard = AppDelegate()
        quitGuard.model = model
        check(quitGuard.applicationShouldTerminate(NSApp) == .terminateNow, "Quit is immediate while idle")
        model.cleaning = true
        check(quitGuard.applicationShouldTerminate(NSApp) == .terminateLater && model.terminateAfterCleaning, "Quit during a removal waits for the batch to finish")
        model.cleaning = false
        model.terminateAfterCleaning = false
        model.sidebar = .duplicates
        model.pageChanged()
        check(model.duplicateGroups.count == 2, "Duplicate groups available")
        model.selectDuplicateCopies()
        check(model.selectedItems.count == 3 && model.selected.isDisjoint(with: model.duplicateKeepers.values), "Bulk selection protects every keeper")
        if let group = model.duplicateGroups.first, let keeper = group.files.first(where: { model.isKeeper($0.id) }), let copy = group.files.first(where: { !model.isKeeper($0.id) }) {
            let count = model.selectedItems.count
            model.toggle(keeper)
            check(model.selectedItems.count == count, "Keeper cannot be selected")
            model.keep(copy, in: group)
            check(model.keeperID(for: group) == copy.id && !model.selected.contains(copy.id), "Changing keeper deselects protected copy")
            model.reviewSelection()
            check(model.reviewKeepers[group.id] == copy.id && model.preservedReviewFiles.contains(where: { $0.id == copy.id }), "Review lists protected files")
            model.showConfirmation = false
            model.selected = [keeper.id]
            model.reviewSelection()
            let expectedRemaining = Set(group.files.filter { $0.id != keeper.id }.map(\.id))
            check(Set(model.preservedReviewFiles.map(\.id)) == expectedRemaining, "Partial selection lists every remaining copy")
            model.duplicateGroups = []
            check(Set(model.preservedReviewFiles.map(\.id)) == expectedRemaining, "Remaining copies are frozen with the review")
            model.showConfirmation = false
        }
        exit(failures == 0 ? 0 : 1)
    }

    private static func log(_ text: String) {
        FileHandle.standardError.write(Data((text + "\n").utf8))
    }
}
