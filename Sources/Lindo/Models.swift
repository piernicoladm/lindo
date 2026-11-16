import AppKit
import Foundation
import LindoKit
import Observation

enum SidebarItem: String, CaseIterable, Identifiable {
    case overview, spaceMap, cleanup, largeFiles, duplicates, applications
    var id: String { rawValue }
    var title: String {
        switch self {
        case .spaceMap: "Mappa spazio"
        case .overview: "Panoramica"
        case .cleanup: "Cache e log"
        case .largeFiles: "File grandi"
        case .duplicates: "Duplicati"
        case .applications: "Applicazioni"
        }
    }
    var symbol: String {
        switch self {
        case .spaceMap: "chart.bar.xaxis"
        case .overview: "internaldrive"
        case .cleanup: "archivebox"
        case .largeFiles: "doc"
        case .duplicates: "doc.on.doc"
        case .applications: "app"
        }
    }
    var subtitle: String {
        switch self {
        case .spaceMap: "Esplora lo spazio occupato, in sola lettura."
        case .overview: "Una visione chiara dello spazio sul tuo Mac."
        case .cleanup: "Cache riconosciute dei browser e log più vecchi di 7 giorni."
        case .largeFiles: "Trova i file più ingombranti in una cartella scelta da te."
        case .duplicates: "Trova copie con contenuto identico e scegli quale conservare."
        case .applications: "Esamina le app installate e scegli quali spostare nel Cestino."
        }
    }
}

enum DuplicateSortOrder: String, CaseIterable, Identifiable {
    case extraBytes, path
    var id: String { rawValue }
    var title: String { self == .extraBytes ? "Spazio extra" : "Percorso" }
}

enum SpaceScope: String, CaseIterable, Identifiable {
    case user, library, downloads, applications
    var id: String { rawValue }
    var title: String {
        switch self {
        case .user: "Utente"
        case .library: "Library"
        case .downloads: "Download"
        case .applications: "Applicazioni"
        }
    }
    var root: URL {
        let home = FileManager.default.homeDirectoryForCurrentUser
        switch self {
        case .user: return home
        case .library: return home.appending(path: "Library")
        case .downloads: return home.appending(path: "Downloads")
        case .applications: return URL(fileURLWithPath: "/Applications")
        }
    }
    var detail: String {
        switch self {
        case .user, .library: "Include Library e cartelle nascoste accessibili. Legge solo metadati; esclude percorsi privati e credenziali."
        case .downloads: "Esamina Download e le sue sottocartelle. Legge solo metadati; esclude percorsi privati e credenziali."
        case .applications: "Misura le applicazioni in /Applications usando solo metadati."
        }
    }
}

struct DiskSpace {
    var total: Int64 = 0
    var available: Int64 = 0
    var used: Int64 { max(0, total - available) }
    var fraction: Double { total > 0 ? min(1, Double(used) / Double(total)) : 0 }
    static func current() -> DiskSpace {
        guard let values = try? FileManager.default.attributesOfFileSystem(forPath: NSHomeDirectory()),
              let total = values[.systemSize] as? NSNumber,
              let free = values[.systemFreeSize] as? NSNumber else { return DiskSpace() }
        return DiskSpace(total: total.int64Value, available: free.int64Value)
    }
}

