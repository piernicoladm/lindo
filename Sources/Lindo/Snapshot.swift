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
                for page in SidebarItem.allCases { model.resetInteraction(for: page) }
                for page in SidebarItem.allCases {
                    model.sidebar = page
                    try? await Task.sleep(for: .milliseconds(600))
                    await capture("\(directory)/\(page.rawValue)-\(mode).png")
                }
                await captureUXStates(model: model, directory: directory, mode: mode)
                await captureProductStates(model: model, directory: directory, mode: mode)
                await captureCoverageStates(model: model, directory: directory, mode: mode)
                await captureReviewStates(model: model, directory: directory, mode: mode)
                model.sidebar = .duplicates
                try? await Task.sleep(for: .milliseconds(300))
                model.selectDuplicateCopies()
                await capture("\(directory)/duplicates-selected-\(mode).png")
                model.reviewSelection()
                try? await Task.sleep(for: .milliseconds(500))
                await capture("\(directory)/duplicate-confirmation-\(mode).png")
                await captureConfirmationSheet("\(directory)/duplicate-confirmation-\(mode)-sheet.png")
                model.showConfirmation = false
                model.selected = []
                if let group = model.duplicateGroups.first, let copy = group.files.first(where: { !model.isKeeper($0.id) }) {
                    model.selected = [copy.id]
                    model.reviewSelection()
                    try? await Task.sleep(for: .milliseconds(500))
                    await capture("\(directory)/duplicate-partial-confirmation-\(mode).png")
                    await captureConfirmationSheet("\(directory)/duplicate-partial-confirmation-\(mode)-sheet.png")
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
                await captureConfirmationSheet("\(directory)/confirmation-\(mode)-sheet.png")
                model.showConfirmation = false
                await captureSyncReviewStates(model: model, directory: directory, mode: mode)
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

                let sampleDownloads = URL(fileURLWithPath: "/Users/demo/Downloads")
                let browser = FolderBrowserModel(initialURL: URL(fileURLWithPath: "/Cartella di esempio"), downloadsURL: sampleDownloads, loader: { url in
                    if url == sampleDownloads {
                        return FolderListing(directory: url, children: [], warnings: [], canSelect: true, parent: nil)
                    }
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
                await browser.openDownloads().value
                try? await Task.sleep(for: .milliseconds(300))
                await capture("\(directory)/folder-chooser-recovered-\(mode).png", window: chooser, probeAreas: ["folder-browser"])
                if !browser.canSelect || browser.currentURL != sampleDownloads {
                    captureFailures += 1
                    log("SNAPSHOT folder recovery did not reach the synthetic Downloads listing")
                }
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
        model.selectDuplicateCopies()
        model.appearedReviewRows = []
        model.reviewSelection()
        try? await Task.sleep(for: .milliseconds(500))
        await captureConfirmationSheet("\(directory)/ten-thousand-review-\(mode)-sheet.png")
        let reviewCount = model.appearedReviewRows.count
        log("LAZY_REVIEW_ROWS \(mode): \(reviewCount) appeared of \(model.reviewDuplicateRows.count)")
        if reviewCount == 0 || reviewCount >= 200 {
            captureFailures += 1
            log("LAZY_REVIEW_ROWS failed viewport bound")
        }
        model.showConfirmation = false
        try? await Task.sleep(for: .milliseconds(300))
        model.items = savedItems
        model.scanDates = savedDates
        model.warnings = savedWarnings
        model.scanOutcomes = savedOutcomes
        model.duplicateGroups = savedGroups
        model.duplicateKeepers = savedKeepers
        model.spaceReport = savedMap
        model.resetInteraction(for: .duplicates)
    }

    /// Fixture-driven screens exercise model transitions without filesystem access or fake clicks.
    private static func captureProductStates(model: AppModel, directory: String, mode: String) async {
        let savedPage = model.sidebar
        let savedItems = model.items, savedDates = model.scanDates, savedScopes = model.scannedScopes
        let savedWarnings = model.warnings, savedOutcomes = model.scanOutcomes
        let savedNotices = model.notices, savedFailures = model.failuresByPage
        let savedKeepers = model.duplicateKeepers, savedMinimum = model.minimumMB
        let savedMap = model.spaceReport, savedMapRoot = model.spaceRoot
        let interactions = SidebarItem.allCases.map { ($0, model.selection(for: $0), model.searchQuery(for: $0)) }

        model.sidebar = .duplicates
        model.selected = Set(model.duplicateGroups.first?.files.dropFirst().prefix(1).map(\.id) ?? [])
        let duplicateSelection = model.selected
        model.query = "Foto_evento"
        model.sidebar = .largeFiles
        model.selected = Set(model.currentItems.prefix(1).map(\.id))
        let largeSelection = model.selected
        model.query = "Workshop"
        model.sidebar = .overview
        model.sidebar = .duplicates
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/selection-restored-duplicates-\(mode).png")
        if model.selected != duplicateSelection || model.query != "Foto_evento" {
            captureFailures += 1; log("SNAPSHOT duplicate navigation did not preserve interaction")
        }
        model.sidebar = .largeFiles
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/selection-restored-largeFiles-\(mode).png")
        if model.selected != largeSelection || model.query != "Workshop" {
            captureFailures += 1; log("SNAPSHOT large-file navigation did not preserve interaction")
        }

        model.minimumMB = 500
        model.items[.largeFiles] = savedItems[.largeFiles]?.filter { $0.bytes >= 500_000_000 }
        model.scanDates[.largeFiles] = savedDates[.largeFiles]
        model.scannedScopes[.largeFiles] = model.configuredScopeDescription(for: .largeFiles)
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/largeFiles-500mb-\(mode).png")

        model.sidebar = .duplicates
        model.query = ""
        model.selected = []
        if let group = model.duplicateGroups.first, let copy = group.files.last {
            model.keep(copy, in: group)
            model.selectDuplicateCopies()
            try? await Task.sleep(for: .milliseconds(300))
            await capture("\(directory)/duplicate-keeper-changed-\(mode).png")
            model.reviewSelection()
            try? await Task.sleep(for: .milliseconds(500))
            await captureConfirmationSheet("\(directory)/duplicate-keeper-review-\(mode)-sheet.png")
            if model.reviewKeepers[group.id] != copy.id || model.reviewItems.contains(where: { $0.id == copy.id }) {
                captureFailures += 1; log("SNAPSHOT changed protected copy entered removal review")
            }
            model.showConfirmation = false
            try? await Task.sleep(for: .milliseconds(300))
        } else {
            captureFailures += 1; log("SNAPSHOT missing synthetic group for protected-copy change")
        }

        model.sidebar = .spaceMap
        let nested = savedMapRoot.appending(path: "Video")
        model.spaceRoot = nested
        model.spaceReport = SpaceReport(root: nested, entries: [
            SpaceEntry(url: nested.appending(path: "Montaggio finale.mov"), allocatedBytes: 12_000_000_000,
                       logicalBytes: 11_998_000_000, fileCount: 1, isDirectory: false, isPartial: false),
            SpaceEntry(url: nested.appending(path: "Riprese"), allocatedBytes: 6_600_000_000,
                       logicalBytes: 6_594_000_000, fileCount: 23, isDirectory: true, isPartial: false)
        ], allocatedBytes: 18_600_000_000, logicalBytes: 18_592_000_000, fileCount: 24,
           inspectedCount: 25, warnings: [], isPartial: false, cancelled: false, duration: 0.1)
        model.warnings[.spaceMap] = []
        model.scanOutcomes[.spaceMap] = .complete
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/spaceMap-nested-\(mode).png")

        model.minimumMB = savedMinimum
        model.items = savedItems
        model.scanDates = savedDates
        model.scannedScopes = savedScopes
        model.warnings = savedWarnings
        model.scanOutcomes = savedOutcomes
        model.notices = savedNotices
        model.failuresByPage = savedFailures
        model.duplicateKeepers = savedKeepers
        model.spaceRoot = savedMapRoot
        model.spaceReport = savedMap
        for (page, selected, query) in interactions {
            model.sidebar = page
            model.selected = selected
            model.query = query
        }
        model.sidebar = savedPage
    }

    private static func captureCoverageStates(model: AppModel, directory: String, mode: String) async {
        let page = model.sidebar, warnings = model.warnings, outcomes = model.scanOutcomes
        let coverage = model.coverageByPage, map = model.spaceReport
        model.sidebar = .duplicates
        model.coverageByPage[.duplicates] = ScanCoverage(excludedCount: 3)
        model.scanOutcomes[.duplicates] = .excluded
        model.warnings[.duplicates] = ["Elementi nascosti esclusi: 2.", "Pacchetti esclusi: 1."]
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/coverage-exclusions-duplicates-\(mode).png")
        model.coverageByPage[.duplicates] = ScanCoverage(excludedCount: 3, hasUnplannedOmissions: true)
        model.scanOutcomes[.duplicates] = .partial
        model.warnings[.duplicates]?.append("Cartella non accessibile: /Users/demo/Documents/Limitata")
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/coverage-mixed-duplicates-\(mode).png")
        if let map {
            let entries = map.entries.map { entry in
                SpaceEntry(id: entry.id, url: entry.url, allocatedBytes: entry.allocatedBytes,
                           logicalBytes: entry.logicalBytes, fileCount: entry.fileCount,
                           isDirectory: entry.isDirectory, isPartial: entry.isPartial,
                           alreadyCountedLinks: entry.alreadyCountedLinks,
                           coverage: ScanCoverage(excludedCount: entry.isPartial ? 1 : 0))
            }
            let excluded = entries.reduce(0) { $0 + $1.coverage.excludedCount }
            let mapCoverage = ScanCoverage(excludedCount: excluded)
            let mapWarnings = ["Percorsi privati esclusi dalle regole: \(excluded)."]
            model.spaceReport = SpaceReport(root: map.root, entries: entries, allocatedBytes: map.allocatedBytes,
                logicalBytes: map.logicalBytes, fileCount: map.fileCount, inspectedCount: map.inspectedCount,
                warnings: mapWarnings, isPartial: true, cancelled: false, duration: map.duration, coverage: mapCoverage)
            model.coverageByPage[.spaceMap] = mapCoverage
            model.scanOutcomes[.spaceMap] = .excluded
            model.warnings[.spaceMap] = mapWarnings
            model.sidebar = .spaceMap
            try? await Task.sleep(for: .milliseconds(300))
            await capture("\(directory)/coverage-exclusions-map-\(mode).png")
        } else {
            captureFailures += 1; log("SNAPSHOT coverage requires the synthetic space map")
        }
        model.sidebar = .overview
        try? await Task.sleep(for: .milliseconds(300))
        await capture("\(directory)/coverage-overview-\(mode).png")
        model.warnings = warnings
        model.scanOutcomes = outcomes
        model.coverageByPage = coverage
        model.spaceReport = map
        model.sidebar = page
    }

    private static func captureSyncReviewStates(model: AppModel, directory: String, mode: String) async {
        guard model.isSample, model.sidebar == .largeFiles, !model.reviewItems.isEmpty else {
            captureFailures += 1
            log("SNAPSHOT sync review requires the existing synthetic large-file confirmation")
            return
        }
        let savedItems = model.items[.largeFiles]
        let savedSelected = model.selected, savedQuery = model.query
        let savedReviewIDs = model.reviewItems.map(\.id), savedSyncStatus = model.reviewSyncStatus
        let savedConfirmation = model.showConfirmation
        model.showConfirmation = false
        try? await Task.sleep(for: .milliseconds(300))

        let states: [(name: String, status: FileSyncStatus, path: String)] = [
            ("synced", .synced, "/Users/demo/Library/CloudStorage/ExampleProvider/Condivisi/Progetto/Presentazione finale.pdf"),
            ("unknown", .unknown, "/Users/demo/Documents/Progetto/Materiali da verificare/Presentazione finale.pdf")
        ]
        for state in states {
            let files = [
                ScanItem(id: "review-local", url: URL(fileURLWithPath: "/Users/demo/Downloads/Archivio progetto.zip"),
                         title: "Archivio progetto.zip", bytes: 128_000_000, category: .largeFiles, syncStatus: .local),
                ScanItem(id: "review-\(state.name)", url: URL(fileURLWithPath: state.path),
                         title: "Presentazione finale.pdf", bytes: 64_000_000, category: .largeFiles, syncStatus: state.status)
            ]
            model.items[.largeFiles] = files
            model.selected = Set(files.map(\.id))
            model.query = ""
            model.reviewSelection()
            if !model.showConfirmation || model.reviewSyncStatus != state.status {
                captureFailures += 1
                log("SNAPSHOT incorrect \(state.name) review state")
            } else {
                try? await Task.sleep(for: .milliseconds(500))
                await captureConfirmationSheet("\(directory)/confirmation-\(state.name)-\(mode)-sheet.png")
            }
            model.showConfirmation = false
            try? await Task.sleep(for: .milliseconds(300))
        }

        model.items[.largeFiles] = savedItems
        model.selected = savedSelected
        model.query = savedQuery
        // Rebuild the original frozen review, including its privately stored sync summary.
        model.reviewSelection()
        model.showConfirmation = savedConfirmation
        if model.reviewItems.map(\.id) != savedReviewIDs || model.reviewSyncStatus != savedSyncStatus {
            captureFailures += 1
            log("SNAPSHOT failed to restore the original review after sync fixtures")
        }
    }

    private static func captureConfirmationSheet(_ path: String) async {
        guard let window = NSApp.windows.first(where: { !$0.isSheet && $0.attachedSheet != nil }),
              let sheet = window.attachedSheet else {
            captureFailures += 1
            log("SNAPSHOT missing attached confirmation sheet: \(path)")
            return
        }
        // ConfirmationView has no geometry probes. Capture the actual sheet, including
        // the AppKit fallback, without emitting an empty layout file that looks verified.
        await capture(path, window: sheet, writeLayout: false)
    }

    private static func capture(_ path: String, window supplied: NSWindow? = nil, probeAreas: Set<String>? = nil,
                                writeLayout: Bool = true) async {
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
        if writeLayout {
            let mainAreas = Set(GridProbes.shared.probes.values.map(\.area))
                .subtracting(["settings", "folder-browser"])
            let areas = probeAreas ?? mainAreas
            GridProbes.shared.write(to: path.replacingOccurrences(of: ".png", with: "-layout.json"), window: window.frame.size, areas: areas)
        }
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
        let savedSelection = model.selected
        model.sidebar = .applications
        check(model.selected.isEmpty && model.query.isEmpty, "Unvisited page starts without a selection or query")
        model.sidebar = .largeFiles
        check(model.selected == savedSelection && model.query == "Workshop", "Navigation restores page selection and query")
        if let item = model.currentItems.first {
            model.selected = [item.id]
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
