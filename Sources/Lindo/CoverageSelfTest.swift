import Foundation
import LindoKit

enum CoverageSelfTest {
    static func run() -> [(String, Bool)] {
        let model = AppModel()
        guard model.isSample else { return [("Coverage checks require sample mode", false)] }
        var checks: [(String, Bool)] = []
        for page in [SidebarItem.cleanup, .largeFiles, .duplicates, .applications] {
            model.coverageByPage[page] = ScanCoverage(excludedCount: 3)
            model.scanOutcomes[page] = ScanOutcome(cancelled: false, partial: false, excludedCount: 3)
            // Opaque wording proves presentation does not classify diagnostic strings.
            model.warnings[page] = ["opaque diagnostic"]
            checks.append(("\(page): expected exclusions have complete-with-exclusions presentation",
                model.scanSummary(for: page).contains("Completata con esclusioni")
                && model.coverageNotice(for: page).contains("esclusioni previste")
                && model.emptyScanMessage(for: page).title == "Analisi completata con esclusioni"))
        }
        checks.append(("Read errors override planned exclusions in coverage presentation",
            ScanOutcome(cancelled: false, partial: true, excludedCount: 3) == .partial))
        checks.append(("Cancellation stays distinct from completed policy exclusions",
            ScanOutcome(cancelled: true, partial: false, excludedCount: 3) == .interrupted))
        model.clearResults(.duplicates)
        checks.append(("Invalidating results clears their structured coverage",
            model.coverageByPage[.duplicates] == nil && model.scanOutcome(for: .duplicates) == nil))
        return checks
    }
}