@Observable
final class AppModel {
    var sidebar: SidebarItem = .overview {
        didSet {
            if sidebar != oldValue {
                folderPicker.cancel()
                pageChanged()
            }
        }
    }
    var disk = DiskSpace.current()
    var items: [SidebarItem: [ScanItem]] = [:] {
        didSet { resultsRevision += 1 }
    }
    var warnings: [SidebarItem: [String]] = [:]
    var scanDates: [SidebarItem: Date] = [:]
    var scannedScopes: [SidebarItem: String] = [:]
    var scanOutcomes: [SidebarItem: ScanOutcome] = [:]
    var coverageByPage: [SidebarItem: ScanCoverage] = [:]
    private var selectionsByPage: [SidebarItem: Set<String>] = [:]
    private var queriesByPage: [SidebarItem: String] = [:]
    var selected: Set<String> {
        get { selection(for: sidebar) }
        set { setSelection(newValue, for: sidebar) }
    }
    var query: String {
        get { queriesByPage[sidebar] ?? "" }
        set { queriesByPage[sidebar] = newValue }
    }
    var largeRoot: URL? {
        didSet { if largeRoot != oldValue { clearResults(.largeFiles) } }
    }
    var duplicateRoot: URL? {
        didSet { if duplicateRoot != oldValue { clearResults(.duplicates) } }
    }
    var searchFocusRequest = 0
    var spaceScope: SpaceScope = .downloads
    var spaceRoot = SpaceScope.downloads.root
    var spaceReport: SpaceReport?
    let spaceScanner = SpaceScanner()
    var duplicateGroups: [DuplicateGroup] = [] {
        didSet {
            resultsRevision += 1
            duplicateExtraBytes = duplicateGroups.reduce(0) { $0 + Int64(max(0, $1.files.count - 1)) * ($1.files.first?.bytes ?? 0) }
        }
    }
    private(set) var duplicateExtraBytes: Int64 = 0
    var duplicateKeepers: [String: String] = [:] {
        didSet {
            duplicateKeeperIDs = Set(duplicateKeepers.values)
            selectionRevision += 1
        }
    }
    private(set) var duplicateKeeperIDs: Set<String> = []
    var duplicateMinimumKB = 0 {
        didSet { if duplicateMinimumKB != oldValue { clearResults(.duplicates) } }
    }
    var duplicateSortOrder: DuplicateSortOrder = .extraBytes
    var reviewKeepers: [String: String] = [:]
    var progressText = "Analisi in corso…"
    var minimumMB = 100 {
        didSet { if minimumMB != oldValue { clearResults(.largeFiles) } }
    }
    var activePage: SidebarItem?
    var scanning = false
    var scanCancelling = false
    var cleaning = false
    var showConfirmation = false
    var showWarnings = false
    var notices: [SidebarItem: String] = [:]
    var failuresByPage: [SidebarItem: [String]] = [:]
    var reviewItems: [ScanItem] = []
    var preservedReviewFiles: [ScanItem] = []
    private(set) var reviewDuplicateGroups: [ReviewDuplicateGroup] = []
    private(set) var reviewDuplicateRows: [ReviewDuplicateRow] = []
    var reviewPage: SidebarItem = .overview
    let isSample = ProcessInfo.processInfo.environment["APP_SAMPLE"] == "1"
    let scanner = MacScanner()
    let duplicateScanner = DuplicateScanner()
    private var scanTask: Task<Void, Never>?
    private(set) var resultsRevision = 0
    private(set) var selectionRevision = 0
    @ObservationIgnored var resultsCache: ResultsProjection?
    @ObservationIgnored var selectionCache: SelectionProjection?
    @ObservationIgnored var projectionBuildCount = 0
    @ObservationIgnored var appearedDuplicateRows: Set<String> = []
    @ObservationIgnored var appearedReviewRows: Set<String> = []
    private let folderPresenter = InlineFolderPicker()
    @ObservationIgnored private lazy var folderPicker = FolderPickerCoordinator(presenter: folderPresenter)
    var choosingFolder: Bool { folderPicker.choosing }

    var presentedFolderBrowser: FolderBrowserModel? { folderPresenter.browser }
    func cancelFolderChoice() { folderPresenter.cancel() }
    func completeFolderChoice(_ url: URL) { folderPresenter.select(url) }

    init() {
        if isSample { seedSamples() }
    }

    var currentItems: [ScanItem] { sidebar == .spaceMap ? [] : (items[sidebar] ?? []) }
    var visibleItems: [ScanItem] { resultsProjection.visibleItems }
    var selectedItems: [ScanItem] { selectionProjection.items }
    var selectedBytes: Int64 { selectionProjection.bytes }
    var listedBytes: Int64 { resultsProjection.listedBytes }
    var totalCandidates: Int { items.values.reduce(0) { $0 + $1.count } }
    var isBusy: Bool { scanning || cleaning || choosingFolder }
    var isScanningCurrentPage: Bool { scanning && activePage == sidebar }
    var activityPage: SidebarItem? { cleaning ? reviewPage : activePage }
    var hiddenSelectedCount: Int {
        let visibleIDs = resultsProjection.visibleIDs
        return selectedItems.filter { !visibleIDs.contains($0.id) }.count
    }
    func clearSearch() { query = "" }
    var notice: String? { notices[sidebar] }
    var failures: [String] { failuresByPage[sidebar] ?? [] }
    var reviewBytes: Int64 { reviewItems.reduce(0) { $0 + $1.bytes } }
    /// Set when the person quits during a confirmed removal: the batch finishes first.
    var terminateAfterCleaning = false
    /// Frozen with the selection, using only metadata already captured by the scan worker.
    private(set) var reviewSyncStatus = FileSyncStatus.local
    var chosenRoot: URL? { sidebar == .duplicates ? duplicateRoot : largeRoot }

    func pageChanged() {
        showWarnings = false
    }

    func selection(for page: SidebarItem) -> Set<String> { selectionsByPage[page] ?? [] }
    func searchQuery(for page: SidebarItem) -> String { queriesByPage[page] ?? "" }

    func setSelection(_ ids: Set<String>, for page: SidebarItem) {
        selectionsByPage[page] = ids
        selectionRevision += 1
    }

    /// A new scan or a changed scope invalidates only that page's interaction state.
    func resetInteraction(for page: SidebarItem) {
        setSelection([], for: page)
        queriesByPage[page] = nil
    }

    func chooseFolder(andScan: Bool = false) {
        guard !isBusy && !isSample && !showConfirmation else { return }
        let page = sidebar
        guard page == .largeFiles || page == .duplicates else { return }
        let directory = chosenRoot ?? FileManager.default.homeDirectoryForCurrentUser.appending(path: "Downloads")
        folderPicker.begin(page: page, directory: directory, andScan: andScan, canApply: { [weak self] in
            guard let self else { return false }
            return self.sidebar == page && !self.isBusy && !self.isSample && !self.showConfirmation
        }, apply: { [weak self] url, shouldScan in
            guard let self else { return }
            if page == .duplicates { self.duplicateRoot = url }
            else { self.largeRoot = url }
            if shouldScan { self.startScan() }
        })
    }

    func startScan() {
        guard !isBusy && !isSample && !showConfirmation else { return }
        if sidebar == .spaceMap { startSpaceScan(); return }
        let page = sidebar == .overview ? SidebarItem.cleanup : sidebar
        if (page == .largeFiles && largeRoot == nil) || (page == .duplicates && duplicateRoot == nil) {
            chooseFolder(andScan: true)
            return
        }
        sidebar = page
        resetInteraction(for: page)
        notices[page] = nil
        failuresByPage[page] = nil
        activePage = page
        scanning = true
        scanCancelling = false
        progressText = "Analisi in corso…"
        let home = FileManager.default.homeDirectoryForCurrentUser
        let root = page == .duplicates ? duplicateRoot : largeRoot
        let threshold = Int64(minimumMB) * 1_000_000
        let duplicateThreshold = max(1, Int64(duplicateMinimumKB) * 1_000)
        let scannedScope = configuredScopeDescription(for: page)
        scanTask = Task {
            var reports: [ScanReport] = []
            var duplicateReport: DuplicateReport?
            switch page {
            case .cleanup:
                let cutoff = Date().addingTimeInterval(-7 * 24 * 60 * 60)
                reports.append(await scanner.scan(category: .caches, roots: [home.appending(path: "Library/Caches")]))
                if !Task.isCancelled {
                    reports.append(await scanner.scan(category: .logs, roots: [home.appending(path: "Library/Logs")], olderThan: cutoff))
                }
            case .largeFiles:
                if let root { reports.append(await scanner.scan(category: .largeFiles, roots: [root], minimumBytes: threshold)) }
            case .applications:
                reports.append(await scanner.scan(category: .applications, roots: [URL(fileURLWithPath: "/Applications"), home.appending(path: "Applications")], minimumBytes: 0))
            case .duplicates:
                if let root {
                    duplicateReport = await duplicateScanner.scan(roots: [root], minimumBytes: duplicateThreshold, progress: { message in
                        await MainActor.run {
                            if self.activePage == page && !self.scanCancelling { self.progressText = message }
                        }
                    })
                }
            case .overview, .spaceMap: break
            }
            if let duplicateReport {
                duplicateGroups = duplicateReport.groups
                duplicateKeepers = Dictionary(uniqueKeysWithValues: duplicateReport.groups.map { ($0.id, $0.keeperID) })
            }
            items[page] = (duplicateReport?.groups.flatMap(\.files) ?? reports.flatMap(\.items)).sorted { $0.bytes > $1.bytes }
            var messages = reports.flatMap(\.warnings) + (duplicateReport?.warnings ?? [])
            let cancelled = Task.isCancelled || reports.contains(where: \.cancelled) || duplicateReport?.cancelled == true
            let coverage = ScanCoverage.combined(reports.map(\.coverage) + (duplicateReport.map { [$0.coverage] } ?? []))
            coverageByPage[page] = coverage
            scanOutcomes[page] = ScanOutcome(cancelled: cancelled, partial: coverage.hasUnplannedOmissions, excludedCount: coverage.excludedCount)
            if cancelled {
                messages.insert("Scansione interrotta: i risultati sono parziali.", at: 0)
            } else if coverage.hasUnplannedOmissions {
                messages.insert("Scansione parziale: alcuni elementi non sono stati esaminati.", at: 0)
            }
            warnings[page] = Array(NSOrderedSet(array: messages)) as? [String] ?? messages
            scanDates[page] = Date()
            scannedScopes[page] = scannedScope
            scanning = false
            scanCancelling = false
            activePage = nil
            disk = .current()
            scanTask = nil
        }
    }

    func setSpaceScope(_ scope: SpaceScope) {
        guard !isBusy else { return }
        spaceScope = scope
        spaceRoot = spaceScopeBoundary
        spaceReport = nil
        warnings[.spaceMap] = nil
        scanDates[.spaceMap] = nil
        scanOutcomes[.spaceMap] = nil
        coverageByPage[.spaceMap] = nil
        scannedScopes[.spaceMap] = nil
        resetInteraction(for: .spaceMap)
    }

    func startSpaceScan(at directory: URL? = nil) {
        guard !isBusy && !isSample && !showConfirmation else { return }
        let root = (directory ?? spaceRoot).standardizedFileURL
        let boundary = spaceScopeBoundary.standardizedFileURL.path
        guard root.path == boundary || root.path.hasPrefix(boundary + "/") else { return }
        spaceRoot = root
        spaceReport = nil
        resetInteraction(for: .spaceMap)
        activePage = .spaceMap
        scanning = true
        scanCancelling = false
        progressText = "Misurazione dello spazio in corso…"
        scanTask = Task {
            let report = await spaceScanner.scan(root: root)
            spaceReport = report
            warnings[.spaceMap] = report.warnings
            scanDates[.spaceMap] = Date()
            scannedScopes[.spaceMap] = report.root.path
            coverageByPage[.spaceMap] = report.coverage
            scanOutcomes[.spaceMap] = ScanOutcome(cancelled: report.cancelled, partial: report.coverage.hasUnplannedOmissions, excludedCount: report.coverage.excludedCount)
            scanning = false
            scanCancelling = false
            activePage = nil
            scanTask = nil
        }
    }

    private var spaceScopeBoundary: URL {
        if isSample {
            let home = URL(fileURLWithPath: "/Users/demo")
            switch spaceScope {
            case .user: return home
            case .library: return home.appending(path: "Library")
            case .downloads: return home.appending(path: "Downloads")
            case .applications: return URL(fileURLWithPath: "/Applications")
            }
        }
        return spaceScope.root
    }
    var spaceCanGoBack: Bool { spaceRoot.standardizedFileURL.path != spaceScopeBoundary.standardizedFileURL.path }

    func cancelScan() {
        guard scanning, !scanCancelling else { return }
        scanCancelling = true
        progressText = "Interruzione in corso…"
        scanTask?.cancel()
    }

    func toggle(_ item: ScanItem) {
        guard sidebar != .spaceMap && !isBusy && !showConfirmation && (sidebar != .duplicates || !isKeeper(item.id)) else { return }
        if selected.contains(item.id) { selected.remove(item.id) } else { selected.insert(item.id) }
    }

    func reviewSelection() {
        guard sidebar != .spaceMap && !isBusy && !showConfirmation && !selectedItems.isEmpty else { return }
        reviewItems = selectedItems
        reviewSyncStatus = FileSyncStatus.combined(reviewItems.lazy.map(\.syncStatus))
        reviewPage = sidebar
        reviewKeepers = sidebar == .duplicates ? duplicateKeepers : [:]
        let selectedIDs = Set(reviewItems.map(\.id))
        reviewDuplicateGroups = sidebar == .duplicates
            ? freezeDuplicateReview(groups: duplicateGroups, selectedIDs: selectedIDs,
                                    keepers: reviewKeepers, order: duplicateSortOrder) : []
        preservedReviewFiles = reviewDuplicateGroups.flatMap(\.preserved)
        reviewDuplicateRows = duplicateReviewRows(reviewDuplicateGroups)
        showConfirmation = true
    }

    func trashSelection() {
        guard reviewPage != .spaceMap && !isBusy && !isSample && !reviewItems.isEmpty else { return }
        let page = reviewPage
        let ids = reviewItems.map(\.id)
        let keepers = reviewKeepers
        cleaning = true
        Task {
            let report: TrashReport
            if page == .duplicates {
                report = await duplicateScanner.moveToTrash(ids: ids, keepers: keepers)
            } else {
                let runtime = await Self.runtimeState()
                report = await scanner.moveToTrash(ids: ids, runningApplicationPaths: runtime.paths, runningBundleIdentifiers: runtime.bundleIdentifiers, liveRunningApplications: { await Self.runtimeState() })
            }
            let moved = Set(report.trashedIDs)
            items[page]?.removeAll { moved.contains($0.id) }
            setSelection(selection(for: page).subtracting(moved), for: page)
            if page == .duplicates { removeTrashedDuplicates(moved) }
            failuresByPage[page] = report.failures
            let requested = Set(ids).count
            notices[page] = moved.isEmpty ? "Nessun elemento spostato. Consulta i dettagli."
                : moved.count < requested ? "\(moved.count) di \(elementCount(requested)) \(moved.count == 1 ? "spostato" : "spostati") nel Cestino. Consulta i dettagli per gli altri. Lo spazio si libera quando lo svuoti dal Finder."
                : "\(elementCount(moved.count)) \(moved.count == 1 ? "spostato" : "spostati") nel Cestino. Lo spazio si libera quando lo svuoti dal Finder."
            reviewItems = []
            preservedReviewFiles = []
            reviewDuplicateGroups = []
            reviewDuplicateRows = []
            cleaning = false
            disk = .current()
            if terminateAfterCleaning {
                terminateAfterCleaning = false
                NSApp.reply(toApplicationShouldTerminate: true)
            }
        }
    }

    private static func runtimeState() async -> RunningApplications {
        let processes = await Task.detached(priority: .userInitiated) { ProcessInventory.capture() }.value
        let current = NSWorkspace.shared.runningApplications
        return RunningApplications(paths: processes.paths.union(current.compactMap { $0.bundleURL?.path }), bundleIdentifiers: Set(current.compactMap(\.bundleIdentifier)), inspectionComplete: processes.isComplete)
    }

    private func seedSamples() {
        disk = DiskSpace(total: 1_000_000_000_000, available: 328_400_000_000)
        largeRoot = URL(fileURLWithPath: "/Users/demo/Downloads")
        duplicateRoot = URL(fileURLWithPath: "/Users/demo/Documents")
        let now = Date()
        spaceRoot = URL(fileURLWithPath: "/Users/demo/Downloads")
        let spaceEntries = [
            SpaceEntry(id: "space-video", url: spaceRoot.appending(path: "Video"), allocatedBytes: 18_600_000_000, logicalBytes: 18_592_000_000, fileCount: 24, isDirectory: true, isPartial: false),
            SpaceEntry(id: "space-archive", url: spaceRoot.appending(path: "Archivi"), allocatedBytes: 6_240_000_000, logicalBytes: 6_230_000_000, fileCount: 86, isDirectory: true, isPartial: false),
            SpaceEntry(id: "space-workshop", url: spaceRoot.appending(path: "Workshop_finale.mov"), allocatedBytes: 4_820_000_000, logicalBytes: 4_819_000_000, fileCount: 1, isDirectory: false, isPartial: false),
            SpaceEntry(id: "space-documents", url: spaceRoot.appending(path: "Documenti"), allocatedBytes: 842_000_000, logicalBytes: 840_000_000, fileCount: 320, isDirectory: true, isPartial: true)
        ]
        spaceReport = SpaceReport(root: spaceRoot, entries: spaceEntries, allocatedBytes: 30_502_000_000, logicalBytes: 30_481_000_000, fileCount: 431, inspectedCount: 458, warnings: ["Esempio: una sottocartella non è accessibile."], isPartial: true, cancelled: false, duration: 1.4)
        scanDates[.spaceMap] = now
        warnings[.spaceMap] = spaceReport?.warnings
        func sample(_ name: String, _ path: String, _ size: Int64, _ category: ScanCategory, _ detail: String) -> ScanItem {
            ScanItem(id: UUID().uuidString, url: URL(fileURLWithPath: path), title: name, bytes: size, modifiedAt: now.addingTimeInterval(-20 * 86400), category: category, detail: detail, syncStatus: .local)
        }
        items[.cleanup] = [
            sample("Chrome · Default", "/Users/demo/Library/Caches/Google/Chrome/Default/Cache", 842_000_000, .caches, "Cache web riconosciuta · chiudi Google Chrome"),
            sample("session-2026-10.log", "/Users/demo/Library/Logs/Studio/session-2026-10.log", 216_000_000, .logs, "Log applicazione"),
            sample("Firefox · profilo personale", "/Users/demo/Library/Caches/Firefox/Profiles/abcdefgh.default/cache2", 128_000_000, .caches, "Cache web riconosciuta · chiudi Firefox")
        ]
        items[.largeFiles] = [
            sample("Workshop_finale.mov", "/Users/demo/Downloads/Workshop_finale.mov", 4_820_000_000, .largeFiles, "File locale"),
            sample("Archivio_progetto.zip", "/Users/demo/Downloads/Archivio_progetto.zip", 1_640_000_000, .largeFiles, "File locale"),
            sample("Presentazione_evento.pdf", "/Users/demo/Downloads/Presentazione_evento.pdf", 184_000_000, .largeFiles, "File locale")
        ]
        items[.applications] = [
            sample("Studio", "/Applications/Studio.app", 2_180_000_000, .applications, "Versione 4.2"),
            sample("Reader", "/Applications/Reader.app", 382_000_000, .applications, "Versione 3.1"),
            sample("Palette", "/Applications/Palette.app", 164_000_000, .applications, "Versione 1.8")
        ]
        let keeper = sample("Presentazione.pdf", "/Users/demo/Documents/Presentazione.pdf", 64_000_000, .largeFiles, "Contenuto identico")
        let copy = sample("Presentazione copia.pdf", "/Users/demo/Documents/Archivio/Presentazione copia.pdf", 64_000_000, .largeFiles, "Contenuto identico")
        let third = sample("Presentazione finale.pdf", "/Users/demo/Documents/Condivisi/Presentazione finale.pdf", 64_000_000, .largeFiles, "Contenuto identico")
        let photo = sample("Foto_evento.jpg", "/Users/demo/Documents/Foto_evento.jpg", 12_000_000, .largeFiles, "Contenuto identico")
        let photoCopy = sample("Foto_evento_2.jpg", "/Users/demo/Documents/Archivio/Foto_evento_2.jpg", 12_000_000, .largeFiles, "Contenuto identico")
        duplicateGroups = [DuplicateGroup(id: "sample-document", files: [keeper, copy, third], keeperID: keeper.id, digest: "sample"), DuplicateGroup(id: "sample-photo", files: [photo, photoCopy], keeperID: photo.id, digest: "sample")]
        duplicateKeepers = Dictionary(uniqueKeysWithValues: duplicateGroups.map { ($0.id, $0.keeperID) })
        items[.duplicates] = duplicateGroups.flatMap(\.files)
        for page in [SidebarItem.cleanup, .largeFiles, .duplicates, .applications] { scanDates[page] = now }
        warnings[.cleanup] = ["Esempio: una cartella non è accessibile. I risultati sono parziali."]
    }
}

func bytes(_ value: Int64) -> String {
    ByteCountFormatter.string(fromByteCount: value, countStyle: .decimal)
}

func elementCount(_ count: Int) -> String { "\(count) \(count == 1 ? "elemento" : "elementi")" }
